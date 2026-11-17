//! `pleamar-wm create …`: Build's engine as a service a desktop drives
//! (docs/create-api.md). The desktop —the «host»— says what kinds of thing it
//! makes (`$PLEAMAR_CONFIG/create-kinds/KIND.json`: a guide, a template, the
//! sizes, what its files must have), draws the whole conversation itself, and
//! runs what is made: this only keeps the drafts, their revisions and their
//! history, speaks to the engine, asks before anything that needs asking, and
//! checks each revision. It never starts what it makes (`activation=host`).
//!
//! Each answer is one JSON object a line on stdout, as `build status` and the
//! others (build_access.rs); `create prompt` stays and says what happens, a
//! line an event, until it is done. Nothing it prints holds a credential.

use crate::build_access::{clean, fail, say, Engine};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

pub const COMMANDS: &[&str] = &["kinds", "begin", "prompt", "permit", "cancel", "check", "keep", "undo", "discard", "remove", "list", "get"];

const USAGE: &str = "pleamar-wm create kinds | begin KIND [--size NAME|W,H] [--name NAME] | begin --from ID | prompt ID TEXT | permit ID REQ once|always|deny | cancel ID | check ID | keep ID | undo ID | discard ID | remove ID | list | get ID";

/// How long a question waits for its answer before it counts as «no».
const ASK_WAIT: Duration = Duration::from_secs(15 * 60);
/// Rounds the engine is given to mend what the check found, after a turn.
const MEND_ROUNDS: usize = 2;

fn data_root() -> PathBuf {
    let data = std::env::var("XDG_DATA_HOME").ok().filter(|v| !v.is_empty()).or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/share"))).unwrap_or_else(|| "/tmp".into());
    PathBuf::from(data).join("pleamar-wm/create")
}

/// Where the engine's sandbox sees `/state` (build/agent/sandbox).
fn engine_state() -> PathBuf {
    if let Some(s) = std::env::var("BUILD_AGENT_STATE").ok().filter(|v| !v.is_empty()) {
        return PathBuf::from(s);
    }
    let state = std::env::var("XDG_STATE_HOME").ok().filter(|v| !v.is_empty()).or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/state"))).unwrap_or_else(|| "/tmp".into());
    PathBuf::from(state).join("pleamar-wm/build")
}

/// Where a running conversation listens (`permit`, `cancel`): short, for a socket's name.
fn sock(id: &str) -> PathBuf {
    let run = std::env::var("XDG_RUNTIME_DIR").ok().filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    run.join(format!("pleamar-create-{id}.sock"))
}

/// A turn that says nothing for this long is taken as stuck.
const SILENT_FOR: Duration = Duration::from_secs(10 * 60);

fn kinds_dir() -> PathBuf {
    PathBuf::from(crate::config::user_dir().unwrap_or_else(|| "/tmp/pleamar".into())).join("create-kinds")
}

/// A name of a file of a creation, as the engine's tools allow them.
fn good_file(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    !stem.is_empty() && stem.len() <= 34 && stem.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') && ["plm", "luau", "svg", "json"].contains(&ext)
}

fn good_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ── kinds ──

#[derive(Clone)]
struct Kind {
    id: String,
    v: Value,
}

impl Kind {
    fn load(id: &str) -> Result<Kind, String> {
        if !good_id(id) {
            return Err(format!("'{id}' is not a kind's name"));
        }
        let path = kinds_dir().join(format!("{id}.json"));
        let text = std::fs::read_to_string(&path).map_err(|_| format!("there is no kind '{id}' ({})", path.display()))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Kind { id: id.to_owned(), v })
    }
    fn all() -> Vec<Kind> {
        let mut out: Vec<Kind> = std::fs::read_dir(kinds_dir())
            .map(|d| d.filter_map(Result::ok).filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_owned)).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| Kind::load(&id).ok())
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
    fn s(&self, key: &str) -> String {
        self.v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
    }
    fn list(&self, key: &str) -> Vec<String> {
        self.v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
    }
    fn main(&self) -> String {
        let m = self.s("main");
        if good_file(&m) { m } else { "scene.plm".to_owned() }
    }
    /// A path the kind names, beside its file when it is not whole.
    fn path(&self, key: &str) -> Option<PathBuf> {
        let p = self.s(key);
        if p.is_empty() {
            return None;
        }
        let p = if let Some(rest) = p.strip_prefix("~/") { PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest) } else { PathBuf::from(&p) };
        Some(if p.is_absolute() { p } else { kinds_dir().join(p) })
    }
    fn size(&self, asked: Option<&str>) -> Result<(u32, u32), String> {
        let sizes = self.v.get("sizes").and_then(Value::as_object).cloned().unwrap_or_default();
        let pair = |v: &Value| -> Option<(u32, u32)> { Some((v.get(0)?.as_u64()? as u32, v.get(1)?.as_u64()? as u32)) };
        match asked {
            Some(a) if a.contains(',') => {
                let (w, h) = a.split_once(',').unwrap();
                let (w, h) = (w.trim().parse::<u32>().map_err(|_| "a size is W,H")?, h.trim().parse::<u32>().map_err(|_| "a size is W,H")?);
                if !(16..=4096).contains(&w) || !(16..=4096).contains(&h) {
                    return Err("a size between 16 and 4096".into());
                }
                Ok((w, h))
            }
            Some(a) => sizes.get(a).and_then(pair).ok_or_else(|| format!("the kind '{}' has no size '{a}'", self.id)),
            None => {
                let def = self.s("default_size");
                sizes.get(&def).or_else(|| sizes.values().next()).and_then(pair).ok_or_else(|| format!("the kind '{}' says no size: ask one with --size W,H", self.id))
            }
        }
    }
    fn about(&self) -> Value {
        json!({ "id": self.id, "name": self.s("name"), "description": self.s("description"), "sizes": self.v.get("sizes").cloned().unwrap_or(json!({})), "default_size": self.s("default_size") })
    }
}

// ── a creation on disk ──
//
// create/ID/state.json   what it is: kind, name, size, revisions, history, what was allowed
// create/ID/files/       its files as they are now
// create/ID/revs/N/      each revision as it was
// $XDG_RUNTIME_DIR/pleamar-create-ID.sock while a `prompt` runs: `permit` and `cancel` speak to it

struct Creation {
    id: String,
    dir: PathBuf,
    st: Value,
}

impl Creation {
    fn open(id: &str) -> Creation {
        if !good_id(id) {
            fail("not_found", format!("'{id}' is not a creation"));
        }
        let dir = data_root().join(id);
        let st = std::fs::read_to_string(dir.join("state.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| fail("not_found", format!("there is no creation '{id}'")));
        Creation { id: id.to_owned(), dir, st }
    }
    fn files(&self) -> PathBuf {
        self.dir.join("files")
    }
    fn save(&self) {
        let tmp = self.dir.join("state.json.new");
        if std::fs::write(&tmp, serde_json::to_string_pretty(&self.st).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, self.dir.join("state.json"));
        }
    }
    fn kind(&self) -> Kind {
        let id = self.st["kind"].as_str().unwrap_or("").to_owned();
        Kind::load(&id).unwrap_or(Kind { id, v: json!({}) })
    }
    fn rev(&self) -> u64 {
        self.st["rev"].as_u64().unwrap_or(0)
    }
    fn file_names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.files()).map(|d| d.filter_map(Result::ok).filter_map(|e| e.file_name().to_str().map(str::to_owned)).filter(|n| good_file(n)).collect()).unwrap_or_default();
        v.sort();
        v
    }
    /// The files as they are now become a new revision.
    fn snapshot(&mut self, why: &str) -> u64 {
        let n = self.rev() + 1;
        let to = self.dir.join("revs").join(n.to_string());
        let _ = std::fs::create_dir_all(&to);
        for f in self.file_names() {
            let _ = std::fs::copy(self.files().join(&f), to.join(&f));
        }
        self.st["rev"] = json!(n);
        let mut revs = self.st["revs"].as_array().cloned().unwrap_or_default();
        revs.push(json!({ "rev": n, "why": why, "at": now_text() }));
        self.st["revs"] = Value::Array(revs);
        n
    }
    fn restore(&self, n: u64) -> bool {
        let from = self.dir.join("revs").join(n.to_string());
        if !from.is_dir() {
            return false;
        }
        // Its own files go back; what the host copied in (a library folder) stays.
        for f in self.file_names() {
            let _ = std::fs::remove_file(self.files().join(f));
        }
        let _ = std::fs::create_dir_all(self.files());
        for e in std::fs::read_dir(&from).into_iter().flatten().filter_map(Result::ok) {
            let _ = std::fs::copy(e.path(), self.files().join(e.file_name()));
        }
        true
    }
    fn history(&self) -> Vec<Value> {
        self.st["history"].as_array().cloned().unwrap_or_default()
    }
    fn said(&mut self, role: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let mut h = self.history();
        h.push(json!({ "role": role, "text": text, "at": now_text() }));
        self.st["history"] = Value::Array(h);
    }
    fn approved(&self, key: &str) -> Vec<String> {
        self.st["approved"][key].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
    }
    fn approve(&mut self, key: &str, v: &str) {
        let mut a = self.approved(key);
        if !a.iter().any(|x| x == v) {
            a.push(v.to_owned());
        }
        self.st["approved"][key] = json!(a);
    }
    fn manifest(&self) -> Value {
        std::fs::read_to_string(self.files().join("widget.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
    }
    /// The host's folders copied in beside its files (the kind's `copy`), afresh:
    /// what its scene includes from the host, for the check and the preview.
    fn copy_in(&self) {
        let kind = self.kind();
        let Some(map) = kind.v.get("copy").and_then(Value::as_object) else { return };
        for (dest, src) in map {
            let Some(src) = src.as_str() else { continue };
            if !good_id(dest) {
                continue;
            }
            let src = if let Some(r) = src.strip_prefix("~/") { PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(r) } else { PathBuf::from(src) };
            let src = if src.is_absolute() { src } else { kinds_dir().join(src) };
            let to = self.files().join(dest);
            let _ = std::fs::remove_dir_all(&to);
            copy_dir(&src, &to, 0);
        }
    }
    fn summary(&self) -> Value {
        let manifest = self.manifest();
        json!({
            "manifest": manifest, "made_by": manifest.get("made_by").and_then(Value::as_str).unwrap_or("ai"),
            "id": self.id, "kind": self.st["kind"], "name": self.st["name"], "size": self.st["size"],
            "state": self.st["state"], "rev": self.rev(), "kept_rev": self.st["kept_rev"],
            "dir": self.files(), "main": self.kind().main(), "updated": self.st["updated"],
        })
    }
}

fn now_text() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    secs.to_string()
}

fn new_id() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("k{}{:03}", t.as_secs(), t.subsec_millis())
}

// ── what a creation may do: Build's own rules (build/logic/creations.luau) ──

const SAFE_RUN: &[&str] = &["date", "playerctl", "brightnessctl", "notify-send", "wpctl", "pactl", "uptime", "uname", "hostname", "whoami"];
const DANGER_RUN: &[(&str, &str)] = &[
    ("rm", "delete files"), ("rmdir", "delete folders"), ("shred", "destroy files"), ("unlink", "delete files"),
    ("dd", "write straight onto disks"), ("truncate", "empty files"), ("wipefs", "erase disks"),
    ("fdisk", "change the disks' partitions"), ("parted", "change the disks' partitions"), ("mount", "mount disks"), ("umount", "unmount disks"),
    ("sudo", "do things as the administrator"), ("pkexec", "do things as the administrator"), ("su", "do things as another user"), ("doas", "do things as the administrator"),
    ("chmod", "change who can open files"), ("chown", "change who owns files"), ("chattr", "change how files can be changed"),
    ("systemctl", "start, stop and change system services"), ("loginctl", "end sessions"),
    ("shutdown", "turn the computer off"), ("reboot", "restart the computer"), ("poweroff", "turn the computer off"), ("halt", "stop the computer"),
    ("passwd", "change passwords"), ("useradd", "create users"), ("usermod", "change users"), ("userdel", "delete users"),
    ("crontab", "run things on its own at times"), ("iptables", "change the firewall"), ("nft", "change the firewall"), ("ufw", "change the firewall"),
    ("ssh", "connect to other computers"), ("scp", "send files to other computers"), ("rsync", "copy files elsewhere"),
    ("nc", "open network connections"), ("ncat", "open network connections"), ("socat", "open network connections"),
    ("pacman", "install or remove software"), ("yay", "install or remove software"), ("paru", "install or remove software"),
    ("apt", "install or remove software"), ("dnf", "install or remove software"), ("flatpak", "install or remove software"),
    ("mv", "move and replace files"), ("cp", "copy over files"), ("tee", "write files"), ("install", "replace files"),
    ("kill", "close programs"), ("pkill", "close programs"), ("killall", "close programs"),
];
const INTERPRETERS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "busybox", "env", "xargs", "find", "awk", "sed", "python", "python3", "perl", "ruby", "node", "lua", "luajit", "php", "osascript"];
const SAFE_SERVICES: &[&str] = &["clock", "clock.seconds", "audio", "media", "network", "battery", "brightness", "workspaces", "window", "apps", "tray", "notifications", "bluetooth", "files", "files.write", "files.remove"];
const ASK_SERVICES: &[(&str, &str)] = &[
    ("audio.*", "change the volume and the sound devices"), ("audio.volume", "change the volume"),
    ("media.*", "control the music that is playing"), ("brightness.*", "change the screen's brightness"),
    ("workspaces.*", "switch workspaces"), ("workspaces.focus", "switch workspaces"),
    ("window.*", "move and close your windows"), ("apps.launch", "open programs"),
    ("notifications.*", "answer and clear notifications"), ("tray.*", "use the icons in the tray"),
    ("network.*", "turn the network on and off, and connect to Wi-Fi"), ("bluetooth.*", "connect and disconnect Bluetooth devices"),
    ("clipboard", "read what you copy"), ("clipboard.set", "put things in what you copy"),
    ("env", "read the session's settings (environment)"), ("session.*", "log out, restart or turn off the computer"),
];
const DANGER_SERVICES: &[(&str, &str)] = &[("auth", "check your password")];
const READERS: &[&str] = &["ls", "cat", "head", "tail", "grep", "rg", "fd", "find", "which", "file", "stat", "wc", "du", "df", "ps", "uname", "date", "whoami", "free", "uptime", "lsblk", "lscpu", "tree", "jq", "realpath", "readlink", "basename", "dirname", "sort", "uniq"];
const FIND_ACTS: &[&str] = &["-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprintf", "-fls"];
const SECRET_WORDS: &[&str] = &[".ssh", ".gnupg", "password", "keyring", "auth.json", ".netrc", "credential", ".env", "cookies", "logins.json", "key4.db", "login data", "token", "secret", "/etc/shadow", ".pki", "wallet"];

fn danger(name: &str) -> Option<&'static str> {
    DANGER_RUN.iter().find(|(n, _)| *n == name).map(|(_, w)| *w).or(if name.starts_with("mkfs") { Some("format disks") } else { None })
}

fn secretive<'a>(words: impl IntoIterator<Item = &'a str>) -> bool {
    words.into_iter().any(|w| {
        let low = w.to_lowercase();
        SECRET_WORDS.iter().any(|s| low.contains(s))
    })
}

/// What a scene's `permissions` blocks ask: the programs it runs and the services it uses.
fn asks_of(content: &str) -> (Vec<String>, Vec<String>) {
    let (mut runs, mut services) = (Vec::new(), Vec::new());
    let mut rest = content;
    while let Some(at) = rest.find("permissions") {
        rest = &rest[at + "permissions".len()..];
        let t = rest.trim_start();
        let Some(body) = t.strip_prefix('{') else { continue };
        let Some(end) = body.find('}') else { break };
        for part in body[..end].split(';') {
            let Some((key, values)) = part.split_once(':') else { continue };
            let key = key.trim();
            let mut v = values;
            while let Some(a) = v.find('"') {
                let after = &v[a + 1..];
                let Some(b) = after.find('"') else { break };
                let val = after[..b].to_owned();
                if key == "run" {
                    runs.push(val);
                } else if key == "services" {
                    services.push(val);
                }
                v = &after[b + 1..];
            }
        }
        rest = &body[end..];
    }
    (runs, services)
}

/// A scene's permissions against Build's rules and what is approved already:
/// (refused, to ask [(key, value, words)], fine [(key, value)]).
#[allow(clippy::type_complexity)]
fn judge(content: &str, approved_run: &[String], approved_services: &[String]) -> (Vec<String>, Vec<(String, String, String)>, Vec<(String, String)>) {
    let (runs, services) = asks_of(content);
    let (mut refused, mut ask, mut fine) = (Vec::new(), Vec::new(), Vec::new());
    for r in runs {
        if approved_run.contains(&r) {
            continue;
        }
        let prog = r.split_whitespace().next().unwrap_or(&r).to_owned();
        let scripted = r.contains(char::is_whitespace);
        let name = prog.rsplit('/').next().unwrap_or(&prog).to_owned();
        if let Some(d) = danger(&name) {
            refused.push(format!("run «{r}», which can {d}"));
        } else if INTERPRETERS.contains(&name.as_str()) && scripted {
            ask.push(("run".into(), r.clone(), format!("run «{r}»: only that script, nothing else")));
        } else if prog.contains('/') || scripted {
            ask.push(("run".into(), r.clone(), format!("run «{r}», a program it does not know")));
        } else if SAFE_RUN.contains(&name.as_str()) {
            fine.push(("run".into(), r));
        } else if INTERPRETERS.contains(&name.as_str()) {
            ask.push(("run".into(), r.clone(), format!("run «{r}»: a program that runs other programs. Through it, this creation could do anything on your computer")));
        } else {
            ask.push(("run".into(), r.clone(), format!("run «{r}», a program it does not know")));
        }
    }
    for s in services {
        if approved_services.contains(&s) {
            continue;
        }
        if let Some((_, w)) = DANGER_SERVICES.iter().find(|(n, _)| *n == s) {
            refused.push((*w).to_owned());
        } else if SAFE_SERVICES.contains(&s.as_str()) {
            fine.push(("services".into(), s));
        } else {
            let words = ASK_SERVICES.iter().find(|(n, _)| *n == s).map(|(_, w)| (*w).to_owned()).unwrap_or_else(|| format!("use «{s}» of the system"));
            ask.push(("services".into(), s.clone(), words));
        }
    }
    (refused, ask, fine)
}

enum SystemVerdict {
    Fine,
    Refuse(String),
    Ask(String),
}

fn judge_system(tool: &str, args: &Value) -> SystemVerdict {
    if tool == "system_read" {
        let path = args["path"].as_str().unwrap_or("");
        return if secretive([path]) { SystemVerdict::Ask(format!("read «{path}», a file that may hold passwords or keys")) } else { SystemVerdict::Fine };
    }
    let program = args["program"].as_str().unwrap_or("");
    let name = program.rsplit('/').next().unwrap_or(program);
    let list: Vec<String> = args["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
    let command = if list.is_empty() { program.to_owned() } else { format!("{program} {}", list.join(" ")) };
    if let Some(d) = danger(name) {
        return SystemVerdict::Refuse(format!("Refused: «{name}» can {d}. Find it out another way."));
    }
    if secretive(list.iter().map(String::as_str)) {
        return SystemVerdict::Ask(format!("run, touching files that may hold passwords or keys: {command}"));
    }
    let acts = name == "find" && list.iter().any(|a| FIND_ACTS.contains(&a.as_str()));
    if READERS.contains(&name) && !acts {
        return SystemVerdict::Fine;
    }
    if name == "curl" && !list.is_empty() && list.iter().all(|a| is_quiet_flag(a) || a == "--compressed" || plain_https(a)) {
        return SystemVerdict::Fine;
    }
    let warn = if INTERPRETERS.contains(&name) { " (a program that runs other programs: it can do anything on your computer)" } else { "" };
    SystemVerdict::Ask(format!("run: {command}{warn}"))
}

fn is_quiet_flag(a: &str) -> bool {
    a.len() > 1 && a.starts_with('-') && !a.starts_with("--") && a[1..].chars().all(|c| "sSLf".contains(c))
}

fn plain_https(a: &str) -> bool {
    let Some(rest) = a.strip_prefix("https://") else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, "\u{1}"));
    !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') && path != "\u{1}" && path.chars().all(|c| c.is_ascii_alphanumeric() || "-._~/%@+=&?".contains(c))
}

/// The web addresses a creation's files name: what its permissions are for, said plainly.
fn domains(texts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in texts {
        for scheme in ["https://", "http://"] {
            let mut rest = t.as_str();
            while let Some(at) = rest.find(scheme) {
                rest = &rest[at + scheme.len()..];
                let host: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.').collect();
                let host = host.trim_end_matches('.').to_owned();
                if host.contains('.') && !out.contains(&host) {
                    out.push(host);
                }
            }
        }
    }
    out
}

// ── the check of a revision ──

/// No problem but warnings: what is nearly right (alignment) never keeps it from being kept.
fn ok_of(problems: &[Value]) -> bool {
    problems.iter().all(|p| p["level"] == "warning")
}

fn me() -> PathBuf {
    crate::link::own_exe()
}

/// What is wrong with the creation as it is now, for its host and for the engine to mend.
fn check(c: &Creation) -> Vec<Value> {
    let kind = c.kind();
    let main = kind.main();
    let mut problems = Vec::new();
    let path = c.files().join(&main);
    let Ok(text) = std::fs::read_to_string(&path) else {
        problems.push(json!({ "code": "missing", "file": main, "message": format!("{main} is not written yet") }));
        return problems;
    };
    let first = kind.s("first_line");
    if !first.is_empty() && text.lines().next().map(str::trim) != Some(first.trim()) {
        problems.push(json!({ "code": "first_line", "file": main, "message": format!("its first line must be «{first}»") }));
    }
    for need in kind.list("requires") {
        if good_file(&need) && !c.files().join(&need).is_file() {
            problems.push(json!({ "code": "missing", "file": need, "message": format!("{need} is missing") }));
        }
    }
    for f in c.file_names().into_iter().filter(|f| f.ends_with(".json")) {
        if let Ok(t) = std::fs::read_to_string(c.files().join(&f)) {
            if let Err(e) = serde_json::from_str::<Value>(&t) {
                problems.push(json!({ "code": "json", "file": f, "message": format!("{f} is not valid JSON: {e}") }));
            }
        }
    }
    let (refused, _, _) = judge(&text, &c.approved("run"), &c.approved("services"));
    for r in refused {
        problems.push(json!({ "code": "refused", "file": main, "message": format!("it asks to {r}: not allowed") }));
    }
    let allowed = kind.list("allow_run");
    if !allowed.is_empty() {
        for r in asks_of(&text).0 {
            let name = r.split_whitespace().next().unwrap_or(&r).rsplit('/').next().unwrap_or("").to_owned();
            if !allowed.contains(&name) {
                problems.push(json!({ "code": "not_for_this_kind", "file": main, "message": format!("this kind does not run «{name}»") }));
            }
        }
    }
    match std::process::Command::new(me()).arg("--check").arg(&path).current_dir(c.files()).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            problems.push(json!({ "code": "check", "file": main, "message": said.lines().filter(|l| !l.trim().is_empty()).take(6).collect::<Vec<_>>().join("\n") }));
        }
        Err(e) => problems.push(json!({ "code": "check", "file": main, "message": e.to_string() })),
    }
    if problems.iter().all(|p| p["code"] != "check") {
        problems.extend(layout(&path));
    }
    // What the host takes care of itself (a shadow it draws outside, say) is not a problem here.
    let ignore = kind.list("ignore");
    problems.retain(|p| !ignore.iter().any(|i| p["code"].as_str() == Some(i.as_str())));
    problems
}

fn copy_dir(from: &Path, to: &Path, depth: u32) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(from) else { return };
    let _ = std::fs::create_dir_all(to);
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()), depth + 1);
        } else if p.is_file() {
            let _ = std::fs::copy(&p, to.join(e.file_name()));
        }
    }
}

/// pleamar's report of how the scene is laid out (`--layout`), where this pleamar has it:
/// text over text, text cut or outside its surface, too little contrast, a shadow cut.
fn layout(path: &Path) -> Vec<Value> {
    let Ok(o) = std::process::Command::new(me()).arg("--layout").arg(path).current_dir(path.parent().unwrap_or(Path::new("."))).output() else { return Vec::new() };
    if !o.status.success() && o.stdout.is_empty() {
        return Vec::new(); // an older pleamar: no report, no problem made up
    }
    // Its last line is the report (a film says a word or two before it).
    let Some(v) = String::from_utf8_lossy(&o.stdout).lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok().filter(|v| v["type"] == "layout")) else { return Vec::new() };
    let warnings = v.get("warnings").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(|mut w| {
        w["level"] = json!("warning");
        w
    });
    v.get("problems").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().chain(warnings).map(|mut p| {
        if p.get("file").is_none() {
            p["file"] = json!(path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
        }
        p
    }).collect()
}

// ── commands ──

pub fn run(args: &[String]) {
    let what = args.first().map(String::as_str).unwrap_or("");
    let arg = |k: usize| args.get(k).map(String::as_str);
    match what {
        "kinds" => say(json!({ "type": "kinds", "kinds": Kind::all().iter().map(Kind::about).collect::<Vec<_>>() })),
        "begin" => begin(&args[1..]),
        "prompt" => match (arg(1), arg(2)) {
            (Some(id), Some(text)) => prompt(id, text),
            _ => fail("usage", USAGE),
        },
        "permit" => match (arg(1), arg(2), arg(3)) {
            (Some(id), Some(req), Some(d @ ("once" | "always" | "deny"))) => control(id, json!({ "type": "permit", "req": req, "decision": d })),
            _ => fail("usage", USAGE),
        },
        "cancel" => match arg(1) {
            Some(id) => control(id, json!({ "type": "cancel" })),
            None => fail("usage", USAGE),
        },
        "check" => match arg(1) {
            Some(id) => {
                let c = Creation::open(id);
                c.copy_in();
                let problems = check(&c);
                say(json!({ "type": "check", "id": id, "rev": c.rev(), "ok": ok_of(&problems), "problems": problems }));
            }
            None => fail("usage", USAGE),
        },
        "keep" => match arg(1) {
            Some(id) => keep(id),
            None => fail("usage", USAGE),
        },
        "undo" => match arg(1) {
            Some(id) => undo(id),
            None => fail("usage", USAGE),
        },
        "discard" => match arg(1) {
            Some(id) => discard(id),
            None => fail("usage", USAGE),
        },
        "remove" => match arg(1) {
            Some(id) => {
                let c = Creation::open(id);
                busy_check(&c);
                let _ = std::fs::remove_dir_all(&c.dir);
                say(json!({ "type": "removed", "id": id }));
            }
            None => fail("usage", USAGE),
        },
        "list" => {
            let mut all: Vec<Value> = std::fs::read_dir(data_root())
                .map(|d| d.filter_map(Result::ok).filter_map(|e| e.file_name().to_str().map(str::to_owned)).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .filter(|id| good_id(id) && data_root().join(id).join("state.json").is_file())
                .map(|id| Creation::open(&id).summary())
                .collect();
            all.sort_by(|a, b| b["updated"].as_str().unwrap_or("").cmp(a["updated"].as_str().unwrap_or("")));
            say(json!({ "type": "list", "creations": all }));
        }
        "get" => match arg(1) {
            Some(id) => {
                let c = Creation::open(id);
                let mut v = c.summary();
                v["type"] = json!("creation");
                v["files"] = json!(c.file_names());
                v["revs"] = c.st["revs"].clone();
                v["history"] = json!(c.history());
                v["approved"] = c.st["approved"].clone();
                v["busy"] = json!(running(&c));
                say(v);
            }
            None => fail("usage", USAGE),
        },
        "--help" | "help" => say(json!({ "type": "usage", "message": USAGE, "commands": COMMANDS })),
        other => fail("usage", format!("'{other}' is not a create command: {USAGE}")),
    }
}

fn begin(args: &[String]) {
    let mut kind_id = None;
    let (mut size, mut name, mut from) = (None, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--size" => { size = args.get(i + 1).cloned(); i += 1; }
            "--name" => { name = args.get(i + 1).cloned(); i += 1; }
            "--from" => { from = args.get(i + 1).cloned(); i += 1; }
            k if kind_id.is_none() && !k.starts_with("--") => kind_id = Some(k.to_owned()),
            other => fail("usage", format!("'{other}': {USAGE}")),
        }
        i += 1;
    }
    if let Some(id) = from {
        // Again with one that exists: a new conversation over it, its history kept;
        // `discard` goes back to the revision that was kept.
        let mut c = Creation::open(&id);
        busy_check(&c);
        c.st["state"] = json!(if c.st["kept_rev"].is_u64() { "editing" } else { "draft" });
        c.st["updated"] = json!(now_text());
        c.save();
        let mut v = c.summary();
        v["type"] = json!("begun");
        v["history"] = json!(c.history());
        say(v);
        return;
    }
    let kind = Kind::load(&kind_id.unwrap_or_else(|| fail("usage", USAGE))).unwrap_or_else(|e| fail("no_kind", e));
    let (w, h) = kind.size(size.as_deref()).unwrap_or_else(|e| fail("usage", e));
    let id = new_id();
    let dir = data_root().join(&id);
    if std::fs::create_dir_all(dir.join("files")).is_err() {
        fail("io", format!("cannot write {}", dir.display()));
    }
    let mut c = Creation {
        id: id.clone(),
        dir,
        st: json!({
            "id": id, "kind": kind.id, "name": name.unwrap_or_else(|| kind.s("name")), "size": [w, h],
            "state": "draft", "rev": 0, "revs": [], "history": [], "approved": { "run": [], "services": [], "system": [] },
            "kept_rev": null, "created": now_text(), "updated": now_text(),
        }),
    };
    c.copy_in();
    // The kind's template, with its size said in it, is where the engine starts.
    if let Some(t) = kind.path("template").and_then(|p| std::fs::read_to_string(p).ok()) {
        let t = t.replace("{{W}}", &w.to_string()).replace("{{H}}", &h.to_string()).replace("{{NAME}}", c.st["name"].as_str().unwrap_or(""));
        let _ = std::fs::write(c.files().join(kind.main()), t);
    }
    if c.files().join(kind.main()).is_file() {
        c.snapshot("template");
    }
    c.save();
    let mut v = c.summary();
    v["type"] = json!("begun");
    say(v);
}

fn keep(id: &str) {
    let mut c = Creation::open(id);
    busy_check(&c);
    c.copy_in();
    let problems = check(&c);
    if problems.iter().any(|p| matches!(p["code"].as_str(), Some("check" | "missing" | "first_line" | "refused" | "json" | "not_for_this_kind"))) {
        say(json!({ "type": "error", "error": "not_ready", "message": "the check finds what keeps it from running", "problems": problems }));
        std::process::exit(1);
    }
    c.st["state"] = json!("kept");
    c.st["kept_rev"] = json!(c.rev());
    c.st["updated"] = json!(now_text());
    c.save();
    let main_text = std::fs::read_to_string(c.files().join(c.kind().main())).unwrap_or_default();
    let (runs, services) = asks_of(&main_text);
    let mut v = c.summary();
    v["type"] = json!("kept");
    v["permissions"] = json!({ "run": runs, "services": services });
    v["warnings"] = json!(problems);
    say(v);
}

fn undo(id: &str) {
    let mut c = Creation::open(id);
    busy_check(&c);
    let n = c.rev();
    let floor = c.st["kept_rev"].as_u64().unwrap_or(1).max(1);
    if n <= floor || !c.restore(n - 1) {
        fail("nothing_to_undo", "there is no earlier revision of this conversation");
    }
    let _ = std::fs::remove_dir_all(c.dir.join("revs").join(n.to_string()));
    let mut revs = c.st["revs"].as_array().cloned().unwrap_or_default();
    revs.retain(|r| r["rev"].as_u64() != Some(n));
    c.st["revs"] = Value::Array(revs);
    c.st["rev"] = json!(n - 1);
    c.st["updated"] = json!(now_text());
    c.save();
    say(json!({ "type": "revision", "id": id, "rev": n - 1, "dir": c.files(), "files": c.file_names(), "why": "undone" }));
}

fn discard(id: &str) {
    let mut c = Creation::open(id);
    busy_check(&c);
    match c.st["kept_rev"].as_u64() {
        // Being edited: back to what was kept, the rest of the conversation's revisions gone.
        Some(k) => {
            c.restore(k);
            for r in (k + 1)..=c.rev() {
                let _ = std::fs::remove_dir_all(c.dir.join("revs").join(r.to_string()));
            }
            let mut revs = c.st["revs"].as_array().cloned().unwrap_or_default();
            revs.retain(|r| r["rev"].as_u64().is_some_and(|n| n <= k));
            c.st["revs"] = Value::Array(revs);
            c.st["rev"] = json!(k);
            c.st["state"] = json!("kept");
            c.save();
            say(json!({ "type": "discarded", "id": id, "back_to": k }));
        }
        None => {
            let _ = std::fs::remove_dir_all(&c.dir);
            say(json!({ "type": "discarded", "id": id }));
        }
    }
}

fn running(c: &Creation) -> bool {
    std::os::unix::net::UnixStream::connect(sock(&c.id)).is_ok()
}

fn busy_check(c: &Creation) {
    if running(c) {
        fail("busy", "a conversation is going on with it: cancel it first");
    }
}

/// `permit` and `cancel`: said to the `prompt` that is running.
fn control(id: &str, order: Value) {
    let c = Creation::open(id);
    let Ok(mut s) = std::os::unix::net::UnixStream::connect(sock(&c.id)) else {
        fail("not_running", "no conversation is going on with it");
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = writeln!(s, "{order}");
    let mut answer = String::new();
    let _ = BufReader::new(&s).read_line(&mut answer);
    match serde_json::from_str::<Value>(answer.trim()) {
        Ok(v) if v["type"] == "error" => {
            say(v);
            std::process::exit(1);
        }
        Ok(v) => say(v),
        Err(_) => fail("not_running", "the conversation did not answer"),
    }
}

// ── a conversation ──

enum Order {
    Permit { req: String, decision: String, answer: std::os::unix::net::UnixStream },
    Cancel { answer: std::os::unix::net::UnixStream },
}

fn listen(c: &Creation) -> Receiver<Order> {
    let path = sock(&c.id);
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap_or_else(|e| fail("io", format!("{}: {e}", path.display())));
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for s in listener.incoming().map_while(Result::ok) {
            let mut line = String::new();
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            if BufReader::new(&s).read_line(&mut line).is_err() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            let order = match v["type"].as_str() {
                Some("permit") => Order::Permit { req: v["req"].as_str().unwrap_or("").to_owned(), decision: v["decision"].as_str().unwrap_or("deny").to_owned(), answer: s },
                Some("cancel") => Order::Cancel { answer: s },
                _ => continue,
            };
            if tx.send(order).is_err() {
                break;
            }
        }
    });
    rx
}

struct Talk {
    c: Creation,
    engine: Engine,
    orders: Receiver<Order>,
    reqs: u64,
    text: String,
    changed: bool,
    cancelled: bool,
    last_why: String,
    looks: u64,
    failed: bool,
}

fn status(code: &str, text: &str) {
    say(json!({ "type": "status", "code": code, "text": text }));
}

fn prompt(id: &str, user: &str) {
    let mut c = Creation::open(id);
    if running(&c) {
        fail("busy", "a conversation is going on with it already");
    }
    let orders = listen(&c);
    c.copy_in();
    let kind = c.kind();
    status("starting", "Starting");
    let engine = Engine::start();
    c.said("user", user);
    c.st["updated"] = json!(now_text());
    c.save();
    let mut t = Talk { c, engine, orders, reqs: 0, text: String::new(), changed: false, cancelled: false, last_why: String::new(), looks: 0, failed: false };
    let (w, h) = (t.c.st["size"][0].as_u64().unwrap_or(0), t.c.st["size"][1].as_u64().unwrap_or(0));
    let guide = kind.path("guide").and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let docs = |topic: &str| std::process::Command::new(me()).args(["--docs", topic]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut history: Vec<Value> = t.c.history().into_iter().map(|h| json!({ "role": h["role"], "text": h["text"] })).collect();
    history.pop(); // the message said now goes as the prompt
    let model = crate::build_access::chosen().unwrap_or_else(|| crate::build_access::DEFAULT_MODEL.to_owned());
    t.engine.send(json!({
        "type": "start", "model": model, "locale": std::env::var("LANG").unwrap_or_default().split(['.', '_']).next().unwrap_or("en"),
        "history": history,
        "build": {
            "name": t.c.st["name"], "kind": format!("{}: {}", kind.s("name"), kind.s("description")),
            "region": { "monitor": "", "mw": w, "mh": h, "x": 0, "y": 0, "w": w, "h": h },
            "files": t.c.file_names(), "skill": docs("skill"), "desktop": docs("desktop"), "guide": guide,
        }
    }));
    if !t.wait_ready() {
        t.failed = true;
        t.finish();
        return;
    }
    let mut round = 0;
    let mut message = user.to_owned();
    loop {
        t.engine.send(json!({ "type": "prompt", "text": message, "now": now_text() }));
        status("thinking", "Thinking");
        let ended = t.turn();
        if !ended || t.cancelled {
            break;
        }
        if !t.changed {
            break;
        }
        status("checking", "Checking the result");
        let problems = check(&t.c);
        say(json!({ "type": "check", "id": t.c.id, "rev": t.c.rev(), "ok": ok_of(&problems), "problems": problems }));
        if problems.is_empty() || round >= MEND_ROUNDS {
            break;
        }
        round += 1;
        status("mending", "Fixing what the check found");
        message = format!(
            "The automatic check of this revision found problems. Fix them, then check and look again:\n{}",
            problems.iter().map(|p| format!("- {} ({}): {}", p["code"].as_str().unwrap_or(""), p["file"].as_str().unwrap_or(""), p["message"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")
        );
        t.changed = false;
    }
    t.finish();
}

impl Talk {
    fn wait_ready(&mut self) -> bool {
        let end = Instant::now() + Duration::from_secs(120);
        while Instant::now() < end {
            match self.engine.recv(Duration::from_millis(200)) {
                Ok(Some(v)) if v["type"] == "ready" => {
                    if v["usable"] == true {
                        return true;
                    }
                    let reason = v["reason"].as_str().unwrap_or("signed_out");
                    say(json!({ "type": "error", "error": "auth_required", "reason": reason, "message": "Build's AI cannot be used now: sign in (pleamar-wm build login)" }));
                    return false;
                }
                Ok(Some(v)) if v["type"] == "failed" => {
                    say(json!({ "type": "error", "error": "engine_failed", "message": clean(v["reason"].as_str().unwrap_or("")) }));
                    return false;
                }
                Ok(_) => {}
                Err(why) => {
                    say(json!({ "type": "error", "error": "engine_failed", "message": why }));
                    return false;
                }
            }
            if self.take_cancel() {
                return false;
            }
        }
        say(json!({ "type": "error", "error": "timeout", "message": "the engine did not start" }));
        false
    }

    fn take_cancel(&mut self) -> bool {
        while let Ok(o) = self.orders.try_recv() {
            match o {
                Order::Cancel { mut answer } => {
                    let _ = writeln!(answer, "{}", json!({ "type": "cancelling", "id": self.c.id }));
                    self.engine.send(json!({ "type": "cancel" }));
                    self.cancelled = true;
                }
                Order::Permit { mut answer, .. } => {
                    let _ = writeln!(answer, "{}", json!({ "type": "error", "error": "not_asked", "message": "nothing is being asked now" }));
                }
            }
        }
        self.cancelled
    }

    fn flush_text(&mut self) {
        let text = std::mem::take(&mut self.text);
        if !text.trim().is_empty() {
            say(json!({ "type": "message", "role": "assistant", "text": text }));
            self.c.said("assistant", text.trim());
            self.c.save();
        }
    }

    /// One turn of the engine, until it is done (true) or fails or is cancelled (false).
    fn turn(&mut self) -> bool {
        let mut heard = Instant::now();
        loop {
            if heard.elapsed() > SILENT_FOR {
                self.flush_text();
                say(json!({ "type": "error", "error": "timeout", "message": format!("the engine said nothing for {} minutes", SILENT_FOR.as_secs() / 60) }));
                self.engine.send(json!({ "type": "cancel" }));
                self.failed = true;
                return false;
            }
            if self.take_cancel() && self.cancelled {
                // it answers `cancelled` and that ends the turn below
            }
            match self.engine.recv(Duration::from_millis(200)) {
                Ok(None) => continue,
                Ok(Some(v)) => match { heard = Instant::now(); v["type"].as_str().unwrap_or("") } {
                    "delta" => self.text.push_str(v["text"].as_str().unwrap_or("")),
                    "propose" => {
                        self.flush_text();
                        let (pid, tool) = (v["id"].as_str().unwrap_or("").to_owned(), v["tool"].as_str().unwrap_or("").to_owned());
                        let (ok, text, denied, image) = self.tool(&tool, &v["args"]);
                        let mut r = json!({ "type": "result", "id": pid, "ok": ok, "text": text, "denied": denied });
                        if let Some(img) = image {
                            r["image"] = json!({ "path": img });
                        }
                        self.engine.send(r);
                        // A question may have waited for the user: the engine's silence counts from now.
                        heard = Instant::now();
                    }
                    "done" => {
                        self.flush_text();
                        return true;
                    }
                    "cancelled" => {
                        self.flush_text();
                        self.cancelled = true;
                        return false;
                    }
                    "failed" => {
                        self.flush_text();
                        say(json!({ "type": "error", "error": "engine_failed", "message": clean(v["reason"].as_str().unwrap_or("")) }));
                        self.failed = true;
                        return false;
                    }
                    _ => {}
                },
                Err(why) => {
                    self.flush_text();
                    say(json!({ "type": "error", "error": "engine_failed", "message": why }));
                    self.failed = true;
                    return false;
                }
            }
        }
    }

    /// A question to the host; its answer, or «deny» when it does not come.
    fn ask(&mut self, what: &str, detail: Value) -> String {
        self.reqs += 1;
        let req = format!("r{}", self.reqs);
        let mut ev = json!({ "type": "permission", "id": self.c.id, "req": req, "what": what });
        if let Value::Object(m) = detail {
            for (k, v) in m {
                ev[k] = v;
            }
        }
        say(ev);
        status("waiting_permission", "Waiting for your answer");
        let end = Instant::now() + ASK_WAIT;
        while Instant::now() < end {
            match self.orders.recv_timeout(Duration::from_millis(300)) {
                Ok(Order::Permit { req: r, decision, mut answer }) if r == req => {
                    let _ = writeln!(answer, "{}", json!({ "type": "permitted", "id": self.c.id, "req": req, "decision": decision }));
                    return decision;
                }
                Ok(Order::Permit { mut answer, .. }) => {
                    let _ = writeln!(answer, "{}", json!({ "type": "error", "error": "not_asked", "message": format!("the question now is {req}") }));
                }
                Ok(Order::Cancel { mut answer }) => {
                    let _ = writeln!(answer, "{}", json!({ "type": "cancelling", "id": self.c.id }));
                    self.engine.send(json!({ "type": "cancel" }));
                    self.cancelled = true;
                    return "deny".into();
                }
                Err(_) => {}
            }
        }
        "deny".into()
    }

    /// What the engine proposed, done or not: (ok, text for it, denied, a picture's path in its sandbox).
    fn tool(&mut self, tool: &str, args: &Value) -> (bool, String, bool, Option<String>) {
        let file = args["file"].as_str().unwrap_or("").to_owned();
        match tool {
            "creation_read" => {
                status("reading", &format!("Reading {file}"));
                if !good_file(&file) {
                    return (false, "Not a file of the creation.".into(), false, None);
                }
                match std::fs::read_to_string(self.c.files().join(&file)) {
                    Ok(t) => (true, t, false, None),
                    Err(_) => (false, format!("{file} does not exist yet."), false, None),
                }
            }
            "creation_write" => {
                status("writing", &format!("Writing {file}"));
                let content = args["content"].as_str().unwrap_or("");
                let why = args["why"].as_str().unwrap_or("").to_owned();
                if !good_file(&file) || content.len() > 512 * 1024 {
                    return (false, "Not a file of the creation, or too big.".into(), false, None);
                }
                if file.ends_with(".plm") {
                    let (refused, asks, fine) = judge(content, &self.c.approved("run"), &self.c.approved("services"));
                    if !refused.is_empty() {
                        return (false, format!("This version was refused: it asks to {}. A creation may not do that. Do it without it.", refused.join("; ")), true, None);
                    }
                    if !asks.is_empty() {
                        let texts = self.texts_with(&file, content);
                        let words: Vec<String> = asks.iter().map(|a| a.2.clone()).collect();
                        let answer = self.ask(
                            "scene",
                            json!({ "asks": asks.iter().map(|a| json!({ "key": a.0, "value": a.1, "words": a.2 })).collect::<Vec<_>>(), "domains": domains(&texts), "why": why, "summary": words.join("; ") }),
                        );
                        if answer == "deny" {
                            return (false, format!("The user said no to: {}. Do it without that.", words.join("; ")), true, None);
                        }
                        for a in &asks {
                            self.c.approve(&a.0, &a.1);
                        }
                    }
                    for (k, v) in fine {
                        self.c.approve(&k, &v);
                    }
                }
                if std::fs::write(self.c.files().join(&file), content).is_err() {
                    return (false, "Could not write it.".into(), false, None);
                }
                self.changed = true;
                self.last_why = why.clone();
                let n = self.c.snapshot(&why);
                self.c.st["updated"] = json!(now_text());
                self.c.save();
                say(json!({ "type": "revision", "id": self.c.id, "rev": n, "dir": self.c.files(), "files": self.c.file_names(), "why": why, "domains": domains(&self.texts_with(&file, content)) }));
                let check = if file.ends_with(".plm") { self.check_text() } else { String::new() };
                (true, format!("Written: {file}.{check}"), false, None)
            }
            "creation_check" => {
                status("checking", "Checking it");
                let p = check(&self.c);
                (p.is_empty(), if p.is_empty() { "ok".into() } else { p.iter().map(|x| x["message"].as_str().unwrap_or("").to_owned()).collect::<Vec<_>>().join("\n") }, false, None)
            }
            "creation_look" => {
                status("looking", "Looking at the result");
                self.look()
            }
            "creation_name" => {
                let name: String = args["name"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(40).collect();
                self.c.st["name"] = json!(name);
                self.c.save();
                say(json!({ "type": "named", "id": self.c.id, "name": name }));
                (true, format!("Named «{name}»."), false, None)
            }
            "pleamar_docs" => {
                let topic = args["topic"].as_str().unwrap_or("reference");
                status("reading_docs", &format!("Reading pleamar's {topic}"));
                let argv: Vec<&str> = if topic == "grammar" { vec!["--grammar"] } else { vec!["--docs", topic] };
                match std::process::Command::new(me()).args(&argv).output() {
                    Ok(o) if o.status.success() => (true, String::from_utf8_lossy(&o.stdout).chars().take(60000).collect(), false, None),
                    _ => (false, "Not available.".into(), false, None),
                }
            }
            "system_run" | "system_read" => self.system(tool, args),
            "creation_key" | "creation_poke" => (false, "Not available here: this creation is made for a desktop that shows it and uses it itself.".into(), false, None),
            other => (false, format!("Unknown tool «{other}»."), false, None),
        }
    }

    fn texts_with(&self, file: &str, content: &str) -> Vec<String> {
        let mut texts: Vec<String> = self.c.file_names().iter().filter(|f| f.as_str() != file).filter_map(|f| std::fs::read_to_string(self.c.files().join(f)).ok()).collect();
        texts.push(content.to_owned());
        texts
    }

    fn check_text(&self) -> String {
        let main = self.c.kind().main();
        match std::process::Command::new(me()).arg("--check").arg(self.c.files().join(&main)).current_dir(self.c.files()).output() {
            Ok(o) if o.status.success() => format!(" Check: {}", String::from_utf8_lossy(&o.stdout).trim()),
            Ok(o) => format!(" Check failed: {}{}", String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()),
            Err(_) => String::new(),
        }
    }

    /// A picture of the scene with no screen (pleamar's `--film`), where the engine can see it:
    /// with no permissions —it runs nothing, uses no service, reaches no network—, as `--layout`
    /// does: a draft is looked at, never run as itself (the host does that, once it is approved).
    fn look(&mut self) -> (bool, String, bool, Option<String>) {
        self.looks += 1;
        let main = self.c.files().join(self.c.kind().main());
        let rel = format!("create-looks/{}-{}", self.c.id, self.looks);
        let out = engine_state().join(&rel);
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::create_dir_all(&out);
        let ran = std::process::Command::new(me()).arg("--scene").arg(&main).args(["--no-permissions", "--no-hud", "--seconds", "1.5", "--fps", "4", "--film"]).arg(&out).current_dir(self.c.files()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        let last = std::fs::read_dir(&out).ok().and_then(|d| d.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".png")).max());
        match (ran, last) {
            (Ok(_), Some(png)) => (true, "This is how it looks now, with no screen (the host shows it where it goes).".into(), false, Some(format!("/state/{rel}/{png}"))),
            _ => (false, "No picture can be taken here: rely on creation_check.".into(), false, None),
        }
    }

    fn system(&mut self, tool: &str, args: &Value) -> (bool, String, bool, Option<String>) {
        let what = if tool == "system_read" { args["path"].as_str().unwrap_or("").to_owned() } else { format!("{} {}", args["program"].as_str().unwrap_or(""), args["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default()).trim().to_owned() };
        status("running", &format!("{} {what}", if tool == "system_read" { "Reading" } else { "Running" }));
        match judge_system(tool, args) {
            SystemVerdict::Refuse(t) => return (false, t, true, None),
            SystemVerdict::Ask(words) => {
                let key = format!("{tool} {what}");
                if !self.c.approved("system").contains(&key) {
                    let answer = self.ask(if tool == "system_read" { "read" } else { "run" }, json!({ "command": what, "words": words, "why": args["why"], "domains": domains(&[what.clone()]) }));
                    match answer.as_str() {
                        "deny" => return (false, "The user said no. Find it out another way, or do without it.".into(), true, None),
                        "always" => {
                            self.c.approve("system", &key);
                            self.c.save();
                        }
                        _ => {}
                    }
                }
            }
            SystemVerdict::Fine => {}
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let expand = |p: &str| if let Some(r) = p.strip_prefix("~/") { format!("{home}/{r}") } else if p == "~" { home.clone() } else { p.to_owned() };
        if tool == "system_read" {
            return match std::fs::read(expand(args["path"].as_str().unwrap_or(""))) {
                Ok(b) => (true, String::from_utf8_lossy(&b[..b.len().min(64 * 1024)]).into_owned(), false, None),
                Err(e) => (false, format!("Could not read it: {e}"), false, None),
            };
        }
        let program = args["program"].as_str().unwrap_or("");
        let list: Vec<String> = args["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(|s| expand(s)).collect()).unwrap_or_default();
        let child = std::process::Command::new(expand(program)).args(&list).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn();
        let Ok(child) = child else { return (false, format!("«{program}» could not start."), false, None) };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(o)) => {
                let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
                if !o.status.success() {
                    out.push_str(&String::from_utf8_lossy(&o.stderr));
                }
                (o.status.success(), out.chars().take(24 * 1024).collect(), false, None)
            }
            _ => (false, "It took too long (20 s) or failed.".into(), false, None),
        }
    }

    fn finish(mut self) {
        self.flush_text();
        let summary = self.c.history().iter().rev().find(|h| h["role"] == "assistant").and_then(|h| h["text"].as_str()).unwrap_or("").to_owned();
        self.c.st["updated"] = json!(now_text());
        self.c.save();
        self.engine.stop();
        let _ = std::fs::remove_file(sock(&self.c.id));
        if self.failed {
            std::process::exit(1);
        }
        if self.cancelled {
            say(json!({ "type": "cancelled", "id": self.c.id, "rev": self.c.rev() }));
        } else {
            say(json!({ "type": "done", "id": self.c.id, "rev": self.c.rev(), "changed": self.changed || self.c.rev() > 0, "summary": summary, "why": self.last_why }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scenes_permissions_are_read() {
        let (runs, services) = asks_of("scene X { permissions { run: \"curl\", \"date\"; services: \"clock\", \"audio.*\" } }");
        assert_eq!(runs, ["curl", "date"]);
        assert_eq!(services, ["clock", "audio.*"]);
    }

    #[test]
    fn builds_rules_judge_them() {
        let (refused, ask, fine) = judge("permissions { run: \"rm\", \"curl\", \"date\", \"sh -c x\"; services: \"clock\", \"auth\", \"env\" }", &[], &[]);
        assert_eq!(refused.len(), 2, "{refused:?}"); // rm, auth
        assert_eq!(ask.iter().map(|a| a.1.as_str()).collect::<Vec<_>>(), ["curl", "sh -c x", "env"]);
        assert_eq!(fine, [("run".to_owned(), "date".to_owned()), ("services".to_owned(), "clock".to_owned())]);
        // What was approved is not asked again.
        let (_, ask, _) = judge("permissions { run: \"curl\" }", &["curl".to_owned()], &[]);
        assert!(ask.is_empty());
    }

    #[test]
    fn running_and_reading_are_judged_as_build_does() {
        let a = |p: &str, args: &[&str]| json!({ "program": p, "args": args });
        assert!(matches!(judge_system("system_run", &a("ls", &["-la"])), SystemVerdict::Fine));
        assert!(matches!(judge_system("system_run", &a("curl", &["-sL", "https://api.open-meteo.com/v1/forecast?latitude=40"])), SystemVerdict::Fine));
        assert!(matches!(judge_system("system_run", &a("curl", &["-o", "/tmp/x", "https://x.org/a"])), SystemVerdict::Ask(_)));
        assert!(matches!(judge_system("system_run", &a("rm", &["-rf", "/"])), SystemVerdict::Refuse(_)));
        assert!(matches!(judge_system("system_run", &a("find", &[".", "-delete"])), SystemVerdict::Ask(_)));
        assert!(matches!(judge_system("system_read", &json!({ "path": "~/.ssh/id_ed25519" })), SystemVerdict::Ask(_)));
        assert!(matches!(judge_system("system_read", &json!({ "path": "~/.config/x.conf" })), SystemVerdict::Fine));
    }

    #[test]
    fn the_addresses_it_names_are_said() {
        let d = domains(&["run(\"curl\", {\"-s\", \"https://api.open-meteo.com/v1/forecast?x=1\"}) -- http://example.org.".into()]);
        assert_eq!(d, ["api.open-meteo.com", "example.org"]);
    }

    #[test]
    fn only_its_own_file_names_are_files() {
        for ok in ["scene.plm", "scene.luau", "icon.svg", "widget.json", "a-b_1.plm"] {
            assert!(good_file(ok), "{ok}");
        }
        for bad in ["../x.plm", "Scene.plm", "x.sh", "x", ".plm", "a/b.plm"] {
            assert!(!good_file(bad), "{bad}");
        }
    }

    /// The rules here are Build's own: each program and service its scene logic
    /// names is named here too, so the two cannot drift apart unseen.
    #[test]
    fn the_rules_are_builds() {
        let luau = include_str!("../build/logic/creations.luau");
        let table = |name: &str| -> Vec<String> {
            let start = luau.find(&format!("local {name} = {{")).unwrap_or_else(|| panic!("{name}"));
            let body = &luau[start..];
            let body = &body[..body.find("\n    }").unwrap()];
            let mut keys = Vec::new();
            // `name = …` and `["a.b"] = …`, past the table's own `local NAME = {`.
            for part in body[body.find('{').unwrap() + 1..].split([',', '\n']) {
                let Some((k, _)) = part.split_once('=') else { continue };
                let k = k.trim();
                let key = if let Some(r) = k.strip_prefix("[\"") { r.strip_suffix("\"]") } else { Some(k).filter(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) };
                if let Some(k) = key.filter(|k| !k.is_empty()) {
                    keys.push(k.to_owned());
                }
            }
            keys
        };
        let mut lua: Vec<String> = table("SAFE_RUN");
        lua.sort();
        let mut rs: Vec<String> = SAFE_RUN.iter().map(|s| s.to_string()).collect();
        rs.sort();
        assert_eq!(lua, rs, "SAFE_RUN");
        let mut lua = table("DANGER_RUN");
        lua.sort();
        let mut rs: Vec<String> = DANGER_RUN.iter().map(|s| s.0.to_string()).collect();
        rs.sort();
        assert_eq!(lua, rs, "DANGER_RUN");
        let mut lua = table("SAFE_SERVICES");
        lua.sort();
        let mut rs: Vec<String> = SAFE_SERVICES.iter().map(|s| s.to_string()).collect();
        rs.sort();
        assert_eq!(lua, rs, "SAFE_SERVICES");
    }
}
