//! `pleamar-wm build status|models|model|login|logout` (docs/build-access.md)
//! against a fake engine (`PLEAMAR_BUILD_ENGINE`): no AI, no network. What
//! each prints —JSON, a line each—, a session that has to be renewed told
//! apart from no session, build.conf's other lines kept, and an engine that
//! cannot start said as such.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

/// Answers as Build's engine does, from canned lines: ChatGPT signed in but
/// its session refused, Claude fine.
const FAKE: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"status"'*)
      echo '{"type":"status","accounts":[{"provider":"openai-codex","type":"oauth","state":"expired","reason":"OAuth refresh failed for openai-codex: OpenAI Codex token refresh failed (401)"},{"provider":"anthropic","type":"oauth","state":"ok","reason":null}],"models":["openai-codex/gpt-6-sol","anthropic/claude-x"]}' ;;
    *'"type":"models"'*)
      echo '{"type":"models","signed":["openai-codex","anthropic"],"models":["openai-codex/gpt-6-sol","anthropic/claude-x"]}' ;;
    *'"type":"login"'*)
      echo 'not json: ignored'
      echo '{"type":"login_url","url":"https://example.invalid/device","code":"ABCD-1234"}'
      echo '{"type":"login_done","ok":true}' ;;
    *'"type":"logout"'*)
      echo '{"type":"models","signed":["anthropic"],"models":["anthropic/claude-x"]}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
  esac
done
"#;

struct Case {
    dir: PathBuf,
}

impl Case {
    fn new(name: &str) -> Case {
        let dir = std::env::temp_dir().join(format!("pleamar-wm-build-access-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(dir.join("engine.sh"), FAKE).unwrap();
        Case { dir }
    }
    fn conf(&self) -> PathBuf {
        self.dir.join("config/build.conf")
    }
    /// The lines it printed, each parsed, and how it ended.
    fn run(&self, engine: &str, args: &[&str]) -> (Vec<Value>, i32) {
        let out = Command::new(env!("CARGO_BIN_EXE_pleamar-wm"))
            .arg("build")
            .args(args)
            .env("PLEAMAR_CONFIG", self.dir.join("config"))
            .env("PLEAMAR_BUILD_ENGINE", engine)
            .env("HOME", &self.dir)
            .env("XDG_DATA_HOME", self.dir.join("data"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .output()
            .expect("pleamar-wm runs");
        let text = String::from_utf8_lossy(&out.stdout);
        let lines = text.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not a JSON line: {l:?} ({e})"))).collect();
        (lines, out.status.code().unwrap_or(-1))
    }
    fn fake(&self) -> String {
        format!("sh {}", self.dir.join("engine.sh").display())
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

#[test]
fn a_session_to_renew_is_not_no_session() {
    let c = Case::new("status");
    std::fs::write(c.conf(), "model openai-codex/gpt-6-sol\nstart = panel\n").unwrap();
    let (lines, code) = c.run(&c.fake(), &["status"]);
    assert_eq!(code, 0, "{lines:?}");
    let st = &lines[0];
    assert_eq!(s(st, "type"), "status");
    assert_eq!(s(st, "model"), "openai-codex/gpt-6-sol");
    assert_eq!(st["usable"], false);
    assert_eq!(s(st, "reason"), "expired");
    assert_eq!(st["accounts"].as_array().unwrap().len(), 2);

    // Its model on the account that works: usable.
    std::fs::write(c.conf(), "model anthropic/claude-x\n").unwrap();
    let (lines, _) = c.run(&c.fake(), &["status"]);
    assert_eq!(lines[0]["usable"], true);
    assert!(lines[0]["reason"].is_null());

    // On an account with no sign-in at all: signed out, not expired.
    std::fs::write(c.conf(), "model github-copilot/x\n").unwrap();
    let (lines, _) = c.run(&c.fake(), &["status"]);
    assert_eq!(s(&lines[0], "reason"), "signed_out");
}

#[test]
fn models_and_choosing_one_keeps_the_rest_of_build_conf() {
    let c = Case::new("models");
    std::fs::write(c.conf(), "# mine\nmodel openai-codex/gpt-6-sol\nstart = panel\n").unwrap();
    let (lines, code) = c.run(&c.fake(), &["models"]);
    assert_eq!(code, 0);
    let list = lines[0]["models"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["chosen"], true);
    assert_eq!(s(&list[1], "provider"), "anthropic");

    let (lines, code) = c.run(&c.fake(), &["model", "anthropic/claude-x"]);
    assert_eq!(code, 0);
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(std::fs::read_to_string(c.conf()).unwrap(), "# mine\nmodel anthropic/claude-x\nstart = panel\n");

    let (lines, code) = c.run(&c.fake(), &["model", "no-slash"]);
    assert_eq!(code, 2);
    assert_eq!(s(&lines[0], "error"), "usage");
}

#[test]
fn signing_in_and_out() {
    let c = Case::new("login");
    let (lines, code) = c.run(&c.fake(), &["login", "anthropic"]);
    assert_eq!(code, 0, "{lines:?}");
    assert_eq!(lines.len(), 2, "the URL, then the end: {lines:?}");
    assert_eq!(s(&lines[0], "type"), "login_url");
    assert_eq!(s(&lines[0], "code"), "ABCD-1234");
    assert_eq!(s(&lines[1], "type"), "login_done");
    assert_eq!(lines[1]["ok"], true);

    let (lines, code) = c.run(&c.fake(), &["logout", "openai-codex"]);
    assert_eq!(code, 0);
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[0]["signed"], serde_json::json!(["anthropic"]));
}

#[test]
fn an_engine_that_cannot_start_says_why() {
    let c = Case::new("dead");
    let (lines, code) = c.run("echo 'build: node is missing' >&2; exit 3", &["status"]);
    assert_eq!(code, 1);
    assert_eq!(s(&lines[0], "error"), "engine_failed");
    assert_eq!(s(&lines[0], "message"), "node is missing");
    let (lines, code) = c.run(&c.fake(), &["login"]);
    assert_eq!(code, 2);
    assert_eq!(s(&lines[0], "error"), "usage");
}
