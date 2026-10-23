//! `wait PID COND [DURATION]`: until something holds. One grammar for every
//! kind of window; each takes what it can and refuses the rest.
//!
//! A terminal's are its layer's; a scene's expression its own `wait`, and
//! the rest its `describe` read a frame apart.

use super::*;
use std::time::{Duration, Instant};

const WAIT: Spec = Spec { values: &["--text", "--from"], flags: &["--json"], ..SHELL };

/// The longest wait, in seconds; `prompt`'s.
const LONGEST: f64 = 60.0;
const PROMPT: f64 = 600.0;
/// Between two reads of a scene: a frame.
const FRAME: Duration = Duration::from_millis(16);
/// How often a Stop, the window made private or closing are asked.
const ASKED: Duration = Duration::from_secs(1);

const FORM: &str = "wait PID has --text T | gone --text T | @X gone | @X [not] STATE | changed | settled DURATION | prompt | expr 'EXPR' [DURATION]";
const CONDITIONS: [&str; 6] = ["has", "gone", "changed", "settled", "prompt", "expr"];
/// A scene item's states, as its records say them, and `enabled`.
const SCENE_STATES: [&str; 7] = ["disabled", "enabled", "checked", "selected", "offscreen", "covered", "persons"];

/// What is waited for.
#[derive(Debug, PartialEq)]
enum Cond {
    Has(String),
    Gone(String),
    ElementGone(Element),
    /// That element has (false: has not) that state.
    State(Element, String, bool),
    Changed,
    Settled(u64),
    Prompt,
    /// A scene's expression; true: said as on main, without `expr`, the
    /// scene reading it whole.
    Expr(String, bool),
}

impl Cond {
    /// As a refusal says it.
    fn word(&self) -> &'static str {
        match self {
            Cond::Has(_) => "has",
            Cond::Gone(_) => "gone",
            Cond::ElementGone(_) => "@X gone",
            Cond::State(..) => "@X STATE",
            Cond::Changed => "changed",
            Cond::Settled(_) => "settled",
            Cond::Prompt => "prompt",
            Cond::Expr(..) => "expr",
        }
    }

}

/// The condition and how long, in seconds: 30 unless given, up to 60
/// (`prompt` 10 minutes).
fn condition(words: &[String], text: Option<&str>) -> R<(Cond, f64)> {
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let at = |s: &str| Element::parse(s);
    let ms = |d: &str| Ok::<_, Fail>((Some(duration(d)?).filter(|s| *s <= LONGEST).ok_or_else(|| usage(format!("settled {d}: up to {LONGEST} s")))? * 1000.0).round() as u64);
    let (cond, rest) = match (&w[..], text) {
        (["has", ..], Some(t)) => (Cond::Has(t.to_owned()), &w[1..]),
        (["gone", ..], Some(t)) => (Cond::Gone(t.to_owned()), &w[1..]),
        (["has" | "gone", ..], None) => return Err(usage(format!("wait PID {} --text T: a text is said as --text", w[0]))),
        (_, Some(_)) => return Err(usage(format!("--text is for has and gone: {FORM}"))),
        (["changed", ..], None) => (Cond::Changed, &w[1..]),
        (["settled", d, ..], None) => (Cond::Settled(ms(d)?), &w[2..]),
        (["settled"], None) => return Err(usage("wait PID settled DURATION [DURATION]: quiet for how long (500ms), then how long to wait")),
        (["prompt", ..], None) => return Ok((Cond::Prompt, seconds(&w[1..], PROMPT)?)),
        (["expr", e @ ..], None) => {
            let (expr, rest) = match e.split_last() {
                Some((d, e)) if !e.is_empty() && is_duration(d) => (e.join(" "), &w[w.len() - 1..]),
                _ => (e.join(" "), &w[w.len()..]),
            };
            if expr.trim().is_empty() {
                return Err(usage("wait PID expr 'status == \"Saved\"' [DURATION]"));
            }
            (Cond::Expr(expr, false), rest)
        }
        ([e, "gone", ..], None) if at(e).is_some() => (Cond::ElementGone(at(e).unwrap_or(Element::Id(0))), &w[2..]),
        ([e, "not", s, ..], None) if at(e).is_some() => (Cond::State(at(e).unwrap_or(Element::Id(0)), s.to_string(), false), &w[3..]),
        ([e, s, ..], None) if at(e).is_some() => (Cond::State(at(e).unwrap_or(Element::Id(0)), s.to_string(), true), &w[2..]),
        // As on main: a scene's expression as it is, its time in it.
        ([first, ..], None) if !CONDITIONS.contains(first) && !first.starts_with('@') => return Ok((Cond::Expr(words.join(" "), true), LONGEST)),
        _ => return Err(usage(FORM)),
    };
    Ok((cond, seconds(rest, LONGEST)?))
}

/// The duration after a condition: 30 s unless given, up to `longest`.
fn seconds(rest: &[&str], longest: f64) -> R<f64> {
    match rest {
        [] => Ok(30.0_f64.min(longest)),
        [s] => Some(duration(s)?).filter(|s| *s > 0.0 && *s <= longest).ok_or_else(|| usage(format!("{s}: a duration up to {longest} s"))),
        _ => Err(usage(format!("{}: one duration after the condition", rest.join(" ")))),
    }
}

/// What a kind of window waits for, as its refusal says it.
pub(super) fn waits(k: &Kinds) -> &'static str {
    if k.is_scene() {
        "has --text T, gone --text T, @name gone, @name [not] STATE, changed, expr 'EXPR'"
    } else if k.is_text() {
        "has --text T [--from M], gone --text T, changed, settled DURATION, prompt"
    } else {
        "nothing"
    }
}

pub(super) fn wait(words: &[String]) -> R {
    until(words, None)
}

/// An action's `--until COND [DURATION]`, read before it acts: wrong (64),
/// or not for that window (3), as `wait` would refuse it.
pub(super) fn check(cond: &[String], k: &Kinds) -> R {
    let a = parse(&[&["0".to_owned()][..], cond].concat(), &WAIT)?;
    let (cond, _) = condition(&a.words, a.text())?;
    let refused = || Err(k.refuse(&format!("wait {}", cond.word())));
    let named = |e: &Element| matches!(e, Element::Name(_));
    if matches!(cond, Cond::Expr(_, true)) && !k.is_scene() {
        return Err(usage(FORM));
    }
    if k.is_text() {
        return match cond {
            Cond::Has(_) | Cond::Gone(_) | Cond::Changed | Cond::Settled(_) | Cond::Prompt => Ok(()),
            _ => refused(),
        };
    }
    match (&k.scene, &cond) {
        (Some(_), Cond::ElementGone(e) | Cond::State(e, ..)) if !named(e) => refused(),
        (Some(_), Cond::Expr(..) | Cond::Has(_) | Cond::Gone(_) | Cond::ElementGone(_) | Cond::State(..) | Cond::Changed) => Ok(()),
        _ => refused(),
    }
}

/// `wait` after an action: a terminal's from the mark taken before it.
pub(super) fn until(words: &[String], mark: Option<u64>) -> R {
    let a = parse(words, &WAIT)?;
    let w = a.win()?;
    let (cond, secs) = condition(&a.words, a.text())?;
    let from = a.number::<u64>("--from")?;
    if from.is_some() && !matches!(cond, Cond::Has(_)) {
        return Err(usage("--from M is for has --text T on a terminal: what came after its mark M"));
    }
    let k = Kinds::of(w)?;
    k.open()?;
    if matches!(cond, Cond::Expr(_, true)) && !k.is_scene() {
        return Err(usage(FORM));
    }
    let json = a.has("--json");
    if k.is_text() {
        return terminal(&k, w, cond, secs, from.or(mark), json);
    }
    if from.is_some() {
        return Err(k.refuse("wait --from"));
    }
    match (&k.scene, cond) {
        (Some((_, sock)), Cond::Expr(e, whole)) => expr(w, sock, &e, whole, secs, json),
        (Some(_), c @ (Cond::Has(_) | Cond::Gone(_) | Cond::ElementGone(_) | Cond::State(..) | Cond::Changed)) => polled(&k, w, &c, secs, json),
        (_, c) => Err(k.refuse(&format!("wait {}", c.word()))),
    }
}

fn timeout(secs: f64) -> Fail {
    Fail::new(fail::TIMEOUT, format!("timeout: it did not hold in {secs} s"))
}

/// `ok` and the record that met it, or as a node.
fn answer(met: Option<&Rec>, json: bool) {
    if json {
        println!("{}", serde_json::json!({ "ok": true, "node": met.map(|r| r.json(false)) }));
        return;
    }
    println!("ok");
    if let Some(r) = met {
        println!("{}", r.line(false));
    }
}

/// While waiting: a Stop since `start` ends it (8), the window made private
/// (2) or gone (7) too.
fn watched(w: &Window, start: Instant) -> R {
    match Term::allowed(&w.pid, false)? {
        Some(ago) if ago < start.elapsed().as_millis() as u64 => Err(fail::stopped()),
        _ => Ok(()),
    }
}

/// A read that failed while waiting: the window closing (7), or made
/// private (2), if that is why.
fn closed(w: &Window, f: Fail) -> Fail {
    if f.code != fail::OTHER {
        return f;
    }
    for _ in 0..4 {
        if let Err(why) = Term::allowed(&w.pid, false) {
            return why;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    f
}

/// A terminal's, from its layer: `ok` and what came out after its mark.
fn terminal(k: &Kinds, w: &Window, cond: Cond, secs: f64, from: Option<u64>, json: bool) -> R {
    let until = match cond {
        Cond::Has(t) => Until::Has(t, Vec::new()),
        Cond::Gone(t) => Until::Gone(t),
        Cond::Changed => Until::Changed,
        Cond::Settled(ms) => Until::Settled(ms),
        Cond::Prompt => Until::Prompt,
        c => return Err(k.refuse(&format!("wait {}", c.word()))),
    };
    if json {
        return Err(k.refuse("wait --json"));
    }
    let t = Term::of(k, w, "wait", false)?;
    let until = match until {
        Until::Has(text, _) if from.is_none() => Until::Has(text.clone(), t.screen()?.into_iter().filter(|l| says(l, &text)).collect()),
        u => u,
    };
    t.wait(&until, secs, from).map_err(|f| closed(w, f))
}

/// A scene's own `wait` for its expression; a Stop, the window made private
/// or closing meanwhile end it.
fn expr(w: &Window, sock: &std::path::Path, e: &str, whole: bool, secs: f64, json: bool) -> R {
    let line = if whole { format!("wait {e}") } else { format!("wait {e} {}ms", (secs * 1000.0).round() as u64) };
    let (tx, rx) = std::sync::mpsc::channel();
    let sock = sock.to_owned();
    std::thread::spawn(move || drop(tx.send(told(&sock, &line))));
    let (start, mut asked) = (Instant::now(), Instant::now());
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(said) => {
                let said = said.map_err(|f| closed(w, f))?;
                if json {
                    println!("{}", serde_json::json!({ "ok": true, "said": said.trim() }));
                } else {
                    print!("{said}");
                }
                return Ok(());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err("the scene did not answer".into()),
        }
        if asked.elapsed() >= ASKED {
            asked = Instant::now();
            watched(w, start)?;
        }
    }
}

/// A scene's items, read a frame apart until the condition holds.
fn polled(k: &Kinds, w: &Window, cond: &Cond, secs: f64, json: bool) -> R {
    if let Cond::ElementGone(Element::Id(n)) | Cond::State(Element::Id(n), ..) = cond {
        return Err(Fail::new(fail::NOT_HERE, format!("{} is a pleamar scene: its items are named, @name (a name of digits: @={n})", k.name)));
    }
    if let Cond::State(_, s, _) = cond {
        if !SCENE_STATES.contains(&s.as_str()) {
            return Err(usage(format!("a scene item's state is {}: not {s}", SCENE_STATES.join(", "))));
        }
    }
    let start = Instant::now();
    let first = described(k)?.recs;
    if let Cond::State(e, ..) = cond {
        if !first.iter().any(|r| r.id == e.said()) {
            return Err(Fail::new(fail::GONE, format!("gone: {} is not in {}: read it again with `tree {}`", e.said(), k.name, k.name)));
        }
    }
    let (mut now, mut asked) = (first.clone(), start);
    loop {
        if let Some(met) = met(cond, &first, &now) {
            answer(met, json);
            return Ok(());
        }
        if start.elapsed().as_secs_f64() >= secs {
            return Err(timeout(secs));
        }
        if asked.elapsed() >= ASKED {
            asked = Instant::now();
            watched(w, start)?;
        }
        std::thread::sleep(FRAME);
        now = described(k).map_err(|f| closed(w, f))?.recs;
    }
}

/// Whether it holds on a scene's records (`first`: those at the start), and
/// the one that met it, if one did.
fn met<'a>(cond: &Cond, first: &[Rec], now: &'a [Rec]) -> Option<Option<&'a Rec>> {
    let shown = |r: &&Rec| !r.states.iter().any(|s| s == "offscreen");
    let is = |r: &Rec, e: &Element| r.id == e.said();
    let has = |r: &Rec, s: &str| match s {
        "enabled" => !r.states.iter().any(|s| s == "disabled"),
        s => r.states.iter().any(|t| t == s),
    };
    // Boxes left out: a scroll moves every one.
    let said = |r: &Rec| (r.id.clone(), r.depth, r.role.clone(), r.name.clone(), r.text.clone(), r.states.clone());
    match cond {
        Cond::Has(t) => now.iter().filter(shown).find(|r| r.says(t)).map(Some),
        Cond::Gone(t) => (!now.iter().filter(shown).any(|r| r.says(t))).then_some(None),
        Cond::ElementGone(e) => (!now.iter().filter(shown).any(|r| is(r, e))).then_some(None),
        Cond::State(e, s, want) => now.iter().find(|r| is(r, e) && has(r, s) == *want).map(Some),
        Cond::Changed => (first.len() != now.len() || first.iter().zip(now).any(|(a, b)| said(a) != said(b))).then_some(None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    fn c(words: &[&str], text: Option<&str>) -> R<(Cond, f64)> {
        condition(&s(words), text)
    }

    fn rec(id: &str, name: &str, states: &[&str]) -> Rec {
        Rec { id: id.into(), depth: 1, role: "button".into(), name: name.into(), text: String::new(), rect: Some([0, 0, 10, 10]), states: states.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn one_grammar_for_every_kind() {
        assert_eq!(c(&["has"], Some("Saved")), Ok((Cond::Has("Saved".into()), 30.0)));
        assert_eq!(c(&["gone", "5s"], Some("Loading")), Ok((Cond::Gone("Loading".into()), 5.0)));
        assert_eq!(c(&["@24", "gone", "5"], None), Ok((Cond::ElementGone(Element::Id(24)), 5.0)));
        assert_eq!(c(&["@save", "not", "disabled", "500ms"], None), Ok((Cond::State(Element::Name("save".into()), "disabled".into(), false), 0.5)));
        assert_eq!(c(&["@=3", "checked"], None), Ok((Cond::State(Element::Name("3".into()), "checked".into(), true), 30.0)));
        assert_eq!(c(&["changed", "2"], None), Ok((Cond::Changed, 2.0)));
        assert_eq!(c(&["settled", "500ms", "10s"], None), Ok((Cond::Settled(500), 10.0)));
        assert_eq!(c(&["prompt", "5m"], None), Ok((Cond::Prompt, 300.0)));
        assert_eq!(c(&["expr", "status == \"Saved\"", "5s"], None), Ok((Cond::Expr("status == \"Saved\"".into(), false), 5.0)));
        assert_eq!(c(&["expr", "dirty", "==", "false"], None), Ok((Cond::Expr("dirty == false".into(), false), 30.0)));
        // As on main: a scene's expression as it is.
        assert_eq!(c(&["dirty", "==", "false", "3s"], None), Ok((Cond::Expr("dirty == false 3s".into(), true), 60.0)));
    }

    #[test]
    fn what_is_said_wrong_is_refused_with_the_form() {
        let bad = |words: &[&str], text: Option<&str>| c(words, text).unwrap_err().code;
        // A text is said as --text; a bare one is not taken.
        assert_eq!(bad(&["has", "Saved"], None), fail::USAGE);
        assert_eq!(bad(&["changed"], Some("OK")), fail::USAGE);
        assert_eq!(bad(&["settled"], None), fail::USAGE);
        assert_eq!(bad(&["settled", "soon"], None), fail::USAGE);
        assert_eq!(bad(&["settled", "2m"], None), fail::USAGE);
        assert_eq!(bad(&["prompt", "11m"], None), fail::USAGE);
        assert_eq!(bad(&["changed", "1", "2"], None), fail::USAGE);
        assert_eq!(bad(&["@24"], None), fail::USAGE);
        assert_eq!(bad(&["@", "gone"], None), fail::USAGE);
        assert_eq!(bad(&["expr"], None), fail::USAGE);
        assert_eq!(bad(&[], None), fail::USAGE);
        // Any other words are no condition: a scene's expression, refused by
        // every other kind (see `wait`).
        assert_eq!(c(&["output"], None), Ok((Cond::Expr("output".into(), true), 60.0)));
        assert!(c(&["has"], None).unwrap_err().msg.contains("--text"));
    }

    fn kinds(scene: bool, shells: usize) -> Kinds {
        Kinds { name: "7002".into(), scene: scene.then(|| ("notes".to_owned(), std::path::PathBuf::new())), shells: (0..shells).map(|k| (7044 + k as i32, Default::default())).collect(), private: false, locked: false, popups: Vec::new(), slot: None }
    }

    #[test]
    fn each_kind_refuses_what_it_does_not_wait_for() {
        let w = Window::parse("7002").unwrap();
        let refused = |k: &Kinds, words: &[&str], text: Option<&str>| {
            let (cond, secs) = c(words, text).unwrap();
            let f = match &k.scene {
                Some(_) if matches!(cond, Cond::Has(_) | Cond::Gone(_) | Cond::ElementGone(_) | Cond::State(..) | Cond::Changed) => polled(k, &w, &cond, secs, false),
                None if k.is_text() => terminal(k, &w, cond, secs, None, false),
                _ => Err(k.refuse(&format!("wait {}", cond.word()))),
            };
            f.unwrap_err()
        };
        let term = kinds(false, 1);
        for words in [&["@24", "gone"][..], &["@24", "checked"], &["expr", "x"]] {
            let f = refused(&term, words, None);
            assert_eq!(f.code, fail::NOT_HERE, "{words:?}");
            assert!(f.msg.starts_with("7002 is a terminal: it does text,look (wait "), "{}", f.msg);
            assert!(f.msg.contains("it waits for has --text T [--from M], gone --text T, changed, settled DURATION, prompt"), "{}", f.msg);
        }
        let scene = kinds(true, 0);
        assert_eq!(refused(&scene, &["prompt"], None).code, fail::NOT_HERE);
        assert_eq!(refused(&scene, &["settled", "500ms"], None).code, fail::NOT_HERE);
        // A scene's items are named, before anything is read.
        assert_eq!(refused(&scene, &["@24", "gone"], None).code, fail::NOT_HERE);
        assert_eq!(refused(&scene, &["@save", "shiny"], None).code, fail::USAGE);
        let look = kinds(false, 0);
        for words in [&["changed"][..], &["@24", "gone"], &["settled", "500ms"], &["prompt"]] {
            let f = refused(&look, words, None);
            assert_eq!(f.code, fail::NOT_HERE, "{words:?}");
            assert!(f.msg.contains("it waits for nothing"), "{}", f.msg);
        }
        assert_eq!(refused(&look, &["has"], Some("x")).code, fail::NOT_HERE);
    }

    #[test]
    fn an_actions_until_is_refused_before_it_acts() {
        let code = |k: &Kinds, words: &[&str]| check(&s(words), k).err().map(|f| f.code);
        let (term, scene, look) = (kinds(false, 1), kinds(true, 0), kinds(false, 0));
        assert_eq!(code(&scene, &[]), Some(fail::USAGE));
        assert_eq!(code(&scene, &["has", "Saved"]), Some(fail::USAGE));
        assert_eq!(code(&scene, &["has", "--text", "Saved", "5s"]), None);
        assert_eq!(code(&scene, &["@save", "not", "disabled"]), None);
        assert_eq!(code(&term, &["prompt", "5s"]), None);
        assert_eq!(code(&term, &["@24", "gone"]), Some(fail::NOT_HERE));
        assert_eq!(code(&scene, &["expr", "dirty"]), None);
        assert_eq!(code(&scene, &["dirty", "==", "true"]), None);
        assert_eq!(code(&scene, &["@24", "gone"]), Some(fail::NOT_HERE));
        assert_eq!(code(&scene, &["prompt"]), Some(fail::NOT_HERE));
        assert_eq!(code(&look, &["settled", "300ms"]), Some(fail::NOT_HERE));
        assert_eq!(code(&look, &["changed"]), Some(fail::NOT_HERE));
        assert_eq!(code(&look, &["dirty", "==", "true"]), Some(fail::USAGE));
    }

    #[test]
    fn a_scene_holds_on_its_records() {
        let first = vec![rec("@save", "Save", &["disabled"]), rec("@-", "Ready", &[]), rec("@far", "Far away", &["offscreen"])];
        let now = vec![rec("@save", "Save", &[]), rec("@-", "Saved", &[]), rec("@far", "Far away", &["offscreen"])];
        let held = |cond: Cond| met(&cond, &first, &now).map(|m| m.map(|r| r.id.clone()));
        assert_eq!(held(Cond::Has("saved".into())), Some(Some("@-".into())));
        assert_eq!(held(Cond::Has("far".into())), None);
        assert_eq!(held(Cond::Gone("ready".into())), Some(None));
        assert_eq!(held(Cond::Gone("save".into())), None);
        assert_eq!(held(Cond::ElementGone(Element::Name("save".into()))), None);
        assert_eq!(held(Cond::ElementGone(Element::Name("far".into()))), Some(None));
        assert_eq!(held(Cond::ElementGone(Element::Name("other".into()))), Some(None));
        assert_eq!(held(Cond::State(Element::Name("save".into()), "enabled".into(), true)), Some(Some("@save".into())));
        assert_eq!(held(Cond::State(Element::Name("save".into()), "disabled".into(), true)), None);
        assert_eq!(held(Cond::State(Element::Name("save".into()), "disabled".into(), false)), Some(Some("@save".into())));
        assert_eq!(held(Cond::Changed), Some(None));
        assert_eq!(met(&Cond::Changed, &first, &first), None);
        // A terminal's lines are case folded as records are.
        assert!(says("Build OK", "build ok") && !says("Build OK", "failed"));
        // A box that moved is no change.
        let mut moved = first.clone();
        moved[0].rect = Some([5, 5, 10, 10]);
        assert_eq!(met(&Cond::Changed, &first, &moved), None);
    }
}
