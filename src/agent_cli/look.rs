//! Pictures: of the desktop, or of a window cut from it.

use super::*;

const LOOK: Spec = Spec { window: Needs::Maybe, element: false, flags: &[], values: &["-o"], maybe: &[], raw: false };

fn default_out() -> String {
    format!("{}/pleamar-agent-look.png", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()))
}

/// `look [PID] [-o FILE]`; old, the file as the last word (`look PID FILE`).
pub(super) fn look(words: &[String]) -> R {
    let a = parse(words, &LOOK)?;
    let file = match &a.words[..] {
        [] => None,
        [f] => Some(f.clone()),
        _ => return Err(usage("look [PID] [-o FILE]")),
    };
    let out = a.out().map(str::to_owned).or(file).unwrap_or_else(default_out);
    // At scale 1: grim's own default is the greatest monitor's scale
    // (a HiDPI monitor, the phone's), and the boxes are the desktop's units.
    let shot = std::process::Command::new("grim").args(["-s", "1", "-"]).output().map_err(|e| format!("grim: {e} (install grim to look)"))?;
    if !shot.status.success() {
        return Err(format!("grim: {}", String::from_utf8_lossy(&shot.stderr).trim()).into());
    }
    let desktop = image::load_from_memory(&shot.stdout).map_err(|e| e.to_string())?.to_rgba8();
    let picture = match &a.window {
        None => desktop,
        Some(w) => {
            let k = Kinds::of(w)?;
            k.open()?;
            let reply = Hands::open()?.say(&format!("r {}", w.pid))?;
            let f: Vec<i64> = reply.strip_prefix("rect ").ok_or_else(|| answered(&reply, Of::Window(&k.name)))?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            let [x, y, w, h, seen, ..] = f[..] else { return Err(reply.into()) };
            if seen == 0 || w <= 0 || h <= 0 {
                return Err(format!("not seen: {} is not seen now (another workspace, put away): `focus {}` it first", k.name, k.name).into());
            }
            // Cut at its box, what falls off the desktop left black: a
            // pixel of the picture stays the point `click` takes.
            let mut cut = image::RgbaImage::from_pixel(w as u32, h as u32, image::Rgba([0, 0, 0, 255]));
            for py in 0..h {
                for px in 0..w {
                    let (dx, dy) = (x + px, y + py);
                    if dx >= 0 && dy >= 0 && (dx as u32) < desktop.width() && (dy as u32) < desktop.height() {
                        cut.put_pixel(px as u32, py as u32, *desktop.get_pixel(dx as u32, dy as u32));
                    }
                }
            }
            cut
        }
    };
    picture.save(&out).map_err(|e| format!("{out}: {e}"))?;
    println!("{out} {}x{}", picture.width(), picture.height());
    Ok(())
}
