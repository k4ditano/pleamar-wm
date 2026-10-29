//! What happened to each named terminal, with its hour: its agent started,
//! left, failed to start, was restarted; each letter posted, handed, ended.
//! One line of JSON each (`{"at": ms, "kind": …, "text": …}`) in
//! `$XDG_STATE_HOME/pleamar-wm/journal/NAME.jsonl`, the newest last, cut to
//! the last few hundred. `link journal NAME [N] [--json]` reads it; the caravan
//! page shows it beside the conversation.

use serde_json::{Value, json};
use std::io::Write;

/// How many lines a journal keeps (cut when it grows past twice this).
const KEPT: usize = 400;

fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::keep::dir()).join("journal")
}

fn file(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let stem: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    (!stem.trim_matches('-').is_empty()).then(|| dir.join(format!("{stem}.jsonl")))
}

/// One more line in NAME's journal. Never fails loudly: a journal is for
/// looking back, nothing waits on it.
pub(crate) fn note(name: &str, kind: &str, text: &str) {
    note_in(&dir(), name, kind, text);
}

fn note_in(dir: &std::path::Path, name: &str, kind: &str, text: &str) {
    let Some(path) = file(dir, name) else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(400).collect();
    let line = json!({ "at": crate::mail::now_ms(), "kind": kind, "text": text }).to_string();
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&path) {
        let _ = writeln!(f, "{line}");
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > (KEPT as u64) * 2 * 300) {
        let all = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<&str> = all.lines().collect();
        if lines.len() > KEPT {
            let tmp = path.with_extension("jsonl.new");
            if std::fs::write(&tmp, lines[lines.len() - KEPT..].join("\n") + "\n").is_ok() {
                let _ = std::fs::rename(tmp, &path);
            }
        }
    }
}

/// Its last `n` lines, the oldest first.
pub(crate) fn last(name: &str, n: usize) -> Vec<Value> {
    last_in(&dir(), name, n)
}

fn last_in(dir: &std::path::Path, name: &str, n: usize) -> Vec<Value> {
    let Some(path) = file(dir, name) else { return Vec::new() };
    let all = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<Value> = all.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

/// An hour as the journal says it: «29-10 12:05:31», local time.
fn hour(ms: u64) -> String {
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return ms.to_string();
    }
    format!("{:02}-{:02} {:02}:{:02}:{:02}", tm.tm_mday, tm.tm_mon + 1, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// `link journal NAME [N] [--json]`.
pub(crate) fn cmd(rest: &[String]) -> Result<String, String> {
    let name = rest.first().filter(|n| !n.starts_with("--")).ok_or("link journal NAME [N] [--json]")?;
    let n = rest.iter().skip(1).find_map(|a| a.parse::<usize>().ok()).unwrap_or(30);
    Ok(said(name, &last(name, n), rest.iter().any(|a| a == "--json")))
}

fn said(name: &str, lines: &[Value], as_json: bool) -> String {
    if as_json {
        return Value::Array(lines.to_vec()).to_string();
    }
    if lines.is_empty() {
        return format!("nothing yet in «{name}»'s journal");
    }
    lines
        .iter()
        .map(|l| format!("{} · {} · {}", hour(l["at"].as_u64().unwrap_or(0)), l["kind"].as_str().unwrap_or(""), l["text"].as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_journal_keeps_its_last_lines_in_order() {
        let dir = std::env::temp_dir().join(format!("pwm-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for k in 0..(KEPT * 2 + 50) {
            note_in(&dir, "Pro hard", "letter", &format!("#{k}   from  general"));
        }
        let got = last_in(&dir, "Pro hard", 3);
        assert_eq!(got.len(), 3);
        assert_eq!(got[2]["text"], format!("#{} from general", KEPT * 2 + 49));
        assert_eq!(got[2]["kind"], "letter");
        assert!(last_in(&dir, "Pro hard", 10_000).len() <= KEPT * 2 + 50);
        assert_eq!(said("Pro hard", &got[1..], false).lines().count(), 2);
        assert!(said("Pro hard", &[], false).starts_with("nothing yet"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
