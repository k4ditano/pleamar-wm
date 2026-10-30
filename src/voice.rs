//! Answers said aloud on the caravan page. Once a companion's answer ends, it
//! is spoken by the speaker there is here —`pleamar-speak OUT.wav`, the text on
//! its input: a script around any voice (Kokoro, Piper…), never a service the
//! page picks— and kept as audio beside its letter, for the page to play. For
//! the companions the page asked it for (since then), and always for the answer
//! to a voice note: spoken to, it speaks back.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What says a text aloud here.
const SPEAKER: &str = "pleamar-speak";
/// How long an answer is said (what is longer is left to read).
const LONGEST: usize = 1500;
/// Answers older than this are not spoken any more (two hours), nor kept (a week).
const FRESH_MS: u64 = 2 * 3600 * 1000;
const KEPT_MS: u64 = 7 * 24 * 3600 * 1000;

fn dir() -> PathBuf {
    PathBuf::from(format!("{}/voice", crate::keep::dir()))
}

fn prefs() -> PathBuf {
    PathBuf::from(format!("{}/voice.json", crate::keep::dir()))
}

/// Whether there is a speaker here (looked up on the PATH, without running anything).
pub(crate) fn speaker() -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(SPEAKER).is_file()))
}

/// Since when `name`'s answers are spoken, if they are.
fn since(name: &str) -> Option<u64> {
    let v: Value = std::fs::read_to_string(prefs()).ok().and_then(|t| serde_json::from_str(&t).ok())?;
    v[name].as_u64()
}

pub(crate) fn on(name: &str) -> bool {
    since(name).is_some()
}

/// `name`'s answers spoken from now on, or no more.
pub(crate) fn set(name: &str, on: bool) -> Result<String, String> {
    let mut v: Value = std::fs::read_to_string(prefs()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    if on {
        v[name] = json!(crate::mail::now_ms());
    } else if let Some(m) = v.as_object_mut() {
        m.remove(name);
    }
    std::fs::create_dir_all(crate::keep::dir()).map_err(|e| e.to_string())?;
    std::fs::write(prefs(), v.to_string()).map_err(|e| e.to_string())?;
    Ok(if on { "on" } else { "off" }.into())
}

/// Where a letter's answer is kept as audio.
pub(crate) fn audio_of(name: &str, letter: &Value) -> PathBuf {
    let stem: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    dir().join(format!("{stem}-{}-{}.m4a", letter["id"].as_u64().unwrap_or(0), letter["at"].as_u64().unwrap_or(0)))
}

/// One of these audios (what `/caravan/file` may give besides what letters name).
pub(crate) fn is_audio(path: &str) -> bool {
    let d = dir();
    let p = Path::new(path);
    !path.contains("/../") && p.parent() == Some(d.as_path()) && path.ends_with(".m4a")
}

// Being spoken now, and those that could not be (not tried again).
static SPEAKING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
static FAILED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn speaking(path: &Path) -> bool {
    SPEAKING.lock().unwrap().iter().any(|p| p == path)
}

/// The answers of `name` that want a voice and have none yet are given one,
/// apart (one after another: the speaker has one voice).
pub(crate) fn tend(name: &str) {
    if !speaker() {
        return;
    }
    let Some(b) = crate::mail::all().into_iter().find(|b| b["name"].as_str() == Some(name)) else { return };
    let from = since(name);
    let now = crate::mail::now_ms();
    for l in b["done"].as_array().into_iter().flatten() {
        let ended = l["ended"].as_u64().unwrap_or(0);
        let answer = l["answer"].as_str().unwrap_or("").trim();
        let to_voice = l["text"].as_str().is_some_and(|t| t.starts_with("[Voice note"));
        if answer.is_empty() || now.saturating_sub(ended) > FRESH_MS || !(to_voice || from.is_some_and(|s| ended >= s)) {
            continue;
        }
        let out = audio_of(name, l);
        if out.exists() || speaking(&out) || FAILED.lock().unwrap().contains(&out) {
            continue;
        }
        SPEAKING.lock().unwrap().push(out.clone());
        let text = spoken(answer);
        let job = std::thread::spawn(move || {
            let _queue = QUEUE.lock().unwrap();
            if let Err(e) = speak(&text, &out) {
                eprintln!("remote · an answer was not spoken: {e}");
                FAILED.lock().unwrap().push(out.clone());
            }
            SPEAKING.lock().unwrap().retain(|p| p != &out);
        });
        let mut jobs = JOBS.lock().unwrap();
        jobs.retain(|j| !j.is_finished());
        jobs.push(job);
    }
    prune(now);
}

/// Being said, for a command that waits for them before it ends (`link caravan speak`).
static JOBS: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// `link caravan speak NAME`: what of `name` wants a voice is given one, waited
/// for; then what was said, as `talk` gives it.
pub(crate) fn speak_now(name: &str) -> Result<String, String> {
    if !speaker() {
        return Err(format!("no speaker here ({SPEAKER} on the PATH)"));
    }
    tend(name);
    let all: Vec<_> = JOBS.lock().unwrap().drain(..).collect();
    for j in all {
        let _ = j.join();
    }
    Ok(crate::caravan::talk(name))
}

static QUEUE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The audios of more than a week ago, gone.
fn prune(now: u64) {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if now.saturating_sub(LAST.load(std::sync::atomic::Ordering::Relaxed)) < 3600 * 1000 {
        return;
    }
    LAST.store(now, std::sync::atomic::Ordering::Relaxed);
    for e in std::fs::read_dir(dir()).into_iter().flatten().flatten() {
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_millis() as u64 > KEPT_MS);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A text said by the speaker, kept as AAC (what every phone plays).
fn speak(text: &str, out: &Path) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let wav = std::env::temp_dir().join(format!("pleamar-speak-{}-{}.wav", std::process::id(), crate::mail::now_ms()));
    let mut child = std::process::Command::new(SPEAKER)
        .arg(&wav)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child.stdin.take().ok_or("no input")?.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    let said = child.wait_with_output().map_err(|e| e.to_string())?;
    if !said.status.success() {
        let _ = std::fs::remove_file(&wav);
        return Err(String::from_utf8_lossy(&said.stderr).trim().to_owned());
    }
    let new = out.with_extension("new.m4a");
    let made = std::process::Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-i"]).arg(&wav).args(["-c:a", "aac", "-b:a", "48k"]).arg(&new).status();
    let _ = std::fs::remove_file(&wav);
    if !made.is_ok_and(|s| s.success()) {
        let _ = std::fs::remove_file(&new);
        return Err("ffmpeg could not make it AAC".into());
    }
    std::fs::rename(&new, out).map_err(|e| e.to_string())
}

/// An answer as it is said: its words without Markdown's marks; code and
/// tables left to read (said so once), a link as «un enlace», a path as its
/// file's name (without its extension); a line ended so the voice stops at it; and what is longer
/// than [`LONGEST`] left to read too. The voice speaks Spanish.
pub(crate) fn spoken(answer: &str) -> String {
    let (mut lines, mut left, mut fence) = (Vec::new(), false, false);
    for line in answer.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            left = true;
            continue;
        }
        if fence {
            continue;
        }
        if t.starts_with('|') && t.matches('|').count() >= 2 {
            left = true;
            continue;
        }
        if t.is_empty() || (t.len() >= 3 && t.chars().all(|c| matches!(c, '-' | '*' | '_' | ' '))) {
            continue;
        }
        let mut t = t.trim_start_matches('#').trim_start_matches('>').trim_start();
        for mark in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
            if let Some(rest) = t.strip_prefix(mark) {
                t = rest;
                break;
            }
        }
        if let Some(dot) = t.find(['.', ')']).filter(|&d| d > 0 && d <= 3 && t[..d].chars().all(|c| c.is_ascii_digit()) && t[d + 1..].starts_with(' ')) {
            t = &t[dot + 2..];
        }
        let mut s = words(t);
        if s.is_empty() {
            continue;
        }
        if !s.ends_with(['.', '!', '?', ':', ';', '…']) {
            s.push('.');
        }
        lines.push(s);
    }
    let mut out = lines.join("\n");
    if out.chars().count() > LONGEST {
        let cut: String = out.chars().take(LONGEST).collect();
        let end = cut.rfind(['.', '!', '?', '\n']).map_or(cut.len(), |e| e + 1);
        out = format!("{}\nEl resto está escrito en el chat.", cut[..end].trim_end());
    } else if left {
        out.push_str("\nLos detalles están escritos en el chat.");
    }
    if out.trim().is_empty() { "Te he contestado por escrito en el chat.".into() } else { out }
}

/// A line's words: links by their text, a bare one as «un enlace», a path by
/// its file's name, and no marks of emphasis or code.
fn words(t: &str) -> String {
    let mut s = String::new();
    let mut rest = t;
    // [text](url) → text
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|c| open + c) else { break };
        let Some(end) = rest[close..].find(')').map(|e| close + e) else { break };
        s.push_str(&rest[..open]);
        s.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    s.push_str(rest);
    let s: String = s.replace("**", "").replace("__", "").replace("~~", "").chars().filter(|&c| c != '`' && c != '*').collect();
    s.split(' ')
        .map(|w| {
            if w.starts_with("http://") || w.starts_with("https://") {
                "un enlace".to_owned()
            } else if w.starts_with('/') && w.matches('/').count() > 1 {
                let file = w.trim_end_matches(['.', ',', ';', ':']).rsplit('/').next().unwrap_or(w);
                file.rsplit_once('.').filter(|(stem, ext)| !stem.is_empty() && (1..=5).contains(&ext.len())).map_or(file, |(stem, _)| stem).to_owned()
            } else {
                w.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_said_without_its_marks() {
        let said = spoken("## Hecho\n\nHe subido **la rama** y `cargo test` pasa.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- uno\n- dos\n1. tres\n\nMira [la tabla](https://x.es/t) y https://y.es, o /home/abel/informe_final.md\n\n```sh\nrm -rf /\n```");
        assert_eq!(said, "Hecho.\nHe subido la rama y cargo test pasa.\nuno.\ndos.\ntres.\nMira la tabla y un enlace o informe final.\nLos detalles están escritos en el chat.");
    }

    #[test]
    fn a_long_answer_is_said_up_to_a_sentence() {
        let said = spoken(&"Una frase que se repite. ".repeat(100));
        assert!(said.chars().count() < LONGEST + 40);
        assert!(said.ends_with(".\nEl resto está escrito en el chat."));
    }

    #[test]
    fn only_its_own_audios_are_given() {
        let d = dir();
        assert!(is_audio(&format!("{}/prueba-3-1700.m4a", d.display())));
        assert!(!is_audio(&format!("{}/../voice.json", d.display())));
        assert!(!is_audio(&format!("{}/x/y.m4a", d.display())));
        assert!(!is_audio("/etc/passwd"));
    }
}
