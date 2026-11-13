//! `wlr-virtual-pointer`, open to every program: a client that binds the global
//! moves the pointer, clicks and turns the wheel, and the scene hears it as it
//! would a mouse's (the same road: `route.rs`). A screenless session
//! (`pleamar-wm headless`) with `tests/virtual-pointer.plm`, whose desk is one zone
//! that counts what reaches it, and a minimal client of the protocol in this file.
//! Without a card to paint with, the session says so and the tests pass without trying, like pleamar's own film tests.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output::WlOutput, wl_pointer, wl_registry::WlRegistry};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1};

struct App;
impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for App {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerManagerV1, _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrVirtualPointerV1, ()> for App {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerV1, _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlOutput, ()> for App {
    fn event(_: &mut Self, _: &WlOutput, _: <WlOutput as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// What the scene's desk counted, the last time it said so.
#[derive(Debug, Default, PartialEq)]
struct Seen {
    x: f32,
    y: f32,
    clicks: f32,
    rights: f32,
    wheeled: f32,
    sideways: f32,
}

fn last_seen(record: &str) -> Seen {
    let row = record.lines().rev().find_map(|l| {
        let v: Vec<f32> = l.split('\t').filter_map(|x| x.parse().ok()).collect();
        (v.len() == 7).then_some(v)
    });
    let v = row.unwrap_or_else(|| vec![0.0; 7]);
    Seen { x: v[1], y: v[2], clicks: v[3], rights: v[4], wheeled: v[5], sideways: v[6] }
}

/// Starts a screenless session on the desk (in a short path: a socket's name is 108 bytes at most) and gives the client's socket and the record file.
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
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/virtual-pointer.plm"))
        .args(["--seconds", "60", "--record", "pointer.x,pointer.y,clicks,rights,wheeled,sideways"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(Stdio::null());
    (c.spawn().expect("pleamar-wm runs"), run, out)
}

/// The socket programs connect to, as the session says it.
fn socket_of(record: &Path, run: &Path, limit: Duration) -> Option<PathBuf> {
    let start = Instant::now();
    while start.elapsed() < limit {
        let mut text = String::new();
        let _ = std::fs::File::open(record).and_then(|mut f| f.read_to_string(&mut text));
        // (Once it has painted its first frame: before that nothing is listening for input.)
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

/// The record as it is once `done` holds of what the desk counted (a frame can take seconds
/// on a software card, in debug: it is waited for, not guessed).
fn wait_for(record: &Path, done: impl Fn(&Seen) -> bool) -> String {
    let start = Instant::now();
    loop {
        let text = std::fs::read_to_string(record).unwrap_or_default();
        if done(&last_seen(&text)) || start.elapsed() > Duration::from_secs(120) {
            return text;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_program_with_a_virtual_pointer_moves_it_clicks_and_turns_the_wheel() {
    let dir = std::env::temp_dir().join(format!("vp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut wm, run, record) = session(&dir);
    let result = std::panic::catch_unwind(|| {
        let Some(socket) = socket_of(&record, &run, Duration::from_secs(60)) else {
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            assert!(text.contains("no graphics driver"), "the session never listened: {text}");
            eprintln!("no card to paint with: not tried");
            return;
        };
        let conn = Connection::from_socket(UnixStream::connect(&socket).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        // Open to every program: no filter, no permission, only asking.
        let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).expect("the compositor advertises zwlr_virtual_pointer_manager_v1");
        let output: WlOutput = globals.bind(&qh, 1..=3, ()).expect("an output to make a pointer for");
        let pointer = manager.create_virtual_pointer(None, &qh, ());
        let mut app = App;
        let mut t = 100u32;
        let mut tick = || {
            t += 20;
            t
        };
        // What the compositor has been sent, it has taken (and handed on) when it answers.
        let settle = |queue: &mut wayland_client::EventQueue<App>, app: &mut App| {
            queue.roundtrip(app).unwrap();
        };
        // Absolute: the middle of the desk (1920 × 1080), as a fraction of any extent.
        pointer.motion_absolute(tick(), 500, 500, 1000, 1000);
        pointer.frame();
        settle(&mut queue, &mut app);
        // Left click, right click.
        pointer.button(tick(), 0x110, wl_pointer::ButtonState::Pressed);
        pointer.frame();
        pointer.button(tick(), 0x110, wl_pointer::ButtonState::Released);
        pointer.frame();
        settle(&mut queue, &mut app);
        pointer.button(tick(), 0x111, wl_pointer::ButtonState::Pressed);
        pointer.frame();
        pointer.button(tick(), 0x111, wl_pointer::ButtonState::Released);
        pointer.frame();
        settle(&mut queue, &mut app);
        // Relative: 100 right and 20 up, from where it is.
        pointer.motion(tick(), 100.0, -20.0);
        pointer.frame();
        settle(&mut queue, &mut app);
        // A wheel, one notch down and one to the right, as a mouse's would say them.
        pointer.axis_source(wl_pointer::AxisSource::Wheel);
        pointer.axis_discrete(tick(), wl_pointer::Axis::VerticalScroll, 15.0, 1);
        pointer.axis_discrete(tick(), wl_pointer::Axis::HorizontalScroll, 15.0, 1);
        pointer.frame();
        settle(&mut queue, &mut app);
        // A pointer made for an output goes over that output (the only one here: the whole desk).
        let ours = manager.create_virtual_pointer_with_output(None, Some(&output), &qh, ());
        ours.motion_absolute(tick(), 250, 750, 1000, 1000);
        ours.frame();
        settle(&mut queue, &mut app);
        ours.button(tick(), 0x110, wl_pointer::ButtonState::Pressed);
        ours.frame();
        ours.button(tick(), 0x110, wl_pointer::ButtonState::Released);
        ours.frame();
        settle(&mut queue, &mut app);
        // Until the desk has painted what it was told (a frame can take seconds on a software card).
        wait_for(&record, |s| s.clicks >= 2.0 && s.rights >= 1.0 && s.wheeled <= -2.0 && (s.x.round(), s.y.round()) == (480.0, 810.0));
    });
    // Only this child, by its pid.
    let _ = wm.kill();
    let _ = wm.wait();
    let text = std::fs::read_to_string(&record).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
    if text.contains("no graphics driver") {
        return;
    }
    let seen = last_seen(&text);
    // Where it ended: 25 % and 75 % of the desk. And what it counted on the way.
    assert_eq!((seen.x.round(), seen.y.round()), (480.0, 810.0), "pointer: {seen:?}");
    assert_eq!(seen.clicks, 2.0, "left clicks (one from each pointer): {seen:?}");
    assert_eq!(seen.rights, 1.0, "right clicks: {seen:?}");
    // A notch down and one to the right, which the desk adds up in `wheel`: the session takes the
    // sideways one to the scene as a mouse's, and `wheel.x` is for the scene's own `WheelX`.
    assert_eq!(seen.wheeled, -2.0, "a notch down and one to the right: {seen:?}");
}

#[test]
fn the_wheel_waits_for_its_frame_and_a_relative_move_is_from_where_the_pointer_is() {
    let dir = std::env::temp_dir().join(format!("vp-b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut wm, run, record) = session(&dir);
    let result = std::panic::catch_unwind(|| {
        let Some(socket) = socket_of(&record, &run, Duration::from_secs(60)) else {
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            assert!(text.contains("no graphics driver"), "the session never listened: {text}");
            eprintln!("no card to paint with: not tried");
            return;
        };
        let conn = Connection::from_socket(UnixStream::connect(&socket).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).unwrap();
        let pointer = manager.create_virtual_pointer(None, &qh, ());
        let mut app = App;
        pointer.motion_absolute(1, 0, 0, 1000, 1000);
        pointer.frame();
        queue.roundtrip(&mut app).unwrap();
        // Moves add up from where it is; it stays on the desk (the corner and the other way off it).
        pointer.motion(2, 30.0, 40.0);
        pointer.motion(3, 30.0, 40.0);
        pointer.motion(4, -500.0, -500.0);
        pointer.motion(5, 7.0, 9.0);
        pointer.frame();
        // A wheel with no `frame` after it is not heard yet.
        pointer.axis_source(wl_pointer::AxisSource::Wheel);
        pointer.axis_discrete(6, wl_pointer::Axis::VerticalScroll, -15.0, -2);
        queue.roundtrip(&mut app).unwrap();
        // A mark: the pointer moving once more, which the desk shows as soon as it paints.
        pointer.motion(7, 1.0, 1.0);
        queue.roundtrip(&mut app).unwrap();
        let text = wait_for(&record, |s| (s.x.round(), s.y.round()) == (8.0, 10.0));
        let before = last_seen(&text);
        assert_eq!((before.x.round(), before.y.round()), (8.0, 10.0), "from the corner: 60,80 → 0,0 → 7,9 → 8,10: {before:?}");
        assert_eq!(before.wheeled, 0.0, "the wheel was heard before its frame: {before:?}");
        pointer.frame();
        queue.roundtrip(&mut app).unwrap();
        let after = last_seen(&wait_for(&record, |s| s.wheeled != 0.0));
        assert_eq!(after.wheeled, 2.0, "two notches up: {after:?}");
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let text = std::fs::read_to_string(&record).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
    let _ = text;
}
