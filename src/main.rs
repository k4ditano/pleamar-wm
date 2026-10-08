//! pleamar-wm — pleamar with a Wayland compositor inside: a scene that says
//! `windows win max 6` holds other programs' windows, and where they go is the
//! scene's. Everything else —the command line, the scenes, the reloads— is
//! pleamar's own.

#[cfg(target_os = "linux")]
mod agent_cli;
#[cfg(target_os = "linux")]
mod remote;
#[cfg(target_os = "linux")]
mod remote_rtc;
mod window_rules;
#[cfg(target_os = "linux")]
mod config;
#[cfg(target_os = "linux")]
mod cursor;
#[cfg(target_os = "linux")]
mod desktop;
#[cfg(target_os = "linux")]
mod nest;
#[cfg(target_os = "linux")]
mod phone;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod headless;
#[cfg(target_os = "linux")]
mod layers;
#[cfg(target_os = "linux")]
mod keys;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod route;
#[cfg(target_os = "linux")]
mod screen;
#[cfg(target_os = "linux")]
mod session;

#[cfg(target_os = "linux")]
fn main() {
    pleamar::provide_windows(|max, to_render| {
        let tx = nest::start(max, to_render)?;
        Some(Box::new(move |m| {
            let _ = tx.send(m);
        }))
    });
    // `pleamar-wm session scene.plm [options]`: a session of its own, from a
    // TTY, without a compositor underneath. Otherwise, pleamar as ever.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `pleamar-wm hyprctl monitors|activewindow`: the session's desktop, said
    // the way Hyprland says it, for what used to ask Hyprland (Marea's
    // screenshots) and now runs here.
    if args.first().map(String::as_str) == Some("hyprctl") {
        std::process::exit(hyprctl(args.get(1).map(String::as_str).unwrap_or("")));
    }
    // `pleamar-wm agent …`: the agent's hands from a shell (see agent_cli.rs).
    if args.first().map(String::as_str) == Some("agent") {
        std::process::exit(agent_cli::run(&args[1..]));
    }
    // `pleamar-wm remote …`: this desktop from a browser elsewhere (see remote.rs).
    if args.first().map(String::as_str) == Some("remote") {
        std::process::exit(remote::run(&args[1..]));
    }
    // `pleamar-wm init`: ~/.config/pleamar with a starting point, never over
    // what is already there.
    if args.first().map(String::as_str) == Some("init") {
        std::process::exit(init());
    }
    // `pleamar-wm scene`: the window manager that comes with it, to start one's own from.
    // With a folder (`pleamar-wm scene ~/.config/pleamar/wm`), written there with its shaders.
    if args.first().map(String::as_str) == Some("scene") {
        match args.get(1) {
            Some(dir) => std::process::exit(write_scene(dir, false)),
            None => print!("{DEFAULT_SCENE}"),
        }
        return;
    }
    // `pleamar-wm keys`: the bindings that come with it, to copy from.
    if args.first().map(String::as_str) == Some("keys") {
        print!("{}", keys::DEFAULTS);
        return;
    }
    // `pleamar-wm config`: the session's configuration as it is understood.
    if args.first().map(String::as_str) == Some("config") {
        println!("{:#?}", config::get());
        return;
    }
    // `pleamar-wm report`: measures the session and whatever runs in it (Marea…)
    // for a while and writes it down, with its monitors, to send when it stutters.
    if args.first().map(String::as_str) == Some("report") {
        std::process::exit(pleamar::report(args[1..].to_vec(), Some(report_section())));
    }
    if args.first().map(String::as_str) == Some("probe") {
        if let Err(e) = probe::run() {
            eprintln!("probe · {e}");
            std::process::exit(1);
        }
        return;
    }
    // `pleamar-wm headless scene.plm [options]`: the session's painting with no screen, to check it.
    if args.first().map(String::as_str) == Some("headless") {
        args.remove(0);
        let scene = if args.first().is_some_and(|a| !a.starts_with("--")) { args.remove(0) } else { default_scene() };
        nest::set_scene_name(&scene);
        layers::expect_monitors();
        // Its scene listens apart: under the same name as a real session's
        // (`session`), it took that one's place, and Marea there could no longer
        // reach her window manager until the next login.
        let own = std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| {
            let dir = format!("{}/pleamar-headless-{}", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()), std::process::id());
            // SAFETY: before any thread is started.
            unsafe { std::env::set_var("PLEAMAR_SOCKETS", &dir) };
            dir
        });
        // A test desktop never tells the user's dbus and systemd where the desktop
        // is: run from a terminal of a real session it inherits that session's
        // `PLEAMAR_WM_EXPORT`, and its portals ended up pointed at a display
        // that was gone seconds later.
        // SAFETY: before any thread is started.
        unsafe { std::env::remove_var("PLEAMAR_WM_EXPORT") };
        let _ = std::fs::create_dir_all(&own);
        println!("headless · its scene listens in {own}");
        pleamar::provide_before_quit(Box::new(move || {
            layers::stop_all();
            nest::stop_launched();
            if own.contains("pleamar-headless-") {
                let _ = std::fs::remove_dir_all(&own);
            }
        }));
        pleamar::provide_platform(Box::new(headless::Headless));
        let mut options = vec!["--scene".to_owned(), scene, "--no-hud".to_owned()];
        options.extend(args);
        pleamar::run_with(options);
        return;
    }
    if args.first().map(String::as_str) == Some("session") {
        args.remove(0);
        // Rebuilt under it, it would start again and every window would go with it.
        pleamar::stay_on_update();
        let scene = if args.first().is_some_and(|a| !a.starts_with("--")) { args.remove(0) } else { default_scene() };
        nest::set_scene_name(&scene);
        layers::expect_monitors();
        pleamar::provide_before_quit(Box::new(|| {
            layers::stop_all();
            nest::stop_launched();
            nest::stop_clients();
        }));
        pleamar::provide_platform(Box::new(session::Session));
        let mut options = vec!["--scene".to_owned(), scene, "--no-hud".to_owned()];
        options.extend(args);
        pleamar::run_with(options);
    } else {
        pleamar::run();
    }
}

#[cfg(target_os = "linux")]
fn hyprctl(what: &str) -> i32 {
    let Some(Ok(text)) = nest::desktop_file().map(std::fs::read_to_string) else {
        eprintln!("pleamar-wm's session is not running");
        return 1;
    };
    match what {
        "monitors" => {
            for (id, line) in text.lines().filter_map(|l| l.strip_prefix("monitor ")).enumerate() {
                let f: Vec<&str> = line.split(' ').collect();
                let [name, w, h, x, y, mhz, focused, scale] = f[..] else { continue };
                let hz = mhz.parse::<f64>().unwrap_or(60_000.0) / 1000.0;
                let scale = scale.parse::<f64>().unwrap_or(1.0);
                println!("Monitor {name} (ID {id}):\n\t{w}x{h}@{hz:.5} at {x}x{y}\n\tscale: {scale:.2}\n\tfocused: {}\n", if focused == "1" { "yes" } else { "no" });
            }
            0
        }
        "activewindow" => {
            match text.lines().find_map(|l| l.strip_prefix("window ")) {
                Some(line) => {
                    let f: Vec<&str> = line.splitn(5, ' ').collect();
                    let [x, y, w, h, names] = f[..] else { return 1 };
                    let (app, title) = names.split_once('\t').unwrap_or((names, ""));
                    println!("Window 0 -> {title}:\n\tat: {x},{y}\n\tsize: {w},{h}\n\tclass: {app}\n\ttitle: {title}\n");
                }
                None => println!("Invalid"),
            }
            0
        }
        _ => {
            eprintln!("pleamar-wm hyprctl monitors | activewindow");
            1
        }
    }
}

/// What pleamar-wm adds to `pleamar --report`: which window manager runs, on
/// which monitors, and what the session's configuration says of them.
#[cfg(target_os = "linux")]
fn report_section() -> String {
    let mut out = format!("## pleamar-wm\n\n- **Version:** {}\n", env!("CARGO_PKG_VERSION"));
    let own = config::user_dir().is_some_and(|d| std::path::Path::new(&format!("{d}/wm/session.plm")).exists());
    out.push_str(&format!("- **Window manager scene:** {}\n", if own { "the user's own (~/.config/pleamar/wm/session.plm)" } else { "the one that comes with it" }));
    match nest::desktop_file().map(std::fs::read_to_string) {
        Some(Ok(text)) => {
            for line in text.lines().filter_map(|l| l.strip_prefix("monitor ")) {
                let f: Vec<&str> = line.split(' ').collect();
                let [name, w, h, x, y, mhz, _, scale] = f[..] else { continue };
                let hz = mhz.parse::<f64>().unwrap_or(0.0) / 1000.0;
                out.push_str(&format!("- **Monitor {name}:** {w}×{h} @ {hz:.2} Hz at {x},{y} · scale {scale}\n"));
            }
        }
        _ => out.push_str("- **Session:** not running here (measured from outside it?)\n"),
    }
    let c = config::get();
    for m in &c.monitors {
        out.push_str(&format!("- **session.conf:** {m:?}\n"));
    }
    out.push_str(&format!("- **Window rules:** {} · pinned to the dock: {}\n", c.windows.len(), c.dock.len()));
    out
}

/// The user's folder, to start from: what the session reads, commented, and
/// the folders for their own window manager and shells.
#[cfg(target_os = "linux")]
fn init() -> i32 {
    let Some(dir) = config::user_dir() else {
        eprintln!("init · no HOME");
        return 1;
    };
    let files: [(&str, &str); 3] = [
        ("session.conf", include_str!("../config.example")),
        (
            "keys.conf",
            "# Your key bindings. `defaults` keeps pleamar-wm's (see them with\n# `pleamar-wm keys`); change or add below. Actions and syntax are explained there.\ndefaults\n\n# bind Super+b launch zen-browser\n# bind Super+q minimize\n# unbind Super+t\n",
        ),
        ("autostart", include_str!("../autostart")),
    ];
    for sub in ["wm", "shells"] {
        let _ = std::fs::create_dir_all(format!("{dir}/{sub}"));
    }
    for (name, text) in files {
        let path = format!("{dir}/{name}");
        if std::path::Path::new(&path).exists() {
            println!("init · {path} is already there: left as it is");
            continue;
        }
        match std::fs::write(&path, text) {
            Ok(()) => println!("init · {path}"),
            Err(e) => eprintln!("init · {path}: {e}"),
        }
    }
    println!("init · {dir}/wm: your own window manager (session.plm), if you want one");
    println!("init · {dir}/shells: your scenes (bars, widgets, apps); start them from autostart");
    0
}

/// The window manager that comes with pleamar-wm, inside it: an installed
/// pleamar-wm has no source folder next to it.
#[cfg(target_os = "linux")]
const DEFAULT_SCENE: &str = include_str!("../examples/session.plm");
/// And the shaders it reads, relative to it.
#[cfg(target_os = "linux")]
const DEFAULT_SHADERS: &[(&str, &str)] = &[
    ("shaders/rain.wgsl", include_str!("../examples/shaders/rain.wgsl")),
    ("shaders/snow.wgsl", include_str!("../examples/shaders/snow.wgsl")),
];

/// The scene and its shaders into `dir`. `refresh`: the runtime copy, kept the
/// same as the one inside; otherwise someone's folder, where nothing is overwritten.
#[cfg(target_os = "linux")]
fn write_scene(dir: &str, refresh: bool) -> i32 {
    let mut failed = 0;
    for (name, text) in std::iter::once(("session.plm", DEFAULT_SCENE)).chain(DEFAULT_SHADERS.iter().copied()) {
        let path = std::path::Path::new(dir).join(name);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let same = std::fs::read_to_string(&path).ok().as_deref() == Some(text);
        if same || (!refresh && path.exists()) {
            if !refresh {
                println!("scene · {} is already there: left as it is", path.display());
            }
            continue;
        }
        match std::fs::write(&path, text) {
            Ok(()) if !refresh => println!("scene · {}", path.display()),
            Ok(()) => {}
            Err(e) => {
                eprintln!("scene · {}: {e}", path.display());
                failed = 1;
            }
        }
    }
    failed
}

/// The scene when none is said: the user's own (~/.config/pleamar/wm/session.plm),
/// or the one that comes with it, written where it can be read.
#[cfg(target_os = "linux")]
fn default_scene() -> String {
    if let Some(own) = config::user_dir().map(|d| format!("{d}/wm/session.plm")).filter(|p| std::path::Path::new(p).exists()) {
        return own;
    }
    // (Named session.plm: a scene answers by its file's name, `--say session`.)
    let dir = format!("{}/pleamar-wm", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()));
    let _ = std::fs::create_dir_all(&dir);
    write_scene(&dir, true);
    format!("{dir}/session.plm")
}

/// Where the session's agent socket is, for the display it serves.
#[cfg(target_os = "linux")]
pub fn agent_socket_path(display: &str) -> String {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    format!("{dir}/pleamar-{display}/cua-inject.sock")
}

#[cfg(any(target_os = "windows", test))]
mod layout;
#[cfg(target_os = "windows")]
mod windows_backend;

#[cfg(target_os = "windows")]
fn main() {
    std::process::exit(windows_backend::run(std::env::args().skip(1).collect()));
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("pleamar-wm supports Linux and the experimental Windows desktop backend");
    std::process::exit(1);
}
