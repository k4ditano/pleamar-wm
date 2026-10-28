//! What a free monitor's canvas keeps from one session to the next: its
//! terminals —their program, their folder, the agent in each, their place on
//! the plane—, the wires between them with their names, and each canvas's
//! camera. Saved while the session runs (never an empty canvas over a kept
//! one); offered when the next one starts («Bring back your canvas»), and
//! brought back with `pleamar-wm link restore`: each terminal opened again in
//! its folder and place, its agent started again on its last conversation,
//! and the wires again.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Where it is kept: `$XDG_STATE_HOME/pleamar-wm`, or beside the configuration
/// when that is somewhere of its own (a test session's `PLEAMAR_CONFIG`).
fn dir() -> String {
    if let Some(d) = std::env::var("PLEAMAR_CONFIG").ok().filter(|v| !v.is_empty()) {
        return format!("{d}/state");
    }
    let state = std::env::var("XDG_STATE_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{}/.local/state", std::env::var("HOME").unwrap_or_default()));
    format!("{state}/pleamar-wm")
}

fn file() -> String {
    format!("{}/canvas.json", dir())
}

/// Offered and not yet answered: nothing is saved over it meanwhile.
fn pending() -> String {
    format!("{}/canvas.pending", dir())
}

/// The scene's values, by name, in order.
fn get(names: &[String]) -> Vec<String> {
    let me = crate::link::own_exe();
    let mut c = std::process::Command::new(me);
    // From inside the session's own process, where its programs find `wm`.
    if let Some(own) = windows_file().and_then(|w| std::path::Path::new(&w).parent().map(|p| p.display().to_string())) {
        c.env("PLEAMAR_SOCKETS", own);
    }
    c.arg("--say").arg("wm").args(names.iter().map(|n| format!("get {n}"))).output().map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_owned()).collect()).unwrap_or_default()
}

fn num(v: Option<&String>) -> f64 {
    v.and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// How to start an agent again on its last conversation.
fn again(agent: &str) -> &'static str {
    match agent {
        "claude" => "claude --continue",
        "codex" => "codex resume --last",
        "opencode" => "opencode --continue",
        "gemini" => "gemini",
        "qwen" => "qwen",
        "copilot" => "copilot",
        _ => "",
    }
}

/// The canvas as it is now: its terminals on free monitors (and any wired
/// ones), their wires, the cameras. `None` with no terminal there.
fn now(windows: &[(usize, u32)]) -> Option<Value> {
    let layers = crate::term::layers_in(&format!("{}/term", std::path::Path::new(&windows_file()?).parent()?.display()));
    // Each window that is a terminal through a layer: its program and what runs in it.
    let terms: Vec<(usize, u32, std::collections::BTreeMap<String, String>)> = windows
        .iter()
        .filter_map(|(slot, pid)| layers.iter().find(|(_, i)| i.get("terminal").and_then(|t| t.parse::<u32>().ok()) == Some(*pid)).map(|(_, i)| (*slot, *pid, i.clone())))
        .collect();
    if terms.is_empty() {
        return None;
    }
    let mut names: Vec<String> = (0..4).flat_map(|k| [format!("free.{k}"), format!("cvx.{k}"), format!("cvy.{k}"), format!("cvz.{k}")]).collect();
    for (slot, ..) in &terms {
        names.extend(["fx", "fy", "fw", "fh"].iter().map(|f| format!("{f}.{slot}")));
        names.push(format!("win.{slot}.screen"));
    }
    names.extend((0..8).flat_map(|k| [format!("wire.{k}.a"), format!("wire.{k}.b"), format!("wire.{k}.label")]));
    let got = get(&names);
    // All of it, or nothing: a scene going away (the session ending) answers
    // «?» for everything, and that was kept as a canvas of nothing at 0, 0.
    if got.len() != names.len() || got.iter().any(|v| v.starts_with('?')) {
        return None;
    }
    let at = |n: &str| names.iter().position(|x| x == n).map(|k| &got[k]);
    let monitors: Vec<Value> = (0..4).map(|k| json!({ "free": num(at(&format!("free.{k}"))) > 0.5, "x": num(at(&format!("cvx.{k}"))), "y": num(at(&format!("cvy.{k}"))), "z": num(at(&format!("cvz.{k}"))) })).collect();
    let free = |m: usize| monitors.get(m).and_then(|v| v["free"].as_bool()).unwrap_or(false);
    let wires: Vec<(i64, i64, String)> = (0..8)
        .map(|k| (num(at(&format!("wire.{k}.a"))) as i64, num(at(&format!("wire.{k}.b"))) as i64, at(&format!("wire.{k}.label")).cloned().unwrap_or_default()))
        .filter(|(a, b, _)| *a >= 0 && *b >= 0 && a != b)
        .collect();
    let wired = |s: usize| wires.iter().any(|(a, b, _)| *a as usize == s || *b as usize == s);
    let kept: Vec<Value> = terms
        .iter()
        .filter(|(slot, ..)| free(num(at(&format!("win.{slot}.screen"))) as usize) || wired(*slot))
        // (A place it never had is no place to bring it back to.)
        .filter(|(slot, ..)| num(at(&format!("fw.{slot}"))) > 1.0)
        // (Nor one with a name: whoever named it brings it back, as it was —an
        // agent started with its role and model—, and here it would come back twice.)
        .filter(|(_, _, i)| i.get("name").is_none_or(|n| n.is_empty()))
        .filter_map(|(slot, pid, i)| {
            let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?.display().to_string();
            Some(json!({
                "slot": slot,
                "program": exe,
                "folder": i.get("cwd").cloned().unwrap_or_default(),
                "agent": i.get("agent").cloned().unwrap_or_default(),
                "monitor": num(at(&format!("win.{slot}.screen"))) as i64,
                "place": [num(at(&format!("fx.{slot}"))), num(at(&format!("fy.{slot}"))), num(at(&format!("fw.{slot}"))), num(at(&format!("fh.{slot}")))],
            }))
        })
        .collect();
    if kept.is_empty() {
        return None;
    }
    let slots: Vec<i64> = kept.iter().filter_map(|w| w["slot"].as_i64()).collect();
    let wires: Vec<Value> = wires.into_iter().filter(|(a, b, _)| slots.contains(a) && slots.contains(b)).map(|(a, b, l)| json!({ "a": a, "b": b, "label": l })).collect();
    Some(json!({ "monitors": monitors, "windows": kept, "wires": wires }))
}

static WINDOWS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn windows_file() -> Option<String> {
    WINDOWS.get().cloned()
}

/// For the session, once its windows file is known: offers what was kept
/// (the scene's `kept.n`, `kept.w`) and, every few seconds, keeps
/// the canvas as it is.
pub fn start(windows: String, to_render: std::sync::mpsc::Sender<pleamar::scene::ToRender>) {
    let _ = WINDOWS.set(windows.clone());
    std::thread::spawn(move || {
        use pleamar::scene::{intern, ToRender};
        // A moment for the scene to be there.
        std::thread::sleep(Duration::from_secs(3));
        let _ = std::fs::create_dir_all(dir());
        if let Some(kept) = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            let n = kept["windows"].as_array().map_or(0, Vec::len);
            if n > 0 {
                let _ = std::fs::write(pending(), "");
                let _ = to_render.send(ToRender::ExternalFact(intern("kept.w"), kept["wires"].as_array().map_or(0, Vec::len).to_string()));
                let _ = to_render.send(ToRender::ExternalFact(intern("kept.n"), n.to_string()));
            }
        }
        let mut last = String::new();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            if std::path::Path::new(&pending()).exists() {
                continue;
            }
            let text = std::fs::read_to_string(&windows).unwrap_or_default();
            let slots: Vec<(usize, u32)> = text
                .lines()
                .filter_map(|l| {
                    let mut w = l.split('\t').next()?.split_whitespace();
                    Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
                })
                .collect();
            let Some(state) = now(&slots) else { continue };
            let out = serde_json::to_string_pretty(&state).unwrap_or_default();
            if out != last {
                let tmp = format!("{}.new", file());
                if std::fs::write(&tmp, &out).and_then(|_| std::fs::rename(&tmp, file())).is_ok() {
                    last = out;
                }
            }
        }
    });
}

/// `link restore [--dismiss]`: the kept canvas brought back, or let go.
pub fn restore(dismiss: bool) -> Result<String, String> {
    let _ = std::fs::remove_file(pending());
    let say = |orders: &[String]| {
        let me = crate::link::own_exe();
        let _ = std::process::Command::new(me).arg("--say").arg("wm").args(orders).output();
    };
    say(&["fact kept.n 0".to_owned()]);
    if dismiss {
        return Ok("the kept canvas stays kept; it is saved over from now on".into());
    }
    let kept: Value = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("no canvas kept")?;
    // Its monitors free again, each camera where it was.
    for (k, m) in kept["monitors"].as_array().into_iter().flatten().enumerate() {
        if m["free"].as_bool() == Some(true) {
            say(&[format!("fact free.{k} 1")]);
            std::thread::sleep(Duration::from_millis(300));
            say(&[format!("fact cvx.{k} {}", m["x"]), format!("fact cvy.{k} {}", m["y"]), format!("fact cvz.{k} {}", m["z"].as_f64().filter(|z| *z > 0.05).unwrap_or(1.0))]);
        }
    }
    let mut new_slot: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    let mut opened = 0;
    let mut agents = 0;
    for w in kept["windows"].as_array().into_iter().flatten() {
        let program = w["program"].as_str().unwrap_or("");
        let folder = w["folder"].as_str().filter(|f| std::path::Path::new(f).is_dir()).map(str::to_owned).unwrap_or_else(|| std::env::var("HOME").unwrap_or_default());
        let Ok((slot, id)) = crate::link::open_terminal(program, &folder, false, None) else { continue };
        opened += 1;
        new_slot.insert(w["slot"].as_i64().unwrap_or(-1), slot);
        // On its monitor (where the agent's hands can take it), in its place.
        let monitor = w["monitor"].as_i64().unwrap_or(0);
        if let Some(pid) = crate::link::pid_of_slot(slot) {
            let _ = std::process::Command::new(crate::link::own_exe()).args(["agent", "send", &pid.to_string(), &monitor.to_string()]).output();
        }
        std::thread::sleep(Duration::from_millis(500));
        let p: Vec<f64> = w["place"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        if let [x, y, ww, hh] = p[..] {
            say(&[format!("fact fx.{slot} {x}"), format!("fact fy.{slot} {y}"), format!("fact fw.{slot} {ww}"), format!("fact fh.{slot} {hh}"), format!("fact fsc.{slot} {monitor}")]);
        }
        // Its agent, on its last conversation.
        let command = again(w["agent"].as_str().unwrap_or(""));
        if !command.is_empty() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && crate::term::ask(&format!("{}/{id}.sock", crate::term::socket_dir()), "info").is_err() {
                std::thread::sleep(Duration::from_millis(200));
            }
            let sock = format!("{}/{id}.sock", crate::term::socket_dir());
            let _ = crate::term::ask(&sock, &format!("paste {}", command.bytes().map(|b| format!("{b:02x}")).collect::<String>()));
            std::thread::sleep(Duration::from_millis(150));
            let _ = crate::term::ask(&sock, "keys enter");
            agents += 1;
        }
    }
    // The wires between them, again, with their names, in the free places.
    let taken: Vec<f64> = get(&(0..8).map(|k| format!("wire.{k}.a")).collect::<Vec<_>>()).iter().map(|v| v.parse().unwrap_or(-1.0)).collect();
    let mut free_places = (0..8).filter(|k| taken.get(*k).is_none_or(|a| *a < 0.0));
    let mut wires = 0;
    for wire in kept["wires"].as_array().into_iter().flatten() {
        let (Some(a), Some(b)) = (new_slot.get(&wire["a"].as_i64().unwrap_or(-1)), new_slot.get(&wire["b"].as_i64().unwrap_or(-1))) else { continue };
        if a == b {
            continue;
        }
        let Some(k) = free_places.next() else { break };
        let mut orders = vec![format!("fact wire.{k}.a {a}"), format!("fact wire.{k}.b {b}")];
        if let Some(l) = wire["label"].as_str().filter(|l| !l.is_empty()) {
            orders.push(format!("text wire.{k}.label {l}"));
        }
        say(&orders);
        wires += 1;
    }
    // (Its hands moved them: the monitor's light goes out.)
    let _ = std::process::Command::new(crate::link::own_exe()).args(["agent", "done"]).output();
    Ok(format!("brought back {opened} terminals ({agents} with their agent on its last conversation) and {wires} wires"))
}
