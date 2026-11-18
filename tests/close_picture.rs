//! A window that goes keeps its last picture for the scene to see it leave. A program that closes outright
//! already did; one that first takes its picture away (a null buffer, as GTK, Qt and Firefox do when they hide
//! or close a window) sent the scene an empty window before it closed, so the scene had nothing to animate.
//! Now that is told as the window closing (`open` 0) and its last picture stays; if it shows again, it opens
//! again with its new picture. A screenless session and a minimal client of the protocol.

use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};

#[derive(Default)]
struct App {
    configured: bool,
}
impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
macro_rules! silent {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
silent!(WlCompositor, WlSurface, WlShm, WlShmPool, WlBuffer, XdgToplevel);
impl Dispatch<XdgWmBase, ()> for App {
    fn event(_: &mut Self, base: &XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}
impl Dispatch<XdgSurface, ()> for App {
    fn event(app: &mut Self, surface: &XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            app.configured = true;
        }
    }
}

fn session(dir: &Path, at: &str) -> (std::process::Child, PathBuf, PathBuf) {
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
        .env("PLEAMAR_HEADLESS_AT", at)
        .env("PLEAMAR_HEADLESS_PNG", dir.join("shot.png"))
        .arg("headless")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/close-picture.plm"))
        .args(["--seconds", "90", "--record", "win.0.open"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(if std::env::var_os("CLOSE_KEEP").is_some() { Stdio::from(std::fs::File::create(dir.join("session.log")).unwrap()) } else { Stdio::null() });
    (c.spawn().expect("pleamar-wm runs"), run, out)
}

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

/// `win.0.open` as the record last says it (-1: not yet).
fn open_now(record: &Path) -> f32 {
    std::fs::read_to_string(record)
        .unwrap_or_default()
        .lines()
        .rev()
        .find_map(|l| {
            let v: Vec<f32> = l.split('\t').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 2).then(|| v[1])
        })
        .unwrap_or(-1.0)
}

fn wait_open(record: &Path, want: f32, what: &str) {
    let start = Instant::now();
    while open_now(record) != want {
        assert!(start.elapsed() < Duration::from_secs(60), "{what}: win.0.open never became {want} (it is {})", open_now(record));
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn buffer(shm: &WlShm, qh: &QueueHandle<App>, dir: &Path, name: &str, (w, h): (i32, i32), rgb: u32) -> WlBuffer {
    let size = (w * h * 4) as usize;
    let mut bytes = Vec::with_capacity(size);
    for _ in 0..w * h {
        bytes.extend_from_slice(&rgb.to_le_bytes());
    }
    let path = dir.join(name);
    std::fs::write(&path, &bytes).unwrap();
    let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, qh, ())
}

fn pixel(png_path: &Path, (x, y): (u32, u32)) -> Option<[u8; 3]> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(png_path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let n = info.color_type.samples();
    let at = (y as usize * info.width as usize + x as usize) * n;
    Some([buf[at], buf[at + 1], buf[at + 2]])
}

/// How the program leaves, and what the picture shows afterwards (at 250, 200: the window's middle).
#[derive(Clone, Copy)]
enum Leave {
    /// A null buffer, then nothing more (still connected): hidden, as GTK does.
    Unmap,
    /// A null buffer, then a green one: shown again.
    UnmapThenGreen,
    /// It closes outright (its surfaces destroyed, disconnected).
    Destroy,
}

fn run(name: &str, leave: Leave) -> Option<([u8; 3], f32)> {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The picture at 40 s: the window has come and gone long before (the first frame takes seconds).
    let (mut wm, run, record) = session(&dir, "40");
    let result = std::panic::catch_unwind(|| {
        let Some(socket) = socket_of(&record, &run, Duration::from_secs(60)) else {
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            assert!(text.contains("no graphics driver"), "the session never listened: {text}");
            return None;
        };
        let conn = Connection::from_socket(UnixStream::connect(&socket).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let mut app = App::default();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let base: XdgWmBase = globals.bind(&qh, 1..=2, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = base.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_title("close-picture".into());
        surface.commit();
        while !app.configured {
            queue.blocking_dispatch(&mut app).unwrap();
        }
        surface.attach(Some(&buffer(&shm, &qh, &dir, "red", (300, 200), 0x00e02020)), 0, 0);
        surface.damage_buffer(0, 0, 300, 200);
        surface.commit();
        queue.roundtrip(&mut app).unwrap();
        wait_open(&record, 1.0, "the red window");
        std::thread::sleep(Duration::from_secs(2));
        match leave {
            Leave::Unmap | Leave::UnmapThenGreen => {
                surface.attach(None, 0, 0);
                surface.commit();
                queue.roundtrip(&mut app).unwrap();
                wait_open(&record, 0.0, "hidden with a null buffer");
                if matches!(leave, Leave::UnmapThenGreen) {
                    std::thread::sleep(Duration::from_secs(1));
                    // Shown again: as a toolkit does, it commits, waits for its configure, then draws.
                    app.configured = false;
                    surface.commit();
                    while !app.configured {
                        queue.blocking_dispatch(&mut app).unwrap();
                    }
                    surface.attach(Some(&buffer(&shm, &qh, &dir, "green", (300, 200), 0x0020c040)), 0, 0);
                    surface.damage_buffer(0, 0, 300, 200);
                    surface.commit();
                    queue.roundtrip(&mut app).unwrap();
                    wait_open(&record, 1.0, "shown again");
                }
            }
            Leave::Destroy => {
                top.destroy();
                xdg.destroy();
                surface.destroy();
                queue.roundtrip(&mut app).unwrap();
                drop(conn);
                wait_open(&record, 0.0, "closed outright");
            }
        }
        let start = Instant::now();
        while !dir.join("shot.png").exists() {
            assert!(start.elapsed() < Duration::from_secs(60), "the session wrote no picture");
            std::thread::sleep(Duration::from_millis(300));
        }
        std::thread::sleep(Duration::from_secs(1));
        Some(open_now(&record))
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let shot = pixel(&dir.join("shot.png"), (250, 200));
    if std::env::var_os("CLOSE_KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let open = match result {
        Err(e) => std::panic::resume_unwind(e),
        Ok(r) => r?,
    };
    Some((shot.expect("a picture"), open))
}

/// GTK's way: a null buffer before it closes. The scene is told it closed, and its last picture (red) stays
/// for the scene to see it leave; before, the scene got an empty window (its background showed there).
#[test]
fn a_window_that_takes_its_picture_away_closes_with_its_last_picture() {
    let Some((px, open)) = run("close-unmap", Leave::Unmap) else { return };
    assert_eq!(open, 0.0, "told as closed");
    assert!(px[0] > 180 && px[1] < 80 && px[2] < 80, "its last picture (red) is still there to see it leave: {px:?}");
}

/// Hidden and shown again: it opens again, with its new picture.
#[test]
fn a_window_hidden_and_shown_again_opens_again_with_its_new_picture() {
    let Some((px, open)) = run("close-remap", Leave::UnmapThenGreen) else { return };
    assert_eq!(open, 1.0, "open again");
    assert!(px[1] > 150 && px[0] < 80, "its new picture (green): {px:?}");
}

/// Closed outright: as it always was, closed and its last picture kept.
#[test]
fn a_window_closed_outright_keeps_its_last_picture_as_before() {
    let Some((px, open)) = run("close-destroy", Leave::Destroy) else { return };
    assert_eq!(open, 0.0);
    assert!(px[0] > 180 && px[1] < 80 && px[2] < 80, "its last picture (red): {px:?}");
}
