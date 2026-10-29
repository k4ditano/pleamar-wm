//! `pleamar-wm link mcp`: the same as `pleamar-wm link`, as an MCP server
//! (stdio, JSON-RPC a line) for the agent in a terminal of the session.
//!
//! What it adds to the commands is that the agent knows, from the start,
//! that it can: the server's instructions say which window it is and which
//! windows the person wired to it, and that a wired agent is reached through
//! these tools —not by starting another one of its own (`codex exec`), nor
//! with the cursor—. The tools are `link`'s verbs; the wire is still the
//! permission, checked on every call.

use crate::mcp_serve::{Reply, Tool};
use serde_json::{json, Value};

/// The `link` words a tool becomes.
type Words = fn(&Value) -> Vec<String>;

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn tools() -> Vec<Tool<Words>> {
    let window = json!({ "type": "string", "description": "The window: its number from link_list, the name of its wire («tests»), or the agent that runs there («codex») if only one does." });
    vec![
        Tool {
            name: "link_whoami",
            about: "Which window of the desktop you are in, and which windows the person wired to yours (their number, the wire's name, the agent there and its folder).",
            args: json!({}),
            needed: &[],
            eager: false,
            run: |_| vec!["whoami".into()],
        },
        Tool {
            name: "link_list",
            about: "The windows wired to yours: number, the wire's name, title, the agent that runs there (claude, codex…) and whether it works, is ready or waits for its person, and its folder.",
            args: json!({}),
            needed: &[],
            eager: true,
            run: |_| vec!["list".into()],
        },
        Tool {
            name: "link_delegate",
            about: "Hand a task to the agent in a wired window (another Claude Code, Codex…), or —if you are a companion of a caravan— to any companion of yours by name, through its mailbox, with no wire. To a companion it is always asynchronous: end your turn after it; its answer comes to you as a new message. To a wired window: it is typed there as a pasted task saying it comes from you, and its answer comes back. With wait=false (the default) this returns at once and the answer —or a question it stops to ask on the way— is typed into your terminal when it is there; with wait=true this call returns the answer (or the question). This is how you talk to another agent the person wired to you: never start another agent of your own for it (codex exec, claude -p…), and never use the screen's cursor.",
            args: json!({ "window": window, "task": { "type": "string", "description": "What it has to do, as you would ask it." }, "wait": { "type": "boolean", "description": "Wired windows only: wait here for its answer (true), or have it typed into your terminal later (false, the default). A companion is never waited for." }, "expects_reply": { "type": "boolean", "description": "Companions: true, the default, for a request, a question or a task (its answer comes back to you). False for information that asks for no answer: a heads-up, a status, «done»." }, "timeout": { "type": "number", "description": "With wait=true, seconds to wait here (600). With wait=false its answer is brought however long it works; this many seconds without any sign of it working (6 hours if not said), and you are told it has not answered." } }),
            needed: &["window", "task"],
            eager: true,
            run: |a| {
                let mut w = vec!["delegate".into(), s(a, "window"), s(a, "task")];
                if a.get("wait").and_then(Value::as_bool) == Some(true) {
                    w.push("--wait".into());
                }
                if a.get("expects_reply").and_then(Value::as_bool) == Some(false) {
                    w.push("--no-reply".into());
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
            eager: false,
            run: |a| {
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
            eager: false,
            run: |a| vec!["ask".into(), s(a, "window")],
        },
        Tool {
            name: "link_answer",
            about: "Answer what the agent in a wired window asks: an option by its number or its words, free words where it lets you type them, or yes/no. The window shows who answered. The wire is the person's permission to do this.",
            args: json!({ "window": window, "answer": { "type": "string", "description": "An option's number (\"2\"), the words of an option, words to type, or yes/no." } }),
            needed: &["window", "answer"],
            eager: false,
            run: |a| vec!["answer".into(), s(a, "window"), s(a, "answer")],
        },
        Tool {
            name: "link_read",
            about: "The last lines of a wired window's terminal, as its screen and history show them.",
            args: json!({ "window": window, "lines": { "type": "number", "description": "How many (200)." } }),
            needed: &["window"],
            eager: false,
            run: |a| {
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
            eager: false,
            run: |a| {
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
            eager: false,
            run: |a| {
                let mut w = vec!["keys".into(), s(a, "window")];
                w.extend(a.get("keys").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
                w
            },
        },
        Tool {
            name: "link_restart_me",
            about: "Restart your own session, in this same conversation: what you added or changed since you started (an MCP server with `claude mcp add`, a skill, your settings) is only loaded by starting again. Only a companion (a named terminal) started by its shell can. After it, end your turn: a letter tells you to carry on, with `then` if you give it.",
            args: json!({ "then": { "type": "string", "description": "What to carry on with once you are back (your letter says it)." } }),
            needed: &[],
            eager: false,
            run: |a| vec!["restart-me".into(), s(a, "then")],
        },
        Tool {
            name: "link_sent",
            about: "Where the letters you sent to your companions are: waiting (and how many before), handed, being worked on, answered or not, each with how the one it went to is (free, working, in its shell, not started, could not start). Look here instead of asking a companion whether it got your letter: asking costs a turn of yours and of theirs. Only for a companion (a named terminal).",
            args: json!({}),
            needed: &[],
            eager: false,
            run: |_| vec!["sent".into()],
        },
        Tool {
            name: "link_label",
            about: "Give the wire to a window a name («tests», «frontend»): it is shown on the wire, and the window can be named by it.",
            args: json!({ "window": window, "name": { "type": "string" } }),
            needed: &["window", "name"],
            eager: false,
            run: |a| vec!["label".into(), s(a, "window"), s(a, "name")],
        },
        Tool {
            name: "link_spawn",
            about: "Open a new terminal with an agent (claude, codex…) beside your window, already wired to it, and hand it a task. Returns its window number; its answer comes as with link_delegate. If the agent asks something before it is ready (whether to trust the folder), the question is returned for you to answer with link_answer (whether to trust a folder is the person's call), and the task is handed by itself once it is.",
            args: json!({ "agent": { "type": "string", "description": "claude, codex, gemini, opencode…" }, "task": { "type": "string" }, "folder": { "type": "string", "description": "Where it works (yours by default)." }, "name": { "type": "string", "description": "A name for the wire («tests»)." }, "model": { "type": "string", "description": "The model it starts on, as that agent names it (claude-sonnet-5-5, gpt-6-luna…); its own default if not said." } }),
            needed: &["agent", "task"],
            eager: false,
            run: |a| {
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
    let base = "You run in a terminal of a pleamar-wm desktop. The person can wire windows together (a current of water between them): an agent in a window wired to yours —another Claude Code, Codex, Gemini…— is reached with these tools. To tell, ask or hand a task to another agent the person names («dile a codex…», «ask the one in tests…»), use link_delegate with that window, never another agent of your own (codex exec, claude -p…) and never the screen's cursor. If it stops to ask its person something, link_ask and link_answer. If what they name is not wired to you, say so and ask them to wire it: on a free monitor (Super+W), drag the drop at the start of a window's title bar onto the other window. But if you are a companion of a caravan (your terminal has a name), the others of your caravan are your team: ask any of them by name with link_delegate, no wire needed —it goes through their mailbox, and one not started is started—. Work with your team as teammates do: hand the request, say so, and end your turn; never wait for them nor check on them. Their answer comes to you as a new message, and you carry on with it then. To know whether a letter of yours arrived or where it is, link_sent says it: never ask them. For information that asks for no answer (a heads-up, «done»), expects_reply false. If you add an MCP server, a skill or a setting that only loads when you start, do not ask your person to restart you: link_restart_me does it, in this same conversation.";
    match crate::link::briefing() {
        Ok(now) => format!("{base}\n\nNow: {now}"),
        Err(e) => format!("{base}\n\nNow: {e}"),
    }
}

pub fn run() -> i32 {
    crate::mcp_serve::serve(
        "pleamar-link",
        env!("CARGO_PKG_VERSION"),
        instructions,
        &tools(),
        |t, args| match crate::link::go(&(t.run)(args)) {
            Ok(text) => Reply::text(text, false),
            Err(e) => Reply::text(e, true),
        },
        std::io::stdin().lock(),
        std::io::stdout(),
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tools_as_listed() {
        let mut out = Vec::new();
        let ask = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n";
        crate::mcp_serve::serve("pleamar-link", "0", String::new, &tools(), |_, _| unreachable!(), ask.as_bytes(), &mut out);
        let listed: Value = serde_json::from_slice(&out).unwrap();
        let kept: Value = serde_json::from_str(include_str!("mcp_link_tools.json")).unwrap();
        assert_eq!(listed["result"]["tools"], kept);
    }

    #[test]
    fn the_words_a_call_becomes() {
        let words = |name: &str, args: Value| (tools().iter().find(|t| t.name == name).unwrap().run)(&args);
        assert_eq!(words("link_delegate", json!({ "window": 3, "task": "go", "wait": true, "timeout": 5 })), ["delegate", "3", "go", "--wait", "--timeout", "5"]);
        assert_eq!(words("link_keys", json!({ "window": "tests", "keys": ["up", "enter"] })), ["keys", "tests", "up", "enter"]);
        assert_eq!(words("link_spawn", json!({ "agent": "codex", "task": "t", "model": "m" })), ["spawn", "codex", "t", "--model", "m"]);
    }
}
