//! What the agent's pictures of elements need (`c TARGET @ID`, `c TARGET
//! marks` on the agent's socket): one element's box, read from its program
//! and brought into view; and the window's elements marked on its picture,
//! each outlined and its id on a label of its own, from a font of its own
//! (no font is loaded).
//!
//! Boxes are in look coordinates (`a11y_tree.rs`); what is drawn is in the
//! picture's pixels, at its density.

use crate::{a11y, a11y_tree};
use std::time::Duration;

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const TEXT: &str = "org.a11y.atspi.Text";
const ACTION: &str = "org.a11y.atspi.Action";
const APPLICATION: &str = "org.a11y.atspi.Application";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// Firefox's action for what is inside something that is clicked: not its own.
const ANCESTORS: &str = "click ancestor";
/// `Component.GetExtents` in the window's coordinates.
const WINDOW: u32 = 1;
/// `Component.ScrollTo`: wherever it is seen whole.
const ANYWHERE: u32 = 6;
const SHOWING: u32 = 25;
const CALL: Duration = Duration::from_secs(1);

/// The digits, 5 x 7: a row a byte, its leftmost pixel the fifth bit.
const FONT: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
];
/// Pixels of a picture at a unit a pixel to a pixel of the font.
const GLYPH: i32 = 2;
/// An outline's width, in pixels at a unit a pixel.
const LINE: i32 = 2;
/// Under this many units across or down, an element is not marked.
const SMALLEST: i32 = 6;
const ACTING: [u8; 3] = [255, 40, 150];
const READING: [u8; 3] = [30, 150, 255];
const PLATE: [u8; 3] = [18, 18, 18];
const DIGITS: [u8; 3] = [255, 255, 255];

/// An element marked: its id, its box, and whether it is acted on (a link,
/// a button, a field) or read (a text, a heading).
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub id: u32,
    pub rect: [i32; 4],
    pub acting: bool,
}

/// A box's corners, far as they may be.
fn corners(a: [i32; 4]) -> [i64; 4] {
    let a = a.map(i64::from);
    [a[0], a[1], a[0] + a[2], a[1] + a[3]]
}

/// The box between those corners, cut to what an `i32` holds.
fn from_corners(c: [i64; 4]) -> [i32; 4] {
    let n = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    [n(c[0]), n(c[1]), n(c[2] - c[0]), n(c[3] - c[1])]
}

/// `a` grown by `pad` on every side.
pub fn grow(a: [i32; 4], pad: i32) -> [i32; 4] {
    let (c, p) = (corners(a), i64::from(pad));
    from_corners([c[0] - p, c[1] - p, c[2] + p, c[3] + p])
}

/// What two boxes have in common, if anything.
pub fn meet(a: [i32; 4], b: [i32; 4]) -> Option<[i32; 4]> {
    let (a, b) = (corners(a), corners(b));
    let (x0, y0, x1, y1) = (a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3]));
    (x1 > x0 && y1 > y0).then(|| from_corners([x0, y0, x1, y1]))
}

/// Whether `a` is all within `b`.
pub fn inside(a: [i32; 4], b: [i32; 4]) -> bool {
    a[2] > 0 && a[3] > 0 && meet(a, b) == Some(a)
}

/// Where a run of `text` says `run` (case folded): its first and last
/// character but one, as the program counts them (`utf16`: in UTF-16
/// units, as Qt does; else in characters).
fn run_in(text: &str, run: &str, utf16: bool) -> Option<(i32, i32)> {
    let (text, run): (Vec<char>, Vec<char>) = (text.chars().collect(), run.chars().collect());
    let same = |a: char, b: char| a.to_lowercase().eq(b.to_lowercase());
    if run.is_empty() || run.len() > text.len() {
        return None;
    }
    let at = (0..=text.len() - run.len()).find(|&i| run.iter().zip(&text[i..]).all(|(r, t)| same(*r, *t)))?;
    let count = |cs: &[char]| if utf16 { cs.iter().map(|c| c.len_utf16()).sum::<usize>() } else { cs.len() };
    let start = count(&text[..at]);
    Some((start as i32, (start + count(&text[at..at + run.len()])) as i32))
}

/// Whether the element's program counts text in UTF-16 units (Qt), asked
/// of its application.
fn utf16(k: &(String, String)) -> bool {
    use zbus::zvariant::{OwnedObjectPath, OwnedValue};
    let Ok(((_, app),)) = call::<((String, OwnedObjectPath),), _>(k, ACCESSIBLE, "GetApplication", &()) else { return false };
    let app = (k.0.clone(), app.to_string());
    call::<(OwnedValue,), _>(&app, PROPERTIES, "Get", &(APPLICATION, "ToolkitName")).ok().and_then(|(v,)| String::try_from(v).ok()).is_some_and(|t| t.eq_ignore_ascii_case("qt"))
}

/// One call to an element: its answer, or `gone` if the element (or its
/// program) is not there to give one.
fn call<T, B>(k: &(String, String), interface: &str, member: &str, body: &B) -> Result<T, &'static str>
where
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
{
    match a11y::call_blocking(&k.0, &k.1, interface, member, body, CALL) {
        Ok(reply) => reply.body().deserialize::<T>().map_err(|_| "gone"),
        Err(a11y::Error::Timeout) => Err("timeout"),
        Err(_) => Err("gone"),
    }
}

/// An element of that window, as `A` numbered it: its box (none when its
/// program gives none, or one of no size or far away), and whether it is
/// shown. With `run`, the box of the run of its text that says it, when its
/// text says more than that (a word in a paragraph).
pub fn element_box(pid: u32, slot: usize, id: u32, run: Option<&str>) -> Result<(Option<[i32; 4]>, bool), &'static str> {
    let k = a11y_tree::key_of(pid, slot, id).ok_or("gone")?;
    let (states,): (Vec<u32>,) = call(&k, ACCESSIBLE, "GetState", &())?;
    let ((x, y, w, h),): ((i32, i32, i32, i32),) = call(&k, COMPONENT, "GetExtents", &(WINDOW,))?;
    let showing = states.first().is_some_and(|s| s & (1 << SHOWING) != 0);
    let words = run.and_then(|run| {
        let (text,): (String,) = call(&k, TEXT, "GetText", &(0i32, -1i32)).ok()?;
        // Characters past the first plane are two units to Qt, one to the others.
        let wide = text.chars().any(|c| c.len_utf16() > 1) && utf16(&k);
        let (start, end) = run_in(&text, run, wide).filter(|_| text.trim().chars().count() > run.chars().count())?;
        // Firefox answers an empty box the first time it is asked for a run.
        let range = || call::<((i32, i32, i32, i32),), _>(&k, TEXT, "GetRangeExtents", &(start, end, WINDOW)).ok().and_then(|((x, y, w, h),)| a11y_tree::sane([x, y, w, h]));
        range().or_else(range)
    });
    Ok((words.or(a11y_tree::sane([x, y, w, h])).map(|b| a11y_tree::placed(pid, slot, b)), showing))
}

/// The element scrolled into view by its program: whether it says it did.
/// Not once the user stopped the agent after `stops`.
pub fn scroll_to(pid: u32, slot: usize, id: u32, stops: u64) -> bool {
    !crate::a11y_act::stopped_since(stops) && a11y_tree::key_of(pid, slot, id).is_some_and(|k| call::<(bool,), _>(&k, COMPONENT, "ScrollTo", &(ANYWHERE,)).is_ok_and(|t| t.0))
}

/// What a record says that marking needs: id, depth, role, box, states and
/// whether it has a name or a text.
fn fields(record: &str) -> Option<(u32, usize, &str, Option<[i32; 4]>, &str, bool)> {
    let f: Vec<&str> = record.split_whitespace().collect();
    let [id, depth, role, x, y, w, h, states, name, text] = f[..] else { return None };
    let n = |v: &str| v.parse::<i32>().ok();
    let rect = (|| Some([n(x)?, n(y)?, n(w)?, n(h)?]))();
    Some((id.parse().ok()?, depth.parse().ok()?, role, rect, states, name != "-" || text != "-"))
}

/// Roles that are read rather than acted on.
fn reading(role: &str) -> bool {
    matches!(role, "heading" | "text" | "label" | "static" | "paragraph" | "caption" | "document_text")
}

/// Roles that hold what is acted on rather than being it, focusable or with
/// actions as they may be (a document, a notebook, Firefox's toolbars).
fn holds(role: &str) -> bool {
    matches!(role, "tool_bar" | "menu_bar" | "status_bar" | "page_tab_list" | "separator" | "scroll_pane" | "split_pane" | "splitter" | "viewport" | "internal_frame" | "document_web" | "document_frame" | "panel" | "filler" | "section")
}

/// How a record is marked, if it is: acted on (`focusable`, with an action
/// of its own, or a tab), or with `all` read (a heading, a text that says
/// something). `actions` is asked only when the rest does not decide. The
/// window's frame, what holds others, an element without a box or one too
/// small are not marked.
fn kind(record: &str, all: bool, mut actions: impl FnMut(u32) -> bool) -> Option<Mark> {
    let (id, depth, role, rect, states, says) = fields(record)?;
    let rect = rect.filter(|r| depth > 0 && !holds(role) && r[2] >= SMALLEST && r[3] >= SMALLEST)?;
    let acting = role == "page_tab" || states.split(',').any(|s| s == "focusable") || actions(id);
    (acting || (all && reading(role) && says)).then_some(Mark { id, rect, acting })
}

/// The records of a read (`nodes V | ...`'s, without the head) that are
/// marked, and their marks: whether an element has actions of its own is
/// asked of its program, all at once.
pub fn pick<'a>(pid: u32, slot: usize, records: &[&'a str], all: bool) -> Vec<(Mark, &'a str)> {
    // Those whose mark depends on it.
    let mut ask: Vec<u32> = Vec::new();
    for r in records {
        let _ = kind(r, all, |id| {
            ask.push(id);
            false
        });
    }
    let keys: Vec<(u32, (String, String))> = ask.into_iter().filter_map(|id| Some((id, a11y_tree::key_of(pid, slot, id)?))).collect();
    let ex = async_executor::LocalExecutor::new();
    let counts = futures_lite::future::block_on(ex.run(a11y_tree::all(
        &ex,
        keys.iter().map(|(_, k)| async move { a11y_tree::ask::<(Vec<(String, String, String)>,), _>(k, ACTION, "GetActions", &()).await.map_or(0, |t| t.0.iter().filter(|a| a.0 != ANCESTORS).count()) }).collect(),
    )));
    let with: Vec<u32> = keys.iter().zip(counts).filter(|(_, n)| *n > 0).map(|((id, _), _)| *id).collect();
    records.iter().filter_map(|r| Some((kind(r, all, |id| with.contains(&id))?, *r))).collect()
}

/// A label's size in pixels, for that many digits at `k` pixels a unit.
fn label_size(digits: usize, k: i32) -> (i32, i32) {
    let g = GLYPH * k;
    ((6 * digits as i32 + 1) * g, 9 * g)
}

fn overlap(a: [i32; 4], b: [i32; 4]) -> bool {
    let (a, b) = (corners(a), corners(b));
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Where each label goes (its box's pixels and its size given): above the
/// box's top left corner; inside it, from that corner, when above would be
/// off the picture or over a label put before; nowhere when both would.
fn place(labels: &[([i32; 4], (i32, i32))], size: (i32, i32)) -> Vec<Option<[i32; 4]>> {
    let mut put: Vec<[i32; 4]> = Vec::new();
    labels
        .iter()
        .map(|(r, (w, h))| {
            let x = r[0].min(size.0.saturating_sub(*w)).max(0);
            let above = [x, r[1].saturating_sub(*h), *w, *h];
            let within = [x, r[1].max(0), *w, *h];
            let fits = |l: &[i32; 4]| l[1] >= 0 && i64::from(l[1]) + i64::from(l[3]) <= i64::from(size.1) && !put.iter().any(|p| overlap(*p, *l));
            let at = [above, within].into_iter().find(|l| fits(l))?;
            put.push(at);
            Some(at)
        })
        .collect()
}

/// A box filled with a colour, cut where the picture ends.
fn fill(picture: &mut image::RgbaImage, r: [i32; 4], c: [u8; 3]) {
    let (w, h) = (picture.width() as i32, picture.height() as i32);
    for y in r[1].max(0)..r[1].saturating_add(r[3]).min(h) {
        for x in r[0].max(0)..r[0].saturating_add(r[2]).min(w) {
            picture.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
        }
    }
}

fn outline(picture: &mut image::RgbaImage, r: [i32; 4], t: i32, c: [u8; 3]) {
    let (right, bottom) = (r[0].saturating_add(r[2]).saturating_sub(t), r[1].saturating_add(r[3]).saturating_sub(t));
    fill(picture, [r[0], r[1], r[2], t], c);
    fill(picture, [r[0], bottom, r[2], t], c);
    fill(picture, [r[0], r[1], t, r[3]], c);
    fill(picture, [right, r[1], t, r[3]], c);
}

/// A label: its plate, edged in its mark's colour, and its digits on it.
fn label(picture: &mut image::RgbaImage, at: [i32; 4], id: u32, k: i32, c: [u8; 3]) {
    let g = GLYPH * k;
    fill(picture, at, PLATE);
    outline(picture, at, k, c);
    for (n, d) in id.to_string().bytes().enumerate() {
        let rows = FONT[(d - b'0') as usize];
        let (x0, y0) = (at[0] + g + 6 * g * n as i32, at[1] + g);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (0b10000 >> col) != 0 {
                    fill(picture, [x0 + col * g, y0 + row as i32 * g, g, g], DIGITS);
                }
            }
        }
    }
}

/// The marks drawn on a picture at `density` pixels a unit: every outline,
/// then the labels over them. A box not on the picture (a row scrolled out)
/// is not drawn.
pub fn draw(picture: &mut image::RgbaImage, marks: &[Mark], density: f64) {
    let k = (density.round() as i32).max(1);
    let px = |v: i64| (v as f64 * density).round() as i32;
    let size = (picture.width() as i32, picture.height() as i32);
    let (marks, boxes): (Vec<&Mark>, Vec<[i32; 4]>) = marks
        .iter()
        .map(|m| {
            let c = corners(m.rect);
            (m, [px(c[0]), px(c[1]), px(c[2]).saturating_sub(px(c[0])), px(c[3]).saturating_sub(px(c[1]))])
        })
        .filter(|(_, b)| meet(*b, [0, 0, size.0, size.1]).is_some())
        .unzip();
    let colour = |m: &Mark| if m.acting { ACTING } else { READING };
    for (m, b) in marks.iter().zip(&boxes) {
        outline(picture, *b, LINE * k, colour(m));
    }
    let at = place(&marks.iter().zip(&boxes).map(|(m, b)| (*b, label_size(m.id.to_string().len(), k))).collect::<Vec<_>>(), size);
    for (m, at) in marks.iter().zip(at) {
        if let Some(at) = at {
            label(picture, at, m.id, k, colour(m));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_digit_is_five_by_seven_and_its_own() {
        for (d, rows) in FONT.iter().enumerate() {
            assert!(rows.iter().all(|r| *r < 32), "{d} is wider than 5");
            assert!(rows.iter().any(|r| *r != 0));
        }
        for a in 0..10 {
            for b in a + 1..10 {
                assert_ne!(FONT[a], FONT[b], "{a} and {b}");
            }
        }
    }

    #[test]
    fn a_label_is_its_digits_on_a_plate() {
        assert_eq!(label_size(1, 1), (14, 18));
        assert_eq!(label_size(3, 1), (38, 18));
        assert_eq!(label_size(2, 2), (52, 36));
        let mut p = image::RgbaImage::new(40, 30);
        label(&mut p, [0, 0, 26, 18], 17, 1, ACTING);
        let at = |x: u32, y: u32| p.get_pixel(x, y).0;
        // The edge, the plate, and the 1's top pixel (its third column).
        assert_eq!(at(0, 0), [255, 40, 150, 255]);
        assert_eq!(at(1, 1), [18, 18, 18, 255]);
        assert_eq!(at(2 + 2 * 2, 2), [255, 255, 255, 255]);
        assert_eq!(at(2, 2), [18, 18, 18, 255]);
        // The 7's top row, all lit, from the second glyph on.
        assert!((0..5).all(|c| at(2 + 12 + c * 2, 2) == [255, 255, 255, 255]));
        // Nothing past the plate.
        assert_eq!(at(30, 25), [0, 0, 0, 0]);
    }

    #[test]
    fn a_label_goes_above_its_corner_else_inside_else_nowhere() {
        let size = (400, 300);
        let l = (14, 18);
        // Above the corner.
        assert_eq!(place(&[([50, 100, 80, 30], l)], size), vec![Some([50, 82, 14, 18])]);
        // At the picture's top: inside.
        assert_eq!(place(&[([50, 5, 80, 30], l)], size), vec![Some([50, 5, 14, 18])]);
        // At its right edge: moved left to be whole.
        assert_eq!(place(&[([395, 100, 5, 30], l)], size), vec![Some([386, 82, 14, 18])]);
        // A child at its parent's corner: inside, under the parent's.
        let nested = place(&[([50, 100, 200, 100], l), ([50, 100, 80, 30], l)], size);
        assert_eq!(nested, vec![Some([50, 82, 14, 18]), Some([50, 100, 14, 18])]);
        // A third there has no room left.
        let three = place(&[([50, 100, 200, 100], l), ([50, 100, 80, 30], l), ([52, 101, 40, 20], l)], size);
        assert_eq!(three[2], None);
        // Side by side, each its own.
        let row = place(&[([10, 100, 30, 30], l), ([60, 100, 30, 30], l)], size);
        assert!(row.iter().all(|r| r.is_some_and(|r| r[1] == 82)));
    }

    #[test]
    fn what_is_marked_and_how() {
        let none = |_| false;
        let some = |_| true;
        assert_eq!(kind("24 1 push_button 260 610 180 36 focusable 4d -", false, none), Some(Mark { id: 24, rect: [260, 610, 180, 36], acting: true }));
        // An action makes it acted on; without one, not marked.
        assert!(kind("3 1 button 1409 73 32 38 - 436c6f7365 -", false, some).is_some_and(|m| m.acting));
        assert_eq!(kind("3 1 button 1409 73 32 38 - 436c6f7365 -", false, none), None);
        // Text and headings with `all` only, as read.
        assert_eq!(kind("5 2 label 10 10 80 20 - 48 -", false, none), None);
        assert_eq!(kind("5 2 label 10 10 80 20 - 48 -", true, none), Some(Mark { id: 5, rect: [10, 10, 80, 20], acting: false }));
        assert_eq!(kind("5 2 label 10 10 80 20 - - -", true, none), None);
        // The frame, no box, too small.
        assert_eq!(kind("1 0 frame 0 0 800 600 focusable - -", true, some), None);
        assert_eq!(kind("6 1 push_button - - - - focusable 4d -", true, some), None);
        assert_eq!(kind("7 1 push_button 0 0 5 30 focusable 4d -", true, some), None);
        assert_eq!(kind("... more 3", true, some), None);
        // What holds others, though focusable; a tab, though neither.
        assert_eq!(kind("25 1 internal_frame 85 144 829 448 focusable - -", true, some), None);
        assert_eq!(kind("2 1 tool_bar 85 70 829 36 - 42 -", true, some), None);
        assert!(kind("101 2 page_tab 187 846 44 23 - 70 -", false, none).is_some_and(|m| m.acting));
    }

    #[test]
    fn a_run_is_found_case_folded_and_counted_in_characters() {
        assert_eq!(run_in("Hello World", "world", false), Some((6, 11)));
        assert_eq!(run_in("a\u{f1}o nuevo", "NUEVO", false), Some((4, 9)));
        assert_eq!(run_in("short", "longer text", false), None);
        assert_eq!(run_in("abc", "", false), None);
        // Past the first plane: a character to GTK, two units to Qt.
        assert_eq!(run_in("\u{1f600} ok \u{1f600}", "ok", false), Some((2, 4)));
        assert_eq!(run_in("\u{1f600} ok \u{1f600}", "ok", true), Some((3, 5)));
        assert_eq!(run_in("a \u{1f600}b", "\u{1f600}b", true), Some((2, 5)));
    }

    #[test]
    fn boxes_far_away_do_not_overflow() {
        let none = [i32::MIN, i32::MIN, -1, -1];
        assert_eq!(meet(grow(none, 400), [0, 0, 100, 100]), None);
        assert_eq!(meet([i32::MAX, i32::MAX, i32::MAX, i32::MAX], [0, 0, 100, 100]), None);
        assert!(!inside(none, [0, 0, 100, 100]));
        assert!(!overlap(none, [0, 0, 10, 10]));
        let size = (100, 100);
        assert_eq!(place(&[([i32::MIN, i32::MIN, 10, 10], (14, 18))], size), vec![Some([0, 0, 14, 18])]);
        let mut p = image::RgbaImage::new(10, 10);
        fill(&mut p, [i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX], ACTING);
        outline(&mut p, [i32::MIN, i32::MIN, i32::MAX, i32::MAX], 2, ACTING);
    }

    #[test]
    fn what_is_off_the_picture_is_not_drawn() {
        let mut p = image::RgbaImage::new(100, 50);
        let marks = [Mark { id: 3, rect: [10, -80, 40, 20], acting: true }, Mark { id: 4, rect: [10, 60, 40, 20], acting: true }, Mark { id: 5, rect: [i32::MIN, i32::MIN, i32::MAX, 5], acting: true }];
        draw(&mut p, &marks, 1.0);
        assert!(p.pixels().all(|c| c.0 == [0, 0, 0, 0]));
        draw(&mut p, &[Mark { id: 6, rect: [10, -10, 40, 20], acting: false }], 1.0);
        // Its outline's bottom edge, and its label inside from the top.
        assert_eq!(p.get_pixel(40, 8).0, [30, 150, 255, 255]);
        assert_eq!(p.get_pixel(11, 1).0, [18, 18, 18, 255]);
    }

    #[test]
    fn boxes_grown_and_met() {
        assert_eq!(grow([10, 10, 20, 20], 4), [6, 6, 28, 28]);
        assert_eq!(meet([-5, -5, 20, 20], [0, 0, 100, 100]), Some([0, 0, 15, 15]));
        assert_eq!(meet([200, 0, 20, 20], [0, 0, 100, 100]), None);
        assert!(inside([10, 10, 5, 5], [0, 0, 100, 100]));
        assert!(!inside([95, 10, 10, 5], [0, 0, 100, 100]));
        assert!(!inside([10, 10, 0, 0], [0, 0, 100, 100]));
    }
}
