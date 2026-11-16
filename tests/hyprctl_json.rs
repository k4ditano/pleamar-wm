//! `pleamar-wm hyprctl [-j] monitors|activewindow|locked` against a desktop file
//! written here (`PLEAMAR_SOCKETS/desktop`, as a session writes it): no session
//! is started. The JSON is Hyprland's: a turned monitor's size unturned with its
//! turn in `transform`, `-j` before or after the word, `{}` with no window, and a
//! session from before the turn was written (eight fields) read as upright.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn desktop(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pleamar-wm-hyprctl-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("desktop"), text).unwrap();
    dir
}

fn hyprctl(dir: &PathBuf, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pleamar-wm"))
        .arg("hyprctl")
        .args(args)
        .env("PLEAMAR_SOCKETS", dir)
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
}

const TWO: &str = "monitor DP-1 1440 2560 0 0 164834 0 1 1\n\
                   monitor DP-2 2560 1440 1440 733 164834 1 1.5 0\n\
                   locked 0\n\
                   window 2726 795 1250 1311 kitty\tA title with spaces\n";

#[test]
fn monitors_json_either_order() {
    let dir = desktop("monitors", TWO);
    for args in [&["-j", "monitors"][..], &["monitors", "-j"][..]] {
        let (code, out) = hyprctl(&dir, args);
        assert_eq!(code, 0);
        let list: Value = serde_json::from_str(&out).expect("JSON");
        let list = list.as_array().unwrap();
        assert_eq!(list.len(), 2);
        // Turned a quarter: Hyprland's size is the mode's, unturned.
        assert_eq!(list[0]["name"], "DP-1");
        assert_eq!(list[0]["id"], 0);
        assert_eq!((list[0]["width"].as_u64(), list[0]["height"].as_u64()), (Some(2560), Some(1440)));
        assert_eq!(list[0]["transform"], 1);
        assert_eq!(list[0]["focused"], false);
        assert_eq!(list[1]["name"], "DP-2");
        assert_eq!((list[1]["x"].as_i64(), list[1]["y"].as_i64()), (Some(1440), Some(733)));
        assert_eq!(list[1]["scale"].as_f64(), Some(1.5));
        assert_eq!(list[1]["focused"], true);
        assert!((list[1]["refreshRate"].as_f64().unwrap() - 164.834).abs() < 1e-9);
    }
    // Without -j, the text as before: the size as it stands.
    let (_, out) = hyprctl(&dir, &["monitors"]);
    assert!(out.contains("Monitor DP-1 (ID 0):\n\t1440x2560@164.83400 at 0x0"), "{out}");
}

#[test]
fn activewindow_json_and_none() {
    let dir = desktop("active", TWO);
    let (code, out) = hyprctl(&dir, &["-j", "activewindow"]);
    assert_eq!(code, 0);
    let w: Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(w["at"], serde_json::json!([2726, 795]));
    assert_eq!(w["size"], serde_json::json!([1250, 1311]));
    assert_eq!(w["class"], "kitty");
    assert_eq!(w["title"], "A title with spaces");

    let dir = desktop("nowindow", "monitor DP-2 2560 1440 0 0 60000 1 1 0\nlocked 1\n");
    let (_, out) = hyprctl(&dir, &["activewindow", "-j"]);
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), serde_json::json!({}));
    let (_, out) = hyprctl(&dir, &["-j", "locked"]);
    assert_eq!(out.trim(), "true");
}

#[test]
fn session_from_before_the_turn_is_upright() {
    let dir = desktop("old", "monitor HDMI-A-1 1920 1080 0 0 60000 1 1\nlocked 0\n");
    let (code, out) = hyprctl(&dir, &["monitors", "-j"]);
    assert_eq!(code, 0);
    let list: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(list[0]["transform"], 0);
    assert_eq!((list[0]["width"].as_u64(), list[0]["height"].as_u64()), (Some(1920), Some(1080)));
}
