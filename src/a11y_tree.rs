//! A window's accessibility tree, as the agent reads it (`A` on the agent's
//! socket): what its program published on the accessibility bus (`a11y.rs`),
//! walked from the window's frame through pleamar-wm's own reader.
//!
//! Each element given has a short number of its own (`ID`), kept while it
//! lives and never given to another while the window is open, so an agent
//! can act on it later by it. Each read has a version, counted across the
//! windows (one window's is never another's): the last few of a window are
//! kept, for what changed since one of them.
//!
//! A record is `ID DEPTH ROLE X Y W H STATES NAME TEXT`: the role as AT-SPI
//! names it with `_` for spaces, the box in the window's coordinates (`- - -
//! -` without one), its states comma separated (`-` with none), its name and
//! text hex encoded (`-` if empty), each cut at 400 characters.

use crate::a11y;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const TEXT: &str = "org.a11y.atspi.Text";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const NULL: &str = "/org/a11y/atspi/null";
/// `Component.GetExtents` in the window's coordinates.
const WINDOW: u32 = 1;

/// How far a walk goes: elements visited, given, how deep, and for how long.
const VISITED: usize = 4000;
const GIVEN: usize = 400;
const DEPTH: usize = 40;
const LIMIT: Duration = Duration::from_millis(2500);
/// How long one call to a program is waited for.
const CALL: Duration = Duration::from_secs(1);
/// Characters of a name or a text given.
const CUT: usize = 400;
/// Reads of a window kept.
const KEPT: usize = 4;

/// AT-SPI's states (`AtspiStateType`), by their bit.
const ACTIVE: u32 = 1;
const CHECKED: u32 = 4;
const COLLAPSED: u32 = 5;
const EDITABLE: u32 = 7;
const ENABLED: u32 = 8;
const EXPANDABLE: u32 = 9;
const EXPANDED: u32 = 10;
const FOCUSABLE: u32 = 11;
const FOCUSED: u32 = 12;
const PRESSED: u32 = 20;
const SELECTED: u32 = 23;
const SENSITIVE: u32 = 24;
const SHOWING: u32 = 25;
const VISIBLE: u32 = 30;
const REQUIRED: u32 = 33;
const INVALID_ENTRY: u32 = 36;

/// What is asked of a window's tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// All that is shown (`all`: what is not shown too).
    Tree(bool),
    /// The element with the keyboard's focus, and those it is in.
    Focus,
    /// The element at that point of the window, and those it is in.
    At(i32, i32),
    /// The elements whose name or text has that in it (case folded), and those they are in.
    Find(String),
    /// What is shown, as what changed since that read (`true`: boxes too).
    Since(u64, bool),
}

/// An element given.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: u32,
    pub depth: usize,
    pub role: String,
    pub rect: Option<[i32; 4]>,
    pub states: Vec<&'static str>,
    pub name: String,
    pub text: String,
}

impl Node {
    pub fn record(&self) -> String {
        let hex = |s: &str| if s.is_empty() { "-".to_owned() } else { s.chars().take(CUT).collect::<String>().bytes().map(|b| format!("{b:02x}")).collect() };
        let rect = self.rect.map_or("- - - -".to_owned(), |r| format!("{} {} {} {}", r[0], r[1], r[2], r[3]));
        let states = if self.states.is_empty() { "-".to_owned() } else { self.states.join(",") };
        format!("{} {} {} {rect} {states} {} {}", self.id, self.depth, self.role, hex(&self.name), hex(&self.text))
    }
}

/// Where an element is: its program's connection and its object.
type Key = (String, String);

/// What is kept of a window between reads: its elements' numbers and its
/// last reads.
#[derive(Default)]
struct Kept {
    ids: HashMap<Key, u32>,
    keys: HashMap<u32, Key>,
    next: u32,
    version: u64,
    reads: VecDeque<(u64, Vec<Node>)>,
    /// Those of the reads kept that were of what is shown, as `tree` gives it.
    shown: Vec<u64>,
}

impl Kept {
    fn id(&mut self, k: &Key) -> u32 {
        if let Some(id) = self.ids.get(k) {
            return *id;
        }
        self.next += 1;
        self.ids.insert(k.clone(), self.next);
        self.keys.insert(self.next, k.clone());
        self.next
    }
}

/// By window: its process and its slot (a slot is another window's later).
static KEPT_OF: Mutex<Option<HashMap<(u32, usize), Kept>>> = Mutex::new(None);
/// The last version given, of any window.
static VERSION: AtomicU64 = AtomicU64::new(0);

/// The element that number names in that window, as it was given.
pub fn key_of(pid: u32, slot: usize, id: u32) -> Option<(String, String)> {
    KEPT_OF.lock().unwrap().as_ref()?.get(&(pid, slot))?.keys.get(&id).cloned()
}

/// A window gone: what was kept of it, forgotten.
pub fn forget(pid: u32, slot: usize) {
    if let Some(all) = KEPT_OF.lock().unwrap().as_mut() {
        all.remove(&(pid, slot));
    }
}

/// What is read of an element to decide whether, and how, it is given.
#[derive(Clone, Debug, Default)]
struct Seen {
    key: Key,
    parent: Option<usize>,
    role: String,
    name: String,
    states: u64,
    interfaces: Vec<String>,
    children: Vec<Key>,
    rect: Option<[i32; 4]>,
    text: String,
    /// Not walked into: not shown, or no box.
    pruned: bool,
}

impl Seen {
    fn has(&self, bit: u32) -> bool {
        self.states & (1 << bit) != 0
    }

    /// Left out of what is given, its children kept: a container that says nothing.
    fn bare(&self) -> bool {
        match self.role.as_str() {
            "filler" | "panel" | "section" | "redundant_object" | "unknown" => true,
            "document_frame" => self.name.is_empty() && self.text.is_empty(),
            _ => false,
        }
    }

    fn state_names(&self, all: bool) -> Vec<&'static str> {
        let mut s = Vec::new();
        for (bit, name) in [(FOCUSED, "focused"), (FOCUSABLE, "focusable"), (CHECKED, "checked"), (PRESSED, "pressed"), (SELECTED, "selected"), (EXPANDED, "expanded"), (EDITABLE, "editable")] {
            if self.has(bit) {
                s.push(name);
            }
        }
        if self.has(COLLAPSED) || (self.has(EXPANDABLE) && !self.has(EXPANDED)) {
            s.push("collapsed");
        }
        if !self.has(ENABLED) && !self.has(SENSITIVE) {
            s.push("disabled");
        }
        if self.has(REQUIRED) {
            s.push("required");
        }
        if self.has(INVALID_ENTRY) {
            s.push("invalid");
        }
        if all && !self.has(SHOWING) {
            s.push("offscreen");
        }
        s
    }
}

/// A call to an element, its answer read as `T`; none if it failed or was late.
pub(crate) async fn ask<T, B>(k: &Key, interface: &str, member: &str, body: &B) -> Option<T>
where
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
{
    let call = a11y::call(&k.0, &k.1, interface, member, body);
    let late = async {
        async_io::Timer::after(CALL).await;
        Err(a11y::Error::Timeout)
    };
    let reply = futures_lite::future::or(call, late).await.ok()?;
    reply.body().deserialize::<T>().ok()
}

async fn children(k: &Key) -> Vec<Key> {
    ask::<(Vec<(String, OwnedObjectPath)>,), _>(k, ACCESSIBLE, "GetChildren", &()).await.map(|t| t.0.into_iter().map(|(n, p)| (n, p.to_string())).filter(|(_, p)| p != NULL).collect()).unwrap_or_default()
}

async fn name(k: &Key) -> String {
    ask::<(OwnedValue,), _>(k, PROPERTIES, "Get", &(ACCESSIBLE, "Name")).await.and_then(|t| String::try_from(t.0).ok()).unwrap_or_default()
}

/// Its role, its name, its states and its interfaces, asked at once.
async fn seen(k: Key, parent: Option<usize>) -> Seen {
    let role = async { ask::<(String,), _>(&k, ACCESSIBLE, "GetRoleName", &()).await.map(|t| t.0.replace(' ', "_")).unwrap_or_else(|| "unknown".into()) };
    let states = async { ask::<(Vec<u32>,), _>(&k, ACCESSIBLE, "GetState", &()).await.map(|t| t.0.iter().take(2).enumerate().fold(0u64, |s, (i, w)| s | (*w as u64) << (32 * i))).unwrap_or(0) };
    let interfaces = async { ask::<(Vec<String>,), _>(&k, ACCESSIBLE, "GetInterfaces", &()).await.map(|t| t.0).unwrap_or_default() };
    let ((role, name), (states, interfaces)) = futures_lite::future::zip(futures_lite::future::zip(role, name(&k)), futures_lite::future::zip(states, interfaces)).await;
    Seen { key: k, parent, role, name, states, interfaces, ..Seen::default() }
}

async fn extents(k: &Key) -> Option<[i32; 4]> {
    ask::<((i32, i32, i32, i32),), _>(k, COMPONENT, "GetExtents", &(WINDOW,)).await.map(|((x, y, w, h),)| [x, y, w, h])
}

/// Its text, without the characters that stand for embedded objects.
async fn text(k: &Key) -> String {
    ask::<(String,), _>(k, TEXT, "GetText", &(0i32, -1i32)).await.map(|t| t.0.replace('\u{fffc}', "").trim().to_owned()).unwrap_or_default()
}

/// Each future on the executor, all at once: their answers, in order.
pub(crate) async fn all<'a, T: 'a>(ex: &async_executor::LocalExecutor<'a>, futures: Vec<impl Future<Output = T> + 'a>) -> Vec<T> {
    let tasks: Vec<_> = futures.into_iter().map(|f| ex.spawn(f)).collect();
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        out.push(t.await);
    }
    out
}

/// Roles whose text is what they say.
fn texty(role: &str) -> bool {
    matches!(role, "text" | "entry" | "paragraph" | "heading" | "label" | "static" | "caption" | "document_text" | "terminal" | "editbar" | "list_item" | "table_cell" | "link")
}

/// A window's tree, as `A` answers: `nodes V` and its records, `|` between
/// them; `since`: `changes V` and what changed, or `full |` and the whole
/// tree when that read is not kept or the frame is another. Waits on the
/// programs: not on the compositor's loop. `family`: the processes that may
/// serve the tree when the window's own did not publish one (a browser's).
pub fn read(slot: usize, pid: u32, title: &str, family: impl Fn(u32) -> bool, ask: Ask) -> String {
    let Some(program) = a11y::program_of(pid).or_else(|| a11y::programs().into_iter().find(|p| p.root.is_some() && !p.reader && family(p.pid))) else {
        return if a11y::address().is_none() { "err no-bus".into() } else { "err no-tree".into() };
    };
    let Some(root) = program.root.clone() else { return "err no-tree".into() };
    let ex = async_executor::LocalExecutor::new();
    let walked = futures_lite::future::block_on(ex.run(walk(&ex, root, title, &ask)));
    let Some((seen, more)) = walked else { return "err no-frame".into() };
    let mut guard = KEPT_OF.lock().unwrap();
    let kept = guard.get_or_insert_with(HashMap::new).entry((pid, slot)).or_default();
    let nodes = give(&seen, &ask, kept);
    // The read asked against, if it is kept and was of what is shown.
    let then = match ask {
        Ask::Since(v, boxes) => Some(kept.reads.iter().find(|(r, _)| *r == v && kept.shown.contains(r)).map(|(_, n)| (n.clone(), boxes))),
        _ => None,
    };
    kept.version = VERSION.fetch_add(1, Ordering::Relaxed) + 1;
    let version = kept.version;
    kept.reads.push_back((version, nodes.clone()));
    if matches!(ask, Ask::Tree(false) | Ask::Since(..)) {
        kept.shown.push(version);
    }
    while kept.reads.len() > KEPT {
        kept.reads.pop_front();
    }
    let oldest = kept.reads.front().map_or(0, |r| r.0);
    kept.shown.retain(|v| *v >= oldest);
    let mut out = match then {
        Some(Some((then, boxes))) if same_frame(&then, &nodes) => {
            let mut out = format!("changes {version}");
            for c in changes(&then, &nodes, boxes) {
                out.push_str(" | ");
                out.push_str(&c);
            }
            out
        }
        Some(_) => format!("full | {}", records(version, &nodes)),
        None => records(version, &nodes),
    };
    if more > 0 {
        out.push_str(&format!(" | ... more {more}"));
    }
    out
}

/// `nodes V` and the records, `|` between them.
fn records(version: u64, nodes: &[Node]) -> String {
    let mut out = format!("nodes {version}");
    for n in nodes {
        out.push_str(" | ");
        out.push_str(&n.record());
    }
    out
}

/// Whether two reads are of the same frame: the same element, by the same
/// name (a page that navigated, a window that is now another, are not).
fn same_frame(then: &[Node], now: &[Node]) -> bool {
    match (then.first(), now.first()) {
        (Some(a), Some(b)) => a.id == b.id && a.name == b.name,
        (a, b) => a.is_none() && b.is_none(),
    }
}

/// What changed from one read to the next, in the order the elements are
/// in now, then those gone: `+ RECORD` given now and not then, `~ RECORD`
/// its role, name, text or states changed (`boxes`: its box too), `- ID`
/// given then and not now.
pub fn changes(then: &[Node], now: &[Node], boxes: bool) -> Vec<String> {
    let before: HashMap<u32, &Node> = then.iter().map(|n| (n.id, n)).collect();
    let mut out = Vec::new();
    for n in now {
        match before.get(&n.id) {
            None => out.push(format!("+ {}", n.record())),
            Some(b) if b.role != n.role || b.name != n.name || b.text != n.text || b.states != n.states || (boxes && b.rect != n.rect) => out.push(format!("~ {}", n.record())),
            Some(_) => {}
        }
    }
    let after: std::collections::HashSet<u32> = now.iter().map(|n| n.id).collect();
    out.extend(then.iter().filter(|n| !after.contains(&n.id)).map(|n| format!("- {}", n.id)));
    out
}

/// The window's frame among its program's: the one with its title, else the
/// active one, else the only one.
async fn frame(ex: &async_executor::LocalExecutor<'_>, root: Key, title: &str) -> Option<Seen> {
    let frames = all(ex, children(&root).await.into_iter().map(|k| seen(k, None)).collect()).await;
    let named = frames.iter().position(|f| !title.is_empty() && f.name == title);
    let active = frames.iter().position(|f| f.has(ACTIVE));
    let only = (frames.len() == 1).then_some(0);
    named.or(active).or(only).map(|k| frames[k].clone())
}

/// What there is under the frame, a depth at a time (each depth's elements
/// asked at once), within the limits; and how many were left unseen.
async fn walk(ex: &async_executor::LocalExecutor<'_>, root: Key, title: &str, ask: &Ask) -> Option<(Vec<Seen>, usize)> {
    let start = Instant::now();
    let frame = frame(ex, root, title).await?;
    let everything = matches!(ask, Ask::Tree(true));
    let mut seen = vec![frame];
    let mut level = vec![0usize];
    let mut left = 0;
    for _ in 0..DEPTH {
        // Its children, for those not left out.
        let open: Vec<usize> = level.into_iter().filter(|&i| !seen[i].pruned).collect();
        let kids = all(ex, open.iter().map(|&i| { let k = seen[i].key.clone(); async move { children(&k).await } }).collect()).await;
        let mut next: Vec<(Key, usize)> = Vec::new();
        for (&i, kids) in open.iter().zip(kids) {
            seen[i].children = kids.clone();
            next.extend(kids.into_iter().map(|k| (k, i)));
        }
        if next.is_empty() || seen.len() >= VISITED || start.elapsed() > LIMIT {
            left += next.len();
            break;
        }
        let room = VISITED - seen.len();
        if next.len() > room {
            left += next.len() - room;
            next.truncate(room);
        }
        let got = all(ex, next.into_iter().map(|(k, p)| seen_k(k, p)).collect()).await;
        level = Vec::new();
        for mut s in got {
            s.pruned = !everything && !(s.has(SHOWING) && s.has(VISIBLE));
            level.push(seen.len());
            seen.push(s);
        }
    }
    // Boxes and texts, for what may be given.
    let wanted: Vec<usize> = (0..seen.len()).filter(|&i| !seen[i].pruned).collect();
    let boxes = all(ex, wanted.iter().map(|&i| { let k = seen[i].key.clone(); async move { extents(&k).await } }).collect()).await;
    for (&i, b) in wanted.iter().zip(boxes) {
        seen[i].rect = b;
        if b.is_some_and(|b| b[2] <= 0 || b[3] <= 0) && !everything && i != 0 {
            seen[i].pruned = true;
        }
    }
    let texts: Vec<usize> = wanted.iter().copied().filter(|&i| {
        let s = &seen[i];
        !s.pruned && s.role != "password_text" && s.interfaces.iter().any(|f| f == TEXT) && (texty(&s.role) || (s.name.is_empty() && s.children.is_empty()))
    }).collect();
    let got = all(ex, texts.iter().map(|&i| { let k = seen[i].key.clone(); async move { text(&k).await } }).collect()).await;
    for (&i, t) in texts.iter().zip(got) {
        seen[i].text = t;
    }
    Some((seen, left))
}

fn seen_k(k: Key, parent: usize) -> impl Future<Output = Seen> {
    seen(k, Some(parent))
}

/// What is given of what was walked, in the order it is in (each element
/// before what it holds): numbered, at the depth of those given above it.
fn give(seen: &[Seen], ask: &Ask, kept: &mut Kept) -> Vec<Node> {
    if seen.is_empty() {
        return Vec::new();
    }
    let everything = matches!(ask, Ask::Tree(true));
    // Each one's children, as walked.
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); seen.len()];
    for (i, s) in seen.iter().enumerate().skip(1) {
        if let Some(p) = s.parent {
            kids[p].push(i);
        }
    }
    let shown = |s: &Seen| {
        let repeats = s.children.is_empty() && matches!(s.role.as_str(), "text" | "static") && s.parent.is_some_and(|p| !seen[p].name.is_empty() && seen[p].name == s.text);
        !s.bare() && !repeats && (everything || s.rect.is_some() || s.parent.is_none())
    };
    // How deep each one is in what was walked.
    let deep = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = seen[i].parent {
            d += 1;
            i = p;
        }
        d
    };
    // Which are given: all, or those that match and what they are in. The
    // element at a point is the deepest whose box holds it (the toolkits'
    // own `GetAccessibleAtPoint` stops at some containers: Qt's splitters).
    let pick: Vec<bool> = match ask {
        Ask::Tree(_) | Ask::Since(..) => (0..seen.len()).map(|i| !seen[i].pruned && shown(&seen[i])).collect(),
        Ask::Focus | Ask::Find(_) | Ask::At(..) => {
            let needle = if let Ask::Find(t) = ask { t.to_lowercase() } else { String::new() };
            let holds = |s: &Seen, x: i32, y: i32| s.rect.is_some_and(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]);
            let under = match ask {
                Ask::At(x, y) => (0..seen.len()).filter(|&i| !seen[i].pruned && shown(&seen[i]) && holds(&seen[i], *x, *y)).max_by_key(|&i| (deep(i), i)),
                _ => None,
            };
            let hit = |i: usize, s: &Seen| match ask {
                Ask::Focus => s.has(FOCUSED),
                Ask::At(..) => Some(i) == under,
                _ => !needle.is_empty() && (s.name.to_lowercase().contains(&needle) || s.text.to_lowercase().contains(&needle)),
            };
            let mut pick = vec![false; seen.len()];
            for i in 0..seen.len() {
                if seen[i].pruned || !hit(i, &seen[i]) {
                    continue;
                }
                let mut at = Some(i);
                while let Some(k) = at {
                    pick[k] |= shown(&seen[k]) || k == i;
                    at = seen[k].parent;
                }
            }
            pick
        }
    };
    let mut out = Vec::new();
    // Each one's own, then what it holds: (element, depth given).
    let mut stack = vec![(0usize, 0usize)];
    while let Some((i, depth)) = stack.pop() {
        if seen[i].pruned && i != 0 {
            continue;
        }
        let given = pick[i];
        if given {
            if out.len() >= GIVEN {
                break;
            }
            let s = &seen[i];
            let text = if s.text == s.name { String::new() } else { s.text.clone() };
            out.push(Node { id: kept.id(&s.key), depth, role: s.role.clone(), rect: s.rect, states: s.state_names(everything), name: s.name.clone(), text });
        }
        let below = if given { depth + 1 } else { depth };
        for &k in kids[i].iter().rev() {
            stack.push((k, below));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(key: &str, parent: Option<usize>, role: &str, name: &str, rect: Option<[i32; 4]>) -> Seen {
        let states = (1u64 << SHOWING) | (1 << VISIBLE) | (1 << ENABLED);
        Seen { key: (":1.1".into(), key.into()), parent, role: role.into(), name: name.into(), states, rect, ..Seen::default() }
    }

    #[test]
    fn a_record_says_its_box_states_and_hex_text() {
        let n = Node { id: 24, depth: 1, role: "push_button".into(), rect: Some([260, 610, 180, 36]), states: vec!["focusable"], name: "Merge".into(), text: String::new() };
        assert_eq!(n.record(), "24 1 push_button 260 610 180 36 focusable 4d65726765 -");
        let none = Node { rect: None, states: Vec::new(), ..n };
        assert_eq!(none.record(), "24 1 push_button - - - - - 4d65726765 -");
    }

    #[test]
    fn containers_that_say_nothing_are_left_out_and_their_children_kept() {
        let mut kept = Kept::default();
        let seen = vec![
            s("/f", None, "frame", "Page", Some([0, 0, 800, 600])),
            s("/p", Some(0), "panel", "", Some([0, 0, 800, 600])),
            s("/b", Some(1), "push_button", "Save", Some([10, 10, 80, 30])),
            s("/l", Some(0), "link", "Home", Some([10, 50, 80, 20])),
        ];
        let nodes = give(&seen, &Ask::Tree(false), &mut kept);
        let got: Vec<(String, usize)> = nodes.iter().map(|n| (n.name.clone(), n.depth)).collect();
        assert_eq!(got, vec![("Page".into(), 0), ("Save".into(), 1), ("Home".into(), 1)]);
    }

    #[test]
    fn an_element_keeps_its_number_between_reads() {
        let mut kept = Kept::default();
        let seen = vec![s("/f", None, "frame", "Page", Some([0, 0, 800, 600])), s("/b", Some(0), "push_button", "Save", Some([10, 10, 80, 30]))];
        let first = give(&seen, &Ask::Tree(false), &mut kept);
        let again = give(&seen[..], &Ask::Tree(false), &mut kept);
        assert_eq!(first.iter().map(|n| n.id).collect::<Vec<_>>(), again.iter().map(|n| n.id).collect::<Vec<_>>());
        let other = vec![s("/f", None, "frame", "Page", Some([0, 0, 800, 600])), s("/c", Some(0), "push_button", "Cancel", Some([10, 50, 80, 30]))];
        let third = give(&other, &Ask::Tree(false), &mut kept);
        assert_eq!(third[1].id, 3);
    }

    #[test]
    fn a_search_gives_what_matches_and_what_it_is_in() {
        let mut kept = Kept::default();
        let seen = vec![
            s("/f", None, "frame", "Page", Some([0, 0, 800, 600])),
            s("/m", Some(0), "menu_bar", "Menu", Some([0, 0, 800, 30])),
            s("/b", Some(1), "menu_item", "Save As", Some([0, 0, 80, 30])),
            s("/o", Some(0), "link", "Other", Some([0, 40, 80, 30])),
        ];
        let nodes = give(&seen, &Ask::Find("save".into()), &mut kept);
        let got: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(got, vec!["Page", "Menu", "Save As"]);
    }

    #[test]
    fn the_element_at_a_point_is_the_deepest_whose_box_holds_it() {
        let mut kept = Kept::default();
        let seen = vec![
            s("/f", None, "frame", "Calc", Some([0, 0, 1000, 1000])),
            s("/s", Some(0), "splitter", "", Some([0, 30, 1000, 800])),
            s("/t", Some(1), "text", "Result", Some([0, 30, 1000, 700])),
            s("/x", Some(2), "filler", "", Some([1, 31, 998, 698])),
            s("/b", Some(0), "push_button", "4", Some([600, 900, 80, 30])),
        ];
        let at = |x, y, kept: &mut Kept| give(&seen, &Ask::At(x, y), kept).iter().map(|n| n.name.clone()).collect::<Vec<_>>();
        assert_eq!(at(150, 140, &mut kept), vec!["Calc", "", "Result"]);
        assert_eq!(at(650, 910, &mut kept), vec!["Calc", "4"]);
        assert_eq!(at(10, 950, &mut kept), vec!["Calc"]);
    }

    #[test]
    fn a_text_leaf_repeating_its_parent_is_left_out() {
        let mut kept = Kept::default();
        let mut leaf = s("/t", Some(1), "text", "", Some([10, 10, 80, 30]));
        leaf.text = "Save".into();
        let seen = vec![s("/f", None, "frame", "Page", Some([0, 0, 800, 600])), s("/b", Some(0), "push_button", "Save", Some([10, 10, 80, 30])), leaf];
        let nodes = give(&seen, &Ask::Tree(false), &mut kept);
        assert_eq!(nodes.len(), 2);
    }

    fn n(id: u32, role: &str, name: &str, rect: [i32; 4], states: Vec<&'static str>) -> Node {
        Node { id, depth: (id > 1) as usize, role: role.into(), rect: Some(rect), states, name: name.into(), text: String::new() }
    }

    #[test]
    fn changes_give_what_came_what_went_and_what_is_other() {
        let then = vec![n(1, "frame", "Page", [0, 0, 800, 600], vec![]), n(2, "check_box", "Bold", [10, 10, 80, 20], vec!["focusable"]), n(3, "page_tab", "Page 1", [10, 40, 80, 20], vec!["selected"]), n(4, "label", "One", [10, 70, 80, 20], vec![])];
        let now = vec![n(1, "frame", "Page", [0, 0, 800, 600], vec![]), n(2, "check_box", "Bold", [10, 10, 80, 20], vec!["focusable", "checked"]), n(3, "page_tab", "Page 1", [10, 40, 80, 20], vec!["selected"]), n(5, "label", "Two", [10, 70, 80, 20], vec![])];
        assert_eq!(changes(&then, &now, false), vec!["~ 2 1 check_box 10 10 80 20 focusable,checked 426f6c64 -", "+ 5 1 label 10 70 80 20 - 54776f -", "- 4"]);
        assert!(changes(&now, &now, true).is_empty());
    }

    #[test]
    fn boxes_are_compared_only_when_asked() {
        let then = vec![n(1, "frame", "Page", [0, 0, 800, 600], vec![]), n(2, "link", "Home", [10, 10, 80, 20], vec![])];
        let now = vec![n(1, "frame", "Page", [0, 0, 800, 600], vec![]), n(2, "link", "Home", [10, -90, 80, 20], vec![])];
        assert!(changes(&then, &now, false).is_empty());
        assert_eq!(changes(&then, &now, true), vec!["~ 2 1 link 10 -90 80 20 - 486f6d65 -"]);
    }

    #[test]
    fn a_frame_that_is_another_or_renamed_is_not_the_same() {
        let a = vec![n(1, "frame", "Page", [0, 0, 800, 600], vec![])];
        assert!(same_frame(&a, &[n(1, "frame", "Page", [0, 0, 640, 480], vec!["focused"])]));
        assert!(!same_frame(&a, &[n(1, "frame", "Other page", [0, 0, 800, 600], vec![])]));
        assert!(!same_frame(&a, &[n(9, "dialog", "Page", [0, 0, 800, 600], vec![])]));
        assert!(!same_frame(&a, &[]));
    }
}
