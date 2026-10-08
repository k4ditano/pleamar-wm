//! The session's key bindings: `~/.config/pleamar/keys.conf`, or the ones
//! that come with pleamar-wm (`keys.conf` here) if there is none. One a line:
//!
//! ```text
//! defaults                            the ones that come with it, here; then add or change
//! bind Super+Return launch kitty      a program
//! bind Super+q close                  an action of the window manager's scene (its events)
//! unbind Super+t                      one of the defaults, gone
//! bind Super+3 workspace 3            an action with a number (its payload)
//! gesture swipe3_down close           a touchpad gesture, the same way
//! bind XF86AudioRaiseVolume repeat locked launch marea volume_up
//! shortcut push-to-talk Super+F9      a program's global shortcut, on that key
//! ```
//!
//! Before the action, `repeat` makes a held key act again at the keyboard's
//! pace (volume, brightness), `locked` lets it act with the screen locked, and
//! `release` makes it act when the key is let go —hold to talk, beside the
//! same key's binding that starts it—.
//!
//! An action is any event the window manager's scene declares, so a scene of
//! one's own (`~/.config/pleamar/wm/session.plm`) brings its own actions and
//! this file names them. A bound key goes to the binding and to nobody else:
//! not to the scene, not to a program.

use pleamar::scene::Mods;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Launch(String),
    /// An event of the scene, with a number if the line gives one:
    /// `bind Super+3 workspace 3`.
    Emit(String, Option<f32>),
    /// A program's global shortcut let go (its session and its id): the
    /// portal tells it.
    ShortcutUp(String, String),
}

#[derive(Clone, Debug)]
pub struct Bind {
    ctrl: bool,
    alt: bool,
    shift: bool,
    logo: bool,
    key: String,
    pub action: Action,
    /// Held down, it acts again at the keyboard's pace.
    pub repeat: bool,
    /// It acts with the screen locked too.
    pub locked: bool,
    /// It acts when the key is let go, not when it goes down.
    pub release: bool,
}

impl Bind {
    fn same(&self, o: &Bind) -> bool {
        o.key == self.key && o.ctrl == self.ctrl && o.alt == self.alt && o.shift == self.shift && o.logo == self.logo && o.release == self.release
    }
}

#[derive(Default, Debug)]
pub struct Keys {
    binds: Vec<Bind>,
    gestures: Vec<(String, Action)>,
    /// Which key a program's global shortcut goes on, whatever it asked for:
    /// `shortcut [app:]id Mods+key`.
    shortcuts: Vec<(Option<String>, String, Bind)>,
}

impl Keys {
    /// What a key does, if it is bound: by its keysym's name, with the
    /// modifiers held. Its action, and how it acts.
    pub fn bind(&self, name: &str, mods: Mods) -> Option<&Bind> {
        self.find(name, mods, false)
    }

    /// What the same key does when it is let go (`release`).
    pub fn on_release(&self, name: &str, mods: Mods) -> Option<&Bind> {
        self.find(name, mods, true)
    }

    fn find(&self, name: &str, mods: Mods, release: bool) -> Option<&Bind> {
        let name = name.to_lowercase();
        self.binds.iter().rev().find(|b| b.key == name && b.ctrl == mods.ctrl && b.alt == mods.alt && b.shift == mods.shift && b.logo == mods.logo && b.release == release)
    }

    /// The key the user gave a program's shortcut, if any (`shortcut`).
    pub fn shortcut(&self, app: &str, id: &str) -> Option<Bind> {
        self.shortcuts.iter().rev().find(|(a, i, _)| i == id && a.as_deref().is_none_or(|a| a == app)).map(|(_, _, b)| b.clone())
    }

    pub fn gesture(&self, name: &str) -> Option<&Action> {
        self.gestures.iter().rev().find(|(g, _)| g == name).map(|(_, a)| a)
    }
}

/// The bindings that come with pleamar-wm.
pub const DEFAULTS: &str = include_str!("../keys.conf");

pub fn get() -> &'static Keys {
    static KEYS: OnceLock<Keys> = OnceLock::new();
    KEYS.get_or_init(|| {
        let mut k = Keys::default();
        match crate::config::user_dir().map(|d| format!("{d}/keys.conf")).filter(|p| std::path::Path::new(p).exists()) {
            Some(file) => {
                println!("keys · {file}");
                parse(&std::fs::read_to_string(&file).unwrap_or_default(), &mut k);
            }
            None => parse(DEFAULTS, &mut k),
        }
        k
    })
}

pub fn parse(text: &str, k: &mut Keys) {
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // A comment after the binding (`bind Super+q close  # ⌘Q`). Not after
        // a command, which the shell reads whole (`#` may be part of it there).
        let line = match line.find(" #").or_else(|| line.find("\t#")) {
            Some(at) if !line.split_whitespace().any(|w| w == "launch") => line[..at].trim_end(),
            _ => line,
        };
        let mut words = line.splitn(3, char::is_whitespace);
        let (what, first, rest) = (words.next().unwrap_or(""), words.next().unwrap_or("").trim(), words.next().unwrap_or("").trim());
        match what {
            "defaults" => parse(DEFAULTS, k),
            "bind" => match (combo(first), flags(rest)) {
                (Some(mut b), (repeat, locked, release, Some(a))) => {
                    b.action = a;
                    b.repeat = repeat;
                    b.locked = locked;
                    b.release = release;
                    k.binds.retain(|o| !o.same(&b));
                    k.binds.push(b);
                }
                _ => eprintln!("keys · line {}: «{line}» is not «bind Mods+key action»", n + 1),
            },
            "unbind" => match combo(first) {
                //  Both: what it does going down and letting go.
                Some(b) => k.binds.retain(|o| !(o.key == b.key && o.ctrl == b.ctrl && o.alt == b.alt && o.shift == b.shift && o.logo == b.logo)),
                None => eprintln!("keys · line {}: «{line}» is not «unbind Mods+key»", n + 1),
            },
            "shortcut" => {
                let (id, key) = (first, rest.split_whitespace().next().unwrap_or(""));
                match combo(key).filter(|_| !id.is_empty()) {
                    Some(b) => {
                        let (app, id) = match id.split_once(':') {
                            Some((a, i)) => (Some(a.to_owned()), i.to_owned()),
                            None => (None, id.to_owned()),
                        };
                        k.shortcuts.push((app, id, b));
                    }
                    None => eprintln!("keys · line {}: «{line}» is not «shortcut [app:]id Mods+key»", n + 1),
                }
            }
            "gesture" => match action(rest) {
                Some(a) if !first.is_empty() => {
                    k.gestures.retain(|(g, _)| g != first);
                    k.gestures.push((first.to_owned(), a));
                }
                _ => eprintln!("keys · line {}: «{line}» is not «gesture name action»", n + 1),
            },
            _ => eprintln!("keys · line {}: I don't know «{what}» (bind, unbind, gesture, defaults)", n + 1),
        }
    }
}

/// `Super+Shift+Left`: the modifiers in any order, and the key last, by its
/// keysym's name (`q`, `Return`, `Left`, `space`, `Print`, `minus`).
/// A key as the portals write it (`CTRL+SHIFT+a`, `LOGO+F9`): the same as here.
pub fn trigger(s: &str) -> Option<Bind> {
    combo(s)
}

impl Bind {
    /// Whether that key, with those modifiers held, is this one.
    pub fn fits(&self, name: &str, mods: Mods) -> bool {
        self.key == name.to_lowercase() && self.ctrl == mods.ctrl && self.alt == mods.alt && self.shift == mods.shift && self.logo == mods.logo
    }

    /// How it is written, for whoever shows it: `Super+Shift+F9`.
    pub fn describe(&self) -> String {
        let mut s = String::new();
        for (on, m) in [(self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+"), (self.logo, "Super+")] {
            if on {
                s.push_str(m);
            }
        }
        s + &self.key
    }
}

fn combo(s: &str) -> Option<Bind> {
    let parts: Vec<&str> = s.split('+').filter(|p| !p.is_empty()).collect();
    let (key, mods) = parts.split_last()?;
    let mut b = Bind { ctrl: false, alt: false, shift: false, logo: false, key: key.to_lowercase(), action: Action::Emit(String::new(), None), repeat: false, locked: false, release: false };
    for m in mods {
        match m.to_lowercase().as_str() {
            "super" | "logo" | "mod4" | "win" => b.logo = true,
            "ctrl" | "control" => b.ctrl = true,
            "alt" | "mod1" => b.alt = true,
            "shift" => b.shift = true,
            _ => return None,
        }
    }
    Some(b)
}

/// `repeat`, `locked` and `release` before the action, in any order.
fn flags(mut s: &str) -> (bool, bool, bool, Option<Action>) {
    let (mut repeat, mut locked, mut release) = (false, false, false);
    loop {
        s = s.trim_start();
        if let Some(r) = s.strip_prefix("repeat ") {
            repeat = true;
            s = r;
        } else if let Some(r) = s.strip_prefix("locked ") {
            locked = true;
            s = r;
        } else if let Some(r) = s.strip_prefix("release ") {
            release = true;
            s = r;
        } else {
            return (repeat, locked, release, action(s));
        }
    }
}

fn action(s: &str) -> Option<Action> {
    let s = s.trim();
    match s.split_once(char::is_whitespace) {
        Some(("launch", cmd)) if !cmd.trim().is_empty() => Some(Action::Launch(cmd.trim().to_owned())),
        None if !s.is_empty() && s != "launch" => Some(Action::Emit(s.to_owned(), None)),
        Some((event, n)) if event != "launch" => n.trim().parse::<f32>().ok().map(|n| Action::Emit(event.to_owned(), Some(n))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds() {
        let mut k = Keys::default();
        parse("bind Super+q close\nbind Super+Return launch kitty --single\nbind Shift+Super+m restore_last\ngesture swipe3_down close\n", &mut k);
        let sup = Mods { logo: true, ..Default::default() };
        assert_eq!(k.bind("q", sup).map(|b| &b.action), Some(&Action::Emit("close".into(), None)));
        assert_eq!(k.bind("Return", sup).map(|b| &b.action), Some(&Action::Launch("kitty --single".into())));
        assert_eq!(k.bind("M", Mods { logo: true, shift: true, ..Default::default() }).map(|b| &b.action), Some(&Action::Emit("restore_last".into(), None)));
        assert!(k.bind("q", Mods::default()).is_none());
        assert_eq!(k.gesture("swipe3_down"), Some(&Action::Emit("close".into(), None)));
        parse("unbind Super+q\n", &mut k);
        assert!(k.bind("q", sup).is_none());
    }

    #[test]
    fn comments_after() {
        let mut k = Keys::default();
        parse("bind Super+q           close            # ⌘Q\nbind Super+t launch sh -c 'echo #1'\n", &mut k);
        let sup = Mods { logo: true, ..Default::default() };
        assert_eq!(k.bind("q", sup).map(|b| &b.action), Some(&Action::Emit("close".into(), None)));
        assert_eq!(k.bind("t", sup).map(|b| &b.action), Some(&Action::Launch("sh -c 'echo #1'".into())));
    }

    #[test]
    fn flags() {
        let mut k = Keys::default();
        parse("bind Super+Shift+v launch voxtype record start\nbind Super+Shift+v release launch voxtype record stop\n", &mut k);
        let m = Mods { logo: true, shift: true, ..Default::default() };
        assert_eq!(k.bind("v", m).map(|b| &b.action), Some(&Action::Launch("voxtype record start".into())));
        let r = k.on_release("v", m).expect("the release is its own binding");
        assert!(r.release && !r.repeat);
        assert_eq!(r.action, Action::Launch("voxtype record stop".into()));
    }

    #[test]
    fn shortcuts() {
        let mut k = Keys::default();
        parse("shortcut push-to-talk Super+F9\nshortcut com.discordapp.Discord:mute Ctrl+m\n", &mut k);
        let ptt = k.shortcut("anything", "push-to-talk").expect("for any program");
        assert!(ptt.fits("F9", Mods { logo: true, ..Default::default() }));
        assert!(k.shortcut("com.discordapp.Discord", "mute").is_some());
        assert!(k.shortcut("org.other", "mute").is_none());
        assert_eq!(trigger("CTRL+SHIFT+p").map(|b| b.describe()).as_deref(), Some("Ctrl+Shift+p"));
    }

    #[test]
    fn defaults_parse() {
        let mut k = Keys::default();
        parse(DEFAULTS, &mut k);
        assert!(k.bind("q", Mods { logo: true, ..Default::default() }).is_some());
        // The keyboard's own keys: they repeat and work locked.
        let up = k.bind("XF86AudioRaiseVolume", Mods::default()).expect("volume up is bound");
        assert!(up.repeat && up.locked);
    }
}
