//! `wlr-screencopy` says what changed: a program that asks for a picture of a monitor
//! with `copy_with_damage` is told, besides the picture, which part of it differs from
//! the one it was last given, not all of it every time (recorders, wayvnc and Marea's
//! lens copy what changed instead of the whole screen). A screenless session
//! (`pleamar-wm headless`) with `tests/screencopy-damage.plm`, a desk where only one box
//! changes, and a minimal client of the protocol in this file. Where nothing is known
//! (the first picture, a different piece, the pointer drawn in it) all of it is said.
//! Without a card to paint with, the session says so and the tests pass without trying.

use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_pointer, wl_registry::WlRegistry, wl_shm::{self, WlShm}, wl_shm_pool::WlShmPool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1::{self as frame, ZwlrScreencopyFrameV1}, zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1};

/// What the client heard of the frame it asked for.
#[derive(Default)]
struct App {
    /// (format, width, height, stride) of the buffer it wants.
    wants: Option<(u32, u32, u32, u32)>,
    buffer_done: bool,
    damage: Vec<[u32; 4]>,
    ready: bool,
    failed: bool,
}

macro_rules! quiet {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
quiet!(ZwlrScreencopyManagerV1, ZwlrVirtualPointerManagerV1, ZwlrVirtualPointerV1, WlOutput, WlShm, WlShmPool, WlBuffer);

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for App {
    fn event(app: &mut Self, _: &ZwlrScreencopyFrameV1, event: frame::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            frame::Event::Buffer { format, width, height, stride } => {
                if let WEnum::Value(f) = format {
                    if f == wl_shm::Format::Xrgb8888 || app.wants.is_none() {
                        app.wants = Some((f as u32, width, height, stride));
                    }
                }
            }
            frame::Event::BufferDone => app.buffer_done = true,
            frame::Event::Damage { x, y, width, height } => app.damage.push([x, y, width, height]),
            frame::Event::Ready { .. } => app.ready = true,
            frame::Event::Failed => app.failed = true,
            _ => {}
        }
    }
}

/// A screenless session on the desk (in a short path: a socket's name is 108 bytes at most).
fn session(dir: &Path) -> (std::process::Child, PathBuf, PathBuf) {
    let home = dir.join("home");
    let run = dir.join("run");
    for d in [home.join(".config"), home.join(".cache"), home.join(".local/share"), home.join(".local/state"), run.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    let out = dir.join("record.tsv");
    let mut c = Command::new(env!("CARGO_BIN_EXE_pleamar-wm"));
    c.env_clear();
    for keep in ["PATH", "LD_LIBRARY_PATH", "VK_ICD_FILENAMES", "VK_DRIVER_FILES", "LIBGL_ALWAYS_SOFTWARE", "WGPU_BACKEND", "WGPU_ADAPTER_NAME"] {
        if let Some(v) = std::env::var_os(keep) {
            c.env(keep, v);
        }
    }
    c.env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_RUNTIME_DIR", &run)
        .env("PLEAMAR_SOCKETS", run.join("pleamar"))
        .env("PLEAMAR_NO_SKILL_REFRESH", "1")
        .env("PLEAMAR_HEADLESS_AT", "999999")
        .env("PLEAMAR_HEADLESS_PNG", dir.join("shot.png"))
        .arg("headless")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/screencopy-damage.plm"))
        .args(["--seconds", "120"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(Stdio::null());
    (c.spawn().expect("pleamar-wm runs"), run, out)
}

/// The socket programs connect to, once the session has painted its first frame.
fn socket_of(record: &Path, run: &Path, limit: Duration) -> Option<PathBuf> {
    let start = Instant::now();
    while start.elapsed() < limit {
        let mut text = String::new();
        let _ = std::fs::File::open(record).and_then(|mut f| f.read_to_string(&mut text));
        if let Some(name) = text.lines().find_map(|l| l.split("WAYLAND_DISPLAY=").nth(1)).filter(|_| text.contains("first frame")) {
            let path = run.join(name.trim());
            if path.exists() {
                return Some(path);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

struct Client {
    queue: EventQueue<App>,
    app: App,
    manager: ZwlrScreencopyManagerV1,
    output: WlOutput,
    shm: WlShm,
    pointer: ZwlrVirtualPointerV1,
    dir: PathBuf,
}

impl Client {
    fn connect(socket: &Path, dir: &Path) -> Client {
        let conn = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
        let (globals, queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let manager: ZwlrScreencopyManagerV1 = globals.bind(&qh, 1..=3, ()).expect("zwlr_screencopy_manager_v1");
        let output: WlOutput = globals.bind(&qh, 1..=3, ()).expect("an output");
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
        let pointers: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).expect("a virtual pointer, to press the desk");
        let pointer = pointers.create_virtual_pointer(None, &qh, ());
        Client { queue, app: App::default(), manager, output, shm, pointer, dir: dir.to_owned() }
    }

    /// A picture of the monitor (all of it), with or without the pointer in it, asked with
    /// `copy` or `copy_with_damage`. `between` runs after it was asked and before it can be
    /// ready (a change for `copy_with_damage` to wait for). Gives the damage it said, and how big the picture is.
    fn picture(&mut self, with_damage: bool, cursor: bool, between: impl FnOnce(&mut Client)) -> (Vec<[u32; 4]>, (u32, u32), Vec<u8>) {
        let qh = self.queue.handle();
        self.app = App::default();
        let frame = self.manager.capture_output(cursor as i32, &self.output, &qh, ());
        let start = Instant::now();
        while !self.app.buffer_done {
            self.queue.roundtrip(&mut self.app).unwrap();
            assert!(start.elapsed() < Duration::from_secs(60), "the frame never said what buffer it wants");
        }
        let (format, w, h, stride) = self.app.wants.expect("a buffer to give");
        let size = (stride * h) as usize;
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(self.dir.join("pool")).unwrap();
        file.set_len(size as u64).unwrap();
        let pool = self.shm.create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, wl_shm::Format::try_from(format).unwrap(), &qh, ());
        if with_damage {
            frame.copy_with_damage(&buffer);
        } else {
            frame.copy(&buffer);
        }
        self.queue.roundtrip(&mut self.app).unwrap();
        between(self);
        let start = Instant::now();
        while !self.app.ready && !self.app.failed {
            self.queue.roundtrip(&mut self.app).unwrap();
            assert!(start.elapsed() < Duration::from_secs(120), "the picture never came");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!self.app.failed, "the picture failed");
        let damage = std::mem::take(&mut self.app.damage);
        let pixels = std::fs::read(self.dir.join("pool")).unwrap();
        frame.destroy();
        buffer.destroy();
        pool.destroy();
        (damage, (w, h), pixels)
    }

    /// A click on the desk, which changes the box.
    fn press(&mut self) {
        self.pointer.motion_absolute(1, 500, 500, 1000, 1000);
        self.pointer.frame();
        self.pointer.button(2, 0x110, wl_pointer::ButtonState::Pressed);
        self.pointer.frame();
        self.pointer.button(3, 0x110, wl_pointer::ButtonState::Released);
        self.pointer.frame();
        self.queue.roundtrip(&mut self.app).unwrap();
    }
}

/// Runs `body` with a client of a session of its own, and kills the session by its pid.
fn with_client(name: &str, body: impl FnOnce(&mut Client) + std::panic::UnwindSafe) {
    let dir = std::env::temp_dir().join(format!("sc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut wm, run, record) = session(&dir);
    let dir2 = dir.clone();
    let result = std::panic::catch_unwind(move || {
        let Some(socket) = socket_of(&record, &run, Duration::from_secs(60)) else {
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            assert!(text.contains("no graphics driver"), "the session never listened: {text}");
            eprintln!("no card to paint with: not tried");
            return;
        };
        let mut client = Client::connect(&socket, &dir2);
        body(&mut client);
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

/// The box that changes, on the monitor.
const BOX: [u32; 4] = [400, 300, 200, 120];

#[test]
fn a_picture_says_only_what_changed_since_the_last_one() {
    with_client("a", |c| {
        // The first picture: nothing before it, all of it is new.
        let (first, (w, h), mut before) = c.picture(false, false, |_| {});
        assert!(first.is_empty() || first == [[0, 0, w, h]], "a plain copy says nothing or all: {first:?}");
        // The scene's first frames paint all of it (it makes the canvas it then repaints in pieces):
        // a few changes first, each picture waiting for the desk to change.
        let mut said = Vec::new();
        for _ in 0..4 {
            let (damage, _, pixels) = c.picture(true, false, |c| c.press());
            // What it says holds every pixel that really differs from the picture before.
            let differing = pixels.chunks_exact(4).zip(before.chunks_exact(4)).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| ((i as u32 % w), (i as u32 / w)));
            let outside: Vec<(u32, u32)> = differing.filter(|(x, y)| !damage.iter().any(|d| *x >= d[0] && *x < d[0] + d[2] && *y >= d[1] && *y < d[1] + d[3])).take(3).collect();
            assert!(outside.is_empty(), "pixels that changed and are not in the damage {damage:?}: {outside:?}");
            before = pixels;
            said.push(damage);
        }
        // Then only the box that changes, each time.
        for later in &said[2..] {
            assert_eq!(later.len(), 1, "one place: {said:?}");
            let [x, y, dw, dh] = later[0];
            let inside = x <= BOX[0] && y <= BOX[1] && x + dw >= BOX[0] + BOX[2] && y + dh >= BOX[1] + BOX[3];
            assert!(inside, "it has to hold the box that changed {BOX:?}: {said:?}");
            assert!(dw as u64 * dh as u64 * 20 <= w as u64 * h as u64, "much less than the whole screen ({w}×{h}): {said:?}");
        }
        assert_eq!(said[2], said[3], "the same box changed again: {said:?}");
    });
}

#[test]
fn with_the_pointer_in_the_picture_or_nothing_known_it_says_all_of_it() {
    with_client("b", |c| {
        // A copy with the pointer drawn in: its moving is not in what the monitor knows.
        let (_, (w, h), _) = c.picture(false, true, |_| {});
        let (all, _, _) = c.picture(true, true, |c| c.press());
        assert_eq!(all, vec![[0, 0, w, h]], "with the pointer in it, all of it");
    });
}
