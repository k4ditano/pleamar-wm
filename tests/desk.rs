//! `--features exp`: a scene's desk slots (`windows … desk N`). A program's widget on the desktop
//! (layer-shell's bottom level) goes to the scene as a window's frames do, and the scene draws it
//! in its own picture; it still takes the presses where the scene yields and its input region
//! says, and nothing while the session is locked.
//!
//! A screenless session (`pleamar-wm headless`) with `tests/desk.plm`, which draws its slots 400
//! to the right of where the widgets put themselves; a red widget of 200 × 100 at 200, 100 whose
//! input region is its left half; a virtual pointer; and a lock taken with ext-session-lock.
//! Without a card to paint with, the session says so and the test passes without trying.
#![cfg(feature = "exp")]

use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_compositor::WlCompositor,
    wl_output::WlOutput,
    wl_pointer::{self, WlPointer},
    wl_region::WlRegion,
    wl_registry::WlRegistry,
    wl_seat::WlSeat,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1};

#[derive(Debug, Default)]
struct App {
    configured: Option<(u32, u32)>,
    presses: u32,
    lock_configured: Option<(u32, u32, u32)>,
    locked: bool,
}

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
macro_rules! quiet {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
quiet!(WlCompositor, WlSurface, WlShm, WlShmPool, WlBuffer, WlSeat, WlOutput, WlRegion, ZwlrLayerShellV1, ZwlrVirtualPointerManagerV1, ZwlrVirtualPointerV1, ExtSessionLockManagerV1);
impl Dispatch<ZwlrLayerSurfaceV1, ()> for App {
    fn event(app: &mut Self, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, width, height } = event {
            layer.ack_configure(serial);
            app.configured = Some((width, height));
        }
    }
}
impl Dispatch<WlPointer, ()> for App {
    fn event(app: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Button { state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } = event {
            app.presses += 1;
        }
    }
}
impl Dispatch<ExtSessionLockV1, ()> for App {
    fn event(app: &mut Self, _: &ExtSessionLockV1, event: ext_session_lock_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_session_lock_v1::Event::Locked = event {
            app.locked = true;
        }
    }
}
impl Dispatch<ExtSessionLockSurfaceV1, ()> for App {
    fn event(app: &mut Self, s: &ExtSessionLockSurfaceV1, event: ext_session_lock_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, width, height } = event {
            s.ack_configure(serial);
            app.lock_configured = Some((serial, width, height));
        }
    }
}

/// The scene's record: lefts, win.2.open, x, y, width (the last line that says them all).
fn last_seen(record: &str) -> [f32; 5] {
    record
        .lines()
        .rev()
        .find_map(|l| {
            let v: Vec<f32> = l.split('\t').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 6).then(|| [v[1], v[2], v[3], v[4], v[5]])
        })
        .unwrap_or_default()
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
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/desk.plm"))
        .args(["--seconds", "90", "--record", "lefts,win.2.open,win.2.x,win.2.y,win.2.width"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(Stdio::null());
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

/// A buffer of one colour (XRGB), w × h, from a file of its own.
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

/// The colour of the session's picture at a point (the picture `PLEAMAR_HEADLESS_AT` wrote).
fn pixel(png_path: &Path, (x, y): (u32, u32)) -> Option<[u8; 3]> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(png_path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let n = info.color_type.samples();
    let at = (y as usize * info.width as usize + x as usize) * n;
    Some([buf[at], buf[at + 1], buf[at + 2]])
}

#[test]
fn a_widget_on_the_desktop_is_drawn_by_the_scene_and_still_takes_its_presses() {
    let dir = std::env::temp_dir().join(format!("desk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut wm, run, record) = session(&dir, "999999");
    let result = std::panic::catch_unwind(|| {
        let Some(socket) = socket_of(&record, &run, Duration::from_secs(60)) else {
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            assert!(text.contains("no graphics driver"), "the session never listened: {text}");
            eprintln!("no card to paint with: not tried");
            return None;
        };
        let conn = Connection::from_socket(UnixStream::connect(&socket).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let mut app = App::default();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).expect("wl_compositor");
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
        let shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).expect("zwlr_layer_shell_v1");
        let output: WlOutput = globals.bind(&qh, 1..=3, ()).expect("wl_output");
        // The widget: red, 200 × 100, at 200, 100 on the desktop's level; it takes the pointer on its left half.
        let surface = compositor.create_surface(&qh, ());
        let layer = shell.get_layer_surface(&surface, None, Layer::Bottom, "test-desk-widget".into(), &qh, ());
        layer.set_anchor(Anchor::Top | Anchor::Left);
        layer.set_size(200, 100);
        layer.set_margin(100, 0, 0, 200);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        surface.commit();
        let start = Instant::now();
        while app.configured.is_none() {
            queue.blocking_dispatch(&mut app).unwrap();
            assert!(start.elapsed() < Duration::from_secs(60), "the widget was never configured");
        }
        let region = compositor.create_region(&qh, ());
        region.add(0, 0, 100, 100);
        surface.set_input_region(Some(&region));
        surface.attach(Some(&buffer(&shm, &qh, &dir, "red", (200, 100), 0x00e02020)), 0, 0);
        surface.damage_buffer(0, 0, 200, 100);
        surface.commit();
        queue.roundtrip(&mut app).unwrap();
        // The scene has it in its first desk slot, where the widget put itself.
        let start = Instant::now();
        loop {
            let seen = last_seen(&std::fs::read_to_string(&record).unwrap_or_default());
            if seen[1] == 1.0 && seen[2] == 200.0 && seen[3] == 100.0 && seen[4] == 200.0 {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(60), "the scene never had the widget in its desk slot: {seen:?}");
            std::thread::sleep(Duration::from_millis(200));
        }
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).expect("wl_seat");
        let _pointer = seat.get_pointer(&qh, ());
        let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).expect("zwlr_virtual_pointer_manager_v1");
        let vp = manager.create_virtual_pointer(None, &qh, ());
        let t = std::cell::Cell::new(100u32);
        let tick = || {
            t.set(t.get() + 20);
            t.get()
        };
        // (x, y) in pixels of the 1920 × 1080 desk.
        let click = |queue: &mut EventQueue<App>, app: &mut App, x: u32, y: u32| {
            vp.motion_absolute(tick(), x, y, 1920, 1080);
            vp.frame();
            queue.roundtrip(app).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            vp.button(tick(), 0x110, wl_pointer::ButtonState::Pressed);
            vp.frame();
            vp.button(tick(), 0x110, wl_pointer::ButtonState::Released);
            vp.frame();
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(1500) {
                std::thread::sleep(Duration::from_millis(50));
                queue.roundtrip(app).unwrap();
            }
        };
        // Where its input region is (the left half of where it put itself): its press.
        click(&mut queue, &mut app, 250, 150);
        let inside = app.presses;
        // On its right half, outside its region: the scene's, through the zone that yields.
        click(&mut queue, &mut app, 350, 150);
        let outside = app.presses;
        let lefts = last_seen(&std::fs::read_to_string(&record).unwrap_or_default())[0];
        // Locked: nothing reaches it.
        let locks: ExtSessionLockManagerV1 = globals.bind(&qh, 1..=1, ()).expect("ext_session_lock_manager_v1");
        let lock = locks.lock(&qh, ());
        let lock_surface = compositor.create_surface(&qh, ());
        let ls = lock.get_lock_surface(&lock_surface, &output, &qh, ());
        let start = Instant::now();
        while app.lock_configured.is_none() {
            queue.blocking_dispatch(&mut app).unwrap();
            assert!(start.elapsed() < Duration::from_secs(30), "the lock surface was never configured");
        }
        let (_, lw, lh) = app.lock_configured.unwrap();
        lock_surface.attach(Some(&buffer(&shm, &qh, &dir, "black", (lw as i32, lh as i32), 0x00000000)), 0, 0);
        lock_surface.damage_buffer(0, 0, lw as i32, lh as i32);
        lock_surface.commit();
        let start = Instant::now();
        while !app.locked {
            queue.roundtrip(&mut app).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            assert!(start.elapsed() < Duration::from_secs(30), "the session never said it was locked");
        }
        click(&mut queue, &mut app, 250, 150);
        let locked = app.presses;
        lock.unlock_and_destroy();
        ls.destroy();
        queue.roundtrip(&mut app).unwrap();
        drop(layer);
        Some((inside, outside, lefts, locked))
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let text = std::fs::read_to_string(&record).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    let Some((inside, outside, lefts, locked)) = (match result {
        Err(e) => std::panic::resume_unwind(e),
        Ok(r) => r,
    }) else {
        return;
    };
    assert!(text.contains("the scene draws the desktop's widgets itself"), "pleamar-wm did not take the scene's desk: {text}");
    assert_eq!(inside, 1, "a press on the widget's input region reaches it");
    assert_eq!(outside, 1, "a press outside its region does not");
    assert_eq!(lefts, 1.0, "that one is the scene's, through the zone that yields");
    assert_eq!(locked, 1, "nothing reaches it while the session is locked");
}

/// The scene draws the widget, not pleamar-wm: red 400 to the right of where it put itself,
/// and the scene's own blue where it put itself.
#[test]
fn the_scene_draws_the_widget_where_it_says() {
    let dir = std::env::temp_dir().join(format!("desk-shot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Its picture at 30 s: the widget is in its slot long before (the first frame takes seconds).
    let (mut wm, run, record) = session(&dir, "30");
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
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).expect("wl_compositor");
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
        let shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).expect("zwlr_layer_shell_v1");
        let surface = compositor.create_surface(&qh, ());
        let layer = shell.get_layer_surface(&surface, None, Layer::Bottom, "test-desk-widget".into(), &qh, ());
        layer.set_anchor(Anchor::Top | Anchor::Left);
        layer.set_size(200, 100);
        layer.set_margin(100, 0, 0, 200);
        surface.commit();
        let start = Instant::now();
        while app.configured.is_none() {
            queue.blocking_dispatch(&mut app).unwrap();
            assert!(start.elapsed() < Duration::from_secs(60), "the widget was never configured");
        }
        surface.attach(Some(&buffer(&shm, &qh, &dir, "red", (200, 100), 0x00e02020)), 0, 0);
        surface.damage_buffer(0, 0, 200, 100);
        surface.commit();
        queue.roundtrip(&mut app).unwrap();
        let start = Instant::now();
        while last_seen(&std::fs::read_to_string(&record).unwrap_or_default())[1] != 1.0 {
            assert!(start.elapsed() < Duration::from_secs(60), "the scene never had the widget");
            std::thread::sleep(Duration::from_millis(200));
        }
        // The picture the session writes at 30 s.
        let start = Instant::now();
        while !dir.join("shot.png").exists() {
            assert!(start.elapsed() < Duration::from_secs(60), "the session wrote no picture");
            std::thread::sleep(Duration::from_millis(300));
        }
        std::thread::sleep(Duration::from_secs(1));
        drop(layer);
        Some(true)
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let shot = dir.join("shot.png");
    let drawn = pixel(&shot, (650, 150));
    let under = pixel(&shot, (250, 150));
    let _ = std::fs::remove_dir_all(&dir);
    let Some(took) = (match result {
        Err(e) => std::panic::resume_unwind(e),
        Ok(r) => r,
    }) else {
        return;
    };
    assert!(took, "the session's picture could not be taken");
    let drawn = drawn.expect("a picture");
    let under = under.expect("a picture");
    assert!(drawn[0] > 180 && drawn[1] < 80 && drawn[2] < 80, "red where the scene draws it (650, 150): {drawn:?}");
    assert!(under[0] < 60 && under[2] > 30, "the scene's blue where the widget put itself (250, 150), not the widget: {under:?}");
}
