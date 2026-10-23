//! An MCP server over stdio (JSON-RPC a line), whatever its tools: the
//! handshake, the list, the calls, pings and errors. Each server says its
//! name, what the agent is told when it starts, its tools and what a call
//! does; `link mcp` is one.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// A tool: its name, what it is for, its arguments (JSON Schema properties),
/// which of them are needed, whether Claude Code lists it in full from the
/// start, and what its server keeps to run it.
pub struct Tool<R = ()> {
    pub name: &'static str,
    pub about: &'static str,
    pub args: Value,
    pub needed: &'static [&'static str],
    pub eager: bool,
    pub run: R,
}

/// A part of what a call answers: words, or a picture (its bytes and type).
pub enum Content {
    Text(String),
    Image { data: Vec<u8>, mime: &'static str },
}

/// What a call answers: its parts, whether it failed, and the code it failed
/// with when there is one (`structuredContent.code`).
pub struct Reply {
    pub content: Vec<Content>,
    pub error: bool,
    pub code: Option<i32>,
}

impl Reply {
    pub fn text(text: String, error: bool) -> Self {
        Reply { content: vec![Content::Text(text)], error, code: None }
    }
}

/// Answer `input` on `out` until it ends. `instructions` is asked on every
/// `initialize`; `call` gets a known tool with its needed arguments there.
pub fn serve<R>(name: &str, version: &str, instructions: impl Fn() -> String, tools: &[Tool<R>], mut call: impl FnMut(&Tool<R>, &Value) -> Reply, input: impl BufRead, mut out: impl Write) {
    for line in input.lines() {
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
                "serverInfo": { "name": name, "version": version },
                "instructions": instructions(),
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools.iter().map(|t| {
                let mut v = json!({
                    "name": t.name,
                    "description": t.about,
                    "inputSchema": { "type": "object", "properties": t.args, "required": t.needed },
                });
                if t.eager {
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
                        let missing: Vec<&str> = t.needed.iter().copied().filter(|k| args.get(*k).is_none()).collect();
                        let reply = if missing.is_empty() { call(t, &args) } else { Reply::text(format!("missing: {}", missing.join(", ")), true) };
                        Ok(answer(reply))
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
}

/// A call's reply as MCP says it; nothing said is `done`.
fn answer(reply: Reply) -> Value {
    let mut content: Vec<Value> = reply
        .content
        .into_iter()
        .filter(|c| !matches!(c, Content::Text(t) if t.is_empty()))
        .map(|c| match c {
            Content::Text(text) => json!({ "type": "text", "text": text }),
            Content::Image { data, mime } => json!({ "type": "image", "data": crate::remote::base64(&data), "mimeType": mime }),
        })
        .collect();
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "done" }));
    }
    let mut v = json!({ "content": content, "isError": reply.error });
    if let Some(code) = reply.code {
        v["structuredContent"] = json!({ "code": code });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> Vec<Tool> {
        vec![
            Tool { name: "echo", about: "Says it back.", args: json!({ "text": { "type": "string" } }), needed: &["text"], eager: true, run: () },
            Tool { name: "fail", about: "Fails.", args: json!({}), needed: &[], eager: false, run: () },
            Tool { name: "look", about: "A picture.", args: json!({}), needed: &[], eager: false, run: () },
        ]
    }

    /// Each answer `serve` gives to these lines, in order.
    fn talk(lines: &[Value]) -> Vec<Value> {
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut out = Vec::new();
        serve(
            "fake",
            "1.2.3",
            || "be kind".into(),
            &fake(),
            |t, a| match t.name {
                "echo" => Reply::text(a["text"].as_str().unwrap_or("").into(), false),
                "fail" => Reply { content: vec![Content::Text("gone".into())], error: true, code: Some(4) },
                _ => Reply { content: vec![Content::Text("here".into()), Content::Image { data: b"png".to_vec(), mime: "image/png" }], error: false, code: None },
            },
            input.as_bytes(),
            &mut out,
        );
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn call(id: i64, name: &str, args: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } })
    }

    #[test]
    fn handshake_and_list() {
        let a = talk(&[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }),
            json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }),
        ]);
        assert_eq!(a.len(), 4);
        assert_eq!(a[0]["result"], json!({ "protocolVersion": "2025-03-26", "capabilities": { "tools": {} }, "serverInfo": { "name": "fake", "version": "1.2.3" }, "instructions": "be kind" }));
        let tools = a[1]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), ["echo", "fail", "look"]);
        assert_eq!(tools[0]["inputSchema"], json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }));
        assert_eq!(tools[0]["_meta"], json!({ "anthropic/alwaysLoad": true }));
        assert!(tools[1].get("_meta").is_none());
        assert_eq!(a[2], json!({ "jsonrpc": "2.0", "id": 3, "result": {} }));
        assert_eq!(a[3]["error"], json!({ "code": -32601, "message": "no method resources/list" }));
    }

    #[test]
    fn calls() {
        let a = talk(&[call(1, "echo", json!({ "text": "hi" })), call(2, "echo", json!({ "text": "" })), call(3, "echo", json!({})), call(4, "fail", json!({})), call(5, "nope", json!({}))]);
        assert_eq!(a[0]["result"], json!({ "content": [{ "type": "text", "text": "hi" }], "isError": false }));
        assert_eq!(a[1]["result"]["content"], json!([{ "type": "text", "text": "done" }]));
        assert_eq!(a[2]["result"], json!({ "content": [{ "type": "text", "text": "missing: text" }], "isError": true }));
        assert_eq!(a[3]["result"], json!({ "content": [{ "type": "text", "text": "gone" }], "isError": true, "structuredContent": { "code": 4 } }));
        assert_eq!(a[4]["error"], json!({ "code": -32602, "message": "no tool called nope" }));
    }

    #[test]
    fn a_picture() {
        let a = talk(&[call(1, "look", json!({}))]);
        assert_eq!(a[0]["result"]["content"], json!([{ "type": "text", "text": "here" }, { "type": "image", "data": "cG5n", "mimeType": "image/png" }]));
    }
}
