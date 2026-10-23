//! What a window does: a pleamar scene (its socket), and pictures unless it
//! is private. What it does not do is refused here.

use super::*;

/// The verbs, in the order they are told.
const VERBS: [&str; 16] = ["tree", "find", "at", "press", "focus", "type", "key", "hotkey", "wait", "listen", "say", "look", "click", "scroll", "drag", "move"];

/// What a window takes, as `C` and the CLI's own sockets say it.
#[derive(Clone)]
pub(super) struct Kinds {
    /// As the agent named it.
    pub(super) name: String,
    pub(super) scene: Option<(String, std::path::PathBuf)>,
    pub(super) private: bool,
    pub(super) locked: bool,
    /// Its menus' boxes from the root's corner.
    pub(super) popups: Vec<[i64; 4]>,
    /// The window manager's slot of the window it names.
    pub(super) slot: Option<usize>,
}

/// A window of `C`'s answer: process, slot, private, menus.
pub(super) struct Can {
    pub(super) pid: u32,
    pub(super) slot: usize,
    pub(super) private: bool,
    pub(super) popups: Vec<[i64; 4]>,
}

/// `can LOCKED | PID SLOT PRIVATE POPUPS | ...`, read; None for a
/// window manager that does not know `C`.
pub(super) fn can_reply(reply: &str) -> Option<(bool, Vec<Can>)> {
    let mut parts = reply.split(" | ");
    let locked = parts.next()?.strip_prefix("can ")? == "1";
    let windows = parts
        .filter_map(|p| {
            let f: Vec<&str> = p.split_whitespace().collect();
            let [pid, slot, private, popups] = f[..] else { return None };
            let popups = popups.split(';').filter_map(|b| {
                let v: Vec<i64> = b.split(',').filter_map(|n| n.parse().ok()).collect();
                <[i64; 4]>::try_from(v).ok()
            });
            Some(Can { pid: pid.parse().ok()?, slot: slot.parse().ok()?, private: private == "1", popups: popups.collect() })
        })
        .collect();
    Some((locked, windows))
}

impl Kinds {
    /// What that window takes: `C`, and its scene.
    pub(super) fn of(w: &Window) -> R<Kinds> {
        let reply = wm(&format!("C {}", w.pid))?;
        let (locked, can) = match can_reply(&reply) {
            Some((locked, mut c)) if !c.is_empty() => (locked, Some(c.remove(0))),
            // An older window manager.
            _ if reply == "err unknown-command" => (false, None),
            _ => return Err(answered(&reply, Of::Window(&w.pid))),
        };
        Ok(Kinds {
            name: w.pid.clone(),
            scene: scenes().remove(&w.process()),
            private: can.as_ref().is_some_and(|c| c.private),
            locked,
            slot: can.as_ref().map(|c| c.slot),
            popups: can.map(|c| c.popups).unwrap_or_default(),
        })
    }

    pub(super) fn words(&self) -> Vec<&'static str> {
        if self.private || self.locked {
            return Vec::new();
        }
        let mut w = Vec::new();
        if self.scene.is_some() {
            w.push("scene");
        }
        w.push("look");
        w
    }

    /// `scene,look`, or `nothing (private)`.
    pub(super) fn does(&self) -> String {
        does(&self.words(), self.private, self.locked)
    }

    pub(super) fn is_scene(&self) -> bool {
        self.scene.is_some()
    }

    /// What it is, said first in a refusal.
    fn is(&self) -> &'static str {
        if self.scene.is_some() {
            "a pleamar scene"
        } else {
            "a window"
        }
    }

    /// Its verbs.
    pub(super) fn verbs(&self) -> Vec<&'static str> {
        let w = self.words();
        VERBS.into_iter().filter(|v| w.iter().any(|k| verbs_of(k).contains(v))).collect()
    }

    /// A private window, or a locked screen: nothing is done with it.
    pub(super) fn open(&self) -> R {
        if self.locked {
            return Err(answered("locked", Of::Window(&self.name)));
        }
        if self.private {
            return Err(answered("private", Of::Window(&self.name)));
        }
        Ok(())
    }

    /// What it does not do, for every kind and verb alike: exit 3, and what it does.
    pub(super) fn refuse(&self, what: &str) -> Fail {
        if self.private || self.locked {
            return self.open().unwrap_err();
        }
        let hint = match what {
            _ if what.starts_with("wait ") => format!("it waits for {}", super::wait::waits(self)),
            "say" => "say is for a pleamar scene".to_owned(),
            _ if self.scene.is_some() => format!("its items by name: `tree {}` gives them as @name", self.name),
            _ => format!("no elements: `look {}`, then `click {} X Y`", self.name, self.name),
        };
        Fail::new(fail::NOT_HERE, format!("{} is {}: it does {} ({what} is not for it; {hint})", self.name, self.is(), self.does()))
    }
}

fn verbs_of(kind: &str) -> &'static [&'static str] {
    match kind {
        "scene" => &["tree", "find", "at", "press", "focus", "type", "key", "hotkey", "wait", "listen", "say", "look"],
        _ => &["look", "click", "scroll", "drag", "move", "type", "key", "hotkey"],
    }
}

pub(super) fn does(words: &[&str], private: bool, locked: bool) -> String {
    match words {
        [] if locked => "nothing (the screen is locked)".to_owned(),
        [] if private => "nothing (private)".to_owned(),
        _ => words.join(","),
    }
}

/// `agent can PID`: what it does, and its verbs.
pub(super) fn can(a: &Args) -> R {
    let k = Kinds::of(a.win()?)?;
    println!("{} does {}", k.name, k.does());
    if k.private || k.locked {
        println!("(ask the user: it is not read, looked at nor acted on)");
        return Ok(());
    }
    println!("verbs {}", k.verbs().join(" "));
    if let Some((scene, _)) = &k.scene {
        println!("scene {scene}: its items are @name (`tree {}`), a name of digits @=3", k.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(scene: bool, private: bool) -> Kinds {
        Kinds {
            name: "7002".into(),
            scene: scene.then(|| ("notes".to_owned(), std::path::PathBuf::new())),
            private,
            locked: false,
            popups: Vec::new(),
            slot: None,
        }
    }

    #[test]
    fn what_each_window_does() {
        assert_eq!(kinds(true, false).does(), "scene,look");
        assert_eq!(kinds(false, false).does(), "look");
        assert_eq!(kinds(false, true).does(), "nothing (private)");
        assert_eq!(kinds(false, false).verbs(), ["type", "key", "hotkey", "look", "click", "scroll", "drag", "move"]);
    }

    #[test]
    fn what_a_window_does_not_do_is_refused_alike() {
        let f = kinds(false, false).refuse("press");
        assert_eq!(f.code, fail::NOT_HERE);
        assert_eq!(f.msg, "7002 is a window: it does look (press is not for it; no elements: `look 7002`, then `click 7002 X Y`)");
        assert!(kinds(false, false).refuse("say").msg.starts_with("7002 is a window: it does look (say is not for it; say is for a pleamar scene)"));
        assert!(kinds(true, false).refuse("click").msg.starts_with("7002 is a pleamar scene: it does scene,look"));
        // Private: exit 2, never another way.
        assert_eq!(kinds(false, true).refuse("tree").code, fail::PRIVATE);
    }

    #[test]
    fn c_is_read() {
        let (locked, w) = can_reply("can 0 | 4521 3 0 - | 7002 4 1 10,20,200,90;0,-40,50,40").unwrap();
        assert!(!locked);
        assert_eq!((w[0].pid, w[0].slot, w[0].private, w[0].popups.len()), (4521, 3, false, 0));
        assert_eq!((w[1].private, &w[1].popups[..]), (true, &[[10, 20, 200, 90], [0, -40, 50, 40]][..]));
        assert!(can_reply("can 1").unwrap().0);
        assert!(can_reply("err unknown-command").is_none());
    }
}
