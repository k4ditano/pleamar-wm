//! A program's own «maximize» and «restore» (pleamar 0.12, `maximized:` on the scene's `window`): the scene
//! is told (`window_maximize`) only when the program asks for what it is not, and the program is told it is
//! maximized in its configure when the scene has it so. Before, every request was told as the same toggle
//! and no program was ever told it was maximized, so its button always said «maximize». A screenless
//! session (`pleamar-wm headless`) with `tests/maximize.plm`, which maximizes window 0 as a desktop would,
//! and a minimal xdg-shell client in this file. Only with a pleamar beside it that has `maximized:` (0.8.0-exp.51
//! on, `pleamar_maximized`); without a card to paint with the session says so and the test passes without trying.
#![cfg(pleamar_maximized)]

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

/// What the compositor last said to the window: whether maximized, and the size it gave.
#[derive(Default)]
struct App {
    maximized: Option<bool>,
    size: (i32, i32),
}
impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlCompositor, ()> for App {
    fn event(_: &mut Self, _: &WlCompositor, _: <WlCompositor as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSurface, ()> for App {
    fn event(_: &mut Self, _: &WlSurface, _: <WlSurface as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<XdgWmBase, ()> for App {
    fn event(_: &mut Self, base: &XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}
impl Dispatch<XdgSurface, ()> for App {
    fn event(_: &mut Self, surface: &XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
        }
    }
}
impl Dispatch<XdgToplevel, ()> for App {
    fn event(app: &mut Self, _: &XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_toplevel::Event::Configure { width, height, states } = event {
            let states: Vec<u32> = states.chunks_exact(4).map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect();
            app.maximized = Some(states.contains(&(xdg_toplevel::State::Maximized as u32)));
            app.size = (width, height);
        }
    }
}

/// Starts a screenless session (in a short path: a socket's name is 108 bytes at most) and gives
/// the client's socket, the record file and the file the session's own messages go to.
fn session(dir: &Path) -> (std::process::Child, PathBuf, PathBuf, PathBuf) {
    let home = dir.join("home");
    let run = dir.join("run");
    for d in [home.join(".config"), home.join(".cache"), home.join(".local/share"), home.join(".local/state"), run.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    let out = dir.join("record.tsv");
    let log = dir.join("session.log");
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
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/maximize.plm"))
        .args(["--seconds", "60", "--record", "big"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&log).unwrap());
    (c.spawn().expect("pleamar-wm runs"), run, out, log)
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

/// Runs `body` against a session, or says there is no card and passes. The session is stopped
/// (this child only, by its pid) and its directory removed, whatever `body` does.
/// `body` gets the session's log and what it printed (its record, with the `windows ·` lines).
fn with_session(name: &str, body: impl FnOnce(UnixStream, &Path, &Path) + std::panic::UnwindSafe) {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut wm, run, record, log) = session(&dir);
    let result = std::panic::catch_unwind(|| {
        let Some(socket) = socket_of(&record, &run, Duration::from_secs(60)) else {
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            assert!(text.contains("no graphics driver"), "the session never listened: {text}");
            eprintln!("no card to paint with: not tried");
            return;
        };
        body(UnixStream::connect(&socket).unwrap(), &log, &record);
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
fn a_program_is_told_it_is_maximized_and_its_own_buttons_reach_the_scene_only_when_they_change_something() {
    with_session("maxi", |stream, _log, record| {
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
        let base: XdgWmBase = globals.bind(&qh, 1..=2, ()).unwrap();
        let mut app = App::default();
        let surface = compositor.create_surface(&qh, ());
        let xdg = base.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_title("maxi".into());
        top.set_app_id("maxi.test".into());
        surface.commit();
        // Until `until` holds or a few seconds go by: the scene answers on its next round.
        let mut wait = |app: &mut App, until: &dyn Fn(&App) -> bool, secs: u64| {
            let start = Instant::now();
            loop {
                queue.roundtrip(app).unwrap();
                if until(app) || start.elapsed() > Duration::from_secs(secs) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait(&mut app, &|a| a.maximized.is_some() && a.size.0 > 0, 10);
        assert_eq!(app.maximized, Some(false), "a new window is not maximized");
        // «Restore» when it is not maximized (Electron says it as it is created): only answered, the scene is not told.
        top.unset_maximized();
        wait(&mut app, &|a| a.maximized == Some(true), 2);
        assert_eq!(app.maximized, Some(false), "a «restore» of a window that is not maximized reached the scene, which maximized it");
        // «Maximize»: the scene is told, maximizes it, and the program is told so, with its new size.
        top.set_maximized();
        wait(&mut app, &|a| a.maximized == Some(true) && a.size == (1000, 700), 10);
        assert_eq!(app.maximized, Some(true), "maximized by the scene, and not told so");
        assert_eq!(app.size, (1000, 700), "the size the scene gives a maximized one");
        // «Maximize» again: it already is; the scene would toggle it back if it heard it.
        top.set_maximized();
        wait(&mut app, &|a| a.maximized == Some(false), 2);
        assert_eq!(app.maximized, Some(true), "a «maximize» of a maximized window reached the scene");
        // «Restore»: back, and told so.
        top.unset_maximized();
        wait(&mut app, &|a| a.maximized == Some(false) && a.size == (600, 400), 10);
        assert_eq!(app.maximized, Some(false), "restored by the scene, and not told so");
        assert_eq!(app.size, (600, 400));
        // Each of them in the session's log, with what came of it: a button whose request never
        // comes can be told from one the scene does nothing with.
        let printed = std::fs::read_to_string(record).unwrap_or_default();
        let said: Vec<&str> = printed.lines().filter(|l| l.contains("«maxi.test» asks")).collect();
        let want = [
            "asks to be restored: it already is: only answered",
            "asks to be maximized: the scene is told (window_maximize)",
            "asks to be maximized: it already is: only answered",
            "asks to be restored: the scene is told (window_maximize)",
        ];
        assert_eq!(said.len(), want.len(), "one line for each request: {said:#?}");
        for (line, w) in said.iter().zip(want) {
            assert!(line.starts_with("windows · window ") && line.ends_with(w), "{line:?} should end with {w:?}");
        }
    });
}
