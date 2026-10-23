//! What a window does: a pleamar scene (its socket), a terminal (the term
//! layer of its process), a tree (on the accessibility bus, as `C` says),
//! and pictures unless it is private. What it does not do is refused here.

use super::*;

/// The verbs, in the order they are told.
const VERBS: [&str; 17] = ["tree", "find", "at", "press", "focus", "type", "key", "hotkey", "run", "wait", "listen", "say", "look", "click", "scroll", "drag", "move"];

/// What a window takes, as `C` and the CLI's own sockets say it.
#[derive(Clone)]
pub(super) struct Kinds {
    /// As the agent named it.
    pub(super) name: String,
    pub(super) scene: Option<(String, std::path::PathBuf)>,
    /// Its terminal's shells (only the one named, if one was).
    pub(super) shells: Vec<(i32, std::collections::BTreeMap<String, String>)>,
    pub(super) tree: bool,
    pub(super) private: bool,
    pub(super) locked: bool,
    /// Its menus' boxes from the root's corner.
    pub(super) popups: Vec<[i64; 4]>,
    /// The window manager's slot of the window it names.
    pub(super) slot: Option<usize>,
}

/// A window of `C`'s answer: process, slot, tree, private, menus.
pub(super) struct Can {
    pub(super) pid: u32,
    pub(super) slot: usize,
    pub(super) tree: bool,
    pub(super) private: bool,
    pub(super) popups: Vec<[i64; 4]>,
}

/// `can LOCKED | PID SLOT TREE PRIVATE POPUPS | ...`, read; None for a
/// window manager that does not know `C`.
pub(super) fn can_reply(reply: &str) -> Option<(bool, Vec<Can>)> {
    let mut parts = reply.split(" | ");
    let locked = parts.next()?.strip_prefix("can ")? == "1";
    let windows = parts
        .filter_map(|p| {
            let f: Vec<&str> = p.split_whitespace().collect();
            let [pid, slot, tree, private, popups] = f[..] else { return None };
            let popups = popups.split(';').filter_map(|b| {
                let v: Vec<i64> = b.split(',').filter_map(|n| n.parse().ok()).collect();
                <[i64; 4]>::try_from(v).ok()
            });
            Some(Can { pid: pid.parse().ok()?, slot: slot.parse().ok()?, tree: tree == "1", private: private == "1", popups: popups.collect() })
        })
        .collect();
    Some((locked, windows))
}

impl Kinds {
    /// What that window takes: `C`, its scene, its shells.
    pub(super) fn of(w: &Window) -> R<Kinds> {
        let layers = crate::term::layers();
        let reply = wm(&format!("C {}", w.pid))?;
        if reply == "err unknown-root-pid" {
            // A shell's id said as if it were a window.
            if let Some((id, info)) = layers.iter().find(|(id, _)| *id as u32 == w.process()) {
                let t = info.get("terminal").map_or("", String::as_str);
                return Err(usage(format!("{id} is a shell, not a window: say it with its terminal's window, {t}:{id}")));
            }
        }
        let (locked, can) = match can_reply(&reply) {
            Some((locked, mut c)) if !c.is_empty() => (locked, Some(c.remove(0))),
            // An older window manager: a tree is tried.
            _ if reply == "err unknown-command" => (false, None),
            _ => return Err(answered(&reply, Of::Window(&w.pid))),
        };
        let mut shells = crate::term::shells_of(w.process(), &layers);
        if let Some(id) = w.shell {
            let (_, info) = layers.iter().find(|(i, _)| *i == id).ok_or_else(|| usage(format!("no shell {id}: `pleamar-wm agent can {}` names its shells", w.pid)))?;
            if info.get("terminal").and_then(|t| t.parse::<u32>().ok()) != Some(w.process()) {
                return Err(usage(format!("shell {id} is not in {}'s terminal: `pleamar-wm agent can {}` names its shells", w.pid, w.pid)));
            }
            shells.retain(|(i, _)| *i == id);
        }
        Ok(Kinds {
            name: match w.shell {
                Some(s) => format!("{}:{s}", w.pid),
                None => w.pid.clone(),
            },
            scene: scenes().remove(&w.process()),
            shells,
            tree: can.as_ref().is_none_or(|c| c.tree),
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
        if !self.shells.is_empty() {
            w.push("text");
        }
        if self.tree {
            w.push("tree");
        }
        w.push("look");
        w
    }

    /// `tree,look`, or `nothing (private)`.
    pub(super) fn does(&self) -> String {
        does(&self.words(), self.private, self.locked)
    }

    pub(super) fn is_scene(&self) -> bool {
        self.scene.is_some()
    }

    pub(super) fn is_text(&self) -> bool {
        !self.shells.is_empty()
    }

    /// What it is, said first in a refusal.
    fn is(&self) -> &'static str {
        if self.scene.is_some() {
            "a pleamar scene"
        } else if !self.shells.is_empty() {
            "a terminal"
        } else if self.tree {
            "a window with a tree"
        } else {
            "a window without a tree"
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
            "run" => "run is for a terminal's shell".to_owned(),
            "say" => "say is for a pleamar scene".to_owned(),
            _ if self.scene.is_some() => format!("its items by name: `tree {}` gives them as @name", self.name),
            _ if !self.shells.is_empty() => format!("no @IDs: read it with `tree {}`, find lines with `find {} --text T`", self.name, self.name),
            _ if self.tree => format!("its elements by number: `tree {}` gives them as @ID", self.name),
            _ => format!("no elements: `look {}`, then `click {} X Y`", self.name, self.name),
        };
        Fail::new(fail::NOT_HERE, format!("{} is {}: it does {} ({what} is not for it; {hint})", self.name, self.is(), self.does()))
    }
}

fn verbs_of(kind: &str) -> &'static [&'static str] {
    match kind {
        "scene" => &["tree", "find", "at", "press", "focus", "type", "key", "hotkey", "wait", "listen", "say", "look"],
        "tree" => &["tree", "find", "at", "press", "focus", "type", "key", "hotkey", "wait", "listen", "look"],
        "text" => &["tree", "find", "run", "type", "key", "hotkey", "wait", "listen"],
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

/// `agent can PID`: what it does, its verbs, and of a terminal what runs in
/// it, where, its size, how long it has been quiet and its shells.
pub(super) fn can(a: &Args) -> R {
    let w = a.win()?;
    let k = Kinds::of(w)?;
    println!("{} does {}", k.name, k.does());
    if k.private || k.locked {
        println!("(ask the user: it is not read, looked at nor acted on)");
        return Ok(());
    }
    println!("verbs {}", k.verbs().join(" "));
    if let Some((scene, _)) = &k.scene {
        println!("scene {scene}: its items are @name (`tree {}`), a name of digits @=3", k.name);
    } else if k.tree {
        println!("elements @ID, as `tree {}` numbers them", k.name);
    }
    if k.shells.len() > 1 {
        let ids: Vec<String> = k.shells.iter().map(|(id, _)| id.to_string()).collect();
        println!("shells {} (name one: {}:{})", ids.join(","), w.pid, ids[0]);
    }
    for (id, info) in &k.shells {
        let g = |key: &str| info.get(key).map_or("", String::as_str);
        let quiet = g("quiet").parse::<u64>().map_or(String::new(), |q| format!("quiet {:.1} s", q as f64 / 1000.0));
        let full = if g("fullscreen") == "1" { " (a full-screen program)" } else { "" };
        println!("shell {id}: {} in {}, {}, {quiet}{full}, title {:?}", g("command"), g("cwd"), g("size"), g("title"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(scene: bool, shells: usize, tree: bool, private: bool) -> Kinds {
        Kinds {
            name: "7002".into(),
            scene: scene.then(|| ("notes".to_owned(), std::path::PathBuf::new())),
            shells: (0..shells).map(|k| (7044 + k as i32, Default::default())).collect(),
            tree,
            private,
            locked: false,
            popups: Vec::new(),
            slot: None,
        }
    }

    #[test]
    fn what_each_window_does() {
        assert_eq!(kinds(false, 0, true, false).does(), "tree,look");
        assert_eq!(kinds(false, 2, false, false).does(), "text,look");
        assert_eq!(kinds(false, 1, true, false).does(), "text,tree,look");
        assert_eq!(kinds(true, 0, false, false).does(), "scene,look");
        assert_eq!(kinds(false, 0, false, false).does(), "look");
        assert_eq!(kinds(false, 0, true, true).does(), "nothing (private)");
        assert_eq!(kinds(false, 1, false, false).verbs(), ["tree", "find", "type", "key", "hotkey", "run", "wait", "listen", "look", "click", "scroll", "drag", "move"]);
    }

    #[test]
    fn what_a_window_does_not_do_is_refused_alike() {
        let f = kinds(false, 1, false, false).refuse("press");
        assert_eq!(f.code, fail::NOT_HERE);
        assert_eq!(f.msg, "7002 is a terminal: it does text,look (press is not for it; no @IDs: read it with `tree 7002`, find lines with `find 7002 --text T`)");
        assert!(kinds(false, 0, true, false).refuse("run").msg.starts_with("7002 is a window with a tree: it does tree,look (run is not for it; run is for a terminal's shell)"));
        assert!(kinds(false, 0, false, false).refuse("tree").msg.contains("no elements: `look 7002`"));
        assert!(kinds(true, 0, false, false).refuse("run").msg.starts_with("7002 is a pleamar scene: it does scene,look"));
        // Private: exit 2, never another way.
        assert_eq!(kinds(false, 0, true, true).refuse("tree").code, fail::PRIVATE);
    }

    #[test]
    fn c_is_read() {
        let (locked, w) = can_reply("can 0 | 4521 3 1 0 - | 7002 4 0 1 10,20,200,90;0,-40,50,40").unwrap();
        assert!(!locked);
        assert_eq!((w[0].pid, w[0].slot, w[0].tree, w[0].private, w[0].popups.len()), (4521, 3, true, false, 0));
        assert_eq!((w[1].tree, w[1].private, &w[1].popups[..]), (false, true, &[[10, 20, 200, 90], [0, -40, 50, 40]][..]));
        assert!(can_reply("can 1").unwrap().0);
        assert!(can_reply("err unknown-command").is_none());
    }
}
