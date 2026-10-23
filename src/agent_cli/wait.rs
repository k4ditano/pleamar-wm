//! `wait PID COND [DURATION]`: until something holds. One grammar for every
//! kind of window; each takes what it can and refuses the rest.
//!
//! A scene's expression is its own `wait`, the rest its `describe` read a
//! frame apart.

use super::*;
use std::time::{Duration, Instant};

const WAIT: Spec = Spec { values: &["--text"], flags: &["--json"], ..PLAIN };

/// The longest wait, in seconds.
const LONGEST: f64 = 60.0;
/// Between two reads of a scene: a frame.
const FRAME: Duration = Duration::from_millis(16);

const FORM: &str = "wait PID has --text T | gone --text T | @X gone | @X [not] STATE | changed | expr 'EXPR' [DURATION]";
const CONDITIONS: [&str; 4] = ["has", "gone", "changed", "expr"];
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
            Cond::Expr(..) => "expr",
        }
    }
}

/// The condition and how long, in seconds: 30 unless given, up to 60.
fn condition(words: &[String], text: Option<&str>) -> R<(Cond, f64)> {
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let at = |s: &str| Element::parse(s);
    let (cond, rest) = match (&w[..], text) {
        (["has", ..], Some(t)) => (Cond::Has(t.to_owned()), &w[1..]),
        (["gone", ..], Some(t)) => (Cond::Gone(t.to_owned()), &w[1..]),
        (["has" | "gone", ..], None) => return Err(usage(format!("wait PID {} --text T: a text is said as --text", w[0]))),
        (_, Some(_)) => return Err(usage(format!("--text is for has and gone: {FORM}"))),
        (["changed", ..], None) => (Cond::Changed, &w[1..]),
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
    } else {
        "nothing"
    }
}

pub(super) fn wait(words: &[String]) -> R {
    until(words)
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
    match (&k.scene, &cond) {
        (Some(_), Cond::ElementGone(e) | Cond::State(e, ..)) if !named(e) => refused(),
        (Some(_), _) => Ok(()),
        _ => refused(),
    }
}

/// `wait`, and an action's `--until`.
pub(super) fn until(words: &[String]) -> R {
    let a = parse(words, &WAIT)?;
    let w = a.win()?;
    let (cond, secs) = condition(&a.words, a.text())?;
    let k = Kinds::of(w)?;
    k.open()?;
    if matches!(cond, Cond::Expr(_, true)) && !k.is_scene() {
        return Err(usage(FORM));
    }
    let json = a.has("--json");
    match (&k.scene, cond) {
        (Some((_, sock)), Cond::Expr(e, whole)) => expr(sock, &e, whole, secs, json),
        (Some(_), c) => polled(&k, &c, secs, json),
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

/// A scene's own `wait` for its expression.
fn expr(sock: &std::path::Path, e: &str, whole: bool, secs: f64, json: bool) -> R {
    let line = if whole { format!("wait {e}") } else { format!("wait {e} {}ms", (secs * 1000.0).round() as u64) };
    let said = told(sock, &line)?;
    if json {
        println!("{}", serde_json::json!({ "ok": true, "said": said.trim() }));
    } else {
        print!("{said}");
    }
    Ok(())
}

/// A scene's items, read a frame apart until the condition holds.
fn polled(k: &Kinds, cond: &Cond, secs: f64, json: bool) -> R {
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
    let mut now = first.clone();
    loop {
        if let Some(met) = met(cond, &first, &now) {
            answer(met, json);
            return Ok(());
        }
        if start.elapsed().as_secs_f64() >= secs {
            return Err(timeout(secs));
        }
        std::thread::sleep(FRAME);
        now = described(k)?.recs;
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
        Cond::Expr(..) => None,
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
        assert_eq!(bad(&["changed", "1", "2"], None), fail::USAGE);
        assert_eq!(bad(&["changed", "2m"], None), fail::USAGE);
        assert_eq!(bad(&["@24"], None), fail::USAGE);
        assert_eq!(bad(&["@", "gone"], None), fail::USAGE);
        assert_eq!(bad(&["expr"], None), fail::USAGE);
        assert_eq!(bad(&[], None), fail::USAGE);
        // Any other words are no condition: a scene's expression, refused by
        // every other kind (see `wait`).
        assert_eq!(c(&["output"], None), Ok((Cond::Expr("output".into(), true), 60.0)));
        assert!(c(&["has"], None).unwrap_err().msg.contains("--text"));
    }

    fn kinds(scene: bool) -> Kinds {
        Kinds { name: "7002".into(), scene: scene.then(|| ("notes".to_owned(), std::path::PathBuf::new())), private: false, locked: false, popups: Vec::new(), slot: None }
    }

    #[test]
    fn each_kind_refuses_what_it_does_not_wait_for() {
        let refused = |k: &Kinds, words: &[&str], text: Option<&str>| {
            let (cond, secs) = c(words, text).unwrap();
            let f = match &k.scene {
                Some(_) => polled(k, &cond, secs, false),
                None => Err(k.refuse(&format!("wait {}", cond.word()))),
            };
            f.unwrap_err()
        };
        let scene = kinds(true);
        // A scene's items are named, before anything is read.
        assert_eq!(refused(&scene, &["@24", "gone"], None).code, fail::NOT_HERE);
        assert_eq!(refused(&scene, &["@save", "shiny"], None).code, fail::USAGE);
        let look = kinds(false);
        for words in [&["changed"][..], &["@24", "gone"], &["expr", "x"]] {
            let f = refused(&look, words, None);
            assert_eq!(f.code, fail::NOT_HERE, "{words:?}");
            assert!(f.msg.contains("it waits for nothing"), "{}", f.msg);
        }
        assert_eq!(refused(&look, &["has"], Some("x")).code, fail::NOT_HERE);
    }

    #[test]
    fn an_actions_until_is_refused_before_it_acts() {
        let code = |k: &Kinds, words: &[&str]| check(&s(words), k).err().map(|f| f.code);
        let (scene, look) = (kinds(true), kinds(false));
        assert_eq!(code(&scene, &[]), Some(fail::USAGE));
        assert_eq!(code(&scene, &["has", "Saved"]), Some(fail::USAGE));
        assert_eq!(code(&scene, &["has", "--text", "Saved", "5s"]), None);
        assert_eq!(code(&scene, &["@save", "not", "disabled"]), None);
        assert_eq!(code(&scene, &["expr", "dirty"]), None);
        assert_eq!(code(&scene, &["dirty", "==", "true"]), None);
        assert_eq!(code(&scene, &["@24", "gone"]), Some(fail::NOT_HERE));
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
        // A box that moved is no change.
        let mut moved = first.clone();
        moved[0].rect = Some([5, 5, 10, 10]);
        assert_eq!(met(&Cond::Changed, &first, &moved), None);
    }
}
