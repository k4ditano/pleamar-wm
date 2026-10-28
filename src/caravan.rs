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

/// The terminal named `name`: its layer and what the scene knows of it.
fn terminal(name: &str) -> Option<(i32, std::collections::BTreeMap<String, String>)> {
    crate::term::layers().into_iter().find(|(_, i)| i.get("name").map(String::as_str) == Some(name))
}

/// Everyone the caravan page shows: each mailbox and each named terminal, with
/// its card, how it is, what it asks on its screen, and its mailbox in short;
/// and what waits at the gate. One JSON, for `/caravan/state`.
pub(crate) fn companions() -> String {
    let boxes = crate::mail::all();
    let layers = crate::term::layers();
    let agents = crate::agents::listed();
    let mut names: Vec<String> = boxes.iter().filter_map(|b| b["name"].as_str().map(str::to_owned)).collect();
    for (_, i) in &layers {
        if let Some(n) = i.get("name").filter(|n| !n.is_empty() && !names.contains(n)) {
            names.push(n.clone());
        }
    }
    let first_line = |t: &str| t.lines().map(|l| l.trim().trim_start_matches(['●', '⏺', '•']).trim()).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect::<String>();
    let mut out: Vec<Value> = names
        .iter()
        .map(|name| {
            let b = boxes.iter().find(|b| b["name"].as_str() == Some(name.as_str())).cloned().unwrap_or(Value::Null);
            let term = layers.iter().find(|(_, i)| i.get("name") == Some(name));
            let agent = term.and_then(|(id, _)| agents.iter().find(|a| a["layer"].as_i64() == Some(i64::from(*id))));
            let hook = term.and_then(|(_, i)| i.get("hook")).map_or("", String::as_str);
            let state = match (term, agent) {
                (None, _) => "away",
                (_, Some(a)) if !a["question"].is_null() || hook == "asking" => "asking",
                (_, Some(a)) if a["state"] == "working" => "working",
                (_, Some(_)) => "free",
                (Some(_), None) => "shell",
            };
            let last = b["done"].as_array().and_then(|d| d.first());
            json!({
                "name": name,
                "card": b["card"],
                "state": state,
                "agent": agent.map_or(Value::Null, |a| a["agent"].clone()),
                "question": agent.map_or(Value::Null, |a| a["question"].clone()),
                "detail": agent.map_or(Value::Null, |a| a["detail"].clone()),
                "paused": b["paused"] == true,
                "waiting": b["waiting"].as_array().map_or(0, Vec::len),
                "doing": b["out"]["text"].as_str().map(first_line),
                "last": last.and_then(|l| l["answer"].as_str()).map(first_line),
                "last_at": last.map_or(Value::Null, |l| l["ended"].clone()),
                "how": last.map_or(Value::Null, |l| l["how"].clone()),
            })
        })
        .collect();
    let title = |c: &Value| c["card"]["title"].as_str().filter(|t| !t.is_empty()).or(c["name"].as_str()).unwrap_or("").to_lowercase();
    out.sort_by_key(title);
    let pending: Vec<Value> = crate::gate::pending()
        .iter()
        .map(|p| json!({ "id": p["id"], "name": p["name"], "agent": p["agent"], "tool": p["tool"], "what": p["what"].as_str().map(|w| w.chars().take(600).collect::<String>()), "routine": p["routine"] }))
        .collect();
    json!({ "companions": out, "pending": pending, "at": crate::mail::now_ms() }).to_string()
}

/// One name's conversation, for `/caravan/talk`: its letters as they came, the
/// oldest first —those answered (as many as its mailbox keeps), the one it is
/// at, those waiting— each with what it answered.
pub(crate) fn talk(name: &str) -> String {
    let b = crate::mail::all().into_iter().find(|b| b["name"].as_str() == Some(name)).unwrap_or(Value::Null);
    let cut = |v: &Value, n: usize| v.as_str().map(|t| t.chars().take(n).collect::<String>());
    let letter = |l: &Value, state: &str| {
        json!({
            "id": l["id"], "from": l["from"], "text": cut(&l["text"], 8000), "at": l["at"], "ended": l["ended"],
            "answer": cut(&l["answer"], 24000), "how": l["how"], "state": state,
        })
    };
    let mut letters: Vec<Value> = b["done"].as_array().into_iter().flatten().rev().map(|l| letter(l, "done")).collect();
    if !b["out"].is_null() {
        letters.push(letter(&b["out"], b["out"]["state"].as_str().unwrap_or("handing")));
    }
    letters.extend(b["waiting"].as_array().into_iter().flatten().map(|l| letter(l, "waiting")));
    json!({ "name": name, "card": b["card"], "paused": b["paused"] == true, "letters": letters }).to_string()
}

/// The last lines on a name's terminal, as text (`/caravan/screen`).
pub(crate) fn screen(name: &str) -> String {
    let text = terminal(name).map(|(id, _)| crate::link::read(id, 80)).unwrap_or_default();
    json!({ "name": name, "up": terminal(name).is_some(), "text": text.trim_end() }).to_string()
}

/// What the caravan page does (`POST /caravan/do`, a JSON with `do`): a
/// letter for a name; the gate's answer; an answer to what an agent asks on
/// its screen, only to the question shown (its key); Ctrl+C to it; and its
/// mailbox held, let go, a letter first or gone.
pub(crate) fn act(body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "not understood".to_owned())?;
    let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
    let name = s("name");
    let slot = || -> Result<String, String> {
        let (id, _) = terminal(&name).ok_or_else(|| format!("«{name}» has no terminal open"))?;
        crate::agents::listed()
            .iter()
            .find(|a| a["layer"].as_i64() == Some(i64::from(id)))
            .and_then(|a| a["slot"].as_u64())
            .map(|n| n.to_string())
            .ok_or_else(|| format!("no agent runs in «{name}»"))
    };
    let num = |k: &str| v[k].as_u64().map(|n| n.to_string()).unwrap_or_default();
    match s("do").as_str() {
        "post" => {
            let text: String = s("text").trim().chars().take(8000).collect();
            if text.is_empty() {
                return Err("nothing to send".into());
            }
            crate::mail::post(&[name, text])
        }
        "gate" => crate::gate::answer(&[s("id"), s("verdict"), "--by".to_owned(), "phone".to_owned()]),
        "answer" => crate::agents::act(&["answer".to_owned(), slot()?, num("n"), "--key".to_owned(), s("key")]),
        "stop" => crate::agents::act(&["stop".to_owned(), slot()?]),
        "pause" | "resume" => crate::mail::mail(&[name, format!("--{}", s("do"))]),
        "cancel" | "first" => crate::mail::mail(&[name, format!("--{}", s("do")), num("id")]),
        other => Err(format!("«{other}»: nothing to do")),
    }
}
