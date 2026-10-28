//! What agents' hands (`pleamar-wm agent`, agent.rs) may touch, and what they
//! did. Every click, key and press goes through the compositor, so the
//! person's rules can be kept there, whatever the agent believes it may do.
//!
//! Who: the name of the terminal the agent runs in (`PLEAMAR_NAME`, as `link
//! open --name` gave it), read from the environment of the process at the
//! other end of its socket or of one of its parents; and whether it is at a
//! letter from a routine (mail.rs). The rules are the gate's (`gate.rules`),
//! with the tool `hands` and, as WHAT, the window's program and title
//! (`zen Control horario - Factorial`, lower case), `panel` for a panel, or
//! `open COMMAND` for a program it opens:
//!   - a `deny` that fits refuses it, for anyone;
//!   - an `allow` that fits lets it;
//!   - one that has an `allow` about its hands (its name, `routine`, or `*`)
//!     touches nothing else: naming where it may go is fencing it in;
//!   - one with none is as free as ever.
//!
//! A broken rules file refuses all hands.
//!
//! Each step (a click, a press, a key, a text, a program opened) is written
//! down as it is done —who, which window, what, by which rule— with a small
//! picture of the window for clicks and presses: the review, step by step.

use serde_json::{json, Value};

/// How many steps (and their pictures) are kept.
const KEPT: usize = 400;

fn dir() -> String {
    format!("{}/gate/hands", crate::keep::dir())
}

pub(crate) fn log_file() -> String {
    format!("{}/steps.jsonl", dir())
}

/// The name of the terminal a process runs in: its `PLEAMAR_NAME`, or its
/// parents' (eight up at most). Empty for none.
pub(crate) fn who_of(pid: u32) -> String {
    let mut pid = pid;
    for _ in 0..8 {
        if pid <= 1 {
            break;
        }
        if let Ok(env) = std::fs::read(format!("/proc/{pid}/environ"))
            && let Some(v) = env.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(b"PLEAMAR_NAME="))
        {
            return String::from_utf8_lossy(v).chars().filter(|c| !c.is_control()).take(64).collect();
        }
        pid = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| s.rsplit_once(") ").and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse().ok())).unwrap_or(0);
    }
    String::new()
}

/// What a step touches, as rules read it.
pub(crate) fn what(app: &str, title: &str) -> String {
    format!("{app} {title}").to_lowercase()
}

/// Whether `name` may do that: `Ok(None)` free (no rule about it), `Ok(Some(rule))`
/// allowed by that rule, `Err(why)` refused.
pub(crate) fn allowed(name: &str, what: &str) -> Result<Option<String>, String> {
    let routine = crate::mail::routine_of(name).unwrap_or_default();
    crate::gate::hands(name, &routine, what)
}

/// A step, written down; its picture's file name if it is to have one.
pub(crate) fn record(step: Value, picture: bool) -> Option<String> {
    let _ = std::fs::create_dir_all(dir());
    let at = crate::mail::now_ms();
    let shot = picture.then(|| format!("{at}-{}.jpg", std::process::id() % 1000));
    let mut step = step;
    step["at"] = json!(at);
    if let Some(s) = &shot {
        step["picture"] = json!(s);
    }
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(log_file()) {
        let _ = writeln!(f, "{step}");
    }
    trim();
    shot.map(|s| format!("{}/{s}", dir()))
}

/// The record cut to the last `KEPT` steps (every so often), and the pictures
/// of the steps cut, gone with them: only files of this folder named as
/// `record` names them.
fn trim() {
    let Ok(text) = std::fs::read_to_string(log_file()) else { return };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < KEPT + KEPT / 4 {
        return;
    }
    let (old, kept) = lines.split_at(lines.len() - KEPT);
    for l in old {
        if let Some(p) = serde_json::from_str::<Value>(l).ok().and_then(|v| v["picture"].as_str().map(str::to_owned))
            && p.ends_with(".jpg")
            && !p.contains('/')
        {
            let _ = std::fs::remove_file(format!("{}/{p}", dir()));
        }
    }
    let tmp = format!("{}.new", log_file());
    if std::fs::write(&tmp, kept.join("\n") + "\n").is_ok() {
        let _ = std::fs::rename(tmp, log_file());
    }
}

/// A picture made small (480 wide at most) and kept as JPEG, readable only by
/// its owner.
pub(crate) fn save_picture(path: &str, picture: &image::RgbaImage) {
    let (w, h) = picture.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let scale = (480.0 / w as f32).min(1.0);
    let small = image::imageops::resize(picture, ((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1), image::imageops::FilterType::Triangle);
    let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(path) {
        let mut out = std::io::BufWriter::new(f);
        let _ = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70).encode_image(&rgb);
    }
}

/// `pleamar-wm agent steps [N] [--json]`: the last steps, the last first.
pub(crate) fn steps(n: usize, json_out: bool) -> String {
    let text = std::fs::read_to_string(log_file()).unwrap_or_default();
    let mut v: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v.truncate(n);
    if json_out {
        for s in &mut v {
            if let Some(p) = s["picture"].as_str().map(|p| format!("{}/{p}", dir())) {
                s["picture"] = json!(p);
            }
        }
        return Value::Array(v).to_string();
    }
    if v.is_empty() {
        return "no step of any agent's hands yet".into();
    }
    v.iter()
        .map(|s| format!("{} {} in «{}»: {}{}", s["who"].as_str().filter(|w| !w.is_empty()).unwrap_or("an agent"), s["did"].as_str().unwrap_or(""), s["title"].as_str().unwrap_or(""), s["what"].as_str().unwrap_or(""), if s["refused"].is_string() { format!(" — refused: {}", s["refused"].as_str().unwrap_or("")) } else { String::new() }))
        .collect::<Vec<_>>()
        .join("\n")
}
