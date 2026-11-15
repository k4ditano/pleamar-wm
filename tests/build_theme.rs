//! Build's look (docs/build-theme.md): the scene loads with its own theme,
//! with the example one, and not with one that misses a colour (which is why
//! Build keeps its own when a desktop's does not load).

use std::path::{Path, PathBuf};
use std::process::Command;

fn scene_with(name: &str, theme: &str) -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("build");
    let d = std::env::temp_dir().join(format!("pleamar-wm-build-theme-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("lang")).unwrap();
    for f in ["build.plm", "panel.plm", "lang/es.plm"] {
        std::fs::copy(here.join(f), d.join(f)).unwrap();
    }
    std::fs::write(d.join("theme.plm"), theme).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pleamar-wm"), d.join("pleamar")).unwrap();
    d
}

fn check(d: &Path) -> bool {
    Command::new(d.join("pleamar")).arg("--check").arg(d.join("build.plm")).output().unwrap().status.success()
}

#[test]
fn its_own_and_the_example_load() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("build");
    let own = std::fs::read_to_string(here.join("theme.plm")).unwrap();
    let example = std::fs::read_to_string(here.join("theme-example/theme.plm")).unwrap();
    assert!(check(&scene_with("own", &own)), "Build's own theme.plm does not load");
    assert!(check(&scene_with("example", &example)), "theme-example/theme.plm does not load");
}

#[test]
fn a_theme_missing_a_colour_does_not() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("build");
    let own = std::fs::read_to_string(here.join("theme.plm")).unwrap();
    let broken: String = own.lines().filter(|l| !l.trim_start().starts_with("let mint")).collect::<Vec<_>>().join("\n");
    assert!(!check(&scene_with("broken", &broken)));
}
