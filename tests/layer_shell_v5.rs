//! `zwlr_layer_shell_v1` version 5 (`src/layer_v5.rs`): hyprtoolkit 0.6 (hyprpolkitagent,
//! hyprlauncher) asks for it and used to be thrown out with «Invalid binding of
//! zwlr_layer_shell_v1 version 5». A screenless session (`pleamar-wm headless`) and a
//! minimal client of the protocol in this file:
//!  * a client that binds version 5 is not thrown out, and its `set_exclusive_edge` is taken;
//!  * a wrong edge (not one of the anchors) is a protocol error, `invalid_exclusive_edge`;
//!  * the compositor's log says which program it threw out (pid and name) and why.
//!
//! Without a card to paint with the session says so and the tests pass without trying, like
//! the other tests of the session (`tests/virtual_pointer.rs`).

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

#[derive(Default)]
struct App {
    /// The serial of the last `configure` the compositor sent.
    configured: Option<u32>,
    closed: bool,
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
impl Dispatch<ZwlrLayerShellV1, ()> for App {
    fn event(_: &mut Self, _: &ZwlrLayerShellV1, _: <ZwlrLayerShellV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrLayerSurfaceV1, ()> for App {
    fn event(app: &mut Self, _: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => app.configured = Some(serial),
            zwlr_layer_surface_v1::Event::Closed => app.closed = true,
            _ => {}
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

/// A layer surface anchored as asked (with its zone and, if any, its edge), committed once.
fn layer_surface(conn: &Connection, queue: &mut EventQueue<App>, app: &mut App, anchor: Anchor, edge: Option<Anchor>) -> (ZwlrLayerSurfaceV1, WlSurface) {
    let (globals, _) = registry_queue_init::<App>(conn).unwrap();
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
    // What the program asks for: version 5, no less.
    let shell: ZwlrLayerShellV1 = globals.bind(&qh, 5..=5, ()).expect("the compositor advertises zwlr_layer_shell_v1 version 5");
    assert_eq!(shell.version(), 5);
    let surface = compositor.create_surface(&qh, ());
    let layer = shell.get_layer_surface(&surface, None, Layer::Top, "test-bar".into(), &qh, ());
    layer.set_size(200, 30);
    layer.set_anchor(anchor);
    layer.set_exclusive_zone(30);
    if let Some(edge) = edge {
        layer.set_exclusive_edge(edge);
    }
    surface.commit();
    queue.roundtrip(app).unwrap();
    (layer, surface)
}

#[test]
fn a_program_asking_for_layer_shell_5_binds_and_names_its_exclusive_edge() {
    with_session("ls5-a", |stream, log| {
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        // The global is announced with version 5 (smithay's own said 4).
        let announced = globals.contents().with_list(|l| l.iter().find(|g| g.interface == "zwlr_layer_shell_v1").map(|g| g.version));
        assert_eq!(announced, Some(5));
        let mut app = App::default();
        // A bar in the top-left corner: its zone's edge cannot be told from its anchors, so it names it.
        let (layer, surface) = layer_surface(&conn, &mut queue, &mut app, Anchor::Top | Anchor::Left, Some(Anchor::Top));
        let serial = app.configured.expect("the compositor configured the surface");
        layer.ack_configure(serial);
        // A change of the edge (to the other of its anchors) is taken at the next commit too.
        layer.set_exclusive_edge(Anchor::Left);
        surface.commit();
        queue.roundtrip(&mut app).unwrap();
        queue.roundtrip(&mut app).unwrap();
        assert!(!app.closed, "the surface was closed");
        // Not thrown out: still connected and answering, and the session does not say it threw anybody out.
        std::thread::sleep(Duration::from_millis(300));
        let text = std::fs::read_to_string(log).unwrap_or_default();
        assert!(!text.contains("thrown out"), "the session threw the program out: {text}");
    });
}

#[test]
fn an_edge_that_is_not_an_anchor_is_a_protocol_error_and_the_log_says_who_was_thrown_out() {
    with_session("ls5-b", |stream, log| {
        let conn = Connection::from_socket(stream).unwrap();
        let (_, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let mut app = App::default();
        // Anchored to the top, with the bottom as its exclusive edge: not one of its anchors.
        let bad = {
            let (globals, _) = registry_queue_init::<App>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
            let shell: ZwlrLayerShellV1 = globals.bind(&qh, 5..=5, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let layer = shell.get_layer_surface(&surface, None, Layer::Top, "bad-bar".into(), &qh, ());
            layer.set_size(200, 30);
            layer.set_anchor(Anchor::Top);
            layer.set_exclusive_zone(30);
            layer.set_exclusive_edge(Anchor::Bottom);
            surface.commit();
            queue.roundtrip(&mut app)
        };
        let error = bad.expect_err("the compositor threw the program out");
        let said = format!("{error:?}");
        assert!(said.contains("zwlr_layer_surface_v1") && said.contains("code: 4"), "invalid_exclusive_edge (4) on the layer surface: {said}");
        // The log: who (this test's process and name, as /proc says them) and what it did wrong.
        let who = format!("pid {} ({})", std::process::id(), std::fs::read_to_string("/proc/self/comm").unwrap().trim());
        let start = Instant::now();
        let line = loop {
            let text = std::fs::read_to_string(log).unwrap_or_default();
            if let Some(l) = text.lines().find(|l| l.contains("thrown out")) {
                break l.to_string();
            }
            assert!(start.elapsed() < Duration::from_secs(10), "no «thrown out» line in the log: {text}");
            std::thread::sleep(Duration::from_millis(100));
        };
        assert!(line.contains(&who), "the line says who ({who}): {line}");
        assert!(line.contains("exclusive edge"), "the line says why: {line}");
    });
}
