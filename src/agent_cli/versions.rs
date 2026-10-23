//! The versions of each window an agent last read: a tree's (the window
//! manager's), a terminal's mark, a scene's (kept here, with what it was).

use super::*;

/// Where this agent keeps the last version it got of each window:
/// `$XDG_RUNTIME_DIR/pleamar-agent/NAME` (its name hex), a line `WINDOW V
/// SECONDS` each, the window as it was said (`4521`, `7002:7044`) and when.
pub(super) fn versions_file() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").ok()?).join("pleamar-agent").join(hex(&agent_name())))
}

/// The last version read of a window, and when (seconds since 1970).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Last {
    pub(super) v: u64,
    pub(super) at: u64,
}

impl Last {
    /// `since 12, read at 14:02`.
    pub(super) fn said(&self) -> String {
        format!("since {}, read at {}", self.v, clock(self.at))
    }
}

/// A line of the file, read: an older one has no time.
fn line_of<'a>(l: &'a str, w: &str) -> Option<Last> {
    let mut f = l.split(' ');
    (f.next()? == w).then_some(())?;
    Some(Last { v: f.next()?.parse().ok()?, at: f.next().and_then(|t| t.parse().ok()).unwrap_or(0) })
}

pub(super) fn last_version(w: &str) -> Option<Last> {
    std::fs::read_to_string(versions_file()?).ok()?.lines().find_map(|l| line_of(l, w))
}

/// The version an answer of `A`'s gave, kept for that window.
pub(super) fn keep_version(w: &str, reply: &str) {
    let head = reply.strip_prefix("full | ").unwrap_or(reply).split(" | ").next().unwrap_or_default();
    if let Some(v) = head.strip_prefix("nodes ").or_else(|| head.strip_prefix("changes ")).and_then(|v| v.parse().ok()) {
        keep(w, v);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `HH:MM` of that moment, here; `--:--` for none.
fn clock(at: u64) -> String {
    let t = at as libc::time_t;
    // SAFETY: localtime_r into a value of its own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if at == 0 || unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return "--:--".to_owned();
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// Held while it is read and written: two at once lost a window's line.
fn locked(path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path.parent()?);
    let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(path.with_extension("lock")).ok()?;
    // SAFETY: flock on a file this holds open.
    unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX) };
    Some(f)
}

/// Written aside and moved over it: a reader never sees half of it. The owner's alone.
fn write(path: &std::path::Path, text: &str) {
    use std::os::unix::fs::OpenOptionsExt;
    let aside = path.with_extension(std::process::id().to_string());
    let written = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&aside).and_then(|mut f| f.write_all(text.as_bytes()));
    if written.and_then(|_| std::fs::rename(&aside, path)).is_err() {
        let _ = std::fs::remove_file(&aside);
    }
}

/// That version kept for that window, read now.
pub(super) fn keep(w: &str, v: u64) {
    let Some(path) = versions_file() else { return };
    let _lock = locked(&path);
    let mut kept: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().filter(|l| l.split(' ').next() != Some(w)).map(str::to_owned).collect();
    kept.push(format!("{w} {v} {}", now()));
    write(&path, &(kept.join("\n") + "\n"));
}

/// Where a scene window's last read is kept: beside the versions, by window.
fn scene_file(w: &str) -> Option<std::path::PathBuf> {
    let f = versions_file()?;
    Some(f.with_file_name(format!("{}.scene.{w}", f.file_name()?.to_str()?)))
}

/// A scene window's last read as kept: its version, the scene's name and its records.
pub(super) fn kept_scene(w: &str) -> Option<(u64, String, Vec<Rec>)> {
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(scene_file(w)?).ok()?).ok()?;
    let recs = j["recs"].as_array()?.iter().filter_map(Rec::from_kept).collect();
    Some((j["version"].as_u64()?, j["scene"].as_str()?.to_owned(), recs))
}

/// A scene window read now, kept as its next version: that version.
pub(super) fn keep_scene(w: &str, scene: &str, recs: &[Rec]) -> u64 {
    let v = last_version(w).map_or(0, |l| l.v) + 1;
    if let Some(path) = scene_file(w) {
        let _lock = locked(&path);
        let all: Vec<serde_json::Value> = recs.iter().map(Rec::kept).collect();
        write(&path, &serde_json::json!({ "version": v, "scene": scene, "recs": all }).to_string());
    }
    keep(w, v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_a_window_its_version_and_when() {
        assert_eq!(line_of("4521 12 1700000000", "4521"), Some(Last { v: 12, at: 1700000000 }));
        assert_eq!(line_of("4521 12", "4521"), Some(Last { v: 12, at: 0 }));
        assert_eq!(line_of("45210 12 1", "4521"), None);
        assert_eq!(line_of("7002:7044 900 5", "7002:7044"), Some(Last { v: 900, at: 5 }));
        assert_eq!(Last { v: 3, at: 0 }.said(), "since 3, read at --:--");
        assert!(Last { v: 3, at: 1700000000 }.said().starts_with("since 3, read at "));
    }
}
