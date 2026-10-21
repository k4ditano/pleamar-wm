//! `pleamar-wm link …`: what an agent in a terminal does with the windows
//! wired to its own, on a canvas.
//!
//! A wire between two windows (dragged from one's port to the other) is the
//! person's word that what runs in one may read, type in and hand tasks to
//! the other. This is how: it finds the caller's window by its terminal
//! (`PLEAMAR_TERMINAL`, the layer of term.rs, and the window of that
//! terminal's process), the wires from the window manager's scene, and the
//! terminal of the window at the other end; then reads and types through
//! that terminal's layer. A window wired to the caller's that runs no
//! terminal through a layer can be listed, not read nor typed in.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

type Info = BTreeMap<String, String>;

/// A window of the session: its place in the scene and its process.
struct Win {
    slot: usize,
    pid: u32,
    title: String,
}

/// The session's windows, as pleamar-wm writes them (`windows` next to its
/// other sockets): `slot pid<TAB>title`, one a line.
fn windows() -> Result<Vec<Win>, String> {
    let dir = std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()).ok_or("not inside a pleamar-wm session")?;
    let text = std::fs::read_to_string(format!("{dir}/windows")).map_err(|_| "this pleamar-wm session says nothing of its windows (an older pleamar-wm?)".to_string())?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let (head, title) = l.split_once('\t').unwrap_or((l, ""));
            let mut w = head.split_whitespace();
            Some(Win { slot: w.next()?.parse().ok()?, pid: w.next()?.parse().ok()?, title: title.to_owned() })
        })
        .collect())
}

/// Orders to the window manager's scene, answered in order.
/// This program's own file. Replaced while it runs (an update, a build), Linux
/// names it «… (deleted)» and nothing can be started from that: the new one is
/// where it was.
pub(crate) fn own_exe() -> std::path::PathBuf {
    let me = std::env::current_exe().unwrap_or_else(|_| "pleamar-wm".into());
    std::path::PathBuf::from(me.to_string_lossy().trim_end_matches(" (deleted)"))
}

fn say(orders: &[String]) -> Vec<String> {
    let me = own_exe();
    let out = std::process::Command::new(me).arg("--say").arg("wm").args(orders).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect()).unwrap_or_default()
}

/// The wires: which slots each joins, by its place (`wire.K`), and what the
/// person called it.
fn wires() -> Vec<(usize, i64, i64, String)> {
    let orders: Vec<String> = (0..8).flat_map(|k| [format!("get wire.{k}.a"), format!("get wire.{k}.b")]).collect();
    let got: Vec<i64> = say(&orders).iter().map(|v| v.trim().parse::<f64>().map_or(-1, |v| v.round() as i64)).collect();
    (0..8)
        .filter_map(|k| Some((k, *got.get(2 * k)?, *got.get(2 * k + 1)?)))
        .filter(|(_, a, b)| *a >= 0 && *b >= 0)
        .map(|(k, a, b)| (k, a, b, say(&[format!("get wire.{k}.label")]).first().map(|l| l.trim().to_owned()).unwrap_or_default()))
        .collect()
}

/// The terminal layer whose terminal is that process, if one.
fn layer_of(pid: u32, layers: &[(i32, Info)]) -> Option<(i32, Info)> {
    let mut of: Vec<&(i32, Info)> = layers.iter().filter(|(_, i)| i.get("terminal").and_then(|t| t.parse::<u32>().ok()) == Some(pid)).collect();
    // A terminal with several shells (tabs) speaks for the one that moved last.
    of.sort_by_key(|(_, i)| i.get("quiet").and_then(|q| q.parse::<u64>().ok()).unwrap_or(u64::MAX));
    of.first().map(|l| (*l).clone())
}

struct Here {
    layer: i32,
    info: Info,
    slot: usize,
    wins: Vec<Win>,
    layers: Vec<(i32, Info)>,
    wires: Vec<(usize, i64, i64, String)>,
}

/// Who calls: its terminal's layer, and the window that terminal is.
fn here() -> Result<Here, String> {
    let layer: i32 = std::env::var("PLEAMAR_TERMINAL").ok().and_then(|v| v.parse().ok()).ok_or("this is not a terminal of a pleamar-wm session (no PLEAMAR_TERMINAL): only an agent in one can use the windows wired to it")?;
    let layers = crate::term::layers();
    let info = layers.iter().find(|(id, _)| *id == layer).map(|(_, i)| i.clone()).ok_or("this terminal's layer does not answer")?;
    let terminal: u32 = info.get("terminal").and_then(|t| t.parse().ok()).unwrap_or(0);
    let wins = windows()?;
    let slot = wins.iter().find(|w| w.pid == terminal).map(|w| w.slot).ok_or("this terminal's window is not one of the session's")?;
    Ok(Here { layer, info, slot, wins, layers, wires: wires() })
}

impl Here {
    /// The windows wired to the caller's, with the wire's place and which end the caller is.
    fn wired(&self) -> Vec<(usize, &Win, bool)> {
        self.wires
            .iter()
            .filter_map(|(k, a, b, _)| {
                let (mine_is_a, other) = if *a as usize == self.slot { (true, *b) } else if *b as usize == self.slot { (false, *a) } else { return None };
                self.wins.iter().find(|w| w.slot == other as usize).map(|w| (*k, w, mine_is_a))
            })
            .collect()
    }

    /// What the person called that wire, if anything.
    fn label(&self, k: usize) -> &str {
        self.wires.iter().find(|w| w.0 == k).map_or("", |w| w.3.as_str())
    }

    /// The one named: by its number, or by what runs in it if only one is that.
    fn target(&self, name: &str) -> Result<(usize, &Win, bool, Option<(i32, Info)>), String> {
        let wired = self.wired();
        let found: Vec<&(usize, &Win, bool)> = match name.trim_start_matches('#').parse::<usize>() {
            Ok(n) => wired.iter().filter(|(_, w, _)| w.slot == n).collect(),
            // By the wire's name first («tests»), then by what runs there.
            Err(_) if wired.iter().any(|(k, _, _)| self.label(*k).eq_ignore_ascii_case(name)) => wired.iter().filter(|(k, _, _)| self.label(*k).eq_ignore_ascii_case(name)).collect(),
            Err(_) => wired
                .iter()
                .filter(|(_, w, _)| {
                    let l = layer_of(w.pid, &self.layers);
                    l.as_ref().and_then(|(_, i)| i.get("agent")).is_some_and(|a| a == name) || w.title.to_lowercase().contains(&name.to_lowercase())
                })
                .collect(),
        };
        match found.as_slice() {
            [(k, w, a)] => Ok((*k, w, *a, layer_of(w.pid, &self.layers))),
            [] if name.parse::<usize>().is_ok() => Err(format!("window {name} isn't wired to yours. Ask the person to wire them: on a free monitor (Super+W), drag the drop at the start of one's title bar onto the other.")),
            [] => Err(format!("no window wired to yours is «{name}». `pleamar-wm link list` says which are.")),
            _ => Err(format!("more than one window wired to yours is «{name}»: name it by its number (`pleamar-wm link list`).")),
        }
    }
}

fn line_of(slot: usize, w: &Win, layer: Option<&(i32, Info)>, label: &str) -> String {
    let g = |k: &str| layer.and_then(|(_, i)| i.get(k)).cloned().unwrap_or_default();
    let what = match layer {
        None => "no terminal of its own to read or type in".to_owned(),
        Some(_) => {
            let agent = g("agent");
            let state = match (agent.is_empty(), g("state").as_str()) {
                (true, _) => String::new(),
                (false, "working") => " (working)".into(),
                (false, "idle") => " (ready)".into(),
                _ => String::new(),
            };
            let runs = if agent.is_empty() { g("command") } else { agent };
            format!("{runs}{state} in {}", tilde(&g("cwd")))
        }
    };
    let named = if label.is_empty() { String::new() } else { format!("[{label}]  ") };
    format!("{slot}  {named}«{}»  {what}", w.title)
}

fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && path.starts_with(&h) => format!("~{}", &path[h.len()..]),
        _ => path.to_owned(),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn socket(id: i32) -> String {
    format!("{}/{id}.sock", crate::term::socket_dir())
}

fn info(id: i32) -> Info {
    crate::term::ask(&socket(id), "info")
        .map(|t| t.lines().filter_map(|l| l.split_once(' ').map(|(k, v)| (k.to_owned(), v.to_owned())).or_else(|| Some((l.to_owned(), String::new())))).collect())
        .unwrap_or_default()
}

fn read(id: i32, lines: usize) -> String {
    crate::term::ask(&socket(id), &format!("read {lines}")).unwrap_or_default()
}

fn paste(id: i32, text: &str) -> Result<(), String> {
    match crate::term::ask(&socket(id), &format!("paste {}", hex(text.as_bytes())))? {
        r if r.starts_with("ok") => Ok(()),
        r => Err(r.trim().to_owned()),
    }
}

fn keys(id: i32, names: &str) -> Result<(), String> {
    match crate::term::ask(&socket(id), &format!("keys {names}"))? {
        r if r.starts_with("ok") => Ok(()),
        r => Err(r.trim().to_owned()),
    }
}

/// Done with what it was doing: its agent says so through its hooks, or
/// shows it is ready (✳), or —no title to go by— nothing has come out of it
/// for a while.
fn at_rest(i: &Info) -> bool {
    if let Some(h) = i.get("hook").filter(|h| !h.is_empty()) {
        return h == "done";
    }
    match i.get("state").map(String::as_str) {
        Some("idle") => true,
        Some("working") => false,
        _ => i.get("quiet").and_then(|q| q.parse::<u64>().ok()).is_some_and(|q| q > 4000),
    }
}

thread_local! {
    /// How many answers the target's hooks had told when the task was handed.
    static HANDED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    /// When it was handed, in ms since the epoch: an answer kept before is not its.
    static HANDED_AT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The agent's last answer, whole, from the record of the conversation it
/// keeps: a full-screen agent (Codex, opencode) leaves nothing in the
/// terminal's history, so the start of a long answer is gone from its screen.
/// Only one given after `after` (ms since the epoch) to a task that came
/// through a wire, or —`after` 0— the last one.
fn kept_answer(i: &Info, after: u64) -> Option<String> {
    let cwd = i.get("cwd").filter(|c| !c.is_empty())?;
    match i.get("agent")?.as_str() {
        "claude" => claude_answer(i.get("layer").and_then(|l| l.parse().ok()).unwrap_or(0), cwd, after),
        "codex" => codex_answer(cwd, after),
        "opencode" => opencode_answer(cwd, after),
        _ => None,
    }
}

/// When the last task through a wire was handed to that terminal (ms since the
/// epoch, 0 if none), and how many answers its hooks had told then.
fn handed(id: i32) -> (u64, u64) {
    let num = |f: &str| std::fs::read_to_string(format!("{}/{id}.{f}", crate::term::socket_dir())).ok().and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(0);
    (num("handed"), num("handed-answers"))
}

/// Claude Code writes each conversation in `~/.claude/projects/<folder, its
/// / and . as ->/<session>.jsonl` (or `$CLAUDE_CONFIG_DIR`'s): of that folder's
/// written since `after`, the one a task through a wire came to after it if
/// one did, and in it the last message it wrote. Its hooks say the same when
/// they are set up; this is for when they are not, or say nothing (a turn
/// stopped with Esc).
fn claude_answer(id: i32, cwd: &str, after: u64) -> Option<String> {
    // The one its hooks said it writes, if they did.
    if let Some(said) = transcript_of(id).and_then(|p| claude_said(&tail_of(&p, 16 << 20)?, after)) {
        return Some(said.1);
    }
    let home = std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| format!("{}/.claude", std::env::var("HOME").unwrap_or_default()));
    let folder: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut files: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(format!("{home}/projects/{folder}"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64, e.path())))
        .filter(|(at, _)| at + 1000 >= after)
        .collect();
    files.sort();
    let mut said: Vec<(bool, String)> = files.iter().rev().take(4).filter_map(|(_, p)| claude_said(&tail_of(p, 16 << 20)?, after)).collect();
    // The one the task came to; failing that, the one written last.
    let k = said.iter().position(|(asked, _)| *asked).unwrap_or(0);
    (k < said.len()).then(|| said.swap_remove(k).1)
}

/// The record Claude Code's hooks said that terminal's conversation is kept in.
fn transcript_of(id: i32) -> Option<std::path::PathBuf> {
    let path = std::fs::read_to_string(format!("{}/{id}.transcript", crate::term::socket_dir())).ok()?;
    Some(std::path::PathBuf::from(path.trim())).filter(|p| p.is_file())
}

/// Whether a Claude Code that ended its turn still has commands of its own
/// running in the background, started since `after`: it is waiting for them
/// (a build, tests), and goes on by itself when they are done.
fn in_background(id: i32, i: &Info, after: u64) -> bool {
    if after == 0 || i.get("agent").map(String::as_str) != Some("claude") {
        return false;
    }
    transcript_of(id).and_then(|p| tail_of(&p, 16 << 20)).is_some_and(|t| !running(&t, after).is_empty())
}

/// Of (the end of) Claude Code's record: the commands it left running in the
/// background since `after` that have not told it they ended.
fn running(text: &str, after: u64) -> Vec<String> {
    const STARTED: &str = "running in background with ID: ";
    let mut ids: Vec<String> = Vec::new();
    for l in text.lines().filter(|l| l.contains(STARTED) || l.contains("<task-id>")) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        if v["type"] != "user" || v["timestamp"].as_str().and_then(iso_ms).is_none_or(|t| t + 1000 < after) {
            continue;
        }
        // What it was told: a tool's result (text, or blocks of it), or a notice.
        let mut said = String::new();
        let mut gather = |c: &serde_json::Value| match c {
            serde_json::Value::String(s) => said.push_str(s),
            c => c.as_array().into_iter().flatten().for_each(|b| {
                if let Some(t) = b["text"].as_str() {
                    said.push_str(t);
                }
                match &b["content"] {
                    serde_json::Value::String(s) => said.push_str(s),
                    c => c.as_array().into_iter().flatten().filter_map(|x| x["text"].as_str()).for_each(|t| said.push_str(t)),
                }
            }),
        };
        gather(&v["message"]["content"]);
        let word = |from: &str| from.chars().take_while(char::is_ascii_alphanumeric).collect::<String>();
        for (k, _) in said.match_indices(STARTED) {
            let id = word(&said[k + STARTED.len()..]);
            if !id.is_empty() && !ids.contains(&id) {
                ids.push(id);
            }
        }
        for (k, _) in said.match_indices("<task-id>") {
            let id = word(&said[k + 9..]);
            let status = said[k..].split("<status>").nth(1).map(word).unwrap_or_default();
            if status != "running" {
                ids.retain(|i| *i != id);
            }
        }
    }
    ids
}

/// The last `bytes` of a file, from the first whole line in them: a
/// conversation's record can be hundreds of megabytes.
fn tail_of(path: &std::path::Path, bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(bytes))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let from = if len > bytes { buf.iter().position(|b| *b == b'\n').map_or(buf.len(), |k| k + 1) } else { 0 };
    Some(String::from_utf8_lossy(&buf[from..]).into_owned())
}

/// Of (the end of) Claude Code's record: its last message written after
/// `after` —and after the last task through a wire, if one came then—, all
/// its text, and whether such a task came.
fn claude_said(text: &str, after: u64) -> Option<(bool, String)> {
    let (mut asked, mut id, mut answer) = (false, String::new(), String::new());
    for v in text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()) {
        if v["isSidechain"] == true || v["timestamp"].as_str().and_then(iso_ms).is_none_or(|t| t + 1000 < after) {
            continue;
        }
        let content = &v["message"]["content"];
        let texts: Vec<&str> = match content {
            serde_json::Value::String(s) => vec![s.as_str()],
            c => c.as_array().map(|c| c.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect()).unwrap_or_default(),
        };
        match v["type"].as_str() {
            Some("user") if after > 0 && texts.iter().any(|t| t.contains("through a wire]")) => {
                asked = true;
                id.clear();
                answer.clear();
            }
            Some("assistant") if !texts.is_empty() => {
                // A message comes in several lines, a block each.
                let this = v["message"]["id"].as_str().unwrap_or("");
                if this != id || this.is_empty() {
                    id = this.to_owned();
                    answer.clear();
                }
                for t in texts {
                    if !answer.is_empty() {
                        answer.push_str("\n\n");
                    }
                    answer.push_str(t.trim());
                }
            }
            _ => {}
        }
    }
    (!answer.trim().is_empty()).then_some((asked, answer))
}

/// `2026-10-21T15:09:58.999Z` in ms since the epoch.
fn iso_ms(t: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let ms = t.get(19..).and_then(|r| r.strip_prefix('.')).map_or(0, |r| {
        let digits: String = r.chars().take_while(char::is_ascii_digit).take(3).collect();
        format!("{digits:0<3}").parse::<i64>().unwrap_or(0)
    });
    // Days from the civil date (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(((days * 24 + hh) * 60 + mm) * 60_000 + ss * 1000 + ms).ok()
}

/// Codex writes each conversation in `~/.codex/sessions/…/rollout-*.jsonl`
/// (or `$CODEX_HOME`'s): the one in that folder written last, and in it the
/// answer that ended the turn the task began.
fn codex_answer(cwd: &str, after: u64) -> Option<String> {
    let home = std::env::var("CODEX_HOME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| format!("{}/.codex", std::env::var("HOME").unwrap_or_default()));
    let mut files = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from(format!("{home}/sessions"))];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                let at = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
                if at + 1000 >= after {
                    files.push((at, p));
                }
            }
        }
    }
    files.sort();
    for (_, p) in files.iter().rev() {
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        if let Some(found) = codex_said(&text, cwd, after) {
            return found;
        }
    }
    None
}

/// Of one of Codex's records, if it is that folder's: the answer that ended
/// the turn the last task through a wire began (after `after`), if it has ended.
fn codex_said(text: &str, cwd: &str, after: u64) -> Option<Option<String>> {
    let mut lines = text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok());
    if lines.next()?["payload"]["cwd"].as_str() != Some(cwd) {
        return None;
    }
    let (mut asked, mut answer) = (after == 0, None);
    for v in lines {
        let p = &v["payload"];
        if after > 0 && p["role"] == "user" && p["content"].as_array().is_some_and(|c| c.iter().any(|t| t["text"].as_str().is_some_and(|t| t.contains("through a wire]")))) {
            asked = true;
            answer = None;
        }
        if asked && p["type"] == "task_complete" && p["completed_at"].as_u64().is_some_and(|s| s * 1000 + 1000 >= after) {
            answer = p["last_agent_message"].as_str().map(str::to_owned);
        }
    }
    Some(answer.filter(|a| !a.trim().is_empty()))
}

/// opencode keeps its conversations in a database of its own, and says one
/// as JSON (`opencode export`): of the folder's last, the last message it
/// finished after the task, in its words.
fn opencode_answer(cwd: &str, after: u64) -> Option<String> {
    // Into a file: into a pipe, it stops at 64 KB (it leaves before the rest is read).
    let run = |args: &[&str]| -> Option<serde_json::Value> {
        let path = format!("{}/pleamar-link-opencode-{}.json", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()), std::process::id());
        let file = std::fs::File::create(&path).ok()?;
        let ran = std::process::Command::new("opencode").args(args).current_dir(cwd).stdin(std::process::Stdio::null()).stdout(file).stderr(std::process::Stdio::null()).status();
        let text = std::fs::read(&path);
        let _ = std::fs::remove_file(&path);
        ran.ok()?;
        serde_json::from_slice(&text.ok()?).ok()
    };
    let list = run(&["session", "list", "--format", "json"])?;
    let session = list.as_array()?.iter().filter(|s| s["directory"].as_str() == Some(cwd) && s["updated"].as_u64().is_some_and(|u| u + 1000 >= after)).max_by_key(|s| s["updated"].as_u64())?;
    let all = run(&["export", session["id"].as_str()?])?;
    let said = |m: &serde_json::Value| m["parts"].as_array().map(|ps| ps.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    let messages = all["messages"].as_array()?;
    let from = if after == 0 { 0 } else { messages.iter().rposition(|m| m["info"]["role"] == "user" && said(m).contains("through a wire]"))? };
    let last = messages[from..].iter().rev().find(|m| m["info"]["role"] == "assistant" && m["info"]["time"]["completed"].is_u64() && !said(m).trim().is_empty())?;
    Some(said(last).trim().to_owned())
}

/// How a wait ended.
enum Waited {
    /// At rest, asking nothing.
    Done,
    /// It stopped to ask its person something (a permission, a choice).
    Asks(crate::term::Question),
    /// Its terminal is gone.
    Gone,
    /// Still at it when the time was up.
    Late,
}

/// The question on that terminal's screen, if its agent is not working.
/// Its hooks may say it asks before the screen can be read as a question.
fn asked(id: i32, i: &Info) -> Option<crate::term::Question> {
    let hook = i.get("hook").map(String::as_str).unwrap_or("");
    if hook == "working" || (hook.is_empty() && i.get("state").map(String::as_str) == Some("working")) {
        return None;
    }
    let screen = read(id, 40);
    crate::term::question(&screen).or_else(|| (hook == "asking").then(|| crate::term::Question { text: format!("it asks its person something its screen shows: {}", screen.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim()), options: Vec::new(), cursor: 0, yes_no: false }))
}

/// How many answers its hooks have told its layer, and the last one.
fn hooked_answer(id: i32) -> (u64, String) {
    let text = crate::term::ask(&socket(id), "answer").unwrap_or_default();
    let (n, rest) = text.split_once('\n').unwrap_or((text.as_str(), ""));
    (n.trim().parse().unwrap_or(0), rest.trim_end().to_owned())
}

/// Waits for that terminal to be done —at rest twice in a row, asking
/// nothing: a title that blinks between frames is not an end—, or, with
/// `asks`, to stop on a question (seen twice too).
fn wait_for(id: i32, timeout: Duration, asks: bool) -> Waited {
    let start = Instant::now();
    let (mut rest, mut asking) = (0, 0);
    // At rest with commands of its own still running (it waits for them), since
    // when; and whether it still had them, looked at every few seconds. Not for
    // ever: a server it left running ends nothing.
    let mut held: Option<Instant> = None;
    let mut looked: Option<(Instant, bool)> = None;
    while start.elapsed() < timeout {
        let i = info(id);
        if i.is_empty() {
            return Waited::Gone;
        }
        match asked(id, &i) {
            Some(q) => {
                rest = 0;
                asking += 1;
                if asks && asking >= 2 {
                    return Waited::Asks(q);
                }
            }
            None => {
                asking = 0;
                let mut still = at_rest(&i);
                if still {
                    let waits = match looked {
                        Some((at, w)) if at.elapsed() < Duration::from_secs(5) => w,
                        _ => {
                            let w = in_background(id, &i, HANDED_AT.with(|h| h.get()));
                            looked = Some((Instant::now(), w));
                            w
                        }
                    };
                    if waits && held.get_or_insert_with(Instant::now).elapsed() < Duration::from_secs(15 * 60) {
                        still = false;
                    }
                } else {
                    (held, looked) = (None, None);
                }
                rest = if still { rest + 1 } else { 0 };
                if rest >= 2 {
                    return Waited::Done;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Waited::Late
}

/// A question, said so an agent can answer it.
fn said(slot: usize, q: &crate::term::Question) -> String {
    let mut out = format!("window {slot} asks: {}\n", q.text);
    if q.yes_no {
        out.push_str(&format!("Answer: pleamar-wm link answer {slot} yes|no"));
    } else {
        for (k, (n, t)) in q.options.iter().enumerate() {
            out.push_str(&format!("  {n}) {t}{}\n", if k == q.cursor { "   ← selected now" } else { "" }));
        }
        out.push_str(&format!("Answer: pleamar-wm link answer {slot} NUMBER (or the words of an option; free words go where it lets you type)"));
    }
    out
}

/// What has come out since the task was handed: after the line that
/// says what it is (the last one, if it handed several), or since a reading
/// `before` had that many lines.
fn since(id: i32, before: usize) -> String {
    if let Some(n) = HANDED.with(|h| h.get()) {
        let (now, text) = hooked_answer(id);
        if now > n && !text.is_empty() {
            return text;
        }
    }
    let i = info(id);
    if let Some(text) = kept_answer(&i, HANDED_AT.with(|h| h.get())) {
        return text;
    }
    let all = read(id, 2000);
    let lines: Vec<&str> = all.lines().collect();
    // A full-screen agent has no history: what is not on its screen is gone, and
    // `before` counted lines of a screen that has been drawn again since.
    let from = match handed_at(&lines) {
        Some(k) => k + 1,
        None if i.get("fullscreen").map(String::as_str) == Some("1") => 0,
        None => before.saturating_sub(3).min(lines.len()),
    };
    answer(&lines[from..])
}

/// The last line where the line that says what a task is ends: its
/// «through a wire]», even when the agent wrapped it over two or three lines
/// (Codex leaves «through a» on one and «wire]» on the next).
fn handed_at(lines: &[&str]) -> Option<usize> {
    let bare = |ls: &[&str]| ls.concat().chars().filter(|c| !c.is_whitespace()).collect::<String>();
    (0..lines.len()).rev().find(|&k| lines[k].contains(']') && bare(&lines[k.saturating_sub(2)..=k]).contains("throughawire]"))
}

/// Of what came out after the task, the answer: from the agent's first
/// message (Claude Code's `●`/`⏺`, Codex's `•`) if it marks them, and
/// without what an agent draws under it —its input box between two rules,
/// or Codex's prompt with no rules and the lines under it, «done in 7s»,
/// «Worked for 2s»—. opencode marks nothing, but boxes what it is told and
/// its input (`┃`): its answer is what comes between them, without how long
/// it thought («+ Thought: 526ms») and the line that says who answered and
/// in how long («Build  DeepSeek V4.1 Flash Free   2.0s»). At most 80 lines.
fn answer(lines: &[&str]) -> String {
    let rule = |l: &str| {
        let t = l.trim();
        t.chars().count() >= 20 && t.chars().all(|c| matches!(c, '─' | '━' | '-' | '═' | '╌'))
    };
    // Its input box: the first rule after which, before another rule, its prompt is.
    let prompt = |l: &str| matches!(l.trim_start().chars().next(), Some('❯' | '›' | '>'));
    let mut end = lines.len();
    for (k, l) in lines.iter().enumerate() {
        if rule(l) && lines[k + 1..].iter().take_while(|n| !rule(n)).any(|n| prompt(n)) {
            end = k;
            break;
        }
    }
    let marked = |l: &str| matches!(l.trim_start().chars().next(), Some('●' | '⏺' | '•'));
    let start = lines[..end].iter().position(|l| marked(l));
    // Codex draws its prompt (`› Ask Codex to do anything`) with no rules round it.
    if let Some(s) = start {
        if let Some(p) = lines[s..end].iter().position(|l| matches!(l.chars().next(), Some('›' | '❯'))) {
            end = s + p;
        }
    }
    let boxed = |l: &str| matches!(l.trim_start().chars().next(), Some('┃' | '╹' | '╻'));
    let mut from = start.unwrap_or(0);
    if start.is_none() && lines[..end].iter().any(|l| boxed(l)) {
        // opencode: past the box of what it was told, up to its input's.
        from = lines[..end].iter().position(|l| !boxed(l) && !l.trim().is_empty()).unwrap_or(end);
        end = lines[from..end].iter().position(|l| boxed(l)).map_or(end, |k| from + k);
    }
    let took = |l: &str| {
        let last = l.split_whitespace().last().unwrap_or("");
        l.split_whitespace().count() >= 3 && last.ends_with('s') && last.trim_end_matches(['m', 's']).parse::<f64>().is_ok()
    };
    let mut kept: Vec<&str> = lines[from..end].iter().map(|l| l.trim_end()).filter(|l| !l.trim_start().starts_with("+ Thought")).skip_while(|l| l.is_empty()).collect();
    // Below the answer: blank lines, and how long it thought (`✻ Cogitated for 7s · done 20:25`,
    // Codex's `─ Worked for 2s ───`).
    while kept.last().is_some_and(|l| {
        let t = l.trim_start();
        t.is_empty() || t.starts_with("Worked for") || t.contains("Restart to update") || t.contains("? for shortcuts") || t.contains("ctrl+p commands") || matches!(t.chars().next(), Some('✻' | '✳' | '✶' | '✽' | '✢' | '·' | '─')) || (start.is_none() && took(t))
    }) {
        kept.pop();
    }
    let keep = kept.len().saturating_sub(80);
    kept[keep..].join("\n")
}

fn flow(k: usize, v: i32) {
    say(&[format!("fact wire.{k}.flow {v}")]);
}

fn number(args: &[String], flag: &str, default: u64) -> u64 {
    args.iter().position(|a| a == flag).and_then(|p| args.get(p + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn words(args: &[String]) -> String {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        match a.as_str() {
            "--timeout" | "--lines" => skip = true,
            "--enter" | "--wait" => {}
            _ => out.push(a.as_str()),
        }
    }
    out.join(" ")
}

pub fn run(args: &[String]) -> i32 {
    match go(args) {
        Ok(out) => {
            if !out.is_empty() {
                println!("{}", out.trim_end());
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

const USAGE: &str = "pleamar-wm link — the windows wired to yours (on a canvas, the person wires them)
  whoami                          which window you are, and what is wired to it
  spawn AGENT TASK [--folder D] [--name NAME] [--model M]
                                  a new terminal beside yours, wired to it, with that agent
                                  (claude, codex…) started and TASK handed to it
  setup [--yes] [--undo]          lets your agents use wires by themselves: the skill, the MCP
                                  server (`link mcp`) for Claude Code and Codex, Claude's hooks
  label N NAME                    names the wire to N («tests»): shown on it, and N can be NAME
  list                            the windows wired to yours: number, title, what runs there
  read N [LINES]                  the last lines of its terminal (200)
  send N TEXT [--enter]           TEXT typed into it as a paste; --enter presses Enter after
  keys N KEY…                     keys: enter tab escape up down left right backspace ctrl+c y 1 …
  wait N [--timeout S]            until its agent is done (ready again), then its answer; or what
                                  it stopped to ask
  ask N                           what it asks its person now, if anything: the question and options
  answer N NUMBER|WORDS|yes|no    answers that: an option by number or words, words where it lets
                                  you type them (the person sees on it who answered)
  delegate N TASK [--wait] [--timeout S]
                                  hands TASK to the agent there. Without --wait it returns at
                                  once, and its answer —or a question it stops to ask on the
                                  way— is typed into your terminal; with --wait, it is what
                                  this prints.
N is a window's number from `list`, its wire's name (`tests`), or the agent that runs there
(`codex`) if only one is.";

/// Which window the caller is and what is wired to it: what an agent is told
/// when it starts (`link mcp`).
pub(crate) fn briefing() -> Result<String, String> {
    go(&["whoami".to_owned()])
}

pub(crate) fn go(args: &[String]) -> Result<String, String> {
    let verb = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or(&[]);
    match verb {
        "whoami" => {
            let h = here()?;
            let me = h.wins.iter().find(|w| w.slot == h.slot).map(|w| w.title.clone()).unwrap_or_default();
            let agent = h.info.get("agent").filter(|a| !a.is_empty()).cloned().unwrap_or_else(|| "a shell".into());
            let mut out = format!("You are window {} «{me}»: {agent} in {} (terminal {}).\n", h.slot, tilde(h.info.get("cwd").map_or("", String::as_str)), h.layer);
            let wired = h.wired();
            if wired.is_empty() {
                out.push_str("No window is wired to yours.");
            } else {
                out.push_str("Wired to it:\n");
                for (k, w, _) in wired {
                    out.push_str(&format!("  {}\n", line_of(w.slot, w, layer_of(w.pid, &h.layers).as_ref(), h.label(k))));
                }
            }
            Ok(out)
        }
        "list" => {
            let h = here()?;
            let wired = h.wired();
            if wired.is_empty() {
                return Ok("No window is wired to yours. The person wires them: on a free monitor (Super+W), dragging the drop at the start of a window's title bar onto another window.".into());
            }
            Ok(wired.iter().map(|(k, w, _)| line_of(w.slot, w, layer_of(w.pid, &h.layers).as_ref(), h.label(*k))).collect::<Vec<_>>().join("\n"))
        }
        // The wire to that window, named («tests»): shown on it, and a name to give it.
        "label" => {
            let name = rest.first().ok_or(USAGE)?;
            let h = here()?;
            let (k, w, _, _) = h.target(name)?;
            let text = words(&rest[1..]).replace(['\n', '\t'], " ");
            say(&[format!("text wire.{k}.label {text}")]);
            Ok(if text.is_empty() { format!("the wire to window {} has no name now", w.slot) } else { format!("the wire to window {} is «{text}»", w.slot) })
        }
        "read" | "send" | "keys" | "wait" | "ask" | "answer" | "delegate" => {
            let name = rest.first().ok_or(USAGE)?;
            let h = here()?;
            let (k, w, mine_is_a, layer) = h.target(name)?;
            let (id, tinfo) = layer.ok_or_else(|| format!("window {} «{}» has no terminal of its own (through pleamar-wm's layer) to read or type in", w.slot, w.title))?;
            match verb {
                "read" => Ok(read(id, rest.get(1).and_then(|n| n.parse().ok()).unwrap_or(200))),
                "send" => {
                    let text = words(&rest[1..]);
                    if text.is_empty() {
                        return Err("send N TEXT [--enter]".into());
                    }
                    paste(id, &text)?;
                    if rest.iter().any(|a| a == "--enter") {
                        std::thread::sleep(Duration::from_millis(150));
                        keys(id, "enter")?;
                    }
                    Ok(format!("typed into window {}", w.slot))
                }
                "keys" => {
                    keys(id, &rest[1..].join(" "))?;
                    std::thread::sleep(Duration::from_millis(300));
                    Ok(read(id, 30))
                }
                "wait" => {
                    let timeout = Duration::from_secs(number(rest, "--timeout", 600));
                    match wait_for(id, timeout, true) {
                        Waited::Done => {
                            taken(id);
                            // As the one who handed it the last task would bring it: what its
                            // hooks or its record kept since, whole; its screen only failing those.
                            let (at, n) = handed(id);
                            HANDED.with(|c| c.set(Some(n)));
                            HANDED_AT.with(|c| c.set(at));
                            Ok(since(id, read(id, 2000).lines().count().saturating_sub(120)))
                        }
                        Waited::Asks(q) => Ok(said(w.slot, &q)),
                        Waited::Gone => Err(format!("window {}'s terminal is gone", w.slot)),
                        Waited::Late => Err(format!("window {} is still at it after {} s", w.slot, timeout.as_secs())),
                    }
                }
                "ask" => {
                    let i = info(id);
                    Ok(match asked(id, &i) {
                        Some(q) => said(w.slot, &q),
                        None => format!("window {} asks nothing now", w.slot),
                    })
                }
                "answer" => {
                    let reply = words(&rest[1..]);
                    if reply.is_empty() {
                        return Err("answer N NUMBER|yes|no|WORDS".into());
                    }
                    let i = info(id);
                    let q = asked(id, &i).ok_or_else(|| format!("window {} asks nothing now (`send` types in it)", w.slot))?;
                    let chose = choose(id, &q, &reply).map_err(|e| format!("{e}\n{}", said(w.slot, &q)))?;
                    // Said on it a while: whose answer this was.
                    say(&[format!("emit answered {}", w.slot * 100 + h.slot + 1)]);
                    std::thread::sleep(Duration::from_millis(700));
                    Ok(format!("answered window {}: {chose}", w.slot))
                }
                _ => {
                    let task = words(&rest[1..]);
                    if task.is_empty() {
                        return Err("delegate N TASK [--wait] [--timeout S]".into());
                    }
                    if tinfo.get("agent").is_none_or(|a| a.is_empty()) {
                        return Err(format!("window {} runs no agent ({}): `send` types in it instead", w.slot, tinfo.get("command").cloned().unwrap_or_default()));
                    }
                    if tinfo.get("state").map(String::as_str) == Some("working") {
                        return Err(format!("window {}'s agent is busy: `wait {}` until it is done", w.slot, w.slot));
                    }
                    // Over a question, the task's Enter would answer it.
                    if let Some(q) = asked(id, &tinfo) {
                        return Err(format!("{}\nNothing was handed: it asks that first. Answer it (`pleamar-wm link answer {}`), then hand the task.", said(w.slot, &q), w.slot));
                    }
                    let me = h.wins.iter().find(|w| w.slot == h.slot).map(|w| w.title.clone()).unwrap_or_default();
                    let who = h.info.get("agent").filter(|a| !a.is_empty()).cloned().unwrap_or_else(|| "a shell".into());
                    let before = read(id, 2000).lines().count();
                    HANDED.with(|c| c.set(Some(hooked_answer(id).0)));
                    HANDED_AT.with(|c| c.set(now_ms()));
                    // A task handed there before this one is not brought back by itself.
                    let _ = std::fs::write(format!("{}/{id}.handed", crate::term::socket_dir()), HANDED_AT.with(|c| c.get()).to_string());
                    let _ = std::fs::write(format!("{}/{id}.handed-answers", crate::term::socket_dir()), HANDED.with(|c| c.get()).unwrap_or(0).to_string());
                    // The drop runs along the wire, towards it, while it works.
                    flow(k, if mine_is_a { 1 } else { -1 });
                    // A line that says what this is, as a paste of its own (a long
                    // paste is taken as data, not as something asked), the task
                    // after it, and Enter.
                    paste(id, &format!("[a task from {who} in window {} «{me}», through a wire] It is in the pasted text: do it, and end with your answer.", h.slot))?;
                    std::thread::sleep(Duration::from_millis(120));
                    paste(id, &task)?;
                    std::thread::sleep(Duration::from_millis(150));
                    keys(id, "enter")?;
                    // Taken? Its agent starts working (or something comes out).
                    let started = (0..10).any(|_| {
                        std::thread::sleep(Duration::from_millis(400));
                        let i = info(id);
                        i.get("state").map(String::as_str) == Some("working") || i.get("quiet").and_then(|q| q.parse::<u64>().ok()).is_some_and(|q| q < 300)
                    });
                    if !started {
                        keys(id, "enter")?;
                    }
                    // Waited for here, ten minutes; brought later by a messenger of its own,
                    // six hours: a task of an agent can take an hour, and the answer of one that
                    // outlasted the messenger was never brought (it said «did not finish» and left).
                    let waits_here = rest.iter().any(|a| a == "--wait") || !h.info.get("agent").is_some_and(|a| !a.is_empty());
                    let timeout = number(rest, "--timeout", if waits_here { 600 } else { 6 * 3600 });
                    // A shell has nowhere to be told the answer later (it would
                    // run it): it waits for it.
                    let caller_agent = h.info.get("agent").is_some_and(|a| !a.is_empty());
                    if rest.iter().any(|a| a == "--wait") || !caller_agent {
                        return match wait_for(id, Duration::from_secs(timeout), true) {
                            Waited::Done => {
                                back(k, mine_is_a);
                                Ok(since(id, before))
                            }
                            Waited::Asks(q) => {
                                flow(k, 0);
                                Ok(format!("{}\nIt stopped to ask that while on your task. Once answered, `pleamar-wm link wait {}` brings its answer.", said(w.slot, &q), w.slot))
                            }
                            Waited::Gone => Err(format!("window {}'s terminal went away", w.slot)),
                            Waited::Late => {
                                flow(k, 0);
                                Err(format!("window {} did not finish in {timeout} s; so far:\n{}", w.slot, since(id, before)))
                            }
                        };
                    }
                    // Not waited for: a process of its own waits, and types the
                    // answer into this terminal when it is there.
                    let me_exe = own_exe();
                    let mut c = std::process::Command::new("setsid");
                    c.arg("-f").arg(me_exe).args(["link", "_deliver", &id.to_string(), &h.layer.to_string(), &k.to_string(), &(mine_is_a as u8).to_string(), &before.to_string(), &timeout.to_string(), &w.slot.to_string(), &HANDED.with(|c| c.get()).unwrap_or(u64::MAX).to_string(), &HANDED_AT.with(|c| c.get()).to_string()]);
                    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                    c.spawn().map_err(|e| e.to_string())?;
                    Ok(format!("Handed to window {} «{}». Its answer will be typed into this terminal when it is done; or `pleamar-wm link wait {}`.", w.slot, w.title, w.slot))
                }
            }
        }
        // (The process `delegate` leaves to bring the answer back.)
        "_deliver" => {
            let n = |i: usize| rest.get(i).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            let (id, mine, k, mine_is_a, before, timeout, slot) = (n(0) as i32, n(1) as i32, n(2) as usize, n(3) == 1, n(4) as usize, n(5) as u64, n(6));
            HANDED.with(|c| c.set(rest.get(7).and_then(|v| v.parse().ok())));
            HANDED_AT.with(|c| c.set(rest.get(8).and_then(|v| v.parse().ok()).unwrap_or(0)));
            let deadline = Instant::now() + Duration::from_secs(timeout);
            let mut told = String::new();
            let done = loop {
                match wait_for(id, deadline.saturating_duration_since(Instant::now()), true) {
                    Waited::Done => break true,
                    Waited::Gone | Waited::Late => break false,
                    // It stopped to ask its person: the one who handed it the
                    // task is told (once for each question), and may answer.
                    Waited::Asks(q) => {
                        if q.text != told || told.is_empty() {
                            told = q.text.clone();
                            tell(mine, &format!("[window {slot} stopped to ask something while on the task you handed it] It is in the pasted text."), &said(slot as usize, &q))?;
                        }
                        // Until it is answered (by anyone), or the time is up.
                        while Instant::now() < deadline && asked(id, &info(id)).is_some_and(|n| n.text == told) {
                            std::thread::sleep(Duration::from_millis(700));
                        }
                    }
                }
            };
            back(k, mine_is_a);
            // Already brought by `wait`, or another task handed there since (whose
            // answer is the one the agent's record ends with): not typed in.
            let at = HANDED_AT.with(|h| h.get());
            let later = std::fs::read_to_string(format!("{}/{id}.handed", crate::term::socket_dir())).ok().and_then(|t| t.trim().parse::<u64>().ok()).is_some_and(|t| t > at);
            if taken_since(id, at) || later {
                return Ok(String::new());
            }
            let answer = since(id, before);
            let head = if done { format!("[the answer from window {slot}, to the task you handed it] It is in the pasted text.") } else { format!("[window {slot} did not finish your task in {timeout} s] What it said so far is in the pasted text.") };
            tell(mine, &head, &answer)?;
            Ok(String::new())
        }
        // (The process `spawn` leaves to hand the task once what it asked first is answered.)
        "_handover" => {
            let slot: usize = rest.first().and_then(|v| v.parse().ok()).ok_or(USAGE)?;
            let id: i32 = rest.get(1).and_then(|v| v.parse().ok()).ok_or(USAGE)?;
            hand_over(slot, id, rest.get(2).map_or("", String::as_str), rest.get(3).map_or("", String::as_str)).map(|_| String::new())
        }
        "mcp" => {
            crate::mcp::run();
            Ok(String::new())
        }
        "hook" => {
            hook(rest.first().map_or("", String::as_str));
            Ok(String::new())
        }
        "setup" => setup(rest.iter().any(|a| a == "--yes"), rest.iter().any(|a| a == "--undo")),
        "spawn" => spawn(rest),
        "restore" => crate::keep::restore(rest.iter().any(|a| a == "--dismiss")),
        _ => Err(USAGE.into()),
    }
}

/// `link hook claude`: what Claude Code's hooks say (JSON on stdin), told to
/// the layer of the terminal it runs in —how it is, and its last answer,
/// exactly—. Quiet and quick whatever happens: it never holds the agent.
fn hook(agent: &str) {
    let Some(layer) = std::env::var("PLEAMAR_TERMINAL").ok().filter(|v| !v.is_empty()) else { return };
    let mut input = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&input) else { return };
    let event = v.get("hook_event_name").and_then(|e| e.as_str()).unwrap_or("");
    let state = match (agent, event) {
        (_, "UserPromptSubmit" | "PreToolUse" | "PostToolUse") => "working",
        (_, "PermissionRequest") => "asking",
        (_, "Notification") => match v.get("notification_type").and_then(|t| t.as_str()).unwrap_or("") {
            "permission_prompt" | "elicitation_dialog" | "agent_needs_input" => "asking",
            "idle_prompt" => "done",
            _ => return,
        },
        (_, "Stop" | "SessionStart") => "done",
        _ => return,
    };
    let sock = format!("{}/{layer}.sock", crate::term::socket_dir());
    // Where it keeps this conversation (a new one after /clear or /resume).
    if let Some(path) = v.get("transcript_path").and_then(|t| t.as_str()).filter(|_| matches!(event, "SessionStart" | "UserPromptSubmit" | "Stop")) {
        let _ = std::fs::write(format!("{}/{layer}.transcript", crate::term::socket_dir()), path);
    }
    let _ = crate::term::ask(&sock, &format!("note state {state}"));
    if event == "Stop" {
        if let Some(text) = v.get("last_assistant_message").and_then(|t| t.as_str()).filter(|t| !t.trim().is_empty()) {
            let _ = crate::term::ask(&sock, &format!("note answer {}", hex(text.as_bytes())));
        }
    }
}

/// The skill that tells an agent about wires, for each agent installed.
const SKILL: &str = include_str!("../skill/pleamar-link/SKILL.md");

/// The events Claude Code tells its hooks of, that say how it is.
const CLAUDE_EVENTS: [&str; 7] = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PermissionRequest", "Notification", "Stop"];
const HOOK: &str = "pleamar-wm link hook claude";

/// `link setup`: what lets the agents use wires by themselves —the skill,
/// the MCP server (`link mcp`) for Claude Code and Codex, and Claude Code's
/// hooks (exact state and answers)—. Says what it will change and asks
/// first (`--yes` does not); `--undo` takes it all away.
fn setup(yes: bool, undo: bool) -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME")?;
    let has = |p: &str| std::path::Path::new(&format!("{home}/{p}")).is_dir();
    let on_path = |c: &str| std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(c).is_file()));
    let claude = has(".claude") && on_path("claude");
    let codex = has(".codex") && on_path("codex");
    let mut plan: Vec<String> = Vec::new();
    for (ok, dir) in [(has(".claude"), ".claude/skills/pleamar-link"), (has(".codex"), ".codex/skills/pleamar-link")] {
        if ok {
            plan.push(format!("{} the skill ~/{dir}/SKILL.md", if undo { "remove" } else { "write" }));
        }
    }
    if claude {
        plan.push(format!("{} the MCP server «pleamar-link» in Claude Code (user scope): `claude mcp {}`", if undo { "remove" } else { "register" }, if undo { "remove" } else { "add" }));
        plan.push(format!("{} Claude Code's hooks for wires in ~/.claude/settings.json (a copy of it is kept as settings.json.before-pleamar-link)", if undo { "take away" } else { "add" }));
    }
    if codex {
        plan.push(format!("{} the MCP server «pleamar-link» in Codex: `codex mcp {}`", if undo { "remove" } else { "register" }, if undo { "remove" } else { "add" }));
    }
    if plan.is_empty() {
        return Err("no agent found to set up (Claude Code: ~/.claude and `claude`; Codex: ~/.codex and `codex`)".into());
    }
    println!("pleamar-wm link setup will:\n  - {}", plan.join("\n  - "));
    if !yes {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return Err("run it in a terminal to answer, or with --yes".into());
        }
        print!("Go ahead? [y/N] ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut a = String::new();
        let _ = std::io::stdin().read_line(&mut a);
        if !matches!(a.trim(), "y" | "Y" | "s" | "S" | "yes" | "si" | "sí") {
            return Ok("nothing changed".into());
        }
    }
    let mut done: Vec<String> = Vec::new();
    for (ok, dir) in [(has(".claude"), format!("{home}/.claude/skills/pleamar-link")), (has(".codex"), format!("{home}/.codex/skills/pleamar-link"))] {
        if !ok {
            continue;
        }
        if undo {
            let _ = std::fs::remove_file(format!("{dir}/SKILL.md"));
            let _ = std::fs::remove_dir(&dir);
        } else {
            std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(format!("{dir}/SKILL.md"), SKILL)).map_err(|e| format!("{dir}: {e}"))?;
        }
        done.push(format!("skill {dir}"));
    }
    let quiet = |c: &mut std::process::Command| c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    if claude {
        quiet(std::process::Command::new("claude").args(["mcp", "remove", "--scope", "user", "pleamar-link"]));
        if !undo {
            if !quiet(std::process::Command::new("claude").args(["mcp", "add", "--scope", "user", "pleamar-link", "--", "pleamar-wm", "link", "mcp"])) {
                return Err("`claude mcp add` did not go through".into());
            }
        }
        done.push("Claude Code's MCP server".into());
        claude_hooks(&format!("{home}/.claude/settings.json"), undo)?;
        done.push("Claude Code's hooks".into());
    }
    if codex {
        quiet(std::process::Command::new("codex").args(["mcp", "remove", "pleamar-link"]));
        if !undo && !quiet(std::process::Command::new("codex").args(["mcp", "add", "pleamar-link", "--", "pleamar-wm", "link", "mcp"])) {
            return Err("`codex mcp add` did not go through".into());
        }
        done.push("Codex's MCP server".into());
    }
    Ok(format!(
        "{}: {}.{}",
        if undo { "taken away" } else { "set up" },
        done.join(", "),
        if undo { "" } else { " Agents started from now on know about wires; restart the ones already open." }
    ))
}

/// Claude Code's settings, with this program's hooks for wires in them (or
/// without, `undo`): every other setting and hook as it was.
fn claude_hooks(path: &str, undo: bool) -> Result<(), String> {
    use serde_json::{json, Value};
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut v: Value = serde_json::from_str(&text).map_err(|e| format!("{path} is not JSON I can read ({e}): nothing changed"))?;
    if std::path::Path::new(path).exists() && !std::path::Path::new(&format!("{path}.before-pleamar-link")).exists() {
        std::fs::copy(path, format!("{path}.before-pleamar-link")).map_err(|e| e.to_string())?;
    }
    let root = v.as_object_mut().ok_or("settings.json is not an object")?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("its hooks are not an object")?;
    for event in CLAUDE_EVENTS {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        let Some(list) = list.as_array_mut() else { continue };
        // Ours out first (any left from before), then in again unless undone.
        list.retain(|g| !g.get("hooks").and_then(Value::as_array).is_some_and(|hs| hs.iter().any(|h| h.get("command").and_then(Value::as_str) == Some(HOOK))));
        if !undo {
            list.push(json!({ "hooks": [{ "type": "command", "command": HOOK, "timeout": 5 }] }));
        }
    }
    hooks.retain(|_, l| l.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        root.remove("hooks");
    }
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(format!("{path}.new"), out + "\n").and_then(|_| std::fs::rename(format!("{path}.new"), path)).map_err(|e| format!("{path}: {e}"))
}

/// The process of the window in that place.
pub(crate) fn pid_of_slot(slot: usize) -> Option<u32> {
    windows().ok()?.into_iter().find(|w| w.slot == slot).map(|w| w.pid)
}

/// A program the session starts as it starts its own: loose from the caller.
/// `quiet`: its window opens without taking the person's keyboard.
fn start(exe: &str, cwd: &str, quiet: bool) -> Result<u32, String> {
    let mut c = std::process::Command::new(exe);
    c.current_dir(cwd).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // Not as the caller's terminal: the new one gets a layer of its own. Inside
    // the caller's layer `SHELL` is the real shell again; the new terminal
    // starts the session's link to it, as one the session starts does.
    c.env_remove("PLEAMAR_TERMINAL").env_remove("PLEAMAR_TTY");
    // Nor as a child of the caller's agent: what it marks its own commands
    // with (a Claude Code started under it saves no transcript of its own).
    for (k, _) in std::env::vars_os() {
        let name = k.to_string_lossy();
        if agent_marker(&name) {
            c.env_remove(&k);
        }
    }
    if let Some(link) = std::env::var("PLEAMAR_REAL_SHELL").ok().and_then(|real| crate::term::link_of(&real)) {
        c.env("SHELL", link);
    }
    if quiet {
        c.env(crate::term::QUIET, "1");
    }
    use std::os::unix::process::CommandExt;
    c.process_group(0);
    c.spawn().map(|ch| ch.id()).map_err(|e| format!("{exe}: {e}"))
}

/// What an agent puts in the environment of the commands it runs, so they know
/// they run under it; not what the person sets (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`).
fn agent_marker(name: &str) -> bool {
    name.starts_with("CLAUDE_CODE_") || matches!(name, "CLAUDECODE" | "CLAUDE_PID" | "CLAUDE_EFFORT" | "AI_AGENT" | "CODEX_SANDBOX" | "CODEX_SANDBOX_NETWORK_DISABLED" | "CODEX_THREAD_ID" | "OPENCODE" | "OPENCODE_PID")
}

/// A terminal opened for an agent: the same program as the caller's (kitty,
/// Alacritty…), in that folder. Its window's place and its layer, once both
/// are there. `quiet`: the keyboard stays where the person has it.
pub(crate) fn open_terminal(exe: &str, cwd: &str, quiet: bool) -> Result<(usize, i32), String> {
    let before: Vec<usize> = windows()?.iter().map(|w| w.slot).collect();
    let pid = start(exe, cwd, quiet)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        let wins = windows()?;
        // Its own window, or —a terminal that hands windows to a server of
        // its own— the new one that opened.
        let new = wins.iter().find(|w| w.pid == pid).or_else(|| wins.iter().find(|w| !before.contains(&w.slot)));
        let Some(w) = new else { continue };
        if let Some((id, _)) = layer_of(w.pid, &crate::term::layers()) {
            return Ok((w.slot, id));
        }
    }
    Err(format!("{exe} opened no window with a terminal of its own in 20 s"))
}

/// `link spawn AGENT TASK [--folder D] [--name L] [--model M]`: a new terminal
/// beside the caller's window, wired to it, with that agent started (on that
/// model), and the task handed as with `delegate`.
fn spawn(rest: &[String]) -> Result<String, String> {
    let agent = rest.first().ok_or("spawn AGENT TASK [--folder DIR] [--name NAME] [--model MODEL]")?.to_lowercase();
    let flag = |f: &str| rest.iter().position(|a| a == f).and_then(|p| rest.get(p + 1)).cloned();
    let mut words: Vec<&str> = Vec::new();
    let mut skip = false;
    for a in &rest[1..] {
        if skip {
            skip = false;
            continue;
        }
        if a == "--folder" || a == "--name" || a == "--model" {
            skip = true;
            continue;
        }
        words.push(a);
    }
    let task = words.join(" ");
    if task.is_empty() {
        return Err("spawn AGENT TASK [--folder DIR] [--name NAME] [--model MODEL]".into());
    }
    let command = match agent.as_str() {
        "claude" | "claude-code" => "claude",
        "codex" => "codex",
        "gemini" => "gemini",
        "opencode" => "opencode",
        "qwen" => "qwen",
        "copilot" => "copilot",
        other => return Err(format!("«{other}»: claude, codex, gemini, opencode, qwen or copilot")),
    };
    // Typed at a shell: a model's name is letters, digits and `-._/:` only.
    let command = match flag("--model") {
        Some(m) if !m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric() || "-._/:".contains(c)) => format!("{command} {} {m}", if matches!(command, "claude" | "copilot") { "--model" } else { "-m" }),
        Some(m) => return Err(format!("«{m}» is not a model's name")),
        None => command.to_owned(),
    };
    let h = here()?;
    let cwd = flag("--folder").map(|f| f.replacen('~', &std::env::var("HOME").unwrap_or_default(), 1)).unwrap_or_else(|| h.info.get("cwd").cloned().unwrap_or_else(|| ".".into()));
    let terminal = h.info.get("terminal").and_then(|t| t.parse::<u32>().ok()).ok_or("this terminal's program is not known")?;
    let exe = std::fs::read_link(format!("/proc/{terminal}/exe")).map(|p| p.display().to_string()).map_err(|e| format!("this terminal's program: {e}"))?;
    // Beside the caller, for the caller: the person goes on typing where they were.
    let (slot, id) = open_terminal(&exe, &cwd, true)?;
    // Beside the caller's window, on the plane, as big.
    let place: Vec<f64> = say(&[format!("get fx.{}", h.slot), format!("get fy.{}", h.slot), format!("get fw.{}", h.slot), format!("get fh.{}", h.slot)]).iter().filter_map(|v| v.trim().parse().ok()).collect();
    if let [x, y, w, hh] = place[..] {
        if w > 1.0 {
            say(&[format!("fact fx.{slot} {}", x + w + 40.0), format!("fact fy.{slot} {y}"), format!("fact fw.{slot} {w}"), format!("fact fh.{slot} {hh}")]);
        }
    }
    // Wired, in the first free place.
    let taken: Vec<f64> = say(&(0..8).map(|k| format!("get wire.{k}.a")).collect::<Vec<_>>()).iter().filter_map(|v| v.trim().parse().ok()).collect();
    if taken.len() < 8 {
        return Err("the desktop does not say which wires there are (is this pleamar-wm's session?)".into());
    }
    let k = taken.iter().position(|a| *a < 0.0).ok_or("eight wires already: cut one first")?;
    let mut orders = vec![format!("fact wire.{k}.a {}", h.slot), format!("fact wire.{k}.b {slot}")];
    if let Some(name) = flag("--name") {
        orders.push(format!("text wire.{k}.label {name}"));
    }
    say(&orders);
    // The agent, and once it is ready, the task.
    paste(id, &command)?;
    std::thread::sleep(Duration::from_millis(120));
    keys(id, "enter")?;
    let opened = format!("Opened window {slot} with {command} in {}, wired to yours.", tilde(&cwd));
    match ready_for(id, Duration::from_secs(60), "") {
        Ready::Yes => {
            let handed = go(&["delegate".to_owned(), slot.to_string(), task])?;
            Ok(format!("{opened} {handed}"))
        }
        // Not answered here (whether to trust a folder is the person's, or the
        // caller's to ask them): told, and the task handed once it is answered.
        Ready::Asks(q) => {
            let next = "Answer it (link_answer); your task is handed to it by itself once it is ready, and anything else it asks first is typed into your terminal.";
            if h.info.get("agent").is_some_and(|a| !a.is_empty()) {
                let mut c = std::process::Command::new("setsid");
                c.arg("-f").arg(own_exe()).args(["link", "_handover", &slot.to_string(), &id.to_string(), &q.text, &task]);
                c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                c.spawn().map_err(|e| e.to_string())?;
                Ok(format!("{opened} Before your task it asks:\n{}\n{next}", said(slot, &q)))
            } else {
                eprintln!("{opened} Before the task it asks:\n{}\nAnswer it there or with `pleamar-wm link answer {slot}`; the task is handed once it is ready.", said(slot, &q));
                hand_over(slot, id, &q.text, &task)
            }
        }
        Ready::Gone(why) => Err(format!("{opened} But {why}: your task was not handed.")),
        Ready::Late => Err(format!("{opened} But it did not get ready in 60 s: hand it the task with link_delegate {slot} when it is")),
    }
}

/// How an agent just started came to be.
enum Ready {
    /// In its input box, waiting for a task.
    Yes,
    /// Asking its person something first (whether to trust the folder…).
    Asks(crate::term::Question),
    /// It left, or its terminal did.
    Gone(String),
    Late,
}

/// Waits for the agent started in that terminal to be in its input box, or to
/// ask something other than `told` (seen twice, so as not to read a screen
/// half drawn), or to leave.
fn ready_for(id: i32, timeout: Duration, told: &str) -> Ready {
    let start = Instant::now();
    // Something told was asked by it: it was there.
    let (mut seen, mut asking, mut ready) = (!told.is_empty(), 0, 0);
    while start.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(600));
        let i = info(id);
        if i.is_empty() {
            return Ready::Gone("its terminal went away".into());
        }
        let agent = i.get("agent").cloned().unwrap_or_default();
        if agent.is_empty() {
            // Not started yet, or gone back to its shell.
            if seen && i.get("quiet").and_then(|q| q.parse::<u64>().ok()).is_some_and(|q| q > 1500) {
                return Ready::Gone("its agent left before it was ready".into());
            }
            continue;
        }
        seen = true;
        match asked(id, &i) {
            Some(q) if q.text != told => {
                ready = 0;
                asking += 1;
                if asking >= 2 {
                    return Ready::Asks(q);
                }
            }
            Some(_) => (asking, ready) = (0, 0),
            None => {
                asking = 0;
                let screen = read(id, 40);
                // Its input box drawn: Claude Code's `❯` between two rules, Codex's `›`.
                let boxed = match agent.as_str() {
                    "claude" => claude_box(&screen).is_some(),
                    "codex" => screen.lines().any(|l| l.starts_with('›')),
                    _ => true,
                };
                let still = i.get("quiet").and_then(|q| q.parse::<u64>().ok()).is_some_and(|q| q > 1500);
                ready = if boxed && still && at_rest(&i) { ready + 1 } else { 0 };
                if ready >= 2 {
                    return Ready::Yes;
                }
            }
        }
    }
    Ready::Late
}

/// The task for an agent that asked something before it was ready, once
/// that is answered: handed to it as `delegate` would; what else it asks
/// first, typed into the caller's terminal. Not if it was handed another task
/// meanwhile (the caller did not wait), nor if it left.
fn hand_over(slot: usize, id: i32, told: &str, task: &str) -> Result<String, String> {
    let since = now_ms();
    let mine: i32 = std::env::var("PLEAMAR_TERMINAL").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut told = told.to_owned();
    let deadline = Instant::now() + Duration::from_secs(3600);
    loop {
        if handed(id).0 >= since {
            return Ok(format!("window {slot} was handed a task some other way: yours was not"));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match ready_for(id, left, &told) {
            Ready::Yes => return go(&["delegate".to_owned(), slot.to_string(), task.to_owned()]),
            Ready::Asks(q) => {
                told = q.text.clone();
                if mine > 0 {
                    tell(mine, &format!("[window {slot}, opened for your task, asks something more before it can take it] It is in the pasted text."), &said(slot, &q))?;
                }
            }
            Ready::Gone(why) => {
                let text = format!("window {slot}: {why}, and your task was not handed to it.");
                if mine > 0 {
                    tell(mine, &format!("[window {slot} did not take the task you gave it when you opened it] It is in the pasted text."), &text)?;
                }
                return Err(text);
            }
            Ready::Late => return Err(format!("window {slot} was not ready in an hour: your task was not handed")),
        }
    }
}

/// Into the asking agent's terminal, once it is free —not over its person
/// typing, nor into the middle of its own work—: a line, the text, Enter.
fn tell(mine: i32, head: &str, text: &str) -> Result<(), String> {
    // Claude Code, Codex and opencode take what is typed while they work, and
    // read it between two steps: there it only waits for its person not to be
    // in the middle of writing (and, in Claude Code's, for its box to be
    // empty). Any other agent, until it is done.
    // Never over what its person is writing, at rest or not: not in the middle
    // of a line, or —Claude Code— with its box empty (a draft rubbed out is
    // none). Only both: what Claude Code suggests in its empty box, in grey
    // (`❯ vale, avísame cuando conteste`), reads as written, and nobody wrote it.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3600) {
        let i = info(mine);
        if i.is_empty() {
            break;
        }
        let agent = i.get("agent").map(String::as_str).unwrap_or("");
        let free = i.get("draft").map(String::as_str) != Some("1") || (agent == "claude" && claude_box_empty(&read(mine, 12)));
        if free && (at_rest(&i) || matches!(agent, "claude" | "codex" | "opencode")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(700));
    }
    paste(mine, head)?;
    std::thread::sleep(Duration::from_millis(120));
    paste(mine, text)?;
    std::thread::sleep(Duration::from_millis(150));
    keys(mine, "enter")
}

/// Claude Code's input box, between its two rules: nothing written after its `❯`.
fn claude_box_empty(screen: &str) -> bool {
    claude_box(screen).is_some_and(str::is_empty)
}

/// What is written in Claude Code's input box (its `❯` between two rules), if it is drawn.
fn claude_box(screen: &str) -> Option<&str> {
    let lines: Vec<&str> = screen.lines().collect();
    let rule = |l: &str| l.trim().chars().count() >= 20 && l.trim().chars().all(|c| c == '─');
    let k = lines.iter().rposition(|l| l.trim_start().starts_with('❯'))?;
    (k > 0 && rule(lines[k - 1]) && lines.get(k + 1).is_some_and(|l| rule(l))).then(|| lines[k].trim_start().trim_start_matches('❯').trim())
}

/// That a task's answer was brought by `wait`, and when: the process that
/// would type it in later leaves it.
fn taken(id: i32) {
    let _ = std::fs::write(format!("{}/{id}.taken", crate::term::socket_dir()), now_ms().to_string());
}

fn taken_since(id: i32, at: u64) -> bool {
    at > 0 && std::fs::read_to_string(format!("{}/{id}.taken", crate::term::socket_dir())).ok().and_then(|t| t.trim().parse::<u64>().ok()).is_some_and(|t| t >= at)
}

/// Answers a question on that terminal: an option by its number or its
/// words (the cursor taken there, and Enter), words where it lets you type
/// them, or yes/no. What was chosen.
fn choose(id: i32, q: &crate::term::Question, reply: &str) -> Result<String, String> {
    let r = reply.trim();
    if q.yes_no {
        let yes = matches!(r.to_lowercase().as_str(), "y" | "yes" | "s" | "si" | "sí" | "1" | "true" | "ok");
        keys(id, if yes { "y" } else { "n" })?;
        std::thread::sleep(Duration::from_millis(60));
        keys(id, "enter")?;
        return Ok(if yes { "yes".into() } else { "no".into() });
    }
    let low = r.to_lowercase();
    let by_number = r.parse::<u32>().ok().and_then(|n| q.options.iter().position(|o| o.0 == n));
    let by_words = q.options.iter().position(|o| o.1.to_lowercase().contains(&low));
    // Free words: the option that lets you type (Claude Code's «Type something»,
    // «No, and tell Claude what to do differently»).
    let typing = q.options.iter().position(|o| {
        let t = o.1.to_lowercase();
        t.contains("type something") || t.contains("tell claude") || t.starts_with("other") || t.contains("something else")
    });
    let (k, then_type) = match (by_number, by_words, typing) {
        (Some(k), _, _) => (k, false),
        (None, Some(k), _) => (k, false),
        (None, None, Some(k)) => (k, true),
        _ => return Err(format!("«{r}» is none of its options")),
    };
    let steps = k as i64 - q.cursor as i64;
    let mut ks: Vec<&str> = vec![if steps > 0 { "down" } else { "up" }; steps.unsigned_abs() as usize];
    ks.push("enter");
    keys(id, &ks.join(" "))?;
    if then_type {
        std::thread::sleep(Duration::from_millis(350));
        paste(id, r)?;
        std::thread::sleep(Duration::from_millis(150));
        keys(id, "enter")?;
        return Ok(format!("«{}», then wrote: {r}", q.options[k].1));
    }
    Ok(format!("{}) {}", q.options[k].0, q.options[k].1))
}

/// The answer goes back along the wire, a moment, and it is still again.
fn back(k: usize, mine_is_a: bool) {
    flow(k, if mine_is_a { -1 } else { 1 });
    std::thread::sleep(Duration::from_millis(1600));
    flow(k, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claude_code_answer_comes_without_its_input_box() {
        let screen = "  (trabajando en ~/Proyectos/pleamar). Esto es una prueba de los cables de pleamar-wm:
  respóndeme en una sola frase quién eres y en qué carpeta estás. No hagas nada más.

● Soy Claude Code (Opus 5.5) y estoy trabajando en la carpeta /home/abel.

✻ Cogitated for 7s · done 20:25



───────────────────────────────────────────────────────────────────────────────────────────
❯
───────────────────────────────────────────────────────────────────────────────────────────
  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← 1 agent";
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(answer(&lines), "● Soy Claude Code (Opus 5.5) y estoy trabajando en la carpeta /home/abel.");
    }

    #[test]
    fn an_answer_of_several_lines_keeps_them_all() {
        let lines = ["task tail", "", "● First line.", "  second line", "", "● And a last word.", "", "────────────────────────", "❯ ", "────────────────────────"];
        assert_eq!(answer(&lines), "● First line.\n  second line\n\n● And a last word.");
    }

    #[test]
    fn a_codex_answer_comes_without_its_prompt_and_footer() {
        let screen = "  >_ OpenAI Codex (v0.161.0)
     ~


› [a task from claude in window 0 «◑ Saludo a Codex», through a
  wire] It is in the pasted text: do it, and end with your
  answer.Salúdame, por favor: respóndeme con un saludo corto.


• ¡Hola! 👋

  Worked for 2s • 22:02



› Ask Codex to do anything

  GPT-6-Luna high · ~ · Saluda brevemente
  ← for agents · ? for shortcuts";
        let lines: Vec<&str> = screen.lines().collect();
        let from = handed_at(&lines).expect("the wrapped line that says what the task is");
        assert_eq!(lines[from], "  wire] It is in the pasted text: do it, and end with your");
        assert_eq!(answer(&lines[from + 1..]), "• ¡Hola! 👋");
    }

    #[test]
    fn the_line_that_says_what_a_task_is_is_found_on_one_line_too() {
        let lines = ["old", "› [a task from claude in window 0 «x», through a wire] It is in the pasted text", "• hi"];
        assert_eq!(handed_at(&lines), Some(1));
        assert_eq!(handed_at(&["nothing", "here"]), None);
    }

    #[test]
    fn a_codex_answer_with_a_rule_under_it_loses_the_rule() {
        let lines = ["• Done: three files.", "", "─ Worked for 1m 02s ──────────────────────", "", "› Ask Codex to do anything"];
        assert_eq!(answer(&lines), "• Done: three files.");
    }

    #[test]
    fn an_opencode_answer_comes_from_between_its_boxes() {
        let screen = "  ┃  [a task from claude in window 0 «◑ Saludo a Codex», through a wire] It is in the
  ┃  pasted text: do it, and end with your answer.Hola opencode: soy Claude, en la
  ┃  terminal de al lado. Salúdame con un saludo corto y dime qué modelo eres, en una
  ┃  frase.
  ┃

     + Thought: 526ms

     Hola Claude! Soy deepseek-v4.1-flash-free, corriendo como opencode.

        Build  DeepSeek V4.1 Flash Free   2.0s

  ┃
  ┃
  ┃
  ┃  Build ▀ DeepSeek V4.1 Flash Free OpenCode
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /home/abel/Proyectos/pleamar                                 13.0K (7%)  ctrl+p commands";
        let lines: Vec<&str> = screen.lines().collect();
        let from = handed_at(&lines).expect("its box's first line") + 1;
        assert_eq!(answer(&lines[from..]), "     Hola Claude! Soy deepseek-v4.1-flash-free, corriendo como opencode.");
    }

    #[test]
    fn a_codex_answer_is_read_whole_from_its_record() {
        let long = (1..=60).map(|k| format!("line {k}")).collect::<Vec<_>>().join("\n");
        let record = [
            r#"{"type":"session_meta","payload":{"cwd":"/w"}}"#.to_owned(),
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"[a task from claude in window 0 «x», through a wire] It is in the pasted text: do it, and end with your answer.first"}]}}"#.to_owned(),
            r#"{"type":"event_msg","payload":{"type":"task_complete","completed_at":100,"last_agent_message":"the first answer"}}"#.to_owned(),
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"[a task from claude in window 0 «x», through a wire] It is in the pasted text: do it, and end with your answer.second"}]}}"#.to_owned(),
            serde_json::json!({"type": "event_msg", "payload": {"type": "task_complete", "completed_at": 200, "last_agent_message": long}}).to_string(),
        ]
        .join("\n");
        assert_eq!(codex_said(&record, "/w", 150_000), Some(Some(long.clone())));
        assert_eq!(codex_said(&record, "/w", 0), Some(Some(long)));
        // Another folder's is not looked into; a task whose turn has not ended yet has no answer.
        assert_eq!(codex_said(&record, "/other", 0), None);
        assert_eq!(codex_said(&record, "/w", 300_000), Some(None));
    }

    #[test]
    fn a_claude_code_answer_is_read_whole_from_its_record() {
        let long = (1..=60).map(|k| format!("## {k}\nline {k}")).collect::<Vec<_>>().join("\n");
        let line = |v: serde_json::Value| v.to_string();
        let record = [
            line(serde_json::json!({"type": "user", "timestamp": "2026-10-21T15:00:00.000Z", "message": {"content": "[a task from claude in window 0 «x», through a wire] first"}})),
            line(serde_json::json!({"type": "assistant", "timestamp": "2026-10-21T15:00:05.000Z", "message": {"id": "m1", "content": [{"type": "text", "text": "the first answer"}]}})),
            line(serde_json::json!({"type": "user", "timestamp": "2026-10-21T15:09:59.000Z", "message": {"content": [{"type": "text", "text": "[a task from claude in window 0 «x», through a wire] second"}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": "2026-10-21T15:10:01.000Z", "message": {"id": "m2", "content": [{"type": "text", "text": "Let me look."}, {"type": "tool_use", "name": "Bash"}]}})),
            line(serde_json::json!({"type": "user", "timestamp": "2026-10-21T15:10:02.000Z", "message": {"content": [{"type": "tool_result", "content": "through a wire] in a file"}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": "2026-10-21T15:10:03.000Z", "isSidechain": true, "message": {"id": "s1", "content": [{"type": "text", "text": "a subagent's"}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": "2026-10-21T15:10:50.000Z", "message": {"id": "m3", "content": [{"type": "text", "text": long}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": "2026-10-21T15:10:50.100Z", "message": {"id": "m3", "content": [{"type": "tool_use", "name": "Read"}]}})),
        ]
        .join("\n");
        let handed = iso_ms("2026-10-21T15:09:58.999Z").unwrap();
        assert_eq!(handed, 1_792_595_398_999);
        assert_eq!(claude_said(&record, handed), Some((true, long.clone())));
        assert_eq!(claude_said(&record, 0), Some((false, long)));
        // Nothing written since: no answer.
        assert_eq!(claude_said(&record, handed + 3_600_000), None);
    }

    #[test]
    fn a_claude_code_waiting_for_its_own_commands_is_not_done() {
        let line = |v: serde_json::Value| v.to_string();
        let started = |ts: &str, id: &str| line(serde_json::json!({"type": "user", "timestamp": ts, "message": {"content": [{"type": "tool_result", "content": format!("Command running in background with ID: {id}. Output is being written to: /tmp/x")}]}}));
        let ended = |ts: &str, id: &str| line(serde_json::json!({"type": "user", "timestamp": ts, "message": {"content": format!("<task-notification>\n<task-id>{id}</task-id>\n<status>completed</status>\n</task-notification>")}}));
        let handed = iso_ms("2026-10-21T15:00:00.000Z").unwrap();
        let record = [started("2026-10-21T14:00:00.000Z", "server1"), started("2026-10-21T15:01:00.000Z", "build22"), started("2026-10-21T15:01:05.000Z", "wait333")].join("\n");
        // One left running before the task (a server) is not waited for.
        assert_eq!(running(&record, handed), ["build22", "wait333"]);
        let later = [record.clone(), ended("2026-10-21T15:04:00.000Z", "build22")].join("\n");
        assert_eq!(running(&later, handed), ["wait333"]);
        let all = [later, ended("2026-10-21T15:04:01.000Z", "wait333")].join("\n");
        assert!(running(&all, handed).is_empty());
    }

    #[test]
    fn what_codex_and_opencode_draw_at_the_bottom_is_no_answer() {
        assert_eq!(answer(&["  GPT-6-Luna high · ~/Proyectos/pleamar-wm", "  ← for agents · ? for shortcuts"]), "  GPT-6-Luna high · ~/Proyectos/pleamar-wm");
        assert_eq!(answer(&["   /home/abel/Proyectos/pleamar-wm                      28.3K (14%)  ctrl+p commands"]), "");
    }

    #[test]
    fn a_program_replaced_while_it_runs_still_finds_itself() {
        // Linux names its file «… (deleted)» once it is replaced; the new one is where it was.
        let me = own_exe();
        assert!(!me.to_string_lossy().ends_with("(deleted)"));
    }

    #[test]
    fn an_agents_marks_are_left_out_and_the_persons_settings_kept() {
        for m in ["CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ID", "AI_AGENT", "CODEX_THREAD_ID", "OPENCODE"] {
            assert!(agent_marker(m), "{m}");
        }
        for k in ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "ANTHROPIC_API_KEY", "PATH"] {
            assert!(!agent_marker(k), "{k}");
        }
    }

    #[test]
    fn claude_codes_box_is_empty_only_with_nothing_after_its_prompt() {
        let rule = "────────────────────────────────────────";
        let empty = format!("● working…\n{rule}\n❯ \n{rule}\n  ⏵⏵ auto mode on");
        assert!(claude_box_empty(&empty));
        let typing = format!("● working…\n{rule}\n❯ half a sentence\n{rule}\n  ⏵⏵ auto mode on");
        assert!(!claude_box_empty(&typing));
        // Several lines being written: the last `❯` is the box's, with text.
        let lines = format!("{rule}\n❯ first line\n  second line\n{rule}");
        assert!(!claude_box_empty(&lines));
        assert!(!claude_box_empty("no box here"));
    }

    #[test]
    fn an_agent_that_marks_nothing_gives_what_it_said() {
        let lines = ["ANSWER: FIND THE LEAK", ">"];
        assert_eq!(answer(&lines), "ANSWER: FIND THE LEAK\n>");
    }
}
