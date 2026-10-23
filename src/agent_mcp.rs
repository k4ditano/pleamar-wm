//! `pleamar-wm agent mcp`: the agent's verbs as an MCP server (stdio,
//! JSON-RPC a line), beside `link mcp`. A tool's arguments are structured,
//! so nothing is quoted; each call runs `pleamar-wm agent ...` as a process,
//! its words a list, and answers what it printed and its exit code
//! (`isError` and `exit N: ` before its error when not 0). `look` answers the
//! picture itself; `do` takes its steps as a list.

use crate::mcp_serve::{Content, Reply, Tool};
use serde_json::{json, Value};

/// What a call runs: the words after `agent`, and its input.
#[derive(Debug, PartialEq)]
struct Call {
    words: Vec<String>,
    input: Option<String>,
}

type Words = fn(&Value) -> Result<Call, String>;

/// The longest `listen` a call waits for, in seconds.
const LISTEN: f64 = 60.0;
/// What a bad argument exits with, as the CLI's.
const USAGE: i32 = 64;

/// An argument as a word: a text, a number or a yes.
fn s(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn yes(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool) == Some(true)
}

/// The words of a verb on a window, and the rest pushed by `more`.
fn on(verb: &str, a: &Value, more: impl FnOnce(&mut Vec<String>)) -> Result<Call, String> {
    let mut w = vec![verb.to_owned(), s(a, "window").unwrap_or_default()];
    more(&mut w);
    Ok(Call { words: w, input: None })
}

/// `--flag` for each of these that is true.
fn flags(w: &mut Vec<String>, a: &Value, names: &[(&str, &str)]) {
    w.extend(names.iter().filter(|(k, _)| yes(a, k)).map(|(_, f)| f.to_string()));
}

/// `OPTION VALUE` for each of these that is said.
fn values(w: &mut Vec<String>, a: &Value, names: &[(&str, &str)]) {
    for (k, o) in names {
        if let Some(v) = s(a, k) {
            w.extend([o.to_string(), v]);
        }
    }
}

/// An element or a text to find it by.
fn which(w: &mut Vec<String>, a: &Value) {
    if let Some(e) = s(a, "element") {
        w.push(if e.starts_with('@') { e } else { format!("@{e}") });
    }
    values(w, a, &[("text", "--text")]);
    flags(w, a, &[("first", "--first")]);
}

fn words(w: &mut Vec<String>, a: &Value, names: &[&str]) {
    w.extend(names.iter().filter_map(|k| s(a, k)));
}

/// A duration: seconds when a number.
fn duration(a: &Value, k: &str) -> Option<String> {
    match a.get(k) {
        Some(Value::Number(n)) => Some(format!("{n}s")),
        _ => s(a, k),
    }
}

/// `wait`'s condition, from its parts.
fn condition(a: &Value) -> Result<Vec<String>, String> {
    let until = s(a, "until").unwrap_or_default();
    let text = s(a, "text");
    let element = s(a, "element").map(|e| if e.starts_with('@') { e } else { format!("@{e}") });
    let need = |v: Option<String>, what: &str| v.ok_or_else(|| format!("until={until} needs {what}"));
    Ok(match until.as_str() {
        "has" => vec!["has".into(), "--text".into(), need(text, "text")?],
        "gone" => match (text, element) {
            (Some(t), _) => vec!["gone".into(), "--text".into(), t],
            (None, Some(e)) => vec![e, "gone".into()],
            _ => return Err("until=gone needs text or element".into()),
        },
        "state" | "not" => {
            let mut w = vec![need(element, "element")?];
            if until == "not" {
                w.push("not".into());
            }
            w.push(need(s(a, "state"), "state")?);
            w
        }
        "settled" => vec!["settled".into(), duration(a, "quiet").unwrap_or_else(|| "500ms".into())],
        "changed" | "prompt" => vec![until.clone()],
        "expr" => vec!["expr".into(), need(s(a, "expr"), "expr")?],
        _ => return Err(format!("until is has, gone, state, not, changed, settled, prompt or expr, not {until:?}")),
    })
}

/// A `do` step said as a line: as it is, or a tool's arguments.
fn step(window: &str, v: &Value) -> Result<String, String> {
    if let Some(line) = v.as_str() {
        return Ok(line.to_owned());
    }
    let name = s(v, "tool").ok_or("a step is a line, or {\"tool\": \"press\", ...its arguments but the window}")?;
    let all = tools();
    let t = all.iter().find(|t| t.name == name && t.args.get("window").is_some() && name != "do").ok_or_else(|| format!("{name} is not a step: a step is a tool on the window"))?;
    let mut a = v.clone();
    a["window"] = json!(window);
    let mut w = (t.run)(&a)?.words;
    w.remove(1);
    Ok(w.iter().map(|w| crate::agent_cli::quote(w)).collect::<Vec<_>>().join(" "))
}

fn tools() -> Vec<Tool<Words>> {
    let window = json!({ "type": "string", "description": "The window: PID or PID.N as `windows` names it; a terminal's shell PID:SHELL." });
    let element = json!({ "type": "string", "description": "A scene's item: @save (@=3 a name of digits)." });
    let text = json!({ "type": "string", "description": "Its whole name (case folded); several match: they are listed (exit 6), or first=true." });
    let first = json!({ "type": "boolean", "description": "Take the first of several that match." });
    let num = |d: &str| json!({ "type": "number", "description": d });
    let dur = |d: &str| json!({ "type": ["number", "string"], "description": d });
    let flag = |d: &str| json!({ "type": "boolean", "description": d });
    let st = |d: &str| json!({ "type": "string", "description": d });
    vec![
        Tool {
            name: "windows",
            about: "Every window: PID (PID.N for one of a program's several), program, title, box, monitor, and what it does: scene (a pleamar scene: read and act by item), text (a terminal: read as text, run commands), look (pictures, the pointer), nothing (private: ask the user). A terminal with several shells names them.",
            args: json!({}),
            needed: &[],
            eager: true,
            run: |_| Ok(Call { words: vec!["windows".into()], input: None }),
        },
        Tool {
            name: "can",
            about: "What one window does and its verbs; a terminal's shells, what runs in each, where, its size and how long it has been quiet.",
            args: json!({ "window": window }),
            needed: &["window"],
            eager: false,
            run: |a| on("can", a, |_| {}),
        },
        Tool {
            name: "open",
            about: "Start a program for the agent on the monitor it works on, without the user's keyboard. Never start programs from your own shell.",
            args: json!({ "command": st("The command line: firefox, gtk3-widget-factory."), "monitor": num("A monitor's number (the agent's own if not said).") }),
            needed: &["command"],
            eager: false,
            run: |a| {
                let mut w = vec!["open".into()];
                values(&mut w, a, &[("monitor", "--monitor")]);
                w.push("--".into());
                words(&mut w, a, &["command"]);
                Ok(Call { words: w, input: None })
            },
        },
        Tool {
            name: "tree",
            about: "Read a window: a scene's items as `@name role \"name\" X,Y WxH states`, indented under what they are in, and its version; a terminal: `mark M` and its lines. Read before you look.",
            args: json!({
                "window": window,
                "since": { "type": ["boolean", "number"], "description": "What changed since your last read of it (true), or since that version or mark." },
                "within": element,
                "role": st("Only elements of that role, under what they are in."),
                "max": num("At most this many lines, the rest counted."),
                "hidden": flag("What is not shown too (scrolled away, collapsed)."),
                "history": flag("A terminal: all it keeps."),
                "long": flag("Names cut at 400, not 120."),
                "json": flag("As JSON."),
            }),
            needed: &["window"],
            eager: true,
            run: |a| {
                on("tree", a, |w| {
                    match a.get("since") {
                        Some(Value::Bool(true)) => w.push("--since".into()),
                        Some(Value::Number(n)) => w.extend(["--since".into(), n.to_string()]),
                        _ => {}
                    }
                    values(w, a, &[("within", "--within"), ("role", "--role"), ("max", "--max")]);
                    flags(w, a, &[("hidden", "--hidden"), ("history", "--history"), ("long", "--long"), ("json", "--json")]);
                })
            },
        },
        Tool {
            name: "find",
            about: "What says a text in its name or text, under what it is in; a terminal's lines that say it, by number.",
            args: json!({ "window": window, "text": st("What to look for (case folded)."), "long": flag("Names cut at 400."), "json": flag("As JSON.") }),
            needed: &["window", "text"],
            eager: false,
            run: |a| {
                on("find", a, |w| {
                    values(w, a, &[("text", "--text")]);
                    flags(w, a, &[("long", "--long"), ("json", "--json")]);
                })
            },
        },
        Tool {
            name: "at",
            about: "A scene's item at a point of the window's picture, under what it is in.",
            args: json!({ "window": window, "x": num("In the picture's coordinates."), "y": num("In the picture's coordinates."), "json": flag("As JSON.") }),
            needed: &["window", "x", "y"],
            eager: false,
            run: |a| {
                on("at", a, |w| {
                    words(w, a, &["x", "y"]);
                    flags(w, a, &[("json", "--json")]);
                })
            },
        },
        Tool {
            name: "press",
            about: "Press a scene's item: the agent's cursor goes to it and presses it. Say it by element, or by text.",
            args: json!({
                "window": window,
                "element": element,
                "text": text,
                "first": first,
                "button": st("left (the default), right or middle."),
                "count": num("1 to 3 presses."),
            }),
            needed: &["window"],
            eager: true,
            run: |a| {
                on("press", a, |w| {
                    which(w, a);
                    words(w, a, &["button", "count"]);
                })
            },
        },
        Tool {
            name: "focus",
            about: "The focus in its window to a scene's item (a field, to type in). Without an element: the user's keyboard to the window, and its workspace shown.",
            args: json!({ "window": window, "element": element, "text": text, "first": first }),
            needed: &["window"],
            eager: false,
            run: |a| on("focus", a, |w| which(w, a)),
        },
        Tool {
            name: "type",
            about: "Type text into a scene's item or what has the keyboard; a terminal: into its shell, a newline is Enter.",
            args: json!({ "window": window, "text": st("What to type, as it is."), "element": element }),
            needed: &["window", "text"],
            eager: true,
            run: |a| {
                on("type", a, |w| {
                    if let Some(e) = s(a, "element") {
                        w.push(if e.starts_with('@') { e } else { format!("@{e}") });
                    }
                    w.push("--".into());
                    words(w, a, &["text"]);
                })
            },
        },
        Tool {
            name: "key",
            about: "Press a key: enter tab escape backspace space up down left right delete home end pageup pagedown f1..f12.",
            args: json!({ "window": window, "key": st("Its name.") }),
            needed: &["window", "key"],
            eager: false,
            run: |a| on("key", a, |w| words(w, a, &["key"])),
        },
        Tool {
            name: "hotkey",
            about: "Press a chord: ctrl+l, ctrl+shift+t, alt+f4.",
            args: json!({ "window": window, "keys": st("Modifiers and a key, joined by +.") }),
            needed: &["window", "keys"],
            eager: false,
            run: |a| on("hotkey", a, |w| words(w, a, &["keys"])),
        },
        Tool {
            name: "click",
            about: "The agent's pointer clicks at a point of the window's picture (look first: a page moves). For a window that only does look, or what press cannot reach.",
            args: json!({ "window": window, "x": num("In the picture's coordinates."), "y": num("In the picture's coordinates."), "button": st("left (the default), right or middle."), "count": num("1 to 3 clicks.") }),
            needed: &["window", "x", "y"],
            eager: false,
            run: |a| {
                on("click", a, |w| {
                    words(w, a, &["x", "y"]);
                    if s(a, "button").is_some() || s(a, "count").is_some() {
                        w.push(s(a, "button").unwrap_or_else(|| "left".into()));
                    }
                    words(w, a, &["count"]);
                })
            },
        },
        Tool {
            name: "scroll",
            about: "The agent's pointer scrolls at a point of the window's picture.",
            args: json!({ "window": window, "x": num("In the picture's coordinates."), "y": num("In the picture's coordinates."), "direction": st("up, down (the default), left or right."), "steps": num("How many (3, up to 50).") }),
            needed: &["window", "x", "y"],
            eager: false,
            run: |a| {
                on("scroll", a, |w| {
                    words(w, a, &["x", "y"]);
                    if s(a, "direction").is_some() || s(a, "steps").is_some() {
                        w.push(s(a, "direction").unwrap_or_else(|| "down".into()));
                    }
                    words(w, a, &["steps"]);
                })
            },
        },
        Tool {
            name: "drag",
            about: "The agent's pointer drags from one point of the window's picture to another.",
            args: json!({ "window": window, "x1": num("From."), "y1": num("From."), "x2": num("To."), "y2": num("To.") }),
            needed: &["window", "x1", "y1", "x2", "y2"],
            eager: false,
            run: |a| on("drag", a, |w| words(w, a, &["x1", "y1", "x2", "y2"])),
        },
        Tool {
            name: "look",
            about: "A picture of the window (or, without one, the desktop), and its size; of a scene's item, its corner in the window's picture. Look only for what reading cannot tell: layout, an image, a colour.",
            args: json!({
                "window": window,
                "element": element,
                "text": text,
                "first": first,
                "box": { "type": "array", "items": { "type": "number" }, "description": "A part of it: [x, y, w, h] in the picture's coordinates." },
                "pad": num("Around an element, this many units."),
                "hidpi": flag("At the window's own pixels; says the scale."),
            }),
            needed: &[],
            eager: true,
            run: |a| {
                let said = ["element", "text", "box"].iter().filter(|k| a.get(**k).is_some()).count();
                if said > 1 {
                    return Err("look takes one of element, text or box".into());
                }
                let mut w = vec!["look".into()];
                words(&mut w, a, &["window"]);
                which(&mut w, a);
                flags(&mut w, a, &[("hidpi", "--hidpi")]);
                values(&mut w, a, &[("pad", "--pad")]);
                w.extend(a.get("box").and_then(Value::as_array).into_iter().flatten().map(|n| n.to_string()));
                Ok(Call { words: w, input: None })
            },
        },
        Tool {
            name: "run",
            about: "A terminal: a command typed into its shell, Enter, and once the shell waits again `mark M`, how many lines came before its last 200, and those. A timeout (5): what came so far; wait with until=prompt or tree with since=M goes on from there.",
            args: json!({ "window": window, "command": st("The command line, as it is."), "timeout": dur("How long to wait for it (30 s, up to 10 m): seconds, or 500ms, 5s, 2m."), "full": flag("All its lines, not the last 200.") }),
            needed: &["window", "command"],
            eager: false,
            run: |a| {
                on("run", a, |w| {
                    words(w, a, &["command"]);
                    w.extend(duration(a, "timeout"));
                    flags(w, a, &[("full", "--full")]);
                })
            },
        },
        Tool {
            name: "wait",
            about: "Wait until something holds, and answer `ok` and what met it. until: has (text shown), gone (text, or a scene's item), state / not (a scene's item has, or lost, state), changed, settled (a terminal quiet for that long), prompt (a terminal's shell waits again), expr (a scene's expression). What a window does not wait for is refused (exit 3).",
            args: json!({
                "window": window,
                "until": { "type": "string", "enum": ["has", "gone", "state", "not", "changed", "settled", "prompt", "expr"] },
                "text": st("For has and gone."),
                "element": element,
                "state": st("For state and not: disabled, enabled, checked, selected, offscreen."),
                "quiet": dur("For settled: quiet for how long (500ms)."),
                "expr": st("For expr: status == \"Saved\"."),
                "from": num("A terminal, for has: lines after its mark M."),
                "timeout": dur("How long (30 s, up to 60 s; prompt up to 10 m)."),
                "json": flag("What met it as JSON."),
            }),
            needed: &["window", "until"],
            eager: false,
            run: |a| {
                let cond = condition(a)?;
                on("wait", a, |w| {
                    w.extend(cond);
                    w.extend(duration(a, "timeout"));
                    values(w, a, &[("from", "--from")]);
                    flags(w, a, &[("json", "--json")]);
                })
            },
        },
        Tool {
            name: "listen",
            about: "What happens in a window for a while, a line each: a scene's events; a terminal's new lines.",
            args: json!({ "window": window, "duration": dur("How long (10 s, up to 60 s).") }),
            needed: &["window"],
            eager: false,
            run: |a| {
                let secs = match a.get("duration") {
                    None => 10.0,
                    Some(v) => match v.as_f64() {
                        Some(n) => n,
                        None => crate::agent_cli::seconds(&s(a, "duration").unwrap_or_default())?,
                    },
                };
                on("listen", a, |w| w.push(format!("{}s", secs.clamp(0.1, LISTEN))))
            },
        },
        Tool {
            name: "say",
            about: "Any order to a pleamar scene: type query words, drag knob 0 -40.",
            args: json!({ "window": window, "order": st("The order, as the scene takes it.") }),
            needed: &["window", "order"],
            eager: false,
            run: |a| on("say", a, |w| words(w, a, &["order"])),
        },
        Tool {
            name: "do",
            about: "Several steps on one window in one call, in order, stopping at the first that fails. Each step is a line as the CLI says it without `pleamar-wm agent` and the window (\"press @save\", \"type @query -- 'ana'\", \"key tab\", \"wait has --text OK 5s\"), or a tool's arguments without the window ({\"tool\": \"press\", \"element\": \"@save\"}). The answer is each step's under `step N: COMMAND`; on a failure its number, and the exit code is its.",
            args: json!({ "window": window, "steps": { "type": "array", "items": { "type": ["string", "object"] }, "description": "The steps." } }),
            needed: &["window", "steps"],
            eager: true,
            run: |a| {
                let w = s(a, "window").unwrap_or_default();
                let lines = a.get("steps").and_then(Value::as_array).ok_or("steps: a list")?.iter().map(|v| step(&w, v)).collect::<Result<Vec<_>, _>>()?;
                Ok(Call { words: vec!["do".into(), w], input: Some(lines.join("\n") + "\n") })
            },
        },
        Tool {
            name: "done",
            about: "Finished: the light on the monitor goes out now. Say it when you are done with the desktop.",
            args: json!({}),
            needed: &[],
            eager: false,
            run: |_| Ok(Call { words: vec!["done".into()], input: None }),
        },
    ]
}

const INSTRUCTIONS: &str = "You use a pleamar-wm desktop with the agent's own pointer and keyboard; the user's stay theirs. The loop:
1. Find the window and what it does: windows (or can for one). scene: read and act by item; text: a terminal, read as text and run commands; look: pictures and the pointer only; nothing: private, ask the user. Use only what it does: the rest is refused (exit 3) with what it can.
2. Open what is not open with open, never from your own shell.
3. Read before you look: tree gives a scene's items as @name role \"name\" X,Y WxH states, under their container; find for a long window. Look only for what reading cannot tell.
4. Act by item: press, type, focus with element; a terminal: run. Several steps on one window: do. Click by coordinates only when the window only does look.
5. Check from the action's answer; wait for what takes time; tree with since for what changed since your last read.
6. A failure ends with `exit N`; decide by N: 1 read the message; 2 private (ask the user, never another way); 3 not for this window; 4 the item is gone (read again); 5 timeout (after an action, the action happened); 6 ambiguous (name one); 7 the window went (windows again); 8 the user stopped you (stop, say where you left it); 64 wrong arguments; 69 no session (tell the user).
7. done when you are finished.";

/// What a process printed and exited with.
struct Ran {
    code: i32,
    out: String,
    err: String,
}

/// `pleamar-wm agent WORDS...`, `input` on its input.
fn exec(words: &[String], input: Option<&str>) -> Ran {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe().unwrap_or_else(|_| "pleamar-wm".into());
    let child = Command::new(exe).arg("agent").args(words).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Ran { code: 1, out: String::new(), err: format!("pleamar-wm: {e}") },
    };
    if let (Some(text), Some(mut i)) = (input, child.stdin.take()) {
        let _ = i.write_all(text.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) => Ran { code: o.status.code().unwrap_or(1), out: String::from_utf8_lossy(&o.stdout).into_owned(), err: String::from_utf8_lossy(&o.stderr).into_owned() },
        Err(e) => Ran { code: 1, out: String::new(), err: format!("pleamar-wm: {e}") },
    }
}

/// Where a call's picture goes, a file of its own.
fn picture_file() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, Into::into);
    dir.join(format!("pleamar-agent-mcp-{}-{}.png", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)))
}

/// A tool called: its words run by `exec`, and what they answered.
fn reply(t: &Tool<Words>, args: &Value, exec: impl Fn(&[String], Option<&str>) -> Ran) -> Reply {
    let mut call = match (t.run)(args) {
        Ok(c) => c,
        Err(e) => return Reply { content: vec![Content::Text(format!("exit {USAGE}: {e}"))], error: true },
    };
    let file = (t.name == "look").then(picture_file);
    if let Some(f) = &file {
        call.words.extend(["-o".into(), f.display().to_string()]);
    }
    let ran = exec(&call.words, call.input.as_deref());
    let mut content = Vec::new();
    match &file {
        Some(f) => {
            let path = f.display().to_string();
            content.push(Content::Text(ran.out.replace(&format!("{path} "), "").replace(&path, "")));
            if let Ok(data) = std::fs::read(f) {
                content.push(Content::Image { data, mime: "image/png" });
            }
            let _ = std::fs::remove_file(f);
        }
        None => content.push(Content::Text(ran.out)),
    }
    let err = ran.err.trim_end();
    match ran.code {
        0 if err.trim().is_empty() => {}
        0 => content.push(Content::Text(err.to_owned())),
        n if err.trim().is_empty() => content.push(Content::Text(format!("exit {n}"))),
        n => content.push(Content::Text(format!("exit {n}: {err}"))),
    }
    Reply { content, error: ran.code != 0 }
}

pub fn run() {
    crate::mcp_serve::serve("pleamar-desktop", env!("CARGO_PKG_VERSION"), || INSTRUCTIONS.into(), &tools(), |t, a| reply(t, a, exec), std::io::stdin().lock(), std::io::stdout());
}

/// `agent setup [--yes] [--undo]`: this server registered with the agents
/// here (Claude Code, Codex), as `link setup` does its own; asks first.
pub fn setup(args: &[String]) -> Result<(), String> {
    use crate::link::{clients, go_ahead, mcp_plan, register_mcp, Client, NO_CLIENT};
    let (yes, undo) = (args.iter().any(|a| a == "--yes"), args.iter().any(|a| a == "--undo"));
    let (claude, codex) = clients()?;
    let here: Vec<(Client, &str)> = [(claude, Client::Claude, "Claude Code"), (codex, Client::Codex, "Codex")].into_iter().filter(|c| c.0).map(|c| (c.1, c.2)).collect();
    if here.is_empty() {
        return Err(NO_CLIENT.into());
    }
    if !go_ahead("pleamar-wm agent setup", &here.iter().map(|(c, _)| mcp_plan(*c, "pleamar-desktop", undo)).collect::<Vec<_>>(), yes)? {
        println!("nothing changed");
        return Ok(());
    }
    for (c, _) in &here {
        register_mcp(*c, "pleamar-desktop", &["agent", "mcp"], undo)?;
    }
    let names: Vec<&str> = here.iter().map(|(_, n)| *n).collect();
    println!("{}: the MCP server pleamar-desktop in {}.{}", if undo { "taken away" } else { "set up" }, names.join(" and "), if undo { "" } else { " Agents started from now on have it; restart the ones already open." });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> Tool<Words> {
        tools().into_iter().find(|t| t.name == name).unwrap()
    }

    fn words(name: &str, a: Value) -> Vec<String> {
        (tool(name).run)(&a).unwrap().words
    }

    #[test]
    fn the_tools_are_the_verbs() {
        let names: Vec<&str> = tools().iter().map(|t| t.name).collect();
        assert_eq!(names, ["windows", "can", "open", "tree", "find", "at", "press", "focus", "type", "key", "hotkey", "click", "scroll", "drag", "look", "run", "wait", "listen", "say", "do", "done"]);
        for t in tools() {
            assert!(t.about.is_ascii() && t.args.to_string().is_ascii(), "{}", t.name);
            for k in t.needed {
                assert!(t.args.get(k).is_some(), "{}: {k}", t.name);
            }
        }
        assert!(INSTRUCTIONS.is_ascii());
    }

    #[test]
    fn the_words_each_call_becomes() {
        let w = "4521";
        assert_eq!(words("windows", json!({})), ["windows"]);
        assert_eq!(words("done", json!({})), ["done"]);
        assert_eq!(words("can", json!({ "window": "7002:7044" })), ["can", "7002:7044"]);
        assert_eq!(words("open", json!({ "command": "foot -e htop", "monitor": 1 })), ["open", "--monitor", "1", "--", "foot -e htop"]);
        assert_eq!(words("open", json!({ "command": "--weird" })), ["open", "--", "--weird"]);
        assert_eq!(words("tree", json!({ "window": w })), ["tree", w]);
        assert_eq!(words("tree", json!({ "window": w, "since": true, "max": 20, "hidden": true, "json": true })), ["tree", w, "--since", "--max", "20", "--hidden", "--json"]);
        assert_eq!(words("tree", json!({ "window": w, "since": 12, "within": "@3", "role": "link", "focused": false })), ["tree", w, "--since", "12", "--within", "@3", "--role", "link"]);
        assert_eq!(words("tree", json!({ "window": "7002:7044", "history": true, "long": true })), ["tree", "7002:7044", "--history", "--long"]);
        assert_eq!(words("find", json!({ "window": w, "text": "Save as", "json": true })), ["find", w, "--text", "Save as", "--json"]);
        assert_eq!(words("at", json!({ "window": w, "x": 10, "y": 20.5 })), ["at", w, "10", "20.5"]);
        assert_eq!(words("press", json!({ "window": w, "element": "@24" })), ["press", w, "@24"]);
        assert_eq!(words("press", json!({ "window": w, "element": 24, "button": "right", "count": 2 })), ["press", w, "@24", "right", "2"]);
        assert_eq!(words("press", json!({ "window": w, "text": "OK", "first": true })), ["press", w, "--text", "OK", "--first"]);
        assert_eq!(words("focus", json!({ "window": w, "element": "save" })), ["focus", w, "@save"]);
        assert_eq!(words("focus", json!({ "window": w })), ["focus", w]);
        assert_eq!(words("type", json!({ "window": w, "element": "@12", "text": "--until @bob" })), ["type", w, "@12", "--", "--until @bob"]);
        assert_eq!(words("type", json!({ "window": w, "text": "-", "element": "@3" })), ["type", w, "@3", "--", "-"]);
        assert_eq!(words("key", json!({ "window": w, "key": "tab" })), ["key", w, "tab"]);
        assert_eq!(words("hotkey", json!({ "window": w, "keys": "ctrl+l" })), ["hotkey", w, "ctrl+l"]);
        assert_eq!(words("click", json!({ "window": w, "x": 1, "y": 2 })), ["click", w, "1", "2"]);
        assert_eq!(words("click", json!({ "window": w, "x": 1, "y": 2, "count": 2 })), ["click", w, "1", "2", "left", "2"]);
        assert_eq!(words("scroll", json!({ "window": w, "x": 1, "y": 2, "steps": 5 })), ["scroll", w, "1", "2", "down", "5"]);
        assert_eq!(words("scroll", json!({ "window": w, "x": 1, "y": 2, "direction": "up" })), ["scroll", w, "1", "2", "up"]);
        assert_eq!(words("drag", json!({ "window": w, "x1": 1, "y1": 2, "x2": 3, "y2": 4 })), ["drag", w, "1", "2", "3", "4"]);
        assert_eq!(words("look", json!({})), ["look"]);
        assert_eq!(words("look", json!({ "window": w, "element": "@24", "pad": 8 })), ["look", w, "@24", "--pad", "8"]);
        assert_eq!(words("look", json!({ "window": w, "hidpi": true })), ["look", w, "--hidpi"]);
        assert_eq!(words("look", json!({ "window": w, "box": [0, 0, 100, 50] })), ["look", w, "0", "0", "100", "50"]);
        assert_eq!(words("look", json!({ "window": w, "text": "OK", "first": true })), ["look", w, "--text", "OK", "--first"]);
        for bad in [json!({ "window": w, "text": "a", "element": "@1" }), json!({ "window": w, "element": "@save", "box": [0, 0, 1, 1] })] {
            assert!((tool("look").run)(&bad).is_err(), "{bad}");
        }
        assert_eq!(words("run", json!({ "window": "7002:7044", "command": "ls -l", "timeout": 60, "full": true })), ["run", "7002:7044", "ls -l", "60s", "--full"]);
        assert_eq!(words("run", json!({ "window": w, "command": "make", "timeout": "2m" })), ["run", w, "make", "2m"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "has", "text": "OK", "timeout": 5 })), ["wait", w, "has", "--text", "OK", "5s"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "has", "text": "x", "from": 120 })), ["wait", w, "has", "--text", "x", "--from", "120"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "gone", "text": "Loading" })), ["wait", w, "gone", "--text", "Loading"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "gone", "element": "@4" })), ["wait", w, "@4", "gone"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "state", "element": "@4", "state": "checked" })), ["wait", w, "@4", "checked"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "not", "element": "@4", "state": "busy", "json": true })), ["wait", w, "@4", "not", "busy", "--json"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "settled" })), ["wait", w, "settled", "500ms"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "settled", "quiet": "1s", "timeout": 10 })), ["wait", w, "settled", "1s", "10s"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "prompt" })), ["wait", w, "prompt"]);
        assert_eq!(words("wait", json!({ "window": w, "until": "expr", "expr": "status == \"Saved\"" })), ["wait", w, "expr", "status == \"Saved\""]);
        for bad in [json!({ "window": w, "until": "has" }), json!({ "window": w, "until": "soon" }), json!({ "window": w, "until": "state", "element": "@4" })] {
            assert!((tool("wait").run)(&bad).is_err(), "{bad}");
        }
        assert_eq!(words("listen", json!({ "window": w })), ["listen", w, "10s"]);
        assert_eq!(words("listen", json!({ "window": w, "duration": 600 })), ["listen", w, "60s"]);
        assert_eq!(words("listen", json!({ "window": w, "duration": "2s" })), ["listen", w, "2s"]);
        assert_eq!(words("listen", json!({ "window": w, "duration": "5m" })), ["listen", w, "60s"]);
        assert_eq!(words("say", json!({ "window": w, "order": "drag knob 0 -40" })), ["say", w, "drag knob 0 -40"]);
    }

    #[test]
    fn do_takes_lines_and_tools() {
        let c = (tool("do").run)(&json!({ "window": "4521", "steps": [
            "press @12",
            { "tool": "type", "element": "@12", "text": "ana 'maria'" },
            { "tool": "key", "key": "tab" },
            { "tool": "wait", "until": "has", "text": "OK", "timeout": 5 },
        ] }))
        .unwrap();
        assert_eq!(c.words, ["do", "4521"]);
        assert_eq!(c.input.as_deref(), Some("press @12\ntype @12 -- 'ana '\\''maria'\\'''\nkey tab\nwait has --text OK 5s\n"));
        for bad in [json!({ "tool": "windows" }), json!({ "tool": "do", "steps": [] }), json!({ "element": "@1" }), json!(3)] {
            assert!((tool("do").run)(&json!({ "window": "4521", "steps": [bad] })).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_call_answers_its_output_and_code() {
        let ok = reply(&tool("press"), &json!({ "window": "4521", "element": "@3" }), |w, i| {
            assert_eq!((w, i), (&["press".to_string(), "4521".into(), "@3".into()][..], None));
            Ran { code: 0, out: "pressed @3\n".into(), err: String::new() }
        });
        assert!(matches!(&ok.content[..], [Content::Text(t)] if t == "pressed @3\n"));
        assert!(!ok.error);
        let gone = reply(&tool("press"), &json!({ "window": "4521", "element": "@3" }), |_, _| Ran { code: 4, out: String::new(), err: "gone: @3\n".into() });
        assert!(matches!(&gone.content[..], [Content::Text(o), Content::Text(e)] if o.is_empty() && e == "exit 4: gone: @3"));
        assert!(gone.error);
        let bad = reply(&tool("wait"), &json!({ "window": "4521", "until": "soon" }), |_, _| unreachable!());
        assert!(bad.error);
        assert!(matches!(&bad.content[..], [Content::Text(e)] if e.starts_with("exit 64: ")));
        let script = reply(&tool("do"), &json!({ "window": "4521", "steps": ["key tab"] }), |w, i| {
            assert_eq!((w.join(" "), i), ("do 4521".into(), Some("key tab\n")));
            Ran { code: 0, out: "step 1: key tab\n".into(), err: String::new() }
        });
        assert!(!script.error);
    }

    #[test]
    fn look_answers_the_picture() {
        let r = reply(&tool("look"), &json!({ "window": "4521", "element": "@24" }), |w, _| {
            let [.., o, f] = w else { panic!() };
            assert_eq!(o, "-o");
            std::fs::write(f, b"\x89PNG..").unwrap();
            Ran { code: 0, out: format!("{f} 120x40 at 10,20\n"), err: String::new() }
        });
        match &r.content[..] {
            [Content::Text(t), Content::Image { data, mime }] => {
                assert_eq!(t, "120x40 at 10,20\n");
                assert_eq!((&data[..], *mime), (&b"\x89PNG.."[..], "image/png"));
            }
            _ => panic!("text and a picture"),
        }
        assert!(!r.error);
        // Not looked: no picture, and its error.
        let r = reply(&tool("look"), &json!({ "window": "4521" }), |_, _| Ran { code: 2, out: String::new(), err: "private: 4521".into() });
        assert_eq!(r.content.len(), 2);
        assert!(!r.content.iter().any(|c| matches!(c, Content::Image { .. })));
        assert!(r.error);
        assert!(matches!(&r.content[1], Content::Text(e) if e == "exit 2: private: 4521"));
    }
}
