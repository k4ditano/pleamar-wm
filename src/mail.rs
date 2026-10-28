//! The mailbox of a named terminal (`link open --name`): tasks for the agent
//! in it, kept until it is free to take them —at rest, asking nothing, its
//! person not in the middle of writing to it— and handed to it then, one at a
//! time, in order. Whoever posts needs no window of their own (a shell's
//! finder, a routine, another agent), nor the terminal to be open yet: a
//! letter for a name waits for a terminal of that name.
//!
//! Each mailbox is a file of its own in `$XDG_STATE_HOME/pleamar-wm/mail`
//! (kept across sessions): the letters waiting, the one handed now, and the
//! last answers. The agents' watcher (term.rs) looks every second and leaves a
//! messenger (`link _mail`) with each letter it hands: it types it in, waits
//! for the agent to be done and keeps what it answered. Anything that changes
//! a mailbox says so in the scene's `mail.seq`; `link mail --json` reads them.

use serde_json::{Value, json};
use std::io::Write;

/// How many answers a mailbox keeps.
const KEPT: usize = 20;

fn dir() -> String {
    format!("{}/mail", crate::keep::dir())
}

/// The file of a name's mailbox: the name in letters, digits and `-` (its
/// own spelling is kept inside).
fn path(name: &str) -> String {
    let stem: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    format!("{}/{}.json", dir(), if stem.trim_matches('-').is_empty() { "-" } else { &stem })
}

/// A name as a terminal can have one: what `link open --name` takes.
pub(crate) fn valid(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(format!("«{name}» is no terminal's name: up to 64 characters, none of them a control one"));
    }
    Ok(())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A name's mailbox, changed by `f` with nobody else at it meanwhile (a lock
/// beside it), and written back whole —first beside it, then in its place,
/// readable only by its owner—.
pub(crate) fn with<R>(name: &str, f: impl FnOnce(&mut Value) -> R) -> Result<R, String> {
    let file = path(name);
    std::fs::create_dir_all(dir()).map_err(|e| format!("{}: {e}", dir()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(format!("{file}.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    let mut b = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    for (k, v) in [
        ("name", json!(name)),
        ("paused", json!(false)),
        ("next", json!(1)),
        ("waiting", json!([])),
        ("out", Value::Null),
        ("done", json!([])),
    ] {
        if b.get(k).is_none() {
            b[k] = v;
        }
    }
    let out = f(&mut b);
    let text = serde_json::to_string_pretty(&b).map_err(|e| e.to_string())?;
    let tmp = format!("{file}.new");
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut w = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        w.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Every mailbox, as kept.
pub(crate) fn all() -> Vec<Value> {
    let mut out: Vec<Value> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str::<Value>(&std::fs::read_to_string(e.path()).ok()?).ok())
        .filter(|b| b["name"].is_string())
        .collect();
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
}

/// That a mailbox changed, for the scene: `mail.seq`, a number it has not had
/// lately (it only has to change).
pub(crate) fn seq() -> String {
    (now_ms() % 10_000_000).to_string()
}

fn told() {
    crate::link::say_wm(&[format!("fact mail.seq {}", seq())]);
}

/// `link post NAME TEXT… [--from WHO]`: a letter in NAME's mailbox.
pub(crate) fn post(args: &[String]) -> Result<String, String> {
    let mut from = String::new();
    let mut words = Vec::new();
    let mut k = 0;
    while k < args.len() {
        if args[k] == "--from" {
            from = args.get(k + 1).cloned().ok_or("--from WHO: who it is from")?;
            k += 2;
            continue;
        }
        words.push(args[k].as_str());
        k += 1;
    }
    let name = words.first().copied().ok_or(USAGE)?.to_owned();
    valid(&name)?;
    let text = words[1..].join(" ");
    if text.trim().is_empty() {
        return Err(USAGE.into());
    }
    let from = from.replace(['\n', '\t', '[', ']'], " ").trim().to_owned();
    let (id, ahead) = with(&name, |b| {
        let id = b["next"].as_u64().unwrap_or(1);
        b["next"] = json!(id + 1);
        let ahead = b["waiting"].as_array().map_or(0, Vec::len) + usize::from(!b["out"].is_null());
        if let Some(w) = b["waiting"].as_array_mut() {
            w.push(json!({"id": id, "from": from, "text": text, "at": now_ms()}));
        }
        (id, ahead)
    })?;
    told();
    Ok(match ahead {
        0 => format!("letter {id} for «{name}»: handed to it as soon as it is free"),
        n => format!("letter {id} for «{name}», after {n} more"),
    })
}

/// `link mail [NAME] [--json]`, and what is done to one: `--cancel ID`,
/// `--first ID`, `--pause`, `--resume`, `--forget ID` (an answer kept).
pub(crate) fn mail(args: &[String]) -> Result<String, String> {
    let flag = |f: &str| args.iter().position(|a| a == f);
    let arg_of = |f: &str| flag(f).and_then(|k| args.get(k + 1)).and_then(|v| v.parse::<u64>().ok());
    let name = args.first().filter(|a| !a.starts_with("--")).cloned();
    let change = ["--cancel", "--first", "--pause", "--resume", "--forget"].iter().find(|f| flag(f).is_some());
    if let Some(&what) = change {
        let name = name.ok_or("NAME first: whose mailbox")?;
        if !std::path::Path::new(&path(&name)).exists() {
            return Err(format!("«{name}» has no mailbox"));
        }
        let id = arg_of(what);
        if matches!(what, "--cancel" | "--first" | "--forget") && id.is_none() {
            return Err(format!("{what} ID: a letter's number, as `link mail {name}` gives it"));
        }
        let said = with(&name, |b| {
            let list = if what == "--forget" { "done" } else { "waiting" };
            let at = b[list].as_array().and_then(|w| w.iter().position(|l| l["id"].as_u64() == id));
            match (what, at) {
                ("--pause", _) => {
                    b["paused"] = json!(true);
                    Ok(format!("«{name}»'s mailbox is paused: nothing more is handed until --resume"))
                }
                ("--resume", _) => {
                    b["paused"] = json!(false);
                    Ok(format!("«{name}»'s mailbox goes on"))
                }
                (_, None) if what != "--forget" && b["out"]["id"].as_u64() == id => Err(format!("letter {} was handed already: it is the agent's now", id.unwrap_or(0))),
                (_, None) => Err(format!("«{name}» has no letter {} {}", id.unwrap_or(0), if what == "--forget" { "answered" } else { "waiting" })),
                ("--first", Some(k)) => {
                    let l = b["waiting"].as_array_mut().map(|w| w.remove(k)).unwrap_or(Value::Null);
                    if let Some(w) = b["waiting"].as_array_mut() {
                        w.insert(0, l);
                    }
                    Ok(format!("letter {} goes first", id.unwrap_or(0)))
                }
                (_, Some(k)) => {
                    b[list].as_array_mut().map(|w| w.remove(k));
                    Ok(format!("letter {} is gone", id.unwrap_or(0)))
                }
            }
        })??;
        told();
        return Ok(said);
    }
    let boxes: Vec<Value> = all().into_iter().filter(|b| name.as_deref().is_none_or(|n| b["name"].as_str() == Some(n))).collect();
    if flag("--json").is_some() {
        return Ok(Value::Array(boxes).to_string());
    }
    if boxes.is_empty() {
        return Ok(match name {
            Some(n) => format!("«{n}» has no mailbox"),
            None => "no mailbox yet: `pleamar-wm link post NAME TEXT` starts one".into(),
        });
    }
    let line = |l: &Value| {
        format!(
            "{}{}: {}",
            l["id"],
            l["from"].as_str().filter(|f| !f.is_empty()).map(|f| format!(" (from {f})")).unwrap_or_default(),
            l["text"].as_str().unwrap_or("").replace('\n', " ").chars().take(70).collect::<String>()
        )
    };
    let mut out = String::new();
    for b in boxes {
        out.push_str(&format!("{}{}\n", b["name"].as_str().unwrap_or(""), if b["paused"] == true { " (paused)" } else { "" }));
        if !b["out"].is_null() {
            out.push_str(&format!("  at it now ({}): {}\n", b["out"]["state"].as_str().unwrap_or("handing"), line(&b["out"])));
        }
        for l in b["waiting"].as_array().into_iter().flatten() {
            out.push_str(&format!("  waiting {}\n", line(l)));
        }
        for l in b["done"].as_array().into_iter().flatten().take(3) {
            out.push_str(&format!("  {} {}\n", l["how"].as_str().unwrap_or("done"), line(l)));
        }
    }
    Ok(out.trim_end().to_owned())
}

/// The watcher's turn, every second (term.rs): for each named terminal whose
/// agent is free (`free`: at rest, asking nothing, nobody writing in it), the
/// next letter, if its mailbox has one and is not paused and has none out;
/// and a letter out whose messenger is gone (the session ended under it) is
/// put down as lost. True if a mailbox changed.
pub(crate) fn turn(named: &[(String, usize, i32, bool)], sockets: &str) -> bool {
    let mut changed = false;
    for (name, slot, layer, free) in named {
        if valid(name).is_err() || !std::path::Path::new(&path(name)).exists() {
            continue;
        }
        let r = with(name, |b| {
            let mut changed = false;
            if let Some(pid) = b["out"]["pid"].as_u64() {
                let alive = std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("_mail"));
                if alive {
                    return false;
                }
                let mut l = b["out"].take();
                l["how"] = json!("lost");
                l["ended"] = json!(now_ms());
                keep_done(b, l);
                changed = true;
            }
            if !*free || b["paused"] == true || b["waiting"].as_array().is_none_or(Vec::is_empty) {
                return changed;
            }
            let mut l = b["waiting"].as_array_mut().map(|w| w.remove(0)).unwrap_or(Value::Null);
            let mut c = std::process::Command::new("setsid");
            c.arg("-f").arg(crate::link::own_exe()).args(["link", "_mail", name, &slot.to_string(), &layer.to_string()]);
            c.env("PLEAMAR_SOCKETS", sockets);
            c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
            // `setsid -f` leaves at once; its child says its number first.
            let pid = c
                .spawn()
                .ok()
                .and_then(|ch| ch.wait_with_output().ok())
                .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next()?.trim().parse::<u64>().ok());
            match pid {
                Some(pid) => {
                    l["slot"] = json!(slot);
                    l["state"] = json!("handing");
                    l["pid"] = json!(pid);
                    b["out"] = l;
                }
                None => b["waiting"].as_array_mut().map(|w| w.insert(0, l)).unwrap_or(()),
            }
            true
        });
        changed |= r.unwrap_or(false);
    }
    changed
}

/// Whether the letter the terminal named so is at now came from a routine
/// (`--from "routine NAME"`): that routine's name. Read as it is, no lock: a
/// glance, not a change.
pub(crate) fn routine_of(name: &str) -> Option<String> {
    if valid(name).is_err() {
        return None;
    }
    let b: Value = serde_json::from_str(&std::fs::read_to_string(path(name)).ok()?).ok()?;
    b["out"]["from"].as_str()?.strip_prefix("routine ").map(|r| r.trim().to_owned()).filter(|r| !r.is_empty())
}

/// An answer kept, the last first, so many at most.
pub(crate) fn keep_done(b: &mut Value, l: Value) {
    if let Some(d) = b["done"].as_array_mut() {
        d.insert(0, l);
        d.truncate(KEPT);
    }
}

pub(crate) const USAGE: &str = "pleamar-wm link post NAME TEXT… [--from WHO]
                                  a letter for the agent in the terminal named NAME (`link open
                                  --name`): handed to it when it is free, after those before it
pleamar-wm link mail [NAME] [--json]
                                  the mailboxes: what waits, what each is at, its last answers
pleamar-wm link mail NAME --cancel ID | --first ID | --pause | --resume | --forget ID";
