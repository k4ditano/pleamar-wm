//! `pleamar-wm agent …`: the agent's hands from a shell, for an AI agent (or
//! anyone) that wants to use the desktop without writing the protocol. It
//! speaks `cua-inject v1` (see `agent.rs`) to the session's socket.
//!
//! A window is named by its process (`pleamar-wm agent windows` lists them).
//! Its coordinates are the pixels of `pleamar-wm agent look PID`: what is seen
//! in that picture at (x, y) is where `click PID x y` lands.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

const HELP: &str = "pleamar-wm agent — use the desktop with the agent's own pointer and keyboard
(needs `agent on` in ~/.config/pleamar/session.conf; your mouse and keyboard stay yours)

  windows                         every window: its name (its process, or PID.N for one of a
                                  program's several windows), box, monitor, keyboard, program,
                                  title, and «dialog of PID» for a dialog (a «Save as» is its own)
  monitors                        the monitors, numbered as `windows` and `send` count them
  look [PID] [FILE]               a picture of that window (or of the whole desktop); prints the file
                                  (a program with a dialog open: the dialog, which is where it listens)
  move PID X Y                    the agent's cursor to X, Y of the window's picture
  click PID X Y [left|right|middle] [COUNT]
  drag PID X1 Y1 X2 Y2            press at one point, glide to the other, let go
  scroll PID X Y up|down|left|right [STEPS]
  type PID TEXT                   text (accents, ñ, emoji too), typed into the window without taking the keyboard
  type PID -                      the same, the text read from the input (for a password)
  key PID NAME                    enter tab escape backspace space up down left right delete home end pageup pagedown f1…f12
  hotkey PID MODS+KEY             ctrl+l, ctrl+shift+t, alt+f4 …
  open [--monitor N] COMMAND…      start a program for the agent: the monitor it works on lights
                                  first, and the window opens there without taking your keyboard
                                  (the monitor: N, or the one it works on, or one you are not on)
  focus PID                       give that window your keyboard (and show its workspace)
  send PID MONITOR                that window to another monitor (`monitors` numbers them)
  done                            finished: the light on the monitor goes out now (by itself it
                                  waits a minute and a half, in case the agent is thinking)
  stop                            the user's: the agent stops, and what it tries next is refused
                                  until it says `done` (or for a minute)
  raw LINE…                       protocol lines, as they are (cua-inject v1)

A window made with pleamar (`windows` says «pleamar scene NAME») is asked and used by name:
  tree PID [json]                 what is on it: every button, slider, field, list, item and text,
                                  with its name, what it says, its state and its box
  press PID NAME [right|middle] [COUNT]
                                  your cursor goes to it and it is pressed; the answer is what
                                  happened (events, facts, texts, what opened)
  wait PID CONDITION [TIMEOUT]    answers as soon as it holds: status == \"Saved\", dirty == false 3s
  watch PID [SECONDS]             a line for each thing that happens on it
  say PID ORDER…                  any other order to the scene: type query words, drag knob 0 -40,
                                  hold card, wheel list -3, key escape

Look before each click: a page moves under you. A pleamar window does not need it: ask it.";

/// Where the session's socket is.
fn socket() -> Option<String> {
    if let Ok(p) = std::env::var("CUA_INJECT_SOCKET") {
        if !p.is_empty() {
            return Some(p);
        }
    }
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    let path = super::agent_socket_path(&display);
    std::path::Path::new(&path).exists().then_some(path)
}

/// One line to the session's socket, as no agent in particular (its cursor's
/// label is left as it is): the reply.
pub(crate) fn tell(line: &str) -> Result<String, String> {
    let path = socket().ok_or("no agent socket here")?;
    let stream = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    for l in ["cua-inject v1", line] {
        writeln!(writer, "{l}").map_err(|e| e.to_string())?;
        reply.clear();
        reader.read_line(&mut reply).map_err(|e| e.to_string())?;
    }
    Ok(reply.trim_end().to_owned())
}

// ── pleamar windows: asked and used by name ──────────────────────

/// Where the session's pleamar programs listen: what the session told them
/// (`PLEAMAR_SOCKETS`), or the folder it gives them.
fn scene_sockets() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    let (run, display) = (std::env::var("XDG_RUNTIME_DIR").ok()?, std::env::var("WAYLAND_DISPLAY").ok()?);
    Some(format!("{run}/pleamar-{display}").into())
}

/// One order to a scene's socket, and its answer as it comes; `each` gets
/// every line (a `watch` goes on talking).
fn order(sock: &std::path::Path, line: &str, each: &mut dyn FnMut(&str)) -> Result<(), String> {
    order_within(sock, line, if line.starts_with("watch") { None } else { Some(std::time::Duration::from_secs(65)) }, each)
}

fn order_within(sock: &std::path::Path, line: &str, patience: Option<std::time::Duration>, each: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("{}: {e}", sock.display()))?;
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let _ = s.shutdown(std::net::Shutdown::Write);
    let _ = s.set_read_timeout(patience);
    for l in BufReader::new(s).lines().map_while(Result::ok) {
        each(&l);
    }
    Ok(())
}

fn ask(sock: &std::path::Path, line: &str) -> Result<String, String> {
    let mut out = String::new();
    order(sock, line, &mut |l| {
        out.push_str(l);
        out.push('\n');
    })?;
    Ok(out)
}

/// Every pleamar program of the session that answers, by its process: its
/// scene's name and its socket. They are asked who they are (`hello`).
fn scenes() -> std::collections::HashMap<u32, (String, std::path::PathBuf)> {
    let mut found = std::collections::HashMap::new();
    let Some(dir) = scene_sockets() else { return found };
    let Ok(entries) = std::fs::read_dir(&dir) else { return found };
    for e in entries.filter_map(Result::ok) {
        let path = e.path();
        // The cursor's and the hands' own sockets live there too, and are no scene's.
        if path.extension().is_none_or(|x| x != "sock") || path.file_stem().is_some_and(|n| n == "cursor" || n == "cua-inject") {
            continue;
        }
        // `pleamar 0.2.25 · scene notes · pid 4521 · language 0.2`. Briefly: some
        // sockets there are not a scene's (the cursor's) and never answer.
        let mut hello = String::new();
        if order_within(&path, "hello", Some(std::time::Duration::from_millis(300)), &mut |l| hello.push_str(l)).is_err() {
            continue;
        }
        let field = |k: &str| hello.split(" · ").find_map(|f| f.strip_prefix(k)).map(|v| v.trim().to_owned());
        if let (Some(scene), Some(pid)) = (field("scene "), field("pid ").and_then(|p| p.parse::<u32>().ok())) {
            found.insert(pid, (scene, path));
        }
    }
    found
}

/// The scene of a window, named as `windows` names it (`4521`, `4521.2`).
fn scene_of(pid: &str) -> Result<(String, std::path::PathBuf), String> {
    let n: u32 = pid.split('.').next().and_then(|p| p.parse().ok()).ok_or("which window: its number")?;
    scenes().remove(&n).ok_or_else(|| format!("{pid} is not a pleamar window, or it does not answer: use look and click"))
}

/// The monitors, as the session counts them: name, and box in units.
pub(crate) fn monitors() -> Result<Vec<(String, f64, f64, f64, f64)>, String> {
    let reply = Hands::open()?.say("o")?;
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

struct Hands {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Hands {
    fn open() -> Result<Self, String> {
        let path = socket().ok_or("no agent socket here: is this a pleamar-wm session with `agent on` in session.conf?")?;
        let stream = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut hands = Hands { reader: BufReader::new(stream), writer };
        let hello = hands.say("cua-inject v1")?;
        if hello != "cua-inject v1" {
            return Err(format!("the socket answered «{hello}»"));
        }
        // Who it is, for the cursor's label: `PLEAMAR_AGENT_NAME` (Marea
        // says hers), «agent» otherwise. An older session does not know
        // it: nothing is lost.
        let name = std::env::var("PLEAMAR_AGENT_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "agent".to_owned());
        let _ = hands.say(&format!("n {}", hex(&name)));
        Ok(hands)
    }

    fn say(&mut self, line: &str) -> Result<String, String> {
        writeln!(self.writer, "{line}").map_err(|e| e.to_string())?;
        let mut reply = String::new();
        self.reader.read_line(&mut reply).map_err(|e| e.to_string())?;
        Ok(reply.trim_end().to_owned())
    }

    /// The cursor to a point of that window, the way a hand takes it there:
    /// along the way, eased, with the motions a pointer makes. It is seen
    /// travelling (not jumping), and programs that only light what is under
    /// the pointer when it moves over it (a browser's menu: it took no press
    /// on an item it had not seen the pointer move over) see it move.
    fn arrive(&mut self, pid: &str, x: f64, y: f64) -> Result<(), String> {
        let numbers = |r: &str, head: &str| -> Option<Vec<f64>> { r.strip_prefix(head).map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect()) };
        let from = self.say("p 0")?;
        let rect = self.say(&format!("r {pid}"))?;
        let (f, r) = (numbers(&from, "at ").filter(|f| f.len() >= 2), numbers(&rect, "rect ").filter(|r| r.len() >= 4));
        // From where it is, if that is on this window; else from a little
        // before the point, so there is a way to go.
        let start = match (&f, &r) {
            (Some(f), Some(r)) => {
                let (sx, sy) = (f[0] - r[0], f[1] - r[1]);
                (sx >= 0.0 && sy >= 0.0 && sx < r[2] && sy < r[3]).then_some((sx, sy))
            }
            _ => None,
        };
        let (w, h) = r.as_ref().map_or((f64::MAX, f64::MAX), |r| (r[2].max(1.0), r[3].max(1.0)));
        let (sx, sy) = start.unwrap_or(((x - 70.0).clamp(0.0, w - 1.0), (y - 45.0).clamp(0.0, h - 1.0)));
        let far = ((x - sx).powi(2) + (y - sy).powi(2)).sqrt();
        if far < 2.0 {
            return self.act(&format!("m root:{pid} 0 {x} {y}"));
        }
        let total = (220.0 + far * 0.45).clamp(260.0, 620.0);
        let steps = ((far / 22.0).round() as u32).clamp(8, 24);
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let e = t * t * (3.0 - 2.0 * t);
            let (px, py) = (sx + (x - sx) * e, sy + (y - sy) * e);
            if k < steps {
                // On the way it may cross where nothing of the window is
                // (its shadow): those steps are only skipped.
                let _ = self.say(&format!("m root:{pid} 0 {px:.1} {py:.1}"));
                std::thread::sleep(std::time::Duration::from_millis((total / steps as f64) as u64));
            } else {
                self.act(&format!("m root:{pid} 0 {x} {y}"))?;
            }
        }
        // A moment on the spot, as a hand stops before it presses.
        std::thread::sleep(std::time::Duration::from_millis(70));
        Ok(())
    }

    /// A command that has to be answered `ok`.
    fn act(&mut self, line: &str) -> Result<(), String> {
        match self.say(line)? {
            r if r == "ok" => Ok(()),
            r if r == "err stopped-by-user" => Err("the user stopped the agent: stop here and tell them where you left it (`pleamar-wm agent done` lets the hands be used again)".to_owned()),
            r => Err(format!("{line}: {r}")),
        }
    }
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> String {
    if s == "-" {
        return String::new();
    }
    let bytes: Vec<u8> = (0..s.len() / 2).filter_map(|k| u8::from_str_radix(&s[2 * k..2 * k + 2], 16).ok()).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn button(name: Option<&String>) -> Result<u32, String> {
    match name.map(String::as_str).unwrap_or("left") {
        "left" => Ok(272),
        "right" => Ok(273),
        "middle" => Ok(274),
        other => Err(format!("a button is left, right or middle, not «{other}»")),
    }
}

fn number(s: Option<&String>, what: &str) -> Result<f64, String> {
    s.and_then(|v| v.parse::<f64>().ok()).ok_or_else(|| format!("{what}: a number"))
}

pub fn run(args: &[String]) -> i32 {
    match go(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("agent · {e}");
            1
        }
    }
}

fn go(args: &[String]) -> Result<(), String> {
    let Some(what) = args.first() else {
        println!("{HELP}");
        return Ok(());
    };
    // A window: its process (`4521`), or one of a program's several windows (`4521.3`), as `windows` names it.
    let pid = || {
        args.get(1)
            .filter(|p| match p.split_once('.') {
                Some((a, b)) => a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok(),
                None => p.parse::<u32>().is_ok(),
            })
            .cloned()
            .ok_or("which window: its number, as `pleamar-wm agent windows` names it".to_owned())
    };
    let target = |pid: &str| format!("root:{pid}");
    match what.as_str() {
        "help" | "--help" | "-h" => println!("{HELP}"),
        "windows" => {
            let reply = Hands::open()?.say("l")?;
            let list = reply.strip_prefix("windows").ok_or(reply.clone())?;
            let entries: Vec<Vec<&str>> = list.split('|').map(|e| e.split_whitespace().collect::<Vec<&str>>()).filter(|f| f.len() >= 9).collect();
            let scenes = scenes();
            for f in &entries {
                // A program with several windows: each by its own name.
                let several = entries.iter().filter(|e| e[0] == f[0]).count() > 1;
                let name = match f.get(11) {
                    Some(slot) if several => format!("{}.{slot}", f[0]),
                    _ => f[0].to_owned(),
                };
                let seen = match f.get(9) {
                    Some(m) if f[5] == "1" && *m != "-1" => format!("seen on monitor {m}"),
                    _ if f[5] == "1" => "seen".to_owned(),
                    _ => "hidden".to_owned(),
                };
                let keys = if f[6] == "1" { " · has the keyboard" } else { "" };
                let dialog = match f.get(10) {
                    Some(p) if *p != "0" => format!(" · dialog of {p}"),
                    _ => String::new(),
                };
                // A pleamar window: it can be asked (`tree`) and used by name (`press`).
                let scene = match f[0].parse::<u32>().ok().and_then(|p| scenes.get(&p)) {
                    Some((n, _)) => format!(" · pleamar scene {n}: tree, press"),
                    None => String::new(),
                };
                println!("{name:>8}  {}  «{}»  {}x{} at {},{}  {seen}{keys}{dialog}{scene}", unhex(f[7]), unhex(f[8]), f[3], f[4], f[1], f[2]);
            }
        }
        "monitors" => {
            let reply = Hands::open()?.say("o")?;
            let list = reply.strip_prefix("monitors").ok_or(reply.clone())?;
            for entry in list.split('|').map(str::trim).filter(|e| !e.is_empty()) {
                let f: Vec<&str> = entry.split_whitespace().collect();
                if f.len() < 6 {
                    continue;
                }
                println!("{}  {}  {}x{} at {},{}", f[0], f[5], f[3], f[4], f[1], f[2]);
            }
        }
        "open" => {
            let mut rest: Vec<String> = args[1..].to_vec();
            let mut monitor = -1i64;
            if rest.first().map(String::as_str) == Some("--monitor") {
                monitor = rest.get(1).and_then(|m| m.parse().ok()).ok_or("which monitor: its number (pleamar-wm agent monitors)")?;
                rest.drain(..2);
            }
            if rest.first().map(String::as_str) == Some("--") {
                rest.remove(0);
            }
            let command = rest.join(" ");
            if command.trim().is_empty() {
                return Err("what to open: pleamar-wm agent open firefox".into());
            }
            let reply = Hands::open()?.say(&format!("L {monitor} {}", hex(&command)))?;
            let f: Vec<&str> = reply.split_whitespace().collect();
            match f.as_slice() {
                ["opened", pid, screen] => println!("opened on monitor {screen} (process {pid}): its window opens there, without the user's keyboard. Find it with `pleamar-wm agent windows`."),
                _ => return Err(format!("open: {reply}")),
            }
        }
        "send" => {
            let p = pid()?;
            let monitor = args.get(2).filter(|m| m.parse::<usize>().is_ok()).ok_or("which monitor: its number (pleamar-wm agent monitors)")?;
            Hands::open()?.act(&format!("v {p} {monitor}"))?;
        }
        "look" => {
            let out = args.get(2).cloned().unwrap_or_else(|| {
                let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
                format!("{dir}/pleamar-agent-look.png")
            });
            // At scale 1: grim's own default is the greatest monitor's scale
            // (a HiDPI monitor, the phone's), and the boxes are the desktop's units.
            let shot = std::process::Command::new("grim").args(["-s", "1", "-"]).output().map_err(|e| format!("grim: {e} (install grim to look)"))?;
            if !shot.status.success() {
                return Err(format!("grim: {}", String::from_utf8_lossy(&shot.stderr).trim()));
            }
            let desktop = image::load_from_memory(&shot.stdout).map_err(|e| e.to_string())?.to_rgba8();
            let picture = match pid().ok() {
                None => desktop,
                Some(p) => {
                    let reply = Hands::open()?.say(&format!("r {p}"))?;
                    let f: Vec<i64> = reply.strip_prefix("rect ").ok_or(reply.clone())?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                    let [x, y, w, h, seen] = f[..] else { return Err(reply) };
                    if seen == 0 || w <= 0 || h <= 0 {
                        return Err("that window is not seen now (another workspace, put away): `focus` it first".into());
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
        }
        "move" => {
            let p = pid()?;
            Hands::open()?.arrive(&p, number(args.get(2), "x")?, number(args.get(3), "y")?)?;
        }
        "click" => {
            let p = pid()?;
            let (x, y) = (number(args.get(2), "x")?, number(args.get(3), "y")?);
            let b = button(args.get(4))?;
            let count = args.get(5).and_then(|c| c.parse::<u32>().ok()).unwrap_or(1).clamp(1, 3);
            let mut h = Hands::open()?;
            h.arrive(&p, x, y)?;
            for _ in 0..count {
                h.act(&format!("b {} 0 {b} 1", target(&p)))?;
                h.act(&format!("b {} 0 {b} 0", target(&p)))?;
            }
        }
        "drag" => {
            let p = pid()?;
            let (x1, y1, x2, y2) = (number(args.get(2), "x1")?, number(args.get(3), "y1")?, number(args.get(4), "x2")?, number(args.get(5), "y2")?);
            let mut h = Hands::open()?;
            h.arrive(&p, x1, y1)?;
            h.act(&format!("b {} 0 272 1", target(&p)))?;
            for k in 1..=24 {
                let t = k as f64 / 24.0;
                h.act(&format!("m {} 0 {:.1} {:.1}", target(&p), x1 + (x2 - x1) * t, y1 + (y2 - y1) * t))?;
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            h.act(&format!("b {} 0 272 0", target(&p)))?;
        }
        "scroll" => {
            let p = pid()?;
            let (x, y) = (number(args.get(2), "x")?, number(args.get(3), "y")?);
            let (axis, value) = match args.get(4).map(String::as_str).unwrap_or("down") {
                "up" => (0, -15.0),
                "down" => (0, 15.0),
                "left" => (1, -15.0),
                "right" => (1, 15.0),
                other => return Err(format!("scroll up, down, left or right, not «{other}»")),
            };
            let steps = args.get(5).and_then(|s| s.parse::<u32>().ok()).unwrap_or(3).clamp(1, 50);
            let mut h = Hands::open()?;
            h.arrive(&p, x, y)?;
            for _ in 0..steps {
                h.act(&format!("a {} 0 {axis} {value}", target(&p)))?;
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
        }
        "type" => {
            let p = pid()?;
            // `-`: the text from the input, not the command line (a password
            // must not be seen in the list of processes).
            let text = if args.len() == 3 && args[2] == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
                s.strip_suffix('\n').map(str::to_owned).unwrap_or(s)
            } else {
                args[2..].join(" ")
            };
            let mut h = Hands::open()?;
            // In pieces (one line of the protocol has its limits), whole
            // characters each: any text, accents and all.
            let chars: Vec<char> = text.chars().collect();
            for chunk in chars.chunks(500) {
                h.act(&format!("t {} {}", target(&p), hex(&chunk.iter().collect::<String>())))?;
            }
        }
        "key" => {
            let p = pid()?;
            let name = args.get(2).ok_or("which key")?;
            Hands::open()?.act(&format!("k {} {name}", target(&p)))?;
        }
        "hotkey" => {
            let p = pid()?;
            let combo = args.get(2).ok_or("which keys: ctrl+l")?;
            let mut parts: Vec<&str> = combo.split('+').collect();
            let key = parts.pop().filter(|k| !k.is_empty()).ok_or("which key, after the modifiers")?;
            if parts.is_empty() {
                Hands::open()?.act(&format!("k {} {key}", target(&p)))?;
            } else {
                Hands::open()?.act(&format!("h {} {} {key}", target(&p), parts.join(",")))?;
            }
        }
        "tree" => {
            let p = pid()?;
            let (_, sock) = scene_of(&p)?;
            let json = args.get(2).is_some_and(|a| a == "json");
            print!("{}", ask(&sock, if json { "describe json" } else { "describe" })?);
        }
        "press" => {
            let p = pid()?;
            let name = args.get(2).ok_or("press what: its name, as `tree` gives it")?;
            let (_, sock) = scene_of(&p)?;
            // The scene presses it by name, and before that takes the agent's
            // cursor there itself: it is never behind the press.
            let _ = name;
            print!("{}", ask(&sock, &format!("press {}", args[2..].join(" ")))?);
        }
        "wait" | "watch" | "say" => {
            let p = pid()?;
            let (_, sock) = scene_of(&p)?;
            let rest = args[2..].join(" ");
            let line = if what == "say" { rest } else { format!("{what} {rest}") };
            order(&sock, line.trim(), &mut |l| {
                println!("{l}");
                let _ = std::io::stdout().flush();
            })?;
        }
        "done" => Hands::open()?.act("x")?,
        "stop" => Hands::open()?.act("s")?,
        "focus" => {
            let p = pid()?;
            Hands::open()?.act(&format!("f {p}"))?;
        }
        "raw" => {
            let mut h = Hands::open()?;
            for line in &args[1..] {
                println!("{}", h.say(line)?);
            }
        }
        other => return Err(format!("I don't know «{other}»: pleamar-wm agent help")),
    }
    Ok(())
}
