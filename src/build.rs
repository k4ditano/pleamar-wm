//! `pleamar-wm build` (Super+B): build mode, to build your desktop by asking.
//!
//! The first time it starts the build scene (build/: a pleamar scene, its
//! logic and its AI engine), with the mode on; while it runs, it tells it to
//! turn the mode off or on again. Its files go inside this program and are
//! written where they run (~/.local/share/pleamar-wm/build), with the
//! engine's packages, which its logic fetches there the first time.
//!
//! Its look can be a desktop's own (docs/build-theme.md): a theme.plm in
//! `$PLEAMAR_CONFIG/build-theme/` is put over its own each time it starts.

const FILES: &[(&str, &str)] = &[
    ("build.plm", include_str!("../build/build.plm")),
    ("panel.plm", include_str!("../build/panel.plm")),
    ("build.luau", include_str!("../build/build.luau")),
    ("logic/engine.luau", include_str!("../build/logic/engine.luau")),
    ("logic/creations.luau", include_str!("../build/logic/creations.luau")),
    ("logic/draw.luau", include_str!("../build/logic/draw.luau")),
    ("lang/es.plm", include_str!("../build/lang/es.plm")),
    ("agent/worker.mjs", include_str!("../build/agent/worker.mjs")),
    ("agent/tools.mjs", include_str!("../build/agent/tools.mjs")),
    ("agent/package.json", include_str!("../build/agent/package.json")),
    ("agent/package-lock.json", include_str!("../build/agent/package-lock.json")),
    ("agent/sandbox", include_str!("../build/agent/sandbox")),
];

/// Build's own look: its colours (a desktop puts its own over them).
const THEME: &str = include_str!("../build/theme.plm");

/// Where the build scene listens, as pleamar names it: `PLEAMAR_SOCKETS`
/// (what pleamar-wm gives the programs it starts) or `$XDG_RUNTIME_DIR/pleamar`.
fn socket() -> std::path::PathBuf {
    let dir = match std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()) {
        Some(d) => std::path::PathBuf::from(d),
        None => std::path::Path::new(&std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join("pleamar"),
    };
    dir.join("build.sock")
}

/// Where its files are written: next to the engine's packages, which stay.
pub fn dir() -> String {
    let data = std::env::var("XDG_DATA_HOME").ok().filter(|v| !v.is_empty()).or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/share"))).unwrap_or_else(|| "/tmp".into());
    format!("{data}/pleamar-wm/build")
}

/// Written only where they changed: the engine's lock, rewritten, would look
/// newer than its packages and fetch them again.
fn materialize(dir: &str) -> Result<(), String> {
    for (name, text) in FILES {
        let path = std::path::Path::new(dir).join(name);
        if std::fs::read_to_string(&path).is_ok_and(|t| t == *text) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        if name.ends_with("sandbox") {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
    let own = crate::config::user_dir().map(|c| std::path::Path::new(&c).join("build-theme"));
    theme(std::path::Path::new(dir), own.as_deref(), loads)
}

/// Written only if it is not already so.
fn put(path: &std::path::Path, text: &[u8]) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The look (docs/build-theme.md). A desktop's own, from `own`
/// (`$PLEAMAR_CONFIG/build-theme/`): its theme.plm over Build's, its
/// figures (figures/*.svg, *.png) and its fonts (*.ttf, *.otf, *.ttc) next to
/// the scene. Build's own when there is none, or when the scene does not load
/// with it (`loads`): a theme that misses a colour must not leave no Build.
fn theme(dir: &std::path::Path, own: Option<&std::path::Path>, loads: impl Fn(&std::path::Path) -> Result<(), String>) -> Result<(), String> {
    let path = dir.join("theme.plm");
    let mine = own.and_then(|d| std::fs::read(d.join("theme.plm")).ok());
    if let Some(d) = own {
        let small = |p: &std::path::Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= 32 << 20);
        let has = |p: &std::path::Path, ends: &[&str]| p.extension().and_then(|e| e.to_str()).is_some_and(|e| ends.contains(&e.to_ascii_lowercase().as_str()));
        for (from, to, ends) in [(d.to_path_buf(), dir.to_path_buf(), &["ttf", "otf", "ttc"][..]), (d.join("figures"), dir.join("figures"), &["svg", "png"][..])] {
            for entry in std::fs::read_dir(&from).into_iter().flatten().flatten() {
                let p = entry.path();
                if small(&p) && has(&p, ends) {
                    if let Ok(bytes) = std::fs::read(&p) {
                        put(&to.join(entry.file_name()), &bytes)?;
                    }
                }
            }
        }
    }
    let Some(mine) = mine else { return put(&path, THEME.as_bytes()) };
    put(&path, &mine)?;
    if let Err(why) = loads(dir) {
        eprintln!("build · {}/theme.plm does not load, so Build looks as its own: {why}", own.map(|d| d.display().to_string()).unwrap_or_default());
        put(&path, THEME.as_bytes())?;
    }
    Ok(())
}

/// Whether the scene loads as it is (`pleamar --check`, this program by
/// its other name): its first lines of complaint if it does not.
fn loads(dir: &std::path::Path) -> Result<(), String> {
    let Some(bin) = link_pleamar() else { return Ok(()) };
    let out = std::process::Command::new(format!("{bin}/pleamar"))
        .arg("--check")
        .arg(dir.join("build.plm"))
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr).lines().chain(String::from_utf8_lossy(&out.stdout).lines()).take(3).collect::<Vec<_>>().join(" · ");
    Err(said)
}

/// `pleamar`, for what Build starts, checks and reads (the creations, their
/// check, the docs): a link to this program, which is pleamar when called
/// so, in its folder's bin/. Made again when this program has moved.
pub fn link_pleamar() -> Option<String> {
    let bin = format!("{}/bin", dir());
    let link = format!("{bin}/pleamar");
    let me = crate::link::own_exe();
    if std::fs::read_link(&link).ok().as_deref() != Some(me.as_path()) {
        std::fs::create_dir_all(&bin).ok()?;
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&me, &link).ok()?;
    }
    Some(bin)
}

/// Its files where they run, for the control centre too (it signs in with
/// the same engine).
pub fn prepare() {
    if let Err(e) = materialize(&dir()) {
        eprintln!("build · {e}");
    }
}

pub fn run(args: &[String]) {
    // Running: told. `pleamar-wm build` toggles; `on` and `off` say which;
    // `ask` is on, asking first what to build (as `start = panel` does).
    let order = match args.first().map(String::as_str) {
        Some("off") => "emit leave",
        Some("on") => "emit start",
        Some("ask") => "emit ask",
        _ => "emit toggle",
    };
    if let Ok(mut s) = std::os::unix::net::UnixStream::connect(socket()) {
        use std::io::Write;
        let _ = writeln!(s, "{order}");
        return;
    }
    if order == "emit leave" {
        return;
    }
    let dir = dir();
    if let Err(e) = materialize(&dir) {
        eprintln!("build · {e}");
        std::process::exit(1);
    }
    if let Err(e) = std::env::set_current_dir(&dir) {
        eprintln!("build · {dir}: {e}");
        std::process::exit(1);
    }
    // `pleamar` first on its PATH: the one inside this program, whether
    // another is installed or not.
    let path = match (link_pleamar(), std::env::var("PATH")) {
        (Some(bin), Ok(p)) => Some(format!("{bin}:{p}")),
        (Some(bin), Err(_)) => Some(bin),
        _ => None,
    };
    // SAFETY: one thread still: nothing else reads the environment yet.
    unsafe {
        std::env::set_var("BUILD_ON", if order == "emit ask" { "ask" } else { "1" });
        if let Some(p) = path {
            std::env::set_var("PATH", p);
        }
    }
    pleamar::run_with(vec!["--scene".to_owned(), format!("{dir}/build.plm"), "--no-hud".to_owned()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pleamar-wm-build-theme-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn its_own_look_without_a_theme() {
        let d = scratch("none");
        theme(&d.join("build"), Some(&d.join("absent")), |_| panic!("nothing to check")).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("build/theme.plm")).unwrap(), THEME);
        theme(&d.join("build"), None, |_| panic!("nothing to check")).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("build/theme.plm")).unwrap(), THEME);
    }

    #[test]
    fn a_desktops_theme_with_its_figures_and_fonts() {
        let d = scratch("own");
        let own = d.join("build-theme");
        std::fs::create_dir_all(own.join("figures")).unwrap();
        let mine = THEME.replace("#7fe0c4", "#0a84ff");
        std::fs::write(own.join("theme.plm"), &mine).unwrap();
        std::fs::write(own.join("figures/pen.svg"), "<svg/>").unwrap();
        std::fs::write(own.join("Face.ttf"), "font").unwrap();
        std::fs::write(own.join("notes.txt"), "not taken").unwrap();
        theme(&d.join("build"), Some(&own), |_| Ok(())).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("build/theme.plm")).unwrap(), mine);
        assert!(d.join("build/figures/pen.svg").is_file());
        assert!(d.join("build/Face.ttf").is_file());
        assert!(!d.join("build/notes.txt").exists());
        // Gone: its own again.
        std::fs::remove_file(own.join("theme.plm")).unwrap();
        theme(&d.join("build"), Some(&own), |_| Ok(())).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("build/theme.plm")).unwrap(), THEME);
    }

    #[test]
    fn a_theme_that_does_not_load_leaves_its_own() {
        let d = scratch("broken");
        let own = d.join("build-theme");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("theme.plm"), "library BuildTheme { let ink = #ffffff }").unwrap();
        theme(&d.join("build"), Some(&own), |_| Err("mint: no such name".into())).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("build/theme.plm")).unwrap(), THEME);
    }
}
