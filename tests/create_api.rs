//! `pleamar-wm create …` (docs/create-api.md) against a fake engine
//! (`PLEAMAR_BUILD_ENGINE`): no AI, no network. A host's kind, a draft begun
//! from its template, a conversation that writes the scene (its `run: "curl"`
//! asked and allowed), asks to run a program (said no), names it and writes it
//! again; the check, undo, keep, editing it again and going back, the list,
//! and an engine that cannot build said as such. It never starts what it makes.

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Build's engine from canned lines: on the prompt it writes the scene, asks to
/// run curl to a file, names it, writes it again and is done; each step waits
/// for the result of the one before, as the real one does.
const FAKE: &str = r#"#!/bin/sh
step=0
scene='// test widget\nlanguage 0.1\nscene T {\n    surface { size: 120, 80; anchor: top_left; level: bottom }\n    permissions { run: \"curl\" }\n    box { at: 0, 0; size: 120, 80; color: #333333 }\n}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"start"'*) echo '{"type":"ready","usable":true,"model":"fake/one"}' ;;
    *'"type":"prompt"'*)
      step=1
      echo '{"type":"delta","text":"Making it. "}'
      printf '%s\n' "{\"type\":\"propose\",\"id\":\"p1\",\"tool\":\"creation_write\",\"args\":{\"file\":\"scene.plm\",\"why\":\"first\",\"content\":\"$scene// weather from https://api.open-meteo.com/v1/forecast\\n\"}}" ;;
    *'"type":"result"'*)
      step=$((step+1))
      case $step in
        2) echo '{"type":"propose","id":"p2","tool":"system_run","args":{"program":"curl","args":["-o","/tmp/x","https://example.org/a"],"why":"to see the data"}}' ;;
        3) echo '{"type":"propose","id":"p3","tool":"creation_name","args":{"name":"Weather"}}' ;;
        4) printf '%s\n' "{\"type\":\"propose\",\"id\":\"p4\",\"tool\":\"creation_write\",\"args\":{\"file\":\"scene.plm\",\"why\":\"darker\",\"content\":\"$scene\"}}" ;;
        5) echo '{"type":"delta","text":"It is ready: the weather, small."}'
           echo '{"type":"done"}' ;;
      esac ;;
    *'"type":"cancel"'*) echo '{"type":"cancelled"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
  esac
done
"#;

/// One that only looks at what is there, and is done.
const LOOKER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"start"'*) echo '{"type":"ready","usable":true,"model":"fake/one"}' ;;
    *'"type":"prompt"'*) echo '{"type":"propose","id":"l1","tool":"creation_look","args":{}}' ;;
    *'"type":"result"'*) echo '{"type":"done"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
  esac
done
"#;

/// One that cannot build: nobody is signed in.
const SIGNED_OUT: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"start"'*) echo '{"type":"ready","usable":false,"reason":"signed_out"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
  esac
done
"#;

const KIND: &str = r#"{
  "name": "Test widget",
  "description": "a small card on the desktop",
  "main": "scene.plm",
  "first_line": "// test widget",
  "template": "test-widget.plm",
  "sizes": { "small": [120, 80], "wide": [240, 80] },
  "default_size": "small",
  "requires": [],
  "copy": { "lib": "test-lib" },
  "ignore": ["shadow_cut"]
}"#;

struct Case {
    dir: PathBuf,
}

impl Case {
    fn new(name: &str) -> Case {
        let dir = std::env::temp_dir().join(format!("pleamar-wm-create-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config/create-kinds")).unwrap();
        std::fs::write(dir.join("config/create-kinds/test-widget.json"), KIND).unwrap();
        std::fs::write(dir.join("config/create-kinds/test-widget.plm"), "// test widget\nlanguage 0.1\nscene T {\n    surface { size: {{W}}, {{H}}; anchor: top_left; level: bottom }\n}\n").unwrap();
        std::fs::create_dir_all(dir.join("config/create-kinds/test-lib")).unwrap();
        std::fs::write(dir.join("config/create-kinds/test-lib/base.plm"), "// a host's library\n").unwrap();
        std::fs::write(dir.join("engine.sh"), FAKE).unwrap();
        std::fs::write(dir.join("signed-out.sh"), SIGNED_OUT).unwrap();
        std::fs::write(dir.join("looker.sh"), LOOKER).unwrap();
        Case { dir }
    }
    fn cmd(&self, engine: &str, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_pleamar-wm"));
        c.arg("create")
            .args(args)
            .env("PLEAMAR_CONFIG", self.dir.join("config"))
            .env("PLEAMAR_BUILD_ENGINE", format!("sh {}", self.dir.join(engine).display()))
            .env("HOME", &self.dir)
            .env("XDG_DATA_HOME", self.dir.join("data"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env_remove("PLEAMAR_SOCKETS");
        c
    }
    fn run(&self, args: &[&str]) -> (Vec<Value>, i32) {
        let out = self.cmd("engine.sh", args).output().expect("pleamar-wm runs");
        (lines(&String::from_utf8_lossy(&out.stdout)), out.status.code().unwrap_or(-1))
    }
    fn one(&self, args: &[&str]) -> Value {
        let (l, code) = self.run(args);
        assert_eq!(code, 0, "{args:?}: {l:?}");
        assert_eq!(l.len(), 1, "{args:?}: {l:?}");
        l.into_iter().next().unwrap()
    }
}

fn lines(text: &str) -> Vec<Value> {
    text.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not a JSON line: {l:?} ({e})"))).collect()
}

#[test]
fn a_creation_from_a_kind_to_kept_and_back() {
    let c = Case::new("flow");
    let kinds = c.one(&["kinds"]);
    assert_eq!(kinds["kinds"][0]["id"], "test-widget");
    assert_eq!(kinds["kinds"][0]["sizes"]["wide"], serde_json::json!([240, 80]));

    let begun = c.one(&["begin", "test-widget", "--size", "wide"]);
    assert_eq!(begun["type"], "begun");
    let id = begun["id"].as_str().unwrap().to_owned();
    assert_eq!(begun["size"], serde_json::json!([240, 80]));
    assert_eq!(begun["rev"], 1, "the template is its first revision");
    let main = PathBuf::from(begun["dir"].as_str().unwrap()).join("scene.plm");
    assert!(std::fs::read_to_string(&main).unwrap().contains("size: 240, 80"), "the template with its size");
    assert!(main.parent().unwrap().join("lib/base.plm").is_file(), "the host's library copied in beside it");

    // The conversation: its questions answered by another process, as a host does.
    let mut child = c.cmd("engine.sh", &["prompt", &id, "the weather, small"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let mut events = Vec::new();
    for line in out.lines().map_while(Result::ok) {
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {line:?} ({e})"));
        if v["type"] == "permission" {
            let req = v["req"].as_str().unwrap();
            let decision = if v["what"] == "scene" { "once" } else { "deny" };
            let answer = c.one(&["permit", &id, req, decision]);
            assert_eq!(answer["type"], "permitted", "{answer}");
        }
        events.push(v);
    }
    assert!(child.wait().unwrap().success());
    let of = |t: &str| events.iter().filter(|e| e["type"] == t).cloned().collect::<Vec<_>>();

    let asked = of("permission");
    assert_eq!(asked.len(), 2, "{events:#?}");
    assert_eq!(asked[0]["what"], "scene");
    assert_eq!(asked[0]["asks"][0]["value"], "curl");
    assert_eq!(asked[0]["domains"][0], "api.open-meteo.com", "what it is for, said plainly");
    assert_eq!(asked[1]["what"], "run");
    assert!(asked[1]["command"].as_str().unwrap().contains("curl -o /tmp/x"));

    let revs = of("revision");
    assert_eq!(revs.iter().map(|r| r["rev"].as_u64().unwrap()).collect::<Vec<_>>(), [2, 3]);
    assert_eq!(revs[1]["why"], "darker");
    assert!(of("status").iter().any(|s| s["code"] == "writing"), "a line of what it does");
    assert_eq!(of("named")[0]["name"], "Weather");
    let checks = of("check");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0]["ok"], true, "{:?}", checks[0]);
    let done = of("done");
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["summary"], "It is ready: the weather, small.");
    assert_eq!(events.last().unwrap()["type"], "done");

    let got = c.one(&["get", &id]);
    assert_eq!(got["name"], "Weather");
    assert_eq!(got["approved"]["run"], serde_json::json!(["curl"]));
    assert_eq!(got["busy"], false);
    let roles: Vec<&str> = got["history"].as_array().unwrap().iter().map(|h| h["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "assistant"]);

    // Undo: back one revision.
    let undone = c.one(&["undo", &id]);
    assert_eq!(undone["rev"], 2);
    assert!(std::fs::read_to_string(&main).unwrap().contains("open-meteo"));

    // Keep it: its permissions said, nothing started.
    let kept = c.one(&["keep", &id]);
    assert_eq!(kept["type"], "kept");
    assert_eq!(kept["permissions"]["run"], serde_json::json!(["curl"]));
    assert_eq!(kept["kept_rev"], 2);

    // Again with it, a change and then back to what was kept.
    let again = c.one(&["begin", "--from", &id]);
    assert_eq!(again["state"], "editing");
    assert_eq!(again["history"].as_array().unwrap().len(), 3);
    std::fs::write(&main, "changed").unwrap();
    let back = c.one(&["discard", &id]);
    assert_eq!(back["back_to"], 2);
    assert!(std::fs::read_to_string(&main).unwrap().contains("open-meteo"), "what was kept is there again");
    assert_eq!(c.one(&["get", &id])["state"], "kept");

    let list = c.one(&["list"]);
    assert_eq!(list["creations"].as_array().unwrap().len(), 1);
    assert_eq!(list["creations"][0]["name"], "Weather");
    assert_eq!(list["creations"][0]["made_by"], "ai");
    assert!(main.parent().unwrap().join("lib/base.plm").is_file(), "undo and discard leave the library");
    assert_eq!(c.one(&["remove", &id])["type"], "removed");
    assert_eq!(c.one(&["list"])["creations"].as_array().unwrap().len(), 0);
}

#[test]
fn what_cannot_be_done_is_said() {
    let c = Case::new("errors");
    let (l, code) = c.run(&["begin", "no-such-kind"]);
    assert_eq!((code, l[0]["error"].as_str()), (1, Some("no_kind")));
    let (l, code) = c.run(&["frobnicate"]);
    assert_eq!((code, l[0]["error"].as_str()), (2, Some("usage")));
    let id = c.one(&["begin", "test-widget"])["id"].as_str().unwrap().to_owned();
    let (l, code) = c.run(&["permit", &id, "r1", "once"]);
    assert_eq!((code, l[0]["error"].as_str()), (1, Some("not_running")));
    let (l, code) = c.run(&["undo", &id]);
    assert_eq!((code, l[0]["error"].as_str()), (1, Some("nothing_to_undo")));
    // A draft whose first line is not the kind's is not kept.
    std::fs::write(PathBuf::from(c.one(&["get", &id])["dir"].as_str().unwrap()).join("scene.plm"), "language 0.1\nscene T { }\n").unwrap();
    let (l, code) = c.run(&["keep", &id]);
    assert_eq!((code, l[0]["error"].as_str()), (1, Some("not_ready")));
    assert!(l[0]["problems"].as_array().unwrap().iter().any(|p| p["code"] == "first_line"));
    // An engine nobody signed in to: the draft is left as it was.
    let out = c.cmd("signed-out.sh", &["prompt", &id, "anything"]).output().unwrap();
    let l = lines(&String::from_utf8_lossy(&out.stdout));
    assert!(l.iter().any(|e| e["type"] == "error" && e["error"] == "auth_required"), "{l:?}");
    assert_eq!(c.one(&["get", &id])["rev"], 1);
    assert_eq!(c.one(&["discard", &id])["type"], "discarded");
}

/// A draft that would run a program —declared, and run by its logic at once and
/// every tenth of a second— is checked and looked at without it ever running:
/// `--layout` and the picture paint it with no permissions.
#[test]
fn a_draft_is_looked_at_never_run() {
    let c = Case::new("never-run");
    let id = c.one(&["begin", "test-widget"])["id"].as_str().unwrap().to_owned();
    let dir = PathBuf::from(c.one(&["get", &id])["dir"].as_str().unwrap());
    let mark = c.dir.join("PWNED");
    std::fs::write(dir.join("scene.plm"), "// test widget\nlanguage 0.1\nscene T {\n    surface { size: 120, 80; anchor: top_left; level: bottom }\n    permissions { run: \"touch\"; services: \"files\", \"files.write\" }\n    box { from: 0, 0; size: 120, 80; color: #ffffff }\n}\n").unwrap();
    std::fs::write(dir.join("scene.luau"), format!("run(\"touch\", {{ \"{m}-start\" }}, function() end)\nevery(0.1, function() run(\"touch\", {{ \"{m}-every\" }}, function() end) end)\n", m = mark.display())).unwrap();
    let checked = c.one(&["check", &id]);
    assert_eq!(checked["type"], "check", "{checked}");
    let out = c.cmd("looker.sh", &["prompt", &id, "look at it"]).output().unwrap();
    let events = lines(&String::from_utf8_lossy(&out.stdout));
    assert!(events.iter().any(|e| e["type"] == "done"), "{events:?}");
    for end in ["start", "every"] {
        assert!(!c.dir.join(format!("PWNED-{end}")).exists(), "the draft's logic ran a program ({end})");
    }
    // The same scene run as itself (what the host does once it is approved) does run it:
    // the test would see it if the logic never ran at all.
    let _ = Command::new(env!("CARGO_BIN_EXE_pleamar-wm")).arg("--scene").arg(dir.join("scene.plm")).args(["--no-hud", "--seconds", "1", "--fps", "4", "--film"]).arg(c.dir.join("film")).env("HOME", &c.dir).env("PLEAMAR_NO_SKILL_REFRESH", "1").env_remove("WAYLAND_DISPLAY").output();
    assert!(c.dir.join("PWNED-start").exists(), "with its permissions, its logic runs");
}
