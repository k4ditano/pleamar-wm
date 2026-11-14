//! `wp_presentation` says when the screen showed a frame. A program that draws without rest
//! and counts refreshes from those times must find them one refresh apart: 16.67 ms on a
//! 60 Hz monitor. They used to be the moment each answer was given (labelled as vsync), and
//! headless they wandered from 5 to 41 ms apart. A screenless session (`pleamar-wm headless`)
//! with `tests/presentation.plm` and a minimal xdg-shell client in this file.
//! Without a card to paint with, the session says so and the test passes without trying.

use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_callback::{self, WlCallback}, wl_compositor::WlCompositor, wl_output::WlOutput, wl_registry::WlRegistry, wl_shm::{self, WlShm}, wl_shm_pool::WlShmPool, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::presentation_time::client::{wp_presentation::WpPresentation, wp_presentation_feedback::{self, WpPresentationFeedback}};
use wayland_protocols::xdg::shell::client::{xdg_surface::{self, XdgSurface}, xdg_toplevel::XdgToplevel, xdg_wm_base::{self, XdgWmBase}};

#[derive(Default)]
struct App {
    configured: Option<u32>,
    frame_done: bool,
    /// (time, refresh, flags) of each frame presented; how many were discarded.
    presented: Vec<(Duration, u32, u32)>,
    discarded: u32,
}

macro_rules! quiet {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
quiet!(WlCompositor, WlSurface, WlShm, WlShmPool, WlBuffer, WlOutput, WpPresentation, XdgToplevel);

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgWmBase, ()> for App {
    fn event(_: &mut Self, base: &XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for App {
    fn event(app: &mut Self, _: &XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            app.configured = Some(serial);
        }
    }
}

impl Dispatch<WlCallback, ()> for App {
    fn event(app: &mut Self, _: &WlCallback, event: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = event {
            app.frame_done = true;
        }
    }
}

impl Dispatch<WpPresentationFeedback, ()> for App {
    fn event(app: &mut Self, _: &WpPresentationFeedback, event: wp_presentation_feedback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, refresh, flags, .. } => {
                let secs = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                let flags = match flags {
                    WEnum::Value(f) => f.bits(),
                    WEnum::Unknown(b) => b,
                };
                app.presented.push((Duration::new(secs, tv_nsec), refresh, flags));
            }
            wp_presentation_feedback::Event::Discarded => app.discarded += 1,
            _ => {}
        }
    }
}

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
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/presentation.plm"))
        .args(["--seconds", "120"])
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

/// Draws `frames` frames without rest, each one when the last one's frame callback comes,
/// and gives back what wp_presentation said of them.
fn draw_without_rest(socket: &Path, dir: &Path, frames: usize) -> App {
    let conn = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).expect("wl_compositor");
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let base: XdgWmBase = globals.bind(&qh, 1..=2, ()).expect("xdg_wm_base");
    let presentation: WpPresentation = globals.bind(&qh, 1..=1, ()).expect("wp_presentation");
    let mut app = App::default();

    let surface = compositor.create_surface(&qh, ());
    let xdg = base.get_xdg_surface(&surface, &qh, ());
    let top = xdg.get_toplevel(&qh, ());
    top.set_title("presentation".into());
    surface.commit();
    let start = Instant::now();
    while app.configured.is_none() {
        queue.blocking_dispatch(&mut app).unwrap();
        assert!(start.elapsed() < Duration::from_secs(60), "the window was never configured");
    }
    xdg.ack_configure(app.configured.unwrap());

    let (w, h) = (320i32, 240i32);
    let size = (w * h * 4) as usize;
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(dir.join("pool")).unwrap();
    file.set_len(size as u64 * 2).unwrap();
    let pool = shm.create_pool(file.as_fd(), size as i32 * 2, &qh, ());
    let buffers = [pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ()), pool.create_buffer(size as i32, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ())];

    for n in 0..frames {
        app.frame_done = false;
        surface.attach(Some(&buffers[n % 2]), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.frame(&qh, ());
        presentation.feedback(&surface, &qh, ());
        surface.commit();
        let start = Instant::now();
        while !app.frame_done {
            queue.blocking_dispatch(&mut app).unwrap();
            assert!(start.elapsed() < Duration::from_secs(10), "frame {n} never got its frame callback");
        }
    }
    // The last ones' feedback.
    let start = Instant::now();
    while app.presented.len() + (app.discarded as usize) < frames && start.elapsed() < Duration::from_secs(5) {
        queue.roundtrip(&mut app).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    app
}

#[test]
fn presented_times_sit_one_refresh_apart() {
    let dir = std::env::temp_dir().join(format!("pf-{}", std::process::id()));
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
        let app = draw_without_rest(&socket, &dir2, 150);
        // The first frames find the window still being placed: count from the 30th.
        let times: Vec<Duration> = app.presented.iter().skip(30).map(|p| p.0).collect();
        assert!(times.len() >= 100, "too few frames presented: {} (discarded {})", app.presented.len(), app.discarded);
        let gaps: Vec<f64> = times.windows(2).map(|t| t[1].as_secs_f64() * 1e3 - t[0].as_secs_f64() * 1e3).collect();
        let mut sorted = gaps.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        let (min, max) = (sorted[0], sorted[sorted.len() - 1]);
        let refresh = app.presented.last().unwrap().1;
        let flags = app.presented.last().unwrap().2;
        eprintln!("presented {} (discarded {}); gaps ms: median {median:.3}, min {min:.3}, max {max:.3}; refresh {refresh} ns; flags {flags:#x}", app.presented.len(), app.discarded);
        // Each time is a refresh: every gap a whole number of refreshes (to half a millisecond),
        // most of them one. A frame drawn late on a busy runner skips a refresh or more, and
        // that is told as such; a time between two refreshes is what must not happen. Before,
        // told as the moment each answer was given, they sat from 2 to 40 ms apart.
        let period = 1000.0 / 60.0;
        let between: Vec<f64> = gaps.iter().copied().filter(|g| { let k = (g / period).round().max(1.0); (g - k * period).abs() > 0.5 }).collect();
        let skipped = gaps.iter().filter(|g| (*g / period).round() > 1.0).count();
        eprintln!("whole refreshes: {} of {} gaps one refresh, {skipped} skipped one or more, {} between two", gaps.len() - skipped - between.len(), gaps.len(), between.len());
        assert!((median - period).abs() < 0.2, "the median gap is not a 60 Hz refresh: {median:.3} ms");
        assert!(between.is_empty(), "times between two refreshes: {between:?}");
        assert!(skipped * 10 <= gaps.len(), "more than one in ten frames skipped a refresh: {skipped} of {}", gaps.len());
        assert!(gaps.iter().all(|g| *g > 0.0), "times that go back: {gaps:?}");
        assert_eq!(flags & 0x1, 0x1, "told as a vsync time");
    });
    let _ = wm.kill();
    let _ = wm.wait();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
