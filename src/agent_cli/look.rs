//! Pictures: of the desktop, a window or a part of it, or a scene's item.

use super::*;

const LOOK: Spec = Spec { window: Needs::Maybe, element: true, flags: &["--hidpi", "--first"], values: &["--text", "-o", "--pad"], maybe: &[], raw: false, shell: false };

fn default_out() -> String {
    format!("{}/pleamar-agent-look.png", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()))
}

/// `look [PID] [@X|--text T|X Y W H] [--pad N] [--hidpi] [-o FILE]`; old,
/// the file as the last word (`look PID FILE`).
pub(super) fn look(words: &[String]) -> R {
    let a = parse(words, &LOOK)?;
    let hidpi = a.has("--hidpi");
    let ints = |w: &[String]| w.iter().all(|v| v.parse::<i32>().is_ok());
    let (cut, file) = match &a.words[..] {
        [] => (None, None),
        [f] => (None, Some(f.clone())),
        [b @ .., f] if b.len() == 4 && ints(b) => (Some(b.join(" ")), Some(f.clone())),
        b if b.len() == 4 && ints(b) => (Some(b.join(" ")), None),
        _ => return Err(usage("look PID [@X | --text T | X Y W H] [--pad N] [--hidpi] [-o FILE]")),
    };
    let out = a.out().map(str::to_owned).or(file).unwrap_or_else(default_out);
    let Some(w) = &a.window else {
        // At scale 1: grim's own default is the greatest monitor's scale
        // (a HiDPI monitor, the phone's), and the boxes are the desktop's units.
        let shot = std::process::Command::new("grim").args(["-s", "1", &out]).output().map_err(|e| format!("grim: {e} (install grim to look)"))?;
        if !shot.status.success() {
            return Err(format!("grim: {}", String::from_utf8_lossy(&shot.stderr).trim()).into());
        }
        let (w, h) = image::image_dimensions(&out).map_err(|e| e.to_string())?;
        println!("{out} {w}x{h}");
        return Ok(());
    };
    let hi = if hidpi { " hidpi" } else { "" };
    let pad = a.number::<u32>("--pad")?.unwrap_or(0);
    if a.element.is_some() || a.text().is_some() {
        if cut.is_some() || (a.element.is_some() && a.text().is_some()) {
            return Err(usage("look PID takes one of @X, --text T or X Y W H"));
        }
        return look_at(w, &a, hi, pad, &out);
    }
    // The window manager's picture of it: what it drew, covered or
    // on another workspace, with its menus and nothing over it.
    let reply = Hands::open()?.say(&format!("c {}{hi}{}", w.target(), cut.map_or(String::new(), |c| format!(" {c}"))))?;
    let (pw, ph, age, rest) = picture(&reply, &out, &w.pid, None)?;
    // SCALE only with hidpi; X Y only when a menu reaches above or left of the window.
    let (scale, corner) = if hidpi { (rest.first().cloned(), rest.get(1..).unwrap_or_default().to_vec()) } else { (None, rest) };
    let corner = match &corner[..] {
        [x, y] => format!(" at {x},{y} (a menu reaches past the window: add these to the picture's coordinates for click)"),
        _ => String::new(),
    };
    // A window that has not drawn for a while may be behind what it would show now.
    let old = age.filter(|a| *a > 10_000).map_or(String::new(), |a| format!(" (drawn {}s ago)", a / 1000));
    println!("{out} {pw}x{ph}{}{corner}{old}", scale.map_or(String::new(), |s| format!(" scale {s}")));
    Ok(())
}

/// A `picture PATH W H AGE ...` answer, the picture saved to `out`: its
/// size, its age in ms and what came after it.
fn picture(reply: &str, out: &str, w: &str, e: Option<&str>) -> R<(u32, u32, Option<u64>, Vec<String>)> {
    let head = reply.split(" | ").next().unwrap_or_default();
    let f: Vec<&str> = match head.strip_prefix("picture ") {
        Some(p) => p.split_whitespace().collect(),
        None if head == "err timeout" => return Err(answered("no-picture", Of::Window(w))),
        None => return Err(answered(head, e.map_or(Of::Window(w), |e| Of::Element(w, e)))),
    };
    let [path, _, _, age, ref rest @ ..] = f[..] else { return Err(reply.into()) };
    std::fs::copy(path, out).map_err(|e| format!("{out}: {e}"))?;
    let (pw, ph) = image::image_dimensions(out).map_err(|e| e.to_string())?;
    Ok((pw, ph, age.parse().ok(), rest.iter().map(|s| s.to_string()).collect()))
}

/// An item of a scene, cut from its window's picture at its box.
fn look_at(w: &Window, a: &Args, hi: &str, pad: u32, out: &str) -> R {
    let k = Kinds::of(w)?;
    k.open()?;
    if !k.is_scene() {
        return Err(k.refuse("look @X"));
    }
    let read = described(&k)?;
    let rec = match (&a.element, a.text()) {
        (Some(e), _) => read.recs.iter().find(|r| r.id == e.said()).ok_or_else(|| answered("gone", Of::Element(&k.name, &e.said())))?,
        (None, Some(t)) => one(&read.recs.iter().filter(|r| r.named(t)).collect::<Vec<_>>(), t, a.first(), &k.name)?,
        _ => unreachable!(),
    };
    let [x, y, bw, bh] = rec.rect.ok_or_else(|| Fail::from(format!("no box: {} is in a menu whose place is not known: look at the whole window", rec.id)))?;
    let p = pad as i64;
    let reply = Hands::open()?.say(&format!("c {}{hi} {} {} {} {}", w.target(), x - p, y - p, bw + 2 * p, bh + 2 * p))?;
    let (pw, ph, _, rest) = picture(&reply, out, &w.pid, Some(&rec.id))?;
    let scale = if hi.is_empty() { String::new() } else { rest.first().map_or(String::new(), |s| format!(" scale {s}")) };
    println!("{out} {pw}x{ph}{scale} at {},{}", x - p, y - p);
    Ok(())
}
