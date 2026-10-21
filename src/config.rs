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
    /// How each monitor starts: `layout DP-3 scrolling`, `layout * marea`
    /// (the last line that names a monitor, or `*`, is its). See `LAYOUTS`.
    pub layouts: Vec<(String, String)>,
    /// The programs pinned to the dock of free monitors, in order: `dock
    /// kitty zen-browser org.telegram.desktop`.
    pub dock: Vec<String>,
    /// How that dock behaves: `dock-style hide on magnify off dots on into
    /// off size 44 side bottom` (the control centre writes it).
    pub dock_style: DockStyle,
    /// What a program's surface that asks for it (ext-background-effect) has
    /// behind it: `blur passes 3 offset 1 noise 0.02 saturation 1.2 brightness 0.9`.
    pub blur: Blur,
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
    /// `terminals off`: the terminals the session starts get the user's
    /// shell as it is, without the layer that lets wired windows read and
    /// type in them (see `term.rs`). On unless said.
    pub terminals_off: bool,
}

/// How the dock of free monitors behaves (`dock-style …`).
#[derive(Clone, Debug)]
pub struct DockStyle {
    /// Out of sight until the mouse is pushed against its edge; the windows
    /// then have that room too.
    pub hide: bool,
    /// The icons under the mouse swell, the nearest most, instead of rising as buoys.
    pub magnify: bool,
    /// A drop under each program per window it has.
    pub dots: bool,
    /// Put away, a window goes into its program's icon, not onto the dock beside it.
    pub into: bool,
    /// The icons' size, 32 to 72.
    pub size: u32,
    /// Which edge: `bottom`, `left` or `right`.
    pub side: DockSide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSide {
    Bottom,
    Left,
    Right,
}

impl DockSide {
    pub fn name(self) -> &'static str {
        match self {
            DockSide::Bottom => "bottom",
            DockSide::Left => "left",
            DockSide::Right => "right",
        }
    }
    fn of(w: &str) -> Option<DockSide> {
        match w {
            "bottom" => Some(DockSide::Bottom),
            "left" => Some(DockSide::Left),
            "right" => Some(DockSide::Right),
            _ => None,
        }
    }
}

impl Default for DockStyle {
    fn default() -> Self {
        DockStyle { hide: false, magnify: false, dots: true, into: false, size: 44, side: DockSide::Bottom }
    }
}

impl DockStyle {
    /// One of its settings, from words (`hide on`, `size 52`, `side left`).
    pub fn set(&mut self, k: &str, v: Option<&String>) -> Result<(), String> {
        let flag = |v: Option<&String>| on(v).ok_or(format!("`{k} on` or `{k} off`"));
        match k {
            "hide" => self.hide = flag(v)?,
            "magnify" => self.magnify = flag(v)?,
            "dots" => self.dots = flag(v)?,
            "into" => self.into = flag(v)?,
            "size" => self.size = v.and_then(|v| v.parse().ok()).filter(|n| (32..=72).contains(n)).ok_or("`size N`, 32 to 72")?,
            "side" => self.side = v.and_then(|v| DockSide::of(v)).ok_or("`side bottom`, `left` or `right`")?,
            _ => return Err(format!("the dock has no '{k}': hide, magnify, dots, into, size, side")),
        }
        Ok(())
    }

    fn line(&self) -> String {
        let f = |b: bool| if b { "on" } else { "off" };
        format!("dock-style hide {} magnify {} dots {} into {} size {} side {}", f(self.hide), f(self.magnify), f(self.dots), f(self.into), self.size, self.side.name())
    }

    /// The facts the scene reads (`dock.hide`, `dock.side`: 0 bottom, 1 left, 2 right…).
    pub fn facts(&self) -> [(&'static str, String); 6] {
        let b = |b: bool| if b { "1" } else { "0" }.to_owned();
        let side = match self.side {
            DockSide::Bottom => 0,
            DockSide::Left => 1,
            DockSide::Right => 2,
        };
        [("dock.hide", b(self.hide)), ("dock.mag", b(self.magnify)), ("dock.dots", b(self.dots)), ("dock.into", b(self.into)), ("dock.size", self.size.to_string()), ("dock.side", side.to_string())]
    }
}

/// The blur behind a program's surface that asks for it: dual Kawase, as most
/// compositors do it (each pass halves what is behind, and as many come back
/// up), and then a grain and its colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Blur {
    /// How many times it is halved (1 to 6): the more, the wider.
    pub passes: u32,
    /// How far each pass reaches, in pixels of its own (0.5 to 8).
    pub offset: f32,
    /// A grain over it, so it does not band (0 to 0.5).
    pub noise: f32,
    /// 0 grey, 1 as it was, more, more vivid (0 to 3).
    pub saturation: f32,
    /// 1 as it was; less, darker (0 to 3).
    pub brightness: f32,
    /// Windows' menus blurred behind even when they do not ask for it.
    pub menus: bool,
    /// Where those menus are less opaque than this (0 to 1), no blur: their
    /// rounded corners and shadows stay clear.
    pub menus_ignorealpha: f32,
}

impl Default for Blur {
    fn default() -> Self {
        Blur { passes: 3, offset: 1.0, noise: 0.0, saturation: 1.0, brightness: 1.0, menus: false, menus_ignorealpha: 0.2 }
    }
}

impl Blur {
    /// One of its settings, from words (`passes 4`, `noise 0.02`).
    pub fn set(&mut self, k: &str, v: Option<&String>) -> Result<(), String> {
        let number = |range: std::ops::RangeInclusive<f32>| v.and_then(|v| v.parse::<f32>().ok()).filter(|n| range.contains(n)).ok_or(format!("`{k} N`, {} to {}", range.start(), range.end()));
        match k {
            "passes" => self.passes = number(1.0..=6.0)?.round() as u32,
            "offset" => self.offset = number(0.5..=8.0)?,
            "noise" => self.noise = number(0.0..=0.5)?,
            "saturation" => self.saturation = number(0.0..=3.0)?,
            "brightness" => self.brightness = number(0.0..=3.0)?,
            "menus-ignorealpha" => self.menus_ignorealpha = number(0.0..=1.0)?,
            "menus" => {
                self.menus = match v.map(String::as_str) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err(format!("`{k} on` or `{k} off`")),
                }
            }
            _ => return Err(format!("the blur has no '{k}': passes, offset, noise, saturation, brightness, menus, menus-ignorealpha")),
        }
        Ok(())
    }
}

/// A rule for the windows that match it: by their program (`app=`, the
/// app_id or the X11 class) and/or their title (`title=`), with no case, and
/// `*` for anything (`app=org.gnome.*`). Several may match: each says what it says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowRule {
    pub app: Option<String>,
    pub title: Option<String>,
    /// It floats over the layout, at its own size, as a dialog does.
    pub float: bool,
    pub size: Option<(u32, u32)>,
    /// A monitor by its number (0 the leftmost) or its name.
    pub monitor: Option<String>,
    pub workspace: Option<usize>,
    /// Never seen in what is shared of a whole monitor, nor in its screenshots:
    /// pixelated there (a password manager, a private chat).
    pub private: bool,
    /// `frame=own`: the program draws its own title bar and edges (a
    /// browser's tabs up there); `frame=ours`: the scene draws them, if the
    /// program lets it. Without it, the scene's, for whoever lets it.
    pub frame: Option<bool>,
}

/// What the rules say for a window: float, size, monitor, workspace, private, frame.
#[derive(Default, Debug, PartialEq)]
pub struct ForWindow {
    pub float: bool,
    pub size: Option<(u32, u32)>,
    pub monitor: Option<String>,
    pub workspace: Option<usize>,
    pub private: bool,
    /// Some(true) the scene's frame, Some(false) its own.
    pub frame: Option<bool>,
}

/// `*` for anything, no case: `org.gnome.*`, `*Picture*`, `firefox`.
fn matches(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.to_lowercase(), text.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let mut at = 0;
    for (k, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match t[at..].find(part) {
            Some(i) if k > 0 || i == 0 => at += i + part.len(),
            _ => return false,
        }
    }
    parts.last().is_some_and(|l| l.is_empty()) || at == t.len()
}

impl Config {
    pub fn for_window(&self, app: &str, title: &str) -> ForWindow {
        let mut out = ForWindow::default();
        for r in &self.windows {
            if r.app.as_deref().is_some_and(|p| !matches(p, app)) || r.title.as_deref().is_some_and(|p| !matches(p, title)) {
                continue;
            }
            out.float |= r.float;
            out.private |= r.private;
            out.size = r.size.or(out.size);
            out.monitor = r.monitor.clone().or(out.monitor);
            out.workspace = r.workspace.or(out.workspace);
            out.frame = r.frame.or(out.frame);
        }
        out
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
/// The layouts a monitor can start in, by their number in the scene
/// (`lay.N`); `free` is no layout: windows where they are left, on a plane
/// with no edges seen through a camera; `canvas`, a name it had, is free.
pub const LAYOUTS: [&str; 10] = ["leader", "columns", "rows", "grid", "leader-right", "scrolling", "marea", "fractal", "free", "canvas"];
/// Where `free` is in `LAYOUTS`: it and what comes after it are free.
pub const FREE: usize = 8;

pub fn layout_index(name: &str) -> Option<usize> {
    LAYOUTS.iter().position(|l| *l == name).or((name == "leader-left").then_some(0))
}

impl Config {
    /// The layout a monitor starts in, if the configuration says one.
    pub fn layout_of(&self, monitor: &str) -> Option<usize> {
        self.layouts.iter().rev().find(|(m, _)| m == monitor || m == "*").and_then(|(_, l)| layout_index(l))
    }
}

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

/// Writes one of the dock's settings (`dock-style …` in session.conf): the
/// line as it is with that one changed, where it was, or at the end.
pub fn write_dock_style(k: &str, v: &str) -> Result<(), String> {
    let path = std::env::var("PLEAMAR_WM_CONFIG").ok().or_else(|| user_dir().map(|d| format!("{d}/session.conf"))).ok_or("there is no configuration folder")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut style = DockStyle::default();
    for l in text.lines() {
        let w = words(l.split('#').next().unwrap_or(""));
        if w.first().map(String::as_str) == Some("dock-style") {
            for p in w[1..].chunks(2) {
                let _ = style.set(&p[0], p.get(1));
            }
        }
    }
    style.set(k, Some(&v.to_owned()))?;
    let line = style.line();
    let mut out: Vec<String> = Vec::new();
    let mut put = false;
    for l in text.lines() {
        let w = words(l.split('#').next().unwrap_or(""));
        if w.first().map(String::as_str) == Some("dock-style") {
            if !put {
                out.push(line.clone());
                put = true;
            }
            continue;
        }
        out.push(l.to_owned());
    }
    if !put {
        out.push(line);
    }
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, out.join("\n") + "\n").map_err(|e| format!("could not write {path}: {e}"))
}

/// Writes how a monitor starts (`layout MONITOR NAME` in session.conf; `*`
/// for every monitor): its line replaced where it was, or added at the end.
pub fn write_layout(monitor: &str, name: &str) -> Result<(), String> {
    if layout_index(name).is_none() {
        return Err(format!("'{name}' is not a layout: {}", LAYOUTS.join(", ")));
    }
    let path = std::env::var("PLEAMAR_WM_CONFIG").ok().or_else(|| user_dir().map(|d| format!("{d}/session.conf"))).ok_or("there is no configuration folder")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let line = format!("layout {monitor} {name}");
    let mut out: Vec<String> = Vec::new();
    let mut put = false;
    for l in text.lines() {
        let w = words(l.split('#').next().unwrap_or(""));
        if w.first().map(String::as_str) == Some("layout") && w.get(1).map(String::as_str) == Some(monitor) {
            if !put {
                out.push(line.clone());
                put = true;
            }
            continue;
        }
        out.push(l.to_owned());
    }
    if !put {
        out.push(line);
    }
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, out.join("\n") + "\n").map_err(|e| format!("could not write {path}: {e}"))
}

/// The file that says the monitors must not go dark (`pleamar-wm idle hold`):
/// it is there, or it is not, so that a script can change it while the session
/// runs, which `session.conf` (read once) cannot.
pub fn idle_hold_path() -> std::path::PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/tmp".into());
    std::path::PathBuf::from(dir).join("pleamar-wm-idle-hold")
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

/// Words, with "quoted ones" kept whole (an empty variant is `""`).
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
        } else if ch == '"' {
            chars.next();
            out.push(chars.by_ref().take_while(|c| *c != '"').collect());
        } else {
            // `title="Some title"`: quotes inside a word keep its spaces, and go.
            let mut w = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                chars.next();
                if c == '"' {
                    w.extend(chars.by_ref().take_while(|c| *c != '"'));
                } else {
                    w.push(c);
                }
            }
            out.push(w);
        }
    }
    out
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
            "dock-style" => pairs(&mut |k, v| {
                if let Err(e) = c.dock_style.set(k, v) {
                    eprintln!("config · line {}: {e}", n + 1);
                }
            }),
            "blur" => pairs(&mut |k, v| {
                if let Err(e) = c.blur.set(k, v) {
                    eprintln!("config · line {}: {e}", n + 1);
                }
            }),
            "window" => {
                let mut r = WindowRule::default();
                let mut k = 0;
                let mut ok = true;
                while k < rest.len() {
                    let w = rest[k].as_str();
                    if let Some(v) = w.strip_prefix("app=") {
                        r.app = Some(v.to_owned());
                    } else if let Some(v) = w.strip_prefix("title=") {
                        r.title = Some(v.to_owned());
                    } else if w == "float" {
                        r.float = true;
                    } else if w == "private" {
                        r.private = true;
                    } else if let Some(v) = w.strip_prefix("frame=") {
                        r.frame = match v {
                            "ours" => Some(true),
                            "own" => Some(false),
                            _ => None,
                        };
                        ok &= r.frame.is_some();
                    } else if w == "size" {
                        r.size = rest.get(k + 1).and_then(|s| s.split_once('x')).and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)));
                        ok &= r.size.is_some();
                        k += 1;
                    } else if w == "monitor" {
                        r.monitor = rest.get(k + 1).cloned();
                        ok &= r.monitor.is_some();
                        k += 1;
                    } else if w == "workspace" {
                        r.workspace = rest.get(k + 1).and_then(|s| s.parse().ok()).filter(|n: &usize| *n >= 1);
                        ok &= r.workspace.is_some();
                        k += 1;
                    } else {
                        ok = false;
                    }
                    k += 1;
                }
                if ok && (r.app.is_some() || r.title.is_some()) {
                    c.windows.push(r);
                } else {
                    eprintln!("config · line {}: a window rule is `window app=NAME|title=TEXT [float] [size WxH] [monitor N|NAME] [workspace N] [private] [frame=own|ours]`", n + 1);
                }
            }
            "layout" => match rest {
                [monitor, name] if layout_index(name).is_some() => c.layouts.push((monitor.clone(), name.clone())),
                _ => eprintln!("config · line {}: a layout line is `layout MONITOR|* NAME`, NAME one of {}", n + 1, LAYOUTS.join(", ")),
            },
            "agent" => c.agent = on(rest.first()).unwrap_or_else(|| {
                eprintln!("config · line {}: `agent on` or `agent off`", n + 1);
                false
            }),
            "terminals" => c.terminals_off = !on(rest.first()).unwrap_or_else(|| {
                eprintln!("config · line {}: `terminals on` or `terminals off`", n + 1);
                true
            }),
            "idle" => pairs(&mut |k, v| match k {
                "off-after" => c.off_after = v.and_then(|v| v.parse().ok()).filter(|s| *s > 0),
                _ => eprintln!("config · line {}: idle has no '{k}'", n + 1),
            }),
            "phone" => match rest.split_first() {
                Some((w, cmd)) if w == "lock" && !cmd.is_empty() => c.phone_lock = Some(cmd.join(" ")),
                _ => eprintln!("config · line {}: `phone lock COMMAND` (or `phone lock none`)", n + 1),
            },
            other => eprintln!("config · line {}: '{other}' is not something it knows (monitor, layout, keyboard, pointer, touchpad, idle, agent, terminals, phone)", n + 1),
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
    fn the_blur_line_sets_what_it_says_and_leaves_the_rest() {
        let mut c = Config::default();
        parse("blur passes 5 noise 0.03 saturation 1.4 menus on menus-ignorealpha 0.5", &mut c);
        assert_eq!(c.blur, Blur { passes: 5, noise: 0.03, saturation: 1.4, menus: true, menus_ignorealpha: 0.5, ..Blur::default() });
        // Out of its range, or a word it does not know: said, and left as it was.
        let mut b = Blur::default();
        assert!(b.set("passes", Some(&"9".to_owned())).is_err());
        assert!(b.set("menus", Some(&"yes".to_owned())).is_err());
        assert!(b.set("glow", Some(&"1".to_owned())).is_err());
        assert_eq!(b, Blur::default());
    }

    #[test]
    fn a_layout_line_is_written_in_place_of_its_old_one() {
        let dir = std::env::temp_dir().join(format!("pleamar-wm-layout-{}", std::process::id()));
        let file = dir.join("session.conf");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "monitor DP-3 preferred\nlayout DP-3 grid # mine\ndock kitty\n").unwrap();
        // SAFETY: a test of its own process; nothing else reads it meanwhile.
        unsafe { std::env::set_var("PLEAMAR_WM_CONFIG", &file) };
        write_layout("DP-3", "scrolling").unwrap();
        write_layout("*", "marea").unwrap();
        assert!(write_layout("DP-3", "sideways").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "monitor DP-3 preferred\nlayout DP-3 scrolling\ndock kitty\nlayout * marea\n");
        unsafe { std::env::remove_var("PLEAMAR_WM_CONFIG") };
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_monitor_starts_in_the_layout_its_line_says() {
        let mut c = Config::default();
        parse("layout * marea\nlayout HDMI-A-1 scrolling\nlayout DP-2 free\nlayout DP-9 sideways\nlayout DP-1 canvas", &mut c);
        assert_eq!(c.layout_of("HDMI-A-1"), Some(5));
        assert_eq!(c.layout_of("DP-3"), Some(6), "`*` for the rest");
        assert_eq!(c.layout_of("DP-2"), Some(FREE));
        assert_eq!(c.layout_of("DP-1"), Some(FREE + 1));
        assert_eq!(c.layouts.len(), 4, "a layout it does not know is said, not kept");
        assert_eq!(layout_index("fractal"), Some(7));
        assert_eq!(Config::default().layout_of("DP-3"), None);
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
        assert!(matches("*picture*", "Picture-in-Picture") && !matches("fire", "firefox") && matches("fire*", "firefox"));
        parse("window app=org.keepassxc.* private", &mut c);
        assert!(c.for_window("org.keepassxc.KeePassXC", "Passwords").private);
        assert!(!c.for_window("kitty", "zsh").private);
        parse("window app=zen frame=own\nwindow app=cA frame=ours\nwindow app=kitty frame=nobody", &mut c);
        assert_eq!(c.for_window("zen", "Inicio").frame, Some(false));
        assert_eq!(c.for_window("cA", "sh").frame, Some(true));
        assert_eq!(c.for_window("kitty", "zsh").frame, None, "an unknown frame word is no rule");
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
