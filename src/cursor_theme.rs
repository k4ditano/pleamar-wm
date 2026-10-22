//! The cursor theme's pictures, for the session's cursor plane and for the
//! pointer a screenless session draws into its pictures: the system's theme
//! (XCURSOR_THEME, or what ~/.icons/default inherits), each shape looked for
//! by its older names too, each picture's frames at the size closest to the
//! one asked for, in the plane's 64 × 64; and an arrow of our own where the
//! theme has none.

use pleamar::scene::Cursor;
use std::collections::HashSet;

/// One frame: its 64 × 64 pixels as the card takes them, its tip, and how
/// long it shows (ms; a shape of one frame stays).
pub type Frame = (Vec<u8>, (i32, i32), u32);

pub struct Theme {
    /// Its name, if there is one to read.
    pub name: Option<String>,
    theme: Option<xcursor::CursorTheme>,
    /// The pictures it lacks, not looked for again.
    missing: HashSet<String>,
}

impl Theme {
    pub fn load() -> Theme {
        let name = std::env::var("XCURSOR_THEME").ok().filter(|t| !t.is_empty()).or_else(|| {
            let home = std::env::var("HOME").ok()?;
            let text = std::fs::read_to_string(format!("{home}/.icons/default/index.theme")).ok()?;
            text.lines().find_map(|l| l.trim().strip_prefix("Inherits=")).map(|v| v.split(',').next().unwrap_or("").trim().to_owned())
        });
        let theme = name.as_deref().map(xcursor::CursorTheme::load);
        Theme { name, theme, missing: HashSet::new() }
    }

    /// A picture by its name (`pointer`, `default-to-pointer`), for each size
    /// asked for: the frames of the closest one the theme has, in order. None
    /// if it lacks it, which is remembered.
    pub fn frames(&mut self, name: &str, sizes: &[u32]) -> Option<Vec<(u32, Vec<Frame>)>> {
        if self.missing.contains(name) {
            return None;
        }
        let mut candidates = vec![name.to_owned()];
        if let Some(k) = KINDS.iter().find(|k| k.name() == name) {
            candidates.extend(alternatives(*k).iter().map(|n| n.to_string()));
        }
        if let Ok(icon) = name.parse::<smithay::input::pointer::CursorIcon>() {
            candidates.extend(icon.alt_names().iter().map(|n| n.to_string()));
        }
        let images = self.theme.as_ref().and_then(|t| {
            candidates.iter().find_map(|n| {
                let path = t.load_icon(n)?;
                let data = std::fs::read(path).ok()?;
                let images: Vec<_> = xcursor::parser::parse_xcursor(&data)?.into_iter().filter(|i| i.width <= 64 && i.height <= 64).collect();
                (!images.is_empty()).then_some(images)
            })
        });
        let Some(images) = images else {
            self.missing.insert(name.to_owned());
            return None;
        };
        // An animated one is several images of each size.
        let each = sizes
            .iter()
            .filter_map(|&size| {
                let best = images.iter().map(|i| i.size).min_by_key(|s| (*s as i64 - size as i64).abs())?;
                let frames = images
                    .iter()
                    .filter(|i| i.size == best)
                    .map(|i| {
                        let mut px = vec![0u8; 64 * 64 * 4];
                        for y in 0..i.height as usize {
                            let row = &i.pixels_rgba[y * i.width as usize * 4..(y + 1) * i.width as usize * 4];
                            px[y * 64 * 4..y * 64 * 4 + row.len()].copy_from_slice(row);
                        }
                        (px, (i.xhot as i32, i.yhot as i32), i.delay)
                    })
                    .collect();
                Some((size, frames))
            })
            .collect();
        Some(each)
    }

    /// That picture is not to be had after all (it could not be put where it goes).
    pub fn lacks(&mut self, name: &str) {
        self.missing.insert(name.to_owned());
    }

    /// Or it is, from elsewhere (our own arrow).
    pub fn has(&mut self, name: &str) {
        self.missing.remove(name);
    }
}

/// The usual arrow, for a theme without one: white, with a dark edge, its tip at 0, 0.
pub fn arrow() -> Vec<u8> {
    let mut px = vec![0u8; 64 * 64 * 4];
    let arrow: [&str; 19] = [
        "X", "XX", "X.X", "X..X", "X...X", "X....X", "X.....X", "X......X", "X.......X", "X........X", "X.........X", "X..........X", "X......XXXXX", "X...X..X", "X..XX..X", "X.X  X..X",
        "XX   X..X", "X     X..X", "      XXX",
    ];
    for (y, row) in arrow.iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            let v: [u8; 4] = match c {
                'X' => [20, 20, 20, 255],
                '.' => [255, 255, 255, 255],
                _ => continue,
            };
            let i = (y * 64 + x) * 4;
            px[i..i + 4].copy_from_slice(&v);
        }
    }
    px
}

/// A program's own cursor in the plane's 64 × 64 (what is beyond, cut), its
/// tip inside it.
pub fn program_picture(p: &crate::layers::CursorPicture) -> Option<(Vec<u8>, (i32, i32))> {
    let ((w, h), data, hot) = &**p;
    let mut px = vec![0u8; 64 * 64 * 4];
    for y in 0..(*h).min(64) {
        let n = (*w).min(64) * 4;
        px[y * 64 * 4..y * 64 * 4 + n].copy_from_slice(data.get(y * w * 4..y * w * 4 + n)?);
    }
    Some((px, (hot.0.clamp(0, 63), hot.1.clamp(0, 63))))
}

/// The cursor's size a scale asks for: XCURSOR_SIZE (24) times it.
pub fn size_at(scale: f64) -> u32 {
    static BASE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let base = *BASE.get_or_init(|| std::env::var("XCURSOR_SIZE").ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0).unwrap_or(24.0));
    (base * scale).round() as u32
}

/// What is shown: a picture of the theme by its name, and whether it is a
/// transition, played once (backwards, if it is the one for the other way round).
pub struct Playing {
    pub asset: String,
    pub reverse: bool,
    pub transition: bool,
}

impl Playing {
    /// Where in its `len` frames it is `frame` frames in: a transition played
    /// backwards counts down, a shape goes round.
    pub fn index(&self, frame: usize, len: usize) -> usize {
        if self.transition {
            let i = frame.min(len.saturating_sub(1));
            if self.reverse { len - 1 - i } else { i }
        } else {
            frame % len.max(1)
        }
    }
}

/// The shapes read when a session starts.
pub const KINDS: [Cursor; 12] = [
    Cursor::Normal,
    Cursor::Hand,
    Cursor::Text,
    Cursor::Grab,
    Cursor::Grabbing,
    Cursor::EwResize,
    Cursor::NsResize,
    Cursor::NwseResize,
    Cursor::NeswResize,
    Cursor::Move,
    Cursor::NotAllowed,
    Cursor::Crosshair,
];

/// The older names themes give those shapes, after their standard one.
pub fn alternatives(kind: Cursor) -> &'static [&'static str] {
    match kind {
        Cursor::Normal => &["left_ptr", "arrow"],
        Cursor::Hand => &["hand2", "pointing_hand", "hand1"],
        Cursor::Text => &["xterm", "ibeam"],
        Cursor::Grab => &["openhand", "hand1"],
        Cursor::Grabbing => &["closedhand", "fleur"],
        Cursor::EwResize => &["col-resize", "sb_h_double_arrow", "h_double_arrow", "size_hor"],
        Cursor::NsResize => &["row-resize", "sb_v_double_arrow", "v_double_arrow", "size_ver"],
        Cursor::NwseResize => &["size_fdiag", "bd_double_arrow", "bottom_right_corner"],
        Cursor::NeswResize => &["size_bdiag", "fd_double_arrow", "bottom_left_corner"],
        Cursor::Move => &["all-scroll", "fleur", "size_all"],
        Cursor::NotAllowed => &["crossed_circle", "forbidden"],
        Cursor::Crosshair => &["cross", "tcross"],
        Cursor::Named(_) => &[],
    }
}

/// The theme's transition from one shape to another, if it has one: `a-to-b`
/// played forwards, or else `b-to-a` played backwards. `has` loads one by
/// name and says whether it is there.
pub fn transition_between(a: &str, b: &str, mut has: impl FnMut(&str) -> bool) -> Option<(String, bool)> {
    let forwards = format!("{a}-to-{b}");
    if has(&forwards) {
        return Some((forwards, false));
    }
    let backwards = format!("{b}-to-{a}");
    has(&backwards).then_some((backwards, true))
}
