//! `pleamar-wm gate …`: the gate an agent's action goes through when it would
//! ask its person for permission. Claude Code and Codex call it from their
//! `PermissionRequest` hook (`gate hook claude|codex`), opencode from a plugin
//! (`gate ask`): the action waits, stopped for real, until it is decided —by
//! the person's rules, or by the person from their shell (`gate answer`)—.
//! Nobody deciding in time, or no shell listening, or outside a session: the
//! agent asks on its own screen, as ever. It never decides by itself.
//!
//! The rules are the person's, in `gate.rules` beside the session's
//! configuration, one a line: `deny|allow WHO TOOL WHAT` (WHO an agent or a
//! terminal's name, TOOL a tool, WHAT a pattern with `*` and `?` over what it
//! does —its command, its file—; `*` for any). Denials first, then
//! permissions; the rest is asked. A line that cannot be read denies
//! everything until it is mended.
//!
//! A request made while the agent is at a letter from a routine (its mailbox,
//! mail.rs: `--from "routine NAME"`) is held to less: rules can name it
//! (WHO `routine`), and with no rule it is asked, but nobody answering, or no
//! shell listening, denies it —nobody watches its screen—.
//!
//! Every decision is written down before it is told (`gate.log`, JSON a line,
//! secrets hidden): when, who, in which window, what, the decision and by
//! whom. The scene's `gate.seq` changes when a request comes or goes; a shell
//! that answers says so with `gate.listen`.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// How long a request waits for the person before its agent asks on its own
/// screen; its hook is given more (`setup`).
const WAIT: u64 = 90;
const HOOK_TIMEOUT: u64 = 120;

/// How long to wait for the person: `WAIT`, or `PLEAMAR_GATE_WAIT` seconds (tests).
fn wait() -> Duration {
    Duration::from_secs(std::env::var("PLEAMAR_GATE_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(WAIT).min(HOOK_TIMEOUT - 5))
}

fn dir() -> String {
    format!("{}/gate", crate::keep::dir())
}

fn pending_dir() -> String {
    format!("{}/pending", dir())
}

fn log_file() -> String {
    format!("{}/log.jsonl", dir())
}

/// The person's rules: `gate.rules` in the session's configuration folder.
pub(crate) fn rules_file() -> Option<String> {
    crate::config::user_dir().map(|d| format!("{d}/gate.rules"))
}

fn now_ms() -> u64 {
    crate::mail::now_ms()
}

/// What an agent asks to do, and who asks.
#[derive(Clone, Default)]
struct Ask {
    agent: String,
    name: String,
    slot: i64,
    tool: String,
    what: String,
    cwd: String,
    /// The routine whose letter it is at, if one.
    routine: String,
}

/// One rule of `gate.rules`.
struct Rule {
    allow: bool,
    who: String,
    tool: String,
    what: String,
    line: usize,
}

/// `*` any run, `?` one character; the rest as it is.
fn glob(pat: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), text.chars().collect());
    let (mut i, mut j, mut star, mut mark) = (0, 0, None::<usize>, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// The first word, and what comes after it (trimmed).
fn word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(k) => (&s[..k], s[k..].trim()),
        None => (s, ""),
    }
}

/// The rules, or which line cannot be read.
fn rules() -> Result<Vec<Rule>, String> {
    let Some(file) = rules_file() else { return Ok(Vec::new()) };
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(Vec::new()) };
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = Vec::new();
    for (k, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (verb, rest) = word(line);
        let (who, rest) = word(rest);
        let (tool, what) = word(rest);
        let allow = match verb {
            "allow" => true,
            "deny" => false,
            _ => return Err(format!("{file}, line {}: «{line}» starts with neither allow nor deny", k + 1)),
        };
        if who.is_empty() || tool.is_empty() || what.is_empty() {
            return Err(format!("{file}, line {}: «{line}» needs WHO TOOL WHAT (`*` for any)", k + 1));
        }
        let what = if let Some(rest) = what.strip_prefix("~/") { format!("{home}/{rest}") } else { what.to_owned() };
        out.push(Rule { allow, who: who.to_lowercase(), tool: tool.to_lowercase(), what, line: k + 1 });
    }
    Ok(out)
}

/// Whether a rule says something of that: denials first.
fn by_rules(rules: &[Rule], a: &Ask) -> Option<(bool, String)> {
    let fits = |r: &Rule| {
        (r.who == "*" || r.who == a.agent.to_lowercase() || (!a.name.is_empty() && r.who == a.name.to_lowercase()) || (r.who == "routine" && !a.routine.is_empty()))
            && (r.tool == "*" || r.tool == a.tool.to_lowercase())
            && glob(&r.what, &a.what)
    };
    for allow in [false, true] {
        if let Some(r) = rules.iter().find(|r| r.allow == allow && fits(r)) {
            return Some((allow, format!("rule on line {}", r.line)));
        }
    }
    None
}

/// The window of the terminal the hook runs in (its layer, `PLEAMAR_TERMINAL`).
fn slot_here() -> i64 {
    let Some(layer) = std::env::var("PLEAMAR_TERMINAL").ok().and_then(|v| v.parse::<i32>().ok()) else { return -1 };
    let Some(term) = crate::term::layers().into_iter().find(|(id, _)| *id == layer).and_then(|(_, i)| i.get("terminal").and_then(|t| t.parse::<u32>().ok())) else { return -1 };
    crate::link::windows().ok().and_then(|ws| ws.into_iter().find(|w| w.pid == term)).map_or(-1, |w| w.slot as i64)
}

/// A value of the session's scene, if there is a session and it has it.
fn scene_get(name: &str) -> Option<String> {
    crate::link::say_wm(&[format!("get {name}")]).into_iter().next().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty() && !v.starts_with('?') && !v.starts_with("error"))
}

fn told() {
    crate::link::say_wm(&[format!("fact gate.seq {}", crate::mail::seq())]);
}

/// A line of the record, written before the decision is told.
fn record(a: &Ask, id: &str, decision: &str, by: &str, why: &str) {
    let _ = std::fs::create_dir_all(dir());
    let line = json!({
        "at": now_ms(), "id": id, "agent": a.agent, "name": a.name, "slot": a.slot, "tool": a.tool,
        "what": crate::agents::redact(&a.what), "cwd": a.cwd, "routine": a.routine, "decision": decision, "by": by, "why": crate::agents::redact(why),
    });
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(log_file()) {
        let _ = writeln!(f, "{line}");
    }
}

/// The decision on `a`: `Some((allow, by, why))`, or `None` for «ask on its
/// own screen». By the rules if one says; if not, waits for the person
/// (`gate answer`) while a shell listens, `wait` at most.
fn decide(a: &Ask, wait: Duration) -> Option<(bool, String, String)> {
    let id = format!("{:x}{:04x}", now_ms(), std::process::id() & 0xffff);
    let decided = match rules() {
        Err(broken) => Some((false, "rules".to_owned(), broken)),
        Ok(rs) => by_rules(&rs, a).map(|(allow, by)| (allow, by, String::new())),
    };
    if let Some((allow, by, why)) = decided {
        record(a, &id, if allow { "allow" } else { "deny" }, &by, &why);
        return Some((allow, by, why));
    }
    // Nobody to ask: no session, or no shell in it that answers. A routine's is
    // denied: nobody is at its screen either.
    if std::env::var("PLEAMAR_SOCKETS").ok().filter(|v| !v.is_empty()).is_none() || scene_get("gate.listen").is_none_or(|v| v == "0" || v == "false") {
        if !a.routine.is_empty() {
            let why = "a routine, and nobody listening".to_owned();
            record(a, &id, "deny", "routine", &why);
            return Some((false, "routine".into(), why));
        }
        record(a, &id, "screen", "nobody listening", "");
        return None;
    }
    let _ = std::fs::create_dir_all(pending_dir());
    let file = format!("{}/{id}.json", pending_dir());
    let req = json!({ "id": id, "at": now_ms(), "pid": std::process::id(), "agent": a.agent, "name": a.name, "slot": a.slot, "tool": a.tool, "what": a.what, "cwd": a.cwd, "routine": a.routine });
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = format!("{file}.new");
        let ok = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(req.to_string().as_bytes()));
        if ok.is_err() || std::fs::rename(&tmp, &file).is_err() {
            record(a, &id, "screen", "could not wait", "");
            return None;
        }
    }
    told();
    let answer_file = format!("{}/{id}.answer", pending_dir());
    let start = Instant::now();
    let mut got = None;
    while start.elapsed() < wait {
        if let Ok(t) = std::fs::read_to_string(&answer_file) {
            let mut lines = t.lines();
            let verdict = lines.next().unwrap_or("").trim().to_owned();
            let by = lines.next().unwrap_or("").trim().to_owned();
            let why = lines.collect::<Vec<_>>().join(" ");
            got = match verdict.as_str() {
                "allow" => Some((true, if by.is_empty() { "person".into() } else { by }, why)),
                "deny" => Some((false, if by.is_empty() { "person".into() } else { by }, why)),
                _ => None,
            };
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(&answer_file);
    match &got {
        Some((allow, by, why)) => record(a, &id, if *allow { "allow" } else { "deny" }, by, why),
        None if !a.routine.is_empty() => {
            let why = "a routine, and nobody answered in time".to_owned();
            record(a, &id, "deny", "routine", &why);
            told();
            return Some((false, "routine".into(), why));
        }
        None => record(a, &id, "screen", "nobody answered in time", ""),
    }
    told();
    got
}

/// Of a tool's input, what it does, in a line: its command, its file, its address.
fn what_of(tool: &str, input: &Value) -> String {
    for k in ["command", "cmd", "file_path", "path", "notebook_path", "url", "pattern", "query"] {
        if let Some(s) = input.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return s.to_owned();
        }
        if let Some(arr) = input.get(k).and_then(Value::as_array).filter(|a| !a.is_empty()) {
            return arr.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ");
        }
    }
    let all = input.to_string();
    if all == "null" || all == "{}" { tool.to_owned() } else { all.chars().take(300).collect() }
}

/// `gate hook claude|codex`: their `PermissionRequest` hook. Quiet (no output)
/// unless decided: then the decision as they read it.
fn hook(agent: &str) -> i32 {
    let mut input = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    let Ok(v) = serde_json::from_str::<Value>(&input) else { return 0 };
    if v.get("hook_event_name").and_then(Value::as_str) != Some("PermissionRequest") {
        return 0;
    }
    let tool = v.get("tool_name").and_then(Value::as_str).unwrap_or("").to_owned();
    let name = std::env::var(crate::term::NAME).unwrap_or_default();
    let a = Ask {
        agent: agent.to_owned(),
        routine: crate::mail::routine_of(&name).unwrap_or_default(),
        name,
        slot: slot_here(),
        what: what_of(&tool, v.get("tool_input").unwrap_or(&Value::Null)),
        tool,
        cwd: v.get("cwd").and_then(Value::as_str).unwrap_or("").to_owned(),
    };
    let Some((allow, by, why)) = decide(&a, wait()) else { return 0 };
    let decision = if allow {
        json!({ "behavior": "allow" })
    } else {
        let msg = if why.is_empty() { format!("Denied by the person's gate ({by}).") } else { format!("Denied by the person's gate ({by}): {why}") };
        json!({ "behavior": "deny", "message": msg })
    };
    println!("{}", json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } }));
    0
}

/// `gate ask --agent A --tool T [--cwd D] [--wait S] -- WHAT…`: the same, for
/// whatever has no hook of that shape (opencode's plugin): prints `allow`,
/// `deny` or nothing.
fn ask(args: &[String]) -> i32 {
    let flag = |f: &str| args.iter().position(|a| a == f).and_then(|k| args.get(k + 1)).cloned().unwrap_or_default();
    let what = args.iter().position(|a| a == "--").map(|k| args[k + 1..].join(" ")).unwrap_or_default();
    let name = std::env::var(crate::term::NAME).unwrap_or_default();
    let a = Ask { agent: flag("--agent"), routine: crate::mail::routine_of(&name).unwrap_or_default(), name, slot: slot_here(), tool: flag("--tool"), what, cwd: flag("--cwd") };
    if a.agent.is_empty() || a.tool.is_empty() {
        eprintln!("gate ask --agent AGENT --tool TOOL [--cwd DIR] [--wait SECONDS] -- WHAT");
        return 2;
    }
    let wait = flag("--wait").parse().map(Duration::from_secs).unwrap_or_else(|_| wait());
    match decide(&a, wait) {
        Some((true, ..)) => println!("allow"),
        Some((false, ..)) => println!("deny"),
        None => {}
    }
    0
}

/// The requests waiting now; any whose hook is gone, taken away.
fn pending() -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for e in std::fs::read_dir(pending_dir()).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let alive = v["pid"].as_u64().is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
        if !alive {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        out.push(v);
    }
    out.sort_by_key(|v| v["at"].as_u64().unwrap_or(0));
    out
}

/// `gate answer ID allow|deny [--by WHO] [--why WORDS]`.
fn answer(args: &[String]) -> Result<String, String> {
    let id = args.first().filter(|i| i.chars().all(|c| c.is_ascii_hexdigit()) && !i.is_empty()).ok_or("gate answer ID allow|deny [--by WHO] [--why WORDS]")?;
    let verdict = args.get(1).map(String::as_str).filter(|v| matches!(*v, "allow" | "deny")).ok_or("allow or deny")?;
    let flag = |f: &str| args.iter().position(|a| a == f).and_then(|k| args.get(k + 1)).cloned().unwrap_or_default();
    if !pending().iter().any(|p| p["id"].as_str() == Some(id)) {
        return Err(format!("no request {id} waits now (decided, gone, or it asked on its own screen)"));
    }
    let clean = |s: String| s.replace(['\n', '\r'], " ");
    let tmp = format!("{}/{id}.answer.new", pending_dir());
    std::fs::write(&tmp, format!("{verdict}\n{}\n{}\n", clean(flag("--by")), clean(flag("--why")))).and_then(|_| std::fs::rename(&tmp, format!("{}/{id}.answer", pending_dir()))).map_err(|e| e.to_string())?;
    Ok(format!("{id}: {verdict}"))
}

/// What a request or a decision is, in a line.
fn line_of(v: &Value) -> String {
    let who = v["name"].as_str().filter(|n| !n.is_empty()).or(v["agent"].as_str()).unwrap_or("?");
    format!("{who} (window {}) {}: {}", v["slot"], v["tool"].as_str().unwrap_or(""), v["what"].as_str().unwrap_or("").chars().take(100).collect::<String>())
}

/// `gate log [N] [--json]`: the last decisions, the last first.
fn log(args: &[String]) -> String {
    let n: usize = args.iter().find_map(|a| a.parse().ok()).unwrap_or(20);
    let text = std::fs::read_to_string(log_file()).unwrap_or_default();
    let mut lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    lines.reverse();
    lines.truncate(n);
    if args.iter().any(|a| a == "--json") {
        return Value::Array(lines).to_string();
    }
    if lines.is_empty() {
        return "nothing decided yet".into();
    }
    lines.iter().map(|v| format!("{} by {}: {}", v["decision"].as_str().unwrap_or(""), v["by"].as_str().unwrap_or(""), line_of(v))).collect::<Vec<_>>().join("\n")
}

/// The hook command, for each agent.
fn hook_command(agent: &str) -> String {
    format!("pleamar-wm gate hook {agent}")
}

/// The plugin that puts opencode's permission requests through the gate.
const OPENCODE_PLUGIN: &str = include_str!("../skill/pleamar-gate/opencode-plugin.js");

/// A hooks file in Claude Code's shape (Claude Code's settings, Codex's
/// hooks.json), with the gate's `PermissionRequest` hook in it (or without,
/// `undo`); everything else as it was, and a copy kept the first time.
fn put_hook(path: &str, command: &str, undo: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut v: Value = serde_json::from_str(&text).map_err(|e| format!("{path} is not JSON I can read ({e}): nothing changed"))?;
    if std::path::Path::new(path).exists() && !std::path::Path::new(&format!("{path}.before-pleamar-gate")).exists() {
        std::fs::copy(path, format!("{path}.before-pleamar-gate")).map_err(|e| e.to_string())?;
    }
    let root = v.as_object_mut().ok_or(format!("{path} is not an object"))?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({})).as_object_mut().ok_or("its hooks are not an object")?;
    let list = hooks.entry("PermissionRequest").or_insert_with(|| json!([]));
    if let Some(list) = list.as_array_mut() {
        list.retain(|g| !g.get("hooks").and_then(Value::as_array).is_some_and(|hs| hs.iter().any(|h| h.get("command").and_then(Value::as_str) == Some(command))));
        if !undo {
            list.push(json!({ "hooks": [{ "type": "command", "command": command, "timeout": HOOK_TIMEOUT }] }));
        }
    }
    hooks.retain(|_, l| l.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        root.remove("hooks");
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(format!("{path}.new"), out + "\n").and_then(|_| std::fs::rename(format!("{path}.new"), path)).map_err(|e| format!("{path}: {e}"))
}

/// Whether a hooks file has the gate's hook.
fn has_hook(path: &str, command: &str) -> bool {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| {
        v["hooks"]["PermissionRequest"].as_array().is_some_and(|l| l.iter().any(|g| g["hooks"].as_array().is_some_and(|hs| hs.iter().any(|h| h["command"].as_str() == Some(command)))))
    })
}

/// Where each agent is told about the gate: (agent, its file, installed?).
fn places() -> Result<Vec<(&'static str, String, bool)>, String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME")?;
    let config = std::env::var("XDG_CONFIG_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{home}/.config"));
    let claude_dir = std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{home}/.claude"));
    let codex_dir = std::env::var("CODEX_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{home}/.codex"));
    let mut out = Vec::new();
    let is_dir = |p: &str| std::path::Path::new(p).is_dir();
    if is_dir(&claude_dir) {
        let f = format!("{claude_dir}/settings.json");
        let on = has_hook(&f, &hook_command("claude"));
        out.push(("claude", f, on));
    }
    if is_dir(&codex_dir) {
        let f = format!("{codex_dir}/hooks.json");
        let on = has_hook(&f, &hook_command("codex"));
        out.push(("codex", f, on));
    }
    if is_dir(&format!("{config}/opencode")) {
        let f = format!("{config}/opencode/plugin/pleamar-gate.js");
        let on = std::path::Path::new(&f).exists();
        out.push(("opencode", f, on));
    }
    Ok(out)
}

/// `gate setup [--yes] [--undo]`: the gate in each agent here; asks first.
fn setup(yes: bool, undo: bool) -> Result<String, String> {
    let places = places()?;
    if places.is_empty() {
        return Err("no agent found to put the gate in (~/.claude, ~/.codex, ~/.config/opencode)".into());
    }
    let plan: Vec<String> = places
        .iter()
        .map(|(agent, f, _)| match *agent {
            "opencode" => format!("{} the plugin {f}", if undo { "remove" } else { "write" }),
            _ => format!("{} {agent}'s PermissionRequest hook in {f} (a copy kept as .before-pleamar-gate)", if undo { "take away" } else { "add" }),
        })
        .collect();
    if !crate::link::go_ahead("pleamar-wm gate setup", &plan, yes)? {
        return Ok("nothing changed".into());
    }
    for (agent, f, _) in &places {
        match *agent {
            "opencode" if undo => {
                let _ = std::fs::remove_file(f);
            }
            "opencode" => {
                if let Some(parent) = std::path::Path::new(f).parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(f, OPENCODE_PLUGIN).map_err(|e| format!("{f}: {e}"))?;
            }
            a => put_hook(f, &hook_command(a), undo)?,
        }
    }
    Ok(format!("{} for {}. Agents started from now on {}; restart the ones already open.", if undo { "the gate taken away" } else { "the gate set up" }, places.iter().map(|p| p.0).collect::<Vec<_>>().join(", "), if undo { "ask on their own screen" } else { "ask through it" }))
}

/// `gate status [--json]`: whether each agent has it, and the rules.
fn status(json_out: bool) -> Result<String, String> {
    let places = places()?;
    let rules = rules();
    let rules_text = rules_file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
    let n = rules.as_ref().map_or(0, Vec::len);
    if json_out {
        return Ok(json!({
            "agents": places.iter().map(|(a, f, on)| json!({ "agent": a, "file": f, "on": on })).collect::<Vec<_>>(),
            "rules_file": rules_file(), "rules": n, "broken": rules.as_ref().err(),
            "lines": rules_text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).collect::<Vec<_>>(),
        })
        .to_string());
    }
    let mut out: Vec<String> = places.iter().map(|(a, f, on)| format!("{a}: {} ({f})", if *on { "through the gate" } else { "not set up" })).collect();
    out.push(match &rules {
        Ok(_) => format!("{n} rules in {}", rules_file().unwrap_or_default()),
        Err(e) => format!("rules broken, everything is denied: {e}"),
    });
    Ok(out.join("\n"))
}

/// What a new `gate.rules` says: how to write one, and nothing in force.
const RULES_TEMPLATE: &str = "# The gate's rules (pleamar-wm gate): one a line, deny|allow WHO TOOL WHAT.
# WHO: an agent (claude, codex, opencode), a terminal's name (Prohard) or `routine` (a task a routine
# handed: with no rule it is asked, and denied if nobody answers). TOOL: a tool (Bash, Edit,
# Write, WebFetch…). WHAT: a pattern with * and ? over its command or its file. * for any.
# Denials go first, then permissions; whatever no rule names is asked to you.
# A line that cannot be read denies everything until it is mended.
#
# deny  *       Bash  git push*
# allow claude  Bash  git status*
# allow Prohard Edit  ~/Proyectos/calima/*
";

/// `gate rules [--init]`: where the rules are; `--init` writes a first one
/// (only comments) if there is none.
fn rules_path(init: bool) -> Result<String, String> {
    let f = rules_file().ok_or("no configuration folder")?;
    if init && !std::path::Path::new(&f).exists() {
        if let Some(parent) = std::path::Path::new(&f).parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&f, RULES_TEMPLATE).map_err(|e| format!("{f}: {e}"))?;
    }
    Ok(f)
}

pub fn run(args: &[String]) -> i32 {
    let rest = args.get(1..).unwrap_or(&[]);
    let out = match args.first().map(String::as_str) {
        Some("hook") => return hook(rest.first().map_or("claude", String::as_str)),
        Some("ask") => return ask(rest),
        Some("list") => {
            let p = pending();
            Ok(if rest.iter().any(|a| a == "--json") {
                Value::Array(p).to_string()
            } else if p.is_empty() {
                "nothing waits at the gate".into()
            } else {
                p.iter().map(|v| format!("{} {}", v["id"].as_str().unwrap_or(""), line_of(v))).collect::<Vec<_>>().join("\n")
            })
        }
        Some("answer") => answer(rest),
        Some("log") => Ok(log(rest)),
        Some("status") => status(rest.iter().any(|a| a == "--json")),
        Some("rules") => rules_path(rest.iter().any(|a| a == "--init")),
        Some("setup") => setup(rest.iter().any(|a| a == "--yes"), rest.iter().any(|a| a == "--undo")),
        _ => Err(USAGE.into()),
    };
    match out {
        Ok(s) => {
            if !s.is_empty() {
                println!("{s}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

const USAGE: &str = "pleamar-wm gate — what agents ask to do, decided by the person's rules or by the person
  setup [--yes] [--undo]          puts the gate in Claude Code and Codex (their PermissionRequest hook)
                                  and opencode (a plugin); asks first
  status [--json]                 which agents go through it, and the rules (gate.rules)
  list [--json]                   the requests waiting for the person now
  answer ID allow|deny [--by WHO] [--why WORDS]
  log [N] [--json]                the last decisions, the last first
  rules [--init]                  where the rules are (--init writes a first one, all comments)
  hook claude|codex               (their hook: the request on stdin, the decision on stdout)
  ask --agent A --tool T [--cwd D] [--wait S] -- WHAT
                                  (for a plugin: prints allow, deny or nothing)
gate.rules, one a line: deny|allow WHO TOOL WHAT — WHO an agent, a terminal's name or `routine`, TOOL a tool,
WHAT a pattern (* and ?) over its command or file; * for any. Denials first; the rest is asked.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_fits_as_a_shell_glob() {
        assert!(glob("git status*", "git status --short"));
        assert!(glob("*", "anything"));
        assert!(!glob("git push*", "git status"));
        assert!(glob("rm ?.txt", "rm a.txt"));
        assert!(glob("*/secret/*", "/home/a/secret/k"));
    }

    #[test]
    fn denials_go_first() {
        let r = |allow, who: &str, tool: &str, what: &str, line| Rule { allow, who: who.into(), tool: tool.into(), what: what.into(), line };
        let rules = [r(true, "*", "bash", "git *", 1), r(false, "*", "bash", "git push*", 2)];
        let a = |what: &str| Ask { agent: "claude".into(), tool: "Bash".into(), what: what.into(), ..Default::default() };
        assert_eq!(by_rules(&rules, &a("git status")).map(|d| d.0), Some(true));
        assert_eq!(by_rules(&rules, &a("git push origin main")).map(|d| d.0), Some(false));
        assert!(by_rules(&rules, &a("ls")).is_none());
    }

    #[test]
    fn a_rule_is_four_fields_the_last_with_spaces() {
        assert_eq!(word("allow  claude Bash git status *"), ("allow", "claude Bash git status *"));
        let (_, r) = word("allow claude Bash git status *");
        let (_, r) = word(r);
        assert_eq!(word(r), ("Bash", "git status *"));
    }

    #[test]
    fn routine_is_a_who_only_at_a_routine() {
        let rules = [Rule { allow: true, who: "routine".into(), tool: "bash".into(), what: "echo*".into(), line: 1 }];
        let a = Ask { agent: "claude".into(), tool: "Bash".into(), what: "echo hi".into(), ..Default::default() };
        assert!(by_rules(&rules, &a).is_none());
        let r = Ask { routine: "Factorial".into(), ..a.clone() };
        assert_eq!(by_rules(&rules, &r).map(|d| d.0), Some(true));
    }

    #[test]
    fn a_name_is_a_who() {
        let rules = [Rule { allow: true, who: "prohard".into(), tool: "*".into(), what: "*".into(), line: 1 }];
        let a = Ask { agent: "claude".into(), name: "Prohard".into(), tool: "Edit".into(), what: "x".into(), ..Default::default() };
        assert!(by_rules(&rules, &a).is_some());
        let b = Ask { name: "Golio".into(), ..a.clone() };
        assert!(by_rules(&rules, &b).is_none());
    }

    #[test]
    fn what_a_tool_does_in_a_line() {
        assert_eq!(what_of("Bash", &json!({ "command": "ls -la", "description": "list" })), "ls -la");
        assert_eq!(what_of("Write", &json!({ "file_path": "/a/b", "content": "x" })), "/a/b");
        assert_eq!(what_of("WebFetch", &json!({ "url": "https://x", "prompt": "p" })), "https://x");
    }
}
