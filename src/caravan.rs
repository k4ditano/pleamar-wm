//! The caravan for the remote page (remote.rs): what a phone sees of the
//! agents at home and does with them —what waits at the gate (gate.rs), each
//! named terminal and its mailbox (mail.rs)— without the desktop's picture.

use serde_json::{json, Value};

/// What waits for the person, the named terminals and their mailboxes, as one JSON.
pub(crate) fn state() -> String {
    let mut named: Vec<Value> = Vec::new();
    for (_, i) in crate::term::layers() {
        let Some(name) = i.get("name").filter(|n| !n.is_empty()) else { continue };
        if named.iter().any(|n| n["name"] == name.as_str()) {
            continue;
        }
        let hook = i.get("hook").map(String::as_str).unwrap_or("");
        let state = if hook == "asking" {
            "asking"
        } else if hook == "working" || (hook.is_empty() && i.get("state").map(String::as_str) == Some("working")) {
            "working"
        } else if i.get("agent").is_some_and(|a| !a.is_empty()) {
            "free"
        } else {
            "shell"
        };
        named.push(json!({ "name": name, "agent": i.get("agent").cloned().unwrap_or_default(), "state": state }));
    }
    let first_line = |t: &str| t.lines().map(|l| l.trim().trim_start_matches(['●', '⏺', '•']).trim()).find(|l| !l.is_empty()).unwrap_or("").chars().take(160).collect::<String>();
    let boxes: Vec<Value> = crate::mail::all()
        .iter()
        .map(|b| {
            let last = b["done"].as_array().and_then(|d| d.first());
            json!({
                "name": b["name"], "paused": b["paused"],
                "waiting": b["waiting"].as_array().map_or(0, Vec::len),
                "out": b["out"]["state"], "doing": b["out"]["text"].as_str().map(first_line),
                "last": last.and_then(|l| l["answer"].as_str()).map(first_line), "how": last.map(|l| l["how"].clone()),
            })
        })
        .collect();
    let pending: Vec<Value> = crate::gate::pending()
        .iter()
        .map(|p| json!({ "id": p["id"], "who": p["name"].as_str().filter(|n| !n.is_empty()).or(p["agent"].as_str()), "tool": p["tool"], "what": p["what"].as_str().map(|w| w.chars().take(300).collect::<String>()), "routine": p["routine"] }))
        .collect();
    json!({ "pending": pending, "named": named, "boxes": boxes }).to_string()
}

/// `gate ID allow|deny` from the phone.
pub(crate) fn answer(rest: &str) -> String {
    let mut w = rest.split_whitespace();
    let (id, verdict) = (w.next().unwrap_or(""), w.next().unwrap_or(""));
    match crate::gate::answer(&[id.to_owned(), verdict.to_owned(), "--by".to_owned(), "phone".to_owned()]) {
        Ok(_) => format!("{} {}", if verdict == "allow" { "Allowed" } else { "Denied" }, id),
        Err(e) => e,
    }
}

/// `post NAME TEXT…` from the phone: a letter in that mailbox, from the person.
pub(crate) fn post(rest: &str) -> String {
    let (name, text) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
    if name.is_empty() || text.trim().is_empty() {
        return "a name and a task".into();
    }
    match crate::mail::post(&[name.to_owned(), text.trim().to_owned()]) {
        Ok(said) => said,
        Err(e) => e,
    }
}
