//! How the session is set up: `~/.config/pleamar/session.conf` (or
//! `PLEAMAR_WM_CONFIG`; the old `~/.config/pleamar-wm/config` still counts),
//! one thing a line, the way `autostart` is.
//!
//! Everything of the user's is in one folder, `~/.config/pleamar/`, the one
//! that goes into their dotfiles: `session.conf`, `keys.conf`, `autostart`,
//! `wm/session.plm` (their own window manager), their `shells/`. What is not
//! there is taken from what comes with pleamar-wm.
//!
//! ```text
//! # which mode, where, and whether it is used at all
//! monitor DP-3 1920x1080@165 at 0,0
//! monitor HDMI-A-1 preferred at 1920,0
//! monitor HDMI-A-2 off
//! monitor DP-1 highest scale 1.5 vrr
//! monitor HDMI-A-2 preferred at 1920,0 transform 90
//! keyboard layout es variant "" options caps:escape repeat 25 delay 400
//! pointer accel flat speed 0
//! touchpad tap on natural on dwt on speed 0.2
//! idle off-after 600
//! window app=pavucontrol float size 820x560
//! window app=discord workspace 3 monitor HDMI-A-1
//! window title="Picture-in-Picture" float
//! window app=org.keepassxc.KeePassXC private
//! ```
//!
//! What it does not say is looked for in Hyprland's own configuration
//! (`~/.config/hypr`: its `monitor` lines, `kb_layout`, `accel_profile`,
//! `sensitivity`), so that a desktop set up there comes out the same here.

use std::sync::OnceLock;

/// A monitor as it is asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonitorRule {
    pub name: String,
    pub mode: ModeWish,
    /// Where it goes on the desktop; `None`, after the others, left to right.
    pub at: Option<(i32, i32)>,
    pub scale: Option<f64>,
    /// How far it is turned, in quarter turns: 1 is 90°, a monitor standing
    /// on its side. The same numbers, and the same way round, as Hyprland's
    /// (and wlroots') `transform`: what is right there is right here.
    pub transform: u8,
    pub vrr: bool,
    pub off: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ModeWish {
    /// The one the monitor says it prefers.
    #[default]
    Preferred,
    /// Its biggest, at the most refresh that size has.
    Highest,
    /// That size, at the refresh closest to this one (0: the most it has).
    Exact(u32, u32, f64),
}

#[derive(Clone, Debug, Default)]
pub struct Keyboard {
    pub layout: Option<String>,
    pub variant: Option<String>,
    pub options: Option<String>,
    /// Keys a second once repeating, and after how many ms it starts.
    pub rate: Option<u32>,
    pub delay: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Pointing {
    /// `flat` or `adaptive`.
    pub accel: Option<String>,
    /// From −1 (slowest) to 1.
    pub speed: Option<f64>,
    pub natural: Option<bool>,
    pub tap: Option<bool>,
    pub dwt: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub monitors: Vec<MonitorRule>,
    /// The programs pinned to the dock of free monitors, in order: `dock
    /// kitty zen-browser org.telegram.desktop`.
    pub dock: Vec<String>,
    pub keyboard: Keyboard,
    pub pointer: Pointing,
    pub touchpad: Pointing,
    /// Seconds without input before the monitors go dark (none: never).
    pub off_after: Option<u64>,
    /// What locks the session when it comes back from the phone to the desk
    /// (`phone lock COMMAND`; `none`, nothing). By default, Marea's lock.
    pub phone_lock: Option<String>,
    /// What some windows do when they open: `window app=… [float] [size WxH]
    /// [monitor N|NAME] [workspace N] [private]`.
    pub windows: Vec<WindowRule>,
    /// `agent on`: a computer-use agent (Cua Driver) may use the desktop
    /// with a pointer and a keyboard of its own (see `agent.rs`).
    pub agent: bool,
}

pub use crate::window_rules::{WindowRule, ForWindow};
use crate::window_rules::words;

impl Config {
    pub fn for_window(&self, app: &str, title: &str) -> ForWindow {
        crate::window_rules::for_window(&self.windows, app, title)
    }
}

impl Config {
    pub fn monitor(&self, name: &str) -> Option<&MonitorRule> {
        self.monitors.iter().rev().find(|m| m.name == name)
    }

    /// Keys a second and the wait before repeating: what is said, or 25 and 400.
    pub fn repeat(&self) -> (u32, u32) {
        (self.keyboard.rate.unwrap_or(25).max(1), self.keyboard.delay.unwrap_or(400))
    }
}

/// Writes the `dock` line of session.conf: where the first one was (the
/// others gone), or at the end. Nothing else of the file changes.
pub fn write_dock(pins: &[String]) {
    let Some(dir) = user_dir() else { return };
    let path = format!("{dir}/session.conf");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let line = format!("dock {}", pins.join(" "));
    let mut out: Vec<String> = Vec::new();
    let mut put = false;
    for l in text.lines() {
        if l.trim_start().starts_with("dock ") || l.trim() == "dock" {
            if !put {
                if !pins.is_empty() {
                    out.push(line.clone());
                }
                put = true;
            }
            continue;
        }
        out.push(l.to_owned());
    }
    if !put && !pins.is_empty() {
        out.push(line);
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Err(e) = std::fs::write(&path, out.join("\n") + "\n") {
        eprintln!("config · could not write {path}: {e}");
    }
}

/// The session's configuration, read once.
pub fn get() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(read)
}

/// Where the user's configuration lives: `PLEAMAR_CONFIG`, or
/// `$XDG_CONFIG_HOME/pleamar` (`~/.config/pleamar`).
pub fn user_dir() -> Option<String> {
    if let Some(d) = std::env::var("PLEAMAR_CONFIG").ok().filter(|v| !v.is_empty()) {
        return Some(d);
    }
    Some(format!("{}/pleamar", config_home()?))
}

fn config_home() -> Option<String> {
    std::env::var("XDG_CONFIG_HOME").ok().filter(|v| !v.is_empty()).or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.config")))
}

/// One of the user's files: in `~/.config/pleamar/`, or where it used to be
/// (`~/.config/pleamar-wm/…`) if only that one exists. `None` if neither does.
pub fn user_file(name: &str, old: &str) -> Option<String> {
    let new = user_dir().map(|d| format!("{d}/{name}"));
    let old = config_home().map(|h| format!("{h}/pleamar-wm/{old}"));
    [new, old].into_iter().flatten().find(|p| std::path::Path::new(p).exists())
}

fn path() -> Option<String> {
    std::env::var("PLEAMAR_WM_CONFIG").ok().or_else(|| user_file("session.conf", "config"))
}

fn read() -> Config {
    let mut c = Config::default();
    let file = path();
    if let Some(text) = file.as_deref().and_then(|f| std::fs::read_to_string(f).ok()) {
        println!("config · {}", file.as_deref().unwrap_or(""));
        parse(&text, &mut c);
    }
    hyprland(&mut c);
    c
}

fn on(v: Option<&String>) -> Option<bool> {
    match v.map(String::as_str) {
        Some("on" | "yes" | "true" | "1") => Some(true),
        Some("off" | "no" | "false" | "0") => Some(false),
        _ => None,
    }
}

pub fn parse(text: &str, c: &mut Config) {
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let w = words(line);
        let rest = &w[1..];
        let pairs = |f: &mut dyn FnMut(&str, Option<&String>)| {
            let mut k = 0;
            while k < rest.len() {
                f(&rest[k], rest.get(k + 1));
                k += 2;
            }
        };
        match w[0].as_str() {
            "monitor" => match monitor_line(rest) {
                Some(m) => c.monitors.push(m),
                None => eprintln!("config · line {}: a monitor line is `monitor NAME [WxH@HZ|preferred|highest] [at X,Y] [scale S] [transform 90|180|270] [vrr] [off]`", n + 1),
            },
            "keyboard" => pairs(&mut |k, v| {
                let v = v.cloned();
                match k {
                    "layout" => c.keyboard.layout = v,
                    "variant" => c.keyboard.variant = v,
                    "options" => c.keyboard.options = v,
                    "repeat" | "rate" => c.keyboard.rate = v.and_then(|v| v.parse().ok()),
                    "delay" => c.keyboard.delay = v.and_then(|v| v.parse().ok()),
                    _ => eprintln!("config · line {}: the keyboard has no '{k}'", n + 1),
                }
            }),
            what @ ("pointer" | "touchpad") => {
                let mut p = if what == "pointer" { c.pointer.clone() } else { c.touchpad.clone() };
                pairs(&mut |k, v| match k {
                    "accel" => p.accel = v.cloned(),
                    "speed" => p.speed = v.and_then(|v| v.parse().ok()),
                    "natural" => p.natural = on(v),
                    "tap" => p.tap = on(v),
                    "dwt" => p.dwt = on(v),
                    _ => eprintln!("config · line {}: the {what} has no '{k}'", n + 1),
                });
                if what == "pointer" {
                    c.pointer = p;
                } else {
                    c.touchpad = p;
                }
            }
            "dock" => c.dock.extend(rest.iter().cloned()),
            "window" => match crate::window_rules::parse(rest) {
                Some(rule) => c.windows.push(rule),
                None => eprintln!("config · line {}: a window rule is `window app=NAME|title=TEXT [float] [size WxH] [monitor N|NAME] [workspace N] [private]`", n + 1),
            },
            "agent" => c.agent = on(rest.first()).unwrap_or_else(|| {
                eprintln!("config · line {}: `agent on` or `agent off`", n + 1);
                false
            }),
            "idle" => pairs(&mut |k, v| match k {
                "off-after" => c.off_after = v.and_then(|v| v.parse().ok()).filter(|s| *s > 0),
                _ => eprintln!("config · line {}: idle has no '{k}'", n + 1),
            }),
            "phone" => match rest.split_first() {
                Some((w, cmd)) if w == "lock" && !cmd.is_empty() => c.phone_lock = Some(cmd.join(" ")),
                _ => eprintln!("config · line {}: `phone lock COMMAND` (or `phone lock none`)", n + 1),
            },
            other => eprintln!("config · line {}: '{other}' is not something it knows (monitor, keyboard, pointer, touchpad, idle, agent, phone)", n + 1),
        }
    }
}

fn monitor_line(w: &[String]) -> Option<MonitorRule> {
    let mut m = MonitorRule { name: w.first()?.clone(), ..Default::default() };
    let mut k = 1;
    while k < w.len() {
        match w[k].as_str() {
            "off" | "disable" => m.off = true,
            "vrr" => m.vrr = true,
            "preferred" => m.mode = ModeWish::Preferred,
            "highest" | "highrr" | "highres" => m.mode = ModeWish::Highest,
            "at" => {
                m.at = w.get(k + 1).and_then(|p| position(p));
                k += 1;
            }
            "scale" => {
                m.scale = w.get(k + 1).and_then(|s| s.parse().ok()).filter(|s: &f64| *s > 0.0);
                k += 1;
            }
            // `transform 90` (or `rotate 90`): in degrees; Hyprland's 0–3 are
            // taken too (`transform 1`).
            "transform" | "rotate" => {
                m.transform = match w.get(k + 1).map(String::as_str) {
                    Some("90" | "1") => 1,
                    Some("180" | "2") => 2,
                    Some("270" | "3") => 3,
                    Some("0" | "normal") => 0,
                    _ => {
                        eprintln!("config · {}: a monitor turns `transform 90`, `180` or `270`", m.name);
                        0
                    }
                };
                k += 1;
            }
            mode => m.mode = mode_wish(mode)?,
        }
        k += 1;
    }
    Some(m)
}

/// `1920x1080@165`, `1920x1080@164.99Hz`, `1920x1080`.
fn mode_wish(s: &str) -> Option<ModeWish> {
    let (size, hz) = s.split_once('@').unwrap_or((s, "0"));
    let (w, h) = size.split_once('x')?;
    Some(ModeWish::Exact(w.parse().ok()?, h.parse().ok()?, hz.trim_end_matches("Hz").trim_end_matches("hz").parse().ok()?))
}

/// `1920,0` or Hyprland's `1920x0`.
fn position(s: &str) -> Option<(i32, i32)> {
    let (x, y) = s.split_once(',').or_else(|| s.split_once('x'))?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// What Hyprland's configuration says and this one does not: its monitors
/// (by name, the last line for each wins), the layout and the pointer.
fn hyprland(c: &mut Config) {
    let Some(home) = std::env::var("HOME").ok() else { return };
    let mut files = Vec::new();
    collect(&std::path::PathBuf::from(format!("{home}/.config/hypr")), &mut files, 0);
    // The file that says the monitors last wins; one generated for them
    // (`…displays…`) is loaded at the end.
    files.sort_by_key(|f| (f.to_string_lossy().contains("display"), f.clone()));
    let mut found: Vec<MonitorRule> = Vec::new();
    let mut layout = None;
    let mut accel = None;
    let mut speed = None;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("--") || t.starts_with('#') {
                continue;
            }
            if let Some(m) = hypr_monitor(t) {
                found.retain(|x| x.name != m.name);
                found.push(m);
            }
            if let Some(v) = setting(t, "kb_layout") {
                layout = Some(v);
            }
            if let Some(v) = setting(t, "accel_profile") {
                accel = Some(v);
            }
            if let Some(v) = setting(t, "sensitivity").and_then(|v| v.parse().ok()) {
                speed = Some(v);
            }
        }
    }
    for m in found {
        if c.monitor(&m.name).is_none() {
            println!("config · {} as Hyprland has it: {:?}{}", m.name, m.mode, m.at.map_or(String::new(), |(x, y)| format!(" at {x},{y}")));
            c.monitors.insert(0, m);
        }
    }
    if c.keyboard.layout.is_none() {
        c.keyboard.layout = layout;
    }
    for p in [&mut c.pointer, &mut c.touchpad] {
        if p.accel.is_none() {
            p.accel = accel.clone();
        }
        if p.speed.is_none() {
            p.speed = speed;
        }
    }
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 2 {
            collect(&p, out, depth + 1);
        } else if p.extension().is_some_and(|x| x == "lua" || x == "conf") {
            out.push(p);
        }
    }
}

/// `kb_layout = "es",` (Lua) or `kb_layout = es` (hyprland.conf).
fn setting(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
    let v = rest.trim().trim_end_matches(',').trim().trim_matches('"').trim();
    (!v.is_empty()).then(|| v.to_owned())
}

/// `hl.monitor({ output = "DP-3", mode = "1920x1080@165.00Hz", position = "0x0", scale = 1 })`
/// or `monitor = DP-3, 1920x1080@165, 0x0, 1`. Only by a real name.
fn hypr_monitor(line: &str) -> Option<MonitorRule> {
    if let Some(body) = line.strip_prefix("hl.monitor(") {
        let field = |k: &str| {
            let at = body.find(&format!("{k} "))?;
            let rest = body[at + k.len()..].trim_start().strip_prefix('=')?.trim_start();
            let end = rest.find([',', '}']).unwrap_or(rest.len());
            Some(rest[..end].trim().to_owned())
        };
        let name = field("output")?;
        if !name.starts_with('"') {
            return None;
        }
        let unq = |s: String| s.trim_matches('"').to_owned();
        let mut w = vec![unq(name)];
        if let Some(mode) = field("mode").map(unq) {
            w.push(if mode == "disable" { "off".into() } else { mode });
        }
        if let Some(p) = field("position").map(unq).filter(|p| p != "auto") {
            w.push("at".into());
            w.push(p);
        }
        if let Some(s) = field("scale").map(unq).filter(|s| s != "auto") {
            w.push("scale".into());
            w.push(s);
        }
        // (4–7 are mirrored as well: only the turn is taken.)
        if let Some(t) = field("transform").map(unq).and_then(|t| t.parse::<u8>().ok()) {
            w.push("transform".into());
            w.push((t % 4).to_string());
        }
        return monitor_line(&w);
    }
    let rest = line.strip_prefix("monitor")?.trim_start().strip_prefix('=')?;
    let f: Vec<String> = rest.split(',').map(|s| s.trim().to_owned()).collect();
    let name = f.first().filter(|n| !n.is_empty())?.clone();
    let mut w = vec![name];
    match f.get(1).map(String::as_str) {
        Some("disable") => w.push("off".into()),
        Some(m) if !m.is_empty() => w.push(m.into()),
        _ => {}
    }
    if let Some(p) = f.get(2).filter(|p| *p != "auto" && !p.is_empty()) {
        w.push("at".into());
        w.push(p.clone());
    }
    if let Some(s) = f.get(3).filter(|s| *s != "auto" && !s.is_empty()) {
        w.push("scale".into());
        w.push(s.clone());
    }
    // `monitor = DP-1, 1920x1080, 0x0, 1, transform, 1`
    if let Some(k) = f.iter().position(|x| x == "transform") {
        if let Some(t) = f.get(k + 1).and_then(|t| t.parse::<u8>().ok()) {
            w.push("transform".into());
            w.push((t % 4).to_string());
        }
    }
    monitor_line(&w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let mut c = Config::default();
        parse("monitor DP-3 1920x1080@165 at 0,0\nmonitor HDMI-A-1 preferred at 1920,0 # right\nkeyboard layout es variant \"\" repeat 30\ntouchpad tap on natural on speed 0.3\nidle off-after 600", &mut c);
        assert_eq!(c.monitors[0].mode, ModeWish::Exact(1920, 1080, 165.0));
        assert_eq!(c.monitors[1].at, Some((1920, 0)));
        assert_eq!(c.keyboard.layout.as_deref(), Some("es"));
        assert_eq!(c.keyboard.variant.as_deref(), Some(""));
        assert_eq!(c.repeat(), (30, 400));
        assert_eq!(c.touchpad.tap, Some(true));
        assert_eq!(c.off_after, Some(600));
    }

    #[test]
    fn window_rules() {
        let mut c = Config::default();
        parse("window app=pavucontrol float size 820x560\nwindow app=org.gnome.* workspace 2\nwindow title=\"Picture in Picture\" float monitor HDMI-A-1\nwindow float", &mut c);
        assert_eq!(c.windows.len(), 3);
        let r = c.for_window("pavucontrol", "Volume");
        assert!(r.float && r.size == Some((820, 560)));
        assert_eq!(c.for_window("org.gnome.Calculator", "").workspace, Some(2));
        assert_eq!(c.for_window("firefox", "picture in picture").monitor.as_deref(), Some("HDMI-A-1"));
        assert_eq!(c.for_window("kitty", "zsh"), ForWindow::default());
        assert!(crate::window_rules::matches("*picture*", "Picture-in-Picture") && !crate::window_rules::matches("fire", "firefox") && crate::window_rules::matches("fire*", "firefox"));
        parse("window app=org.keepassxc.* private", &mut c);
        assert!(c.for_window("org.keepassxc.KeePassXC", "Passwords").private);
        assert!(!c.for_window("kitty", "zsh").private);
    }

    #[test]
    fn hyprland_lines() {
        let m = hypr_monitor(r#"hl.monitor({ output = "DP-3", mode = "1920x1080@165.00Hz", position = "0x0", scale = 1, transform = 0 })"#).unwrap();
        assert_eq!(m.name, "DP-3");
        assert_eq!(m.mode, ModeWish::Exact(1920, 1080, 165.0));
        assert_eq!(m.at, Some((0, 0)));
        assert_eq!(m.scale, Some(1.0));
        assert!(hypr_monitor(r#"hl.monitor({ output = MONITOR1, mode = "preferred" })"#).is_none());
        let m = hypr_monitor("monitor = HDMI-A-1, 1920x1080@60, 1920x0, 1").unwrap();
        assert_eq!(m.at, Some((1920, 0)));
        assert_eq!(m.transform, 0);
        assert_eq!(hypr_monitor("monitor = DP-2, 1920x1080, 0x0, 1, transform, 1").unwrap().transform, 1);
        assert_eq!(hypr_monitor(r#"hl.monitor({ output = "DP-2", mode = "preferred", transform = 3 })"#).unwrap().transform, 3);
        let mut c = Config::default();
        parse("monitor DP-2 preferred transform 90\nmonitor DP-3 rotate 270", &mut c);
        assert_eq!((c.monitors[0].transform, c.monitors[1].transform), (1, 3));
        assert_eq!(setting(r#"kb_layout = "es","#, "kb_layout").as_deref(), Some("es"));
    }
}
