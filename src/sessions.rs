//! Each named terminal's agent session, kept across sessions, and its restart.
//!
//! An agent started in a named terminal (`link open --name`) is told of by its
//! hooks (`link hook`): the conversation it is in now (Claude Code's
//! `session_id`, a new one after `/clear` or `/resume`) is kept by that name in
//! `$XDG_STATE_HOME/pleamar-wm/sessions/NAME.json`, so that a shell can start
//! it again in that same conversation —not «the last of this folder», which may
//! be another's that works there too—.
//!
//! Restarting: what an agent has loaded when it starts (MCP servers, skills,
//! settings) is only loaded again by starting it again. A script that starts
//! it in a loop asks `link session NAME --loop` for the session to resume (and
//! says so: it can be restarted), runs the agent in the foreground (`set -m`,
//! so the terminal still sees the agent), and after it ends asks `link session
//! NAME --again`: true only if a restart was asked. `link restart NAME [--then
//! TEXT]` asks it: ends the agent (SIGTERM; its conversation is on disk), and
//! once it is gone leaves TEXT as a letter in its mailbox, handed to it when
//! it is back and free. The agent itself asks it through `link_restart_me`.
//! `--fresh` starts it in a new conversation instead (`link session` says `-`
//! until it has one): a conversation grown long costs its whole length again
//! at every turn.
//!
//! After the agent ends, the script says so (`link session NAME --ended T RC`,
//! T when it was started): one that ends within seconds, not asked to, could
//! not start —a CLI that is not there, a conversation it did not find…—. That
//! is kept (`failed`, with the last lines of its screen), told to the person
//! (a notification, the phone) and to whoever left it a letter, and a shell
//! does not start it again on its own for a while.
//!
//! Codex tells no hooks its conversation: it is found in the file it keeps open
//! (`~/.codex/sessions/…/rollout-…-UUID.jsonl`), looked at while it runs.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// An agent that ends within this many seconds, not asked to, could not start.
const QUICK: u64 = 15;
/// How long a failed start is told, and keeps a shell from starting it again.
pub(crate) const FAILED_FOR_MS: u64 = 30 * 60 * 1000;
/// Above this many tokens in its context, the page says it grew long.
pub(crate) const LONG_CONTEXT: u64 = 120_000;

fn dir() -> PathBuf {
    let state = std::env::var("XDG_STATE_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{}/.local/state", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(state).join("pleamar-wm").join("sessions")
}

fn plain(name: &str) -> bool {
    !name.is_empty() && name.len() <= 80 && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.')) && !name.starts_with('.')
}

fn read(name: &str) -> Value {
    std::fs::read_to_string(dir().join(format!("{name}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}))
}

fn write(name: &str, v: &Value) {
    use std::os::unix::fs::OpenOptionsExt;
    let d = dir();
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let path = d.join(format!("{name}.json"));
    let new = d.join(format!("{name}.json.new"));
    let ok = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new).and_then(|mut f| std::io::Write::write_all(&mut f, v.to_string().as_bytes()));
    if ok.is_ok() {
        let _ = std::fs::rename(new, path);
    }
}

fn change(name: &str, f: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let mut v = read(name);
    if !v.is_object() {
        v = json!({});
    }
    if let Some(m) = v.as_object_mut() {
        f(m);
    }
    write(name, &v);
}

/// From a hook, in a named terminal: the conversation it is in now.
pub(crate) fn heard(agent: &str, hook: &Value) {
    let Some(name) = std::env::var("PLEAMAR_NAME").ok().filter(|n| plain(n)) else { return };
    let Some(session) = hook.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 80) else { return };
    let transcript = hook.get("transcript_path").and_then(Value::as_str).unwrap_or("").to_owned();
    record(&name, agent, session, &transcript);
}

fn short(session: &str) -> &str {
    session.get(..8).unwrap_or(session)
}

/// That NAME's agent is in this conversation now: kept, and what was kept
/// against it (a failed start, a new conversation asked for) is past.
fn record(name: &str, agent: &str, session: &str, transcript: &str) {
    let was = read(name);
    if was["session"].as_str() == Some(session) && was["agent"].as_str() == Some(agent) && was.get("failed").is_none() && was.get("fresh").is_none() {
        return;
    }
    if was["session"].as_str() != Some(session) {
        crate::journal::note(name, "conversation", &format!("{agent}, {}", short(session)));
    }
    change(name, |m| {
        let new = m.get("session").and_then(Value::as_str) != Some(session);
        m.insert("agent".into(), json!(agent));
        m.insert("session".into(), json!(session));
        m.insert("transcript".into(), json!(transcript));
        m.insert("at".into(), json!(crate::mail::now_ms()));
        m.remove("failed");
        if new {
            m.remove("fresh");
        }
    });
}

/// The watcher's look at a named terminal whose agent is Codex: its
/// conversation, from the file it keeps open.
pub(crate) fn look_codex(name: &str, layer: i32) {
    if !plain(name) {
        return;
    }
    let Some(pid) = agent_pid(layer) else { return };
    let Some(path) = std::fs::read_dir(format!("/proc/{pid}/fd")).into_iter().flatten().flatten().filter_map(|e| std::fs::read_link(e.path()).ok()).find(|p| {
        p.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.starts_with("rollout-") && f.ends_with(".jsonl")) && p.to_string_lossy().contains("/sessions/")
    }) else {
        return;
    };
    // rollout-2026-10-29T07-16-08-01a1864d-3102-7af3-a3c7-1c5c962ef300.jsonl: the UUID is its last 36.
    let stem = path.file_stem().and_then(|f| f.to_str()).unwrap_or("");
    let Some(id) = stem.get(stem.len().saturating_sub(36)..).filter(|id| id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')) else { return };
    record(name, "codex", id, &path.to_string_lossy());
}

/// A failed start of NAME's agent, if it is recent: `{at, text, rc}`.
pub(crate) fn failed(name: &str) -> Option<Value> {
    let v = read(name);
    let f = v.get("failed")?.clone();
    (crate::mail::now_ms().saturating_sub(f["at"].as_u64().unwrap_or(0)) < FAILED_FOR_MS).then_some(f)
}

/// How long NAME's conversation is now, in tokens, and the most its model
/// takes (0: not known), from its last answer in its record.
pub(crate) fn context(name: &str) -> Option<(u64, u64)> {
    let v = read(name);
    let t = v["transcript"].as_str().filter(|t| !t.is_empty())?;
    // Looked at again only when its record changed (the live page asks often).
    type Seen = std::collections::HashMap<String, (std::time::SystemTime, u64, Option<(u64, u64)>)>;
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    let meta = std::fs::metadata(t).ok()?;
    let mark = (meta.modified().ok()?, meta.len());
    let mut seen = SEEN.lock().ok()?;
    let seen = seen.get_or_insert_with(Default::default);
    if let Some((_, _, got)) = seen.get(t).filter(|(m, l, _)| (*m, *l) == mark) {
        return *got;
    }
    let got = tokens_in(std::path::Path::new(t), v["agent"].as_str().unwrap_or(""));
    seen.insert(t.to_owned(), (mark.0, mark.1, got));
    got
}

fn tokens_in(path: &std::path::Path, agent: &str) -> Option<(u64, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    // Its end is enough: the last answer is there.
    let from = len.saturating_sub(512 * 1024);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    for line in text.lines().rev() {
        if agent == "codex" {
            if !line.contains("\"token_count\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let info = &v["payload"]["info"];
            if let Some(n) = info["last_token_usage"]["input_tokens"].as_u64() {
                return Some((n, info["model_context_window"].as_u64().unwrap_or(0)));
            }
        } else {
            if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let u = &v["message"]["usage"];
            if u.is_object() && !v["isSidechain"].as_bool().unwrap_or(false) {
                let n = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum::<u64>();
                let model = v["message"]["model"].as_str().unwrap_or("");
                return Some((n, if model.contains("[1m]") { 1_000_000 } else { 200_000 }));
            }
        }
    }
    None
}

/// After the agent ended (its script): how long it ran, and what then. Within
/// [`QUICK`] seconds, with an exit of its own (not a signal), not asked to: it
/// could not start.
fn ended(name: &str, started: u64, rc: &str) -> Result<String, String> {
    let now_s = crate::mail::now_ms() / 1000;
    let ran = now_s.saturating_sub(started);
    let v = read(name);
    if v["restart"] == true {
        crate::journal::note(name, "restarting", &format!("ended after {ran} s to start again"));
        return Ok(String::new());
    }
    // (Ended by a signal —killed, or its terminal closed— is no start that failed.)
    let signalled = rc.parse::<i32>().is_ok_and(|c| c >= 128);
    if ran >= QUICK || started == 0 || signalled {
        crate::journal::note(name, "exit", &format!("after {} (exit {rc})", span(ran)));
        change(name, |m| {
            m.remove("failed");
        });
        return Ok(String::new());
    }
    // What it said as it went: the last lines of its screen.
    let screen = crate::term::layers()
        .into_iter()
        .find(|(_, i)| i.get("name").map(String::as_str) == Some(name))
        .map(|(id, _)| crate::link::read(id, 30))
        .unwrap_or_default();
    let said: Vec<&str> = screen.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('❯') && !l.starts_with('$') && !l.contains("caravana-") && !l.chars().all(|c| !c.is_alphanumeric())).collect();
    let text = said[said.len().saturating_sub(3)..].join(" · ");
    let text = if text.is_empty() { format!("it ended at once (exit {rc})") } else { text.chars().take(300).collect() };
    let at = crate::mail::now_ms();
    change(name, |m| {
        m.insert("failed".into(), json!({ "at": at, "text": text, "rc": rc }));
    });
    crate::journal::note(name, "failed", &text);
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "pleamar", "-u", "critical", &format!("{name} could not start"), &text])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut c| std::thread::spawn(move || c.wait()));
    // Whoever left it a letter, if that is a companion: once, now.
    let senders: Vec<String> = crate::mail::all()
        .iter()
        .filter(|b| b["name"].as_str() == Some(name))
        .flat_map(|b| b["waiting"].as_array().cloned().unwrap_or_default())
        .filter_map(|l| l["from"].as_str().map(str::to_owned))
        .filter(|f| plain(f) && f != name && read(f)["loops"] == true)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for who in senders {
        let _ = crate::mail::post(&[who, format!("{name} could not start ({text}). Your letter waits in its mailbox until it is started again; do not send it again."), "--from".into(), "pleamar".into(), "--no-reply".into()]);
    }
    Ok(format!("«{name}» could not start: {text}"))
}

fn span(s: u64) -> String {
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

/// `link session NAME [--loop | --again | --json | --ended T RC]`.
pub(crate) fn session(rest: &[String]) -> Result<String, String> {
    let name = rest.first().map_or("", String::as_str);
    if !plain(name) {
        return Err("link session NAME [--loop | --again | --json | --ended T RC]".into());
    }
    match rest.get(1).map(String::as_str) {
        Some("--ended") => ended(name, rest.get(2).and_then(|t| t.parse().ok()).unwrap_or(0), rest.get(3).map_or("?", String::as_str)),
        // After the agent ended: whether it was to start again (once).
        Some("--again") => {
            let v = read(name);
            if v["restart"] != true {
                return Err("no restart asked".into());
            }
            change(name, |m| {
                m.remove("restart");
            });
            Ok(String::new())
        }
        Some("--json") => Ok(read(name).to_string()),
        // The session to resume, if its conversation is still on disk; with
        // `--loop`, the script that asks can start it again.
        loops => {
            if loops == Some("--loop") {
                change(name, |m| {
                    m.insert("loops".into(), json!(true));
                    m.remove("restart");
                });
            }
            let v = read(name);
            // A new conversation asked for: none to resume, until it has one.
            if v["fresh"] == true {
                return Ok("-".into());
            }
            let t = v["transcript"].as_str().unwrap_or("");
            let there = t.is_empty() || std::path::Path::new(t).is_file();
            // Claude Code finds a conversation by the folder it was begun in (its
            // transcript's folder is that path, its marks as dashes): in another, none.
            let here = std::env::current_dir().map(|d| d.display().to_string().chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' }).collect::<String>()).unwrap_or_default();
            let same = v["agent"] != "claude" || t.is_empty() || t.contains(&format!("/projects/{here}/"));
            Ok(v["session"].as_str().filter(|_| there && same).unwrap_or("").to_owned())
        }
    }
}

/// The agent's process in a terminal: the outermost of those under it that
/// is a coding agent.
fn agent_pid(layer: i32) -> Option<i32> {
    let mut found: Vec<(i32, i32)> = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        let Ok(env) = std::fs::read(format!("/proc/{pid}/environ")) else { continue };
        if !env.split(|b| *b == 0).any(|v| v == format!("PLEAMAR_TERMINAL={layer}").as_bytes()) {
            continue;
        }
        let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let words: Vec<String> = cmd.split(|b| *b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
        if crate::term::agent_of(&words.join(" ")).is_none() {
            continue;
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let ppid = stat.rsplit_once(") ").and_then(|(_, r)| r.split_whitespace().nth(1)).and_then(|p| p.parse().ok()).unwrap_or(0);
        found.push((pid, ppid));
    }
    found.iter().find(|(_, pp)| !found.iter().any(|(p, _)| p == pp)).map(|(p, _)| *p)
}

/// Whether that name's agent can be restarted: its script loops, and it runs.
pub(crate) fn restartable(name: &str) -> bool {
    plain(name) && read(name)["loops"] == true && crate::term::layers().iter().any(|(_, i)| i.get("name").map(String::as_str) == Some(name))
}

/// `link restart NAME [--fresh] [--then TEXT…]`.
pub(crate) fn restart(rest: &[String]) -> Result<String, String> {
    let name = rest.first().cloned().unwrap_or_default();
    let then_at = rest.iter().position(|a| a == "--then");
    let then = then_at.map_or_else(String::new, |k| rest[k + 1..].join(" "));
    let fresh = rest[..then_at.unwrap_or(rest.len())].iter().any(|a| a == "--fresh");
    if !plain(&name) {
        return Err("link restart NAME [--fresh] [--then TEXT]".into());
    }
    if read(&name)["loops"] != true {
        return Err(format!("«{name}» is not started by a script that starts it again: it cannot be restarted from here"));
    }
    let (layer, _) = crate::term::layers().into_iter().find(|(_, i)| i.get("name").map(String::as_str) == Some(name.as_str())).ok_or_else(|| format!("«{name}» has no terminal open"))?;
    let pid = agent_pid(layer).ok_or_else(|| format!("no agent runs in «{name}»"))?;
    // (Codex: its conversation as it is now, to come back to it.)
    if crate::term::layers().iter().any(|(id, i)| *id == layer && i.get("agent").map(String::as_str) == Some("codex")) {
        look_codex(&name, layer);
    }
    change(&name, |m| {
        m.insert("restart".into(), json!(true));
        if fresh {
            m.insert("fresh".into(), json!(true));
        }
    });
    crate::journal::note(&name, "restart", if fresh { "asked, in a new conversation" } else { "asked" });
    // SAFETY: a signal to a process of this person's, found by its terminal.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let start = Instant::now();
    while std::path::Path::new(&format!("/proc/{pid}")).exists() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if std::path::Path::new(&format!("/proc/{pid}")).exists() {
        // SAFETY: as above; it did not end on its own.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    if fresh {
        // Nothing to carry on in a new conversation, but what was asked with it.
        if !then.trim().is_empty() {
            crate::mail::post(&[name.clone(), then.trim().to_owned(), "--from".into(), "pleamar".into(), "--no-reply".into()])?;
        }
        return Ok(format!("«{name}» restarted in a new conversation"));
    }
    let text = if then.trim().is_empty() {
        "Your session was restarted (MCP servers, skills and settings are loaded again), in the same conversation. Carry on with what you were doing.".to_owned()
    } else {
        format!("Your session was restarted (MCP servers, skills and settings are loaded again), in the same conversation. Carry on: {}", then.trim())
    };
    crate::mail::post(&[name.clone(), text, "--from".into(), "pleamar".into(), "--no-reply".into()])?;
    Ok(format!("«{name}» restarted: it resumes its conversation, and a letter tells it to carry on"))
}

/// The agent asks it of itself (from its MCP): after a moment, so that what it
/// gets back is said first, and from a process of its own, that its end does
/// not take with it.
pub(crate) fn restart_me(then: &str) -> Result<String, String> {
    let name = std::env::var("PLEAMAR_NAME").ok().filter(|n| plain(n)).ok_or("only an agent in a named terminal (a companion) can restart itself; ask your person to restart you")?;
    if read(&name)["loops"] != true {
        return Err("your terminal was not started by a script that starts you again: ask your person to restart you".into());
    }
    let me = crate::link::own_exe();
    let mut c = std::process::Command::new("sh");
    c.args(["-c", "sleep 3; exec \"$0\" link restart \"$1\" --then \"$2\"", &me.to_string_lossy(), &name, then]);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid in the child before exec: its own session, out of the agent's.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map_err(|e| e.to_string())?;
    Ok("In a few seconds this session ends and starts again in this same conversation, with what is installed now. End your turn now; a letter will tell you to carry on.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_long_a_conversation_is_from_its_record() {
        let dir = std::env::temp_dir().join(format!("pwm-ctx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let claude = dir.join("c.jsonl");
        std::fs::write(&claude, concat!(
            r#"{"type":"assistant","message":{"model":"claude-sonnet-5-5","usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}"#, "\n",
            r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":900000}}}"#, "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5[1m]","usage":{"input_tokens":4,"cache_creation_input_tokens":988,"cache_read_input_tokens":58829,"output_tokens":164}}}"#, "\n",
            r#"{"type":"user","message":{"content":"next"}}"#, "\n",
        )).unwrap();
        assert_eq!(tokens_in(&claude, "claude"), Some((59821, 1_000_000)));
        let codex = dir.join("x.jsonl");
        std::fs::write(&codex, concat!(
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":25493},"model_context_window":258400}}}"#, "\n",
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"hecho"}}"#, "\n",
        )).unwrap();
        assert_eq!(tokens_in(&codex, "codex"), Some((25493, 258400)));
        assert_eq!(tokens_in(&dir.join("none.jsonl"), "claude"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
