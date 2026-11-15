//! `xdg-decoration`: who draws a window's frame is what the program asks. «Client side» is «I do» (an
//! Electron window with no frame, a Qt one with its own bar) and gets that answer, so the scene puts no frame
//! over its bar; «server side» (or nothing said) is the scene's, as it always was. pleamar-wm used to answer
//! server side to everybody, so Discord had two title bars. A screenless session (`pleamar-wm headless`) and a
//! minimal client of the protocol in this file. Without a card to paint with the session says so and the
//! tests pass without trying, like the other tests of the session.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::{self, Mode, ZxdgToplevelDecorationV1},
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};

/// What the compositor has said to the client: the last mode of each decoration, by the number it was made with.
#[derive(Default)]
struct App {
    modes: [Option<Mode>; 4],
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
    fn event(_: &mut Self, _: &XdgToplevel, _: <XdgToplevel as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZxdgDecorationManagerV1, ()> for App {
    fn event(_: &mut Self, _: &ZxdgDecorationManagerV1, _: <ZxdgDecorationManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZxdgToplevelDecorationV1, usize> for App {
    fn event(app: &mut Self, _: &ZxdgToplevelDecorationV1, event: zxdg_toplevel_decoration_v1::Event, n: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode: WEnum::Value(mode) } = event {
            app.modes[*n] = Some(mode);
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
        // Any desk will do: what is looked at is the protocol, not what the scene counts.
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/virtual-pointer.plm"))
        .args(["--seconds", "60", "--record", "pointer.x,pointer.y,clicks,rights,wheeled,sideways"])
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
fn with_session(name: &str, body: impl FnOnce(UnixStream, &Path) + std::panic::UnwindSafe) {
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
        body(UnixStream::connect(&socket).unwrap(), &log);
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
fn a_program_that_draws_its_own_frame_is_told_so_and_one_that_asks_for_the_scenes_is_told_that() {
    with_session("deco", |stream, _log| {
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
        let base: XdgWmBase = globals.bind(&qh, 1..=2, ()).unwrap();
        let manager: ZxdgDecorationManagerV1 = globals.bind(&qh, 1..=1, ()).expect("the compositor offers xdg-decoration");
        let mut app = App::default();
        // Two windows, each with its decoration (before its first commit, as toolkits do).
        let mut windows = Vec::new();
        for n in 0..2 {
            let surface = compositor.create_surface(&qh, ());
            let xdg = base.get_xdg_surface(&surface, &qh, ());
            let top = xdg.get_toplevel(&qh, ());
            let deco = manager.get_toplevel_decoration(&top, &qh, n);
            windows.push((surface, xdg, top, deco));
        }
        // A: «client side»: I draw my own frame (what an Electron window with no frame says).
        windows[0].3.set_mode(Mode::ClientSide);
        // B: first the same, then it changes its mind: the scene's.
        windows[1].3.set_mode(Mode::ClientSide);
        for (surface, ..) in &windows {
            surface.commit();
        }
        queue.roundtrip(&mut app).unwrap();
        queue.roundtrip(&mut app).unwrap();
        assert_eq!(app.modes[0], Some(Mode::ClientSide), "asked «client side» and was not believed: {:?}", app.modes);
        assert_eq!(app.modes[1], Some(Mode::ClientSide), "{:?}", app.modes);
        windows[1].3.set_mode(Mode::ServerSide);
        windows[1].0.commit();
        queue.roundtrip(&mut app).unwrap();
        queue.roundtrip(&mut app).unwrap();
        assert_eq!(app.modes[1], Some(Mode::ServerSide), "asked «server side» afterwards and was not given it: {:?}", app.modes);
        // And A is as it was.
        assert_eq!(app.modes[0], Some(Mode::ClientSide));
    });
}
