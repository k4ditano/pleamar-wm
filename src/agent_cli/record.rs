//! Elements in one format, a scene's items: `@save button "name" text
//! "..." X,Y WxH states`, indented by depth; or as JSON.

use super::*;

/// An element, as the agent reads it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Rec {
    /// `@24`, `@save`, `@=3`; `@-` for a text that cannot be acted on.
    pub(super) id: String,
    pub(super) depth: usize,
    pub(super) role: String,
    pub(super) name: String,
    pub(super) text: String,
    /// In look coordinates.
    pub(super) rect: Option<[i64; 4]>,
    /// Those that are not the default.
    pub(super) states: Vec<String>,
}

/// How much of a name or a text is given.
pub(super) fn cut(long: bool) -> usize {
    if long { 400 } else { 120 }
}

impl Rec {
    /// The number its id is, if it is one.
    pub(super) fn number(&self) -> Option<u32> {
        self.id.strip_prefix('@')?.parse().ok()
    }

    /// `@24 push_button "name" text "..." X,Y WxH states`.
    pub(super) fn line(&self, long: bool) -> String {
        let quote = |s: &str| format!("{:?}", s.chars().take(cut(long)).collect::<String>());
        let mut line = format!("{} {}", self.id, self.role);
        if !self.name.is_empty() {
            line.push_str(&format!(" {}", quote(&self.name)));
        }
        if !self.text.is_empty() && self.text != self.name {
            line.push_str(&format!(" text {}", quote(&self.text)));
        }
        if let Some([x, y, w, h]) = self.rect {
            line.push_str(&format!(" {x},{y} {w}x{h}"));
        }
        if !self.states.is_empty() {
            line.push_str(&format!(" {}", self.states.join(",")));
        }
        line
    }

    /// What changed from this to `now`: `+STATE`, `-STATE`, and what its
    /// role, name, text or box were.
    pub(super) fn to(&self, now: &Rec, long: bool) -> Vec<String> {
        let quote = |s: &str| format!("{:?}", s.chars().take(cut(long)).collect::<String>());
        let mut d: Vec<String> = now.states.iter().filter(|s| !self.states.contains(s)).map(|s| format!("+{s}")).collect();
        d.extend(self.states.iter().filter(|s| !now.states.contains(s)).map(|s| format!("-{s}")));
        if self.role != now.role {
            d.push(format!("role was {}", self.role));
        }
        if self.name != now.name {
            d.push(format!("name was {}", quote(&self.name)));
        }
        if self.text != now.text {
            d.push(format!("text was {}", quote(&self.text)));
        }
        if self.rect != now.rect {
            d.push(self.rect.map_or("had no box".into(), |[x, y, w, h]| format!("was at {x},{y} {w}x{h}")));
        }
        d
    }

    pub(super) fn json(&self, long: bool) -> serde_json::Value {
        let c = |s: &str| s.chars().take(cut(long)).collect::<String>();
        serde_json::json!({
            "id": if self.id == "@-" { serde_json::Value::Null } else { self.id.clone().into() },
            "depth": self.depth,
            "role": self.role,
            "name": c(&self.name),
            "text": c(&self.text),
            "box": self.rect,
            "states": self.states,
        })
    }

    /// Whether it says that: in its name or its text, case folded.
    pub(super) fn says(&self, text: &str) -> bool {
        let t = text.to_lowercase();
        self.name.to_lowercase().contains(&t) || self.text.to_lowercase().contains(&t)
    }

    /// Whether that is its whole name, case folded (an unnamed one: its text).
    pub(super) fn named(&self, text: &str) -> bool {
        let t = text.to_lowercase();
        self.name.to_lowercase() == t || (self.name.is_empty() && self.text.to_lowercase() == t)
    }
}

/// Of the records, those that hold and those they are in, as a tree is.
pub(super) fn with_containers(recs: &[Rec], holds: impl Fn(&Rec) -> bool) -> Vec<Rec> {
    let mut keep = vec![false; recs.len()];
    for k in (0..recs.len()).filter(|k| holds(&recs[*k])) {
        keep[k] = true;
        let mut depth = recs[k].depth;
        for j in (0..k).rev() {
            if recs[j].depth < depth {
                keep[j] = true;
                depth = recs[j].depth;
            }
        }
    }
    recs.iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r.clone()).collect()
}

/// Of those whose whole name is that text, the one meant: the only one, or
/// with `--first` the first; several are listed (exit 6).
pub(super) fn one<'a>(found: &[&'a Rec], text: &str, first: bool, w: &str) -> R<&'a Rec> {
    match found {
        [] => Err(Fail::from(format!("not found: nothing in {w} is named {text:?} (`find {w} --text T` searches names and texts)"))),
        [one] => Ok(one),
        [one, ..] if first => Ok(one),
        several => {
            let lines: Vec<String> = several.iter().map(|r| format!("  {}", r.line(false))).collect();
            Err(Fail::new(fail::AMBIGUOUS, format!("ambiguous: {} in {w} are named {text:?}: name one by its @, or --first\n{}", several.len(), lines.join("\n"))))
        }
    }
}

/// The deepest record whose box holds the point, and those it is in.
pub(super) fn at_point(recs: &[Rec], x: f64, y: f64) -> Vec<Rec> {
    let holds = |r: &Rec| r.rect.is_some_and(|[rx, ry, w, h]| x >= rx as f64 && y >= ry as f64 && x < (rx + w) as f64 && y < (ry + h) as f64);
    let deepest = recs.iter().enumerate().filter(|(_, r)| holds(r)).max_by_key(|(k, r)| (r.depth, std::cmp::Reverse(r.rect.map_or(0, |b| b[2] * b[3])), *k)).map(|(k, _)| k);
    match deepest {
        Some(d) => with_containers(recs, |r| std::ptr::eq(r, &recs[d])),
        None => Vec::new(),
    }
}

/// Roles acted on whatever their states say.
fn acted(role: &str) -> bool {
    matches!(
        role,
        "push_button" | "button" | "toggle_button" | "push_button_menu" | "check_box" | "radio_button" | "menu_item" | "check_menu_item" | "radio_menu_item" | "link" | "entry" | "password_text" | "combo_box" | "page_tab" | "slider" | "spin_button" | "scroll_bar" | "tree_item" | "switch" | "toggle" | "tab" | "field" | "item"
    )
}

/// Roles that say nothing to an agent, named or not.
fn idle(role: &str) -> bool {
    matches!(role, "separator" | "icon" | "animation" | "filler" | "redundant_object" | "unknown")
}

/// Runs of alike rows past this many are folded.
const ALIKE: usize = 8;

impl Rec {
    /// Whether `tree` gives it: what can be acted on (a scene's named items,
    /// a role that is, or focusable, editable, checked...), a heading, a
    /// text with something to say.
    pub(super) fn useful(&self) -> bool {
        let named_item = self.id != "@-" && self.number().is_none();
        let acts = acted(&self.role) || self.states.iter().any(|s| matches!(s.as_str(), "focusable" | "editable" | "checked" | "pressed" | "selected" | "expanded" | "collapsed"));
        named_item || acts || self.role == "heading" || ((!self.name.is_empty() || !self.text.is_empty()) && !idle(&self.role))
    }

    /// As it is kept between reads: whole, uncut.
    pub(super) fn kept(&self) -> serde_json::Value {
        serde_json::json!({ "id": self.id, "depth": self.depth, "role": self.role, "name": self.name, "text": self.text, "box": self.rect, "states": self.states })
    }

    pub(super) fn from_kept(j: &serde_json::Value) -> Option<Rec> {
        let s = |k: &str| j[k].as_str().map(str::to_owned);
        let rect = j["box"].as_array().and_then(|b| <[i64; 4]>::try_from(b.iter().filter_map(|v| v.as_i64()).collect::<Vec<_>>()).ok());
        let states = j["states"].as_array()?.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
        Some(Rec { id: s("id")?, depth: j["depth"].as_u64()? as usize, role: s("role")?, name: s("name")?, text: s("text")?, rect, states })
    }
}

/// Where a row's own ends: the first after it that is not in it.
fn end_of(recs: &[Rec], i: usize) -> usize {
    (i + 1..recs.len()).find(|&j| recs[j].depth <= recs[i].depth).unwrap_or(recs.len())
}

/// Rows folded: so many more of that role, without a name, at that depth,
/// before the record `at` of what is given.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Fold {
    pub(super) at: usize,
    pub(super) depth: usize,
    pub(super) count: u64,
    pub(super) role: String,
}

/// Runs of more than `ALIKE` rows one after another under one container,
/// of a role and states, with no name nor text in them nor in what they
/// hold: the first `ALIKE` given, the rest folded. Named rows never are.
pub(super) fn fold(recs: Vec<Rec>) -> (Vec<Rec>, Vec<Fold>) {
    let nameless = |a: usize, b: usize| recs[a..b].iter().all(|r| r.name.is_empty() && r.text.is_empty());
    let (mut out, mut folds, mut i) = (Vec::new(), Vec::new(), 0);
    while i < recs.len() {
        let (first, mut rows, mut end) = (&recs[i], 1, end_of(&recs, i));
        if nameless(i, end) {
            let mut kept_to = None;
            while end < recs.len() && recs[end].depth == first.depth && recs[end].role == first.role && recs[end].states == first.states {
                let next = end_of(&recs, end);
                if !nameless(end, next) {
                    break;
                }
                rows += 1;
                if rows == ALIKE + 1 {
                    kept_to = Some(end);
                }
                end = next;
            }
            if let Some(cut) = kept_to {
                out.extend(recs[i..cut].iter().cloned());
                folds.push(Fold { at: out.len(), depth: first.depth, count: (rows - ALIKE) as u64, role: first.role.clone() });
                i = end;
                continue;
            }
        }
        out.push(recs[i].clone());
        i += 1;
    }
    (out, folds)
}

/// A read of elements: what it is (`tree 12`, `scene notes`), its records
/// and its rows folded, how many were not given, and why they were not.
pub(super) struct Read {
    pub(super) head: String,
    pub(super) version: Option<String>,
    pub(super) recs: Vec<Rec>,
    pub(super) folds: Vec<Fold>,
    pub(super) more: u64,
    pub(super) left: String,
}

impl Read {
    /// As `tree` gives it: what is useful under its containers (`role`: that
    /// role's instead), alike rows folded, at most `max` of those given for
    /// themselves (the rest counted; their containers come with them).
    pub(super) fn shaped(self, role: Option<&str>, max: Option<usize>, w: &str) -> Read {
        let hit = |r: &Rec| role.map_or_else(|| r.useful(), |role| r.role == role);
        let first = self.recs.first().map(|r| r as *const Rec);
        let recs = with_containers(&self.recs, |r| (role.is_none() && Some(r as *const Rec) == first) || hit(r));
        let (mut recs, mut folds) = fold(recs);
        let mut more = self.more;
        let mut left = self.left;
        let hits: Vec<usize> = (0..recs.len()).filter(|&k| hit(&recs[k])).collect();
        if let Some(max) = max.filter(|m| *m > 0 && hits.len() > *m) {
            let cut = hits[max - 1] + 1;
            more += (hits.len() - max) as u64 + folds.iter().filter(|f| f.at >= cut).map(|f| f.count).sum::<u64>();
            recs.truncate(cut);
            folds.retain(|f| f.at < cut);
            left = format!("`tree {w} --within @X` for a part, `find {w} --text T` for what you look for");
        }
        Read { recs, folds, more, left, ..self }
    }

    /// Its nodes as JSON, a fold as `{ "depth", "role", "folded" }`.
    pub(super) fn json_nodes(&self, long: bool) -> Vec<serde_json::Value> {
        let mut all = Vec::new();
        for k in 0..=self.recs.len() {
            all.extend(self.folds.iter().filter(|f| f.at == k).map(|f| serde_json::json!({ "depth": f.depth, "role": f.role, "folded": f.count })));
            all.extend(self.recs.get(k).map(|r| r.json(long)));
        }
        all
    }

    pub(super) fn print(&self, json: bool, long: bool) {
        let fold_at = |k: usize| self.folds.iter().filter(move |f| f.at == k);
        if json {
            println!("{}", serde_json::json!({ "version": self.version, "nodes": self.json_nodes(long), "more": self.more }));
            return;
        }
        println!("{}", self.head);
        if self.recs.is_empty() {
            println!("(nothing says that)");
        }
        for k in 0..=self.recs.len() {
            for f in fold_at(k) {
                println!("{}... {} more {} without a name", "  ".repeat(f.depth), f.count, f.role);
            }
            if let Some(r) = self.recs.get(k) {
                println!("{}{}", "  ".repeat(r.depth), r.line(long));
            }
        }
        if self.more > 0 {
            println!("... {} more not given: {}", self.more, self.left);
        }
    }
}

/// What changed since a read, in the order the elements are in now, then
/// those gone: `+` came, `~` its role, name, text or states (or box) are
/// other now (and what it was), `-` went: as it was, and how many more went
/// in it (a branch gone is its top).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Change {
    Came(Rec),
    Other(Rec, Rec),
    Went(Rec, usize),
}

impl Change {
    fn rec(&self) -> &Rec {
        match self {
            Change::Came(r) | Change::Other(r, _) | Change::Went(r, _) => r,
        }
    }

    fn sign(&self) -> &'static str {
        match self {
            Change::Came(_) => "+",
            Change::Other(..) => "~",
            Change::Went(..) => "-",
        }
    }

    /// `+ @ID ...`, `~ @ID ... (+STATE, -STATE, name was "N")`, `- @ID ...
    /// (N more in it)`; an unnamed text gone: `- @- "what it said"`.
    fn line(&self, long: bool) -> String {
        let more = |k: usize| if k > 0 { format!(" ({k} more in it)") } else { String::new() };
        match self {
            Change::Went(r, k) if r.id == "@-" => format!("- @- {:?}{}", if r.name.is_empty() { &r.text } else { &r.name }.chars().take(cut(long)).collect::<String>(), more(*k)),
            Change::Went(r, k) => format!("- {}{}", r.line(long), more(*k)),
            Change::Other(r, was) => match was.to(r, long) {
                d if d.is_empty() => format!("~ {}", r.line(long)),
                d => format!("~ {} ({})", r.line(long), d.join(", ")),
            },
            c => format!("{} {}", c.sign(), c.rec().line(long)),
        }
    }
}

/// An answer to `--since`: what changed, or the whole tree (`full`).
pub(super) enum Since {
    Changes { version: String, list: Vec<Change>, more: u64 },
    Full(Read),
}

/// What a scene's items are known by between reads: a name, or an unnamed
/// one's container and what it says (the second alike: `#2`).
fn scene_keys(recs: &[Rec]) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    (0..recs.len())
        .map(|k| {
            let r = &recs[k];
            if r.id != "@-" {
                return r.id.clone();
            }
            let parent = (0..k).rev().find(|&j| recs[j].depth < r.depth).map_or("", |j| recs[j].id.as_str());
            let key = format!("{parent}/{}/{}", r.name, r.text);
            let n = seen.entry(key.clone()).or_insert(0);
            *n += 1;
            if *n == 1 { key } else { format!("{key}#{n}") }
        })
        .collect()
}

/// What changed from one read of a scene to the next, by name (`boxes`:
/// boxes too); an unnamed text that says another thing went, and came.
pub(super) fn scene_changes(then: &[Rec], now: &[Rec], boxes: bool) -> Vec<Change> {
    let (tk, nk) = (scene_keys(then), scene_keys(now));
    let before: std::collections::HashMap<&str, &Rec> = tk.iter().map(String::as_str).zip(then).collect();
    let mut out = Vec::new();
    for (k, n) in nk.iter().zip(now) {
        match before.get(k.as_str()) {
            None => out.push(Change::Came(n.clone())),
            Some(b) if b.role != n.role || b.name != n.name || b.text != n.text || b.states != n.states || (boxes && b.rect != n.rect) => out.push(Change::Other(n.clone(), (*b).clone())),
            Some(_) => {}
        }
    }
    let after: std::collections::HashSet<&str> = nk.iter().map(String::as_str).collect();
    out.extend(went(tk.iter().zip(then).map(|(k, r)| (r.depth, !after.contains(k.as_str())))).into_iter().map(|(i, k)| Change::Went(then[i].clone(), k)));
    out
}

/// Of records in order, by their depth and whether they went: the first of
/// each branch that went, and how many more went in it.
fn went(all: impl IntoIterator<Item = (usize, bool)>) -> Vec<(usize, usize)> {
    let (mut out, mut open): (Vec<(usize, usize)>, Option<usize>) = (Vec::new(), None);
    for (i, (depth, gone)) in all.into_iter().enumerate() {
        if let (Some(d), Some(last)) = (open, out.last_mut()) {
            if depth > d {
                last.1 += usize::from(gone);
                continue;
            }
        }
        open = gone.then_some(depth);
        if gone {
            out.push((i, 0));
        }
    }
    out
}

/// How an answer to `--since` is given: `last`, `since V, read at HH:MM`
/// when it was this agent's last read; why a `full` is; what is given
/// (what is useful, or of that role; at most `max`).
pub(super) struct Said<'a> {
    pub(super) last: Option<String>,
    pub(super) why_full: &'a str,
    pub(super) role: Option<&'a str>,
    pub(super) max: Option<usize>,
    pub(super) json: bool,
    pub(super) long: bool,
}

/// `changes V2` and a line each, or `full`, why, and the whole read shaped
/// as `tree` shapes it. JSON: `version`, `full` and either `changes` (each
/// a node with `change`) or `nodes`.
pub(super) fn print_since(since: Since, w: &str, s: &Said) {
    let (version, list, mut more) = match since {
        Since::Full(read) => {
            let read = read.shaped(s.role, s.max, w);
            if s.json {
                println!("{}", serde_json::json!({ "version": read.version, "full": true, "nodes": read.json_nodes(s.long), "more": read.more }));
                return;
            }
            if let Some(l) = &s.last {
                println!("{l}");
            }
            println!("full: {}", s.why_full);
            read.print(false, s.long);
            return;
        }
        Since::Changes { version, list, more } => (version, list, more),
    };
    let wanted = |c: &Change| match (s.role, c) {
        (Some(role), c) => c.rec().role == role,
        (None, Change::Went(..)) => true,
        (None, c) => c.rec().useful(),
    };
    let mut list: Vec<Change> = list.into_iter().filter(wanted).collect();
    if let Some(max) = s.max.filter(|m| list.len() > *m) {
        more += (list.len() - max) as u64;
        list.truncate(max);
    }
    if s.json {
        let all: Vec<serde_json::Value> = list
            .iter()
            .map(|c| match c {
                c => {
                    let mut j = c.rec().json(s.long);
                    j["change"] = c.sign().into();
                    match c {
                        Change::Went(_, k) => j["more"] = (*k).into(),
                        Change::Other(r, was) => j["was"] = was.to(r, s.long).into(),
                        Change::Came(_) => {}
                    }
                    j
                }
            })
            .collect();
        println!("{}", serde_json::json!({ "version": version, "full": false, "changes": all, "more": more }));
        return;
    }
    if let Some(l) = &s.last {
        println!("{l}");
    }
    println!("changes {version}");
    if list.is_empty() && more == 0 {
        println!("(nothing changed)");
    }
    for c in &list {
        println!("{}", c.line(s.long));
    }
    if more > 0 {
        println!("... {more} more not given: `find {w} --text T` for what you look for");
    }
}

/// Where a scene's surfaces are in its window's look coordinates: the
/// window's title (of several windows of one scene, the part that is this
/// one) and its menus' boxes from the root's corner, as `C` gives them.
#[derive(Default)]
pub(super) struct Place {
    pub(super) title: Option<String>,
    pub(super) popups: Vec<[i64; 4]>,
}

/// `describe json` as records. The window's own surface counts from the
/// root's corner as look coordinates do; a popup is placed at the menu of
/// the window that has its size; one that matches none is given without a
/// box. Panels and other windows are not this window's: counted, not given.
pub(super) fn scene_read(name: &str, json: &str, place: &Place) -> R<Read> {
    let parts: Vec<serde_json::Value> = serde_json::from_str(json.trim()).map_err(|_| Fail::from(format!("the scene answered no description: {}", json.trim())))?;
    let kind = |p: &serde_json::Value| p["kind"].as_str().unwrap_or_default().to_owned();
    let windows: Vec<usize> = (0..parts.len()).filter(|k| kind(&parts[*k]).starts_with("window")).collect();
    let mine = place.title.as_ref().and_then(|t| windows.iter().copied().find(|k| kind(&parts[*k]) == format!("window \u{ab}{t}\u{bb}"))).or(windows.first().copied());
    let mut used = vec![false; place.popups.len()];
    let (mut recs, mut more, mut others) = (Vec::new(), 0u64, Vec::new());
    fn count(nodes: &serde_json::Value) -> u64 {
        nodes.as_array().map_or(0, |a| a.iter().map(|n| 1 + count(&n["children"])).sum())
    }
    for (k, p) in parts.iter().enumerate() {
        let offset = if Some(k) == mine {
            Some((0, 0))
        } else if kind(p) == "popup" {
            let size = |i: usize| p["size"][i].as_i64().unwrap_or(-1);
            let found = place.popups.iter().enumerate().position(|(j, b)| !used[j] && (b[2] - size(0)).abs() <= 1 && (b[3] - size(1)).abs() <= 1);
            found.map(|j| {
                used[j] = true;
                (place.popups[j][0], place.popups[j][1])
            })
        } else {
            more += count(&p["nodes"]);
            others.push(format!("{} {}", kind(p), p["name"].as_str().unwrap_or_default()));
            continue;
        };
        scene_nodes(&p["nodes"], 0, offset, &mut recs);
    }
    let left = format!("they are in {}, not in this window", others.join(", "));
    Ok(Read { head: format!("scene {name}"), version: None, recs, folds: Vec::new(), more, left })
}

fn scene_nodes(nodes: &serde_json::Value, depth: usize, offset: Option<(i64, i64)>, out: &mut Vec<Rec>) {
    let Some(all) = nodes.as_array() else { return };
    for n in all {
        let s = |k: &str| n[k].as_str().unwrap_or_default().to_owned();
        let role = s("role");
        let name = s("name");
        let id = if role == "text" || name.is_empty() {
            "@-".to_owned()
        } else {
            Element::Name(name).said()
        };
        let b = |i: usize| n["box"][i].as_f64().unwrap_or(0.0).round() as i64;
        let mut states = Vec::new();
        if n["active"] == false {
            states.push("disabled");
        }
        if n["checked"] == true {
            states.push("checked");
        }
        if n["selected"] == true {
            states.push("selected");
        }
        if n["off_view"] == true {
            states.push("offscreen");
        }
        if n["covered_by"].is_string() {
            states.push("covered");
        }
        if n["agent"] == "no" {
            states.push("persons");
        }
        out.push(Rec {
            id,
            depth,
            role,
            name: s("label"),
            text: s("value"),
            rect: offset.map(|(x, y)| [b(0) + x, b(1) + y, b(2), b(3)]),
            states: states.into_iter().map(str::to_owned).collect(),
        });
        scene_nodes(&n["children"], depth + 1, offset, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// pleamar's `describe json` of a notes window with a menu open, and a panel.
    const DESCRIBED: &str = r#"[
      {"name":"main","kind":"window \u00abNotes\u00bb","size":[640,480],"scale":1.0,"nodes":[
        {"role":"field","label":"Search notes","box":[16,9,240,27],"name":"query","active":true,"agent":"yes","value":"inv"},
        {"role":"list","label":"","box":[16,60,608,120],"name":"list","active":true,"agent":"yes","children":[
          {"role":"item","label":"October invoice","box":[16,60,608,38],"name":"hit#r0","active":true,"agent":"yes","selected":true},
          {"role":"item","label":"Fifth note","box":[16,220,608,38],"name":"hit#r4","active":true,"agent":"yes","off_view":true}
        ]},
        {"role":"text","label":"No note open","box":[16,390,96,20]},
        {"role":"button","label":"Save","box":[544,428,80,36],"name":"save","active":false,"agent":"yes"},
        {"role":"button","label":"Delete","box":[360,428,80,36],"name":"delete","active":true,"agent":"no","covered_by":"menu"},
        {"role":"toggle","label":"Loud","box":[300,9,40,20],"name":"3","active":true,"agent":"yes","checked":true,"value":"loud"}
      ]},
      {"name":"menu","kind":"popup","size":[200,90],"scale":1.0,"nodes":[
        {"role":"item","label":"Rename","box":[0,0,200,30],"name":"rename","active":true,"agent":"yes"}
      ]},
      {"name":"bar","kind":"panel","size":[1920,32],"scale":1.0,"nodes":[
        {"role":"button","label":"Clock","box":[0,0,80,32],"name":"clock","active":true,"agent":"yes"}
      ]}
    ]"#;

    #[test]
    fn a_scene_is_read_as_records() {
        let place = Place { title: Some("Notes".into()), popups: vec![[300, 200, 200, 90]] };
        let read = scene_read("notes", DESCRIBED, &place).unwrap();
        let lines: Vec<String> = read.recs.iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.line(false))).collect();
        assert_eq!(
            lines,
            [
                "@query field \"Search notes\" text \"inv\" 16,9 240x27",
                "@list list 16,60 608x120",
                "  @hit#r0 item \"October invoice\" 16,60 608x38 selected",
                "  @hit#r4 item \"Fifth note\" 16,220 608x38 offscreen",
                "@- text \"No note open\" 16,390 96x20",
                "@save button \"Save\" 544,428 80x36 disabled",
                "@delete button \"Delete\" 360,428 80x36 covered,persons",
                "@=3 toggle \"Loud\" text \"loud\" 300,9 40x20 checked",
                "@rename item \"Rename\" 300,200 200x30",
            ]
        );
        assert_eq!((read.head.as_str(), read.version.as_deref(), read.more), ("scene notes", None, 1));
        assert!(read.left.contains("panel bar"));
        assert_eq!(read.recs[4].json(false)["id"], serde_json::Value::Null);
        // A menu whose place is not known: given without a box.
        let read = scene_read("notes", DESCRIBED, &Place::default()).unwrap();
        assert_eq!(read.recs.last().unwrap().line(false), "@rename item \"Rename\"");
        assert_eq!(at_point(&read.recs, 20.0, 70.0).iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["@list", "@hit#r0"]);
        assert!(scene_read("notes", "? the render does not answer", &place).is_err());
    }

    fn r(id: &str, depth: usize, role: &str, name: &str, states: &[&str]) -> Rec {
        Rec { id: id.into(), depth, role: role.into(), name: name.into(), text: String::new(), rect: Some([0, 0, 10, 10]), states: states.iter().map(|s| s.to_string()).collect() }
    }

    fn lines(read: &Read) -> Vec<String> {
        let mut out = Vec::new();
        for k in 0..=read.recs.len() {
            out.extend(read.folds.iter().filter(|f| f.at == k).map(|f| format!("{}... {} more {}", "  ".repeat(f.depth), f.count, f.role)));
            out.extend(read.recs.get(k).map(|r| format!("{}{}", "  ".repeat(r.depth), r.id)));
        }
        out
    }

    fn read_of(recs: Vec<Rec>) -> Read {
        Read { head: "tree 1".into(), version: Some("1".into()), recs, folds: Vec::new(), more: 0, left: String::new() }
    }

    #[test]
    fn a_tree_gives_what_is_useful_under_its_containers() {
        let recs = vec![
            r("@1", 0, "frame", "", &[]),
            r("@2", 1, "separator", "", &[]),
            r("@3", 1, "button", "Close", &[]),
            r("@4", 1, "table", "", &[]),
            r("@5", 2, "progress_bar", "", &[]),
            r("@6", 2, "label", "Size", &[]),
            r("@7", 1, "scroll_pane", "", &[]),
            r("@8", 2, "icon", "view-refresh-symbolic", &[]),
            r("@9", 1, "heading", "", &[]),
            r("@10", 1, "panel", "", &[]),
            r("@11", 2, "text", "", &["focusable", "editable"]),
        ];
        assert_eq!(lines(&read_of(recs.clone()).shaped(None, None, "1")), ["@1", "  @3", "  @4", "    @6", "  @9", "  @10", "    @11"]);
        // A role asked: those, under their containers, useful or not.
        assert_eq!(lines(&read_of(recs.clone()).shaped(Some("progress_bar"), None, "1")), ["@1", "  @4", "    @5"]);
        // At most so many given for themselves, with their containers; the rest counted.
        let read = read_of(recs.clone()).shaped(None, Some(2), "1");
        assert_eq!((lines(&read), read.more), (vec!["@1".to_owned(), "  @3".into(), "  @4".into(), "    @6".into()], 2));
        assert!(read.left.contains("--within"));
        let read = read_of(recs.clone()).shaped(Some("text"), Some(1), "1");
        assert_eq!((lines(&read), read.more), (vec!["@1".to_owned(), "  @10".into(), "    @11".into()], 0));
        // A scene: its named items are acted on; an unnamed text that says something is read.
        assert!(r("@list", 0, "list", "", &[]).useful());
        assert!(r("@-", 0, "text", "No note open", &[]).useful());
        assert!(!r("@-", 0, "text", "", &[]).useful());
    }

    #[test]
    fn runs_of_alike_rows_without_names_are_folded() {
        let mut recs = vec![r("@1", 0, "list", "Files", &[])];
        recs.extend((2..14).map(|k| r(&format!("@{k}"), 1, "list_item", "", &["focusable"])));
        // One that holds a name is not one of them.
        recs.push(r("@20", 1, "list_item", "", &["focusable"]));
        recs.push(r("@21", 2, "label", "notes.txt", &[]));
        recs.extend((30..40).map(|k| r(&format!("@{k}"), 1, "list_item", "", &["focusable"])));
        // Named rows never fold.
        recs.extend((50..70).map(|k| r(&format!("@{k}"), 1, "link", &format!("link {k}"), &["focusable"])));
        let (out, folds) = fold(recs);
        let read = Read { folds, ..read_of(out) };
        let got = lines(&read);
        assert_eq!(&got[..10], ["@1", "  @2", "  @3", "  @4", "  @5", "  @6", "  @7", "  @8", "  @9", "  ... 4 more list_item"]);
        assert_eq!(&got[10..13], ["  @20", "    @21", "  @30"]);
        assert!(got.contains(&"  ... 2 more list_item".to_owned()));
        assert_eq!(got.iter().filter(|l| l.contains("link") || l.starts_with("  @5") || l.starts_with("  @6")).count(), 20 + 2);
        // A run of eight, or one whose states differ, stays as it is.
        let eight: Vec<Rec> = (0..8).map(|k| r(&format!("@{k}"), 0, "table_cell", "", &[])).collect();
        assert!(fold(eight).1.is_empty());
        let mixed: Vec<Rec> = (0..12).map(|k| r(&format!("@{k}"), 0, "table_cell", "", if k == 5 { &["selected"] } else { &[] })).collect();
        assert!(fold(mixed).1.is_empty());
    }

    #[test]
    fn a_fold_is_a_line_and_a_json_node() {
        let recs: Vec<Rec> = (0..10).map(|k| r(&format!("@{k}"), 0, "table_cell", "", &["focusable"])).collect();
        let read = read_of(recs).shaped(None, None, "1");
        let j = read.json_nodes(false);
        assert_eq!(j.len(), 9);
        assert_eq!(j[8], serde_json::json!({ "depth": 0, "role": "table_cell", "folded": 2 }));
    }

    #[test]
    fn kept_records_come_back() {
        let one = Rec { text: "a \"quoted\" text".into(), ..r("@hit#r0", 2, "item", "October", &["selected"]) };
        assert_eq!(Rec::from_kept(&one.kept()), Some(one.clone()));
        assert_eq!(Rec::from_kept(&Rec { rect: None, ..one.clone() }.kept()).unwrap().rect, None);
    }

    #[test]
    fn a_scene_is_compared_by_name() {
        let place = Place { title: Some("Notes".into()), popups: vec![[300, 200, 200, 90]] };
        let then = scene_read("notes", DESCRIBED, &place).unwrap().recs;
        // Pressed: saved (Save no longer disabled), the menu closed, the empty text says another thing.
        let now_json = DESCRIBED.replace(r#""name":"save","active":false"#, r#""name":"save","active":true"#).replace("No note open", "October invoice open");
        let now = scene_read("notes", &now_json, &Place { title: Some("Notes".into()), popups: Vec::new() }).unwrap().recs;
        let now: Vec<Rec> = now.into_iter().filter(|r| r.id != "@rename").collect();
        let got: Vec<String> = scene_changes(&then, &now, false).iter().map(|c| c.line(false)).collect();
        assert_eq!(got, ["+ @- text \"October invoice open\" 16,390 96x20", "~ @save button \"Save\" 544,428 80x36 (-disabled)", "- @- \"No note open\"", "- @rename item \"Rename\" 300,200 200x30"]);
        assert!(scene_changes(&then, &then, true).is_empty());
        // Boxes only when asked.
        let moved: Vec<Rec> = then.iter().map(|r| Rec { rect: r.rect.map(|[x, y, w, h]| [x, y + 5, w, h]), ..r.clone() }).collect();
        assert!(scene_changes(&then, &moved, false).is_empty());
        assert_eq!(scene_changes(&then, &moved, true).len(), then.len());
        // Unnamed texts alike under one container are told apart by their order.
        let twice = vec![r("@list", 0, "list", "", &[]), r("@-", 1, "text", "-", &[]), r("@-", 1, "text", "-", &[])];
        assert_eq!(scene_keys(&twice), ["@list", "@list/-/", "@list/-/#2"]);
        assert_eq!(scene_changes(&twice, &twice[..2], false).iter().map(|c| c.line(false)).collect::<Vec<_>>(), ["- @- \"-\""]);
    }
}
