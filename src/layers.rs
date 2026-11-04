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
    /// Fingers moving what is under the pointer, so many units (down and to
    /// the right, positive); and lifted.
    Scroll(f64, f64),
    ScrollEnd,
    /// A key for that one: it takes the keyboard if it did not have it.
    Key { id: u64, code: u32, down: bool },
    /// The keyboard goes back to the windows.
    KeyboardBack,
    /// A monitor (by its name) was put together with what the programs on it
    /// had drawn: they may draw again.
    FrameDone(String),
    /// A monitor's frame (by its name) reached the screen: its flip landed,
    /// on a refresh. The windows on it may draw the next one.
    Landed(String),
    /// Buffers a monitor no longer reads.
    Released(Vec<u64>),
    /// The monitors changed (one was plugged in or out): see `monitors()`.
    Monitors,
    /// A picture a program asked for (wlr-screencopy): its pixels, BGRA,
    /// row after row with no padding; none if it could not be taken.
    Captured { id: u64, pixels: Option<Vec<u8>> },
    /// The same picture, written by the card straight into the program's buffer on the card
    /// (linux-dmabuf): whether it was.
    CapturedOnCard { id: u64, ok: bool },
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
    /// The picture each monitor shows, at its own size (by its place in
    /// `monitors()`); where there is none, `picture`.
    pub each: Vec<Option<std::sync::Arc<(Vec<u8>, (i32, i32))>>>,
    pub at: (f64, f64),
    pub moves: u64,
}

impl PointerSeen {
    /// Its picture as that monitor shows it.
    pub fn picture_on(&self, monitor: usize) -> Option<std::sync::Arc<(Vec<u8>, (i32, i32))>> {
        self.each.get(monitor).cloned().flatten().or_else(|| self.picture.clone())
    }
}

static POINTER_SEEN: Mutex<Option<PointerSeen>> = Mutex::new(None);

/// One picture for every monitor.
pub fn set_pointer_picture(picture: Option<std::sync::Arc<(Vec<u8>, (i32, i32))>>) {
    set_pointer_pictures(picture, Vec::new());
}

/// A picture for whoever does not say which monitor, and each monitor's own.
pub fn set_pointer_pictures(picture: Option<std::sync::Arc<(Vec<u8>, (i32, i32))>>, each: Vec<Option<std::sync::Arc<(Vec<u8>, (i32, i32))>>>) {
    {
        let mut p = POINTER_SEEN.lock().unwrap();
        let seen = p.get_or_insert_with(PointerSeen::default);
        seen.picture = picture;
        seen.each = each;
        seen.moves += 1;
    }
    // A new shape, or the next frame of one, changes the pictures it is in.
    pointer_moved();
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
    let Some(picture) = p.picture_on(monitor) else { return };
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

/// A window's menu, as the compositor inside last placed it: its surface
/// on a monitor (`show`'s id), which monitor, and where it is from the
/// window's corner (its geometry's) and its size, in units.
#[derive(Clone, Debug)]
pub struct WindowMenu {
    pub id: u64,
    pub monitor: usize,
    pub at: (i32, i32),
    pub size: (u32, u32),
}

/// Each window's menus, with the window's own size in units, in the order
/// they are drawn: for its pictures (`window_menus`).
static MENUS: Mutex<Vec<((u32, u32), Vec<WindowMenu>)>> = Mutex::new(Vec::new());

pub fn set_menus(slot: usize, size: (u32, u32), menus: Vec<WindowMenu>) {
    let mut m = MENUS.lock().unwrap();
    if m.len() <= slot {
        m.resize(slot + 1, ((0, 0), Vec::new()));
    }
    m[slot] = (size, menus);
}

/// Whether a window has menus on a monitor (`window_menus` has something to read).
pub fn has_menus(slot: usize) -> bool {
    MENUS.lock().unwrap().get(slot).is_some_and(|(_, m)| !m.is_empty())
}

/// How many times each program's surface has changed its look (`show`).
static GENERATIONS: Mutex<std::collections::BTreeMap<u64, u64>> = Mutex::new(std::collections::BTreeMap::new());

/// Each menu's last picture read back, and the look it had then.
static READ: Mutex<std::collections::BTreeMap<u64, (u64, ClientPicture)>> = Mutex::new(std::collections::BTreeMap::new());

/// A window's menus read back from their monitors, with the window's size
/// in units, bottom to top; none if it has none. Only those whose look
/// changed since they were last read are read again, each monitor putting
/// itself together for them (a frame, a second at most in all; one that is
/// paused, not at all: the last picture of them stays). Not on the
/// compositor's loop.
pub fn window_menus(slot: usize) -> Option<((u32, u32), Vec<(WindowMenu, ClientPicture)>)> {
    let (size, menus) = MENUS.lock().unwrap().get(slot).cloned().filter(|(_, m)| !m.is_empty())?;
    let looks: Vec<u64> = {
        let g = GENERATIONS.lock().unwrap();
        menus.iter().map(|m| g.get(&m.id).copied().unwrap_or(0)).collect()
    };
    let stale: Vec<(usize, u64)> = {
        let read = READ.lock().unwrap();
        menus.iter().zip(&looks).filter(|(m, look)| read.get(&m.id).is_none_or(|(had, _)| had != *look)).map(|(m, _)| (m.monitor, m.id)).collect()
    };
    let mut monitors: Vec<usize> = stale.iter().map(|s| s.0).collect();
    monitors.sort_unstable();
    monitors.dedup();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
    for k in monitors {
        let (tx, rx) = std::sync::mpsc::channel();
        read_clients(k, stale.iter().filter(|s| s.0 == k).map(|s| s.1).collect(), tx);
        let got = rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now())).unwrap_or_default();
        let mut read = READ.lock().unwrap();
        for p in got {
            let Some(look) = menus.iter().position(|m| m.id == p.id).map(|i| looks[i]) else { continue };
            read.insert(p.id, (look, p));
        }
    }
    let read = READ.lock().unwrap();
    let pictures = menus.into_iter().filter_map(|m| read.get(&m.id).map(|(_, p)| p.clone()).map(|p| (m, p)));
    Some((size, pictures.collect()))
}

/// A window's picture from the render (cut to the window, at its own
/// density) with its menus over it, cut where it ends.
pub fn with_menus(slot: usize, picture: &mut [u8], size: (u32, u32)) {
    let Some(((uw, _), menus)) = window_menus(slot) else { return };
    let density = if uw > 0 { size.0 as f64 / uw as f64 } else { 1.0 };
    for (menu, its) in &menus {
        lay_menu(picture, size, (0, 0), density, menu, its);
    }
}

/// A menu laid over a window's picture (BGRA, premultiplied) of `size`
/// pixels whose corner is the window's at `origin` pixels, at `density`
/// pixels a unit: stretched to its size there, cut where the picture ends.
pub fn lay_menu(picture: &mut [u8], size: (u32, u32), origin: (i32, i32), density: f64, menu: &WindowMenu, its: &ClientPicture) {
    let (pw, ph) = its.size;
    if pw == 0 || ph == 0 {
        return;
    }
    let x0 = origin.0 + (menu.at.0 as f64 * density).round() as i32;
    let y0 = origin.1 + (menu.at.1 as f64 * density).round() as i32;
    let w = (menu.size.0 as f64 * density).round() as i32;
    let h = (menu.size.1 as f64 * density).round() as i32;
    for y in y0.max(0)..(y0 + h).min(size.1 as i32) {
        let sy = (((y - y0) as i64 * ph as i64) / h.max(1) as i64).min(ph as i64 - 1) as u32;
        for x in x0.max(0)..(x0 + w).min(size.0 as i32) {
            let sx = (((x - x0) as i64 * pw as i64) / w.max(1) as i64).min(pw as i64 - 1) as u32;
            let s = ((sy * pw + sx) * 4) as usize;
            let d = ((y as u32 * size.0 + x as u32) * 4) as usize;
            let a = its.pixels[s + 3] as u32;
            if a == 255 {
                picture[d..d + 4].copy_from_slice(&its.pixels[s..s + 4]);
            } else if a > 0 {
                for k in 0..4 {
                    picture[d + k] = (its.pixels[s + k] as u32 + picture[d + k] as u32 * (255 - a) / 255).min(255) as u8;
                }
            }
        }
    }
}

/// Whether a window is seen at that point of that monitor (its pixels).
pub fn window_at(monitor: &str, x: f64, y: f64) -> bool {
    SHOWN.lock().unwrap().iter().flatten().any(|(m, r)| m == monitor && x >= r[0] as f64 && y >= r[1] as f64 && x < (r[0] + r[2]) as f64 && y < (r[1] + r[3]) as f64)
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

pub fn private(slot: usize) -> bool {
    PRIVATE.lock().unwrap().get(slot).copied().unwrap_or(false)
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
    /// Where it is less opaque than this (0: anywhere), what is behind is not
    /// blurred: a blur it was given without asking, kept off its rounded corners.
    pub blur_cut: f32,
    /// Which program it is (0: none known), so that its own changes do not
    /// count as «something changed behind» for its own pictures.
    pub owner: u64,
    /// Shown smaller than it is drawn (1: as it is): a panel wider than the
    /// phone's monitor, so that its middle fits (see `phone_zoom`). The
    /// pieces keep their own pixels; where they go and where the pointer
    /// touches them is scaled by this.
    pub zoom: f64,
    /// Put together on the monitor. One that is not is still read and kept,
    /// for its pictures (`read_clients`): the menus of a window not shown.
    pub drawn: bool,
}

/// A program's surface on a monitor, read back (`read_clients`): its pixels
/// over transparent, BGRA, premultiplied, row after row with no padding.
#[derive(Clone)]
pub struct ClientPicture {
    pub id: u64,
    pub size: (u32, u32),
    pub pixels: Vec<u8>,
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

/// A program under the pointer asked for no cursor at all (a game, a film
/// full screen): the session hides it until one is asked for again.
static CURSOR_HIDDEN: Mutex<Option<channel::Sender<bool>>> = Mutex::new(None);
static HIDDEN_NOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_cursor_hidden_sink(tx: channel::Sender<bool>) {
    *CURSOR_HIDDEN.lock().unwrap() = Some(tx);
}

/// Told only when it changes: a program's cursor changes on every link.
pub fn hide_cursor(hidden: bool) {
    if HIDDEN_NOW.swap(hidden, std::sync::atomic::Ordering::Relaxed) == hidden {
        return;
    }
    if let Some(tx) = CURSOR_HIDDEN.lock().unwrap().as_ref() {
        let _ = tx.send(hidden);
    }
}

/// A cursor a program draws itself (a game's, GTK 3's from its theme): its
/// pixels as the card takes them (ARGB8888), their size, and its tip in them.
pub type CursorPicture = std::sync::Arc<((usize, usize), Vec<u8>, (i32, i32))>;
static CUSTOM_CURSOR: Mutex<Option<channel::Sender<Option<CursorPicture>>>> = Mutex::new(None);

pub fn set_custom_cursor_sink(tx: channel::Sender<Option<CursorPicture>>) {
    *CUSTOM_CURSOR.lock().unwrap() = Some(tx);
}

/// Each new picture of it, or none: back to the shapes the session has.
pub fn custom_cursor(picture: Option<CursorPicture>) {
    if let Some(tx) = CUSTOM_CURSOR.lock().unwrap().as_ref() {
        let _ = tx.send(picture);
    }
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

/// A monitor's colour ramps, as a program asks (wlr-gamma-control: wlsunset,
/// gammastep, a shell's night light): which one, and its red, green and blue
/// ramps one after the other; none, its own back.
static GAMMA: Mutex<Option<channel::Sender<(usize, Option<Vec<u16>>)>>> = Mutex::new(None);

pub fn set_gamma_sink(tx: channel::Sender<(usize, Option<Vec<u16>>)>) {
    *GAMMA.lock().unwrap() = Some(tx);
}

pub fn request_gamma(monitor: usize, ramps: Option<Vec<u16>>) {
    if let Some(tx) = GAMMA.lock().unwrap().as_ref() {
        let _ = tx.send((monitor, ramps));
    }
}

/// How many steps each monitor's ramps have, as the session last said: 0, it
/// has none a program can set (the phone's, a headless one, a card without).
static GAMMA_SIZES: Mutex<Vec<u32>> = Mutex::new(Vec::new());

pub fn set_gamma_sizes(sizes: Vec<u32>) {
    *GAMMA_SIZES.lock().unwrap() = sizes;
}

pub fn gamma_size(monitor: usize) -> u32 {
    GAMMA_SIZES.lock().unwrap().get(monitor).copied().unwrap_or(0)
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

/// Fingers from elsewhere than a device here (a phone's glass, through
/// `pleamar-wm remote`): how far they moved what is under the pointer, in
/// units (down and to the right, positive), or that they were lifted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fingers {
    Moved(f64, f64),
    Up,
}

type FingersSink = Box<dyn Fn(Fingers) -> bool + Send>;
static FINGERS: Mutex<Option<FingersSink>> = Mutex::new(None);

pub fn set_fingers_sink(sink: FingersSink) {
    *FINGERS.lock().unwrap() = Some(sink);
}

/// false: no session of our own to hand them to.
pub fn fingers(f: Fingers) -> bool {
    FINGERS.lock().unwrap().as_ref().is_some_and(|sink| sink(f))
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
/// How many times the keyboard has been given to a window (`set_focus`, in
/// nest.rs). The route (route.rs) keeps the program's surface that was clicked
/// for its keys; when a window takes the keyboard from it by other means (the
/// scene's focus on hover, a shortcut), that count moves and the surface is
/// no longer who the keys are for.
static WINDOW_KEYBOARD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn keyboard_to_window() {
    WINDOW_KEYBOARD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn window_keyboard() -> u64 {
    WINDOW_KEYBOARD.load(std::sync::atomic::Ordering::Relaxed)
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
    let left: Vec<Screen> = {
        let mut all = MONITORS.lock().unwrap();
        let left = all.iter().filter(|(_, sc)| !monitors.iter().any(|(_, now)| std::sync::Arc::ptr_eq(sc, now))).map(|(_, sc)| sc.clone()).collect();
        *all = monitors;
        left
    };
    // A monitor that leaves takes the programs' surfaces on it with it: they
    // are told they are closed, and `hide` no longer finds them there. A
    // monitor that comes back as the same one (a headless session's) came
    // back with them still on it: a lock screen from before the monitors
    // slept took the keys meant for the new one (#38).
    let mut unread = Vec::new();
    for sc in left {
        let (lock, cv) = &*sc;
        let mut st = lock.lock().unwrap();
        for old in std::mem::take(&mut st.clients) {
            GENERATIONS.lock().unwrap().remove(&old.id);
            READ.lock().unwrap().remove(&old.id);
            for p in old.pieces {
                if let (Some(PieceContent::Dmabuf(_)), Some(b)) = (&p.content, p.buffer) {
                    if !st.held.contains(&b) {
                        unread.push(b);
                    }
                }
            }
        }
        st.dirty = true;
        cv.notify_all();
    }
    if !unread.is_empty() {
        tell(ToLayers::Released(unread));
    }
}

/// One monitor moved on the desktop, the rest as they are (a headless
/// session's `pleamar-wm output … at`): false if there is none by that name.
pub fn place(name: &str, (x, y): (i32, i32)) -> bool {
    let mut all = MONITORS.lock().unwrap();
    let Some((m, _)) = all.iter_mut().find(|(m, _)| m.name == name) else { return false };
    (m.x, m.y) = (x, y);
    true
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
        let seen = note_shown(&mut st, old.as_ref(), new.as_ref());
        // Its look changed: its picture is read again (`window_menus`).
        if new.as_ref().is_some_and(|n| old.as_ref().is_none_or(|o| !same_place(o, n)) || n.pieces.iter().any(|p| p.content.is_some())) {
            *GENERATIONS.lock().unwrap().entry(id).or_default() += 1;
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
        // One not drawn before nor now changes nothing on the monitor: what it
        // brought is read the next time it is put together for something else,
        // or for its picture (`read_clients`).
        if seen {
            st.dirty = true;
            cv.notify_all();
        }
    }
    if !unread.is_empty() {
        tell(ToLayers::Released(unread));
    }
}

/// Whether two of the same surface are in the same place: the same box,
/// level and pieces, and drawn or not as it was.
fn same_place(o: &ClientLayer, n: &ClientLayer) -> bool {
    o.rect == n.rect && o.level == n.level && o.drawn == n.drawn && o.pieces.len() == n.pieces.len() && o.pieces.iter().zip(&n.pieces).all(|(a, b)| a.at == b.at && a.size == b.size)
}

/// What changes on the monitor when a surface goes from `old` to `new`:
/// where it was and where it is, if it moved, grew, changed level, came or
/// went, or was drawn or no longer; else only the pieces with something
/// new. And whether it is seen there at all, before or now.
fn note_shown(st: &mut crate::screen::ScreenState, old: Option<&ClientLayer>, new: Option<&ClientLayer>) -> bool {
    match (old, new) {
        (Some(o), Some(n)) if same_place(o, n) => {
            for p in n.pieces.iter().filter(|p| n.drawn && p.content.is_some()) {
                st.note_change(n.piece_rect(p), n.owner);
            }
        }
        _ => {
            for l in old.into_iter().chain(new).filter(|l| l.drawn) {
                for p in &l.pieces {
                    st.note_change(l.piece_rect(p), l.owner);
                }
            }
        }
    }
    old.is_some_and(|o| o.drawn) || new.is_some_and(|n| n.drawn)
}

/// A program's surface is gone.
pub fn hide(id: u64) {
    GENERATIONS.lock().unwrap().remove(&id);
    READ.lock().unwrap().remove(&id);
    let mut unread = Vec::new();
    for (_, sc) in MONITORS.lock().unwrap().iter() {
        let (lock, cv) = &**sc;
        let mut st = lock.lock().unwrap();
        if let Some(i) = st.clients.iter().position(|c| c.id == id) {
            let old = st.clients.remove(i);
            for p in old.pieces.iter().filter(|_| old.drawn) {
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

/// Those programs' surfaces on that monitor, read back the next time it is
/// put together, drawn there or not: the ones it does not have are left out.
/// None at once from a monitor that is paused (another TTY): it is not put
/// together until it is back.
pub fn read_clients(monitor: usize, ids: Vec<u64>, to: std::sync::mpsc::Sender<Vec<ClientPicture>>) {
    let monitors = MONITORS.lock().unwrap();
    let Some((_, sc)) = monitors.get(monitor) else {
        let _ = to.send(Vec::new());
        return;
    };
    let (lock, cv) = &**sc;
    let mut st = lock.lock().unwrap();
    if st.paused {
        let _ = to.send(Vec::new());
        return;
    }
    st.reads.push((ids, to));
    st.dirty = true;
    cv.notify_all();
}

/// Where the pictures that go onto the card are to be written, by picture.
static CARD_TARGETS: Mutex<Vec<(u64, pleamar::scene::DmabufPiece)>> = Mutex::new(Vec::new());

/// The same, into the program's buffer on the card (`target`): nothing is read
/// back, and the program's memory is not touched by the processor.
pub fn capture_card(monitor: usize, id: u64, piece: [i32; 4], on_change: bool, owner: u64, target: pleamar::scene::DmabufPiece) {
    CARD_TARGETS.lock().unwrap().push((id, target));
    capture(monitor, id, piece, on_change, owner);
}

/// Where picture `id` goes, if it goes onto the card.
pub fn take_card_target(id: u64) -> Option<pleamar::scene::DmabufPiece> {
    let mut all = CARD_TARGETS.lock().unwrap();
    let k = all.iter().position(|t| t.0 == id)?;
    Some(all.remove(k).1)
}

/// A picture no longer wanted: its program let it go before it was taken.
pub fn uncapture(id: u64) {
    CARD_TARGETS.lock().unwrap().retain(|t| t.0 != id);
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
pub(crate) mod tests {
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

    fn menu(drawn: bool, content: bool) -> ClientLayer {
        let piece = ClientPiece { key: 1, at: (0, 0), size: (40, 30), px: (40, 30), content: content.then_some(PieceContent::Kept), buffer: None, opaque: false };
        ClientLayer { id: 3, level: 3, rect: [100, 200, 40, 30], pieces: vec![piece], region: None, keyboard: 0, blur: Vec::new(), blur_cut: 0.0, owner: 0, zoom: 1.0, drawn }
    }

    /// What `show` notes on the monitor for one surface going from `old` to
    /// `new`, and whether the monitor is put together for it.
    fn noted(old: Option<ClientLayer>, new: Option<ClientLayer>) -> (Vec<[i32; 4]>, bool) {
        let sc = crate::screen::screen("A".into(), (1920, 1080), Box::new(Nowhere));
        let mut st = sc.0.lock().unwrap();
        st.changed_all = false;
        let seen = note_shown(&mut st, old.as_ref(), new.as_ref());
        (st.changed.iter().map(|c| c.0).collect(), seen)
    }

    /// A menu that is no longer drawn where it was (its window hidden) is
    /// taken off the monitor, and one drawn again where it was is put back,
    /// even with nothing new in it.
    #[test]
    fn a_menu_drawn_or_not_in_the_same_place_is_noted() {
        let whole = vec![[100, 200, 40, 30]];
        assert_eq!(noted(Some(menu(true, false)), Some(menu(false, false))), (whole.clone(), true));
        assert_eq!(noted(Some(menu(false, false)), Some(menu(true, false))), (whole.clone(), true));
        // Drawn and drawn: only what is new.
        assert_eq!(noted(Some(menu(true, false)), Some(menu(true, false))), (Vec::new(), true));
        assert_eq!(noted(Some(menu(true, false)), Some(menu(true, true))), (whole, true));
    }

    /// A menu not drawn, before nor now, puts nothing together: it brings
    /// what it draws, and the monitor stays as it is.
    #[test]
    fn a_menu_not_drawn_does_not_put_the_monitor_together() {
        assert_eq!(noted(Some(menu(false, false)), Some(menu(false, true))), (Vec::new(), false));
        assert_eq!(noted(None, Some(menu(false, true))), (Vec::new(), false));
        assert_eq!(noted(Some(menu(false, true)), None), (Vec::new(), false));
    }

    /// Dragging a file while a window redraws: the pointer moves the drag
    /// icon (`move_drag`, the input thread) while the compositor lets go of a
    /// window's buffers (`forget`) and a window goes fullscreen. Both take
    /// `MONITORS` and each monitor's lock; in opposite orders they froze the
    /// session for good.
    /// Held by a test that drags, and by one that would see the drag.
    pub static DRAGGING: Mutex<()> = Mutex::new(());

    #[test]
    fn dragging_while_windows_redraw_does_not_freeze() {
        let _drag = DRAGGING.lock().unwrap_or_else(|e| e.into_inner());
        let info = MonitorInfo { name: "A".into(), size: (1920, 1080), x: 0, y: 0, mhz: 60_000, scale: 1.0 };
        register(vec![(info, crate::screen::screen("A".into(), (1920, 1080), Box::new(Nowhere)))]);
        show(0, ClientLayer { id: 7, level: 3, rect: [0, 0, 32, 32], pieces: Vec::new(), region: Some(Vec::new()), keyboard: 0, blur: Vec::new(), blur_cut: 0.0, owner: 0, zoom: 1.0, drawn: true });
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
