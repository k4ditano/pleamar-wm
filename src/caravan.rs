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
    json!({ "companions": out, "pending": pending, "asks": asks(), "at": crate::mail::now_ms() }).to_string()
}

/// What a shell asks the person itself, beside what its agents ask on their
/// screens (Calima's cards: choose one, turn a routine on, let an agent's
/// hands use an app). The shell puts it (`link asks put ID JSON`: `name`, the
/// companion it is for; `kind`; `title`; `text`; 2 to 4 `options`; `scene` and
/// `event`, how to tell it) and drops it once answered or gone; the caravan
/// page shows it and tells the phone. Answered there, the choice is left as
/// `ID.answer` and the scene told (`pleamar --say SCENE "emit EVENT"`): the
/// shell takes it (`link asks take`) and does what its own button does. The
/// page only chooses; the shell decides what that means.
fn asks_dir() -> std::path::PathBuf {
    let term = crate::term::socket_dir();
    std::path::Path::new(&term).parent().map_or_else(|| std::path::PathBuf::from(&term), std::path::Path::to_path_buf).join("asks")
}

fn plain(s: &str, most: usize) -> bool {
    !s.is_empty() && s.len() <= most && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The asks there are, the oldest first.
pub(crate) fn asks() -> Vec<Value> {
    let mut out: Vec<Value> = std::fs::read_dir(asks_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().strip_suffix(".json")?.to_owned();
            let v: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some(json!({ "id": id, "name": v["name"], "kind": v["kind"], "title": v["title"], "text": v["text"], "options": v["options"], "at": v["at"] }))
        })
        .collect();
    out.sort_by_key(|a| a["at"].as_u64().unwrap_or(0));
    out
}

/// `link asks put ID JSON | drop ID | take`.
pub(crate) fn asks_cmd(rest: &[String]) -> Result<String, String> {
    let dir = asks_dir();
    let arg = |k: usize| rest.get(k).map_or("", String::as_str);
    match arg(0) {
        "put" => {
            let id = arg(1);
            if !plain(id, 40) {
                return Err("an id is letters, digits, - and _".into());
            }
            let v: Value = serde_json::from_str(arg(2)).map_err(|_| "the ask is a JSON object".to_owned())?;
            let text = |k: &str, n: usize| v[k].as_str().unwrap_or("").chars().take(n).collect::<String>();
            let options: Vec<String> = v["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str()).map(|o| o.chars().take(60).collect()).filter(|o: &String| !o.trim().is_empty()).take(4).collect();
            let (scene, event) = (text("scene", 60), text("event", 60));
            if options.len() < 2 || !plain(&scene, 60) || !plain(&event, 60) {
                return Err("an ask has 2 to 4 options, and the scene and event to tell".into());
            }
            let ask = json!({ "name": text("name", 80), "kind": text("kind", 20), "title": text("title", 200), "text": text("text", 600), "options": options, "scene": scene, "event": event, "at": crate::mail::now_ms() });
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{id}.json"));
            write_private(&path, &ask.to_string())?;
            let _ = std::fs::remove_file(dir.join(format!("{id}.answer")));
            Ok(id.to_owned())
        }
        "drop" => {
            let id = arg(1);
            if !plain(id, 40) {
                return Err("no such ask".into());
            }
            let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
            let _ = std::fs::remove_file(dir.join(format!("{id}.answer")));
            Ok(String::new())
        }
        "take" => {
            let mut got = serde_json::Map::new();
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(id) = name.strip_suffix(".answer") else { continue };
                if let Some(n) = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["n"].as_u64()) {
                    got.insert(id.to_owned(), json!(n));
                }
                let _ = std::fs::remove_file(e.path());
            }
            Ok(Value::Object(got).to_string())
        }
        _ => Err("link asks put ID JSON | drop ID | take".into()),
    }
}

/// Written whole (beside, then renamed) and for its owner alone.
fn write_private(path: &std::path::Path, text: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let new = path.with_extension("new");
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new).and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes())).map_err(|e| e.to_string())?;
    std::fs::rename(&new, path).map_err(|e| e.to_string())
}

/// The person chose on the page: the answer left for the shell, the ask gone
/// from the page, and the shell told.
fn answer_ask(id: &str, n: u64) -> Result<String, String> {
    let dir = asks_dir();
    let path = dir.join(format!("{id}.json"));
    if !plain(id, 40) {
        return Err("no such ask".into());
    }
    let ask: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("it is no longer asked")?;
    let options = ask["options"].as_array().map_or(0, Vec::len) as u64;
    if n >= options {
        return Err("no such option".into());
    }
    write_private(&dir.join(format!("{id}.answer")), &json!({ "n": n }).to_string())?;
    let _ = std::fs::remove_file(&path);
    let (scene, event) = (ask["scene"].as_str().unwrap_or(""), ask["event"].as_str().unwrap_or(""));
    let pleamar = std::env::var("PLEAMAR").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "pleamar".into());
    let _ = std::process::Command::new(pleamar).args(["--say", scene, &format!("emit {event}")]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    Ok(ask["options"][n as usize].as_str().unwrap_or("").to_owned())
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
            "files": files_in(&format!("{}\n{}", l["text"].as_str().unwrap_or(""), l["answer"].as_str().unwrap_or(""))),
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
        // An ask of the shell's own (a card): the option chosen, counted from 0.
        "card" => answer_ask(&s("id"), v["n"].as_u64().ok_or("which option?")?),
        // A letter, with what was sent up for it from the page (`files`, names in
        // the remote's inbox, as `/upload` gave them) and a voice note (`audio`):
        // transcribed here, the letter goes once it is (an agent cannot listen).
        "post" => {
            let text: String = s("text").trim().chars().take(8000).collect();
            let inbox = crate::remote_files::inbox();
            let inside = |rel: &str| -> Option<std::path::PathBuf> {
                let p = inbox.join(rel);
                (!rel.is_empty() && !rel.contains("..") && p.is_file()).then_some(p)
            };
            let files: Vec<std::path::PathBuf> = v["files"].as_array().into_iter().flatten().filter_map(|f| inside(f.as_str()?)).take(10).collect();
            let audio = v["audio"].as_str().and_then(inside);
            if text.is_empty() && files.is_empty() && audio.is_none() {
                return Err("nothing to send".into());
            }
            let attached = move |text: String| {
                if files.is_empty() {
                    return text;
                }
                let list: Vec<String> = files.iter().map(|f| format!("- {}", f.display())).collect();
                format!("{text}{}Attached (files on this computer):\n{}", if text.is_empty() { "" } else { "\n\n" }, list.join("\n"))
            };
            let Some(audio) = audio else {
                return crate::mail::post(&[name, attached(text)]);
            };
            let hearing = std::thread::spawn(move || {
                let heard = transcribe(&audio);
                let note = match &heard {
                    Ok(t) => format!("[Voice note, transcribed] {t}"),
                    Err(e) => format!("[Voice note: not transcribed — {e}]"),
                };
                let body = [note, text].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
                let body = format!("{}\n\n(The audio: {})", attached(body), audio.display());
                if let Err(e) = crate::mail::post(&[name, body]) {
                    eprintln!("remote · a voice note was not posted: {e}");
                }
            });
            HEARING.lock().unwrap().push(hearing);
            Ok("transcribing".into())
        }
        "gate" => crate::gate::answer(&[s("id"), s("verdict"), "--by".to_owned(), "phone".to_owned()]),
        "answer" => crate::agents::act(&["answer".to_owned(), slot()?, num("n"), "--key".to_owned(), s("key")]),
        "stop" => crate::agents::act(&["stop".to_owned(), slot()?]),
        "pause" | "resume" => crate::mail::mail(&[name, format!("--{}", s("do"))]),
        "cancel" | "first" => crate::mail::mail(&[name, format!("--{}", s("do")), num("id")]),
        other => Err(format!("«{other}»: nothing to do")),
    }
}

/// What a file is, by its extension: its media type, and how a page shows it.
fn kind_of(path: &str) -> (&'static str, &'static str) {
    let lower = path.to_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "png" => ("image/png", "image"),
        "jpg" | "jpeg" => ("image/jpeg", "image"),
        "webp" => ("image/webp", "image"),
        "gif" => ("image/gif", "image"),
        "webm" | "weba" => ("audio/webm", "audio"),
        "ogg" | "oga" | "opus" => ("audio/ogg", "audio"),
        "m4a" | "aac" => ("audio/mp4", "audio"),
        "mp3" => ("audio/mpeg", "audio"),
        "wav" => ("audio/wav", "audio"),
        "mp4" | "m4v" => ("video/mp4", "video"),
        "mov" => ("video/quicktime", "video"),
        "pdf" => ("application/pdf", "file"),
        "txt" | "md" | "log" | "csv" | "json" => ("text/plain; charset=utf-8", "file"),
        _ => ("application/octet-stream", "file"),
    }
}

/// The files a letter or an answer names that are there: absolute paths
/// (`/…/fondo.png`, inside a Markdown link or `code` too), each once, at
/// most eight. For a page to show them (images, audio) or offer them.
pub(crate) fn files_in(text: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let bytes = text.as_bytes();
    let mut k = 0;
    while k < bytes.len() && out.len() < 8 {
        let starts = bytes[k] == b'/' && (k == 0 || matches!(bytes[k - 1], b' ' | b'\n' | b'\t' | b'(' | b'`' | b'"' | b'\'' | b'[' | b'<' | b':' | b'*'));
        if !starts {
            k += 1;
            continue;
        }
        // Up to each space or mark after it, to the end of its line (a name may have
        // spaces: «Nota de voz 2026-10-29.webm»): of the first 16, the longest that is a file.
        let line = text[k..].find('\n').map_or(text.len(), |e| k + e);
        let mut cuts: Vec<usize> = text[k..line].char_indices().filter(|&(_, c)| c.is_whitespace() || matches!(c, ')' | ']' | '`' | '"' | '\'' | '<' | '>' | '*')).map(|(i, _)| k + i).collect();
        cuts.push(line);
        cuts.truncate(16);
        let found = cuts.iter().rev().find_map(|&end| {
            let path = text[k..end].trim_end_matches(['.', ',', ';', ':']);
            let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
            Some((path, meta, end))
        });
        let Some((path, meta, end)) = found else {
            k = cuts.first().copied().unwrap_or(line).max(k + 1);
            continue;
        };
        k = end.max(k + 1);
        if path.len() < 3 || path.contains("/../") || out.iter().any(|f| f["path"] == path) {
            continue;
        }
        let (_, kind) = kind_of(path);
        out.push(json!({ "path": path, "name": path.rsplit('/').next().unwrap_or(path), "size": meta.len(), "kind": kind }));
    }
    out
}

/// A file a letter or an answer of some mailbox names, for the caravan page
/// (`/caravan/file?path=`): only those, of 2 GB at most, and nothing else of
/// the disk. Its path, its media type, how a page shows it and its size.
pub(crate) fn attachment(path: &str) -> Result<(std::path::PathBuf, &'static str, &'static str, u64), String> {
    if !path.starts_with('/') || path.contains("/../") {
        return Err("not a path".into());
    }
    let said = crate::mail::all().iter().any(|b| {
        let has = |l: &Value| l["answer"].as_str().is_some_and(|a| a.contains(path)) || l["text"].as_str().is_some_and(|t| t.contains(path));
        b["done"].as_array().into_iter().flatten().any(has) || b["waiting"].as_array().into_iter().flatten().any(has) || has(&b["out"])
    });
    if !said {
        return Err("no letter names it".into());
    }
    let meta = std::fs::metadata(path).map_err(|_| "not there".to_owned())?;
    if !meta.is_file() || meta.len() > 2 << 30 {
        return Err("not a file it gives".into());
    }
    let (mime, kind) = kind_of(path);
    Ok((std::path::PathBuf::from(path), mime, kind, meta.len()))
}

/// The voice notes still being transcribed: the page's server lives on, but a
/// command (`link caravan do`) waits for them before it ends ([`heard`]).
static HEARING: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn heard() {
    let all: Vec<_> = HEARING.lock().unwrap().drain(..).collect();
    for h in all {
        let _ = h.join();
    }
}

/// A voice note as text: converted to what a transcriber takes (16 kHz mono
/// WAV, with ffmpeg) and given to the first there is —voxtype, whisper.cpp's
/// whisper-cli—, here, never to a service elsewhere. What it said, or why not.
fn transcribe(audio: &std::path::Path) -> Result<String, String> {
    let have = |cmd: &str| std::process::Command::new("sh").args(["-c", &format!("command -v {cmd}")]).output().is_ok_and(|o| o.status.success());
    let wav = std::env::temp_dir().join(format!("pleamar-voice-{}-{}.wav", std::process::id(), crate::mail::now_ms()));
    let made = std::process::Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-i"]).arg(audio).args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"]).arg(&wav).status();
    if !made.is_ok_and(|s| s.success()) {
        return Err("ffmpeg could not read it".into());
    }
    let out = if have("voxtype") {
        std::process::Command::new("voxtype").args(["-q", "transcribe"]).arg(&wav).output()
    } else if have("whisper-cli") {
        std::process::Command::new("whisper-cli").args(["-nt", "-np", "-l", "auto", "-f"]).arg(&wav).output()
    } else {
        let _ = std::fs::remove_file(&wav);
        return Err("no transcriber here (voxtype or whisper-cli)".into());
    };
    let _ = std::fs::remove_file(&wav);
    let out = out.map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    if text.is_empty() { Err("it heard nothing".into()) } else { Ok(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_a_letter_names_are_those_that_are_there() {
        let dir = std::env::temp_dir().join(format!("pleamar-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (pic, doc) = (dir.join("fondo.png"), dir.join("informe final.md"));
        std::fs::write(&pic, b"png").unwrap();
        std::fs::write(&doc, b"# hi").unwrap();
        let text = format!(
            "Hecho: [fondo]({}). Lo demás en `{}`, y {}, que no está; {}/../fondo.png tampoco. Otra vez {}.",
            pic.display(),
            dir.join("nada.txt").display(),
            dir.join("no.pdf").display(),
            dir.display(),
            pic.display()
        );
        let files = files_in(&text);
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0]["name"], "fondo.png");
        assert_eq!(files[0]["kind"], "image");
        assert_eq!(files[0]["size"], 3);
        // A folder is not a file; a name may have spaces (in a list, a link or a sentence).
        let spaced = files_in(&format!("en {} y\n- {}\n(The audio: {})", dir.display(), doc.display(), doc.display()));
        assert_eq!(spaced.len(), 1, "{spaced:?}");
        assert_eq!(spaced[0]["name"], "informe final.md");
        assert_eq!(files_in(&format!("Mira {} y dime", doc.display()))[0]["name"], "informe final.md");
        // And one in a long sentence, with many words after it.
        let long = format!("Hecho: [fondo]({}). {}", pic.display(), "una palabra más ".repeat(30));
        assert_eq!(files_in(&long).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn how_a_file_is_shown_comes_from_its_extension() {
        assert_eq!(kind_of("/a/Nota de voz.WEBM"), ("audio/webm", "audio"));
        assert_eq!(kind_of("/a/b.m4a").1, "audio");
        assert_eq!(kind_of("/a/b.jpeg").1, "image");
        assert_eq!(kind_of("/a/b.mov").1, "video");
        assert_eq!(kind_of("/a/b.pdf"), ("application/pdf", "file"));
        assert_eq!(kind_of("/a/sin-extension").0, "application/octet-stream");
    }

    #[test]
    fn only_a_path_a_letter_names_is_given() {
        assert!(attachment("relative/path").is_err());
        assert!(attachment("/tmp/../etc/passwd").is_err());
        assert!(attachment("/etc/hostname").is_err(), "no letter names it");
    }
}
