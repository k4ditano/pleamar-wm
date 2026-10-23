//! Acting on a window: `press`, `focus`, `type`, `key`, `hotkey`; a scene's
//! item by its name.
//!
//! An action answers what it did, then what it changed: the window's
//! version is taken before it, and read again after it; `--until` waits for
//! a condition instead.

use super::*;

const PRESS: Spec = Spec { element: true, flags: &["--first", "--no-wait"], values: &["--text", "--max"], ..PLAIN };
const FOCUS: Spec = Spec { element: true, flags: &["--first", "--no-wait"], values: &["--text", "--max"], ..PLAIN };
const TYPE: Spec = Spec { element: true, flags: &["--no-wait"], values: &["--max"], ..PLAIN };
const KEY: Spec = Spec { flags: &["--no-wait"], values: &["--max"], ..PLAIN };
/// The pointer's: `click`, `drag`, `scroll`.
pub(super) const POINTER: Spec = Spec { flags: &["--no-wait"], values: &["--max"], ..PLAIN };

/// The most lines of changes an action answers, unless `--max N`.
const MAX: usize = 60;

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
    /// A scene's items.
    Scene(Vec<Rec>),
    /// Pictures only.
    Look,
    /// Not taken: `--no-wait`.
    Nothing,
}

/// An action under way: the window, its version before it, at most how
/// many changes are answered, and what is waited for after it.
pub(super) struct Acting {
    k: Kinds,
    pub(super) h: Hands,
    before: Before,
    max: usize,
    until: Option<Vec<String>>,
}

impl Acting {
    /// Before acting on that window: its version, unless `--no-wait`
    /// without `--until`. A wrong `--until` is refused now, before
    /// anything is done.
    pub(super) fn take(k: &Kinds, a: &Args, until: Option<Vec<String>>, h: Hands) -> R<Acting> {
        if let Some(c) = &until {
            wait::check(c, k)?;
        }
        let max = a.number::<usize>("--max")?.filter(|m| *m > 0).unwrap_or(MAX);
        let before = if a.has("--no-wait") && until.is_none() {
            Before::Nothing
        } else if k.is_scene() {
            Before::Scene(described(k)?.recs)
        } else {
            Before::Look
        };
        Ok(Acting { k: k.clone(), h, before, max, until })
    }

    /// The answer: what was done (`said`, then what the scene said of it),
    /// then the condition `--until` waited for, and what changed since the
    /// version taken before. An item gone: what changed since this agent's
    /// last read, and exit 4.
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
            let held = wait::until(&[&[self.k.name.clone()][..], &cond].concat());
            let changes = self.changes();
            held.as_ref().map_or(Ok(()), |_| changes)?;
            return held.map_err(|f| match f.code {
                fail::TIMEOUT => Fail::new(f.code, f.msg.replacen("timeout: ", &format!("timeout: {said}, then "), 1)),
                _ => f,
            });
        }
        if matches!(self.before, Before::Nothing) {
            return Ok(());
        }
        self.changes()
    }

    /// How a change is said: as `tree --since` says it, at most `max`.
    fn said<'a>(&self, last: Option<String>, why_full: &'a str) -> Said<'a> {
        Said { last, why_full, role: None, max: Some(self.max), json: false, long: false }
    }

    /// What changed since the version taken before, as `tree --since V`
    /// answers it, kept as this agent's last read; pictures only: `done`.
    fn changes(&mut self) -> R {
        let name = self.k.name.clone();
        match &self.before {
            Before::Scene(then) => {
                let now = described(&self.k)?.recs;
                let v = keep_scene(&name, self.k.scene.as_ref().map_or("", |s| s.0.as_str()), &now);
                let list = scene_changes(then, &now, false).into_iter().filter(|c| !matches!(c, Change::Came(r) if r.states.iter().any(|s| s == "offscreen"))).collect();
                print_since(Since::Changes { version: v.to_string(), list, more: 0 }, &name, &self.said(None, ""));
            }
            Before::Look => println!("done"),
            Before::Nothing => {}
        }
        Ok(())
    }

    /// An item gone: what changed in the window since this agent's last
    /// read of it, read now.
    fn gone(&mut self) -> R {
        if !self.k.is_scene() {
            return Ok(());
        }
        let name = self.k.name.clone();
        let last = last_version(&name);
        let now = described(&self.k)?;
        let scene = self.k.scene.as_ref().map_or("", |s| s.0.as_str()).to_owned();
        let then = kept_scene(&name).filter(|(v, s, _)| Some(*v) == last.map(|l| l.v) && *s == scene);
        let v = keep_scene(&name, &scene, &now.recs);
        let since = match then {
            Some((_, _, then)) => Since::Changes { version: v.to_string(), list: scene_changes(&then, &now.recs, false), more: 0 },
            None => Since::Full(Read { head: format!("tree {v} (scene {scene})"), version: Some(v.to_string()), ..now }),
        };
        print_since(since, &name, &self.said(last.map(|l| l.said()), "no read of it kept by this agent"));
        Ok(())
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

/// `press PID @X|--text T [--first] [right|middle] [COUNT]`; old: `press
/// PID NAME` on a scene.
pub(super) fn press(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let mut a = parse(&words, &PRESS)?;
    let w = a.win()?.clone();
    let k = Kinds::of(&w)?;
    k.open()?;
    let Some((_, sock)) = &k.scene else { return Err(k.refuse("press")) };
    if a.element.is_none() && a.text().is_none() && !a.words.is_empty() {
        a.element = Some(Element::Name(a.words.remove(0)));
    }
    let name = item(&k, &a)?;
    let (b, count) = button_and_count(&a.words)?;
    let act = Acting::take(&k, &a, until, Hands::open()?)?;
    // The scene presses it by name, and before that takes the agent's
    // cursor there itself: it is never behind the press.
    let done = told(sock, &format!("press {name}{} {count}", how(b, 1)));
    act.answer(&format!("pressed {}{}", Element::Name(name).said(), how(b, count)), done)
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
    let Some((_, sock)) = &k.scene else { return Err(k.refuse("focus")) };
    let name = item(&k, &a)?;
    let act = Acting::take(&k, &a, until, Hands::open()?)?;
    let done = told(sock, &format!("focus {name}"));
    act.answer(&format!("focused {}", Element::Name(name).said()), done)
}

/// `type PID [@X] [--] TEXT|-`: into a scene's @X, or what has the
/// keyboard.
pub(super) fn type_(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let a = parse(&words, &TYPE)?;
    let w = a.win()?.clone();
    if a.words.is_empty() {
        return Err(usage("type what: type PID [@X] [--] TEXT, or - for the input"));
    }
    let text = typed(&a.words)?;
    let k = Kinds::of(&w)?;
    k.open()?;
    if let Some(e) = &a.element {
        let Some((_, sock)) = &k.scene else { return Err(k.refuse(&format!("type {}", e.said()))) };
        let name = item(&k, &a)?;
        let act = Acting::take(&k, &a, until, Hands::open()?)?;
        let done = told(sock, &format!("type {name} {text}"));
        return act.answer(&format!("typed into {}", Element::Name(name).said()), done);
    }
    let mut act = Acting::take(&k, &a, until, Hands::open()?)?;
    let done = act.h.type_into(&w, &text);
    act.answer(&format!("typed into {}", k.name), done.map(|_| String::new()))
}

/// `key PID NAME`: as the window manager types it.
pub(super) fn key(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let a = parse(&words, &KEY)?;
    let w = a.win()?.clone();
    let name = a.word(0).ok_or_else(|| usage("which key: key PID enter"))?;
    let k = Kinds::of(&w)?;
    let mut act = Acting::take(&k, &a, until, Hands::open()?)?;
    let done = act.h.act(&format!("k {} {name}", w.target()));
    act.answer(&format!("pressed {name}"), done.map(|_| String::new()))
}

/// `hotkey PID MODS+KEY`: a chord.
pub(super) fn hotkey(words: &[String]) -> R {
    let (words, until) = split_until(words);
    let a = parse(&words, &KEY)?;
    let w = a.win()?.clone();
    let combo = a.word(0).ok_or_else(|| usage("which keys: hotkey PID ctrl+l"))?;
    let mut parts: Vec<&str> = combo.split('+').collect();
    let key = parts.pop().filter(|k| !k.is_empty()).ok_or_else(|| usage("which key, after the modifiers: ctrl+l"))?;
    let k = Kinds::of(&w)?;
    let mut act = Acting::take(&k, &a, until, Hands::open()?)?;
    let done = if parts.is_empty() {
        act.h.act(&format!("k {} {key}", w.target()))
    } else {
        act.h.act(&format!("h {} {} {key}", w.target(), parts.join(",")))
    };
    act.answer(&format!("pressed {combo}"), done.map(|_| String::new()))
}

/// The text `type` takes: its words, or (`-`) the input, so that a
/// password is not seen in the list of processes.
fn typed(words: &[String]) -> R<String> {
    if words.len() == 1 && words[0] == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
        return Ok(s.strip_suffix('\n').map(str::to_owned).unwrap_or(s));
    }
    Ok(words.join(" "))
}

impl Hands {
    /// Text typed into the window, in pieces (one line of the protocol has
    /// its limits), whole characters each: any text, accents and all.
    pub(super) fn type_into(&mut self, w: &Window, text: &str) -> R {
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(500) {
            self.act(&format!("t {} {}", w.target(), hex(&chunk.iter().collect::<String>())))?;
        }
        Ok(())
    }
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
    fn a_button_and_a_count() {
        assert_eq!(how(273, 2), " right 2");
        assert_eq!(how(272, 1), "");
    }
}
