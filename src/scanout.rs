//! Direct scanout: a window that fills a monitor, with nothing of the scene over
//! it, shown by putting its own buffer on the monitor's primary plane —no frame
//! put together, nothing copied—, as niri, wlroots and KWin do for a game or a
//! video at full screen.
//!
//! Off unless asked (`PLEAMAR_WM_SCANOUT=1`, or `scanout on` in
//! `session.conf`): it has not been tried on a real session. What decides it is
//! here, and it is all that can be tested without one; every change of the
//! decision is said in the log («scanout · DP-1: on …» / «off: …»), so that it
//! can be watched in a session before anything is put on a plane.
//!
//! What is asked, in this order (the first that fails is the reason said):
//! 1. the render says the monitor's frame would be exactly one window's
//!    picture: one piece, opaque, at full opacity, upright, covering the whole
//!    monitor pixel for pixel, with nothing drawn over it (no bar, no glass, no
//!    popup, no subsurface);
//! 2. the monitor is upright (a turned one is put together upright and turned);
//! 3. the buffer is a dmabuf of one plane, of the monitor's size;
//! 4. the primary plane takes its format with its layout (modifier), as its
//!    `IN_FORMATS` lists them; an ARGB buffer said to be opaque is shown as its
//!    XRGB twin, which every primary plane takes;
//! 5. nobody is copying the monitor (screencopy, sharing the screen, the lens):
//!    they read the frame put together, which there would not be.
//!
//! And then the card has the last word: an atomic commit with `TEST_ONLY`
//! before the real one; if it says no, the frame is put together as now.

// The decision is all there is so far: what calls it (the render's word, the plane
// flip) is the second part (docs/scanout.md).
#![allow(dead_code)]

/// A DRM format, as a fourcc code.
pub type Fourcc = u32;

const fn fourcc(s: &[u8; 4]) -> Fourcc {
    (s[0] as u32) | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
}

pub const ARGB8888: Fourcc = fourcc(b"AR24");
pub const XRGB8888: Fourcc = fourcc(b"XR24");
pub const ABGR8888: Fourcc = fourcc(b"AB24");
pub const XBGR8888: Fourcc = fourcc(b"XB24");
pub const ARGB2101010: Fourcc = fourcc(b"AR30");
pub const XRGB2101010: Fourcc = fourcc(b"XR30");
pub const ABGR2101010: Fourcc = fourcc(b"AB30");
pub const XBGR2101010: Fourcc = fourcc(b"XB30");

/// `DRM_FORMAT_MOD_INVALID`: a buffer that says no layout (the driver's own).
pub const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// A format with alpha, and its twin without: an opaque window drawn in ARGB is
/// the same picture read as XRGB.
pub fn opaque_twin(f: Fourcc) -> Fourcc {
    match f {
        ARGB8888 => XRGB8888,
        ABGR8888 => XBGR8888,
        ARGB2101010 => XRGB2101010,
        ABGR2101010 => XBGR2101010,
        other => other,
    }
}

fn has_alpha(f: Fourcc) -> bool {
    opaque_twin(f) != f
}

/// The fourcc as it is written (`XR24`).
pub fn name(f: Fourcc) -> String {
    f.to_le_bytes().iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '?' }).collect()
}

/// What the render says of a monitor's frame: it would be exactly this
/// window's picture, and nothing else (see pleamar's `Frames::sole_window`).
#[derive(Clone, Debug, PartialEq)]
pub struct Sole {
    /// The program's buffer it is (the number the render knows it by).
    pub buffer: u64,
    /// The window, for the log.
    pub what: String,
}

/// What a buffer on the card is.
#[derive(Clone, Debug, PartialEq)]
pub struct Buffer {
    pub size: (u32, u32),
    pub format: Fourcc,
    pub modifier: u64,
    pub planes: usize,
    /// The program says it is opaque all over (its opaque region covers it).
    pub opaque: bool,
}

/// What the monitor is.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub name: String,
    pub size: (u32, u32),
    /// Turned (`transform 90|180|270`).
    pub turned: bool,
    /// Somebody reads its frames (screencopy, sharing, a lens).
    pub copied: bool,
}

/// Why not, said once each time it changes.
#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    /// The render's own reason: what else is on the monitor.
    Scene(String),
    Turned,
    NotOnCard,
    Planes(usize),
    Size((u32, u32), (u32, u32)),
    Format(Fourcc, u64),
    Copied,
    /// The card said no to it (the atomic test).
    Refused(String),
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Why::Scene(s) => write!(f, "{s}"),
            Why::Turned => write!(f, "the monitor is turned"),
            Why::NotOnCard => write!(f, "the window's buffer is not on the card (shared memory)"),
            Why::Planes(n) => write!(f, "its buffer has {n} planes"),
            Why::Size(b, m) => write!(f, "its buffer is {}×{}, the monitor {}×{}", b.0, b.1, m.0, m.1),
            Why::Format(c, m) => write!(f, "the primary plane does not take {} with layout {m:#x}", name(*c)),
            Why::Copied => write!(f, "somebody is copying the monitor"),
            Why::Refused(e) => write!(f, "the card refused it: {e}"),
        }
    }
}

/// What goes on the plane: the buffer, read as this format.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub buffer: u64,
    pub format: Fourcc,
    pub modifier: u64,
}

/// Whether that monitor's frame can be the window's buffer itself, and as what.
/// `sole`: the render's word (`Err`: what else there is); `buffer`: that
/// window's buffer, if it is on the card; `plane`: the primary plane's
/// `IN_FORMATS`.
pub fn decide(screen: &Screen, sole: Result<&Sole, &str>, buffer: Option<&Buffer>, plane: &[(Fourcc, u64)]) -> Result<Plan, Why> {
    let sole = sole.map_err(|s| Why::Scene(s.to_owned()))?;
    if screen.turned {
        return Err(Why::Turned);
    }
    let b = buffer.ok_or(Why::NotOnCard)?;
    if b.planes != 1 {
        return Err(Why::Planes(b.planes));
    }
    if b.size != screen.size {
        return Err(Why::Size(b.size, screen.size));
    }
    // With alpha, only if the program says it is opaque: then as its twin without.
    let format = if has_alpha(b.format) {
        if !b.opaque {
            return Err(Why::Scene("the window is not opaque".to_owned()));
        }
        opaque_twin(b.format)
    } else {
        b.format
    };
    let takes = |f: Fourcc| plane.iter().any(|(c, m)| *c == f && (*m == b.modifier || (b.modifier == MOD_INVALID && *m == 0)));
    if !takes(format) {
        return Err(Why::Format(format, b.modifier));
    }
    if screen.copied {
        return Err(Why::Copied);
    }
    Ok(Plan { buffer: sole.buffer, format, modifier: b.modifier })
}

/// `PLEAMAR_WM_SCANOUT=1`, or `scanout on` in `session.conf`.
pub fn asked() -> bool {
    match std::env::var("PLEAMAR_WM_SCANOUT") {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => crate::config::get().scanout,
    }
}

/// The decision of each monitor, said only when it changes.
#[derive(Default)]
pub struct Said {
    last: std::collections::HashMap<String, String>,
}

impl Said {
    /// The line to say for that monitor now, if it is not the one said last.
    pub fn line(&mut self, screen: &str, d: &Result<Plan, Why>, what: Option<&str>) -> Option<String> {
        let line = match d {
            Ok(p) => format!("scanout · {screen}: on — {} straight to the screen ({}, layout {:#x})", what.unwrap_or("the window"), name(p.format), p.modifier),
            Err(w) => format!("scanout · {screen}: off — {w}"),
        };
        // The same reason again (another buffer of the same window) is not said again.
        let key = match d {
            Ok(_) => format!("on {}", what.unwrap_or("")),
            Err(w) => format!("off {w}"),
        };
        (self.last.insert(screen.to_owned(), key.clone()).as_deref() != Some(key.as_str())).then_some(line)
    }
}

/// A primary plane's `IN_FORMATS` (`struct drm_format_modifier_blob`): every
/// format with every layout it takes. Malformed, nothing.
pub fn in_formats(blob: &[u8]) -> Vec<(Fourcc, u64)> {
    let u32_at = |o: usize| blob.get(o..o + 4).map(|b| u32::from_ne_bytes(b.try_into().unwrap()));
    let u64_at = |o: usize| blob.get(o..o + 8).map(|b| u64::from_ne_bytes(b.try_into().unwrap()));
    // version, flags, count_formats, formats_offset, count_modifiers, modifiers_offset
    let (Some(_version), Some(nf), Some(fo), Some(nm), Some(mo)) = (u32_at(0), u32_at(8), u32_at(12), u32_at(16), u32_at(20)) else { return Vec::new() };
    let formats: Vec<Fourcc> = (0..nf as usize).filter_map(|k| u32_at(fo as usize + 4 * k)).collect();
    let mut out = Vec::new();
    // Each modifier: formats (a 64-bit mask), offset (u32), pad (u32), modifier (u64).
    for k in 0..nm as usize {
        let at = mo as usize + 24 * k;
        let (Some(mask), Some(offset), Some(modifier)) = (u64_at(at), u32_at(at + 8), u64_at(at + 16)) else { break };
        for bit in 0..64 {
            if mask & (1 << bit) != 0 {
                if let Some(f) = formats.get(offset as usize + bit) {
                    out.push((*f, modifier));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen { name: "DP-1".into(), size: (2560, 1440), turned: false, copied: false }
    }
    fn sole() -> Sole {
        Sole { buffer: 7, what: "window 2 (mpv)".into() }
    }
    fn buffer(format: Fourcc, modifier: u64) -> Buffer {
        Buffer { size: (2560, 1440), format, modifier, planes: 1, opaque: false }
    }
    // NVIDIA-ish: XR24/AR24 linear and one block layout.
    const BLOCK: u64 = 0x0300_0000_0060_6014;
    fn plane() -> Vec<(Fourcc, u64)> {
        vec![(XRGB8888, 0), (XRGB8888, BLOCK), (ARGB8888, 0), (ARGB8888, BLOCK)]
    }

    #[test]
    fn a_window_that_is_the_whole_frame_goes_straight_to_the_screen() {
        let p = decide(&screen(), Ok(&sole()), Some(&buffer(XRGB8888, BLOCK)), &plane()).unwrap();
        assert_eq!(p, Plan { buffer: 7, format: XRGB8888, modifier: BLOCK });
    }

    #[test]
    fn anything_of_the_scene_over_it_keeps_it_put_together() {
        let d = decide(&screen(), Err("the bar is over it"), Some(&buffer(XRGB8888, BLOCK)), &plane());
        assert_eq!(d, Err(Why::Scene("the bar is over it".into())));
    }

    #[test]
    fn a_turned_monitor_a_smaller_buffer_or_a_second_plane_do_not() {
        let mut s = screen();
        s.turned = true;
        assert_eq!(decide(&s, Ok(&sole()), Some(&buffer(XRGB8888, 0)), &plane()), Err(Why::Turned));
        let mut b = buffer(XRGB8888, 0);
        b.size = (1920, 1080);
        assert!(matches!(decide(&screen(), Ok(&sole()), Some(&b), &plane()), Err(Why::Size(..))));
        let mut b = buffer(XRGB8888, 0);
        b.planes = 2;
        assert_eq!(decide(&screen(), Ok(&sole()), Some(&b), &plane()), Err(Why::Planes(2)));
        assert_eq!(decide(&screen(), Ok(&sole()), None, &plane()), Err(Why::NotOnCard));
    }

    #[test]
    fn a_layout_or_a_format_the_plane_does_not_take_does_not() {
        assert_eq!(decide(&screen(), Ok(&sole()), Some(&buffer(XRGB8888, 0x1234)), &plane()), Err(Why::Format(XRGB8888, 0x1234)));
        assert_eq!(decide(&screen(), Ok(&sole()), Some(&buffer(XRGB2101010, 0)), &plane()), Err(Why::Format(XRGB2101010, 0)));
        // A buffer with no layout said is the driver's own, linear for the plane.
        assert!(decide(&screen(), Ok(&sole()), Some(&buffer(XRGB8888, MOD_INVALID)), &plane()).is_ok());
    }

    #[test]
    fn with_alpha_only_if_opaque_and_then_as_its_twin() {
        let mut b = buffer(ARGB8888, BLOCK);
        assert!(matches!(decide(&screen(), Ok(&sole()), Some(&b), &plane()), Err(Why::Scene(_))));
        b.opaque = true;
        assert_eq!(decide(&screen(), Ok(&sole()), Some(&b), &plane()).unwrap().format, XRGB8888);
        assert_eq!(name(XRGB8888), "XR24");
    }

    #[test]
    fn nobody_copying_the_monitor() {
        let mut s = screen();
        s.copied = true;
        assert_eq!(decide(&s, Ok(&sole()), Some(&buffer(XRGB8888, 0)), &plane()), Err(Why::Copied));
    }

    #[test]
    fn a_change_is_said_once() {
        let mut said = Said::default();
        let on = decide(&screen(), Ok(&sole()), Some(&buffer(XRGB8888, 0)), &plane());
        assert!(said.line("DP-1", &on, Some("window 2 (mpv)")).unwrap().contains("on — window 2 (mpv)"));
        assert!(said.line("DP-1", &on, Some("window 2 (mpv)")).is_none());
        let off = decide(&screen(), Err("a popup is over it"), None, &plane());
        assert!(said.line("DP-1", &off, None).unwrap().ends_with("off — a popup is over it"));
        assert!(said.line("DP-2", &off, None).is_some());
    }

    #[test]
    fn a_planes_formats_are_read_from_its_blob() {
        // version 1, two formats (XR24, AR24), two modifiers: linear for both, BLOCK for XR24 only.
        let mut b = Vec::new();
        let push32 = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_ne_bytes());
        let push64 = |b: &mut Vec<u8>, v: u64| b.extend_from_slice(&v.to_ne_bytes());
        push32(&mut b, 1); // version
        push32(&mut b, 0); // flags
        push32(&mut b, 2); // count_formats
        push32(&mut b, 24); // formats_offset
        push32(&mut b, 2); // count_modifiers
        push32(&mut b, 32); // modifiers_offset
        push32(&mut b, XRGB8888);
        push32(&mut b, ARGB8888);
        push64(&mut b, 0b11);
        push32(&mut b, 0);
        push32(&mut b, 0);
        push64(&mut b, 0);
        push64(&mut b, 0b01);
        push32(&mut b, 0);
        push32(&mut b, 0);
        push64(&mut b, BLOCK);
        let got = in_formats(&b);
        assert_eq!(got, vec![(XRGB8888, 0), (ARGB8888, 0), (XRGB8888, BLOCK)]);
        assert!(in_formats(&b[..10]).is_empty());
    }
}
