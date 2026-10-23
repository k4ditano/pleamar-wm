//! Reading a window: `tree`, `find`, `at`, and following it, `listen`.

use super::*;

const TREE: Spec = Spec { flags: &["--hidden", "--json", "--long", "--boxes"], values: &["--within", "--role", "--max"], maybe: &["--since"], ..PLAIN };
const FIND: Spec = Spec { flags: &["--json", "--long"], values: &["--text"], ..PLAIN };
const AT: Spec = Spec { flags: &["--json", "--long"], ..PLAIN };

const TREE_FORM: &str = "tree PID [--hidden] [--within @X] [--role ROLE] [--max N] [--since [V]] [--json] [--long]";

/// What a `tree` gives: a part, a role, at most so many.
struct Part<'a> {
    within: Option<Element>,
    role: Option<&'a str>,
    max: Option<usize>,
}

/// `tree PID`: the whole window and its version, what is useful in it
/// under its containers; `--since [V]` what changed since this agent's last
/// read (or V); `--within @X`, `--role`, `--max` a part of it. Old: `tree
/// PID json`.
pub(super) fn tree(words: &[String]) -> R {
    let a = parse(words, &TREE)?;
    let w = a.win()?;
    let json = a.has("--json") || a.words == ["json"];
    if !a.words.is_empty() && a.words != ["json"] {
        return Err(usage(TREE_FORM));
    }
    let within = a.value("--within").map(|e| Element::parse(e).ok_or_else(|| usage(format!("--within {e}: an element, @24 or @name")))).transpose()?;
    let part = Part { within, role: a.value("--role"), max: a.number::<usize>("--max")?.filter(|m| *m > 0) };
    if a.has("--since") && part.within.is_some() {
        return Err(usage("--since answers the whole window: --within reads a part of it now"));
    }
    let (k, long) = (Kinds::of(w)?, a.has("--long"));
    k.open()?;
    if k.is_scene() {
        return scene_tree(&k, &a, &part, json, long);
    }
    Err(k.refuse("tree"))
}

/// What is shown of a scene's items: none that is off its view, nor what
/// such an item holds.
fn shown_items(recs: Vec<Rec>) -> Vec<Rec> {
    let mut under: Option<usize> = None;
    recs.into_iter()
        .filter(|r| {
            if under.is_some_and(|d| r.depth > d) {
                return false;
            }
            under = r.states.iter().any(|s| s == "offscreen").then_some(r.depth);
            under.is_none()
        })
        .collect()
}

/// That item of a scene's and what it holds, it at depth 0.
fn subtree(recs: &[Rec], id: &str) -> Option<Vec<Rec>> {
    let i = recs.iter().position(|r| r.id == id)?;
    let d = recs[i].depth;
    let end = (i + 1..recs.len()).find(|&j| recs[j].depth <= d).unwrap_or(recs.len());
    Some(recs[i..end].iter().map(|r| Rec { depth: r.depth - d, ..r.clone() }).collect())
}

/// A scene's `tree`: its items as records, each read kept here as its
/// next version (the scene keeps none) for `--since`, by name.
fn scene_tree(k: &Kinds, a: &Args, part: &Part, json: bool, long: bool) -> R {
    let read = described(k)?;
    let scene = k.scene.as_ref().map_or("", |s| s.0.as_str());
    let hidden = a.has("--hidden");
    let shown = |recs: Vec<Rec>| if hidden { recs } else { shown_items(recs) };
    match &part.within {
        Some(Element::Name(n)) => {
            let id = Element::Name(n.clone()).said();
            let recs = subtree(&read.recs, &id).ok_or_else(|| Fail::new(fail::GONE, format!("gone: {id} is not in {} now: read it again with `tree {}`", k.name, k.name)))?;
            Read { recs: shown(recs), ..read }.shaped(part.role, part.max, &k.name).print(json, long);
            return Ok(());
        }
        Some(Element::Id(n)) => return Err(Fail::new(fail::NOT_HERE, format!("{} is a pleamar scene: its items are named, @name (a name of digits: @={n})", k.name))),
        None => {}
    }
    let last = last_version(&k.name);
    let then = kept_scene(&k.name);
    let v = keep_scene(&k.name, scene, &read.recs);
    let head = format!("tree {v} (scene {scene})");
    if !a.has("--since") {
        Read { head, version: Some(v.to_string()), recs: shown(read.recs), ..read }.shaped(part.role, part.max, &k.name).print(json, long);
        return Ok(());
    }
    let explicit = a.number::<u64>("--since").map_err(|_| usage("--since V: a version, as `tree` gives it"))?;
    let asked = explicit.or(last.map(|l| l.v));
    let since = match then {
        Some((kv, name, then)) if Some(kv) == asked && name == scene => {
            let list = scene_changes(&then, &read.recs, a.has("--boxes"));
            let list = list.into_iter().filter(|c| hidden || !matches!(c, Change::Came(r) if r.states.iter().any(|s| s == "offscreen"))).collect();
            Since::Changes { version: v.to_string(), list, more: 0 }
        }
        _ => Since::Full(Read { head, version: Some(v.to_string()), recs: shown(read.recs), ..read }),
    };
    let why = match asked {
        None => format!("no read of {} kept by this agent yet", k.name),
        Some(asked) => format!("read {asked} is not the last this agent got of it (of a scene only the last is kept)"),
    };
    let said = Said { last: explicit.is_none().then(|| last.map(|l| l.said())).flatten(), why_full: &why, role: part.role, max: part.max, json, long };
    print_since(since, &k.name, &said);
    Ok(())
}

/// `find PID --text T`: what says T, in its name or its text.
pub(super) fn find(words: &[String]) -> R {
    let a = parse(words, &FIND)?;
    let w = a.win()?;
    let text = match (a.text(), a.words.is_empty()) {
        (Some(t), true) => t.to_owned(),
        _ => return Err(usage("find PID --text T: what to look for, said as --text")),
    };
    let (k, json, long) = (Kinds::of(w)?, a.has("--json"), a.has("--long"));
    k.open()?;
    if k.is_scene() {
        let read = described(&k)?;
        Read { recs: with_containers(&read.recs, |r| r.says(&text)), ..read }.print(json, long);
        return Ok(());
    }
    Err(k.refuse("find"))
}

/// `at PID X Y`: the element there, and what it is in.
pub(super) fn at(words: &[String]) -> R {
    let a = parse(words, &AT)?;
    let w = a.win()?;
    let (x, y) = (number(a.word(0), "x")?, number(a.word(1), "y")?);
    let (k, json, long) = (Kinds::of(w)?, a.has("--json"), a.has("--long"));
    k.open()?;
    if k.is_scene() {
        let read = described(&k)?;
        Read { recs: at_point(&read.recs, x, y), ..read }.print(json, long);
        return Ok(());
    }
    Err(k.refuse("at"))
}

/// `listen PID [DURATION]`: a scene's events (old: `watch PID [SECONDS]`),
/// a line each as it happens.
pub(super) fn listen(words: &[String]) -> R {
    let a = parse(words, &PLAIN)?;
    let w = a.win()?;
    let secs = match a.word(0) {
        None => 30.0,
        Some(s) => Some(duration(s)?).filter(|s| *s > 0.0 && *s <= 600.0).ok_or_else(|| usage(format!("{s}: a duration up to 10 minutes")))?,
    };
    let k = Kinds::of(w)?;
    k.open()?;
    if let Some((_, sock)) = &k.scene {
        return Ok(order(sock, &format!("watch {secs}s"), &mut |l| {
            println!("{l}");
            let _ = std::io::stdout().flush();
        })?);
    }
    Err(k.refuse("listen"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, depth: usize, states: &[&str]) -> Rec {
        Rec { id: id.into(), depth, role: "item".into(), name: id.into(), text: String::new(), rect: None, states: states.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn a_scene_part_and_what_is_shown_of_it() {
        let recs = vec![rec("@list", 0, &[]), rec("@a", 1, &[]), rec("@b", 1, &["offscreen"]), rec("@c", 2, &[]), rec("@save", 0, &[])];
        let ids = |v: &[Rec]| v.iter().map(|r| format!("{}{}", r.depth, r.id)).collect::<Vec<_>>();
        assert_eq!(ids(&shown_items(recs.clone())), ["0@list", "1@a", "0@save"]);
        assert_eq!(ids(&subtree(&recs, "@b").unwrap()), ["0@b", "1@c"]);
        assert_eq!(ids(&subtree(&recs, "@list").unwrap()).len(), 4);
        assert!(subtree(&recs, "@nothing").is_none());
    }
}
