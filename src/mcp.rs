//! `pleamar-wm link mcp`: the same as `pleamar-wm link`, as an MCP server
//! (stdio, JSON-RPC a line) for the agent in a terminal of the session.
//!
//! What it adds to the commands is that the agent knows, from the start,
//! that it can: the server's instructions say which window it is and which
//! windows the person wired to it, and that a wired agent is reached through
//! these tools —not by starting another one of its own (`codex exec`), nor
//! with the cursor—. The tools are `link`'s verbs; the wire is still the
//! permission, checked on every call.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// The tools: name, what it is for, its arguments (JSON Schema properties),
/// which of them are needed, and the `link` words they become.
struct Tool {
    name: &'static str,
    about: &'static str,
    args: Value,
    needed: &'static [&'static str],
    words: fn(&Value) -> Vec<String>,
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn tools() -> Vec<Tool> {
    let window = json!({ "type": "string", "description": "The window: its number from link_list, the name of its wire («tests»), or the agent that runs there («codex») if only one does." });
    vec![
        Tool {
            name: "link_whoami",
            about: "Which window of the desktop you are in, and which windows the person wired to yours (their number, the wire's name, the agent there and its folder).",
            args: json!({}),
            needed: &[],
            words: |_| vec!["whoami".into()],
        },
        Tool {
            name: "link_list",
            about: "The windows wired to yours: number, the wire's name, title, the agent that runs there (claude, codex…) and whether it works, is ready or waits for its person, and its folder.",
            args: json!({}),
            needed: &[],
            words: |_| vec!["list".into()],
        },
        Tool {
            name: "link_delegate",
            about: "Hand a task to the agent in a wired window (another Claude Code, Codex…): it is typed there as a pasted task saying it comes from you, and its answer comes back. With wait=false (the default) this returns at once and the answer —or a question it stops to ask on the way— is typed into your terminal when it is there; with wait=true this call returns the answer (or the question). This is how you talk to another agent the person wired to you: never start another agent of your own for it (codex exec, claude -p…), and never use the screen's cursor.",
            args: json!({ "window": window, "task": { "type": "string", "description": "What it has to do, as you would ask it." }, "wait": { "type": "boolean", "description": "Wait here for its answer (true), or have it typed into your terminal later (false, the default)." }, "timeout": { "type": "number", "description": "Seconds to give it (600)." } }),
            needed: &["window", "task"],
            words: |a| {
                let mut w = vec!["delegate".into(), s(a, "window"), s(a, "task")];
                if a.get("wait").and_then(Value::as_bool) == Some(true) {
                    w.push("--wait".into());
                }
                if !s(a, "timeout").is_empty() {
                    w.extend(["--timeout".into(), s(a, "timeout")]);
                }
                w
            },
        },
        Tool {
            name: "link_wait",
            about: "Wait until the agent in a wired window is done with what it is doing, and get its answer; or what it stopped to ask its person.",
            args: json!({ "window": window, "timeout": { "type": "number", "description": "Seconds (600)." } }),
            needed: &["window"],
            words: |a| {
                let mut w = vec!["wait".into(), s(a, "window")];
                if !s(a, "timeout").is_empty() {
                    w.extend(["--timeout".into(), s(a, "timeout")]);
                }
                w
            },
        },
        Tool {
            name: "link_ask",
            about: "What the agent in a wired window is asking its person now, if anything: a permission, a choice, a yes/no — the question and its options, the selected one marked.",
            args: json!({ "window": window }),
            needed: &["window"],
            words: |a| vec!["ask".into(), s(a, "window")],
        },
        Tool {
            name: "link_answer",
            about: "Answer what the agent in a wired window asks: an option by its number or its words, free words where it lets you type them, or yes/no. The window shows who answered. The wire is the person's permission to do this.",
            args: json!({ "window": window, "answer": { "type": "string", "description": "An option's number (\"2\"), the words of an option, words to type, or yes/no." } }),
            needed: &["window", "answer"],
            words: |a| vec!["answer".into(), s(a, "window"), s(a, "answer")],
        },
        Tool {
            name: "link_read",
            about: "The last lines of a wired window's terminal, as its screen and history show them.",
            args: json!({ "window": window, "lines": { "type": "number", "description": "How many (200)." } }),
            needed: &["window"],
            words: |a| {
                let mut w = vec!["read".into(), s(a, "window")];
                if !s(a, "lines").is_empty() {
                    w.push(s(a, "lines"));
                }
                w
            },
        },
        Tool {
            name: "link_send",
            about: "Type text into a wired window's terminal as a paste (and Enter after it, with enter=true). For a shell or a program there; to hand a task to an agent, link_delegate.",
            args: json!({ "window": window, "text": { "type": "string" }, "enter": { "type": "boolean" } }),
            needed: &["window", "text"],
            words: |a| {
                let mut w = vec!["send".into(), s(a, "window"), s(a, "text")];
                if a.get("enter").and_then(Value::as_bool) == Some(true) {
                    w.push("--enter".into());
                }
                w
            },
        },
        Tool {
            name: "link_keys",
            about: "Press keys in a wired window's terminal, in order: enter tab escape up down left right backspace delete home end page_up page_down space ctrl+c, or one character. Returns its screen after.",
            args: json!({ "window": window, "keys": { "type": "array", "items": { "type": "string" } } }),
            needed: &["window", "keys"],
            words: |a| {
                let mut w = vec!["keys".into(), s(a, "window")];
                w.extend(a.get("keys").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
                w
            },
        },
        Tool {
            name: "link_label",
            about: "Give the wire to a window a name («tests», «frontend»): it is shown on the wire, and the window can be named by it.",
            args: json!({ "window": window, "name": { "type": "string" } }),
            needed: &["window", "name"],
            words: |a| vec!["label".into(), s(a, "window"), s(a, "name")],
        },
        Tool {
            name: "link_spawn",
            about: "Open a new terminal with an agent (claude, codex…) beside your window, already wired to it, and hand it a task. Returns its window number; its answer comes as with link_delegate. If the agent asks something before it is ready (whether to trust the folder), the question is returned for you to answer with link_answer (whether to trust a folder is the person's call), and the task is handed by itself once it is.",
            args: json!({ "agent": { "type": "string", "description": "claude, codex, gemini, opencode…" }, "task": { "type": "string" }, "folder": { "type": "string", "description": "Where it works (yours by default)." }, "name": { "type": "string", "description": "A name for the wire («tests»)." }, "model": { "type": "string", "description": "The model it starts on, as that agent names it (claude-sonnet-5-5, gpt-6-luna…); its own default if not said." } }),
            needed: &["agent", "task"],
            words: |a| {
                let mut w = vec!["spawn".into(), s(a, "agent"), s(a, "task")];
                for (k, flag) in [("folder", "--folder"), ("name", "--name"), ("model", "--model")] {
                    if !s(a, k).is_empty() {
                        w.extend([flag.into(), s(a, k)]);
                    }
                }
                w
            },
        },
    ]
}

/// What the agent is told when it starts: where it is and what it can do.
fn instructions() -> String {
    let base = "You run in a terminal of a pleamar-wm desktop. The person can wire windows together (a current of water between them): an agent in a window wired to yours —another Claude Code, Codex, Gemini…— is reached with these tools. To tell, ask or hand a task to another agent the person names («dile a codex…», «ask the one in tests…»), use link_delegate with that window, never another agent of your own (codex exec, claude -p…) and never the screen's cursor. If it stops to ask its person something, link_ask and link_answer. If what they name is not wired to you, say so and ask them to wire it: on a free monitor (Super+W), drag the drop at the start of a window's title bar onto the other window.";
    match crate::link::briefing() {
        Ok(now) => format!("{base}\n\nNow: {now}"),
        Err(e) => format!("{base}\n\nNow: {e}"),
    }
}

pub fn run() -> i32 {
    let tools = tools();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        // A notice (no id) asks for no answer.
        let Some(id) = msg.get("id").cloned() else { continue };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "pleamar-link", "version": env!("CARGO_PKG_VERSION") },
                "instructions": instructions(),
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools.iter().map(|t| {
                let mut v = json!({
                    "name": t.name,
                    "description": t.about,
                    "inputSchema": { "type": "object", "properties": t.args, "required": t.needed },
                });
                // Claude Code lists these in full from the start.
                if matches!(t.name, "link_delegate" | "link_list") {
                    v["_meta"] = json!({ "anthropic/alwaysLoad": true });
                }
                v
            }).collect::<Vec<_>>() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match tools.iter().find(|t| t.name == name) {
                    None => Err((-32602, format!("no tool called {name}"))),
                    Some(t) => {
                        let missing: Vec<&str> = t.needed.iter().copied().filter(|k| s(&args, k).is_empty() && args.get(*k).is_none()).collect();
                        let (text, error) = if missing.is_empty() {
                            match crate::link::go(&(t.words)(&args)) {
                                Ok(text) => (text, false),
                                Err(e) => (e, true),
                            }
                        } else {
                            (format!("missing: {}", missing.join(", ")), true)
                        };
                        Ok(json!({ "content": [{ "type": "text", "text": if text.is_empty() { "done".to_owned() } else { text } }], "isError": error }))
                    }
                }
            }
            _ => Err((-32601, format!("no method {method}"))),
        };
        let reply = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        };
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
    0
}
