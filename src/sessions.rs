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

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    let session = session.to_owned();
    change(&name, |m| {
        if m.get("session").and_then(Value::as_str) == Some(session.as_str()) && m.get("agent").and_then(Value::as_str) == Some(agent) {
            return;
        }
        m.insert("agent".into(), json!(agent));
        m.insert("session".into(), json!(session));
        m.insert("transcript".into(), json!(transcript));
        m.insert("at".into(), json!(crate::mail::now_ms()));
    });
}

/// `link session NAME [--loop | --again | --json]`.
pub(crate) fn session(rest: &[String]) -> Result<String, String> {
    let name = rest.first().map_or("", String::as_str);
    if !plain(name) {
        return Err("link session NAME [--loop | --again | --json]".into());
    }
    match rest.get(1).map(String::as_str) {
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

/// `link restart NAME [--then TEXT…]`.
pub(crate) fn restart(rest: &[String]) -> Result<String, String> {
    let name = rest.first().cloned().unwrap_or_default();
    let then = match rest.iter().position(|a| a == "--then") {
        Some(k) => rest[k + 1..].join(" "),
        None => String::new(),
    };
    if !plain(&name) {
        return Err("link restart NAME [--then TEXT]".into());
    }
    if read(&name)["loops"] != true {
        return Err(format!("«{name}» is not started by a script that starts it again: it cannot be restarted from here"));
    }
    let (layer, _) = crate::term::layers().into_iter().find(|(_, i)| i.get("name").map(String::as_str) == Some(name.as_str())).ok_or_else(|| format!("«{name}» has no terminal open"))?;
    let pid = agent_pid(layer).ok_or_else(|| format!("no agent runs in «{name}»"))?;
    change(&name, |m| {
        m.insert("restart".into(), json!(true));
    });
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
