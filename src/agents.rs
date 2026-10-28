//! `pleamar-wm agents …`: the session's agents and what they ask, for a shell
//! that shows them to the person (Calima's tray). `link` speaks for an agent,
//! and only of the windows wired to its own; this speaks for the person, of
//! every window of the session, in a form a program reads (one JSON object a
//! line). And it answers only the question that was shown: each one has a
//! key, and an answer given with a key that is no longer on the screen —the
//! question changed, or another took its place— is refused, never sent to it.

use crate::term::Question;
use std::collections::BTreeMap;

struct Agent {
    slot: usize,
    layer: i32,
    agent: String,
    cwd: String,
    title: String,
    /// "working", "ready" or "asking": as `ag.N` says it in the scene.
    state: &'static str,
    question: Option<Question>,
    /// What the screen says around the question: the dialog it is in, from its
    /// top edge (or the last rule) down to the options, without the box —where
    /// an agent shows the command or the file it asks about—. At most 12 lines.
    detail: Vec<String>,
}

/// The key of a question: FNV-1a of what makes it the same one again
/// (`link`'s `ask_key`: its options, or its words if it has none), so the
/// same question drawn again keeps it, and another gets another.
fn key(q: &Question) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in crate::link::ask_key(q).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// The terminal of that window with an agent in it, of its layers: the
/// busiest of those with one, as the scene's `ag.N` takes it (term.rs).
fn layer_with_agent(pid: u32, layers: &[(i32, BTreeMap<String, String>)]) -> Option<(i32, BTreeMap<String, String>)> {
    layers
        .iter()
        .filter(|(_, i)| i.get("terminal").and_then(|t| t.parse::<u32>().ok()) == Some(pid))
        .filter(|(_, i)| i.get("agent").is_some_and(|a| !a.is_empty()))
        .min_by_key(|(_, i)| i.get("quiet").and_then(|q| q.parse::<u64>().ok()).unwrap_or(u64::MAX))
        .cloned()
}

fn agents() -> Result<Vec<Agent>, String> {
    let wins = crate::link::windows()?;
    let layers = crate::term::layers();
    let mut out = Vec::new();
    for w in &wins {
        let Some((id, info)) = layer_with_agent(w.pid, &layers) else { continue };
        let hook = info.get("hook").map(String::as_str).unwrap_or("");
        let working = if hook.is_empty() { info.get("state").map(String::as_str) == Some("working") } else { hook == "working" };
        let question = if working { None } else { crate::link::asked(id, &info) };
        let detail = if question.is_some() { detail_of(&crate::term::ask(&format!("{}/{id}.sock", crate::term::socket_dir()), "read 40").unwrap_or_default()) } else { Vec::new() };
        let state = if question.is_some() || hook == "asking" {
            "asking"
        } else if working {
            "working"
        } else {
            "ready"
        };
        out.push(Agent {
            slot: w.slot,
            layer: id,
            agent: info.get("agent").cloned().unwrap_or_default(),
            cwd: info.get("cwd").cloned().unwrap_or_default(),
            title: w.title.clone(),
            state,
            question,
            detail,
        });
    }
    Ok(out)
}

/// Every agent of the session as `agents --json` gives it (remote's caravan page).
pub(crate) fn listed() -> Vec<serde_json::Value> {
    agents().unwrap_or_default().iter().filter_map(|a| serde_json::from_str(&json(a)).ok()).collect()
}

/// `agents answer …` or `agents stop …`, for the remote's caravan page.
pub(crate) fn act(args: &[String]) -> Result<String, String> {
    go(args).map_err(|(_, e)| e)
}

/// The dialog round the question on a screen (see `Agent::detail`).
fn detail_of(screen: &str) -> Vec<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let tail = &lines[lines.len().saturating_sub(40)..];
    let is_option = |l: &str| {
        let t = l.trim().trim_matches(|c| matches!(c, '│' | '┃' | '║' | '|')).trim();
        let t = t.trim_start_matches(['❯', '›', '>', '▶', '→']).trim_start();
        let d: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        !d.is_empty() && t[d.len()..].starts_with(['.', ')'])
    };
    let Some(first) = tail.iter().position(|l| is_option(l)) else { return Vec::new() };
    let edge = |l: &str| {
        let t = l.trim();
        t.starts_with(['╭', '┌', '╔']) || (t.chars().count() >= 10 && t.chars().all(|c| matches!(c, '─' | '━' | '═')))
    };
    let top = tail[..first].iter().rposition(|l| edge(l)).map_or(first.saturating_sub(12), |k| k + 1);
    let kept: Vec<String> = tail[top..first]
        .iter()
        .map(|l| l.trim().trim_matches(|c| matches!(c, '│' | '┃' | '║' | '|')).trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    kept[kept.len().saturating_sub(12)..].to_vec()
}

/// The options a program answers by number: a yes/no question has two, 1 yes and 2 no.
fn options(q: &Question) -> Vec<(u32, String)> {
    if q.yes_no {
        vec![(1, "yes".into()), (2, "no".into())]
    } else {
        q.options.clone()
    }
}

fn json(a: &Agent) -> String {
    let question = a.question.as_ref().map(|q| {
        serde_json::json!({
            "key": key(q),
            "text": q.text,
            "yes_no": q.yes_no,
            "cursor": q.cursor,
            "options": options(q).iter().map(|(n, t)| serde_json::json!({ "n": n, "text": t })).collect::<Vec<_>>(),
        })
    });
    serde_json::json!({
        "slot": a.slot,
        "layer": a.layer,
        "agent": a.agent,
        "cwd": a.cwd,
        "title": a.title,
        "state": a.state,
        "question": question,
        "detail": a.detail,
    })
    .to_string()
}

fn human(a: &Agent) -> String {
    let mut out = format!("window {} · {} · {} · {}", a.slot, a.agent, a.state, a.cwd);
    if let Some(q) = &a.question {
        for l in &a.detail {
            out.push_str(&format!("\n  | {l}"));
        }
        out.push_str(&format!("\n  asks [{}]: {}", key(q), q.text));
        for (n, t) in options(q) {
            out.push_str(&format!("\n    {n}) {t}"));
        }
    }
    out
}

fn find(slot: &str) -> Result<Agent, String> {
    let slot: usize = slot.parse().map_err(|_| format!("«{slot}» is not a window's number"))?;
    agents()?.into_iter().find(|a| a.slot == slot).ok_or_else(|| format!("window {slot} runs no agent"))
}

/// Anything that looks like a secret, hidden: tokens with a known shape
/// (`sk-…`, `ghp_…`, `github_pat_…`, `xoxb-…`, `glpat-…`, `AKIA…`), what follows
/// `Bearer`, `password=`, `token=`, `secret=`, `--password`, and any run of 32
/// or more letters and digits (a key, a hash).
pub(crate) fn redact(text: &str) -> String {
    const PREFIXES: [&str; 8] = ["sk-", "ghp_", "gho_", "ghs_", "github_pat_", "xoxb-", "xoxp-", "glpat-"];
    const AFTER: [&str; 6] = ["bearer", "password=", "passwd=", "token=", "secret=", "--password"];
    let words: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::new();
    let mut hide_next = false;
    for w in words {
        let low = w.to_lowercase();
        let bare = w.trim_matches(|c: char| !c.is_alphanumeric());
        let long = bare.len() >= 32 && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        let aws = bare.len() == 20 && bare.starts_with("AKIA");
        if hide_next {
            out.push("‹oculto›".into());
            hide_next = false;
        } else if let Some(a) = AFTER.iter().find(|a| low.starts_with(*a)) {
            if low.len() > a.len() {
                out.push(format!("{}‹oculto›", &w[..a.len()]));
            } else {
                out.push(w.to_owned());
                hide_next = true;
            }
        } else if PREFIXES.iter().any(|p| bare.starts_with(p) && bare.len() > p.len() + 8) || long || aws {
            out.push("‹oculto›".into());
        } else {
            out.push(w.to_owned());
        }
    }
    out.join(" ")
}

/// One line of the record of what the person answered (`--log FILE`): when,
/// who (`--by`), which window and agent, the question, the option chosen, the
/// class the shell gave it (`--class`) and what came of it. Secrets hidden.
fn record(args: &[String], a: &Agent, what: serde_json::Value) {
    let Some(path) = flag(args, "--log") else { return };
    let q = a.question.as_ref();
    let line = serde_json::json!({
        "t": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        "by": flag(args, "--by").unwrap_or("?"),
        "slot": a.slot,
        "agent": a.agent,
        "cwd": a.cwd,
        "class": flag(args, "--class").unwrap_or(""),
        "question": q.map(|q| redact(&q.text)),
        "detail": a.detail.iter().map(|l| redact(l)).collect::<Vec<_>>(),
        "what": what,
    });
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    use std::io::Write;
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut f) => {
            let _ = writeln!(f, "{line}");
        }
        Err(e) => eprintln!("agents · cannot write the record {path}: {e}"),
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|k| args.get(k + 1)).map(String::as_str)
}

const USAGE: &str = "pleamar-wm agents — the session's agents and what they ask, for the person
  agents [--json]                     every window with an agent: number, agent, folder, state
                                      (working, ready, asking) and, asking, the question with its
                                      key and its options by number. --json: one object a line
  agents answer N OPTION --key KEY    answers window N's question with that option's number, only
                                      if the question on its screen still has that key
  agents stop N                       Ctrl+C to window N's agent
  With answer and stop: --log FILE --by WHO --class A|B|C writes a line (JSON, secrets hidden)
  of what was done, refused or failed to FILE";

pub fn run(args: &[String]) -> i32 {
    match go(args) {
        Ok(out) => {
            if !out.is_empty() {
                println!("{out}");
            }
            0
        }
        Err((code, e)) => {
            eprintln!("{e}");
            code
        }
    }
}

fn go(args: &[String]) -> Result<String, (i32, String)> {
    let fail = |e: String| (1, e);
    match args.first().map(String::as_str) {
        None | Some("--json") | Some("list") => {
            let as_json = args.iter().any(|a| a == "--json");
            let all = agents().map_err(fail)?;
            Ok(all.iter().map(|a| if as_json { json(a) } else { human(a) }).collect::<Vec<_>>().join("\n"))
        }
        Some("answer") => {
            let (slot, option) = (args.get(1).ok_or((2, USAGE.to_owned()))?, args.get(2).ok_or((2, USAGE.to_owned()))?);
            let wanted = flag(args, "--key").ok_or((2, "answer N OPTION --key KEY: the key of the question shown, from `agents --json`".to_owned()))?;
            let n: u32 = option.parse().map_err(|_| (2, format!("«{option}»: an option's number")))?;
            let a = find(slot).map_err(fail)?;
            let refuse = |code: i32, why: String| {
                record(args, &a, serde_json::json!({ "answer": n, "result": "refused", "why": why }));
                Err((code, why))
            };
            let Some(q) = a.question.as_ref() else { return refuse(3, format!("window {} asks nothing now: nothing answered", a.slot)) };
            if key(q) != wanted {
                return refuse(3, format!("window {}'s question is not the one shown (key {} now): nothing answered", a.slot, key(q)));
            }
            let Some(text) = options(q).iter().find(|o| o.0 == n).map(|o| o.1.clone()) else { return refuse(2, format!("window {} has no option {n}: nothing answered", a.slot)) };
            match crate::link::choose(a.layer, q, &n.to_string()) {
                Ok(chose) => {
                    record(args, &a, serde_json::json!({ "answer": n, "option": redact(&text), "result": "answered" }));
                    Ok(format!("answered window {}: {chose}", a.slot))
                }
                Err(e) => {
                    record(args, &a, serde_json::json!({ "answer": n, "option": redact(&text), "result": "failed", "why": e }));
                    Err((1, e))
                }
            }
        }
        Some("stop") => {
            let a = find(args.get(1).ok_or((2, USAGE.to_owned()))?).map_err(fail)?;
            let sent = crate::link::keys(a.layer, "ctrl+c");
            record(args, &a, serde_json::json!({ "stop": true, "result": if sent.is_ok() { "sent" } else { "failed" } }));
            sent.map_err(fail)?;
            Ok(format!("Ctrl+C to window {} ({})", a.slot, a.agent))
        }
        _ => Err((2, USAGE.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str, options: &[&str]) -> Question {
        Question { text: text.into(), options: options.iter().enumerate().map(|(k, o)| (k as u32 + 1, (*o).to_owned())).collect(), cursor: 0, yes_no: false }
    }

    #[test]
    fn the_same_question_drawn_again_keeps_its_key_and_another_does_not() {
        let a = q("Bash command rm notes.tmp", &["Yes", "No"]);
        let again = q("Bash command rm notes.tmp  (redrawn)", &["Yes", "No"]);
        let other = q("Bash command rm notes.tmp", &["Yes", "Yes, and don't ask again", "No"]);
        assert_eq!(key(&a), key(&again), "its options make it the same one");
        assert_ne!(key(&a), key(&other));
        assert_eq!(key(&a).len(), 16);
    }

    #[test]
    fn the_detail_is_the_dialog_round_the_question_without_its_box() {
        let screen = "● I'll remove it.

╭──────────────────────────────────────────────────────╮
│ Bash command                                         │
│                                                      │
│   rm notes.tmp                                       │
│   Remove the scratch file                            │
│                                                      │
│ Do you want to proceed?                              │
│ ❯ 1. Yes                                             │
│   2. Yes, and don't ask again for rm commands        │
│   3. No, and tell Claude what to do differently (esc) │
╰──────────────────────────────────────────────────────╯";
        assert_eq!(detail_of(screen), vec!["Bash command", "rm notes.tmp", "Remove the scratch file", "Do you want to proceed?"]);
        assert!(detail_of("no question here").is_empty());
    }

    #[test]
    fn a_secret_never_reaches_the_record() {
        let r = redact("curl -H Authorization: Bearer abc123 https://x --password hunter2 token=s3cr3t ghp_aaaaaaaaaaaaaaaaaaaa AKIAABCDEFGHIJKLMNOP 0123456789abcdef0123456789abcdef01");
        for secret in ["abc123", "hunter2", "s3cr3t", "ghp_aaaa", "AKIAABCD", "0123456789abcdef0123456789abcdef01"] {
            assert!(!r.contains(secret), "{secret} in {r}");
        }
        assert!(r.contains("curl") && r.contains("https://x"));
        assert_eq!(redact("rm notes.tmp"), "rm notes.tmp");
    }

    #[test]
    fn a_yes_no_question_is_answered_by_number_too() {
        let mut yn = q("Overwrite?", &[]);
        yn.yes_no = true;
        assert_eq!(options(&yn), vec![(1, "yes".to_owned()), (2, "no".to_owned())]);
    }

    #[test]
    fn what_a_program_reads_is_one_object_a_line() {
        let a = Agent { slot: 3, layer: 77, agent: "claude".into(), cwd: "/home/a/x".into(), title: "t\n«x»".into(), state: "asking", question: Some(q("Do it?\nreally", &["Yes", "No"])), detail: vec!["rm x".into()] };
        let line = json(&a);
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["slot"], 3);
        assert_eq!(v["question"]["options"][1]["n"], 2);
        assert_eq!(v["question"]["options"][1]["text"], "No");
        assert_eq!(v["question"]["key"], key(a.question.as_ref().unwrap()));
        assert_eq!(v["detail"][0], "rm x");
        let idle = Agent { question: None, state: "ready", ..a };
        assert!(serde_json::from_str::<serde_json::Value>(&json(&idle)).unwrap()["question"].is_null());
    }
}
