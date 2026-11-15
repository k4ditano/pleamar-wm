//! pleamar #156: the keys of a `screens: each` surface are the copy's whose monitor has them.
//! A screenless session with two monitors (`pleamar-wm headless`), and as a program in it
//! `tests/keys-each.plm` (pleamar-wm is pleamar when called so): one copy per monitor, each
//! with a field and an `on key F5`, and before them a named panel that follows a text naming
//! the second monitor. A click in the second monitor's field and some keys must reach copy 1
//! and not copy 0; then the same on the first. And the panel, written before the copies, must
//! not take them to its monitor (they used to all go to HEADLESS-2).
//! Without a card to paint with, the session says so and the test passes without trying.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn keep_env(c: &mut Command) {
    c.env_clear();
    for keep in ["PATH", "LD_LIBRARY_PATH", "VK_ICD_FILENAMES", "VK_DRIVER_FILES", "LIBGL_ALWAYS_SOFTWARE", "WGPU_BACKEND", "WGPU_ADAPTER_NAME"] {
        if let Some(v) = std::env::var_os(keep) {
            c.env(keep, v);
        }
    }
}

/// The program's own socket folder, once its scene answers there.
fn scene_sockets(run: &Path, limit: Duration) -> Option<PathBuf> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Ok(dirs) = std::fs::read_dir(run) {
            for d in dirs.flatten() {
                if d.path().join("keys-each.sock").exists() {
                    return Some(d.path());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

#[test]
fn the_keys_of_a_copy_per_monitor_go_to_the_copy_whose_monitor_has_them() {
    let dir = std::env::temp_dir().join(format!("ke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (home, run, bin) = (dir.join("home"), dir.join("run"), dir.join("bin"));
    for d in [home.join(".config"), home.join(".cache"), home.join(".local/share"), home.join(".local/state"), run.clone(), bin.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    // pleamar-wm called `pleamar` is pleamar: the program the session starts.
    let pleamar = bin.join("pleamar");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pleamar-wm"), &pleamar).unwrap();
    let scene = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/keys-each.plm");
    std::fs::write(dir.join("autostart"), format!("{} --scene {} --no-hud\n", pleamar.display(), scene.display())).unwrap();
    let log = dir.join("session.log");
    let fifo = run.join("hand");
    let mut c = Command::new(env!("CARGO_BIN_EXE_pleamar-wm"));
    keep_env(&mut c);
    c.env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_RUNTIME_DIR", &run)
        .env("PLEAMAR_SOCKETS", run.join("pleamar"))
        .env("PLEAMAR_NO_SKILL_REFRESH", "1")
        .env("PLEAMAR_WM_AUTOSTART", dir.join("autostart"))
        .env("PLEAMAR_HEADLESS_SCREENS", "2")
        .env("PLEAMAR_HEADLESS_AT", "999999")
        .env("PLEAMAR_HEADLESS_PNG", dir.join("shot.png"))
        .env("PLEAMAR_HEADLESS_INPUT_FIFO", &fifo)
        .arg("headless")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/virtual-pointer.plm"))
        .args(["--seconds", "90"])
        .stdout(std::fs::File::create(&log).unwrap())
        // (The program's own lines —its surfaces, on which monitor— come on its stderr.)
        .stderr(std::fs::OpenOptions::new().append(true).open(&log).unwrap());
    let mut wm = c.spawn().expect("pleamar-wm runs");
    let result = std::panic::catch_unwind(|| {
        let Some(sockets) = scene_sockets(&run, Duration::from_secs(60)) else {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            assert!(text.contains("no graphics driver"), "the program's scene never answered: {text}");
            eprintln!("no card to paint with: not tried");
            return;
        };
        let say = |what: &str| -> String {
            let out = Command::new(&pleamar).env("XDG_RUNTIME_DIR", &run).env("PLEAMAR_SOCKETS", &sockets).args(["--say", "keys-each", what]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        // Until the session and the program have painted (once each, whatever their surfaces).
        let start = Instant::now();
        while std::fs::read_to_string(&log).unwrap_or_default().matches("first frame").count() < 2 {
            assert!(start.elapsed() < Duration::from_secs(60), "the copies never painted: {}", std::fs::read_to_string(&log).unwrap_or_default());
            std::thread::sleep(Duration::from_millis(200));
        }
        std::thread::sleep(Duration::from_secs(2));
        let mut hand = std::fs::OpenOptions::new().write(true).open(&fifo).expect("the session's hand");
        let mut step = |s: &str, wait: u64| {
            writeln!(hand, "{s}").unwrap();
            hand.flush().unwrap();
            std::thread::sleep(Duration::from_millis(wait));
        };
        // The second monitor (x 1920 on): a click in its field, two letters and F5.
        step("2200,115", 400);
        step("down", 100);
        step("up", 1500);
        for k in ["key:x:45", "key:y:21", "key:F5:63"] {
            step(k, 400);
        }
        let wait_for = |what: &str, want: &str| {
            let start = Instant::now();
            while say(what) != want && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(200));
            }
        };
        wait_for("get q.1", "xy");
        let second = (say("get hits.0"), say("get hits.1"), say("get q.0"), say("get q.1"));
        // The first monitor: the same there.
        step("300,115", 400);
        step("down", 100);
        step("up", 1500);
        for k in ["key:z:44", "key:F5:63"] {
            step(k, 400);
        }
        wait_for("get q.0", "z");
        let first = (say("get hits.0"), say("get hits.1"), say("get q.0"), say("get q.1"));
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        assert_eq!(second, ("0".into(), "1".into(), "".into(), "xy".into()), "typed on monitor 2 (hits.0, hits.1, q.0, q.1)");
        assert_eq!(first, ("1".into(), "1".into(), "z".into(), "xy".into()), "then on monitor 1 (hits.0, hits.1, q.0, q.1)");
        // The panel, written before the copies, stays a panel on its monitor and the copies on theirs.
        assert!(text.contains("render · surface 0 on HEADLESS-1 · 1920×1080"), "copy 0 is on the first monitor: {text}");
        assert!(text.contains("render · surface 1 on HEADLESS-2 · 1920×1080 · scale 1 · 60 Hz · copy 1"), "copy 1 on the second: {text}");
        assert!(text.lines().any(|l| l.starts_with("render · surface") && l.contains("on HEADLESS-2 · 300×120") && l.ends_with("panel")), "the panel on the second: {text}");
        assert!(!text.contains("wayland · surface 0 to"), "the copies were moved: {text}");
    });
    let _ = wm.kill();
    let _ = wm.wait();
    // The program it started, by its socket's folder (only ours).
    let _ = Command::new("pkill").arg("-f").arg(format!("{} --scene", dir.join("bin").display())).status();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
