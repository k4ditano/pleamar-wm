//! pleamar-wm — pleamar with a Wayland compositor inside: a scene that says
//! `windows win max 6` holds other programs' windows, and where they go is the
//! scene's. Everything else —the command line, the scenes, the reloads— is
//! pleamar's own.

mod a11y;
mod a11y_tree;
mod a11y_act;
mod marks;
mod a11y_wait;
mod agent_cli;
mod agent_mcp;
mod build;
mod remote;
mod remote_rtc;
mod remote_files;
mod config;
mod cursor;
mod cursor_theme;
mod desktop;
mod nest;
mod phone;
mod portal;
mod headless;
mod layers;
mod keys;
mod probe;
mod route;
mod screen;
mod session;
mod term;
mod link;
mod agents;
mod mcp;
mod mcp_serve;
mod keep;
mod mail;
mod gate;
mod output;

fn main() {
    // Called as the user's shell (the session's `SHELL`): the layer between
    // a terminal and the real shell, or that shell at once (see term.rs).
    term::as_shell();
    // Called `pleamar` (Build's link to it): pleamar itself, the one inside,
    // for what Build starts and checks where no pleamar is installed.
    if std::env::args_os().next().is_some_and(|a| std::path::Path::new(&a).file_name().is_some_and(|n| n == "pleamar")) {
        pleamar::run();
        return;
    }
    pleamar::provide_windows(|max, to_render| {
        let tx = nest::start(max, to_render)?;
        Some(Box::new(move |m| {
            let _ = tx.send(m);
        }))
    });
    // `pleamar-wm session scene.plm [options]`: a session of its own, from a
    // TTY, without a compositor underneath. Otherwise, pleamar as ever.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `pleamar-wm hyprctl monitors|activewindow|locked`: the session's desktop, said
    // the way Hyprland says it, for what used to ask Hyprland (Marea's
    // screenshots) and now runs here.
    if args.first().map(String::as_str) == Some("hyprctl") {
        std::process::exit(hyprctl(args.get(1).map(String::as_str).unwrap_or("")));
    }
    // `pleamar-wm output NAME at X,Y|vrr on|off`: a monitor changed while the
    // session runs, answered with what was done or why not (see output.rs).
    if args.first().map(String::as_str) == Some("output") {
        std::process::exit(output::cli(&args[1..]));
    }
    // `pleamar-wm agent …`: the agent's hands from a shell (see agent_cli.rs).
    if args.first().map(String::as_str) == Some("agent") {
        std::process::exit(agent_cli::run(&args[1..]));
    }
    // `pleamar-wm link …`: an agent in a terminal and the windows wired to its own (see link.rs).
    if args.first().map(String::as_str) == Some("link") {
        std::process::exit(link::run(&args[1..]));
    }
    // `pleamar-wm gate …`: what agents ask to do, decided by the person's rules or by the person (see gate.rs).
    if args.first().map(String::as_str) == Some("gate") {
        std::process::exit(gate::run(&args[1..]));
    }
    // `pleamar-wm agents …`: the session's agents and what they ask, for the person's shell (see agents.rs).
    if args.first().map(String::as_str) == Some("agents") {
        std::process::exit(agents::run(&args[1..]));
    }
    // `pleamar-wm term …`: the terminals' layers, by hand (see term.rs).
    if args.first().map(String::as_str) == Some("term") {
        std::process::exit(term::cli(&args[1..]));
    }
    // `pleamar-wm remote …`: this desktop from a browser elsewhere (see remote.rs).
    if args.first().map(String::as_str) == Some("remote") {
        std::process::exit(remote::run(&args[1..]));
    }
    // `pleamar-wm idle hold|release|status`: keep the monitors from going dark
    // (`idle off-after`) while the session runs, and let them go dark again.
    if args.first().map(String::as_str) == Some("idle") {
        let path = config::idle_hold_path();
        match args.get(1).map(String::as_str) {
            Some("hold") => {
                if let Err(e) = std::fs::write(&path, b"") {
                    eprintln!("idle · {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
            Some("release") => {
                if let Err(e) = std::fs::remove_file(&path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("idle · {}: {e}", path.display());
                        std::process::exit(1);
                    }
                }
            }
            Some("status") => println!("{}", if path.exists() { "hold" } else { "release" }),
            _ => {
                eprintln!("idle · `pleamar-wm idle hold|release|status`");
                std::process::exit(1);
            }
        }
        return;
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
    // `pleamar-wm config`: the session's configuration as it is understood;
    // `config layouts`, how each monitor starts, a line each; `config set
    // layout MONITOR NAME`, that written down (the control centre uses both).
    if args.first().map(String::as_str) == Some("config") {
        match args.get(1).map(String::as_str) {
            Some("layouts") => {
                for (m, l) in &config::get().layouts {
                    println!("{m} {l}");
                }
            }
            Some("set") if args.get(2).map(String::as_str) == Some("layout") && args.len() == 5 => {
                if let Err(e) = config::write_layout(&args[3], &args[4]) {
                    eprintln!("config · {e}");
                    std::process::exit(1);
                }
            }
            // `config set dock hide on`: one of the dock's settings (`dock-style` in session.conf).
            Some("set") if args.get(2).map(String::as_str) == Some("dock") && args.len() == 5 => {
                if let Err(e) = config::write_dock_style(&args[3], &args[4]) {
                    eprintln!("config · {e}");
                    std::process::exit(1);
                }
            }
            // `config set agent on`, `a11y off`, `agent-pace fast`: the control centre's Agent page.
            Some("set") if matches!(args.get(2).map(String::as_str), Some("agent" | "a11y" | "agent-pace")) && args.len() == 4 => {
                if let Err(e) = config::write_setting(&args[2], &args[3]) {
                    eprintln!("config · {e}");
                    std::process::exit(1);
                }
            }
            Some(other) => {
                eprintln!("config · '{other}': `pleamar-wm config`, `config layouts`, `config set agent|a11y on|off`, `config set agent-pace default|fast|instant`, `config set layout MONITOR|* NAME` ({}), or `config set dock hide|magnify|dots|into|size|side VALUE`", config::LAYOUTS.join(", "));
                std::process::exit(1);
            }
            None => println!("{:#?}", config::get()),
        }
        return;
    }
    // `pleamar-wm build` (Super+B): build mode, to build your desktop by asking.
    if args.first().map(String::as_str) == Some("build") {
        build::run(&args[1..]);
        return;
    }
    // `pleamar-wm control`: the control centre, a window to choose how each
    // monitor lays its windows out and to see what each key does.
    if args.first().map(String::as_str) == Some("control") {
        let dir = std::env::var("XDG_CACHE_HOME").ok().filter(|v| !v.is_empty()).or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.cache"))).unwrap_or_else(|| "/tmp".into()) + "/pleamar-wm/control";
        let _ = std::fs::create_dir_all(&dir);
        for (name, text) in [("control.plm", include_str!("../control/control.plm")), ("control.luau", include_str!("../control/control.luau"))] {
            if let Err(e) = std::fs::write(format!("{dir}/{name}"), text) {
                eprintln!("control · could not write {dir}/{name}: {e}");
                std::process::exit(1);
            }
        }
        // Its Build page signs in with Build's own engine.
        build::prepare();
        pleamar::run_with(vec!["--scene".to_owned(), format!("{dir}/control.plm"), "--no-hud".to_owned()]);
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
        let scene = if args.first().is_some_and(|a| !a.starts_with("--")) { args.remove(0) } else { default_scene(true) };
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
        // Nor does it take the real session's portal on the same bus.
        portal::HEADLESS.store(true, std::sync::atomic::Ordering::Relaxed);
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
        let scene = if args.first().is_some_and(|a| !a.starts_with("--")) { args.remove(0) } else { default_scene(false) };
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
        // `true` or `false`, as Hyprland says it: a script that locks and then
        // suspends waits for `true`. No line is no answer, never `false`.
        "locked" => match text.lines().find_map(|l| l.strip_prefix("locked ")) {
            Some("1") => {
                println!("true");
                0
            }
            Some("0") => {
                println!("false");
                0
            }
            _ => {
                eprintln!("pleamar-wm's session does not say whether it is locked");
                1
            }
        },
        _ => {
            eprintln!("pleamar-wm hyprctl monitors | activewindow | locked");
            1
        }
    }
}

/// What pleamar-wm adds to `pleamar --report`: which window manager runs, on
/// which monitors, and what the session's configuration says of them.
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
const DEFAULT_SCENE: &str = include_str!("../examples/session.plm");
/// And the shaders it reads, relative to it.
const DEFAULT_SHADERS: &[(&str, &str)] = &[
    ("shaders/rain.wgsl", include_str!("../examples/shaders/rain.wgsl")),
    ("shaders/snow.wgsl", include_str!("../examples/shaders/snow.wgsl")),
    ("shaders/tide.wgsl", include_str!("../examples/shaders/tide.wgsl")),
];

/// The scene and its shaders into `dir`. `refresh`: the runtime copy, kept the
/// same as the one inside; otherwise someone's folder, where nothing is overwritten.
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
/// or the one that comes with it, written where it can be read. A screenless
/// one writes it in a folder of its own: the session's copy is reloaded when
/// it changes, and an older build would put the desktop back in time.
fn default_scene(headless: bool) -> String {
    if let Some(own) = config::user_dir().map(|d| format!("{d}/wm/session.plm")).filter(|p| std::path::Path::new(p).exists()) {
        return own;
    }
    // (Named session.plm: a scene answers by its file's name, `--say session`.)
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let dir = if headless { format!("{runtime}/pleamar-wm-headless") } else { format!("{runtime}/pleamar-wm") };
    let _ = std::fs::create_dir_all(&dir);
    write_scene(&dir, true);
    format!("{dir}/session.plm")
}

/// Where the session's agent socket is, for the display it serves.
pub fn agent_socket_path(display: &str) -> String {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    format!("{dir}/pleamar-{display}/cua-inject.sock")
}
