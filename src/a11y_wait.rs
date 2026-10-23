//! Waiting for something to happen in a window (`W` on the agent's socket),
//! so the agent does not look again and again: a condition on what its
//! program published (`a11y_tree.rs`), or on what the window manager sees of
//! it (it drew, a dialog of it opened).
//!
//! The tree is walked again when its program says something in it changed
//! (the events a wait needs are registered while it lives), at most every
//! 50 ms and a quarter of the time, and every second whatever it says.
//!
//! Listening (`O`) is the screen reader's side of it: no condition, but a
//! line for each thing a reader would say, as it happens.

use crate::a11y_tree::Node;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The longest wait, in milliseconds.
pub const MOST: u64 = 60_000;
/// The least between two walks.
const EVERY: Duration = Duration::from_millis(50);
/// The most between two walks, events or not.
const ANYWAY: Duration = Duration::from_secs(1);
/// What says that a tree changed, in the form programs compare (see `HEARD`).
const EVENTS: &[&str] = &["Object:ChildrenChanged", "Object:StateChanged", "Object:PropertyChange", "Object:TextChanged", "Focus:"];
/// The states a condition can name: as records say them, and `enabled`.
const STATES: &[&str] = &["focused", "focusable", "checked", "pressed", "selected", "expanded", "collapsed", "editable", "disabled", "required", "invalid", "busy", "enabled"];

/// What is waited for.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    /// An element shown has that in its name or text (case folded).
    Has(String),
    /// None has.
    Gone(String),
    /// That element is no longer shown.
    IdGone(u32),
    /// That element has that state (false: has it not).
    State(u32, String, bool),
    /// The focus is on an element with that in its name.
    Focus(String),
    /// Anything shown changed: an element, its role, name, text or states.
    Changed,
    /// Neither the tree nor the window's picture changed for so many ms.
    Settled(u64),
    /// A dialog of the window opened.
    Dialog,
}

impl Condition {
    /// From the words after `W TARGET MS`; texts hex encoded.
    pub fn parse(w: &[&str], unhex: impl Fn(&str) -> Result<Vec<u8>, &'static str>) -> Result<Condition, &'static str> {
        let text = |h: &str| unhex(h).map(|b| String::from_utf8_lossy(&b).to_lowercase()).ok().filter(|t| !t.is_empty()).ok_or("bad-args");
        let state = |s: &str| STATES.contains(&s).then(|| s.to_owned()).ok_or("unknown-state");
        Ok(match w {
            ["has", hex] => Condition::Has(text(hex)?),
            ["gone", hex] => Condition::Gone(text(hex)?),
            ["focus", hex] => Condition::Focus(text(hex)?),
            ["changed"] => Condition::Changed,
            ["dialog"] => Condition::Dialog,
            ["settled", ms] => Condition::Settled(ms.parse().ok().filter(|ms| *ms <= MOST).ok_or("bad-args")?),
            [id, "gone"] => Condition::IdGone(id.parse().map_err(|_| "bad-args")?),
            [id, "not", s] => Condition::State(id.parse().map_err(|_| "bad-args")?, state(s)?, false),
            [id, s] => Condition::State(id.parse().map_err(|_| "bad-args")?, state(s)?, true),
            _ => return Err("bad-args"),
        })
    }

    /// Whether it is about the tree (`settled` reads it only if there is one).
    pub fn needs_tree(&self) -> bool {
        !matches!(self, Condition::Settled(_) | Condition::Dialog)
    }

    /// The element it names, if it names one.
    fn id(&self) -> Option<u32> {
        match self {
            Condition::IdGone(id) | Condition::State(id, ..) => Some(*id),
            _ => None,
        }
    }

    /// Whether it holds on what is shown now (`first`: what was at the
    /// start): and the element that met it, if one did. `settled` and
    /// `dialog` are the window manager's.
    pub fn holds<'a>(&self, first: &[Node], now: &'a [Node]) -> Option<Option<&'a Node>> {
        let says = |n: &Node, t: &str| n.name.to_lowercase().contains(t) || n.text.to_lowercase().contains(t);
        match self {
            Condition::Has(t) => now.iter().find(|n| says(n, t)).map(Some),
            Condition::Gone(t) => (!now.iter().any(|n| says(n, t))).then_some(None),
            Condition::IdGone(id) => (!now.iter().any(|n| n.id == *id)).then_some(None),
            Condition::State(id, s, want) => now.iter().find(|n| n.id == *id && has(n, s) == *want).map(Some),
            Condition::Focus(t) => now.iter().find(|n| n.states.contains(&"focused") && n.name.to_lowercase().contains(t.as_str())).map(Some),
            Condition::Changed => differs(first, now).then_some(None),
            Condition::Settled(_) | Condition::Dialog => None,
        }
    }
}

fn has(n: &Node, state: &str) -> bool {
    match state {
        "enabled" => !n.states.contains(&"disabled"),
        s => n.states.contains(&s),
    }
}

/// Whether two walks differ in what they give: boxes left out, as `since`
/// does (a scroll moves every one).
fn differs(a: &[Node], b: &[Node]) -> bool {
    let said = |n: &Node| (n.id, n.depth, n.role.clone(), n.states.clone(), n.name.clone(), n.text.clone());
    a.len() != b.len() || a.iter().zip(b).any(|(a, b)| said(a) != said(b))
}

/// A wait under way: what the window manager decides is decided on its
/// loop (`Pending::Wait` in `agent.rs`), the tree on a thread of its own,
/// which ends when this is let go.
pub struct Wait {
    pub condition: Condition,
    pub slot: usize,
    pub pid: u32,
    pub since: Instant,
    pub deadline: Instant,
    /// The windows open when it began: a dialog is one that was not.
    pub open: Vec<usize>,
    answer: Receiver<String>,
    /// When the tree last changed.
    changed: Arc<Mutex<Instant>>,
    over: Arc<AtomicBool>,
}

impl Wait {
    /// `guarded`: a window of the program is private (see `a11y_tree::read`).
    #[allow(clippy::too_many_arguments)]
    pub fn start(condition: Condition, slot: usize, pid: u32, title: String, guarded: bool, family: impl Fn(u32) -> bool + Send + 'static, ms: u64, open: Vec<usize>) -> Wait {
        let since = Instant::now();
        let (tx, answer) = std::sync::mpsc::channel();
        let (changed, over) = (Arc::new(Mutex::new(since)), Arc::new(AtomicBool::new(false)));
        if condition != Condition::Dialog {
            let (condition, changed, over) = (condition.clone(), changed.clone(), over.clone());
            std::thread::spawn(move || watch(&condition, slot, pid, (&title, guarded), family, &tx, &changed, &over));
        }
        Wait { condition, slot, pid, since, deadline: since + Duration::from_millis(ms.min(MOST)), open, answer, changed, over }
    }

    /// The tree's answer, once it has one.
    pub fn answered(&self) -> Option<String> {
        self.answer.try_recv().ok()
    }

    /// Since when neither the tree nor (`drew`) the window changed.
    pub fn quiet_since(&self, drew: Instant) -> Instant {
        self.since.max(drew).max(*self.changed.lock().unwrap())
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        self.over.store(true, Ordering::Relaxed);
    }
}

/// The tree, walked until the condition holds or the wait is over: `ok` and
/// the record that met it, or why it cannot be; for `settled`, only when it
/// last changed.
#[allow(clippy::too_many_arguments)]
fn watch(condition: &Condition, slot: usize, pid: u32, (title, guarded): (&str, bool), family: impl Fn(u32) -> bool, tx: &Sender<String>, changed: &Mutex<Instant>, over: &AtomicBool) {
    let tell = |s: String| {
        if condition.needs_tree() {
            let _ = tx.send(s);
        }
    };
    let Some(listener) = crate::a11y::listen(EVENTS) else { return tell("err no-bus".into()) };
    let start = Instant::now();
    let (names, _, first) = match crate::a11y_tree::shown(slot, pid, title, guarded, &family) {
        Ok(got) => got,
        Err(e) => return tell(format!("err {e}")),
    };
    if condition.id().is_some_and(|id| crate::a11y_tree::key_of(pid, slot, id).is_none()) {
        return tell("err unknown-id".into());
    }
    let mut last = first.clone();
    let (mut walked, mut took) = (start, start.elapsed());
    loop {
        if let Some(met) = condition.holds(&first, &last) {
            return tell(met.map_or("ok".into(), |n| format!("ok {}", n.record())));
        }
        // An event from its program, or a second gone by.
        let woke = Instant::now();
        let mut stirred = false;
        while !stirred && woke.elapsed() < ANYWAY && !over.load(Ordering::Relaxed) {
            let signal = futures_lite::future::block_on(futures_lite::future::or(async { listener.signals.recv().await.ok() }, async {
                async_io::Timer::after(Duration::from_millis(100)).await;
                None
            }));
            stirred = signal.is_some_and(|m| m.header().sender().is_some_and(|s| names.iter().any(|n| n == s.as_str())));
        }
        if over.load(Ordering::Relaxed) {
            return;
        }
        // A program that never stops saying something (a spinner's states)
        // is not walked more than a quarter of the time: each walk asks it
        // hundreds of things. Slept in short steps: a wait over lets its
        // events go at once.
        let until = Instant::now() + EVERY.max(took * 3).saturating_sub(walked.elapsed().saturating_sub(took));
        while let Some(left) = until.checked_duration_since(Instant::now()).filter(|l| !l.is_zero()) {
            if over.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(left.min(EVERY));
        }
        while listener.signals.try_recv().is_ok() {}
        walked = Instant::now();
        // A walk that fails (the window closing) is the window manager's to say.
        let got = crate::a11y_tree::shown(slot, pid, title, guarded, &family);
        took = walked.elapsed();
        let Ok((_, _, now)) = got else { continue };
        if differs(&last, &now) {
            *changed.lock().unwrap() = Instant::now();
        }
        last = now;
    }
}

/// What a listen hears (`O`, `agent listen`): what a screen reader says. As
/// the registry says them to programs: atk-bridge and Qt compare their
/// events with these as they are, and take no `object:text-changed`.
const HEARD: &[&str] = &["Focus:", "Object:StateChanged:Focused", "Object:TextChanged", "Object:ChildrenChanged", "Object:Announcement"];
/// The longest listen, in milliseconds.
pub const LONGEST: u64 = 600_000;
/// An element's changes within this are one line.
const GATHER: Duration = Duration::from_millis(150);
/// Elements read in a second for texts and live regions, at most: a
/// program that never stops saying something is heard in that much.
const READS: u32 = 20;
/// Elements whose being live or not is kept, and texts last said.
const KNOWN: usize = 2048;

/// What one of a program's events says.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// The element that sent it has the focus now.
    Focus,
    /// Its text changed.
    Text,
    /// That one was put in it, if it says which.
    Added(Option<(String, String)>),
    /// It asks for that to be said.
    Announce(String),
}

/// An event (`org.a11y.atspi.Event.*`, its body `(siiva{sv})`), as what it says.
pub fn heard(m: &zbus::Message) -> Option<Heard> {
    use zbus::zvariant::Value;
    const OBJECT: &str = "org.a11y.atspi.Event.Object";
    let h = m.header();
    let (interface, member) = (h.interface()?.as_str(), h.member()?.as_str());
    // Most of what a program says is none of these (GTK's boxes moving).
    if !matches!(member, "Focus" | "StateChanged" | "TextChanged" | "ChildrenChanged" | "Announcement") {
        return None;
    }
    let body = m.body();
    let s = body.deserialize::<zbus::zvariant::Structure>().ok()?;
    let f = s.fields();
    let kind = match f.first() {
        Some(Value::Str(k)) => k.as_str(),
        _ => "",
    };
    let data = match f.get(3) {
        Some(Value::Value(v)) => Some(&**v),
        v => v,
    };
    match (interface, member) {
        ("org.a11y.atspi.Event.Focus", "Focus") => Some(Heard::Focus),
        (OBJECT, "StateChanged") if kind == "focused" && matches!(f.get(1), Some(Value::I32(1))) => Some(Heard::Focus),
        (OBJECT, "TextChanged") => Some(Heard::Text),
        (OBJECT, "ChildrenChanged") if kind.starts_with("add") => Some(Heard::Added(match data {
            Some(Value::Structure(c)) => match c.fields() {
                [Value::Str(n), Value::ObjectPath(p)] => Some((n.to_string(), p.to_string())),
                _ => None,
            },
            _ => None,
        })),
        (OBJECT, "Announcement") => match data {
            Some(Value::Str(t)) if !t.is_empty() => Some(Heard::Announce(t.to_string())),
            _ => None,
        },
        _ => None,
    }
}

/// A line of a listen: what happened, and the element's record.
fn line(what: &str, n: &Node) -> String {
    format!("{what} {}", n.record())
}

/// A listen under way (`O`): its lines come from a thread that hears the
/// window's program; Stop, the window closing, its dialogs and its time are
/// the window manager's (`Pending::Listen` in `agent.rs`).
pub struct Listen {
    pub slot: usize,
    pub pid: u32,
    pub since: Instant,
    pub deadline: Instant,
    /// The windows open: a dialog is one that was not.
    pub open: Vec<usize>,
    lines: Receiver<String>,
    /// The program's events heard, and the elements read for them.
    counts: Arc<[AtomicU64; 2]>,
    over: Arc<AtomicBool>,
}

impl Listen {
    pub fn start(slot: usize, pid: u32, title: String, guarded: bool, family: impl Fn(u32) -> bool + Send + 'static, ms: u64, open: Vec<usize>) -> Listen {
        let since = Instant::now();
        let (tx, lines) = std::sync::mpsc::channel();
        let (counts, over) = (Arc::new(<[AtomicU64; 2]>::default()), Arc::new(AtomicBool::new(false)));
        let (c, o) = (counts.clone(), over.clone());
        std::thread::spawn(move || hear(slot, pid, (&title, guarded), family, &tx, &c, &o));
        Listen { slot, pid, since, deadline: since + Duration::from_millis(ms.min(LONGEST)), open, lines, counts, over }
    }

    /// The lines heard since last asked.
    pub fn said(&self) -> Vec<String> {
        self.lines.try_iter().collect()
    }

    /// Its last line: why it ended, the events heard and the elements read.
    pub fn end(&self, why: &str) -> String {
        format!("end {why} {} {}", self.counts[0].load(Ordering::Relaxed), self.counts[1].load(Ordering::Relaxed))
    }
}

impl Drop for Listen {
    fn drop(&mut self) {
        self.over.store(true, Ordering::Relaxed);
    }
}

/// The program heard until the listen is over: the focus it starts on, then
/// a line per focus change, change of the focused element's text, live
/// region's news and announcement. An element's changes are gathered for a
/// moment into one line, and what is not the focus is read at most `READS`
/// times a second. Only what is in the window's frame is said: the program's
/// other windows (one of them private) are not this one.
fn hear(slot: usize, pid: u32, (title, guarded): (&str, bool), family: impl Fn(u32) -> bool, tx: &Sender<String>, counts: &[AtomicU64; 2], over: &AtomicBool) {
    use crate::a11y_tree::{element, key_of, live, within};
    type Key = (String, String);
    let Some(listener) = crate::a11y::listen(HEARD) else { return drop(tx.send("no-tree".into())) };
    let (names, frame, now) = match crate::a11y_tree::shown(slot, pid, title, guarded, &family) {
        Ok(got) => got,
        Err(_) => return drop(tx.send("no-tree".into())),
    };
    let first = now.iter().find(|n| n.states.contains(&"focused"));
    let mut focus: Option<Key> = first.and_then(|n| key_of(pid, slot, n.id));
    if let Some(n) = first {
        let _ = tx.send(line("focus", n));
    }
    let read = |k: &Key| {
        counts[1].fetch_add(1, Ordering::Relaxed);
        element(slot, pid, k.clone())
    };
    // Elements to read once their moment is over: what for, and since when.
    let mut gathered: Vec<(Key, &str, Instant)> = Vec::new();
    let mut known: HashMap<Key, bool> = HashMap::new();
    // Whether an element is in the frame, asked once.
    let mut inside: HashMap<Key, bool> = HashMap::new();
    let mut last: HashMap<Key, String> = HashMap::new();
    let mut second = (Instant::now(), 0u32);
    while !over.load(Ordering::Relaxed) {
        // Until the first gathered is due, or the second's reads are given again.
        let due = if second.1 >= READS { Some(second.0 + Duration::from_secs(1)) } else { gathered.iter().map(|g| g.2 + GATHER).min() };
        let next = due.map_or(Duration::from_millis(100), |at| at.saturating_duration_since(Instant::now()));
        let signal = futures_lite::future::block_on(futures_lite::future::or(async { listener.signals.recv().await.ok() }, async {
            async_io::Timer::after(next.min(Duration::from_millis(100))).await;
            None
        }));
        if let Some(m) = signal {
            let hdr = m.header();
            let Some(sender) = hdr.sender().map(|s| s.to_string()).filter(|s| names.contains(s)) else { continue };
            counts[0].fetch_add(1, Ordering::Relaxed);
            let (Some(heard), Some(path)) = (heard(&m), hdr.path()) else { continue };
            let key = (sender, path.to_string());
            if known.len() > KNOWN {
                known.clear();
            }
            if inside.len() > KNOWN {
                inside.clear();
            }
            let mut is_live = |k: &Key| *known.entry(k.clone()).or_insert_with(|| live(k));
            let mut ours = |k: &Key| within(k, &frame, &mut inside);
            match heard {
                // The focus gone to another window of the program: none here.
                Heard::Focus if focus.as_ref() != Some(&key) && !ours(&key) => focus = None,
                Heard::Focus if focus.as_ref() != Some(&key) => {
                    if let Some(n) = read(&key) {
                        let _ = tx.send(line("focus", &n));
                        last.insert(key.clone(), n.text);
                    }
                    focus = Some(key);
                }
                Heard::Text if focus.as_ref() == Some(&key) => gather(&mut gathered, key, "text"),
                Heard::Text if is_live(&key) && ours(&key) => gather(&mut gathered, key, "live"),
                // What is put in it is in it only on its own connection.
                Heard::Added(child) if is_live(&key) && ours(&key) => gather(&mut gathered, child.filter(|c| c.0 == key.0).unwrap_or(key), "live"),
                Heard::Announce(said) if ours(&key) => {
                    let hex: String = said.bytes().map(|b| format!("{b:02x}")).collect();
                    let _ = tx.send(read(&key).map_or_else(|| format!("announce {hex}"), |n| line(&format!("announce {hex}"), &n)));
                }
                Heard::Focus | Heard::Text | Heard::Added(_) | Heard::Announce(_) => (),
            }
        }
        // What is due, within the second's reads.
        if second.0.elapsed() >= Duration::from_secs(1) {
            second = (Instant::now(), 0);
        }
        // Those nearly due go with them: one change is often several events.
        let mut batch = Vec::new();
        let due = gathered.iter().any(|g| g.2.elapsed() >= GATHER);
        while due && second.1 < READS {
            let Some(k) = gathered.iter().position(|g| g.2.elapsed() >= GATHER / 2) else { break };
            second.1 += 1;
            let (key, what, _) = gathered.remove(k);
            // The same text again is not news.
            if let Some(n) = read(&key).filter(|n| last.get(&key) != Some(&n.text)) {
                last.insert(key, n.text.clone());
                batch.push((what, n));
            }
        }
        for n in said(&batch) {
            let _ = tx.send(n);
        }
        if last.len() > KNOWN {
            last.clear();
        }
    }
}

/// The lines of elements read together: a live region's text and the leaf
/// put in it that says the same are one line (Firefox sends both events),
/// and a live region that says nothing is no news.
fn said(batch: &[(&str, Node)]) -> Vec<String> {
    let says = |n: &Node| if n.text.is_empty() { n.name.clone() } else { n.text.clone() };
    let repeated = |i: usize, n: &Node| {
        let s = says(n);
        s.is_empty() || batch.iter().enumerate().any(|(j, (w, o))| j != i && *w == "live" && o.text.contains(&s) && (n.text != o.text || j < i))
    };
    batch.iter().enumerate().filter(|(i, (what, n))| *what != "live" || !repeated(*i, n)).map(|(_, (what, n))| line(what, n)).collect()
}

/// An element to read when its moment is over, unless it waits already.
fn gather<'a>(gathered: &mut Vec<((String, String), &'a str, Instant)>, key: (String, String), what: &'a str) {
    if !gathered.iter().any(|g| g.0 == key) {
        gathered.push((key, what, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(h: &str) -> Result<Vec<u8>, &'static str> {
        (0..h.len() / 2).map(|k| u8::from_str_radix(&h[2 * k..2 * k + 2], 16).map_err(|_| "bad-hex")).collect()
    }

    fn n(id: u32, name: &str, states: &[&'static str]) -> Node {
        Node { id, depth: 1, role: "push_button".into(), rect: Some([0, 0, 10, 10]), states: states.to_vec(), name: name.into(), text: String::new() }
    }

    #[test]
    fn conditions_are_read_from_their_words() {
        let p = |s: &str| Condition::parse(&s.split(' ').collect::<Vec<_>>(), unhex);
        assert_eq!(p("has 5361766564"), Ok(Condition::Has("saved".into())));
        assert_eq!(p("gone 4c6f6164696e67"), Ok(Condition::Gone("loading".into())));
        assert_eq!(p("24 gone"), Ok(Condition::IdGone(24)));
        assert_eq!(p("24 enabled"), Ok(Condition::State(24, "enabled".into(), true)));
        assert_eq!(p("24 not busy"), Ok(Condition::State(24, "busy".into(), false)));
        assert_eq!(p("focus 4f4b"), Ok(Condition::Focus("ok".into())));
        assert_eq!(p("settled 500"), Ok(Condition::Settled(500)));
        assert_eq!(p("changed"), Ok(Condition::Changed));
        assert_eq!(p("dialog"), Ok(Condition::Dialog));
        assert_eq!(p("24 shiny"), Err("unknown-state"));
        assert_eq!(p("has"), Err("bad-args"));
        assert_eq!(p("has zz"), Err("bad-args"));
        assert_eq!(p("settled 70000"), Err("bad-args"));
    }

    #[test]
    fn a_condition_holds_on_what_is_shown_and_gives_what_met_it() {
        let first = vec![n(1, "Window", &[]), n(2, "Save", &["focusable"])];
        let now = vec![n(1, "Window", &[]), n(2, "Save", &["focusable", "focused", "disabled"]), n(3, "Saved to disk", &[])];
        let met = |c: Condition| c.holds(&first, &now).map(|m| m.map(|n| n.id));
        assert_eq!(met(Condition::Has("saved".into())), Some(Some(3)));
        assert_eq!(met(Condition::Has("nothing".into())), None);
        assert_eq!(met(Condition::Gone("loading".into())), Some(None));
        assert_eq!(met(Condition::Gone("save".into())), None);
        assert_eq!(met(Condition::IdGone(2)), None);
        assert_eq!(met(Condition::IdGone(7)), Some(None));
        assert_eq!(met(Condition::State(2, "enabled".into(), false)), Some(Some(2)));
        assert_eq!(met(Condition::State(2, "enabled".into(), true)), None);
        assert_eq!(met(Condition::State(2, "busy".into(), false)), Some(Some(2)));
        assert_eq!(met(Condition::Focus("sav".into())), Some(Some(2)));
        assert_eq!(met(Condition::Changed), Some(None));
        assert_eq!(Condition::Changed.holds(&first, &first), None);
        assert_eq!(Condition::Settled(100).holds(&first, &now), None);
    }

    fn event(interface: &str, member: &str, kind: &str, detail1: i32, data: zbus::zvariant::Value) -> zbus::Message {
        let props = HashMap::<String, zbus::zvariant::Value>::new();
        zbus::Message::signal("/org/a11y/atspi/accessible/7", interface, member).unwrap().sender(":1.4").unwrap().build(&(kind, detail1, 0i32, data, props)).unwrap()
    }

    #[test]
    fn what_an_event_says() {
        use zbus::zvariant::{ObjectPath, Value};
        const OBJECT: &str = "org.a11y.atspi.Event.Object";
        assert_eq!(heard(&event("org.a11y.atspi.Event.Focus", "Focus", "", 0, Value::I32(0))), Some(Heard::Focus));
        assert_eq!(heard(&event(OBJECT, "StateChanged", "focused", 1, Value::I32(0))), Some(Heard::Focus));
        // Losing the focus, or another state, says nothing.
        assert_eq!(heard(&event(OBJECT, "StateChanged", "focused", 0, Value::I32(0))), None);
        assert_eq!(heard(&event(OBJECT, "StateChanged", "busy", 1, Value::I32(0))), None);
        assert_eq!(heard(&event(OBJECT, "TextChanged", "insert", 0, Value::from("4"))), Some(Heard::Text));
        let child = Value::from((":1.4", ObjectPath::try_from("/org/a11y/atspi/accessible/9").unwrap()));
        assert_eq!(heard(&event(OBJECT, "ChildrenChanged", "add", 2, child.clone())), Some(Heard::Added(Some((":1.4".into(), "/org/a11y/atspi/accessible/9".into())))));
        assert_eq!(heard(&event(OBJECT, "ChildrenChanged", "remove", 2, child)), None);
        assert_eq!(heard(&event(OBJECT, "Announcement", "", 0, Value::from("Saved"))), Some(Heard::Announce("Saved".into())));
        assert_eq!(heard(&event(OBJECT, "Announcement", "", 0, Value::from(""))), None);
        assert_eq!(heard(&event(OBJECT, "PropertyChange", "accessible-value", 0, Value::I32(3))), None);
    }

    #[test]
    fn a_live_region_and_its_new_leaf_are_one_line() {
        let region = Node { role: "section".into(), text: "tick 13".into(), ..n(30, "", &[]) };
        let leaf = Node { role: "unknown".into(), ..n(34, "tick 13", &[]) };
        let field = Node { role: "entry".into(), text: "tick 13".into(), ..n(5, "", &["focused"]) };
        let lines = said(&[("live", region.clone()), ("live", leaf.clone()), ("text", field)]);
        assert_eq!(lines, vec!["live 30 1 section 0 0 10 10 - - 7469636b203133", "text 5 1 entry 0 0 10 10 focused - 7469636b203133"]);
        assert_eq!(said(&[("live", leaf.clone()), ("live", region.clone())]), vec!["live 30 1 section 0 0 10 10 - - 7469636b203133"]);
        // Two regions that say the same: once.
        assert_eq!(said(&[("live", region.clone()), ("live", Node { id: 31, ..region.clone() })]).len(), 1);
        assert!(said(&[("live", Node { text: String::new(), ..region })]).is_empty());
        assert_eq!(said(&[("live", leaf)]).len(), 1);
    }

    #[test]
    fn a_box_that_moved_is_not_a_change() {
        let a = vec![n(1, "Window", &[])];
        let mut b = a.clone();
        b[0].rect = Some([5, 5, 10, 10]);
        assert!(!differs(&a, &b));
        b[0].text = "now".into();
        assert!(differs(&a, &b));
    }
}
