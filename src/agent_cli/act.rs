//! Acting on a window: `press`, `focus`, `type`, `key`, `hotkey`; on an
//! element by the number a tree gave it, or a scene's item by its name.
//!
//! An action answers what it did, then what it changed: the window's
//! version is taken before it, and read again once the window has drawn
//! and stopped (`W drawn`); `--until` waits for a condition instead.

use super::*;

const PRESS: Spec = Spec { element: true, flags: &["--first", "--no-wait"], values: &["--text", "--action", "--max"], ..PLAIN };
const FOCUS: Spec = Spec { element: true, flags: &["--first", "--no-wait"], values: &["--text", "--max"], ..PLAIN };
const TYPE: Spec = Spec { element: true, flags: &["--set", "--no-wait"], values: &["--max"], ..SHELL };
const KEY: Spec = Spec { flags: &["--no-wait"], values: &["--max"], ..SHELL };
/// The pointer's: `click`, `drag`, `scroll`.
pub(super) const POINTER: Spec = Spec { flags: &["--no-wait"], values: &["--max"], ..PLAIN };

/// The most lines of changes an action answers, unless `--max N`.
const MAX: usize = 60;
/// How long an action waits for its window to draw, at most, and then to
/// draw nothing new.
const SETTLE_MS: u64 = 1000;
const QUIET_MS: u64 = 150;

/// The words of an action, and those of its `--until COND [DURATION]`: up
/// to `--` (after it, all is the action's text) or the end.
pub(super) fn split_until(words: &[String]) -> (Vec<String>, Option<Vec<String>>) {
    let dash = words.iter().position(|w| w == "--").unwrap_or(words.len());
    match words[..dash].iter().position(|w| w == "--until") {
        Some(u) => ([&words[..u], &words[dash..]].concat(), Some(words[u + 1..dash].to_vec())),
        None => (words.to_vec(), None),
    }
}

/// The window as it was before an action.
enum Before {
    /// A read of its tree, kept as that version.
    Tree(u64),
    /// A scene's items.
    Scene(Vec<Rec>),
    /// A terminal's mark.
    Text(u64),
    /// Pictures only.
    Look,
    /// Not taken: `--no-wait`.
    Nothing,
}

/// How the window was once it was waited for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Settled {
    /// It drew, then nothing new for a moment.
    Quiet,
    /// It did not stop drawing (a spinner, a video, a page loading): after so many seconds.
    Drawing(f64),
    /// It drew nothing in so many seconds.
    Still(f64),
    /// What it acted on closed (a dialog it was in).
    Closed,
    /// An older session, without `drawn`.
    Unknown,
}

impl Settled {
    /// What is said of it, if anything is.
    fn said(self) -> Option<String> {
        match self {
            Settled::Drawing(s) => Some(format!("still drawing after {s:.1} s: the changes so far")),
            Settled::Still(s) => Some(format!("drew nothing in {s:.1} s")),
            Settled::Closed => Some("closed: what it acted on closed".to_owned()),
            Settled::Quiet | Settled::Unknown => None,
        }
    }
}

/// An action under way: the window, its version before it, at most how
/// many changes are answered, and what is waited for after it.
pub(super) struct Acting {
    k: Kinds,
    w: Window,
    pub(super) h: Hands,
    before: Before,
    max: usize,
    until: Option<Vec<String>>,
}

impl Acting {
    /// Before acting on that window: its version, of what the action
    /// changes (`text`: a terminal's text, when it has one), unless
    /// `--no-wait` without `--until`. A wrong `--until` is refused now,
    /// before anything is done.
    pub(super) fn take(k: &Kinds, w: &Window, a: &Args, until: Option<Vec<String>>, text: bool, mut h: Hands) -> R<Acting> {
        if let Some(c) = &until {
            wait::check(c, k, w)?;
        }
        let max = a.number::<usize>("--max")?.filter(|m| *m > 0).unwrap_or(MAX);
        let before = if a.has("--no-wait") && until.is_none() {
            Before::Nothing
        } else if k.is_scene() {
            Before::Scene(described(k)?.recs)
        } else if let Some(m) = text.then(|| Term::of(k, w, "tree", false).and_then(|t| t.mark()).ok()).flatten() {
            Before::Text(m.0)
        } else if k.tree {
            let reply = h.say(&format!("A {} tree", w.target()))?;
            match reply.strip_prefix("nodes ").and_then(|v| v.split(' ').next()?.parse().ok()) {
                Some(v) => Before::Tree(v),
                None if reply == "err no-tree" || reply == "err no-frame" => Before::Look,
                None => return Err(answered(&reply, Of::Window(&k.name))),
            }
        } else {
            Before::Look
        };
        Ok(Acting { k: k.clone(), w: w.clone(), h, before, max, until })
    }

    /// The answer: what was done (`said`, then what the scene said of it),
    /// then the condition `--until` waited for, or how the window settled,
    /// and what changed since the version taken before. An element gone:
    /// what changed since this agent's last read, and exit 4.
    pub(super) fn answer(mut self, said: &str, done: R<String>) -> R {
        let told = match done {
            Ok(t) => t,
            Err(f) if f.code == fail::GONE => {
                let _ = self.gone();
                return Err(f);
            }
            Err(f) => return Err(f),
        };
        println!("{said}");
        print!("{told}");
        if let Some(cond) = self.until.take() {
            let mark = match self.before {
                Before::Text(m) => Some(m),
                _ => None,
            };
            let held = wait::until(&[&[self.k.name.clone()][..], &cond].concat(), mark);
            // A terminal's wait answers what came after the mark itself.
            if mark.is_none() {
                let changes = self.changes(Settled::Quiet);
                held.as_ref().map_or(Ok(()), |_| changes)?;
            }
            return held.map_err(|f| match f.code {
                fail::TIMEOUT => Fail::new(f.code, f.msg.replacen("timeout: ", &format!("timeout: {said}, then "), 1)),
                _ => f,
            });
        }
        if matches!(self.before, Before::Nothing) {
            return Ok(());
        }
        let settled = self.settle()?;
        let changes = self.changes(settled);
        // A dialog that closed: its window is read, if there is one.
        if settled == Settled::Closed {
            return Ok(());
        }
        changes
    }

    /// Until the window drew for it and stopped, at most a second; a window
    /// that draws all the time is given a moment only.
    fn settle(&mut self) -> R<Settled> {
        let start = std::time::Instant::now();
        let reply = self.h.say(&format!("W {} {SETTLE_MS} drawn {QUIET_MS}", self.w.target()))?;
        let took = start.elapsed().as_secs_f64();
        Ok(match reply.as_str() {
            "ok" => Settled::Quiet,
            "ok drawing" => Settled::Drawing(took),
            "err timeout" => Settled::Still(took),
            "err gone" => Settled::Closed,
            "err bad-args" | "err unknown-command" => Settled::Unknown,
            r => return Err(answered(r, Of::Window(&self.k.name))),
        })
    }

    /// How a change is said: as `tree --since` says it, at most `max`.
    fn said<'a>(&self, last: Option<String>, why_full: &'a str) -> Said<'a> {
        Said { last, why_full, role: None, max: Some(self.max), json: false, long: false }
    }

    /// What changed since the version taken before, as `tree --since V`
    /// answers it, kept as this agent's last read; pictures only: `done`.
    fn changes(&mut self, settled: Settled) -> R {
        let name = self.k.name.clone();
        if let Before::Look = self.before {
            println!("done{}", settled.said().map_or(String::new(), |s| format!(": {s}")));
            return Ok(());
        }
        if let Some(s) = settled.said() {
            println!("{s}");
        }
        match &self.before {
            Before::Tree(v) => {
                let reply = self.h.say(&format!("A {} tree since {v}", self.w.target()))?;
                keep_version(&name, &reply);
                print_since(since_reply(&reply, &name)?, &name, &self.said(None, "the window shows another page or dialog"));
            }
            Before::Scene(then) => {
                let now = described(&self.k)?.recs;
                let v = keep_scene(&name, self.k.scene.as_ref().map_or("", |s| s.0.as_str()), &now);
                let list = scene_changes(then, &now, false).into_iter().filter(|c| !matches!(c, Change::Came(r) if r.states.iter().any(|s| s == "offscreen"))).collect();
                print_since(Since::Changes { version: v.to_string(), list, more: 0 }, &name, &self.said(None, ""));
            }
            Before::Text(m) => {
                let t = Term::of(&self.k, &self.w, "tree", false)?;
                let reply = t.ask(&format!("lines {} {m}", crate::term::HISTORY))?;
                let (mark, text) = tail(&reply, self.max, &name, *m);
                keep(&name, mark);
                print!("{text}");
            }
            Before::Look | Before::Nothing => {}
        }
        Ok(())
    }

    /// An element gone: what changed in the window since this agent's last
    /// read of it, read now.
    fn gone(&mut self) -> R {
        let name = self.k.name.clone();
        let last = last_version(&name);
        if self.k.is_scene() {
            let now = described(&self.k)?;
            let scene = self.k.scene.as_ref().map_or("", |s| s.0.as_str()).to_owned();
            let then = kept_scene(&name).filter(|(v, s, _)| Some(*v) == last.map(|l| l.v) && *s == scene);
            let v = keep_scene(&name, &scene, &now.recs);
            let since = match then {
                Some((_, _, then)) => Since::Changes { version: v.to_string(), list: scene_changes(&then, &now.recs, false), more: 0 },
                None => Since::Full(Read { head: format!("tree {v} (scene {scene})"), version: Some(v.to_string()), ..now }),
            };
            print_since(since, &name, &self.said(last.map(|l| l.said()), "no read of it kept by this agent"));
        } else if self.k.tree {
            let reply = self.h.say(&format!("A {} tree since {}", self.w.target(), last.map_or(0, |l| l.v)))?;
            keep_version(&name, &reply);
            print_since(since_reply(&reply, &name)?, &name, &self.said(last.map(|l| l.said()), "no read of it kept by this agent, or the window shows another page or dialog"));
        }
        Ok(())
    }
}

/// A terminal's `lines N M` answer: its mark, and as `tree --since M` gives
/// it, the last `max` lines after how many came before them.
fn tail(reply: &str, max: usize, name: &str, from: u64) -> (u64, String) {
    let mut lines = reply.lines();
    let head = lines.next().unwrap_or_default();
    let mark = head.strip_prefix("mark ").and_then(|m| m.parse().ok()).unwrap_or(from);
    let body: Vec<&str> = lines.collect();
    let screen = body.first() == Some(&"screen");
    let body = &body[usize::from(screen)..];
    let skip = body.len().saturating_sub(max);
    let mut s = format!("{head}\n");
    if screen {
        s.push_str("screen\n");
    }
    if skip > 0 {
        s.push_str(&format!("... {skip} lines before these: `tree {name} --since {from}` gives them\n"));
    }
    for l in &body[skip..] {
        s.push_str(l);
        s.push('\n');
    }
    (mark, s)
}

/// What `e` or `S` answered about that element: `ok`, a `box`, or what it
/// does not do (`err no-action`...); what ends the act, an error.
pub(super) fn meant(reply: String, pid: &str, id: u32) -> R<String> {
    match reply.as_str() {
        "ok" | "err no-action" | "err no-focus" | "err no-scroll" | "err not-editable" => Ok(reply),
        r if r.starts_with("box ") => Ok(reply),
        r => Err(answered(r, Of::Element(pid, &format!("@{id}")))),
    }
}

/// The element a11y numbered that is meant: `@ID`, or the one whose whole
/// name is `--text T` (several: exit 6, or the first with `--first`).
fn numbered(k: &Kinds, w: &Window, a: &Args, h: &mut Hands) -> R<u32> {
    match (&a.element, a.text()) {
        (Some(Element::Id(n)), _) => Ok(*n),
        (Some(e @ Element::Name(_)), _) => Err(Fail::new(fail::NOT_HERE, format!("{} numbers its elements: {} is a scene's name; `tree {}` gives them as @ID", k.name, e.said(), k.name))),
        (None, Some(t)) => {
            let read = nodes(&h.say(&format!("A {} find {}", w.target(), hex(t)))?, &w.pid)?;
            let found: Vec<&Rec> = read.recs.iter().filter(|r| r.named(t)).collect();
            one(&found, t, a.first(), &k.name)?.number().ok_or_else(|| "that element has no number".into())
        }
        (None, None) => Err(usage("which element: @ID, or --text T (its whole name)")),
    }
}

/// `[right|middle] [COUNT]`, in any order.
fn button_and_count(words: &[String]) -> R<(u32, u32)> {
    let (mut b, mut count) = (272, 1);
    for w in words {
        match w.parse::<u32>() {
            Ok(n) if (1..=3).contains(&n) => count = n,
            Ok(_) => return Err(usage("COUNT: 1 to 3 presses")),
            Err(_) => b = button(Some(w))?,
        }
    }
    Ok((b, count))
}

/// A button and a count as they are said after what was pressed: ` right 2`.
pub(super) fn how(b: u32, count: u32) -> String {
    let b = match b {
        273 => " right",
        274 => " middle",
        _ => "",
    };
    format!("{b}{}", if count > 1 { format!(" {count}") } else { String::new() })
}

/// `press PID @X|--text T [--first] [right|middle] [COUNT] [--action N]`;
/// old: `press PID NAME` on a scene.
pub(super) fn press(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let mut a = parse(&words, &PRESS)?;
    let w = a.win()?.clone();
    let k = Kinds::of(&w)?;
    k.open()?;
    if let Some((_, sock)) = &k.scene {
        if a.element.is_none() && a.text().is_none() && !a.words.is_empty() {
            a.element = Some(Element::Name(a.words.remove(0)));
        }
        if a.value("--action").is_some() {
            return Err(Fail::new(fail::NOT_HERE, format!("{} is a pleamar scene: its items have no numbered actions (press @name [right|middle] [COUNT])", k.name)));
        }
        let name = item(&k, &a)?;
        let (b, count) = button_and_count(&a.words)?;
        let act = Acting::take(&k, &w, &a, until, false, Hands::open()?)?;
        // The scene presses it by name, and before that takes the agent's
        // cursor there itself: it is never behind the press.
        let done = told(sock, &format!("press {name}{} {count}", how(b, 1)));
        return act.answer(&format!("pressed {}{}", Element::Name(name).said(), how(b, count)), done);
    }
    if !k.tree || (k.is_text() && w.shell.is_some()) {
        return Err(k.refuse("press"));
    }
    let mut h = Hands::open()?;
    let id = numbered(&k, &w, &a, &mut h)?;
    let (b, count) = button_and_count(&a.words)?;
    let action = a.number::<u32>("--action")?;
    // Pressed all the same: its program may still answer it.
    let off = h.on(&w.pid, id, "box")?.split_whitespace().any(|s| s == "disabled");
    let mut act = Acting::take(&k, &w, &a, until, false, h)?;
    let done = act.h.press_element(&w.pid, id, b, count, action);
    let said = match action {
        Some(n) => format!("pressed @{id} (its action {n})"),
        None => format!("pressed @{id}{}", how(b, count)),
    };
    let said = if off { format!("{said}, but it is disabled: it may do nothing") } else { said };
    act.answer(&said, done.map(|_| String::new()))
}

/// `focus PID @X|--text T`: the focus in its window to it; old, `focus
/// PID`: the window gets the user's keyboard (and its workspace is shown).
pub(super) fn focus(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let a = parse(&words, &FOCUS)?;
    let w = a.win()?.clone();
    if !a.words.is_empty() {
        return Err(usage("focus PID @X, or focus PID --text T"));
    }
    if a.element.is_none() && a.text().is_none() {
        if until.is_some() {
            return Err(usage("--until is for focus PID @X: focus PID gives the window the user's keyboard"));
        }
        return Hands::open()?.act(&format!("f {}", w.pid));
    }
    let k = Kinds::of(&w)?;
    k.open()?;
    if let Some((_, sock)) = &k.scene {
        let name = item(&k, &a)?;
        let act = Acting::take(&k, &w, &a, until, false, Hands::open()?)?;
        let done = told(sock, &format!("focus {name}"));
        return act.answer(&format!("focused {}", Element::Name(name).said()), done);
    }
    if !k.tree || (k.is_text() && w.shell.is_some()) {
        return Err(k.refuse("focus"));
    }
    let mut h = Hands::open()?;
    let id = numbered(&k, &w, &a, &mut h)?;
    let mut act = Acting::take(&k, &w, &a, until, false, h)?;
    let done = act.h.focus_element(&w.pid, id);
    act.answer(&format!("focused @{id}"), done.map(|_| String::new()))
}

/// `type PID [@X] [--set] [--] TEXT|-`: into @X, or what has the
/// keyboard; a terminal's shell through its layer.
pub(super) fn type_(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let a = parse(&words, &TYPE)?;
    let w = a.win()?.clone();
    if a.words.is_empty() {
        return Err(usage("type what: type PID [@X] [--set] [--] TEXT, or - for the input"));
    }
    let text = typed(&a.words)?;
    let k = Kinds::of(&w)?;
    k.open()?;
    if k.is_text() && (w.shell.is_some() || a.element.is_none()) {
        if a.element.is_some() || a.has("--set") {
            return Err(k.refuse("type @X"));
        }
        let t = Term::of(&k, &w, "type", true)?;
        let act = Acting::take(&k, &w, &a, until, true, Hands::open()?)?;
        return act.answer(&format!("typed into {}", k.name), t.type_text(&text).map(|_| String::new()));
    }
    if let Some(e) = &a.element {
        if let Some((_, sock)) = &k.scene {
            let name = item(&k, &a)?;
            let act = Acting::take(&k, &w, &a, until, false, Hands::open()?)?;
            let done = told(sock, &format!("type {name} {text}"));
            return act.answer(&format!("typed into {}", Element::Name(name).said()), done);
        }
        if !k.tree {
            return Err(k.refuse(&format!("type {}", e.said())));
        }
        let mut h = Hands::open()?;
        let id = numbered(&k, &w, &a, &mut h)?;
        let mut act = Acting::take(&k, &w, &a, until, false, h)?;
        // Its focus first; with --set, its whole text at once.
        if a.has("--set") {
            let done = act.h.set_text(&w.pid, id, &text);
            return act.answer(&format!("set the text of @{id}"), done.map(|_| String::new()));
        }
        let done = act.h.focus_element(&w.pid, id).and_then(|_| act.h.type_into(&w, &text));
        return act.answer(&format!("typed into @{id}"), done.map(|_| String::new()));
    }
    if a.has("--set") {
        return Err(usage("--set sets an element's whole text: type PID @X --set TEXT"));
    }
    let mut act = Acting::take(&k, &w, &a, until, false, Hands::open()?)?;
    let done = act.h.type_into(&w, &text);
    act.answer(&format!("typed into {}", k.name), done.map(|_| String::new()))
}

/// `key PID NAME`: a terminal's shell through its layer when it knows the
/// key; else as the window manager types it.
pub(super) fn key(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let a = parse(&words, &KEY)?;
    let w = a.win()?.clone();
    let name = a.word(0).ok_or_else(|| usage("which key: key PID enter"))?;
    let k = Kinds::of(&w)?;
    let mut act = Acting::take(&k, &w, &a, until, true, Hands::open()?)?;
    if k.is_text() && (w.shell.is_some() || crate::term::knows_key(name)) {
        match Term::of(&k, &w, "key", true).and_then(|t| t.key(name)) {
            // An older layer that does not know it.
            Err(e) if w.shell.is_none() && e.msg.starts_with("no key called") => {}
            Err(e) if e.msg.starts_with("no key called") => return Err(usage(e.msg)),
            done => return act.answer(&format!("pressed {name}"), done.map(|_| String::new())),
        }
    }
    let done = act.h.act(&format!("k {} {name}", w.target()));
    act.answer(&format!("pressed {name}"), done.map(|_| String::new()))
}

/// `hotkey PID MODS+KEY`: a chord; into a terminal's shell those it types.
pub(super) fn hotkey(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let a = parse(&words, &KEY)?;
    let w = a.win()?.clone();
    let combo = a.word(0).ok_or_else(|| usage("which keys: hotkey PID ctrl+l"))?;
    let mut parts: Vec<&str> = combo.split('+').collect();
    let key = parts.pop().filter(|k| !k.is_empty()).ok_or_else(|| usage("which key, after the modifiers: ctrl+l"))?;
    let k = Kinds::of(&w)?;
    let mut act = Acting::take(&k, &w, &a, until, true, Hands::open()?)?;
    let typed = combo == "shift+tab" || combo.strip_prefix("ctrl+").is_some_and(|c| c.len() == 1 && c.as_bytes()[0].is_ascii_alphabetic());
    let done = if k.is_text() && (w.shell.is_some() || typed) {
        Term::of(&k, &w, "hotkey", true).and_then(|t| t.key(combo))
    } else if parts.is_empty() {
        act.h.act(&format!("k {} {key}", w.target()))
    } else {
        act.h.act(&format!("h {} {} {key}", w.target(), parts.join(",")))
    };
    act.answer(&format!("pressed {combo}"), done.map(|_| String::new()))
}

impl Hands {
    /// `e` on that element of a window.
    pub(super) fn on(&mut self, pid: &str, id: u32, what: &str) -> R<String> {
        meant(self.say(&format!("e root:{pid} {id} {what}"))?, pid, id)
    }

    /// Text typed into the window, in pieces (one line of the protocol has
    /// its limits), whole characters each: any text, accents and all.
    pub(super) fn type_into(&mut self, w: &Window, text: &str) -> R {
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(500) {
            self.act(&format!("t {} {}", w.target(), hex(&chunk.iter().collect::<String>())))?;
        }
        Ok(())
    }

    /// Where that element is clicked: its center, if it is on screen;
    /// `scroll`: scrolled into view first when it is not. None without a box.
    pub(super) fn spot(&mut self, pid: &str, id: u32, scroll: bool) -> R<Option<(f64, f64)>> {
        // The window in its own units, as the box is: the scene may show it smaller.
        let size = self.shown(pid)?.map(|(r, z)| (r[2] / z, r[3] / z));
        for again in [false, true] {
            let b = self.on(pid, id, "box")?;
            let f: Vec<&str> = b.split_whitespace().collect();
            let [_, x, y, w, h, shown, ..] = f[..] else { return Err(format!("@{id}: {b}").into()) };
            let [Ok(x), Ok(y), Ok(w), Ok(h)] = [x, y, w, h].map(str::parse::<f64>) else { return Ok(None) };
            let (cx, cy) = (x + w / 2.0, y + h / 2.0);
            if shown == "shown" && size.is_some_and(|s| inside(cx, cy, s)) {
                return Ok(Some((cx, cy)));
            }
            if !scroll || again {
                break;
            }
            let _ = self.on(pid, id, "scroll")?;
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if scroll {
            return Err(format!("not on screen: @{id} is not, and it did not scroll into view: `press {pid} @{id} --action 0` acts on it without the pointer").into());
        }
        Ok(None)
    }

    /// The pointer on that element, clicked as `click` does.
    pub(super) fn click_element(&mut self, pid: &str, id: u32, button: u32, count: u32) -> R {
        let (x, y) = self.spot(pid, id, true)?.ok_or_else(|| Fail::from(format!("no box: @{id} has none to click: `press {pid} @{id} --action 0` acts on it without the pointer")))?;
        self.arrive(pid, x, y)?;
        for _ in 0..count {
            self.act(&format!("b root:{pid} 0 {button} 1"))?;
            self.act(&format!("b root:{pid} 0 {button} 0"))?;
        }
        Ok(())
    }

    /// An element pressed: through the pointer; at `instant` by its default
    /// action, where it has one; with `--action N` by that action, the
    /// cursor shown on it.
    pub(super) fn press_element(&mut self, pid: &str, id: u32, b: u32, count: u32, action: Option<u32>) -> R {
        if let Some(n) = action {
            if let Some((x, y)) = self.spot(pid, id, false)? {
                self.arrive(pid, x, y)?;
            }
            return match self.on(pid, id, &format!("action {n}"))?.as_str() {
                "ok" => Ok(()),
                _ => Err(Fail::new(fail::NOT_HERE, format!("no action: @{id} has no action {n}"))),
            };
        }
        // At `instant`, a plain click is its default action, the cursor
        // jumping to it where it is seen.
        if self.pace == Pace::Instant && b == 272 && count == 1 {
            if let Some((x, y)) = self.spot(pid, id, false)? {
                self.arrive(pid, x, y)?;
            }
            if self.on(pid, id, "press")? == "ok" {
                return Ok(());
            }
        }
        self.click_element(pid, id, b, count)
    }

    /// The keyboard's focus in its window to it, the cursor going there
    /// where it is seen; by the pointer if it cannot be given.
    pub(super) fn focus_element(&mut self, pid: &str, id: u32) -> R {
        if let Some((x, y)) = self.spot(pid, id, false)? {
            self.arrive(pid, x, y)?;
        }
        if self.on(pid, id, "focus")? == "ok" {
            return Ok(());
        }
        self.click_element(pid, id, 272, 1)
    }

    /// `type PID @ID --set TEXT`: its whole text, at once.
    pub(super) fn set_text(&mut self, pid: &str, id: u32, text: &str) -> R {
        if text.len() > 30_000 {
            return Err(usage("--set takes up to 30000 bytes: type the rest after it"));
        }
        if let Some((x, y)) = self.spot(pid, id, false)? {
            self.arrive(pid, x, y)?;
        }
        match meant(self.say(&format!("S root:{pid} {id} {}", if text.is_empty() { "-".to_owned() } else { hex(text) }))?, pid, id)?.as_str() {
            "ok" => Ok(()),
            _ => Err(answered("not-editable", Of::Element(pid, &format!("@{id}")))),
        }
    }
}

/// Whether a point is inside a window of that size, in its units: a page
/// scrolled away keeps its elements showing, their boxes off its edges.
fn inside(x: f64, y: f64, (w, h): (f64, f64)) -> bool {
    x >= 0.0 && y >= 0.0 && x < w && y < h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn until_is_split_off_before_the_text() {
        assert_eq!(split_until(&s(&["4521", "@24", "--until", "has", "--text", "Saved", "5s"])), (s(&["4521", "@24"]), Some(s(&["has", "--text", "Saved", "5s"]))));
        assert_eq!(split_until(&s(&["4521", "@12", "--until", "changed", "--", "ana"])), (s(&["4521", "@12", "--", "ana"]), Some(s(&["changed"]))));
        // After `--` it is text.
        assert_eq!(split_until(&s(&["4521", "--", "a", "--until", "x"])), (s(&["4521", "--", "a", "--until", "x"]), None));
        assert_eq!(split_until(&s(&["4521", "--until"])), (s(&["4521"]), Some(Vec::new())));
    }

    #[test]
    fn a_box_is_inside_the_window_in_its_own_units() {
        // The scene shows it at 0.4: 800 wide on screen, 2000 its own.
        let size = ([0.0, 0.0, 800.0, 600.0], 0.4);
        let own = (size.0[2] / size.1, size.0[3] / size.1);
        assert!(inside(1200.0, 900.0, own));
        assert!(!inside(1200.0, 900.0, (size.0[2], size.0[3])));
        assert!(!inside(-5.0, 10.0, own) && !inside(10.0, 1500.0, own));
    }

    #[test]
    fn a_terminal_answers_its_last_lines() {
        let (mark, text) = tail("mark 14\na\nb\nc", 2, "7002", 11);
        assert_eq!(mark, 14);
        assert_eq!(text, "mark 14\n... 1 lines before these: `tree 7002 --since 11` gives them\nb\nc\n");
        assert_eq!(tail("mark 3\nscreen\nx", 60, "7002", 1).1, "mark 3\nscreen\nx\n");
        assert_eq!(how(273, 2), " right 2");
        assert_eq!(how(272, 1), "");
        assert_eq!(Settled::Drawing(0.31).said().unwrap(), "still drawing after 0.3 s: the changes so far");
        assert_eq!(Settled::Quiet.said(), None);
    }
}
