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

use crate::term::layer_of;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

type Info = BTreeMap<String, String>;

/// A window of the session: its place in the scene and its process.
pub(crate) struct Win {
    pub(crate) slot: usize,
    pub(crate) pid: u32,
    pub(crate) title: String,
}

/// The session's windows, as pleamar-wm writes them (`windows` next to its
/// other sockets): `slot pid<TAB>title`, one a line.
pub(crate) fn windows() -> Result<Vec<Win>, String> {
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

pub(crate) fn say_wm(orders: &[String]) -> Vec<String> {
    say(orders)
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

    /// The shell of that window's terminal. Of several, the one that moved
    /// last: the wire is the person's word for the window, and a terminal's
    /// tabs are all in it. But a process with several windows (foot's
    /// server, kitty's single instance) may be at a window not wired, and
    /// is not read nor typed in unless its shell is named (`--shell ID`).
    fn shell_of(&self, w: &Win, shell: Option<i32>) -> Result<Option<(i32, Info)>, String> {
        let shells = crate::term::shells_of(w.pid, &self.layers);
        if let Some(id) = shell {
            // A shell named in a process with several windows may be at any of
            // them: none may be private (the window manager answers for all of
            // that process's windows).
            if self.wins.iter().filter(|o| o.pid == w.pid).count() > 1 {
                match crate::agent_cli::tell(&format!("i {}.{}", w.pid, w.slot)) {
                    Ok(r) if r.starts_with("ok") => {}
                    Ok(r) if r == "err private" => return Err(format!("window {}'s terminal has a private window (or the screen is locked): its shells are not named", w.slot)),
                    Ok(r) => return Err(format!("window {}: the window manager answered {r:?}", w.slot)),
                    Err(e) => return Err(format!("which of window {}'s shells is not private cannot be asked ({e}): name none", w.slot)),
                }
            }
            return shells.into_iter().find(|(i, _)| *i == id).map(Some).ok_or_else(|| format!("shell {id} is not in window {}'s terminal: `pleamar-wm term list` says the terminal of each", w.slot));
        }
        if shells.len() > 1 && self.wins.iter().filter(|o| o.pid == w.pid).count() > 1 {
            let list: Vec<String> = shells
                .iter()
                .map(|(id, i)| {
                    let g = |k: &str| i.get(k).map_or("", String::as_str);
                    format!("  --shell {id}  {}  {}  {:?}", tilde(g("cwd")), g("command"), g("title"))
                })
                .collect();
            return Err(format!("window {}'s terminal has {} shells in several windows, and which is this one's is not known: name one\n{}", w.slot, shells.len(), list.join("\n")));
        }
        Ok(shells.into_iter().next())
    }

    /// The one named: by its number, or by what runs in it if only one is that.
    fn target(&self, name: &str) -> Result<(usize, &Win, bool), String> {
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
            [(k, w, a)] => Ok((*k, w, *a)),
            [] if name.parse::<usize>().is_ok() => Err(format!("window {name} isn't wired to yours. Ask the person to wire them: on a free monitor (Super+W), drag the drop at the start of one's title bar onto the other.")),
            [] => Err(format!("no window wired to yours is «{name}». `pleamar-wm link list` says which are.")),
            _ => Err(format!("more than one window wired to yours is «{name}»: name it by its number (`pleamar-wm link list`).")),
        }
    }
}

/// The caller's caravan: when its terminal has a name (a companion, `link open
/// --name`), every other name with a mailbox or a terminal of its own —its
/// name, what to call it, how it is—. A team: they ask each other by name,
/// through the mailbox (mail.rs), with no wire. A terminal with no name is no
/// one's companion, and has only its wires.
fn crew(h: &Here) -> Vec<(String, String, &'static str)> {
    let Some(me) = h.info.get("name").filter(|n| !n.is_empty()) else { return Vec::new() };
    let how = |name: &str| match h.layers.iter().find(|(_, i)| i.get("name").map(String::as_str) == Some(name)) {
        None => "not started",
        Some((_, i)) if at_rest(i) => "ready",
        Some(_) => "working",
    };
    let mut out: Vec<(String, String, &'static str)> = Vec::new();
    for b in crate::mail::all() {
        let Some(name) = b["name"].as_str().filter(|n| n != me) else { continue };
        let title = b["card"]["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(name).to_owned();
        out.push((name.to_owned(), title, how(name)));
    }
    for (_, i) in &h.layers {
        if let Some(n) = i.get("name").filter(|n| !n.is_empty() && *n != me) {
            if !out.iter().any(|o| &o.0 == n) {
                out.push((n.clone(), n.clone(), how(n)));
            }
        }
    }
    out
}

/// Said to an agent: its caravan, and how to ask them.
fn crew_lines(h: &Here) -> Option<String> {
    let crew = crew(h);
    if crew.is_empty() {
        return None;
    }
    let list: Vec<String> = crew.iter().map(|(n, t, s)| if n == t { format!("  {n} ({s})") } else { format!("  {n} «{t}» ({s})") }).collect();
    Some(format!(
        "Your caravan —your team, no wire needed—: ask any of them by name with `link_delegate` (window: its name). It goes through their mailbox; one not started is started, and its answer comes back to you like a wired one's.\n{}",
        list.join("\n")
    ))
}

/// A task for a companion of the caravan, by name, through its mailbox: its
/// answer waited for here (`--wait`, or a shell), or —an agent— typed into
/// the caller's terminal when it comes, by the messenger that hands it.
fn delegate_by_mail(h: &Here, to: &str, title: &str, rest: &[String]) -> Result<String, String> {
    let task = words(&rest[1..]);
    if task.is_empty() {
        return Err("delegate NAME TASK [--wait] [--timeout S]".into());
    }
    let me = h.info.get("name").cloned().unwrap_or_default();
    // As OpenBot's agents: an agent never waits for a teammate (`--wait` is
    // not for them): it ends its turn, and the answer comes in a later one, as
    // a letter in its own mailbox. Only a shell, with nowhere for an answer to
    // come, waits for it here. `--no-reply`: information, not a request.
    let caller_agent = h.info.get("agent").is_some_and(|a| !a.is_empty());
    let info_only = rest.iter().any(|a| a == "--no-reply");
    let waits = !caller_agent && !info_only;
    let mut post = vec![to.to_owned(), task, "--from".to_owned(), me];
    if waits || info_only {
        post.push("--no-reply".to_owned());
    }
    let said = crate::mail::post(&post)?;
    let id: u64 = said.strip_prefix("letter ").and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok()).ok_or("the mailbox did not say which letter it is")?;
    if waits {
        let timeout = number(rest, "--timeout", 600);
        return match mail_answer(to, id, Duration::from_secs(timeout)) {
            Some(l) => Ok(mail_answer_text(title, &l)),
            None => Err(format!("{title} has not answered in {timeout} s. It goes on: its answer stays in its mailbox (`pleamar-wm link mail {to}`).")),
        };
    }
    if info_only {
        return Ok(format!("Told {title} (letter {id}, no answer asked)."));
    }
    Ok(format!("Your request is in {title}'s mailbox (letter {id}): {title} gets it as soon as it is free (if it is not started, the desktop starts it). End your turn now —say what you asked and that the answer will follow—: do not wait for it nor check on it. {title}'s answer comes to you as a new message, and you carry on with it then."))
}

/// The letter `id` of `to`'s mailbox, once answered (or ended otherwise).
fn mail_answer(to: &str, id: u64, timeout: Duration) -> Option<serde_json::Value> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(l) = crate::mail::all().iter().find(|b| b["name"].as_str() == Some(to)).and_then(|b| b["done"].as_array()?.iter().find(|l| l["id"].as_u64() == Some(id)).cloned()) {
            return Some(l);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    None
}

fn mail_answer_text(title: &str, l: &serde_json::Value) -> String {
    let answer = l["answer"].as_str().unwrap_or("").trim().to_owned();
    match l["how"].as_str().unwrap_or("done") {
        "done" if !answer.is_empty() => answer,
        "done" => format!("{title} answered nothing on its screen."),
        "gone" => format!("{title}'s terminal closed before it answered."),
        "late" => format!("{title} stopped answering. What it said so far:\n{answer}"),
        "refused" => format!("{title} did not take the task: {answer}"),
        other => format!("{title}: {other}. {answer}"),
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
    crate::term::info(&socket(id)).unwrap_or_default()
}

pub(crate) fn read(id: i32, lines: usize) -> String {
    crate::term::ask(&socket(id), &format!("read {lines}")).unwrap_or_default()
}

/// Its screen, once it holds still: two readings a quarter of a second apart
/// that are the same (two seconds at most).
fn settled(id: i32) -> String {
    let start = Instant::now();
    let mut last = read(id, 60);
    while start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(250));
        let now = read(id, 60);
        if now == last {
            return now;
        }
        last = now;
    }
    last
}

/// The task, pasted after the line that says what it is, and taken: once its
/// screen holds still after that line (an agent just started may still be
/// taking it in, and drops a paste that comes meanwhile: the line came and
/// the task did not), and again —up to three times— if its screen does not
/// change with it. Then still again before Enter.
fn paste_task(id: i32, task: &str) -> Result<(), String> {
    let shown = settled(id);
    for _ in 0..3 {
        paste(id, task)?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(250));
            if read(id, 60) != shown {
                settled(id);
                return Ok(());
            }
        }
    }
    Err("its screen did not change with the task pasted".into())
}

fn paste(id: i32, text: &str) -> Result<(), String> {
    match crate::term::ask(&socket(id), &format!("paste {}", hex(text.as_bytes())))? {
        r if r.starts_with("ok") => Ok(()),
        r => Err(r.trim().to_owned()),
    }
}

pub(crate) fn keys(id: i32, names: &str) -> Result<(), String> {
    match crate::term::ask(&socket(id), &format!("keys {names}"))? {
        r if r.starts_with("ok") => Ok(()),
        r => Err(r.trim().to_owned()),
    }
}

/// Done with what it was doing: its agent says so through its hooks —and
/// its title does not say it works: a hook can tell «done» late, as Claude
/// Code's «waiting for you» a minute after its last answer, landing just as
/// a new task starts—, or shows it is ready (✳), or —no title to go by—
/// nothing has come out of it for a while.
pub(crate) fn at_rest(i: &Info) -> bool {
    if let Some(h) = i.get("hook").filter(|h| !h.is_empty()) {
        return h == "done" && i.get("state").map(String::as_str) != Some("working");
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
    /// The answer its hooks told when it ended the task's turn, kept as it was
    /// then: what is asked of it next may already be under way, and its own
    /// answer will take that one's place.
    static STOPPED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// How many answers its hooks had told with that one.
    static STOPPED_N: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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
/// background since `after` that have not told it they ended. That notice,
/// come while it works, is kept as a queued one (`queue-operation`,
/// `attachment`), not as something its person said.
fn running(text: &str, after: u64) -> Vec<String> {
    const STARTED: &str = "running in background with ID: ";
    let mut ids: Vec<String> = Vec::new();
    for l in text.lines().filter(|l| l.contains(STARTED) || l.contains("<task-id>")) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        // Its own words (a command it wrote) say nothing of what runs.
        if v["type"] == "assistant" || v["timestamp"].as_str().and_then(iso_ms).is_none_or(|t| t + 1000 < after) {
            continue;
        }
        let told = v["type"] == "user";
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
        gather(&v["content"]);
        gather(&v["attachment"]["prompt"]);
        let word = |from: &str| from.chars().take_while(char::is_ascii_alphanumeric).collect::<String>();
        for (k, _) in said.match_indices(STARTED).filter(|_| told) {
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
            Some("user") if after > 0 && texts.iter().any(|t| marked_task(t)) => {
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
        if after > 0 && p["role"] == "user" && p["content"].as_array().is_some_and(|c| c.iter().any(|t| t["text"].as_str().is_some_and(marked_task))) {
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
    let from = if after == 0 { 0 } else { messages.iter().rposition(|m| m["info"]["role"] == "user" && marked_task(&said(m)))? };
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
pub(crate) fn asked(id: i32, i: &Info) -> Option<crate::term::Question> {
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
/// `asks`, to stop on a question (seen twice too). Late after `timeout`, or
/// —`idle`— once it has shown no sign of working for that long.
fn wait_for(id: i32, timeout: Duration, asks: bool, idle: Option<Duration>) -> Waited {
    let start = Instant::now();
    let mut sign = Instant::now();
    let (mut rest, mut asking) = (0, 0);
    // At rest with commands of its own still running (it waits for them), since
    // when; and whether it still had them, looked at every few seconds. Not for
    // ever: a server it left running ends nothing.
    let mut held: Option<Instant> = None;
    let mut looked: Option<(Instant, bool)> = None;
    // The answers its hooks have told since the task, last seen: one more is the
    // end of a turn, the task's if nothing it left runs on, though it may be
    // already at what was asked next (queued while it worked) and never seen at rest.
    let handed = HANDED.with(|h| h.get());
    let mut counted = handed;
    while start.elapsed() < timeout {
        let i = info(id);
        if i.is_empty() {
            return Waited::Gone;
        }
        if let Some(n) = handed.filter(|_| i.get("hook").is_some_and(|h| !h.is_empty())) {
            let (count, text) = hooked_answer(id);
            if count > n && Some(count) != counted && !text.is_empty() {
                counted = Some(count);
                if !in_background(id, &i, HANDED_AT.with(|h| h.get())) {
                    STOPPED.with(|s| *s.borrow_mut() = Some(text));
                    STOPPED_N.with(|c| c.set(count));
                    return Waited::Done;
                }
            }
        }
        if !at_rest(&i) {
            sign = Instant::now();
        }
        if idle.is_some_and(|d| sign.elapsed() > d) {
            return Waited::Late;
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

/// What makes a question the same one again: its options (the words above
/// them change as its screen is drawn again), or its words if it has none.
pub(crate) fn ask_key(q: &crate::term::Question) -> String {
    if q.options.is_empty() {
        q.text.clone()
    } else {
        q.options.iter().map(|o| o.1.as_str()).collect::<Vec<_>>().join("\n")
    }
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
    if let Some(text) = STOPPED.with(|s| s.borrow().clone()) {
        return text;
    }
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
    let agent = i.get("agent").map(String::as_str).unwrap_or("");
    let from = match handed_at(&lines) {
        Some(k) => k + 1,
        // An agent we know whose screen does not show the task: what it shows
        // is not what it said to it (its welcome, its logo).
        None if i.get("fullscreen").map(String::as_str) == Some("1") && matches!(agent, "claude" | "codex" | "opencode") => return String::new(),
        None if i.get("fullscreen").map(String::as_str) == Some("1") => 0,
        None => before.saturating_sub(3).min(lines.len()),
    };
    // One that marks what it says, with nothing marked after the task, has
    // said nothing yet: what follows is the task itself.
    if matches!(agent, "claude" | "codex") && !lines[from..].iter().any(|l| matches!(l.trim_start().chars().next(), Some('●' | '⏺' | '•'))) {
        return String::new();
    }
    answer(&lines[from..])
}

/// What came after the task, or —nothing yet— a look again for a while
/// (an answer can be a moment behind its end, and an agent that seemed
/// done may go on): never its task, its welcome or what was there before.
fn brought(id: i32, before: usize, deadline: Instant) -> Option<String> {
    for _ in 0..4 {
        let text = since(id, before);
        if !text.trim().is_empty() {
            return Some(text);
        }
        std::thread::sleep(Duration::from_secs(5));
        if !matches!(wait_for(id, deadline.saturating_duration_since(Instant::now()), true, None), Waited::Done) {
            break;
        }
    }
    let text = since(id, before);
    (!text.trim().is_empty()).then_some(text)
}

/// Said when nothing came after a task.
fn nothing(slot: usize) -> String {
    format!("Nothing came after the task in window {slot}: not on its screen, nor in what its agent keeps. It may not have taken it: `pleamar-wm link read {slot}` shows its screen; hand it again if so.")
}

/// The last line where the line that says what a task is ends: its
/// «through a wire]», even when the agent wrapped it over two or three lines
/// (Codex leaves «through a» on one and «wire]» on the next). Not one the
/// agent says itself, quoting it (`● Done: [a task from …, through the
/// mailbox] …`): after that line is no answer, and its answer would be lost.
fn handed_at(lines: &[&str]) -> Option<usize> {
    let bare = |ls: &[&str]| ls.concat().chars().filter(|c| !c.is_whitespace()).collect::<String>();
    (0..lines.len()).rev().find(|&k| lines[k].contains(']') && {
        let from = k.saturating_sub(2);
        let b = bare(&lines[from..=k]);
        (b.contains("throughawire]") || b.contains("throughthemailbox]")) && {
            // The line where «[a task» starts: the last one from which it is still all there.
            let starts = (from..=k).rev().find(|&j| bare(&lines[j..=k]).contains("[atask")).unwrap_or(from);
            !matches!(lines[starts].trim_start().chars().next(), Some('●' | '⏺' | '•'))
        }
    })
}

/// The line that says what a task is, as a wire or a mailbox (mail.rs) hands it.
fn marked_task(t: &str) -> bool {
    t.contains("through a wire]") || t.contains("through the mailbox]")
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
            "--enter" | "--wait" | "--no-reply" => {}
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
  open [--folder D] [--name NAME] [--terminal PROGRAM] [--quiet] [COMMAND…]
                                  a terminal of its own, wired to nobody, in D (home), named
                                  (`who.N` in the scene, PLEAMAR_NAME inside), with COMMAND
                                  typed into its shell; --quiet, without taking the keyboard.
                                  Prints its window's number. Needs no window of your own
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
Mailboxes (the terminals named with `open --name`; they need no window of your own):
  post NAME TEXT… [--from WHO]    a letter for the agent there: handed to it when it is free
  mail [NAME] [--json]            what waits in each, what each is at, its last answers
  mail NAME --cancel ID | --first ID | --pause | --resume | --forget ID
  caravan [--json]                what waits at the gate, the named terminals and their mailboxes
                                  (what the remote page's «Caravan» shows); `caravan allow|deny ID`;
                                  `caravan all | talk NAME | screen NAME | do JSON`: what its
                                  /caravan page reads and does
N is a window's number from `list`, its wire's name (`tests`), or the agent that runs there
(`codex`) if only one is. A terminal whose one process has several windows (foot's server)
has a shell named for each of those: `--shell ID`, its number in `pleamar-wm term list`.";

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
            if let Some(c) = crew_lines(&h) {
                out.push_str(&format!("\n{c}"));
            }
            Ok(out)
        }
        "list" => {
            let h = here()?;
            let wired = h.wired();
            let crew = crew_lines(&h);
            if wired.is_empty() {
                return Ok(match crew {
                    Some(c) => format!("No window is wired to yours.\n{c}"),
                    None => "No window is wired to yours. The person wires them: on a free monitor (Super+W), dragging the drop at the start of a window's title bar onto another window.".into(),
                });
            }
            let lines = wired.iter().map(|(k, w, _)| line_of(w.slot, w, layer_of(w.pid, &h.layers).as_ref(), h.label(*k))).collect::<Vec<_>>().join("\n");
            Ok(match crew {
                Some(c) => format!("{lines}\n{c}"),
                None => lines,
            })
        }
        // The wire to that window, named («tests»): shown on it, and a name to give it.
        "label" => {
            let name = rest.first().ok_or(USAGE)?;
            let h = here()?;
            let (k, w, _) = h.target(name)?;
            let text = words(&rest[1..]).replace(['\n', '\t'], " ");
            say(&[format!("text wire.{k}.label {text}")]);
            Ok(if text.is_empty() { format!("the wire to window {} has no name now", w.slot) } else { format!("the wire to window {} is «{text}»", w.slot) })
        }
        "read" | "send" | "keys" | "wait" | "ask" | "answer" | "delegate" => {
            // `--shell ID`: which of its terminal's shells (see `shell_of`).
            let mut rest = rest.to_vec();
            let shell = match rest.iter().position(|a| a == "--shell") {
                Some(k) => {
                    let id = rest.get(k + 1).and_then(|s| s.parse::<i32>().ok()).ok_or("--shell ID: a shell's number, as `pleamar-wm term list` gives it")?;
                    rest.drain(k..k + 2);
                    Some(id)
                }
                None => None,
            };
            let rest = &rest[..];
            let name = rest.first().ok_or(USAGE)?;
            let h = here()?;
            // A companion of the caravan not wired to this one: by name, through its mailbox.
            if verb == "delegate" && h.target(name).is_err() {
                if let Some((to, title, _)) = crew(&h).into_iter().find(|(n, t, _)| n.eq_ignore_ascii_case(name) || t.eq_ignore_ascii_case(name)) {
                    return delegate_by_mail(&h, &to, &title, rest);
                }
            }
            let (k, w, mine_is_a) = h.target(name)?;
            let (id, tinfo) = h.shell_of(w, shell)?.ok_or_else(|| format!("window {} «{}» has no terminal of its own (through pleamar-wm's layer) to read or type in", w.slot, w.title))?;
            match verb {
                "read" => Ok(read(id, rest.get(1).and_then(|n| n.parse().ok()).unwrap_or(200))),
                "send" => {
                    let text = words(&rest[1..]);
                    if text.is_empty() {
                        return Err("send N TEXT [--enter]".into());
                    }
                    // Said to an agent whose hooks count its answers, by another agent: what
                    // it answers is brought back as a delegated task's is. It may be at a
                    // task now, and take this in the middle of it (one answer for both,
                    // brought once) or after it (an answer of its own).
                    let enter = rest.iter().any(|a| a == "--enter");
                    let note = enter && h.info.get("agent").is_some_and(|a| !a.is_empty()) && tinfo.get("agent").is_some_and(|a| !a.is_empty()) && tinfo.get("hook").is_some_and(|h| !h.is_empty());
                    let (count, at, busy) = (hooked_answer(id).0, now_ms(), !at_rest(&tinfo));
                    let before = read(id, 2000).lines().count();
                    paste(id, &text)?;
                    if enter {
                        std::thread::sleep(Duration::from_millis(150));
                        keys(id, "enter")?;
                    }
                    if note {
                        let mut c = std::process::Command::new("setsid");
                        c.arg("-f").arg(own_exe()).args(["link", "_deliver", &id.to_string(), &h.layer.to_string(), &k.to_string(), &(mine_is_a as u8).to_string(), &before.to_string(), &(6 * 3600).to_string(), &w.slot.to_string(), &count.to_string(), &at.to_string(), if busy { "note-busy" } else { "note" }]);
                        c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                        c.spawn().map_err(|e| e.to_string())?;
                        return Ok(format!("typed into window {}; what its agent answers to it will be typed into this terminal (once, with the answer of a task it is at, if it takes it in the middle of it)", w.slot));
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
                    // As the one who handed it the last task would bring it (waiting too for
                    // what it left running in the background): what its hooks or its record
                    // kept since, whole; its screen only failing those.
                    let (at, n) = handed(id);
                    HANDED.with(|c| c.set(Some(n)));
                    HANDED_AT.with(|c| c.set(at));
                    match wait_for(id, timeout, true, None) {
                        Waited::Done => {
                            taken(id);
                            Ok(brought(id, read(id, 2000).lines().count().saturating_sub(120), Instant::now() + Duration::from_secs(60)).unwrap_or_else(|| nothing(w.slot)))
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
                    // Taken in, once its screen shows it: an agent still starting may drop
                    // what is typed (opencode), and then it is typed again.
                    let head = format!("[a task from {who} in window {} «{me}», through a wire] It is in the pasted text: do it, and end with your answer.", h.slot);
                    let shown = read(id, 60);
                    let mut took = false;
                    for _ in 0..3 {
                        paste(id, &head)?;
                        let start = Instant::now();
                        while !took && start.elapsed() < Duration::from_secs(3) {
                            std::thread::sleep(Duration::from_millis(250));
                            took = read(id, 60) != shown;
                        }
                        if took {
                            break;
                        }
                    }
                    if !took {
                        flow(k, 0);
                        return Err(format!("window {} did not take what was typed (its screen did not change): is its agent ready? `pleamar-wm link read {}`", w.slot, w.slot));
                    }
                    if let Err(e) = paste_task(id, &task) {
                        flow(k, 0);
                        return Err(format!("window {}: {e}: is its agent ready? `pleamar-wm link read {}`", w.slot, w.slot));
                    }
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
                        return match wait_for(id, Duration::from_secs(timeout), true, None) {
                            Waited::Done => {
                                back(k, mine_is_a);
                                Ok(brought(id, before, Instant::now() + Duration::from_secs(timeout)).unwrap_or_else(|| nothing(w.slot)))
                            }
                            Waited::Asks(q) => {
                                flow(k, 0);
                                Ok(format!("{}\nIt stopped to ask that while on your task. Once answered, `pleamar-wm link wait {}` brings its answer.", said(w.slot, &q), w.slot))
                            }
                            Waited::Gone => Err(format!("window {}'s terminal went away", w.slot)),
                            Waited::Late => {
                                flow(k, 0);
                                Err(format!("window {} did not finish in {timeout} s (it goes on: `pleamar-wm link wait {}` brings its answer when it is done); so far:\n{}", w.slot, w.slot, since(id, before)))
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
            // Brought for something `send` typed, not for a task: `note-busy`, typed
            // while it was at something else.
            let note = rest.get(9).map(String::as_str);
            let mut busy = note == Some("note-busy");
            // As long as it works, however long (a day at most): the time given counts
            // only while it shows no sign of working.
            let started = Instant::now();
            let deadline = started + Duration::from_secs(timeout.max(24 * 3600));
            let idle = Duration::from_secs(timeout);
            let (mut told, mut gone) = (String::new(), false);
            let done = loop {
                match wait_for(id, deadline.saturating_duration_since(Instant::now()), true, Some(idle)) {
                    // Typed while it was at something else, and that turn ended, but it is
                    // at work again at once, with the answers as they were: what was
                    // typed was queued, and this is its own turn. Its answer is the next.
                    Waited::Done if busy && STOPPED.with(|s| s.borrow().is_some()) => {
                        busy = false;
                        let n = STOPPED_N.with(|c| c.get());
                        let again = (0..8).any(|_| {
                            std::thread::sleep(Duration::from_millis(500));
                            info(id).get("hook").map(String::as_str) == Some("working") && hooked_answer(id).0 == n
                        });
                        if !again {
                            break true;
                        }
                        HANDED.with(|c| c.set(Some(n)));
                        STOPPED.with(|s| *s.borrow_mut() = None);
                    }
                    Waited::Done => break true,
                    Waited::Late => break false,
                    Waited::Gone => {
                        gone = true;
                        break false;
                    }
                    // It stopped to ask its person: the one who handed it the
                    // task is told (once for each question), and may answer.
                    Waited::Asks(q) => {
                        if ask_key(&q) != told || told.is_empty() {
                            told = ask_key(&q);
                            tell(mine, &format!("[window {slot} stopped to ask something while on the task you handed it] It is in the pasted text."), &said(slot as usize, &q))?;
                        }
                        // Until it is answered (by anyone: gone from its screen a while,
                        // not just drawn again), or the time is up. Then one that asks the
                        // same, with the same options (a second command), is another.
                        let mut away = 0;
                        while Instant::now() < deadline && away < 2 {
                            std::thread::sleep(Duration::from_millis(700));
                            away = if asked(id, &info(id)).is_some_and(|n| ask_key(&n) == told) { 0 } else { away + 1 };
                        }
                        if away >= 2 {
                            told.clear();
                        }
                    }
                }
            };
            back(k, mine_is_a);
            // Already brought by `wait`, or another task handed there since (whose
            // answer is the one the agent's record ends with): not typed in.
            let at = HANDED_AT.with(|h| h.get());
            let later = note.is_none() && std::fs::read_to_string(format!("{}/{id}.handed", crate::term::socket_dir())).ok().and_then(|t| t.trim().parse::<u64>().ok()).is_some_and(|t| t > at);
            if taken_since(id, at) || later {
                return Ok(String::new());
            }
            // One answer, brought once: a task and what was typed to it on the way
            // can end in the same turn, and each has a messenger waiting for it.
            let count = if STOPPED.with(|s| s.borrow().is_some()) { STOPPED_N.with(|c| c.get()) } else { hooked_answer(id).0 };
            if done && count > 0 && brought_count(id) >= count {
                return Ok(String::new());
            }
            let answer = if done { brought(id, before, deadline) } else { Some(since(id, before)).filter(|t| !t.trim().is_empty()) };
            let (done, answer) = match answer {
                Some(a) => (done, a),
                None if done => {
                    tell(mine, &format!("[window {slot} shows nothing after the task you handed it] It is in the pasted text."), &nothing(slot as usize))?;
                    return Ok(String::new());
                }
                None => (false, "(nothing yet)".to_owned()),
            };
            if gone {
                tell(mine, &format!("[window {slot}'s terminal went away before it answered your task] It is in the pasted text."), &format!("What it said so far:\n{answer}"))?;
                return Ok(String::new());
            }
            if !done {
                let long = |d: Duration| if d.as_secs() < 120 { format!("{} s", d.as_secs()) } else { format!("{} min", d.as_secs() / 60) };
                let text = format!("Window {slot} has not answered your task after {}, and has shown no sign of working for {}: no more is waited for it here. Ask for its answer with `pleamar-wm link wait {slot}` (link_wait) when you want it.\n\nWhat it said so far:\n{answer}", long(started.elapsed()), long(idle));
                tell(mine, &format!("[window {slot} still has not answered your task after {}] It is in the pasted text.", long(started.elapsed())), &text)?;
                return Ok(String::new());
            }
            if count > 0 {
                let _ = std::fs::write(format!("{}/{id}.brought", crate::term::socket_dir()), count.to_string());
            }
            let what = if note.is_some() { "to what you typed to it" } else { "to the task you handed it" };
            tell(mine, &format!("[the answer from window {slot}, {what}] It is in the pasted text."), &answer)?;
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
        "open" => open(rest),
        "post" => crate::mail::post(rest),
        // What the remote page's «Caravan» shows and does (caravan.rs), from a shell.
        "caravan" => match rest.first().map(String::as_str) {
            None | Some("--json") => Ok(crate::caravan::state()),
            Some("allow" | "deny") => Ok(crate::caravan::answer(&format!("{} {}", rest.get(1).map_or("", String::as_str), rest[0]))),
            // What the remote's caravan page reads and does, the same from a shell.
            Some("all") => Ok(crate::caravan::companions()),
            Some("talk") => Ok(crate::caravan::talk(rest.get(1).map_or("", String::as_str))),
            Some("screen") => Ok(crate::caravan::screen(rest.get(1).map_or("", String::as_str))),
            Some("do") => {
                let said = crate::caravan::act(rest.get(1).map_or("", String::as_str));
                crate::caravan::heard();
                said
            }
            _ => Err("link caravan [--json] | allow|deny ID | all | talk NAME | screen NAME | do JSON".into()),
        },
        "asks" => crate::caravan::asks_cmd(rest),
        "mail" => crate::mail::mail(rest),
        // (The messenger the agents' watcher leaves with a letter it hands: mail.rs.)
        "_mail" => mail_deliver(rest),
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
            // Said a minute after its last answer: by then it may have a new task.
            "idle_prompt" if layer_working(&layer) => return,
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

/// Whether that layer's agent says, in its title, that it works.
fn layer_working(layer: &str) -> bool {
    crate::term::ask(&format!("{}/{layer}.sock", crate::term::socket_dir()), "info").is_ok_and(|t| t.lines().any(|l| l == "state working"))
}

/// The skill that tells an agent about wires, for each agent installed.
const SKILL: &str = include_str!("../skill/pleamar-link/SKILL.md");

/// The events Claude Code tells its hooks of, that say how it is.
const CLAUDE_EVENTS: [&str; 7] = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PermissionRequest", "Notification", "Stop"];
const HOOK: &str = "pleamar-wm link hook claude";

/// The agents an MCP server is registered with.
#[derive(Clone, Copy)]
pub(crate) enum Client {
    Claude,
    Codex,
}

pub(crate) const NO_CLIENT: &str = "no agent found to set up (Claude Code: ~/.claude and `claude`; Codex: ~/.codex and `codex`)";

/// Which of them are here: Claude Code (~/.claude and `claude`), Codex
/// (~/.codex and `codex`).
pub(crate) fn clients() -> Result<(bool, bool), String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME")?;
    let has = |p: &str| std::path::Path::new(&format!("{home}/{p}")).is_dir();
    let on_path = |c: &str| std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(c).is_file()));
    Ok((has(".claude") && on_path("claude"), has(".codex") && on_path("codex")))
}

/// Registering (or removing) the MCP server `name`, as a plan says it.
pub(crate) fn mcp_plan(client: Client, name: &str, undo: bool) -> String {
    let (verb, word) = if undo { ("remove", "remove") } else { ("register", "add") };
    match client {
        Client::Claude => format!("{verb} the MCP server «{name}» in Claude Code (user scope): `claude mcp {word}`"),
        Client::Codex => format!("{verb} the MCP server «{name}» in Codex: `codex mcp {word}`"),
    }
}

/// The MCP server `name`, `pleamar-wm ARGS...`, registered anew (or removed).
pub(crate) fn register_mcp(client: Client, name: &str, args: &[&str], undo: bool) -> Result<(), String> {
    let quiet = |c: &mut std::process::Command| c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    let (cmd, scope): (&str, &[&str]) = match client {
        Client::Claude => ("claude", &["--scope", "user"]),
        Client::Codex => ("codex", &[]),
    };
    quiet(std::process::Command::new(cmd).args(["mcp", "remove"]).args(scope).arg(name));
    if !undo && !quiet(std::process::Command::new(cmd).args(["mcp", "add"]).args(scope).args([name, "--", "pleamar-wm"]).args(args)) {
        return Err(format!("`{cmd} mcp add` did not go through"));
    }
    Ok(())
}

/// The plan said, and whether to go ahead: asked in a terminal, unless `yes`.
pub(crate) fn go_ahead(what: &str, plan: &[String], yes: bool) -> Result<bool, String> {
    println!("{what} will:\n  - {}", plan.join("\n  - "));
    if yes {
        return Ok(true);
    }
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err("run it in a terminal to answer, or with --yes".into());
    }
    print!("Go ahead? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut a = String::new();
    let _ = std::io::stdin().read_line(&mut a);
    Ok(matches!(a.trim(), "y" | "Y" | "s" | "S" | "yes" | "si" | "sí"))
}

/// `link setup`: what lets the agents use wires by themselves —the skill,
/// the MCP server (`link mcp`) for Claude Code and Codex, and Claude Code's
/// hooks (exact state and answers)—. Says what it will change and asks
/// first (`--yes` does not); `--undo` takes it all away.
fn setup(yes: bool, undo: bool) -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME")?;
    let has = |p: &str| std::path::Path::new(&format!("{home}/{p}")).is_dir();
    let (claude, codex) = clients()?;
    let mut plan: Vec<String> = Vec::new();
    for (ok, dir) in [(has(".claude"), ".claude/skills/pleamar-link"), (has(".codex"), ".codex/skills/pleamar-link")] {
        if ok {
            plan.push(format!("{} the skill ~/{dir}/SKILL.md", if undo { "remove" } else { "write" }));
        }
    }
    if claude {
        plan.push(mcp_plan(Client::Claude, "pleamar-link", undo));
        plan.push(format!("{} Claude Code's hooks for wires in ~/.claude/settings.json (a copy of it is kept as settings.json.before-pleamar-link)", if undo { "take away" } else { "add" }));
    }
    if codex {
        plan.push(mcp_plan(Client::Codex, "pleamar-link", undo));
    }
    if plan.is_empty() {
        return Err(NO_CLIENT.into());
    }
    if !go_ahead("pleamar-wm link setup", &plan, yes)? {
        return Ok("nothing changed".into());
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
    if claude {
        register_mcp(Client::Claude, "pleamar-link", &["link", "mcp"], undo)?;
        done.push("Claude Code's MCP server".into());
        claude_hooks(&format!("{home}/.claude/settings.json"), undo)?;
        done.push("Claude Code's hooks".into());
    }
    if codex {
        register_mcp(Client::Codex, "pleamar-link", &["link", "mcp"], undo)?;
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
fn start(exe: &str, cwd: &str, quiet: bool, name: Option<&str>) -> Result<u32, String> {
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
    // Its own name, or none: never the caller's.
    match name {
        Some(n) => c.env(crate::term::NAME, n),
        None => c.env_remove(crate::term::NAME),
    };
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
pub(crate) fn open_terminal(exe: &str, cwd: &str, quiet: bool, name: Option<&str>) -> Result<(usize, i32), String> {
    let before: Vec<usize> = windows()?.iter().map(|w| w.slot).collect();
    let known: Vec<i32> = crate::term::layers().iter().map(|(id, _)| *id).collect();
    let pid = start(exe, cwd, quiet, name)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        let wins = windows()?;
        // Its own window, or —a terminal that hands windows to a server of
        // its own— the new one that opened.
        let new = wins.iter().find(|w| w.pid == pid).or_else(|| wins.iter().find(|w| !before.contains(&w.slot)));
        let Some(w) = new else { continue };
        // Its new shell: one that serves many windows has others, at work.
        if let Some((id, _)) = crate::term::shells_of(w.pid, &crate::term::layers()).into_iter().find(|(id, _)| !known.contains(id)) {
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
    let (slot, id) = open_terminal(&exe, &cwd, true, None)?;
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
                c.arg("-f").arg(own_exe()).args(["link", "_handover", &slot.to_string(), &id.to_string(), &ask_key(&q), &task]);
                c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                c.spawn().map_err(|e| e.to_string())?;
                Ok(format!("{opened} Before your task it asks:\n{}\n{next}", said(slot, &q)))
            } else {
                eprintln!("{opened} Before the task it asks:\n{}\nAnswer it there or with `pleamar-wm link answer {slot}`; the task is handed once it is ready.", said(slot, &q));
                hand_over(slot, id, &ask_key(&q), &task)
            }
        }
        Ready::Gone(why) => Err(format!("{opened} But {why}: your task was not handed.")),
        Ready::Late => Err(format!("{opened} But it did not get ready in 60 s: hand it the task with link_delegate {slot} when it is")),
    }
}

/// `link open [--folder D] [--name NAME] [--terminal PROGRAM] [--quiet]
/// [COMMAND…]`: a terminal of its own, not beside anyone's and wired to
/// nobody, named, in that folder, with COMMAND typed into its shell once it is
/// there. What a shell starts its companions with (Calima's caravan): the name
/// is how it knows them again (`who.N`), and a named terminal is theirs to
/// bring back, so the kept canvas leaves it out.
fn open(rest: &[String]) -> Result<String, String> {
    const HOW: &str = "open [--folder DIR] [--name NAME] [--terminal PROGRAM] [--quiet] [COMMAND…]";
    let mut flags: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let (mut words, mut quiet, mut k) = (Vec::new(), false, 0);
    while k < rest.len() {
        match rest[k].as_str() {
            f @ ("--folder" | "--name" | "--terminal") => {
                flags.insert(f, rest.get(k + 1).cloned().ok_or(HOW)?);
                k += 2;
            }
            "--quiet" => {
                quiet = true;
                k += 1;
            }
            _ => {
                words.extend(rest[k..].iter().cloned());
                break;
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = flags.get("--folder").map(|f| if f == "~" || f.starts_with("~/") { f.replacen('~', &home, 1) } else { f.clone() }).unwrap_or_else(|| home.clone());
    if !std::path::Path::new(&cwd).is_dir() {
        return Err(format!("«{cwd}» is not a folder"));
    }
    let name = flags.get("--name").map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
    if let Some(n) = &name {
        if n.chars().count() > 64 || n.chars().any(char::is_control) {
            return Err(format!("«{n}» is not a name: one line, up to 64 letters"));
        }
    }
    // The one asked for; or the one the person uses (`$TERMINAL`); or the first there is.
    let on_path = |p: &str| std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(p).is_file()));
    let exe = flags
        .get("--terminal")
        .cloned()
        .or_else(|| std::env::var("TERMINAL").ok().filter(|t| !t.is_empty()))
        .or_else(|| ["alacritty", "kitty", "foot", "ghostty", "wezterm"].into_iter().find(|t| on_path(t)).map(str::to_owned))
        .ok_or("no terminal to open: --terminal PROGRAM, or $TERMINAL")?;
    let (slot, id) = open_terminal(&exe, &cwd, quiet, name.as_deref())?;
    let command = words.join(" ");
    if !command.is_empty() {
        // Once its shell is there to type into.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && info(id).is_empty() {
            std::thread::sleep(Duration::from_millis(200));
        }
        paste(id, &command)?;
        std::thread::sleep(Duration::from_millis(120));
        keys(id, "enter")?;
    }
    Ok(slot.to_string())
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
    // What was told, until it is gone from its screen a while (answered): then
    // the same again is another question.
    let (mut told, mut away) = (told.to_owned(), 0);
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
        let q = asked(id, &i);
        away = if q.as_ref().is_some_and(|q| ask_key(q) == told) { 0 } else { away + 1 };
        if away >= 2 {
            told.clear();
        }
        match q {
            Some(q) if ask_key(&q) != told => {
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
                    // opencode is quiet a while before it draws itself: its own screen, with its box.
                    "opencode" => i.get("fullscreen").map(String::as_str) == Some("1") && screen.lines().any(|l| l.trim_start().starts_with('┃')),
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
                told = ask_key(&q);
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

/// A letter out of a mailbox (mail.rs), handed by the messenger the watcher
/// left with it: typed into the terminal named so —a line that says what it
/// is, the letter as a paste, Enter—, as `delegate` hands a task; then, until
/// its agent is done, what it is at (working, asking its person) kept in the
/// mailbox for whoever shows it; and at the end its answer, kept with the letter.
fn mail_deliver(rest: &[String]) -> Result<String, String> {
    // Its number first, for the watcher that left it, and nothing more out.
    println!("{}", std::process::id());
    let _ = std::io::Write::flush(&mut std::io::stdout());
    if let Ok(null) = std::fs::OpenOptions::new().write(true).open("/dev/null") {
        use std::os::fd::AsRawFd;
        // SAFETY: both are open descriptors of this process; nothing else writes to stdout here.
        unsafe { libc::dup2(null.as_raw_fd(), 1) };
    }
    let name = rest.first().ok_or(crate::mail::USAGE)?.clone();
    let id: i32 = rest.get(2).and_then(|v| v.parse().ok()).ok_or(crate::mail::USAGE)?;
    let me = std::process::id() as u64;
    let letter = crate::mail::with(&name, |b| (b["out"]["pid"].as_u64() == Some(me)).then(|| b["out"].clone()))?.ok_or("no letter out for this messenger")?;
    let set = |state: &str| {
        let _ = crate::mail::with(&name, |b| {
            if b["out"]["pid"].as_u64() == Some(me) {
                b["out"]["state"] = serde_json::json!(state);
                if state == "working" {
                    b["out"]["given"] = serde_json::json!(now_ms());
                }
            }
        });
        say(&[format!("fact mail.seq {}", crate::mail::seq())]);
    };
    // The letter back where it was, first: not handed after all. `counts`: a
    // try that failed (its screen did not take what was typed); one that was
    // not free after all (an agent still starting, drawing its screen) is no
    // try, and the letter only waits again.
    let undo = |why: &str, counts: bool| -> Result<String, String> {
        let _ = crate::mail::with(&name, |b| {
            if b["out"]["pid"].as_u64() == Some(me) {
                let mut l = b["out"].take();
                let tries = l["tries"].as_u64().unwrap_or(0) + u64::from(counts);
                for k in ["pid", "state", "slot"] {
                    l.as_object_mut().map(|o| o.remove(k));
                }
                if tries >= 3 {
                    l["how"] = serde_json::json!("refused");
                    l["answer"] = serde_json::json!(why);
                    l["ended"] = serde_json::json!(now_ms());
                    crate::mail::keep_done(b, l);
                } else {
                    l["tries"] = serde_json::json!(tries);
                    if let Some(w) = b["waiting"].as_array_mut() {
                        w.insert(0, l);
                    }
                }
            }
        });
        say(&[format!("fact mail.seq {}", crate::mail::seq())]);
        Err(why.to_owned())
    };
    let i = info(id);
    if i.get("name").map(String::as_str) != Some(name.as_str()) {
        return undo("its terminal is not the one named so any more", true);
    }
    if !at_rest(&i) || asked(id, &i).is_some() || i.get("draft").map(String::as_str) == Some("1") {
        return undo("it was not free after all", false);
    }
    let task = letter["text"].as_str().unwrap_or("").to_owned();
    let from = letter["from"].as_str().filter(|f| !f.is_empty()).unwrap_or("your person");
    let before = read(id, 2000).lines().count();
    HANDED.with(|c| c.set(Some(hooked_answer(id).0)));
    HANDED_AT.with(|c| c.set(now_ms()));
    let _ = std::fs::write(format!("{}/{id}.handed", crate::term::socket_dir()), HANDED_AT.with(|c| c.get()).to_string());
    let _ = std::fs::write(format!("{}/{id}.handed-answers", crate::term::socket_dir()), HANDED.with(|c| c.get()).unwrap_or(0).to_string());
    let head = if letter["reply_to"].is_u64() {
        format!("[the answer of {from} to what you asked it, through the mailbox] It is in the pasted text: carry on with what you asked it for, and end with your answer.")
    } else {
        format!("[a task from {from}, through the mailbox] It is in the pasted text: do it, and end with your answer. To hand over a file, give its full path: it is offered to whoever reads it, on the phone too.")
    };
    let shown = read(id, 60);
    let mut took = false;
    for _ in 0..3 {
        paste(id, &head)?;
        let start = Instant::now();
        while !took && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(250));
            took = read(id, 60) != shown;
        }
        if took {
            break;
        }
    }
    if !took {
        return undo("its screen did not change with what was typed: is its agent ready?", true);
    }
    if paste_task(id, &task).is_err() {
        return undo("its screen did not change with the task pasted: is its agent ready?", true);
    }
    keys(id, "enter")?;
    let started = (0..10).any(|_| {
        std::thread::sleep(Duration::from_millis(400));
        let i = info(id);
        i.get("state").map(String::as_str) == Some("working") || i.get("quiet").and_then(|q| q.parse::<u64>().ok()).is_some_and(|q| q < 300)
    });
    if !started {
        keys(id, "enter")?;
    }
    set("working");
    // As long as it works (a day at most); six hours with no sign of it, and no more.
    let deadline = Instant::now() + Duration::from_secs(24 * 3600);
    let idle = Duration::from_secs(6 * 3600);
    let how = loop {
        match wait_for(id, deadline.saturating_duration_since(Instant::now()), true, Some(idle)) {
            Waited::Done => break "done",
            Waited::Late => break "late",
            Waited::Gone => break "gone",
            Waited::Asks(q) => {
                set("asks");
                let key = ask_key(&q);
                let mut away = 0;
                while Instant::now() < deadline && away < 2 {
                    std::thread::sleep(Duration::from_millis(700));
                    away = if asked(id, &info(id)).is_some_and(|n| ask_key(&n) == key) { 0 } else { away + 1 };
                }
                set("working");
            }
        }
    };
    let answer = match how {
        "done" => brought(id, before, deadline).unwrap_or_default(),
        "gone" => String::new(),
        _ => since(id, before),
    };
    crate::mail::with(&name, |b| {
        if b["out"]["pid"].as_u64() == Some(me) {
            let mut l = b["out"].take();
            l.as_object_mut().map(|o| o.remove("pid"));
            l["state"] = serde_json::json!("ended");
            l["how"] = serde_json::json!(how);
            l["answer"] = serde_json::json!(answer);
            l["ended"] = serde_json::json!(now_ms());
            crate::mail::keep_done(b, l);
        }
    })?;
    say(&[format!("fact mail.seq {}", crate::mail::seq())]);
    // From another companion (a named terminal or mailbox, however it posted
    // it), and not itself an answer nor marked `--no-reply`: its answer, as a
    // letter in that one's mailbox, from this one —it takes it in a turn of its
    // own, when it is free, as OpenBot's agents take their teammates' replies;
    // and its person sees it in that conversation—. Not to a routine or a
    // table: no mailbox has those names.
    let sender = letter["from"].as_str().unwrap_or("").to_owned();
    if !sender.is_empty() && letter["noreply"] != true && sender != name && crate::mail::valid(&sender).is_ok() {
        let known = crate::mail::all().iter().any(|b| b["name"].as_str() == Some(sender.as_str())) || crate::term::layers().iter().any(|(_, i)| i.get("name").map(String::as_str) == Some(sender.as_str()));
        if known {
            let title = crate::mail::all().iter().find(|b| b["name"].as_str() == Some(name.as_str())).and_then(|b| b["card"]["title"].as_str().filter(|t| !t.is_empty()).map(str::to_owned)).unwrap_or_else(|| name.clone());
            let asked: String = task.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
            let done = serde_json::json!({ "how": how, "answer": answer });
            let text = format!("{title}'s answer to «{asked}»:\n\n{}", mail_answer_text(&title, &done));
            let id = letter["id"].as_u64().unwrap_or(0).to_string();
            if let Err(e) = crate::mail::post(&[sender.clone(), text, "--from".into(), name.clone(), "--reply-to".into(), id]) {
                eprintln!("link · the answer for {sender} was not posted: {e}");
            }
        }
    }
    Ok(String::new())
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

/// How many answers of that terminal's agent (as its hooks count them) a
/// messenger has brought back.
fn brought_count(id: i32) -> u64 {
    std::fs::read_to_string(format!("{}/{id}.brought", crate::term::socket_dir())).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0)
}

fn taken_since(id: i32, at: u64) -> bool {
    at > 0 && std::fs::read_to_string(format!("{}/{id}.taken", crate::term::socket_dir())).ok().and_then(|t| t.trim().parse::<u64>().ok()).is_some_and(|t| t >= at)
}

/// Answers a question on that terminal: an option by its number or its
/// words (the cursor taken there, and Enter), words where it lets you type
/// them, or yes/no. What was chosen.
pub(crate) fn choose(id: i32, q: &crate::term::Question, reply: &str) -> Result<String, String> {
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
    fn an_answer_that_quotes_the_line_of_its_task_is_not_taken_for_it() {
        // Both pieces pasted at once, and the agent says them back: the line of the task is still the one in its box.
        let lines = [
            "❯ [a task from your person, through the mailbox] It is in the pasted text: do it, and end with your answer.haz la prueba uno",
            "",
            "● Hecho: [a task from your person, through the mailbox] It is in the p",
            "────────────────────────────────────────────────────────────",
            "❯ ",
        ];
        assert_eq!(handed_at(&lines), Some(0));
        assert_eq!(answer(&lines[1..]), "● Hecho: [a task from your person, through the mailbox] It is in the p");
        // Wrapped over two lines, the quote too.
        let wrapped = ["› [a task from your person, through the", "  mailbox] do it", "• Done: [a task from your person, through the", "  mailbox] do it"];
        assert_eq!(handed_at(&wrapped), Some(1));
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
        // Come while it worked, the notice is kept as a queued one.
        let queued = line(serde_json::json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-21T15:04:01.000Z", "content": "<task-notification>\n<task-id>wait333</task-id>\n<status>completed</status>\n</task-notification>"}));
        let all = [later.clone(), queued].join("\n");
        assert!(running(&all, handed).is_empty());
        let attached = line(serde_json::json!({"type": "attachment", "timestamp": "2026-10-21T15:04:01.000Z", "attachment": {"type": "queued_command", "prompt": "<task-notification>\n<task-id>wait333</task-id>\n<status>failed</status>\n</task-notification>"}}));
        assert!(running(&[later, attached].join("\n"), handed).is_empty());
        // What it wrote itself starts nothing.
        let wrote = line(serde_json::json!({"type": "assistant", "timestamp": "2026-10-21T15:05:00.000Z", "message": {"content": [{"type": "text", "text": "Command running in background with ID: fake1"}]}}));
        assert!(running(&wrote, handed).is_empty());
    }

    #[test]
    fn a_late_done_while_its_title_says_it_works_is_no_end() {
        let i = |hook: &str, state: &str| Info::from([("hook".to_owned(), hook.to_owned()), ("state".to_owned(), state.to_owned())]);
        assert!(at_rest(&i("done", "idle")));
        assert!(!at_rest(&i("done", "working")));
        assert!(!at_rest(&i("working", "idle")));
    }

    #[test]
    fn a_question_drawn_again_with_other_words_above_is_the_same() {
        let q = |text: &str| crate::term::Question { text: text.to_owned(), options: vec![(1, "Trust and continue".to_owned()), (2, "Quit".to_owned())], cursor: 0, yes_no: false };
        assert_eq!(ask_key(&q("Trust this folder?")), ask_key(&q("…your permission settings. Folder settings can run code…")));
        let yn = |text: &str| crate::term::Question { text: text.to_owned(), options: Vec::new(), cursor: 0, yes_no: true };
        assert_ne!(ask_key(&yn("Overwrite a? [y/N]")), ask_key(&yn("Overwrite b? [y/N]")));
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
