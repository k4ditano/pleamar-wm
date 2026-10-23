//! The agent's pointer at points of a window's picture: `move`, `click`,
//! `drag`, `scroll`; but `move`, each answers as an action does (act.rs).

use super::*;

pub(super) fn go(verb: &str, words: &[String]) -> R {
    let (words, until) = act::split_until(words);
    let a = parse(&words, if verb == "move" { &PLAIN } else { &act::POINTER })?;
    let w = a.win()?.clone();
    let (p, n) = (&w.pid, |k: usize, what: &str| number(a.word(k), what));
    let target = w.target();
    if verb == "move" {
        return Hands::open()?.arrive(p, n(0, "x")?, n(1, "y")?);
    }
    let k = Kinds::of(&w)?;
    k.open()?;
    let mut act = act::Acting::take(&k, &a, until, Hands::open()?)?;
    let h = &mut act.h;
    let (said, done) = match verb {
        "click" => {
            let (x, y, b) = (n(0, "x")?, n(1, "y")?, button(a.word(2))?);
            let count = a.word(3).and_then(|c| c.parse::<u32>().ok()).unwrap_or(1).clamp(1, 3);
            let done = h.arrive(p, x, y).and_then(|_| {
                for _ in 0..count {
                    h.act(&format!("b {target} 0 {b} 1"))?;
                    h.act(&format!("b {target} 0 {b} 0"))?;
                }
                Ok(())
            });
            (format!("clicked {x},{y}{}", act::how(b, count)), done)
        }
        "drag" => {
            let (x1, y1, x2, y2) = (n(0, "x1")?, n(1, "y1")?, n(2, "x2")?, n(3, "y2")?);
            let done = h.arrive(p, x1, y1).and_then(|_| {
                h.act(&format!("b {target} 0 272 1"))?;
                let (steps, _) = strokes(h.pace);
                for k in 1..=steps {
                    let t = k as f64 / steps as f64;
                    h.act(&format!("m {target} 0 {:.1} {:.1}", x1 + (x2 - x1) * t, y1 + (y2 - y1) * t))?;
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                h.act(&format!("b {target} 0 272 0"))
            });
            (format!("dragged {x1},{y1} to {x2},{y2}"), done)
        }
        _ => {
            let (x, y) = (n(0, "x")?, n(1, "y")?);
            let way = a.word(2).unwrap_or("down");
            let (axis, value) = match way {
                "up" => (0, -15.0),
                "down" => (0, 15.0),
                "left" => (1, -15.0),
                "right" => (1, 15.0),
                other => return Err(usage(format!("scroll up, down, left or right, not {other:?}"))),
            };
            let steps = a.word(3).and_then(|s| s.parse::<u32>().ok()).unwrap_or(3).clamp(1, 50);
            let done = h.arrive(p, x, y).and_then(|_| {
                for _ in 0..steps {
                    h.act(&format!("a {target} 0 {axis} {value}"))?;
                    std::thread::sleep(std::time::Duration::from_millis(strokes(h.pace).1));
                }
                Ok(())
            });
            (format!("scrolled {way} {steps} at {x},{y}"), done)
        }
    };
    act.answer(&said, done.map(|_| String::new()))
}
