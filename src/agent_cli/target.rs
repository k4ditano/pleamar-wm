//! A command line read as one grammar: a window (`PID`, `PID.N`,
//! `PID:SHELL`), an element (`@24`, `@save`, `@=3`), `--text T`, `-o FILE`,
//! `--first`, durations, `--` before text, and the words left.

use super::fail::{usage, R};

/// A window as `windows` names it, and which of its terminal's shells.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Window {
    /// `4521`, or `4521.3` for one of a program's several windows.
    pub(super) pid: String,
    pub(super) shell: Option<i32>,
}

impl Window {
    pub(super) fn parse(s: &str) -> Option<Window> {
        let (pid, shell) = match s.split_once(':') {
            Some((p, sh)) => (p, Some(sh.parse::<i32>().ok().filter(|n| *n > 0)?)),
            None => (s, None),
        };
        let fine = match pid.split_once('.') {
            Some((a, b)) => a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok(),
            None => pid.parse::<u32>().is_ok(),
        };
        fine.then(|| Window { pid: pid.to_owned(), shell })
    }

    /// Its process.
    pub(super) fn process(&self) -> u32 {
        self.pid.split('.').next().and_then(|p| p.parse().ok()).unwrap_or(0)
    }

    /// As the socket names it.
    pub(super) fn target(&self) -> String {
        format!("root:{}", self.pid)
    }
}

/// An element: a number (`@24`), or a scene item's name.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Element {
    Id(u32),
    Name(String),
}

impl Element {
    /// `@24`, `@save`; `@=3` a name of digits.
    pub(super) fn parse(s: &str) -> Option<Element> {
        let rest = s.strip_prefix('@')?;
        if let Some(name) = rest.strip_prefix('=') {
            return (!name.is_empty()).then(|| Element::Name(name.to_owned()));
        }
        match rest.parse::<u32>() {
            Ok(n) => Some(Element::Id(n)),
            Err(_) if !rest.is_empty() => Some(Element::Name(rest.to_owned())),
            Err(_) => None,
        }
    }

    /// As it is said.
    pub(super) fn said(&self) -> String {
        match self {
            Element::Id(n) => format!("@{n}"),
            Element::Name(n) if n.chars().all(|c| c.is_ascii_digit()) => format!("@={n}"),
            Element::Name(n) => format!("@{n}"),
        }
    }
}

/// Whether a verb names a window.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Needs {
    No,
    Maybe,
    Yes,
}

/// What a verb takes besides its words: a window, an element right after
/// it, its flags and the options that take a value. `--text`, `-o` and
/// `--first` are taken where listed.
pub(super) struct Spec {
    pub(super) window: Needs,
    pub(super) element: bool,
    pub(super) flags: &'static [&'static str],
    pub(super) values: &'static [&'static str],
    /// Options whose value may be left out: taken when the next word is a number.
    pub(super) maybe: &'static [&'static str],
    /// Words after the window as they are (`say`, `raw`, `open`): no options read.
    pub(super) raw: bool,
    /// Whether a terminal's shell may be named (`PID:SHELL`).
    pub(super) shell: bool,
}

pub(super) const PLAIN: Spec = Spec { window: Needs::Yes, element: false, flags: &[], values: &[], maybe: &[], raw: false, shell: false };
/// The same, of a verb a terminal's shell takes.
pub(super) const SHELL: Spec = Spec { shell: true, ..PLAIN };

#[derive(Debug, Default, PartialEq)]
pub(super) struct Args {
    pub(super) window: Option<Window>,
    pub(super) element: Option<Element>,
    pub(super) flags: Vec<String>,
    pub(super) values: Vec<(String, String)>,
    pub(super) words: Vec<String>,
    /// From which word on `--` made them text.
    pub(super) literal: Option<usize>,
}

impl Args {
    pub(super) fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }

    pub(super) fn value(&self, option: &str) -> Option<&str> {
        self.values.iter().find(|(o, _)| o == option).map(|(_, v)| v.as_str())
    }

    pub(super) fn text(&self) -> Option<&str> {
        self.value("--text")
    }

    pub(super) fn out(&self) -> Option<&str> {
        self.value("-o")
    }

    pub(super) fn first(&self) -> bool {
        self.has("--first")
    }

    /// The window; one is required here.
    pub(super) fn win(&self) -> R<&Window> {
        self.window.as_ref().ok_or_else(|| usage("which window: PID or PID.N, as `pleamar-wm agent windows` names it"))
    }

    pub(super) fn word(&self, k: usize) -> Option<&str> {
        self.words.get(k).map(String::as_str)
    }

    /// A word that is a number of that option, or none.
    pub(super) fn number<T: std::str::FromStr>(&self, option: &str) -> R<Option<T>> {
        self.value(option).map(|v| v.parse::<T>().map_err(|_| usage(format!("{option} N: a number, not {v:?}")))).transpose()
    }
}

/// Whether a word reads as an option (`--text`, `-o`), not as text or a number.
fn optionish(w: &str) -> bool {
    w.len() > 1 && w.starts_with('-') && w.parse::<f64>().is_err()
}

/// A verb's words, read by its spec.
pub(super) fn parse(words: &[String], spec: &Spec) -> R<Args> {
    let mut a = Args::default();
    let mut k = 0;
    if spec.window != Needs::No {
        match words.first().map(|w| (w, Window::parse(w))) {
            Some((_, Some(w))) => {
                a.window = Some(w);
                k = 1;
            }
            Some((w, None)) if spec.window == Needs::Yes => {
                return Err(usage(match w.split_once(':') {
                    Some(_) => format!("{w}: a window and its shell are PID:SHELL (7002:7044), numbers both"),
                    None => format!("{w}: which window: PID or PID.N, as `pleamar-wm agent windows` names it"),
                }));
            }
            None if spec.window == Needs::Yes => return Err(usage("which window: PID or PID.N, as `pleamar-wm agent windows` names it")),
            _ => {}
        }
    }
    if let Some(Window { pid, shell: Some(s) }) = &a.window {
        if !spec.shell {
            return Err(usage(format!("{pid}:{s} names a terminal's shell: for tree, find, run, type, key, hotkey, wait and can; here say the window, {pid}")));
        }
    }
    if spec.raw {
        a.words = words[k..].to_vec();
        return Ok(a);
    }
    let takes = |o: &str| spec.values.contains(&o);
    while k < words.len() {
        let w = &words[k];
        k += 1;
        if a.literal.is_some() {
            a.words.push(w.clone());
            continue;
        }
        if w == "--" {
            a.literal = Some(a.words.len());
            continue;
        }
        if optionish(w) {
            if takes(w) {
                let v = words.get(k).ok_or_else(|| usage(format!("{w} needs a value after it")))?;
                a.values.push((w.clone(), v.clone()));
                k += 1;
            } else if spec.maybe.contains(&w.as_str()) {
                a.flags.push(w.clone());
                if let Some(v) = words.get(k).filter(|v| v.parse::<u64>().is_ok()) {
                    a.values.push((w.clone(), v.clone()));
                    k += 1;
                }
            } else if spec.flags.contains(&w.as_str()) {
                a.flags.push(w.clone());
            } else {
                return Err(usage(format!("{w}: not an option here (text that starts with - goes after --): `pleamar-wm agent help`")));
            }
            continue;
        }
        // An element, right after the window.
        if spec.element && a.element.is_none() && a.words.is_empty() && w.starts_with('@') {
            a.element = Some(Element::parse(w).ok_or_else(|| usage(format!("{w}: an element is @ and its number (@24) or its name (@save, @=3)")))?);
            continue;
        }
        a.words.push(w.clone());
    }
    Ok(a)
}

/// A duration: `500ms`, `5s`, `2m`, or seconds.
pub(super) fn duration(s: &str) -> R<f64> {
    let n = |v: &str| v.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
    let secs = if let Some(v) = s.strip_suffix("ms") {
        n(v).map(|v| v / 1000.0)
    } else if let Some(v) = s.strip_suffix('s') {
        n(v)
    } else if let Some(v) = s.strip_suffix('m') {
        n(v).map(|v| v * 60.0)
    } else {
        n(s)
    };
    secs.ok_or_else(|| usage(format!("{s}: a duration is 500ms, 5s, 2m or a number of seconds")))
}

pub(super) fn is_duration(s: &str) -> bool {
    duration(s).is_ok()
}

pub(super) fn button(name: Option<&str>) -> R<u32> {
    match name.unwrap_or("left") {
        "left" => Ok(272),
        "right" => Ok(273),
        "middle" => Ok(274),
        other => Err(usage(format!("a button is left, right or middle, not {other:?}"))),
    }
}

pub(super) fn number(s: Option<&str>, what: &str) -> R<f64> {
    s.and_then(|v| v.parse::<f64>().ok()).ok_or_else(|| usage(format!("{what}: a number")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| w.to_string()).collect()
    }

    const TYPE: Spec = Spec { window: Needs::Yes, element: true, flags: &["--set"], values: &[], maybe: &[], raw: false, shell: true };
    const LOOK: Spec = Spec { window: Needs::Maybe, element: true, flags: &["--marks", "--hidpi", "--first"], values: &["--text", "-o", "--pad"], maybe: &[], raw: false, shell: false };

    #[test]
    fn a_window_is_its_process_one_of_its_windows_or_a_shell() {
        assert_eq!(Window::parse("4521"), Some(Window { pid: "4521".into(), shell: None }));
        assert_eq!(Window::parse("4521.3"), Some(Window { pid: "4521.3".into(), shell: None }));
        assert_eq!(Window::parse("7002:7044"), Some(Window { pid: "7002".into(), shell: Some(7044) }));
        assert_eq!(Window::parse("7002.1:7044"), Some(Window { pid: "7002.1".into(), shell: Some(7044) }));
        assert_eq!(Window::parse("7002.1:7044").unwrap().process(), 7002);
        assert_eq!(Window::parse("7002.1").unwrap().target(), "root:7002.1");
        for bad in ["", "x", "4521.", ".3", "7002:", "7002:x", ":7044", "45 21", "-3"] {
            assert_eq!(Window::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_element_is_a_number_or_a_name() {
        assert_eq!(Element::parse("@24"), Some(Element::Id(24)));
        assert_eq!(Element::parse("@save"), Some(Element::Name("save".into())));
        assert_eq!(Element::parse("@hit#r3"), Some(Element::Name("hit#r3".into())));
        assert_eq!(Element::parse("@=3"), Some(Element::Name("3".into())));
        assert_eq!(Element::parse("@"), None);
        assert_eq!(Element::parse("@="), None);
        assert_eq!(Element::parse("24"), None);
        assert_eq!(Element::Name("3".into()).said(), "@=3");
        assert_eq!(Element::Name("save".into()).said(), "@save");
        assert_eq!(Element::Id(7).said(), "@7");
    }

    #[test]
    fn after_two_dashes_it_is_text() {
        let a = parse(&words(&["4521", "@12", "--", "@alice", "--set"]), &TYPE).unwrap();
        assert_eq!(a.window.as_ref().unwrap().pid, "4521");
        assert_eq!(a.element, Some(Element::Id(12)));
        assert_eq!(a.words, words(&["@alice", "--set"]));
        assert!(!a.has("--set"));
        assert_eq!(a.literal, Some(0));
        let a = parse(&words(&["4521", "@12", "--set", "--", "hi", "there"]), &TYPE).unwrap();
        assert!(a.has("--set"));
        assert_eq!(a.words, words(&["hi", "there"]));
        // Without them, a word after the first is text, an @ too.
        let a = parse(&words(&["4521", "hello", "@bob"]), &TYPE).unwrap();
        assert_eq!((a.element, a.words), (None, words(&["hello", "@bob"])));
        // `-` is the input, and a number is never an option.
        let a = parse(&words(&["4521", "-"]), &TYPE).unwrap();
        assert_eq!(a.words, words(&["-"]));
        let a = parse(&words(&["4521", "-12"]), &TYPE).unwrap();
        assert_eq!(a.words, words(&["-12"]));
        assert_eq!(parse(&words(&["4521", "--sett"]), &TYPE).unwrap_err().code, 64);
        assert_eq!(parse(&words(&["4521", "@"]), &TYPE).unwrap_err().code, 64);
    }

    #[test]
    fn options_and_their_values() {
        let a = parse(&words(&["4521", "--text", "Save as", "--first", "-o", "/tmp/x.png", "--pad", "8"]), &LOOK).unwrap();
        assert_eq!((a.text(), a.out(), a.first()), (Some("Save as"), Some("/tmp/x.png"), true));
        assert_eq!(a.number::<u32>("--pad").unwrap(), Some(8));
        assert!(parse(&words(&["4521", "--text"]), &LOOK).is_err());
        let a = parse(&words(&["4521", "--pad", "x"]), &LOOK).unwrap();
        assert_eq!(a.number::<u32>("--pad").unwrap_err().code, 64);
        // No window: the whole desktop.
        let a = parse(&words(&["-o", "/tmp/d.png"]), &LOOK).unwrap();
        assert_eq!((a.window.is_none(), a.out()), (true, Some("/tmp/d.png")));
        let a = parse(&words(&["4521", "@save", "-o", "f.png"]), &LOOK).unwrap();
        assert_eq!(a.element, Some(Element::Name("save".into())));
        let since = Spec { maybe: &["--since"], flags: &["--json"], ..PLAIN };
        let a = parse(&words(&["4521", "--since", "12", "--json"]), &since).unwrap();
        assert_eq!((a.has("--since"), a.value("--since"), a.has("--json")), (true, Some("12"), true));
        let a = parse(&words(&["4521", "--since", "--json"]), &since).unwrap();
        assert_eq!((a.has("--since"), a.value("--since")), (true, None));
    }

    #[test]
    fn a_window_is_asked_for_in_the_right_form() {
        assert_eq!(parse(&words(&[]), &PLAIN).unwrap_err().code, 64);
        assert!(parse(&words(&["firefox"]), &PLAIN).unwrap_err().msg.contains("PID or PID.N"));
        assert!(parse(&words(&["7002:x"]), &PLAIN).unwrap_err().msg.contains("PID:SHELL"));
        // A shell only where a terminal's shell is meant.
        assert_eq!(parse(&words(&["7002:7044", "1", "1"]), &PLAIN).unwrap_err().code, 64);
        assert_eq!(parse(&words(&["7002:7044", "ls"]), &SHELL).unwrap().window.unwrap().shell, Some(7044));
        let raw = Spec { raw: true, ..PLAIN };
        let a = parse(&words(&["8810", "drag", "knob", "0", "-40"]), &raw).unwrap();
        assert_eq!(a.words, words(&["drag", "knob", "0", "-40"]));
    }

    #[test]
    fn durations() {
        assert_eq!(duration("500ms").unwrap(), 0.5);
        assert_eq!(duration("5s").unwrap(), 5.0);
        assert_eq!(duration("2m").unwrap(), 120.0);
        assert_eq!(duration("7").unwrap(), 7.0);
        assert_eq!(duration("1.5").unwrap(), 1.5);
        for bad in ["soon", "", "5h", "-1", "ms"] {
            assert_eq!(duration(bad).unwrap_err().code, 64, "{bad}");
        }
    }
}
