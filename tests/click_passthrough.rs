//! A click on the scene where it has nothing (no zone, no window) goes on to the program's
//! surface under it on the desktop (layer-shell's bottom level), and so do the wheel and the
//! pointer's moves: a clock or a timer on the desktop can be pressed (`screen::pointer_at`).
//! Where the scene has a zone, the zone keeps it; a zone that yields (`yields: true`) lets what
//! is under it take the press first.
//!
//! A screenless session (`pleamar-wm headless`) with `tests/click-passthrough.plm` (three
//! columns: a zone, a zone that yields, nothing), a widget of a program filling the desktop
//! under it, and a virtual pointer that moves, clicks and turns the wheel over each column.
//! Without a card to paint with, the session says so and the test passes without trying, like
//! the other tests of the session (`tests/virtual_pointer.rs`, `tests/layer_shell_v5.rs`).

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
    wl_pointer::{self, WlPointer},
    wl_registry::WlRegistry,
    wl_seat::WlSeat,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1};

/// What the widget on the desktop has been told by its pointer.
#[derive(Debug, Default)]
struct App {
    configured: Option<(u32, u32, u32)>,
    inside: bool,
    entered: u32,
    moves: u32,
    presses: u32,
    wheel: u32,
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
quiet!(WlCompositor, WlSurface, WlShm, WlShmPool, WlBuffer, WlSeat, ZwlrLayerShellV1, ZwlrVirtualPointerManagerV1, ZwlrVirtualPointerV1);
impl Dispatch<ZwlrLayerSurfaceV1, ()> for App {
    fn event(app: &mut Self, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, width, height } = event {
            layer.ack_configure(serial);
            app.configured = Some((serial, width, height));
        }
    }
}
impl Dispatch<WlPointer, ()> for App {
    fn event(app: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { .. } => {
                app.inside = true;
                app.entered += 1;
            }
            wl_pointer::Event::Leave { .. } => app.inside = false,
            wl_pointer::Event::Motion { .. } => app.moves += 1,
            wl_pointer::Event::Button { state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed), .. } => app.presses += 1,
            wl_pointer::Event::Axis { .. } => app.wheel += 1,
            _ => {}
        }
    }
}

/// What the scene's desk counted, the last time it said so: its own clicks, the ones given to
/// the zone that yields, and its wheel.
fn last_seen(record: &str) -> (f32, f32, f32) {
    record
        .lines()
        .rev()
        .find_map(|l| {
            let v: Vec<f32> = l.split('\t').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 4).then(|| (v[1], v[2], v[3]))
        })
        .unwrap_or_default()
}

/// Starts a screenless session on the desk (in a short path: a socket's name is 108 bytes at most).
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
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/click-passthrough.plm"))
        .args(["--seconds", "90", "--record", "mine,yielded,wheeled"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(Stdio::null());
    (c.spawn().expect("pleamar-wm runs"), run, out)
}

/// The socket programs connect to, as the session says it (once it has painted its first frame).
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

/// The widget: a layer surface on the bottom level, over the whole desktop, with a picture
/// (without one it is not drawn, and takes nothing).
fn widget(conn: &Connection, queue: &mut EventQueue<App>, app: &mut App, dir: &Path) -> (ZwlrLayerSurfaceV1, WlSurface, WlBuffer) {
    let (globals, _) = registry_queue_init::<App>(conn).unwrap();
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).expect("wl_compositor");
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).expect("zwlr_layer_shell_v1");
    let surface = compositor.create_surface(&qh, ());
    let layer = shell.get_layer_surface(&surface, None, Layer::Bottom, "test-widget".into(), &qh, ());
    layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
    layer.set_size(0, 0);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    surface.commit();
    let start = Instant::now();
    while app.configured.is_none() {
        queue.blocking_dispatch(app).unwrap();
        assert!(start.elapsed() < Duration::from_secs(60), "the widget was never configured");
    }
    let (_, w, h) = app.configured.unwrap();
    let (w, h) = (if w == 0 { 1920 } else { w as i32 }, if h == 0 { 1080 } else { h as i32 });
    let size = w * h * 4;
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(dir.join("pool")).unwrap();
    file.set_len(size as u64).unwrap();
    let pool = shm.create_pool(file.as_fd(), size, &qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w, h);
    surface.commit();
    queue.roundtrip(app).unwrap();
    (layer, surface, buffer)
}

#[test]
fn a_click_where_the_scene_has_nothing_reaches_the_widget_on_the_desktop_under_it() {
    let dir = std::env::temp_dir().join(format!("cp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut wm, run, record) = session(&dir);
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
        let _keep = widget(&conn, &mut queue, &mut app, &dir);
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).expect("wl_seat");
        let _pointer = seat.get_pointer(&qh, ());
        let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).expect("zwlr_virtual_pointer_manager_v1");
        let pointer = manager.create_virtual_pointer(None, &qh, ());
        let t = std::cell::Cell::new(100u32);
        let tick = || {
            t.set(t.get() + 20);
            t.get()
        };
        // (x, y) in thousandths of the desk; what was sent before is taken and answered with it.
        let go = |queue: &mut EventQueue<App>, app: &mut App, x: u32, y: u32| {
            pointer.motion_absolute(tick(), x, y, 1000, 1000);
            pointer.frame();
            queue.roundtrip(app).unwrap();
        };
        let press = || {
            pointer.button(tick(), 0x110, wl_pointer::ButtonState::Pressed);
            pointer.frame();
            pointer.button(tick(), 0x110, wl_pointer::ButtonState::Released);
            pointer.frame();
        };
        let wheel = || {
            pointer.axis_source(wl_pointer::AxisSource::Wheel);
            pointer.axis_discrete(tick(), wl_pointer::Axis::VerticalScroll, 15.0, 1);
            pointer.frame();
        };
        // The right column, where the scene has nothing: until the widget is drawn and the pointer enters it.
        let start = Instant::now();
        let mut jiggle = 0;
        while !app.inside {
            go(&mut queue, &mut app, 830 + jiggle % 2, 500);
            jiggle += 1;
            std::thread::sleep(Duration::from_millis(200));
            assert!(start.elapsed() < Duration::from_secs(60), "the pointer never entered the widget where the scene has nothing: {app:?}");
        }
        // What the session hands on comes from another thread, a little after it answers: waited
        // for, up to `limit` (a «nothing arrived» is the whole `limit` without it).
        let until = |queue: &mut EventQueue<App>, app: &mut App, limit: Duration, done: &dyn Fn(&App) -> bool| {
            let start = Instant::now();
            while !done(app) && start.elapsed() < limit {
                std::thread::sleep(Duration::from_millis(50));
                queue.roundtrip(app).unwrap();
            }
        };
        let long = Duration::from_secs(10);
        let moves = app.moves;
        go(&mut queue, &mut app, 850, 520);
        until(&mut queue, &mut app, long, &|a| a.moves > moves);
        let moved = app.moves > moves;
        press();
        wheel();
        until(&mut queue, &mut app, long, &|a| a.presses >= 1 && a.wheel >= 1);
        let right = (app.presses, app.wheel);
        // The middle column: a zone that yields. The widget under it takes the press.
        go(&mut queue, &mut app, 500, 500);
        press();
        until(&mut queue, &mut app, long, &|a| a.presses >= 2);
        let in_middle = app.inside;
        let middle = app.presses;
        // The left column: the scene's own zone keeps the press and the wheel; the widget is left.
        go(&mut queue, &mut app, 160, 500);
        until(&mut queue, &mut app, long, &|a| !a.inside);
        let in_left = app.inside;
        press();
        wheel();
        until(&mut queue, &mut app, Duration::from_secs(2), &|_| false);
        let left = (app.presses, app.wheel);
        Some((moved, right, in_middle, middle, in_left, left))
    });
    // Until the desk has counted what it was given (a frame can take seconds on a software card).
    let start = Instant::now();
    let mut text = String::new();
    if matches!(result, Ok(Some(_))) {
        while start.elapsed() < Duration::from_secs(60) {
            text = std::fs::read_to_string(&record).unwrap_or_default();
            let (mine, _, wheeled) = last_seen(&text);
            if mine >= 1.0 && wheeled != 0.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // Only this child, by its pid.
    let _ = wm.kill();
    let _ = wm.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let Some((moved, right, in_middle, middle, in_left, left)) = (match result {
        Err(e) => std::panic::resume_unwind(e),
        Ok(r) => r,
    }) else {
        return;
    };
    let (mine, yielded, wheeled) = last_seen(&text);
    assert!(moved, "the widget heard no move where the scene has nothing");
    assert_eq!(right, (1, 1), "where the scene has nothing, the widget takes the click and the wheel (presses, wheel)");
    assert!(in_middle, "over a zone that yields the pointer is the widget's");
    assert_eq!(middle, 2, "over a zone that yields, the widget takes the press first");
    assert!(!in_left, "over the scene's own zone the pointer is the scene's");
    assert_eq!(left, (2, 1), "over the scene's own zone, the widget hears nothing (presses, wheel)");
    assert_eq!(mine, 1.0, "the scene's own zone counted its click: {text}");
    assert_eq!(yielded, 0.0, "the zone that yields was given nothing (the widget took it): {text}");
    assert_eq!(wheeled, -1.0, "the scene's own zone counted its notch: {text}");
}
