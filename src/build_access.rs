//! `pleamar-wm build status|models|model|login|logout`: Build's sign-in to its
//! AI and the model it builds with, for any shell's settings or a terminal
//! (docs/build-access.md). Each answer is one JSON object a line on stdout;
//! a failure is a line `{"type":"error","error":CODE,"message":…}` and exit 1
//! (2 for a command written wrong).
//!
//! It speaks to Build's own engine (build/agent: the Pi SDK in its sandbox)
//! the JSON lines the control centre speaks, started for that one question
//! and stopped after it. `PLEAMAR_BUILD_ENGINE`, a command, takes the
//! engine's place (the tests' fake one, without AI or network). No
//! credential is ever printed: the engine keeps them, and what it says of a
//! failure comes already cleaned of anything that looks like a token.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Build's model when build.conf names none (logic/engine.luau's `DEFAULT_MODEL`).
const DEFAULT_MODEL: &str = "openai-codex/gpt-6-astra";

/// The subcommands this answers; anything else is build mode's own.
pub const COMMANDS: &[&str] = &["status", "models", "model", "login", "logout"];

fn say(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn fail(code: &str, message: impl Into<String>) -> ! {
    say(json!({ "type": "error", "error": code, "message": message.into() }));
    std::process::exit(if code == "usage" { 2 } else { 1 });
}

/// `${PLEAMAR_CONFIG:-${XDG_CONFIG_HOME:-~/.config}/pleamar}/build.conf`, as
/// Build and the control centre read it.
fn conf_path() -> std::path::PathBuf {
    let dir = crate::config::user_dir().unwrap_or_else(|| "/tmp/pleamar".into());
    std::path::Path::new(&dir).join("build.conf")
}

/// The model build.conf chooses, if it chooses one.
fn chosen() -> Option<String> {
    let text = std::fs::read_to_string(conf_path()).ok()?;
    text.lines().filter_map(|l| l.trim().strip_prefix("model")).filter(|r| r.starts_with(char::is_whitespace)).map(|r| r.trim().to_owned()).filter(|m| !m.is_empty()).last()
}

/// build.conf with its `model` line said anew; every other line kept.
pub fn with_model(old: &str, model: &str) -> String {
    let mut out = String::new();
    let mut put = false;
    for line in old.lines() {
        let is_model = line.trim().strip_prefix("model").is_some_and(|r| r.starts_with(char::is_whitespace));
        if is_model {
            if !put {
                out.push_str(&format!("model {model}\n"));
                put = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !put {
        if out.is_empty() {
            out.push_str("# pleamar-wm build: the model that builds\n");
        }
        out.push_str(&format!("model {model}\n"));
    }
    out
}

struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    said: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Engine {
    fn start() -> Engine {
        let mut cmd = match std::env::var("PLEAMAR_BUILD_ENGINE").ok().filter(|v| !v.trim().is_empty()) {
            Some(fake) => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(fake);
                c
            }
            None => {
                let dir = crate::build::dir();
                if let Err(e) = crate::build::ensure_files() {
                    fail("engine_missing", e);
                }
                // Its packages, fetched the first time (as the control centre does).
                let fetched = Command::new("sh")
                    .arg("-c")
                    .arg(r#"cd "$0/agent" && { [ -d node_modules/@earendil-works/pi-coding-agent ] && [ ! package-lock.json -nt node_modules/.package-lock.json ]; } || { command -v npm >/dev/null || { echo "npm is missing: install Node.js (and npm) to build with AI"; exit 3; }; npm ci --ignore-scripts --no-audit --no-fund >/dev/null 2>&1 || { echo "the engine could not be installed (npm ci)"; exit 4; }; }"#)
                    .arg(&dir)
                    .stdin(Stdio::null())
                    .output();
                match fetched {
                    Ok(o) if o.status.success() => {}
                    Ok(o) => fail("engine_missing", String::from_utf8_lossy(&o.stdout).trim().to_owned()),
                    Err(e) => fail("engine_missing", e.to_string()),
                }
                Command::new(format!("{dir}/agent/sandbox"))
            }
        };
        let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => fail("engine_missing", format!("the engine does not start: {e}")),
        };
        let (tx, rx) = channel();
        let out = child.stdout.take().expect("piped");
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        // What it says on the side: the sandbox's own reasons ("build: node is
        // missing"), the last one kept for a failure.
        let said = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let err = child.stderr.take().expect("piped");
        let keep = said.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Some(why) = line.strip_prefix("build: ") {
                    *keep.lock().unwrap() = why.to_owned();
                }
            }
        });
        let stdin = child.stdin.take();
        Engine { child, stdin, lines: rx, said }
    }

    fn send(&mut self, v: Value) {
        if let Some(w) = self.stdin.as_mut() {
            let _ = writeln!(w, "{v}");
            let _ = w.flush();
        }
    }

    /// The next message from it, by `type`, within `wait`; the others are
    /// handed to `meanwhile` (a sign-in's URL). Its end, or the time, fails.
    fn until(&mut self, kinds: &[&str], wait: Duration, mut meanwhile: impl FnMut(&Value)) -> Value {
        let end = Instant::now() + wait;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
                    if kinds.contains(&kind) {
                        return v;
                    }
                    meanwhile(&v);
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.stop();
                    fail("timeout", format!("the engine did not answer in {} s", wait.as_secs()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // Its last words on the side arrive a moment after its output closes.
                    let _ = self.child.wait();
                    std::thread::sleep(Duration::from_millis(50));
                    let why = self.said.lock().unwrap().clone();
                    fail("engine_failed", if why.is_empty() { "the engine stopped without answering".to_owned() } else { why });
                }
            }
        }
    }

    fn stop(&mut self) {
        self.send(json!({ "type": "shutdown" }));
        self.stdin = None;
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// The provider of `provider/id`.
fn provider_of(model: &str) -> &str {
    model.split_once('/').map_or(model, |(p, _)| p)
}

pub fn run(args: &[String]) {
    let what = args.first().map(String::as_str).unwrap_or("");
    let arg = args.get(1).map(String::as_str);
    match what {
        "status" => status(),
        "models" => models(),
        "model" => model(arg.unwrap_or_else(|| fail("usage", "pleamar-wm build model PROVIDER/ID"))),
        "login" => login(arg.unwrap_or_else(|| fail("usage", "pleamar-wm build login PROVIDER [--open]")), args.iter().any(|a| a == "--open")),
        "logout" => logout(arg.unwrap_or_else(|| fail("usage", "pleamar-wm build logout PROVIDER"))),
        _ => fail("usage", "pleamar-wm build status | models | model PROVIDER/ID | login PROVIDER [--open] | logout PROVIDER"),
    }
}

/// Who is signed in and whether each sign-in still works, the model chosen,
/// and whether Build can build with it now (and if not, why).
fn status() {
    let mut e = Engine::start();
    e.send(json!({ "type": "status" }));
    let m = e.until(&["status"], Duration::from_secs(90), |_| {});
    e.stop();
    if let Some(why) = m.get("reason").and_then(Value::as_str) {
        fail("engine_failed", why);
    }
    let accounts = m.get("accounts").and_then(Value::as_array).cloned().unwrap_or_default();
    let usable_models = strings(&m, "models");
    let picked = chosen();
    let model = picked.clone().unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    let mine = accounts.iter().find(|a| a.get("provider").and_then(Value::as_str) == Some(provider_of(&model)));
    let reason = match mine.and_then(|a| a.get("state")).and_then(Value::as_str) {
        None => Some("signed_out"),
        Some("ok") if usable_models.iter().any(|x| x == &model) => None,
        Some("ok") => Some("model_unavailable"),
        Some("expired") => Some("expired"),
        Some("unreachable") => Some("unreachable"),
        Some(_) => Some("error"),
    };
    say(json!({
        "type": "status",
        "model": model,
        "chosen": picked.is_some(),
        "usable": reason.is_none(),
        "reason": reason,
        "accounts": accounts,
    }));
}

/// The models the signed-in accounts can build with (those that see pictures).
fn models() {
    let mut e = Engine::start();
    e.send(json!({ "type": "models" }));
    let m = e.until(&["models"], Duration::from_secs(90), |_| {});
    e.stop();
    if let Some(why) = m.get("reason").and_then(Value::as_str) {
        fail("engine_failed", why);
    }
    let picked = chosen().unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    let list: Vec<Value> = strings(&m, "models").into_iter().map(|id| json!({ "id": id, "provider": provider_of(&id), "chosen": id == picked })).collect();
    say(json!({ "type": "models", "signed": strings(&m, "signed"), "models": list }));
}

/// The model Build builds with from its next start: build.conf's `model`
/// line, the rest of the file as it was. Not asked of the engine: a model of
/// an account whose session has to be renewed can still be chosen.
fn model(id: &str) {
    let ok = id.split_once('/').is_some_and(|(p, m)| !p.is_empty() && !m.is_empty()) && !id.chars().any(char::is_whitespace);
    if !ok {
        fail("usage", format!("'{id}': a model is PROVIDER/ID, as `pleamar-wm build models` says them"));
    }
    let path = conf_path();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, with_model(&old, id)) {
        fail("config_unwritable", format!("{}: {e}", path.display()));
    }
    say(json!({ "type": "model", "ok": true, "model": id, "path": path.display().to_string() }));
}

/// Signs in to `provider`: the page (and code) to finish it, a line each as
/// the engine says them (opened in the browser with --open), then whether it
/// did. Five minutes to finish.
fn login(provider: &str, open: bool) {
    let mut e = Engine::start();
    e.send(json!({ "type": "login", "provider": provider }));
    let m = e.until(&["login_done"], Duration::from_secs(300), |v| {
        if v.get("type").and_then(Value::as_str) == Some("login_url") {
            let url = v.get("url").and_then(Value::as_str).unwrap_or("");
            say(json!({ "type": "login_url", "provider": provider, "url": url, "code": v.get("code").cloned().unwrap_or(Value::Null) }));
            if open && !url.is_empty() {
                let _ = Command::new("xdg-open").arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
            }
        }
    });
    e.stop();
    if m.get("ok").and_then(Value::as_bool) == Some(true) {
        say(json!({ "type": "login_done", "provider": provider, "ok": true }));
        return;
    }
    let why = m.get("reason").and_then(Value::as_str).unwrap_or("").to_owned();
    let code = if why == "cancelled" {
        "cancelled"
    } else if why.contains("Unknown provider") {
        "unknown_provider"
    } else {
        "login_failed"
    };
    fail(code, why);
}

/// Signs out of `provider`; what is still signed in after it.
fn logout(provider: &str) {
    let mut e = Engine::start();
    e.send(json!({ "type": "logout", "provider": provider }));
    let m = e.until(&["models"], Duration::from_secs(90), |_| {});
    e.stop();
    let signed = strings(&m, "signed");
    say(json!({ "type": "logout", "provider": provider, "ok": !signed.iter().any(|p| p == provider), "signed": signed }));
}

#[cfg(test)]
mod tests {
    use super::with_model;

    #[test]
    fn the_model_line_is_said_anew_and_the_rest_kept() {
        assert_eq!(with_model("", "a/b"), "# pleamar-wm build: the model that builds\nmodel a/b\n");
        assert_eq!(with_model("# mine\nmodel x/y\nstart = panel\n", "a/b"), "# mine\nmodel a/b\nstart = panel\n");
        assert_eq!(with_model("start = panel\n", "a/b"), "start = panel\nmodel a/b\n");
        assert_eq!(with_model("model x/y\nmodel z/w\nmodels-note\n", "a/b"), "model a/b\nmodels-note\n");
    }
}
