//! The session's windows (and what each does), its layers and monitors.

use super::*;

/// The monitors, as the session counts them: name, and box in units.
pub(crate) fn monitors() -> Result<Vec<(String, f64, f64, f64, f64)>, String> {
    let reply = Hands::open().map_err(|f| f.msg)?.say("o").map_err(|f| f.msg)?;
    let list = reply.strip_prefix("monitors").ok_or(reply.clone())?;
    let mut out = Vec::new();
    for entry in list.split('|').map(str::trim).filter(|e| !e.is_empty()) {
        let f: Vec<&str> = entry.split_whitespace().collect();
        if let [_, x, y, w, h, name, ..] = f[..] {
            let n = |v: &str| v.parse::<f64>().unwrap_or(0.0);
            out.push((name.to_owned(), n(x), n(y), n(w), n(h)));
        }
    }
    Ok(out)
}

/// `windows`: each window, its box, where it is seen, and what it does.
pub(super) fn windows() -> R {
    let reply = Hands::open()?.say("l")?;
    let list = reply.strip_prefix("windows").ok_or(reply.clone())?;
    let entries: Vec<Vec<&str>> = list.split('|').map(|e| e.split_whitespace().collect::<Vec<&str>>()).filter(|f| f.len() >= 9).collect();
    let (locked, can) = kind::can_reply(&wm("C")?).unwrap_or((false, Vec::new()));
    let scenes = scenes();
    for f in &entries {
        // A program with several windows: each by its own name.
        let several = entries.iter().filter(|e| e[0] == f[0]).count() > 1;
        let name = match f.get(11) {
            Some(slot) if several => format!("{}.{slot}", f[0]),
            _ => f[0].to_owned(),
        };
        // Not shown, it has no box: it is nowhere.
        let seen = match f.get(9) {
            Some(m) if f[5] == "1" && *m != "-1" => format!("{}x{} at {},{}  seen on monitor {m}", f[3], f[4], f[1], f[2]),
            _ if f[5] == "1" => format!("{}x{} at {},{}  seen", f[3], f[4], f[1], f[2]),
            _ => "hidden".to_owned(),
        };
        let keys = if f[6] == "1" { " \u{b7} has the keyboard" } else { "" };
        let dialog = match f.get(10) {
            Some(p) if *p != "0" => format!(" \u{b7} dialog of {p}"),
            _ => String::new(),
        };
        let pid = f[0].parse::<u32>().unwrap_or(0);
        let c = can.iter().find(|c| c.pid == pid && f.get(11).is_some_and(|s| *s == c.slot.to_string()));
        let k = Kinds {
            name: name.clone(),
            scene: scenes.get(&pid).cloned(),
            private: c.is_some_and(|c| c.private),
            locked,
            popups: Vec::new(),
            slot: c.map(|c| c.slot),
        };
        println!("{name:>8}  {}  \u{ab}{}\u{bb}  {seen}{keys}{dialog}  does {}", unhex(f[7]), unhex(f[8]), k.does());
    }
    Ok(())
}

/// `layers`: the programs' own surfaces (bars, docks, a shell's panels).
pub(super) fn layers() -> R {
    let reply = Hands::open()?.say("y")?;
    let list = reply.strip_prefix("layers").ok_or(reply.clone())?;
    for entry in list.split('|').map(str::trim).filter(|e| !e.is_empty()) {
        let f: Vec<&str> = entry.split_whitespace().collect();
        if f.len() < 12 {
            continue;
        }
        // id pid monitor x y w h level hidden namespace program scene
        let scene = unhex(f[11]);
        let program = unhex(f[10]);
        let who = if scene.is_empty() || scene == "-" { program } else { format!("{program} {scene}") };
        let level = ["background", "bottom", "top", "overlay"].get(f[7].parse::<usize>().unwrap_or(2)).copied().unwrap_or("top");
        let hidden = if f[8] == "1" { " \u{b7} hidden" } else { "" };
        println!("{:>5}  {who}  \u{ab}{}\u{bb}  {}x{} at {},{}  monitor {}  {level}  pid {}{hidden}", f[0], unhex(f[9]), f[5], f[6], f[3], f[4], f[2], f[1]);
    }
    Ok(())
}

/// `hide N`, `show N`: one of those off the screen for now, or back.
pub(super) fn hide(verb: &str, words: &[String]) -> R {
    let id = words.first().filter(|n| n.parse::<u32>().is_ok()).ok_or_else(|| usage("which one: its number, as `pleamar-wm agent layers` says it"))?;
    let reply = Hands::open()?.say(&format!("h {id} {}", u8::from(verb == "hide")))?;
    if reply != "ok" {
        return Err(answered(&reply, Of::Window(id)));
    }
    Ok(())
}

pub(super) fn print_monitors() -> R {
    let reply = Hands::open()?.say("o")?;
    let list = reply.strip_prefix("monitors").ok_or(reply.clone())?;
    for entry in list.split('|').map(str::trim).filter(|e| !e.is_empty()) {
        let f: Vec<&str> = entry.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        println!("{}  {}  {}x{} at {},{}", f[0], f[5], f[3], f[4], f[1], f[2]);
    }
    Ok(())
}
