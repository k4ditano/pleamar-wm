//! What the compositor inside and the monitors share when there are monitors
//! of our own (the session, or headless): which monitors there are, and the
//! programs' layer-shell surfaces —Marea, a bar, a wallpaper— that are put
//! together on them next to the scene's.
//!
//! The compositor tells a monitor what a program's surface shows
//! (`show`); the monitor reads it straight from the program's buffer when it
//! puts itself together, and hands the buffer back once it no longer needs it
//! (`ToLayers::Released`). The input on those surfaces goes from the session to
//! the compositor directly, without passing through the scene.

use crate::screen::Screen;
use pleamar::scene::PieceContent;
use smithay::reexports::calloop::channel;
use std::sync::Mutex;

/// A monitor as the programs see it: its name (`DP-3`), size, where it is on
/// the desktop (its top left corner), and its refresh in mHz.
#[derive(Clone, Debug)]
pub struct MonitorInfo {
    pub name: String,
    pub size: (u32, u32),
    pub x: i32,
    pub y: i32,
    pub mhz: i32,
    /// How many pixels a unit of the desktop is (1, 1.5, 2): `size` is in
    /// pixels, `x` and `y` in units.
    pub scale: f64,
}


/// What the session and the monitors tell the compositor inside.
#[derive(Debug)]
pub enum ToLayers {
    /// A program to start (a key binding's `launch`).
    Launch(String),
    /// The pointer on a program's surface, in its coordinates.
    Pointer { id: u64, x: f64, y: f64 },
    /// No longer on any.
    PointerOut,
    /// A button (the evdev code) or the wheel, on the one under the pointer.
    Button { code: u32, down: bool },
    Wheel(f32),
    /// A key for that one: it takes the keyboard if it did not have it.
    Key { id: u64, code: u32, down: bool },
    /// The keyboard goes back to the windows.
    KeyboardBack,
    /// A monitor (by its name) was put together with what the programs on it
    /// had drawn: they may draw again.
    FrameDone(String),
    /// Buffers a monitor no longer reads.
    Released(Vec<u64>),
    /// The monitors changed (one was plugged in or out): see `monitors()`.
    Monitors,
    /// A picture a program asked for (wlr-screencopy): its pixels, BGRA,
    /// row after row with no padding; none if it could not be taken.
    Captured { id: u64, pixels: Option<Vec<u8>> },
    /// The mouse moved, as it moved (a game that locked the pointer reads
    /// this): accelerated and not, and when, in µs.
    Relative { dx: f64, dy: f64, ux: f64, uy: f64, utime: u64 },
    /// Someone touched something: not idle.
    Activity,
    /// A button went down on the scene (not on a program's surface): menus
    /// that are not under the pointer close.
    ScenePress,
    /// Monitors went on or off (see `powered`).
    Power,
}

/// What a program holding the pointer asks of it: that it stays where it is
/// (a game looking around), or inside its window (on the desktop, x, y, w, h).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hold {
    Locked,
    Confined(Option<[i32; 4]>),
}

static HOLD: Mutex<Option<Hold>> = Mutex::new(None);

/// The pointer as the card shows it —its picture, 64 × 64 BGRA premultiplied,
/// and its tip— and where it is on the desktop, in units; with a count of
/// its moves. For whoever puts it into a picture (sharing the screen).
#[derive(Clone, Default)]
pub struct PointerSeen {
    pub picture: Option<std::sync::Arc<(Vec<u8>, (i32, i32))>>,
    pub at: (f64, f64),
    pub moves: u64,
}

static POINTER_SEEN: Mutex<Option<PointerSeen>> = Mutex::new(None);

pub fn set_pointer_picture(picture: Option<std::sync::Arc<(Vec<u8>, (i32, i32))>>) {
    let mut p = POINTER_SEEN.lock().unwrap();
    let seen = p.get_or_insert_with(PointerSeen::default);
    seen.picture = picture;
    seen.moves += 1;
}

pub fn set_pointer_at(at: (f64, f64)) {
    {
        let mut p = POINTER_SEEN.lock().unwrap();
        let seen = p.get_or_insert_with(PointerSeen::default);
        if seen.at == at {
            return;
        }
        seen.at = at;
        seen.moves += 1;
    }
    pointer_moved();
}

/// The pictures that wait for the screen to change and want the pointer in
/// them (a recorder that asks only for what changes). The pointer is on the
/// card's own plane: its moving puts no monitor together, so for these it
/// has to count as a change.
static POINTER_WAITS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// That picture, asked for with `capture(…, on_change: true, …)`, wants the pointer in it.
pub fn wait_pointer(id: u64) {
    POINTER_WAITS.lock().unwrap().push(id);
}

/// It was taken, or let go.
pub fn unwait_pointer(id: u64) {
    POINTER_WAITS.lock().unwrap().retain(|w| *w != id);
}

/// The pictures that waited for a change with the pointer in them are due now.
fn pointer_moved() {
    let ids = std::mem::take(&mut *POINTER_WAITS.lock().unwrap());
    if ids.is_empty() {
        return;
    }
    for (_, sc) in MONITORS.lock().unwrap().iter() {
        let (lock, cv) = &**sc;
        let mut st = lock.lock().unwrap();
        let mut due = false;
        for c in st.captures.iter_mut().filter(|c| c.2 && ids.contains(&c.0)) {
            c.2 = false;
            due = true;
        }
        if due {
            st.dirty = true;
            cv.notify_all();
        }
    }
}

/// The pointer's picture drawn onto a picture (BGRx; the pointer's is
/// premultiplied), its tip at `tip`, in that picture's pixels.
pub fn stamp_pointer(pointer: &(Vec<u8>, (i32, i32)), (tx, ty): (f64, f64), pixels: &mut [u8], (w, h): (u32, u32)) {
    if tx < 0.0 || ty < 0.0 || tx >= w as f64 || ty >= h as f64 {
        return;
    }
    let (image, (hx, hy)) = pointer;
    let (ox, oy) = (tx as i32 - hx, ty as i32 - hy);
    for y in 0..64i32 {
        let py = oy + y;
        if py < 0 || py >= h as i32 {
            continue;
        }
        for x in 0..64i32 {
            let px = ox + x;
            if px < 0 || px >= w as i32 {
                continue;
            }
            let s = ((y * 64 + x) * 4) as usize;
            let a = image[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = ((py as u32 * w + px as u32) * 4) as usize;
            for k in 0..3 {
                pixels[d + k] = (image[s + k] as u32 + pixels[d + k] as u32 * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

/// The pointer drawn onto the picture of a piece of a monitor (x, y, w, h, in
/// its pixels), if it is there: what a program that records the screen asks
/// for (wlr-screencopy's `overlay_cursor`).
pub fn pointer_onto_piece(monitor: usize, piece: [i32; 4], pixels: &mut [u8]) {
    let Some(p) = pointer_seen() else { return };
    let Some(picture) = p.picture else { return };
    let Some(m) = monitors().into_iter().nth(monitor) else { return };
    let tip = ((p.at.0 - m.x as f64) * m.scale - piece[0] as f64, (p.at.1 - m.y as f64) * m.scale - piece[1] as f64);
    stamp_pointer(&picture, tip, pixels, (piece[2].max(0) as u32, piece[3].max(0) as u32));
}

/// None where there is no pointer of the card's (headless).
pub fn pointer_seen() -> Option<PointerSeen> {
    POINTER_SEEN.lock().unwrap().clone()
}

/// Where each window is seen: on which monitor (its name) and its box there, in pixels.
static SHOWN: Mutex<Vec<Option<(String, [i32; 4])>>> = Mutex::new(Vec::new());

pub fn set_shown(slot: usize, monitor: String, rect: [i32; 4]) {
    let mut s = SHOWN.lock().unwrap();
    if s.len() <= slot {
        s.resize(slot + 1, None);
    }
    s[slot] = Some((monitor, rect));
}

pub fn shown(slot: usize) -> Option<(String, [i32; 4])> {
    SHOWN.lock().unwrap().get(slot).cloned().flatten().filter(|(m, _)| !m.is_empty())
}

/// The windows a rule calls private (`window app=… private`): never seen
/// in what is shared of a whole monitor, nor in a screenshot of it.
static PRIVATE: Mutex<Vec<bool>> = Mutex::new(Vec::new());

pub fn set_private(slot: usize, yes: bool) {
    let mut p = PRIVATE.lock().unwrap();
    if p.len() <= slot {
        p.resize(slot + 1, false);
    }
    p[slot] = yes;
}

/// Where the private windows are seen on that monitor, in its pixels.
pub fn private_on(monitor: &str) -> Vec<[i32; 4]> {
    let private = PRIVATE.lock().unwrap().clone();
    let shown = SHOWN.lock().unwrap();
    private.iter().enumerate().filter(|(_, p)| **p).filter_map(|(k, _)| shown.get(k).cloned().flatten()).filter(|(m, _)| m == monitor).map(|(_, r)| r).collect()
}

/// A program dragging something: its icon, one surface per monitor (which
/// one, and its id there), and where the pointer is on the desktop, in units.
static DRAG: Mutex<(Vec<(usize, u64)>, (f64, f64))> = Mutex::new((Vec::new(), (0.0, 0.0)));

/// Whether a program is dragging something (its icon is up): the pointer then
/// goes to whatever is under it, not to the surface the press began on.
pub fn dragging() -> bool {
    !DRAG.lock().unwrap().0.is_empty()
}

pub fn set_drag_icon(ids: Vec<(usize, u64)>) {
    DRAG.lock().unwrap().0 = ids;
}

/// Where the drag icon goes on that monitor, in its pixels: at the pointer
/// if the pointer is on it, far away if not.
pub fn drag_rect(monitor: usize, w: i32, h: i32) -> [i32; 4] {
    let pos = DRAG.lock().unwrap().1;
    let monitors = MONITORS.lock().unwrap();
    match monitors.get(monitor) {
        Some((m, _)) => rect_on(m, pos, w, h),
        None => [-100_000, -100_000, w, h],
    }
}

fn rect_on(m: &MonitorInfo, pos: (f64, f64), w: i32, h: i32) -> [i32; 4] {
    let (lw, lh) = (m.size.0 as f64 / m.scale, m.size.1 as f64 / m.scale);
    let (x, y) = (pos.0 - m.x as f64, pos.1 - m.y as f64);
    if x >= 0.0 && y >= 0.0 && x < lw && y < lh { [(x * m.scale) as i32, (y * m.scale) as i32, w, h] } else { [-100_000, -100_000, w, h] }
}

/// The pointer moved on the desktop (units): a drag icon goes with it.
pub fn move_drag(pos: (f64, f64)) {
    let ids = {
        let mut d = DRAG.lock().unwrap();
        d.1 = pos;
        if d.0.is_empty() {
            return;
        }
        d.0.clone()
    };
    // The monitors as they are, copied: with a monitor's lock held, `MONITORS`
    // is not to be asked for. Whoever has it (a window's buffers let go,
    // `forget`; one that closes, `hide`) goes on to take each monitor's lock,
    // and the two waited for each other forever: dragging a file over a window
    // that was redrawing froze the whole session.
    let monitors: Vec<(MonitorInfo, Screen)> = MONITORS.lock().unwrap().iter().map(|(m, s)| (m.clone(), s.clone())).collect();
    for (k, id) in ids {
        let Some((m, sc)) = monitors.get(k) else { continue };
        let (lock, cv) = &**sc;
        let mut st = lock.lock().unwrap();
        let Some(i) = st.clients.iter().position(|c| c.id == id) else { continue };
        let old = st.clients[i].rect;
        let new = rect_on(m, pos, old[2], old[3]);
        if new != old {
            st.clients[i].rect = new;
            st.note_change(old, 0);
            st.note_change(new, 0);
            st.dirty = true;
            cv.notify_all();
        }
    }
}

/// Which monitors have a fullscreen window: there, other programs' bars step aside.
pub fn set_fullscreen(on: &[bool]) {
    for (k, (_, sc)) in MONITORS.lock().unwrap().iter().enumerate() {
        let yes = on.get(k).copied().unwrap_or(false);
        let mut st = sc.0.lock().unwrap();
        if st.fullscreen != yes {
            st.fullscreen = yes;
            st.dirty = true;
            st.changed_all = true;
            sc.1.notify_all();
        }
    }
}

pub fn set_pointer_hold(h: Option<Hold>) {
    *HOLD.lock().unwrap() = h;
}

pub fn pointer_hold() -> Option<Hold> {
    *HOLD.lock().unwrap()
}

/// Whether a program keeps the screen awake (idle-inhibit: a video playing).
static INHIBITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_inhibited(yes: bool) {
    INHIBITED.store(yes, std::sync::atomic::Ordering::Relaxed);
}

pub fn inhibited() -> bool {
    INHIBITED.load(std::sync::atomic::Ordering::Relaxed)
}

/// A program's surface on a monitor: where, at what level, and what it shows.
pub struct ClientLayer {
    pub id: u64,
    /// 0 background, 1 bottom, 2 top, 3 overlay, 4 the lock screen.
    pub level: u8,
    /// Where it is on the monitor.
    pub rect: [i32; 4],
    /// It and what hangs from it (subsurfaces, menus), from its corner.
    pub pieces: Vec<ClientPiece>,
    /// Where it takes the pointer, from its corner: in order, added or taken
    /// away (`true` adds). `None` is all of it.
    pub region: Option<Vec<(bool, [i32; 4])>>,
    /// 0 never takes the keyboard, 1 takes all of it, 2 takes it when clicked.
    pub keyboard: u8,
    /// Where what is behind it is shown blurred (ext-background-effect), from its corner.
    pub blur: Vec<[i32; 4]>,
    /// Which program it is (0: none known), so that its own changes do not
    /// count as «something changed behind» for its own pictures.
    pub owner: u64,
    /// Shown smaller than it is drawn (1: as it is): a panel wider than the
    /// phone's monitor, so that its middle fits (see `phone_zoom`). The
    /// pieces keep their own pixels; where they go and where the pointer
    /// touches them is scaled by this.
    pub zoom: f64,
}

pub struct ClientPiece {
    /// The program's surface it comes from.
    pub key: u64,
    pub at: (i32, i32),
    pub size: (u32, u32),
    /// Its own pixels (more than `size` if the program draws at the monitor's scale).
    pub px: (u32, u32),
    /// What it shows, if it is new; the monitor takes it when it puts itself together.
    pub content: Option<PieceContent>,
    /// The buffer on the card it shows, if it is one.
    pub buffer: Option<u64>,
    /// Without alpha (XRGB): whatever the alpha byte holds, it covers.
    pub opaque: bool,
}

impl ClientLayer {
    /// A piece's box on the monitor (its pixels, shown at `zoom`).
    pub fn piece_rect(&self, p: &ClientPiece) -> [i32; 4] {
        let z = self.zoom;
        [self.rect[0] + (p.at.0 as f64 * z).round() as i32, self.rect[1] + (p.at.1 as f64 * z).round() as i32, (p.size.0 as f64 * z).ceil() as i32, (p.size.1 as f64 * z).ceil() as i32]
    }

    /// A point of the monitor, in its own pixels (from its corner).
    pub fn local(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.rect[0] as f64) / self.zoom, (y - self.rect[1] as f64) / self.zoom)
    }

    /// Whether it takes the pointer at that point of the monitor.
    pub fn takes(&self, x: f64, y: f64) -> bool {
        let (lx, ly) = self.local(x, y);
        let inside = |r: &[i32; 4]| lx >= r[0] as f64 && ly >= r[1] as f64 && lx < (r[0] + r[2]) as f64 && ly < (r[1] + r[3]) as f64;
        let Some(root) = self.pieces.first() else { return false };
        // A menu hanging from it takes it wherever it is drawn.
        if self.pieces.iter().skip(1).any(|p| inside(&[p.at.0, p.at.1, p.size.0 as i32, p.size.1 as i32])) {
            return true;
        }
        if !inside(&[root.at.0, root.at.1, root.size.0 as i32, root.size.1 as i32]) {
            return false;
        }
        match &self.region {
            None => true,
            Some(rects) => rects.iter().fold(false, |on, (add, r)| if inside(r) { *add } else { on }),
        }
    }
}

static MONITORS: Mutex<Vec<(MonitorInfo, Screen)>> = Mutex::new(Vec::new());
/// Whether monitors of our own are coming (a session, or headless): the
/// compositor inside may start before they are known, and has to wait for them.
static EXPECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn expect_monitors() {
    EXPECTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The monitors, waiting a moment for them if they are coming and not here yet.
pub fn wait_monitors(most: std::time::Duration) -> Vec<MonitorInfo> {
    let start = std::time::Instant::now();
    while EXPECTED.load(std::sync::atomic::Ordering::Relaxed) && MONITORS.lock().unwrap().is_empty() && start.elapsed() < most {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    monitors()
}
static NEST: Mutex<Option<channel::Sender<ToLayers>>> = Mutex::new(None);
/// The card the session drives, for the compositor inside to import the
/// programs' sync points with (explicit sync).
static CARD: Mutex<Option<smithay::backend::drm::DrmDeviceFd>> = Mutex::new(None);
/// Where the cursor asked for goes: the session, which has the card's cursor
/// plane. `true` when a program's surface asks, `false` when the scene does.
static CURSOR: Mutex<Option<channel::Sender<(bool, pleamar::scene::Cursor)>>> = Mutex::new(None);

pub fn set_cursor_sink(tx: channel::Sender<(bool, pleamar::scene::Cursor)>) {
    *CURSOR.lock().unwrap() = Some(tx);
}

pub fn cursor(from_program: bool, c: pleamar::scene::Cursor) {
    if let Some(tx) = CURSOR.lock().unwrap().as_ref() {
        let _ = tx.send((from_program, c));
    }
}

/// Monitors turned on or off, as a program asks (wlr-output-power-management:
/// hypridle, swayidle, wlopm): which one (all, if none), and on or off.
static POWER: Mutex<Option<channel::Sender<(Option<usize>, bool)>>> = Mutex::new(None);

pub fn set_power_sink(tx: channel::Sender<(Option<usize>, bool)>) {
    *POWER.lock().unwrap() = Some(tx);
}

pub fn request_power(monitor: Option<usize>, on: bool) {
    if let Some(tx) = POWER.lock().unwrap().as_ref() {
        let _ = tx.send((monitor, on));
    }
}

/// The phone's monitor (`pleamar-wm remote` from a phone, see docs/phone.md):
/// put up with its size in pixels and its scale, or taken down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhoneWish {
    pub size: (u32, u32),
    pub scale: f64,
}

type PhoneSink = Box<dyn Fn(Option<PhoneWish>) -> bool + Send>;
static PHONE: Mutex<Option<PhoneSink>> = Mutex::new(None);

pub fn set_phone_sink(sink: PhoneSink) {
    *PHONE.lock().unwrap() = Some(sink);
}

/// Whether there is anyone to put a phone's monitor up: a session of our own
/// (or headless), not pleamar-wm inside another compositor.
pub fn request_phone(wish: Option<PhoneWish>) -> bool {
    PHONE.lock().unwrap().as_ref().is_some_and(|sink| sink(wish))
}

/// The name a phone's monitor has, to tell it from the real ones.
pub const PHONE_NAME: &str = "PHONE-1";

/// Which monitors are on, as the session last said.
static POWERED: Mutex<Vec<bool>> = Mutex::new(Vec::new());

pub fn set_powered(on: Vec<bool>) {
    *POWERED.lock().unwrap() = on;
}

pub fn powered(monitor: usize) -> bool {
    POWERED.lock().unwrap().get(monitor).copied().unwrap_or(true)
}

pub fn set_card(fd: smithay::backend::drm::DrmDeviceFd) {
    *CARD.lock().unwrap() = Some(fd);
}

pub fn card() -> Option<smithay::backend::drm::DrmDeviceFd> {
    CARD.lock().unwrap().clone()
}
/// Whether a lock screen holds the session (ext-session-lock).
static LOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn locked() -> bool {
    LOCKED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Locked, the monitors show only the lock screen's surfaces; they are all
/// put together again at once.
pub fn set_locked(yes: bool) {
    LOCKED.store(yes, std::sync::atomic::Ordering::Relaxed);
    for (_, sc) in MONITORS.lock().unwrap().iter() {
        let mut st = sc.0.lock().unwrap();
        st.dirty = true;
        st.changed_all = true;
        drop(st);
        sc.1.notify_all();
    }
}

/// The monitors of the session, left to right.
pub fn register(monitors: Vec<(MonitorInfo, Screen)>) {
    *MONITORS.lock().unwrap() = monitors;
}

pub fn monitors() -> Vec<MonitorInfo> {
    MONITORS.lock().unwrap().iter().map(|(m, _)| m.clone()).collect()
}

pub fn set_nest(tx: channel::Sender<ToLayers>) {
    *NEST.lock().unwrap() = Some(tx);
}

/// Tells the compositor inside, if there is one.
pub fn tell(m: ToLayers) {
    if let Some(tx) = NEST.lock().unwrap().as_ref() {
        let _ = tx.send(m);
    }
}

/// A program's surface shows this now on that monitor (and on no other).
/// What the monitor has not read yet and is still shown is carried over; a
/// buffer it never got to read goes back. The ones it did read, it hands back
/// itself once it no longer shows them.
pub fn show(monitor: usize, layer: ClientLayer) {
    let mut unread = Vec::new();
    let mut layer = Some(layer);
    let id = layer.as_ref().map_or(0, |l| l.id);
    for (k, (_, sc)) in MONITORS.lock().unwrap().iter().enumerate() {
        let (lock, cv) = &**sc;
        let mut st = lock.lock().unwrap();
        let before = st.clients.iter().position(|c| c.id == id);
        let old = before.map(|i| st.clients.remove(i));
        let mut new = if k == monitor { layer.take() } else { None };
        if old.is_none() && new.is_none() {
            continue;
        }
        // What changes on the monitor: where it was and where it is, if it
        // moved, grew, changed level or came or went; else only the pieces
        // with something new.
        let same_place = match (&old, &new) {
            (Some(o), Some(n)) => o.rect == n.rect && o.level == n.level && o.pieces.len() == n.pieces.len() && o.pieces.iter().zip(&n.pieces).all(|(a, b)| a.at == b.at && a.size == b.size),
            _ => false,
        };
        if same_place {
            if let Some(n) = &new {
                for p in n.pieces.iter().filter(|p| p.content.is_some()) {
                    st.note_change(n.piece_rect(p), n.owner);
                }
            }
        } else {
            for l in old.iter().chain(new.iter()) {
                for p in &l.pieces {
                    st.note_change(l.piece_rect(p), l.owner);
                }
            }
        }
        for p in old.into_iter().flat_map(|o| o.pieces) {
            let Some(content) = p.content else { continue };
            if let Some(n) = new.as_mut().and_then(|n| n.pieces.iter_mut().find(|n| n.key == p.key && n.buffer == p.buffer && n.content.is_none())) {
                n.content = Some(content);
                continue;
            }
            if let (PieceContent::Dmabuf(_), Some(b)) = (&content, p.buffer) {
                if !st.held.contains(&b) {
                    unread.push(b);
                }
            }
        }
        if let Some(n) = new {
            let at = before.unwrap_or(st.clients.len()).min(st.clients.len());
            st.clients.insert(at, n);
        }
        st.dirty = true;
        cv.notify_all();
    }
    if !unread.is_empty() {
        tell(ToLayers::Released(unread));
    }
}

/// A program's surface is gone.
pub fn hide(id: u64) {
    let mut unread = Vec::new();
    for (_, sc) in MONITORS.lock().unwrap().iter() {
        let (lock, cv) = &**sc;
        let mut st = lock.lock().unwrap();
        if let Some(i) = st.clients.iter().position(|c| c.id == id) {
            let old = st.clients.remove(i);
            for p in &old.pieces {
                st.note_change(old.piece_rect(p), old.owner);
            }
            for p in old.pieces {
                if let (Some(PieceContent::Dmabuf(_)), Some(b)) = (&p.content, p.buffer) {
                    if !st.held.contains(&b) {
                        unread.push(b);
                    }
                }
            }
            st.dirty = true;
            cv.notify_all();
        }
    }
    if !unread.is_empty() {
        tell(ToLayers::Released(unread));
    }
}

/// A picture of that piece of that monitor (x, y, w, h), for a program:
/// taken the next time it is put together.
/// With `on_change` (copy_with_damage), not before something in that piece
/// changes: it does not make the monitor be put together, it waits for it.
pub fn capture(monitor: usize, id: u64, piece: [i32; 4], on_change: bool, owner: u64) {
    let monitors = MONITORS.lock().unwrap();
    let Some((_, sc)) = monitors.get(monitor) else {
        drop(monitors);
        tell(ToLayers::Captured { id, pixels: None });
        return;
    };
    let (lock, cv) = &**sc;
    let mut st = lock.lock().unwrap();
    st.captures.push((id, piece, on_change, owner));
    if !on_change {
        st.dirty = true;
        cv.notify_all();
    }
}

/// A picture no longer wanted: its program let it go before it was taken.
pub fn uncapture(id: u64) {
    unwait_pointer(id);
    for (_, sc) in MONITORS.lock().unwrap().iter() {
        sc.0.lock().unwrap().captures.retain(|c| c.0 != id);
    }
}

/// The monitors stop putting themselves together (the process is leaving):
/// each finishes what it is doing with the card and ends.
pub fn stop_all() {
    for (_, sc) in MONITORS.lock().unwrap().iter() {
        sc.0.lock().unwrap().quit = true;
        sc.1.notify_all();
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
}

/// Buffers the programs destroyed: the monitors drop what they kept of them.
pub fn forget(buffers: &[u64]) {
    for (_, sc) in MONITORS.lock().unwrap().iter() {
        sc.0.lock().unwrap().forget.extend_from_slice(buffers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nowhere;
    impl crate::screen::Output for Nowhere {
        fn buffer(&mut self, _: &pleamar::wgpu::Device, _: &[u64]) -> Option<(usize, pleamar::wgpu::Texture)> {
            None
        }
        fn show(&mut self, _: usize, _: pleamar::Sent, _: &pleamar::wgpu::Device, _: &pleamar::wgpu::Queue, _: bool) -> bool {
            false
        }
    }

    /// Dragging a file while a window redraws: the pointer moves the drag
    /// icon (`move_drag`, the input thread) while the compositor lets go of a
    /// window's buffers (`forget`) and a window goes fullscreen. Both take
    /// `MONITORS` and each monitor's lock; in opposite orders they froze the
    /// session for good.
    #[test]
    fn dragging_while_windows_redraw_does_not_freeze() {
        let info = MonitorInfo { name: "A".into(), size: (1920, 1080), x: 0, y: 0, mhz: 60_000, scale: 1.0 };
        register(vec![(info, crate::screen::screen("A".into(), (1920, 1080), Box::new(Nowhere)))]);
        show(0, ClientLayer { id: 7, level: 3, rect: [0, 0, 32, 32], pieces: Vec::new(), region: Some(Vec::new()), keyboard: 0, blur: Vec::new(), owner: 0, zoom: 1.0 });
        set_drag_icon(vec![(0, 7)]);
        let (done, finished) = std::sync::mpsc::channel();
        let mover = {
            let done = done.clone();
            std::thread::spawn(move || {
                for k in 0..200_000 {
                    move_drag(((k % 1900) as f64, (k % 1000) as f64));
                }
                let _ = done.send(());
            })
        };
        let redraws = std::thread::spawn(move || {
            for k in 0..200_000u64 {
                forget(&[k]);
                if k % 64 == 0 {
                    set_fullscreen(&[k % 128 == 0]);
                }
            }
            let _ = done.send(());
        });
        for _ in 0..2 {
            finished.recv_timeout(std::time::Duration::from_secs(60)).expect("the two threads waited for each other: frozen");
        }
        mover.join().unwrap();
        redraws.join().unwrap();
        set_drag_icon(Vec::new());
    }
}
