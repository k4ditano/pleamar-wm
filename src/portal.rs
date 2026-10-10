//! Sharing the screen, as the session's own portal: what a program in a
//! call (Discord, a browser, OBS) asks `xdg-desktop-portal` for, answered here
//! (`org.freedesktop.impl.portal.ScreenCast`) instead of by a portal of
//! another desktop's. The pictures are the monitors' own —the same road as a
//! screenshot (`layers::capture`), taken when the monitor changes, not on a
//! clock— and they leave by PipeWire, one stream per session.
//!
//! What is shared is chosen in the window manager's scene: the portal asks it
//! (`win.picking`) and it answers with `pick` —a monitor, or a window, which
//! is then read from its own buffers, covered or on another workspace—.
//!
//! Two threads: D-Bus (zbus), which answers the portal, and PipeWire, which
//! owns the streams and asks the monitors for their pictures.

use crate::layers;
use pipewire as pw;
use pw::spa;
use spa::pod::{Object, Pod, Property, PropertyFlags, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use pleamar::scene::{NestEvent, Picked, ToRender, WindowPicture};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as ZValue};

/// The name the portal asks for (`pleamar.portal` says it).
const NAME: &str = "org.freedesktop.impl.portal.desktop.pleamar";
const PATH: &str = "/org/freedesktop/portal/desktop";

/// Its pictures' numbers: far from the ones the Wayland side gives, so
/// `hand_picture` knows which are these.
const FIRST: u64 = 1 << 62;
static NEXT: AtomicU64 = AtomicU64::new(FIRST);

/// What is shared: a monitor, or a window of the scene's (its slot).
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Monitor(usize),
    Window(usize),
}

/// What the D-Bus side, the monitors and the render tell the PipeWire thread.
enum Msg {
    /// A stream of that; its node and its size, once PipeWire gives it one.
    /// `cursor`: the pointer drawn into the pictures (the program asked for it).
    Start { session: String, source: Source, cursor: bool, reply: async_channel::Sender<Option<(u32, (u32, u32))>> },
    Stop { session: String },
    /// A picture of a monitor it asked for, taken (BGRx, rows with no padding).
    Picture { id: u64, pixels: Option<Vec<u8>> },
    /// A shared window drew: its picture.
    Window { session: String, picture: WindowPicture },
    /// A look at the pointer, for the streams that draw it.
    Tick,
}

/// How many streams draw the pointer, and the thread that looks at it for
/// them: asleep while there are none.
static WITH_POINTER: AtomicU64 = AtomicU64::new(0);
static LOOKER: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();

fn pointer_streams(more: bool) {
    if more {
        WITH_POINTER.fetch_add(1, Ordering::AcqRel);
        if let Some(t) = LOOKER.get() {
            t.unpark();
        }
    } else {
        let _ = WITH_POINTER.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
    }
}

static TO_PW: Mutex<Option<pw::channel::Sender<Msg>>> = Mutex::new(None);
/// The scene's render: to ask it to choose, and for the windows' pictures.
static TO_RENDER: Mutex<Option<Sender<ToRender>>> = Mutex::new(None);
/// Who waits for the scene's choice (one at a time).
static CHOOSING: Mutex<Option<async_channel::Sender<Option<Picked>>>> = Mutex::new(None);

fn render(m: ToRender) {
    if let Some(tx) = TO_RENDER.lock().unwrap().as_ref() {
        let _ = tx.send(m);
    }
}

/// The scene's answer (`pick`): to whoever asked, and the scene stops choosing.
pub fn picked(what: Option<Picked>) {
    render(ToRender::Nest(NestEvent::Pick(0)));
    if let Some(tx) = CHOOSING.lock().unwrap().take() {
        let _ = tx.try_send(what);
    }
}

fn tell(m: Msg) {
    if let Some(tx) = TO_PW.lock().unwrap().as_ref() {
        let _ = tx.send(m);
    }
}

/// A picture that came back from a monitor: whether it was one of these.
pub fn deliver(id: u64, pixels: Option<Vec<u8>>) -> bool {
    if id < FIRST {
        return false;
    }
    // A screenshot's, waiting for it; or a stream's.
    let waiting = SNAPS.lock().unwrap().as_mut().and_then(|m| m.remove(&id));
    if let Some(tx) = waiting {
        let _ = tx.try_send(pixels);
        return true;
    }
    tell(Msg::Picture { id, pixels });
    true
}

/// The screenshots waiting for a monitor's picture, by its number.
static SNAPS: Mutex<Option<HashMap<u64, async_channel::Sender<Option<Vec<u8>>>>>> = Mutex::new(None);

/// Marea is told when the screen starts and stops being shared: its notices
/// come in quietly meanwhile, and it shows it. Only on the first and the last.
fn sharing(streams: usize) {
    static SHARING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let now = streams > 0;
    if SHARING.swap(now, Ordering::AcqRel) != now {
        layers::tell(layers::ToLayers::Launch(format!("marea {}", if now { "sharing_on" } else { "sharing_off" })));
    }
}

/// The private windows on that monitor (`window app=… private`), made into
/// big squares of their own colour: what they show cannot be read.
fn hide_private(n: usize, pixels: &mut [u8], (w, h): (u32, u32)) {
    let Some(m) = layers::monitors().get(n).cloned() else { return };
    const BLOCK: i32 = 28;
    // Its title bar too, which the scene draws over its box: a title says a lot.
    let bar = (40.0 * m.scale) as i32;
    for r in layers::private_on(&m.name) {
        let r = [r[0] - 4, r[1] - bar, r[2] + 8, r[3] + bar + 4];
        let (x0, y0) = (r[0].max(0), r[1].max(0));
        let (x1, y1) = ((r[0] + r[2]).min(w as i32), (r[1] + r[3]).min(h as i32));
        let mut by = y0;
        while by < y1 {
            let mut bx = x0;
            while bx < x1 {
                let (ex, ey) = ((bx + BLOCK).min(x1), (by + BLOCK).min(y1));
                let mut sum = [0u64; 3];
                let mut n = 0u64;
                for y in by..ey {
                    for x in bx..ex {
                        let i = ((y as u32 * w + x as u32) * 4) as usize;
                        for k in 0..3 {
                            sum[k] += pixels[i + k] as u64;
                        }
                        n += 1;
                    }
                }
                let avg = [(sum[0] / n.max(1)) as u8, (sum[1] / n.max(1)) as u8, (sum[2] / n.max(1)) as u8];
                for y in by..ey {
                    for x in bx..ex {
                        let i = ((y as u32 * w + x as u32) * 4) as usize;
                        pixels[i..i + 3].copy_from_slice(&avg);
                    }
                }
                bx = ex;
            }
            by = (by + BLOCK).min(y1);
        }
    }
}

/// A monitor's picture now, whole (BGRx), with its private windows hidden.
async fn monitor_picture(n: usize) -> Option<(Vec<u8>, (u32, u32))> {
    let m = layers::monitors().get(n).cloned()?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = async_channel::bounded(1);
    SNAPS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, tx);
    layers::capture(n, id, [0, 0, m.size.0 as i32, m.size.1 as i32], false, u64::MAX);
    let pixels = futures_lite::future::or(async { rx.recv().await.ok().flatten() }, async {
        async_io::Timer::after(std::time::Duration::from_secs(3)).await;
        None
    })
    .await;
    let mut pixels = pixels?;
    hide_private(n, &mut pixels, m.size);
    Some((pixels, m.size))
}

/// Who watches a window, for the render (several may watch the same one): a
/// share of it, a single picture of it. Thumbnails (`nest::capture`) take
/// their numbers from `THUMBS` up.
const SHARE: u64 = 1;
const ONCE: u64 = 2;
pub const THUMBS: u64 = 1000;

/// A window's picture now (BGRA premultiplied): asked once, and let go.
async fn window_picture(slot: usize) -> Option<(Vec<u8>, (u32, u32))> {
    let (tx, rx) = std::sync::mpsc::channel::<WindowPicture>();
    render(ToRender::WatchWindow(slot, ONCE, Some(tx)));
    let (done, wait) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = done.try_send(rx.recv_timeout(std::time::Duration::from_secs(3)).ok());
    });
    let picture = wait.recv().await.ok().flatten();
    render(ToRender::WatchWindow(slot, ONCE, None));
    picture.map(|p| (std::sync::Arc::unwrap_or_clone(p.pixels), p.size))
}

/// The whole desktop: every monitor where it is, at the finest scale.
async fn desktop_picture() -> Option<(Vec<u8>, (u32, u32))> {
    let monitors = layers::monitors();
    let scale = monitors.iter().map(|m| m.scale).fold(1.0, f64::max);
    let x0 = monitors.iter().map(|m| m.x).min()?;
    let y0 = monitors.iter().map(|m| m.y).min()?;
    let x1 = monitors.iter().map(|m| m.x as f64 + m.size.0 as f64 / m.scale).fold(0.0, f64::max);
    let y1 = monitors.iter().map(|m| m.y as f64 + m.size.1 as f64 / m.scale).fold(0.0, f64::max);
    let (w, h) = (((x1 - x0 as f64) * scale).round() as u32, ((y1 - y0 as f64) * scale).round() as u32);
    let mut canvas = vec![0u8; (w * h * 4) as usize];
    for (n, m) in monitors.iter().enumerate() {
        let Some((px, (mw, mh))) = monitor_picture(n).await else { continue };
        let (ox, oy) = (((m.x - x0) as f64 * scale) as u32, ((m.y - y0) as f64 * scale) as u32);
        for y in 0..mh.min(h.saturating_sub(oy)) {
            let row = mw.min(w.saturating_sub(ox)) as usize * 4;
            let d = ((oy + y) * w + ox) as usize * 4;
            let s = (y * mw) as usize * 4;
            canvas[d..d + row].copy_from_slice(&px[s..s + row]);
        }
    }
    Some((canvas, (w, h)))
}

/// A picture to a PNG in the temporary folder (BGRA premultiplied in; the
/// alpha of a monitor's picture means nothing, so it is made opaque):
/// its `file://` address.
fn save_png(pixels: &[u8], (w, h): (u32, u32), opaque: bool) -> Option<String> {
    let mut rgba = Vec::with_capacity(pixels.len());
    for p in pixels.chunks_exact(4) {
        let a = if opaque { 255 } else { p[3] };
        let un = |c: u8| if a == 0 || a == 255 { c } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
        rgba.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let path = std::env::temp_dir().join(format!("pleamar-screenshot-{stamp}.png"));
    image::save_buffer(&path, &rgba, w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(format!("file://{}", path.display()))
}

/// Asks the scene to choose (1 a monitor, 2 a window, 3 either); `None`, nothing.
async fn choose(types: u32) -> Option<Picked> {
    let (tx, choice) = async_channel::bounded(1);
    if let Some(old) = CHOOSING.lock().unwrap().replace(tx) {
        let _ = old.try_send(None);
    }
    render(ToRender::Nest(NestEvent::Pick(types)));
    futures_lite::future::or(async { choice.recv().await.ok().flatten() }, async {
        async_io::Timer::after(std::time::Duration::from_secs(120)).await;
        picked(None);
        None
    })
    .await
}

// ── global shortcuts: a program's keys, wherever the keyboard is ─────────

/// A program's shortcut: its session, its id, what it says it does, and the
/// key it is on (the one it asked for, or the one `keys.conf` gave it).
struct Shortcut {
    session: String,
    id: String,
    description: String,
    key: Option<crate::keys::Bind>,
}

static SHORTCUTS: Mutex<Vec<Shortcut>> = Mutex::new(Vec::new());
static CONNECTION: std::sync::OnceLock<zbus::blocking::Connection> = std::sync::OnceLock::new();
/// Which program each session is, for `keys.conf`'s `shortcut app:id`.
static APPS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The program's shortcut on that key, if there is one: its session and id.
pub fn shortcut_at(name: &str, base: Option<&str>, mods: pleamar::scene::Mods) -> Option<(String, String)> {
    let all = SHORTCUTS.lock().unwrap();
    all.iter()
        .find(|s| s.key.as_ref().is_some_and(|k| k.fits(name, mods) || base.is_some_and(|b| k.fits(b, mods))))
        .map(|s| (s.session.clone(), s.id.clone()))
}

/// Tells the program its shortcut went down (true) or up.
pub fn shortcut_signal(session: &str, id: &str, down: bool) {
    let Some(c) = CONNECTION.get() else { return };
    let Ok(path) = zbus::zvariant::ObjectPath::try_from(session) else { return };
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let options: HashMap<String, OwnedValue> = HashMap::new();
    let _ = c.emit_signal(None::<&str>, PATH, "org.freedesktop.impl.portal.GlobalShortcuts", if down { "Activated" } else { "Deactivated" }, &(path, id, ms, options));
}

fn shortcuts_of(session: &str) -> Vec<(String, HashMap<String, OwnedValue>)> {
    SHORTCUTS
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.session == session)
        .map(|s| {
            let mut props = HashMap::from([("description".to_owned(), owned(ZValue::from(s.description.clone())))]);
            if let Some(k) = &s.key {
                props.insert("trigger_description".to_owned(), owned(ZValue::from(k.describe())));
            }
            (s.id.clone(), props)
        })
        .collect()
}

struct GlobalShortcuts {
    sessions: Sessions,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.GlobalShortcuts")]
impl GlobalShortcuts {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> Answer {
        let path = session_handle.to_string();
        APPS.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.clone(), app_id);
        self.sessions.lock().unwrap().insert(path.clone(), Choices::default());
        let session = Session { path: path.clone(), sessions: self.sessions.clone() };
        if server.at(session_handle.as_ref(), session).await.is_err() {
            return (2, HashMap::new());
        }
        (0, HashMap::new())
    }

    /// Each on the key the user gave it in `keys.conf`, or the one it asks
    /// for (`preferred_trigger`); with neither, it is there but on no key.
    async fn bind_shortcuts(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath, shortcuts: Vec<(String, HashMap<String, OwnedValue>)>, _parent_window: String, _options: HashMap<String, OwnedValue>) -> Answer {
        let session = session_handle.to_string();
        let app = APPS.lock().unwrap().as_ref().and_then(|a| a.get(&session).cloned()).unwrap_or_default();
        let keys = crate::keys::get();
        {
            let mut all = SHORTCUTS.lock().unwrap();
            all.retain(|s| s.session != session);
            for (id, props) in shortcuts {
                let text = |k: &str| props.get(k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
                let key = keys.shortcut(&app, &id).or_else(|| crate::keys::trigger(&text("preferred_trigger")));
                println!("portal · {} binds «{id}» on {}", if app.is_empty() { "a program" } else { &app }, key.as_ref().map_or("no key".to_owned(), |k| k.describe()));
                all.push(Shortcut { session: session.clone(), id, description: text("description"), key });
            }
        }
        (0, HashMap::from([("shortcuts".to_owned(), owned(ZValue::from(shortcuts_of(&session))))]))
    }

    async fn list_shortcuts(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath) -> Answer {
        (0, HashMap::from([("shortcuts".to_owned(), owned(ZValue::from(shortcuts_of(session_handle.as_str()))))]))
    }

    /// Their keys are changed in `keys.conf` (`shortcut id Mods+key`): no window for it.
    async fn configure_shortcuts(&self, _session_handle: OwnedObjectPath, _parent_window: String, _options: HashMap<String, OwnedValue>) {}

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

struct Screenshot;

#[zbus::interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl Screenshot {
    async fn screenshot(&self, _handle: OwnedObjectPath, app_id: String, _parent_window: String, options: HashMap<String, OwnedValue>) -> Answer {
        let interactive = options.get("interactive").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        let target = options.get("target").and_then(|v| u32::try_from(v).ok()).unwrap_or(1);
        println!("portal · {} asks for a screenshot", if app_id.is_empty() { "a program" } else { &app_id });
        // Asked to choose, or a window: the same chooser as sharing.
        let picture = if interactive || target == 2 {
            match choose(if target == 2 { 2 } else { 3 }).await {
                Some(Picked::Window(slot)) => window_picture(slot).await.map(|(p, s)| (p, s, false)),
                Some(Picked::Screen(n)) => monitor_picture(n).await.map(|(p, s)| (p, s, true)),
                None => return (1, HashMap::new()),
            }
        } else {
            desktop_picture().await.map(|(p, s)| (p, s, true))
        };
        let Some((pixels, size, opaque)) = picture else { return (2, HashMap::new()) };
        match save_png(&pixels, size, opaque) {
            Some(uri) => (0, HashMap::from([("uri".to_owned(), owned(ZValue::from(uri)))])),
            None => (2, HashMap::new()),
        }
    }

    /// A colour of the screen, chosen with `hyprpicker` (its zoom and its
    /// preview) if it is there.
    async fn pick_color(&self, _handle: OwnedObjectPath, _app_id: String, _parent_window: String, _options: HashMap<String, OwnedValue>) -> Answer {
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let out = std::process::Command::new("hyprpicker").args(["-f", "rgb", "-b", "-q"]).output().ok();
            let _ = tx.try_send(out.filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()));
        });
        let Some(text) = rx.recv().await.ok().flatten() else { return (2, HashMap::new()) };
        let v: Vec<f64> = text.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<f64>().ok()).collect();
        if v.len() < 3 {
            return (1, HashMap::new());
        }
        (0, HashMap::from([("color".to_owned(), owned(ZValue::from((v[0] / 255.0, v[1] / 255.0, v[2] / 255.0))))]))
    }

    /// The whole screen (1) and a window or monitor chosen (2).
    #[zbus(property)]
    fn available_targets(&self) -> u32 {
        1 | 2
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        3
    }
}

/// Starts both threads. Without a session bus or PipeWire there is simply no
/// portal: the session goes on.
pub fn start(to_render: Sender<ToRender>) {
    *TO_RENDER.lock().unwrap() = Some(to_render);
    let (tx, rx) = pw::channel::channel::<Msg>();
    *TO_PW.lock().unwrap() = Some(tx);
    let _ = std::thread::Builder::new().name("portal-pw".into()).spawn(move || {
        if let Err(e) = pipewire_thread(rx) {
            eprintln!("portal · no PipeWire ({e}): the screen cannot be shared");
        }
    });
    if let Ok(h) = std::thread::Builder::new().name("portal-pointer".into()).spawn(|| loop {
        if WITH_POINTER.load(Ordering::Acquire) == 0 {
            std::thread::park();
            continue;
        }
        std::thread::sleep(std::time::Duration::from_millis(33));
        tell(Msg::Tick);
    }) {
        let _ = LOOKER.set(h.thread().clone());
    }
    let _ = std::thread::Builder::new().name("portal-dbus".into()).spawn(|| {
        let sessions: Sessions = Default::default();
        let built = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.serve_at(PATH, ScreenCast { sessions: sessions.clone() }))
            .and_then(|b| b.serve_at(PATH, GlobalShortcuts { sessions }))
            .and_then(|b| b.serve_at(PATH, Screenshot))
            .and_then(|b| b.name(NAME))
            .and_then(|b| b.build());
        match built {
            Ok(connection) => {
                println!("portal · {NAME}: sharing the screen is this session's");
                let _ = CONNECTION.set(connection.clone());
                // The connection answers on its own thread; this one keeps it alive.
                loop {
                    std::thread::park();
                    let _ = &connection;
                }
            }
            Err(e) => eprintln!("portal · not on the session bus ({e})"),
        }
    });
}

// ── D-Bus: what the portal asks ───────────────────────────────────

/// What each session chose, by its object path.
#[derive(Default, Clone)]
struct Choices {
    /// What it may share: 1 monitors, 2 windows (both, 3).
    types: u32,
    cursor: u32,
}
type Sessions = std::sync::Arc<Mutex<HashMap<String, Choices>>>;

struct ScreenCast {
    sessions: Sessions,
}

type Answer = (u32, HashMap<String, OwnedValue>);

fn owned(v: ZValue<'_>) -> OwnedValue {
    OwnedValue::try_from(v).expect("a value with no file descriptors")
}

#[zbus::interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> Answer {
        let path = session_handle.to_string();
        println!("portal · {} asks to share the screen", if app_id.is_empty() { "a program" } else { &app_id });
        self.sessions.lock().unwrap().insert(path.clone(), Choices::default());
        let session = Session { path: path.clone(), sessions: self.sessions.clone() };
        if server.at(session_handle.as_ref(), session).await.is_err() {
            return (2, HashMap::new());
        }
        (0, HashMap::new())
    }

    async fn select_sources(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath, _app_id: String, options: HashMap<String, OwnedValue>) -> Answer {
        let cursor = options.get("cursor_mode").and_then(|v| u32::try_from(v).ok()).unwrap_or(1);
        let types = options.get("types").and_then(|v| u32::try_from(v).ok()).unwrap_or(1) & 3;
        match self.sessions.lock().unwrap().get_mut(session_handle.as_str()) {
            Some(c) => {
                c.cursor = cursor;
                c.types = if types == 0 { 1 } else { types };
                (0, HashMap::new())
            }
            None => (2, HashMap::new()),
        }
    }

    async fn start(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath, _app_id: String, _parent_window: String, _options: HashMap<String, OwnedValue>) -> Answer {
        let session = session_handle.to_string();
        let Some((types, cursor)) = self.sessions.lock().unwrap().get(&session).map(|c| (c.types, c.cursor == 2)) else {
            return (2, HashMap::new());
        };
        // The scene chooses (not forever: a scene of one's own may not know how).
        let source = match choose(types).await {
            Some(Picked::Screen(n)) if types & 1 != 0 => Source::Monitor(n),
            Some(Picked::Window(slot)) if types & 2 != 0 => Source::Window(slot),
            // Turned down (or asked again by someone else): 1, the user said no.
            _ => return (1, HashMap::new()),
        };
        if !self.sessions.lock().unwrap().contains_key(&session) {
            return (2, HashMap::new());
        }
        let (reply, answer) = async_channel::bounded(1);
        tell(Msg::Start { session: session.clone(), source, cursor, reply });
        let Ok(Some((node, px))) = answer.recv().await else { return (2, HashMap::new()) };
        let props: HashMap<String, OwnedValue> = match source {
            Source::Monitor(n) => {
                let Some(m) = layers::monitors().get(n).cloned() else { return (2, HashMap::new()) };
                println!("portal · sharing {} (PipeWire node {node})", m.name);
                let size = ((m.size.0 as f64 / m.scale).round() as i32, (m.size.1 as f64 / m.scale).round() as i32);
                HashMap::from([
                    ("position".to_owned(), owned(ZValue::from((m.x, m.y)))),
                    ("size".to_owned(), owned(ZValue::from(size))),
                    ("source_type".to_owned(), owned(ZValue::from(1u32))),
                ])
            }
            Source::Window(slot) => {
                println!("portal · sharing window {slot} (PipeWire node {node})");
                HashMap::from([
                    ("size".to_owned(), owned(ZValue::from((px.0 as i32, px.1 as i32)))),
                    ("source_type".to_owned(), owned(ZValue::from(2u32))),
                ])
            }
        };
        let streams = vec![(node, props)];
        (0, HashMap::from([("streams".to_owned(), owned(ZValue::from(streams)))]))
    }

    /// Monitors (1) and windows (2).
    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        1 | 2
    }

    /// Hidden (1) and embedded (2): what programs ask for most.
    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        1 | 2
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        5
    }
}

/// A session the portal opened: closing it stops its stream.
struct Session {
    path: String,
    sessions: Sessions,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(&self, #[zbus(object_server)] server: &zbus::ObjectServer) {
        self.sessions.lock().unwrap().remove(&self.path);
        SHORTCUTS.lock().unwrap().retain(|s| s.session != self.path);
        if let Some(a) = APPS.lock().unwrap().as_mut() {
            a.remove(&self.path);
        }
        // Closed while the scene was still choosing (the program gave up): it stops.
        if CHOOSING.lock().unwrap().is_some() {
            picked(None);
        }
        tell(Msg::Stop { session: self.path.clone() });
        let _ = server.remove::<Session, _>(self.path.as_str()).await;
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

// ── PipeWire: the streams ─────────────────────────────────────────

/// A stream's side of things, shared with its callbacks.
struct Feed {
    source: Source,
    size: (u32, u32),
    /// The picture waiting to go out, and the one asked for, not yet taken.
    ready: Option<(Vec<u8>, (u32, u32))>,
    /// The size the stream agreed on with whoever watches: a picture of
    /// another size waits for the next agreement (a window that grew).
    agreed: Option<(u32, u32)>,
    /// The pointer drawn into it: the last picture without it, to draw it
    /// again where it has moved to, and the move it was drawn at.
    cursor: bool,
    clean: Option<(Vec<u8>, (u32, u32))>,
    moves: u64,
    asked: Option<u64>,
    streaming: bool,
    /// Who waits for its node (the D-Bus `Start`).
    reply: Option<async_channel::Sender<Option<(u32, (u32, u32))>>>,
}

impl Feed {
    /// A new picture: kept as it is, and with the pointer on it if it goes.
    fn take(&mut self, mut pixels: Vec<u8>, size: (u32, u32)) {
        if self.cursor {
            self.clean = Some((pixels.clone(), size));
            self.moves = layers::pointer_seen().map_or(0, |p| p.moves);
            pointer_onto(self.source, &mut pixels, size);
        }
        self.ready = Some((pixels, size));
    }

    /// The pointer moved and nothing else: the last picture again, with it
    /// where it is now. Whether there is one to send.
    fn pointer_moved(&mut self) -> bool {
        let moves = layers::pointer_seen().map_or(0, |p| p.moves);
        if !self.cursor || !self.streaming || moves == self.moves {
            return false;
        }
        let Some((pixels, size)) = self.clean.clone() else { return false };
        self.take(pixels, size);
        true
    }
}

/// The pointer drawn onto a picture of a monitor or a window, where it is on
/// it (BGRx; the pointer's picture is premultiplied).
fn pointer_onto(source: Source, pixels: &mut [u8], (w, h): (u32, u32)) {
    let Some(p) = layers::pointer_seen() else { return };
    let Some(picture) = p.picture else { return };
    let monitors = layers::monitors();
    // Where its tip is, in the picture's pixels.
    let tip = match source {
        Source::Monitor(n) => monitors.get(n).map(|m| ((p.at.0 - m.x as f64) * m.scale, (p.at.1 - m.y as f64) * m.scale)),
        Source::Window(slot) => layers::shown(slot).and_then(|(name, r)| {
            let m = monitors.iter().find(|m| m.name == name)?;
            let (x, y) = ((p.at.0 - m.x as f64) * m.scale - r[0] as f64, (p.at.1 - m.y as f64) * m.scale - r[1] as f64);
            (r[2] > 0 && r[3] > 0).then(|| (x * w as f64 / r[2] as f64, y * h as f64 / r[3] as f64))
        }),
    };
    let Some(tip) = tip else { return };
    layers::stamp_pointer(&picture, tip, pixels, (w, h));
}

struct Cast {
    stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<Rc<RefCell<Feed>>>,
    feed: Rc<RefCell<Feed>>,
}

/// The next picture of its monitor: the first at once, the rest when
/// something on it changes (nothing moves, nothing is sent).
fn ask(feed: &mut Feed, at_once: bool) {
    // A window's pictures come by themselves, each time it draws.
    let Source::Monitor(monitor) = feed.source else { return };
    if feed.asked.is_some() {
        return;
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    feed.asked = Some(id);
    // Its owner is nobody's: every change counts (the scene's are said as 0).
    layers::capture(monitor, id, [0, 0, feed.size.0 as i32, feed.size.1 as i32], !at_once, u64::MAX);
}

fn pod(object: Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).map(|(c, _)| c.into_inner()).unwrap_or_default()
}

fn prop(key: u32, value: Value) -> Property {
    Property { key, flags: PropertyFlags::empty(), value }
}

/// What it offers: BGRx at the monitor's size, at any pace up to its own.
fn format(size: (u32, u32), hz: u32) -> Vec<u8> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    pod(Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: vec![
            prop(FormatProperties::MediaType.as_raw(), Value::Id(Id(MediaType::Video.as_raw()))),
            prop(FormatProperties::MediaSubtype.as_raw(), Value::Id(Id(MediaSubtype::Raw.as_raw()))),
            prop(FormatProperties::VideoFormat.as_raw(), Value::Id(Id(spa::param::video::VideoFormat::BGRx.as_raw()))),
            prop(FormatProperties::VideoSize.as_raw(), Value::Rectangle(Rectangle { width: size.0, height: size.1 })),
            prop(
                FormatProperties::VideoFramerate.as_raw(),
                Value::Choice(spa::pod::ChoiceValue::Fraction(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: Fraction { num: hz, denom: 1 }, min: Fraction { num: 0, denom: 1 }, max: Fraction { num: hz.max(1), denom: 1 } }))),
            ),
        ],
    })
}

/// How its buffers are: one block of the whole picture, in shared memory.
fn buffers(size: (u32, u32)) -> Vec<u8> {
    let stride = size.0 as i32 * 4;
    pod(Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            prop(spa::sys::SPA_PARAM_BUFFERS_buffers, Value::Choice(spa::pod::ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: 4, min: 2, max: 8 })))),
            prop(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            prop(spa::sys::SPA_PARAM_BUFFERS_size, Value::Int(stride * size.1 as i32)),
            prop(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride)),
            prop(spa::sys::SPA_PARAM_BUFFERS_dataType, Value::Choice(spa::pod::ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Flags { default: 1 << spa::sys::SPA_DATA_MemFd, flags: vec![] })))),
        ],
    })
}

fn pipewire_thread(rx: pw::channel::Receiver<Msg>) -> Result<(), pw::Error> {
    pw::init();
    let main = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main, None)?;
    let core = context.connect_rc(None)?;
    let casts: Rc<RefCell<HashMap<String, Cast>>> = Default::default();
    let c = casts.clone();
    // Windows chosen whose first picture has not come yet: its size is the stream's.
    let waiting: RefCell<HashMap<String, (usize, bool, async_channel::Sender<Option<(u32, (u32, u32))>>)>> = RefCell::new(HashMap::new());
    let _attached = rx.attach(main.loop_(), move |msg| match msg {
        Msg::Start { session, source: Source::Monitor(n), cursor, reply } => {
            let Some(m) = layers::monitors().get(n).cloned() else {
                let _ = reply.try_send(None);
                return;
            };
            match open(&core, &session, Source::Monitor(n), cursor, m.size, (m.mhz.max(1000) as u32 + 500) / 1000, reply.clone()) {
                Ok(cast) => {
                    c.borrow_mut().insert(session, cast);
                    sharing(c.borrow().len());
                }
                Err(e) => {
                    eprintln!("portal · a stream could not be made: {e}");
                    let _ = reply.try_send(None);
                }
            }
        }
        Msg::Start { session, source: Source::Window(slot), cursor, reply } => {
            // Its pictures, each time it draws, from the render to here.
            let (tx, pictures) = std::sync::mpsc::channel::<WindowPicture>();
            render(ToRender::WatchWindow(slot, SHARE, Some(tx)));
            let to_pw = TO_PW.lock().unwrap().clone();
            let name = session.clone();
            let _ = std::thread::Builder::new().name("portal-window".into()).spawn(move || {
                let Some(to_pw) = to_pw else { return };
                for picture in pictures {
                    if to_pw.send(Msg::Window { session: name.clone(), picture }).is_err() {
                        return;
                    }
                }
            });
            waiting.borrow_mut().insert(session, (slot, cursor, reply));
        }
        Msg::Window { session, picture } => {
            let first = waiting.borrow_mut().remove(&session);
            if let Some((slot, cursor, reply)) = first {
                match open(&core, &session, Source::Window(slot), cursor, picture.size, 60, reply.clone()) {
                    Ok(cast) => {
                        cast.feed.borrow_mut().take(std::sync::Arc::unwrap_or_clone(picture.pixels), picture.size);
                        c.borrow_mut().insert(session, cast);
                        sharing(c.borrow().len());
                    sharing(c.borrow().len());
                    }
                    Err(e) => {
                        eprintln!("portal · a stream could not be made: {e}");
                        let _ = reply.try_send(None);
                        render(ToRender::WatchWindow(slot, SHARE, None));
                    }
                }
                return;
            }
            let casts = c.borrow();
            let Some(cast) = casts.get(&session) else { return };
            let mut feed = cast.feed.borrow_mut();
            // Another size: the stream says so, and whoever watches takes the new one.
            if picture.size != feed.size {
                feed.size = picture.size;
                let bytes = format(picture.size, 60);
                if let Some(p) = Pod::from_bytes(&bytes) {
                    let _ = cast.stream.update_params(&mut [p]);
                }
            }
            feed.take(std::sync::Arc::unwrap_or_clone(picture.pixels), picture.size);
            let streaming = feed.streaming;
            drop(feed);
            if streaming {
                let _ = cast.stream.trigger_process();
            }
        }
        // The pointer moving over what does not change.
        Msg::Tick => {
            for cast in c.borrow().values() {
                let moved = cast.feed.borrow_mut().pointer_moved();
                if moved {
                    let _ = cast.stream.trigger_process();
                }
            }
        }
        Msg::Stop { session } => {
            let first = waiting.borrow_mut().remove(&session);
            if let Some((slot, _, reply)) = first {
                render(ToRender::WatchWindow(slot, SHARE, None));
                let _ = reply.try_send(None);
            }
            let removed = c.borrow_mut().remove(&session);
            if let Some(cast) = removed {
                let feed = cast.feed.borrow();
                if let Some(id) = feed.asked {
                    layers::uncapture(id);
                }
                if let Source::Window(slot) = feed.source {
                    render(ToRender::WatchWindow(slot, SHARE, None));
                }
                if feed.cursor {
                    pointer_streams(false);
                }
                drop(feed);
                let _ = cast.stream.disconnect();
                sharing(c.borrow().len());
                println!("portal · stopped sharing");
            }
        }
        Msg::Picture { id, pixels } => {
            let casts = c.borrow();
            let Some(cast) = casts.values().find(|k| k.feed.borrow().asked == Some(id)) else { return };
            let mut feed = cast.feed.borrow_mut();
            feed.asked = None;
            let size = feed.size;
            // Only a picture of the stream's size: the monitor it shared may
            // have gone, and another one now has its number.
            if let Some(mut px) = pixels.filter(|px| px.len() == size.0 as usize * size.1 as usize * 4) {
                if let Source::Monitor(n) = feed.source {
                    hide_private(n, &mut px, size);
                }
                feed.take(px, size);
                drop(feed);
                let _ = cast.stream.trigger_process();
                feed = cast.feed.borrow_mut();
            }
            // A monitor that was unplugged answers at once that it has nothing:
            // asked again and again, the two threads spun a core between them.
            let there = match feed.source {
                Source::Monitor(n) => n < layers::monitors().len(),
                _ => true,
            };
            if feed.streaming && there {
                ask(&mut feed, false);
            }
        }
    });
    main.run();
    Ok(())
}

fn open(core: &pw::core::CoreRc, session: &str, source: Source, cursor: bool, size: (u32, u32), hz: u32, reply: async_channel::Sender<Option<(u32, (u32, u32))>>) -> Result<Cast, pw::Error> {
    let stream = pw::stream::StreamRc::new(
        core.clone(),
        "pleamar-wm",
        pw::properties::properties! {
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::MEDIA_NAME => "pleamar-wm screen",
            *pw::keys::NODE_DESCRIPTION => session,
        },
    )?;
    let feed = Rc::new(RefCell::new(Feed { source, size, ready: None, agreed: None, cursor, clean: None, moves: 0, asked: None, streaming: false, reply: Some(reply) }));
    let listener = stream
        .add_local_listener_with_user_data(feed.clone())
        .state_changed(|stream, feed, _, new| {
            let mut f = feed.borrow_mut();
            if let Some(reply) = f.reply.take() {
                match new {
                    pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming => {
                        let _ = reply.try_send(Some((stream.node_id(), f.size)));
                    }
                    pw::stream::StreamState::Error(_) => {
                        let _ = reply.try_send(None);
                    }
                    _ => f.reply = Some(reply),
                }
            }
            f.streaming = matches!(new, pw::stream::StreamState::Streaming);
            if f.streaming {
                ask(&mut f, true);
                // A window's picture that came before anyone watched: it goes now.
                if f.ready.is_some() {
                    drop(f);
                    let _ = stream.trigger_process();
                }
            }
        })
        .param_changed(|stream, feed, id, param| {
            let Some(param) = param.filter(|_| id == spa::param::ParamType::Format.as_raw()) else { return };
            // The size agreed: the buffers are made for it, and a picture of
            // that size that was waiting goes now.
            let mut info = spa::param::video::VideoInfoRaw::default();
            if info.parse(param).is_err() {
                return;
            }
            let agreed = (info.size().width, info.size().height);
            feed.borrow_mut().agreed = Some(agreed);
            let bytes = buffers(agreed);
            if let Some(p) = Pod::from_bytes(&bytes) {
                let _ = stream.update_params(&mut [p]);
            }
            if feed.borrow().ready.as_ref().is_some_and(|(_, s)| *s == agreed) {
                let _ = stream.trigger_process();
            }
        })
        .process(|stream, feed| {
            let mut f = feed.borrow_mut();
            // Only a picture of the size agreed fits the buffers: another one
            // waits for its agreement (it was asked for already).
            let Some(agreed) = f.agreed else { return };
            if !f.ready.as_ref().is_some_and(|(_, s)| *s == agreed) {
                return;
            }
            let Some((pixels, (w, h))) = f.ready.take() else { return };
            drop(f);
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else { return };
            let stride = w as usize * 4;
            if let Some(dst) = data.data() {
                let n = (stride * h as usize).min(dst.len()).min(pixels.len());
                dst[..n].copy_from_slice(&pixels[..n]);
            }
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (stride * h as usize) as u32;
        })
        .register()?;
    let bytes = format(size, hz);
    let mut params = [Pod::from_bytes(&bytes).ok_or(pw::Error::CreationFailed)?];
    stream.connect(spa::utils::Direction::Output, None, pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)?;
    // Counted only once it is sure to be there: its `Stop` uncounts it.
    if cursor {
        pointer_streams(true);
    }
    Ok(Cast { stream, _listener: listener, feed })
}
