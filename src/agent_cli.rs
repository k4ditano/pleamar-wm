//! `pleamar-wm agent …`: the agent's hands from a shell, for an AI agent (or
//! anyone) that wants to use the desktop without writing the protocol. It
//! speaks `cua-inject v1` (see `agent.rs`) to the session's socket.
//!
//! A window is named by its process (`pleamar-wm agent windows` lists them).
//! Its coordinates are the pixels of `pleamar-wm agent look PID`: what is seen
//! in that picture at (x, y) is where `click PID x y` lands.

use crate::config::Pace;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

const HELP: &str = "pleamar-wm agent — use the desktop with the agent's own pointer and keyboard
(needs `agent on` in ~/.config/pleamar/session.conf; your mouse and keyboard stay yours)

  windows                         every window: its name (its process, or PID.N for one of a
                                  program's several windows), box, monitor, keyboard, program,
                                  title, and «dialog of PID» for a dialog (a «Save as» is its own)
  monitors                        the monitors, numbered as `windows` and `send` count them
  layers                          the programs' own surfaces —bars, docks, widgets, a shell's
                                  panels—: number, program (and scene), name, box, monitor, level
  hide N · show N                 one of those off the screen for now, or back (its program runs on)
  look [PID] [FILE]               a picture of that window (or of the whole desktop); prints the file
                                  (a program with a dialog open: the dialog, which is where it listens).
                                  The window's own: covered or on another workspace, with its menus
  look PID X Y W H [FILE]         a part of it, in the picture's coordinates
  look PID ... --hidpi            at the window's own pixels; prints the scale (a pixel / scale is a point)
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
  pace [default|fast|instant]     how fast the hands go, for every agent of the session (without
                                  one, the pace now): fast glides quicker, instant goes there in
                                  one motion; the pill on the monitor says it when not default
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

Any other window is read from what its program publishes for screen readers (GTK, Qt, Firefox, Chromium…),
its elements numbered (@ID) and boxed in the picture's coordinates:
  tree PID [all] [json]           what is shown: role, @ID, name, text, box, states (focused, checked,
                                  disabled…), each under what it is in; `all`: what is not shown too
  tree PID --since [V] [--boxes] [json]
                                  only what changed since read V (by default the last you got of it):
                                  + came, - went, ~ other role, name, text or states (--boxes: box);
                                  `full` and the whole tree if the window shows another page or dialog
  read PID focus                  the element with the focus, and what it is in
  read PID at X Y                 the element at that point of the picture
  read PID find TEXT              the elements whose name or text has TEXT in it
  press PID @ID [right|middle] [COUNT]
                                  your cursor goes to it and clicks it, as `click` (scrolled into view
                                  first if it is not); at pace instant, its own action instead
  press PID @ID --action N        its action N (0, its default) without clicking: what cannot be clicked
  press PID \"TEXT\"                the same, on the first element named TEXT
  focus PID @ID                   the focus in its window to it (a field, to type in)
  type PID @ID TEXT|- [--set]     focus it and type; --set: its whole text set at once (long text;
                                  a program that checks each key typed may not see it)
  look PID --marks [all] [FILE]   a picture with what can be acted on outlined and its @ID on it
                                  (all: texts and headings too), and their records as `tree` gives them
  look PID @ID [--pad N] [FILE]   a picture of that element only (scrolled into view first), N units
                                  around it; prints where its corner is in the window's picture
  look PID \"TEXT\" [--pad N] [FILE]
                                  the same, of the first element that says TEXT (in a longer text,
                                  only those words); a FILE has a / or ends in .png
  wait PID CONDITION [SECONDS]    `ok` as soon as it holds (in 30 s, or SECONDS up to 60), and the element that
                                  met it; instead of looking again and again. A Stop ends it. CONDITION:
                                  has \"TEXT\", gone \"TEXT\", @ID gone, @ID STATE, @ID not STATE (enabled, busy,
                                  checked, a state `tree` gives), focus @ID, focus \"NAME\", changed (anything
                                  shown), settled MS (neither the tree nor the picture changed for MS), dialog
  listen PID [SECONDS]            what a screen reader would say, a line each as it happens (for 30 s, or
                                  SECONDS up to 600): focus (where it starts, then each move), text (of the
                                  focused element), live (a live region's news), announce, dialog. Ends at
                                  its time, when the window closes, or at a Stop

A terminal (`pleamar-wm term list`: its shell has pleamar-wm's layer) is read as text, history and all:
  read PID [N] [--since M]        its last N lines (200); the first says `mark M`, the newest line's
                                  number: --since M gives what came after it (or `screen` and the
                                  screen, if what was read was written over: a TUI)
  tree PID                        what runs in it, where, its size, how long it has been quiet; its screen
  wait PID has \"TEXT\"|output|quiet MS|prompt [SECONDS]
                                  `ok` and what came out meanwhile, as soon as it holds (in 30 s, or SECONDS); a Stop ends it
  run PID \"COMMAND\" [SECONDS]     typed, Enter, and what it printed once the shell waits again
  type PID TEXT, key PID NAME     through the terminal (ctrl+c too): it need not be seen nor have the keyboard
                                  (a key it does not know, as any other window's)
  ... PID --shell ID ...          a terminal with several shells (tabs, or one process with several
                                  windows) is refused until one is named: ID from `pleamar-wm term list`

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
    Line::open()?.say(line)
}

/// The session's socket kept open, for what is said many times a second (a
/// finger moving on a phone's glass): each line, and its reply.
pub(crate) struct Line {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Line {
    pub(crate) fn open() -> Result<Line, String> {
        let path = socket().ok_or("no agent socket here")?;
        let stream = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut line = Line { writer, reader: BufReader::new(stream) };
        line.say("cua-inject v1")?;
        Ok(line)
    }

    pub(crate) fn say(&mut self, line: &str) -> Result<String, String> {
        writeln!(self.writer, "{line}").map_err(|e| e.to_string())?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply).map_err(|e| e.to_string())? == 0 {
            return Err("the session hung up".into());
        }
        Ok(reply.trim_end().to_owned())
    }
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
    /// The session's pace, asked once.
    pace: Pace,
}

/// A glide of FAR units at a pace: its time in ms, its steps, and the moment
/// still before pressing; none, one motion.
fn glide(pace: Pace, far: f64) -> Option<(f64, u32, u64)> {
    match pace {
        Pace::Default => Some(((220.0 + far * 0.45).clamp(260.0, 620.0), ((far / 22.0).round() as u32).clamp(8, 24), 70)),
        Pace::Fast => Some(((60.0 + far * 0.12).clamp(80.0, 150.0), ((far / 80.0).round() as u32).clamp(4, 6), 0)),
        Pace::Instant => None,
    }
}

/// A drag's steps and the ms between scrolls at a pace.
fn strokes(pace: Pace) -> (u32, u64) {
    match pace {
        Pace::Default => (24, 60),
        Pace::Fast | Pace::Instant => (8, 20),
    }
}

impl Hands {
    fn open() -> Result<Self, String> {
        let path = socket().ok_or("no agent socket here: is this a pleamar-wm session with `agent on` in session.conf?")?;
        let stream = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut hands = Hands { reader: BufReader::new(stream), writer, pace: Pace::Default };
        let hello = hands.say("cua-inject v1")?;
        if hello != "cua-inject v1" {
            return Err(format!("the socket answered «{hello}»"));
        }
        // Who it is, for the cursor's label: `PLEAMAR_AGENT_NAME` (Marea
        // says hers), «agent» otherwise. An older session does not know
        // it: nothing is lost.
        let _ = hands.say(&format!("n {}", hex(&agent_name())));
        // How fast it goes is the session's (`T`); an older one: as ever.
        if let Some(p) = hands.say("T").unwrap_or_default().strip_prefix("pace ").and_then(Pace::parse) {
            hands.pace = p;
        }
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
        let Some((total, steps, still)) = glide(self.pace, far) else {
            // One motion, from a unit short of the point: a program sees the
            // pointer move over what is there, not only land on it.
            let _ = self.say(&format!("m root:{pid} 0 {:.1} {:.1}", x - (x - sx) / far, y - (y - sy) / far));
            return self.act(&format!("m root:{pid} 0 {x} {y}"));
        };
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
        std::thread::sleep(std::time::Duration::from_millis(still));
        Ok(())
    }

    /// A command that has to be answered `ok`.
    fn act(&mut self, line: &str) -> Result<(), String> {
        match self.say(line)? {
            r if r == "ok" => Ok(()),
            r if r == "err stopped-by-user" => Err(STOPPED.to_owned()),
            r => Err(format!("{line}: {r}")),
        }
    }
}

const STOPPED: &str = "the user stopped the agent: stop here and tell them where you left it (`pleamar-wm agent done` lets the hands be used again)";

// -- terminals: read, followed and typed into through their layer --------

/// A window whose process is a terminal with pleamar-wm's layer (term.rs):
/// its text, history included, is asked of the layer, not seen in pictures.
struct Term {
    pid: String,
    id: i32,
    socket: String,
}

/// What a terminal's wait is for.
enum Until {
    Has(String),
    Output,
    Quiet(u64),
    Prompt,
}

impl Term {
    /// That window's terminal, if its process has a layer, once the window
    /// manager has let the agent at it (`i`): it says no for a private
    /// window or a locked session, and for typing while stopped. A process
    /// with several shells (tabs, a terminal that serves many windows) does
    /// not say which is the window's: refused, with the shells, unless one
    /// is named (`--shell ID`, a layer whose terminal is this process).
    fn of(pid: &str, types: bool, shell: Option<i32>) -> Result<Option<Term>, String> {
        let n: u32 = pid.split('.').next().and_then(|p| p.parse().ok()).ok_or("which window: its number")?;
        let all = crate::term::layers();
        let mut shells = crate::term::shells_of(n, &all);
        if let Some(id) = shell {
            let (_, info) = all.iter().find(|(i, _)| *i == id).ok_or_else(|| format!("no shell {id}: `pleamar-wm term list` gives them"))?;
            if info.get("terminal").and_then(|t| t.parse::<u32>().ok()) != Some(n) {
                return Err(format!("shell {id} is not in {pid}'s terminal: `pleamar-wm term list` says the terminal of each"));
            }
            shells.retain(|(i, _)| *i == id);
        }
        let Some(id) = shells.first().map(|(id, _)| *id) else { return Ok(None) };
        Term::allowed(pid, types)?;
        if shells.len() > 1 {
            let list: Vec<String> = shells
                .iter()
                .map(|(id, i)| {
                    let g = |k: &str| i.get(k).map_or("", String::as_str);
                    format!("  --shell {id}  {}  {}  {:?}", g("cwd"), g("command"), g("title"))
                })
                .collect();
            return Err(format!("{pid}'s terminal has {} shells (tabs, or windows of one terminal) and which is this window's is not known: name one\n{}", shells.len(), list.join("\n")));
        }
        Ok(Some(Term { pid: pid.to_owned(), id, socket: format!("{}/{id}.sock", crate::term::socket_dir()) }))
    }

    /// While the agent is stopped: how long ago the user stopped it, in ms.
    fn allowed(pid: &str, types: bool) -> Result<Option<u64>, String> {
        let line = format!("i {pid}{}", if types { " w" } else { "" });
        match Hands::open()?.say(&line)?.as_str() {
            "ok" => Ok(None),
            r if r.starts_with("ok stopped ") => Ok(r["ok stopped ".len()..].parse().ok()),
            "err private" => Err(format!("{pid} is private (a `private` rule in session.conf), or the session is locked: its terminal is not read nor typed into")),
            "err stopped-by-user" => Err(STOPPED.to_owned()),
            r => Err(format!("{line}: {r}")),
        }
    }

    fn ask(&self, question: &str) -> Result<String, String> {
        match crate::term::ask(&self.socket, question)? {
            r if r.starts_with("error: ") => Err(r["error: ".len()..].trim_end().to_owned()),
            r => Ok(r),
        }
    }

    /// The number of its newest line (`mark` in a read), and what it says.
    fn mark(&self) -> Result<(u64, String), String> {
        let reply = self.ask("lines 1")?;
        let mut lines = reply.lines();
        let mark = lines.next().and_then(|l| l.strip_prefix("mark ")).and_then(|m| m.parse().ok()).ok_or(reply.clone())?;
        Ok((mark, lines.next().unwrap_or_default().to_owned()))
    }

    /// Whether what came after the mark `from` (whose line said `last`) has
    /// that text: the new lines, and of the mark's line only what was written
    /// on it since; the screen when that was written over (`screen`).
    fn has(&self, text: &str, from: u64, last: &str) -> Result<bool, String> {
        let reply = self.ask(&format!("lines {} {from} peek", crate::term::HISTORY))?;
        let mut lines = reply.lines();
        let mark = lines.next().and_then(|l| l.strip_prefix("mark ")).and_then(|m| m.parse::<u64>().ok()).unwrap_or(from);
        let body: Vec<&str> = lines.collect();
        Ok(match body.split_first() {
            Some((&"screen", screen)) => screen.iter().any(|l| l.contains(text)),
            _ => body.iter().enumerate().any(|(k, l)| {
                let again = k == 0 && body.len() as u64 == mark.saturating_sub(from) + 1;
                if again { l.strip_prefix(last).unwrap_or(l) } else { l }.contains(text)
            }),
        })
    }

    fn go(&self, what: &str, rest: &[String]) -> Result<(), String> {
        let seconds = |s: Option<&String>| s.map_or(Ok(30.0), |s| s.parse::<f64>().map_err(|_| format!("{s}: seconds, a number")));
        match what {
            "read" => {
                let since = rest.iter().position(|a| a == "--since");
                let after = match since {
                    Some(k) => format!(" {}", rest.get(k + 1).and_then(|m| m.parse::<u64>().ok()).ok_or("--since M: a mark, as a read's first line says it")?),
                    None => String::new(),
                };
                let n = rest.iter().enumerate().filter(|(k, _)| since.is_none_or(|s| *k != s && *k != s + 1)).find_map(|(_, a)| a.parse::<usize>().ok()).unwrap_or(200);
                print!("{}", self.ask(&format!("lines {n}{after}"))?);
            }
            "tree" => {
                let info = crate::term::info(&self.socket)?;
                println!("terminal {} (layer {})", self.pid, self.id);
                for k in ["command", "cwd", "size", "fullscreen", "quiet"] {
                    println!("{k} {}", info.get(k).map_or("", String::as_str));
                }
                println!();
                print!("{}", self.ask("screen")?);
            }
            "wait" => {
                let (until, secs) = match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    ["has", text, ..] => (Until::Has(text.to_owned()), rest.get(2)),
                    ["output", ..] => (Until::Output, rest.get(1)),
                    ["quiet", ms, ..] => (Until::Quiet(ms.parse().map_err(|_| "quiet MS: milliseconds")?), rest.get(2)),
                    ["prompt", ..] => (Until::Prompt, rest.get(1)),
                    _ => return Err("wait PID has \"TEXT\" | output | quiet MS | prompt [SECONDS]".into()),
                };
                let (start, from) = (std::time::Instant::now(), self.mark()?);
                self.follow(&until, seconds(secs)?, start, &from, false)?;
            }
            "run" => {
                let command = rest.first().filter(|c| !c.trim().is_empty()).ok_or("run what: run PID \"ls -l\"")?;
                let (start, from) = (std::time::Instant::now(), self.mark()?);
                self.ask(&format!("paste {}", hex(command)))?;
                self.ask("keys enter")?;
                self.follow(&Until::Prompt, seconds(rest.get(1))?, start, &from, true)?;
            }
            "type" => {
                self.ask(&format!("paste {}", hex(&typed(rest)?)))?;
            }
            "key" | "hotkey" => {
                self.ask(&format!("keys {}", rest.first().ok_or("which key")?))?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Until it holds, or for so many seconds: then `ok` and what came out
    /// after the mark `from` (as a read's answer). `run`: without the `ok`,
    /// and the prompt counts only once something came out (the command was
    /// taken).
    fn follow(&self, until: &Until, secs: f64, start: std::time::Instant, (from, last): &(u64, String), run: bool) -> Result<(), String> {
        let from = *from;
        let mut asked = std::time::Instant::now();
        // The shell in the foreground for so many looks in a row: between the
        // commands of `a; b` it is, for a moment.
        let mut at_shell = 0;
        loop {
            let info = crate::term::info(&self.socket)?;
            let number = |k: &str| info.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let quiet = number("quiet");
            let came = quiet < start.elapsed().as_millis() as u64;
            at_shell = if number("foreground") == number("shell") { at_shell + 1 } else { 0 };
            let holds = match until {
                Until::Has(text) => self.has(text, from, last)?,
                Until::Output => came,
                Until::Quiet(ms) => quiet >= *ms,
                Until::Prompt => at_shell >= 2 && quiet >= 300 && (came || !run),
            };
            let late = start.elapsed().as_secs_f64() >= secs;
            // A window made private, a session locked or a Stop meanwhile ends it.
            let stopped = asked.elapsed() >= std::time::Duration::from_secs(1) && {
                asked = std::time::Instant::now();
                Term::allowed(&self.pid, false)?.is_some_and(|ago| ago < start.elapsed().as_millis() as u64)
            };
            if holds || late || stopped {
                print!("{}{}", if holds && !run { "ok\n" } else { "" }, self.ask(&format!("lines {} {from}", crate::term::HISTORY))?);
            }
            if holds {
                return Ok(());
            }
            if stopped {
                return Err(format!("{STOPPED} (what came out meanwhile is above)"));
            }
            if late {
                return Err(format!("timeout: it did not happen in {secs} s (what came out meanwhile is above)"));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// The text `type` types: its words, or (`-`) the input, so that a
/// password is not seen in the list of processes.
fn typed(words: &[String]) -> Result<String, String> {
    if words.len() == 1 && words[0] == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
        return Ok(s.strip_suffix('\n').map(str::to_owned).unwrap_or(s));
    }
    Ok(words.join(" "))
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

/// A record of `A`'s (`ID DEPTH ROLE X Y W H STATES NAME TEXT`), read.
struct Rec<'a> {
    id: &'a str,
    depth: usize,
    role: &'a str,
    rect: [&'a str; 4],
    states: &'a str,
    name: String,
    text: String,
}

impl<'a> Rec<'a> {
    fn parse(r: &'a str) -> Option<Self> {
        let f: Vec<&str> = r.split_whitespace().collect();
        let [id, depth, role, x, y, w, h, states, name, text] = f[..] else { return None };
        Some(Rec { id, depth: depth.parse().unwrap_or(0), role, rect: [x, y, w, h], states, name: unhex(name), text: unhex(text) })
    }

    /// `role @ID "name" text X,Y WxH states`.
    fn line(&self) -> String {
        let quote = |s: &str| format!("{:?}", s.chars().take(160).collect::<String>());
        let b = self.rect;
        let mut line = format!("{} @{}", self.role, self.id);
        if !self.name.is_empty() {
            line.push_str(&format!(" {}", quote(&self.name)));
        }
        if !self.text.is_empty() {
            line.push_str(&format!(" text {}", quote(&self.text)));
        }
        if b[0] != "-" {
            line.push_str(&format!(" {},{} {}x{}", b[0], b[1], b[2], b[3]));
        }
        if self.states != "-" {
            line.push_str(&format!(" {}", self.states));
        }
        line
    }

    fn json(&self) -> serde_json::Value {
        let n = |v: &str| v.parse::<i64>().ok();
        let b = self.rect;
        serde_json::json!({ "id": self.id.parse::<u32>().unwrap_or(0), "depth": self.depth, "role": self.role, "x": n(b[0]), "y": n(b[1]), "w": n(b[2]), "h": n(b[3]), "states": if self.states == "-" { Vec::new() } else { self.states.split(',').collect() }, "name": self.name, "text": self.text })
    }
}

/// An `err` of `A`'s, as a person reads it.
fn tree_err(head: &str) -> String {
    match head {
        "err no-tree" => "that window's program publishes nothing for screen readers (or was started before the session's accessibility bus): look at it instead".to_owned(),
        "err no-bus" => "this session has no accessibility bus (an older pleamar-wm, or `agent off`)".to_owned(),
        "err no-frame" => "its program publishes a tree, but none of its frames is this window".to_owned(),
        "err private" => "that window is private (a rule says so), or the screen is locked: it is not read".to_owned(),
        _ => head.to_owned(),
    }
}

/// `A`'s answer (`nodes V | records…`), as a person reads it: `tree V`, then
/// each element indented by its depth, `role @ID "name" text X,Y WxH states`;
/// or as JSON.
fn print_nodes(reply: &str, json: bool) -> Result<(), String> {
    let mut parts = reply.split(" | ");
    let head = parts.next().unwrap_or_default();
    let version = head.strip_prefix("nodes ").ok_or_else(|| tree_err(head))?;
    let mut nodes = Vec::new();
    let mut more = None;
    for r in parts {
        if let Some(n) = r.strip_prefix("... more ") {
            more = Some(n.to_owned());
            continue;
        }
        nodes.extend(Rec::parse(r));
    }
    if json {
        let all: Vec<serde_json::Value> = nodes.iter().map(Rec::json).collect();
        println!("{}", serde_json::json!({ "version": version, "nodes": all, "more": more }));
        return Ok(());
    }
    println!("tree {version}");
    if nodes.is_empty() {
        println!("(nothing: no element has the focus, or none says that)");
    }
    for n in &nodes {
        println!("{}{}", "  ".repeat(n.depth), n.line());
    }
    if let Some(n) = more {
        println!("... {n} more not given: `read PID find TEXT` for what you look for");
    }
    Ok(())
}

// -- elements: acted on by the number a tree gave them ------------------

/// An element as `press`, `focus` and `type` name it: `@24`.
fn element(arg: Option<&String>) -> Option<u32> {
    arg?.strip_prefix('@')?.parse().ok()
}

/// The first element given by `find` whose name (else whose text) is that,
/// case folded.
fn named(reply: &str, text: &str) -> Option<u32> {
    let records: Vec<Vec<&str>> = reply.split(" | ").skip(1).map(|r| r.split_whitespace().collect::<Vec<_>>()).filter(|f| f.len() == 10).collect();
    let is = |hex: &str| unhex(hex).to_lowercase() == text.to_lowercase();
    records.iter().find(|f| is(f[8])).or_else(|| records.iter().find(|f| is(f[9]))).and_then(|f| f[0].parse().ok())
}

/// What `e` or `S` answered about that element: `ok`, a `box`, or what it
/// does not do (`err no-action`...); what ends the act, an error.
fn meant(reply: String, pid: &str, id: u32) -> Result<String, String> {
    match reply.as_str() {
        "err gone" => Err(format!("@{id} is no longer there (or {pid}'s tree never gave it): read the tree again")),
        "err stopped-by-user" => Err(STOPPED.to_owned()),
        "err private" => Err(format!("{pid} is private (a rule says so), or the screen is locked: it is not acted on")),
        "ok" | "err no-action" | "err no-focus" | "err no-scroll" | "err not-editable" => Ok(reply),
        r if r.starts_with("box ") => Ok(reply),
        r => Err(format!("@{id}: {r}")),
    }
}

impl Hands {
    /// `e` on that element of a window.
    fn on(&mut self, pid: &str, id: u32, what: &str) -> Result<String, String> {
        meant(self.say(&format!("e root:{pid} {id} {what}"))?, pid, id)
    }

    /// Where that element is clicked: its center, if it is on screen;
    /// `scroll`: scrolled into view first when it is not. None without a box.
    fn spot(&mut self, pid: &str, id: u32, scroll: bool) -> Result<Option<(f64, f64)>, String> {
        let window = self.say(&format!("r {pid}"))?;
        let size: Vec<f64> = window.strip_prefix("rect ").map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
        for again in [false, true] {
            let b = self.on(pid, id, "box")?;
            let f: Vec<&str> = b.split_whitespace().collect();
            let [_, x, y, w, h, shown] = f[..] else { return Err(format!("@{id}: {b}")) };
            let [Ok(x), Ok(y), Ok(w), Ok(h)] = [x, y, w, h].map(str::parse::<f64>) else { return Ok(None) };
            let (cx, cy) = (x + w / 2.0, y + h / 2.0);
            // Shown, and inside the window: a page scrolled away keeps its
            // elements showing, their boxes off its edges.
            let seen = shown == "shown" && size.len() >= 4 && cx >= 0.0 && cy >= 0.0 && cx < size[2] && cy < size[3];
            if seen {
                return Ok(Some((cx, cy)));
            }
            if !scroll || again {
                break;
            }
            let _ = self.on(pid, id, "scroll")?;
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if scroll {
            return Err(format!("@{id} is not on screen, and it did not scroll into view: `press {pid} @{id} --action 0` acts on it without the pointer"));
        }
        Ok(None)
    }

    /// The pointer on that element, clicked as `click` does.
    fn click_element(&mut self, pid: &str, id: u32, button: u32, count: u32) -> Result<(), String> {
        let (x, y) = self.spot(pid, id, true)?.ok_or_else(|| format!("@{id} has no box to click: `press {pid} @{id} --action 0` acts on it without the pointer"))?;
        self.arrive(pid, x, y)?;
        for _ in 0..count {
            self.act(&format!("b root:{pid} 0 {button} 1"))?;
            self.act(&format!("b root:{pid} 0 {button} 0"))?;
        }
        Ok(())
    }

    /// `press PID @ID [right|middle] [COUNT] [--action N]`, or by its name.
    /// Through the pointer; at `instant` by its default action, where it has
    /// one; with `--action N` by that action, the cursor shown on it.
    fn press_element(&mut self, pid: &str, rest: &[String]) -> Result<(), String> {
        let id = match element(rest.first()) {
            Some(id) => id,
            None => {
                let text = rest.first().ok_or("press what: @ID, or its name")?;
                let reply = self.say(&format!("A root:{pid} find {}", hex(text)))?;
                if !reply.starts_with("nodes ") {
                    print_nodes(&reply, false)?;
                }
                named(&reply, text).ok_or_else(|| format!("no element of {pid} is named {text:?}: `read {pid} find {text}` says what has it"))?
            }
        };
        let action = rest.iter().position(|a| a == "--action");
        if let Some(k) = action {
            let n = rest.get(k + 1).and_then(|n| n.parse::<u32>().ok()).ok_or("--action N: the action's number, 0 its default")?;
            if let Some((x, y)) = self.spot(pid, id, false)? {
                self.arrive(pid, x, y)?;
            }
            return match self.on(pid, id, &format!("action {n}"))?.as_str() {
                "ok" => Ok(()),
                _ => Err(format!("@{id} has no action {n}")),
            };
        }
        let (b, count) = (button(rest.get(1))?, rest.get(2).and_then(|c| c.parse::<u32>().ok()).unwrap_or(1).clamp(1, 3));
        // At `instant`, a plain click is its default action, the cursor
        // jumping to it where it is seen.
        if self.pace == Pace::Instant && b == 272 && count == 1 {
            if let Some((x, y)) = self.spot(pid, id, false)? {
                self.arrive(pid, x, y)?;
            }
            if self.on(pid, id, "press")? == "ok" {
                return Ok(());
            }
        }
        self.click_element(pid, id, b, count)
    }

    /// `focus PID @ID`: the keyboard's focus in its window to it, the cursor
    /// going there where it is seen; by the pointer if it cannot be given.
    fn focus_element(&mut self, pid: &str, id: u32) -> Result<(), String> {
        if let Some((x, y)) = self.spot(pid, id, false)? {
            self.arrive(pid, x, y)?;
        }
        if self.on(pid, id, "focus")? == "ok" {
            return Ok(());
        }
        self.click_element(pid, id, 272, 1)
    }

    /// `type PID @ID TEXT --set`: its whole text, at once.
    fn set_text(&mut self, pid: &str, id: u32, text: &str) -> Result<(), String> {
        if text.len() > 30_000 {
            return Err("--set takes up to 30000 bytes: type the rest after it".into());
        }
        if let Some((x, y)) = self.spot(pid, id, false)? {
            self.arrive(pid, x, y)?;
        }
        match meant(self.say(&format!("S root:{pid} {id} {}", if text.is_empty() { "-".to_owned() } else { hex(text) }))?, pid, id)?.as_str() {
            "ok" => Ok(()),
            _ => Err(format!("@{id} takes no text: focus it and type")),
        }
    }
}

/// `A`'s answer to `tree since` (`changes V | + RECORD | - ID | ~ RECORD`),
/// as a person reads it: `changes V`, then `+ role @ID ...`, `- @ID`, `~ role
/// @ID ...`; or `full` and the whole tree. JSON: `version`, `full` and either
/// `changes` (each a node with `change`: `+`, `~`, or `-` and its id alone)
/// or `nodes`.
fn print_changes(reply: &str, json: bool) -> Result<(), String> {
    if let Some(tree) = reply.strip_prefix("full | ") {
        if !json {
            println!("full: that read is not kept (or there was none), or the window shows another page or dialog");
            return print_nodes(tree, false);
        }
        let mut parts = tree.split(" | ");
        let version = parts.next().and_then(|h| h.strip_prefix("nodes ")).unwrap_or_default();
        let (more, nodes): (Vec<&str>, Vec<&str>) = parts.partition(|r| r.starts_with("... more "));
        let all: Vec<serde_json::Value> = nodes.into_iter().filter_map(Rec::parse).map(|r| r.json()).collect();
        println!("{}", serde_json::json!({ "version": version, "full": true, "nodes": all, "more": more.first().and_then(|m| m.strip_prefix("... more ")) }));
        return Ok(());
    }
    let mut parts = reply.split(" | ");
    let head = parts.next().unwrap_or_default();
    let version = head.strip_prefix("changes ").ok_or_else(|| tree_err(head))?;
    let (mut lines, mut all, mut more) = (Vec::new(), Vec::new(), None);
    for r in parts {
        if let Some(n) = r.strip_prefix("... more ") {
            more = Some(n.to_owned());
        } else if let Some(id) = r.strip_prefix("- ") {
            lines.push(format!("- @{id}"));
            all.push(serde_json::json!({ "change": "-", "id": id.parse::<u32>().unwrap_or(0) }));
        } else if let Some((c, rec)) = r.split_once(' ').and_then(|(c, rec)| Some((c, Rec::parse(rec)?))) {
            lines.push(format!("{c} {}", rec.line()));
            let mut j = rec.json();
            j["change"] = c.into();
            all.push(j);
        }
    }
    if json {
        println!("{}", serde_json::json!({ "version": version, "full": false, "changes": all, "more": more }));
        return Ok(());
    }
    println!("changes {version}");
    for l in &lines {
        println!("{l}");
    }
    if let Some(n) = more {
        println!("... {n} more not compared: `read PID find TEXT` for what you look for");
    }
    Ok(())
}

/// `agent wait`'s words, as `W` says them after its MS: the condition, and
/// the seconds (30 unless given).
fn wait_line(words: &[String]) -> Result<(String, f64), String> {
    let id = |s: &str| s.strip_prefix('@').and_then(|n| n.parse::<u32>().ok());
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let (line, rest) = match w[..] {
        ["has" | "gone", text, ..] => (format!("{} {}", w[0], hex(text)), &w[2..]),
        ["focus", at, ..] => (id(at).map_or_else(|| format!("focus {}", hex(at)), |n| format!("{n} focused")), &w[2..]),
        ["changed" | "dialog", ..] => (w[0].to_owned(), &w[1..]),
        ["settled", ms, ..] => (format!("settled {}", ms.parse::<u64>().map_err(|_| "settled MS: milliseconds")?), &w[2..]),
        [at, "not", state, ..] if id(at).is_some() => (format!("{} not {state}", id(at).unwrap_or(0)), &w[3..]),
        [at, state, ..] if id(at).is_some() => (format!("{} {state}", id(at).unwrap_or(0)), &w[2..]),
        _ => return Err("wait PID has \"TEXT\" | gone \"TEXT\" | @ID gone | @ID [not] STATE | focus @ID|\"NAME\" | changed | settled MS | dialog [SECONDS]".into()),
    };
    let secs = match rest {
        [] => 30.0,
        [s] => s.parse::<f64>().ok().filter(|s| *s > 0.0 && *s <= 60.0).ok_or(format!("{s}: seconds, a number up to 60"))?,
        _ => return Err(format!("{}: one number of seconds after the condition", rest.join(" "))),
    };
    Ok((line, secs))
}

/// `W`'s answer, as a person reads it.
fn print_waited(reply: &str) -> Result<(), String> {
    let Some(met) = reply.strip_prefix("ok") else {
        return Err(match reply {
            "err timeout" => "timeout: it did not happen in time".to_owned(),
            "err stopped-by-user" => STOPPED.to_owned(),
            "err no-tree" => "that window's program publishes nothing for screen readers: only `settled` and `dialog` can be waited for on it".to_owned(),
            "err unknown-id" => "no element of that window has that @ID: `tree PID` numbers them".to_owned(),
            "err unknown-state" => "a state is focused, focusable, checked, pressed, selected, expanded, collapsed, editable, disabled, enabled, required, invalid or busy".to_owned(),
            "err gone" => "the window closed".to_owned(),
            "err private" => "that window is private (a rule says so), or the screen is locked: it is not read".to_owned(),
            r => r.to_owned(),
        });
    };
    println!("ok");
    let met = met.trim();
    if let Some(r) = Rec::parse(met) {
        println!("{}", r.line());
    } else if let [pid, slot, title] = met.split_whitespace().collect::<Vec<&str>>()[..] {
        println!("dialog {:?} (window {pid}.{slot})", unhex(title));
    }
    Ok(())
}

/// A line of `O`'s, read.
#[derive(Debug, PartialEq)]
enum Listened {
    /// What to print: `focus role @ID ...`, `announce "TEXT" (role @ID ...)`, `dialog "TITLE" (window PID.SLOT)`.
    Say(String),
    /// How it ended: why, the program's events heard and the elements read.
    End(String, u64, u64),
}

fn listened(line: &str) -> Listened {
    let (what, rest) = line.split_once(' ').unwrap_or((line, ""));
    let f: Vec<&str> = rest.split_whitespace().collect();
    Listened::Say(match (what, &f[..]) {
        ("focus" | "text" | "live", _) => Rec::parse(rest).map_or_else(|| line.to_owned(), |r| format!("{what} {}", r.line())),
        ("announce", [said, record @ ..]) => match Rec::parse(&record.join(" ")) {
            Some(r) => format!("announce {:?} ({})", unhex(said), r.line()),
            None => format!("announce {:?}", unhex(said)),
        },
        ("dialog", [pid, slot, title]) => format!("dialog {:?} (window {pid}.{slot})", unhex(title)),
        ("no-tree", _) => "(its program publishes nothing for screen readers: only dialogs are heard)".to_owned(),
        ("end", [why, heard, read]) => return Listened::End(why.to_string(), heard.parse().unwrap_or(0), read.parse().unwrap_or(0)),
        _ => line.to_owned(),
    })
}

/// Who the agent is: `PLEAMAR_AGENT_NAME`, `agent` otherwise.
fn agent_name() -> String {
    std::env::var("PLEAMAR_AGENT_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "agent".to_owned())
}

/// Where this agent keeps the last version it got of each window's tree:
/// `$XDG_RUNTIME_DIR/pleamar-agent/NAME` (its name hex), a line `PID V` each.
fn versions_file() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").ok()?).join("pleamar-agent").join(hex(&agent_name())))
}

fn last_version(pid: &str) -> Option<u64> {
    std::fs::read_to_string(versions_file()?).ok()?.lines().find_map(|l| l.strip_prefix(pid)?.strip_prefix(' ')?.parse().ok())
}

/// The version an answer of `A`'s gave, kept for that window: the owner's alone.
fn keep_version(pid: &str, reply: &str) {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let head = reply.strip_prefix("full | ").unwrap_or(reply).split(" | ").next().unwrap_or_default();
    let Some(v) = head.strip_prefix("nodes ").or_else(|| head.strip_prefix("changes ")) else { return };
    let Some(path) = versions_file() else { return };
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path.parent().unwrap());
    // One `tree` at a time reads and writes it (two at once lost a window's
    // line): held until this returns.
    let lock = std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(path.with_extension("lock"));
    if let Ok(f) = &lock {
        // SAFETY: flock on a file this holds open.
        unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(f), libc::LOCK_EX) };
    }
    let mut kept: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().filter(|l| l.split(' ').next() != Some(pid)).map(str::to_owned).collect();
    kept.push(format!("{pid} {v}"));
    // Written aside and moved over it: a reader never sees half of it.
    let aside = path.with_extension(std::process::id().to_string());
    let written = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&aside).and_then(|mut f| f.write_all((kept.join("\n") + "\n").as_bytes()));
    if written.and_then(|_| std::fs::rename(&aside, &path)).is_err() {
        let _ = std::fs::remove_file(&aside);
    }
}

/// `look PID @ID|"TEXT" [--pad N] [FILE]` and `look PID --marks [all]
/// [FILE]`: one element of the window (the first that says TEXT, cut to the
/// words when they are in a longer text), or the window with its elements
/// marked and their records. false: another look. A text is told from a
/// file by the file's `/` or its `.png`.
fn look_at(p: &str, rest: &[&str], hidpi: bool, default_out: impl Fn() -> String) -> Result<bool, String> {
    let mut rest = rest.to_vec();
    let pad = match rest.iter().position(|a| *a == "--pad") {
        Some(k) => {
            let n = rest.get(k + 1).and_then(|n| n.parse::<u32>().ok()).ok_or("--pad N: a number of units")?;
            rest.drain(k..k + 2);
            n
        }
        None => 0,
    };
    let file = |a: &str| a.contains('/') || [".png", ".jpg", ".jpeg"].iter().any(|e| a.to_lowercase().ends_with(e));
    let target = format!("root:{p}");
    let hi = if hidpi { " hidpi" } else { "" };
    let (reply, out) = match rest.as_slice() {
        ["--marks", more @ ..] => {
            let all = more.first() == Some(&"all");
            (Hands::open()?.say(&format!("c {target}{hi} marks{}", if all { " all" } else { "" }))?, more.get(usize::from(all)).map(|f| f.to_string()))
        }
        [at, more @ ..] if at.strip_prefix('@').is_some_and(|id| id.parse::<u32>().is_ok()) => (Hands::open()?.say(&format!("c {target}{hi} {at} {pad}"))?, more.first().map(|f| f.to_string())),
        [text, more @ ..] if !file(text) && text.parse::<f64>().is_err() => {
            let mut h = Hands::open()?;
            let found = h.say(&format!("A {target} find {}", hex(text)))?;
            let head = found.split(" | ").next().unwrap_or_default();
            match head {
                "err no-tree" | "err no-frame" => return Err(format!("\"{text}\" was taken as a text to find, and that window has no tree to find it in: a FILE to save to needs a / or .png (./{text}.png)")),
                // As `read` says it.
                _ if !head.starts_with("nodes ") => print_nodes(&found, false)?,
                _ => {}
            }
            let id = first_match(&found, text).ok_or_else(|| format!("nothing in that window says \"{text}\": a word without a / or .png is a text to find, and a FILE to save to needs one (./{text}.png)"))?;
            (h.say(&format!("c {target}{hi} @{id} {pad} {}", hex(text)))?, more.first().map(|f| f.to_string()))
        }
        _ => return Ok(false),
    };
    let mut parts = reply.splitn(2, " | ");
    let head = parts.next().unwrap_or_default();
    let f: Vec<&str> = head.strip_prefix("picture ").ok_or_else(|| match head {
        "err gone" => "that element is gone (the window changed since it was read): read its tree again".to_owned(),
        "err no-box" => "that element is not in the window, and its program did not scroll it into view".to_owned(),
        "err private" => "that window is private (a rule says so), or the screen is locked: it is not looked at".to_owned(),
        "err no-picture" | "err timeout" => "that window has not drawn anything to look at yet".to_owned(),
        "err no-tree" => "that window's program publishes nothing for screen readers: look at it instead".to_owned(),
        _ => head.to_owned(),
    })?.split_whitespace().collect();
    let [path, _, _, _, ref after @ ..] = f[..] else { return Err(reply) };
    let (scale, after) = if hidpi { (after.first().map_or(String::new(), |s| format!(" scale {s}")), after.get(1..).unwrap_or_default()) } else { (String::new(), after) };
    let out = out.unwrap_or_else(default_out);
    std::fs::copy(path, &out).map_err(|e| format!("{out}: {e}"))?;
    let (w, hh) = image::image_dimensions(&out).map_err(|e| e.to_string())?;
    match after {
        // An element: where its picture's corner is in the window's.
        [x, y] => println!("{out} {w}x{hh}{scale} at {x},{y}"),
        [version] => {
            println!("{out} {w}x{hh}{scale}");
            print_nodes(&format!("nodes {version}{}", parts.next().map_or(String::new(), |r| format!(" | {r}"))), false)?;
        }
        _ => return Err(reply),
    }
    Ok(true)
}

/// Of a search's records (`nodes V | ...`), the first element that says
/// `text`: the deepest of those that do, one inside the other, from there.
fn first_match(found: &str, text: &str) -> Option<u32> {
    let needle = text.to_lowercase();
    let records: Vec<(u32, usize, bool)> = found
        .split(" | ")
        .skip(1)
        .filter_map(|r| {
            let f: Vec<&str> = r.split_whitespace().collect();
            let [id, depth, _, _, _, _, _, _, name, says] = f[..] else { return None };
            Some((id.parse().ok()?, depth.parse().ok()?, unhex(name).to_lowercase().contains(&needle) || unhex(says).to_lowercase().contains(&needle)))
        })
        .collect();
    let mut at = records.iter().position(|r| r.2)?;
    while records.get(at + 1).is_some_and(|n| n.1 > records[at].1 && n.2) {
        at += 1;
    }
    Some(records[at].0)
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
    // `--shell ID`: which of a terminal's shells. Anywhere, but in `type`'s
    // text only right after the window.
    let mut args = args.to_vec();
    let shell = match args.iter().position(|a| a == "--shell") {
        Some(k) if k == 2 || (k > 2 && what != "type") => {
            let id = args.get(k + 1).and_then(|s| s.parse::<i32>().ok()).ok_or("--shell ID: a shell's number, as `pleamar-wm term list` gives it")?;
            args.drain(k..k + 2);
            Some(id)
        }
        _ => None,
    };
    let args = &args[..];
    let what = &args[0];
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
    // A terminal with pleamar-wm's layer is read, followed and typed into
    // through it; a pleamar window keeps its scene's `tree` and `wait`. A
    // key the layer does not know goes as the window manager types it.
    let terminal = match what.as_str() {
        "read" => !matches!(args.get(2).map(String::as_str), Some("focus" | "at" | "find")),
        "run" => true,
        "key" => shell.is_some() || args.get(2).is_some_and(|k| crate::term::knows_key(k)),
        "type" => element(args.get(2)).is_none(),
        "hotkey" => args.get(2).is_some_and(|k| k == "shift+tab" || k.strip_prefix("ctrl+").is_some_and(|c| c.len() == 1 && c.as_bytes()[0].is_ascii_alphabetic())),
        "tree" | "wait" => shell.is_some() || pid().is_ok_and(|p| scene_of(&p).is_err()),
        _ => false,
    };
    if shell.is_some() && !terminal {
        return Err("--shell ID names a terminal's shell: for read, tree, wait, run, type, key and hotkey".into());
    }
    if terminal {
        if let Some(t) = Term::of(&pid()?, matches!(what.as_str(), "run" | "type" | "key" | "hotkey"), shell)? {
            match t.go(what, &args[2..]) {
                // An older layer that does not know it.
                Err(e) if shell.is_none() && what == "key" && e.starts_with("no key called") => {}
                done => return done,
            }
        }
    }
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
        "layers" => {
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
                let hidden = if f[8] == "1" { " · hidden" } else { "" };
                println!("{:>5}  {who}  «{}»  {}x{} at {},{}  monitor {}  {level}  pid {}{hidden}", f[0], unhex(f[9]), f[5], f[6], f[3], f[4], f[2], f[1]);
            }
        }
        "hide" | "show" => {
            let id = args.get(1).ok_or("which one: its number, as `pleamar-wm agent layers` says it")?;
            let reply = Hands::open()?.say(&format!("h {id} {}", u8::from(what == "hide")))?;
            if reply != "ok" {
                return Err(reply);
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
            let window = pid().ok();
            let rest: Vec<&str> = args.iter().skip(if window.is_some() { 2 } else { 1 }).map(String::as_str).collect();
            let hidpi = rest.contains(&"--hidpi");
            let rest: Vec<&str> = rest.into_iter().filter(|a| *a != "--hidpi").collect();
            let default_out = || format!("{}/pleamar-agent-look.png", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()));
            let Some(p) = window else {
                let out = rest.first().map_or_else(default_out, |f| f.to_string());
                // At scale 1: grim's own default is the greatest monitor's scale
                // (a HiDPI monitor, the phone's), and the boxes are the desktop's units.
                let shot = std::process::Command::new("grim").args(["-s", "1", &out]).output().map_err(|e| format!("grim: {e} (install grim to look)"))?;
                if !shot.status.success() {
                    return Err(format!("grim: {}", String::from_utf8_lossy(&shot.stderr).trim()));
                }
                let (w, h) = image::image_dimensions(&out).map_err(|e| e.to_string())?;
                println!("{out} {w}x{h}");
                return Ok(());
            };
            if look_at(&p, &rest, hidpi, default_out)? {
                return Ok(());
            }
            // The window manager's picture of it: what it drew, covered or
            // on another workspace, with its menus and nothing over it.
            let (cut, out) = match rest.as_slice() {
                [x, y, w, h, out @ ..] if [x, y, w, h].iter().all(|v| v.parse::<i32>().is_ok()) => (format!(" {x} {y} {w} {h}"), out.first().map(|f| f.to_string())),
                [out, ..] => (String::new(), Some(out.to_string())),
                [] => (String::new(), None),
            };
            let reply = Hands::open()?.say(&format!("c {}{}{cut}", target(&p), if hidpi { " hidpi" } else { "" }))?;
            let f: Vec<&str> = reply.strip_prefix("picture ").ok_or_else(|| match reply.as_str() {
                "err private" => "that window is private (a rule says so), or the screen is locked: it is not looked at".to_owned(),
                "err no-picture" | "err timeout" => "that window has not drawn anything to look at yet".to_owned(),
                _ => reply.clone(),
            })?.split_whitespace().collect();
            let [path, _, _, age, ref rest @ ..] = f[..] else { return Err(reply) };
            // SCALE only with hidpi; X Y only when a menu reaches above or left of the window.
            let (scale, corner) = if hidpi { (rest.first(), rest.get(1..)) } else { (None, Some(rest)) };
            let out = out.unwrap_or_else(default_out);
            std::fs::copy(path, &out).map_err(|e| format!("{out}: {e}"))?;
            let (w, h) = image::image_dimensions(&out).map_err(|e| e.to_string())?;
            let corner = match corner {
                Some([x, y]) => format!(" at {x},{y} (a menu reaches past the window: add these to the picture's coordinates for click)"),
                _ => String::new(),
            };
            let scale = scale.map_or(String::new(), |s| format!(" scale {s}")) + &corner;
            // A window that has not drawn for a while may be behind what it would show now.
            let old = age.parse::<u64>().ok().filter(|a| *a > 10_000).map_or(String::new(), |a| format!(" (drawn {}s ago)", a / 1000));
            // A name that could have been a text to find: which it was taken for.
            let named = if [".png", ".jpg", ".jpeg"].iter().any(|e| out.to_lowercase().ends_with(e)) { String::new() } else { format!(" (saved to the file {out:?}: a number, or a word with a /, is a FILE, not a text to find)") };
            println!("{out} {w}x{h}{scale}{old}{named}");
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
            let (steps, _) = strokes(h.pace);
            for k in 1..=steps {
                let t = k as f64 / steps as f64;
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
                std::thread::sleep(std::time::Duration::from_millis(strokes(h.pace).1));
            }
        }
        "type" => {
            let p = pid()?;
            let id = element(args.get(2));
            let words: Vec<String> = args[2 + usize::from(id.is_some())..].iter().filter(|a| id.is_none() || *a != "--set").cloned().collect();
            let text = typed(&words)?;
            let mut h = Hands::open()?;
            // Into that element: its focus first; with --set, its whole text at once.
            if let Some(id) = id {
                if args[3..].iter().any(|a| a == "--set") {
                    return h.set_text(&p, id, &text);
                }
                h.focus_element(&p, id)?;
            }
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
            let json = args.iter().skip(2).any(|a| a == "json");
            match scene_of(&p) {
                Ok((_, sock)) => print!("{}", ask(&sock, if json { "describe json" } else { "describe" })?),
                // Any other window: what its program published for screen readers.
                Err(_) if args.iter().any(|a| a == "--since") => {
                    // The read asked against: the one given, else the last this agent got (none: `0`, never kept, the whole tree).
                    let at = args.iter().position(|a| a == "--since").unwrap();
                    let v = match args.get(at + 1).filter(|a| !a.starts_with("--") && *a != "json") {
                        Some(v) => v.parse::<u64>().map_err(|_| format!("--since V: a version, as `tree` gives it, not {v}"))?,
                        None => last_version(&p).unwrap_or(0),
                    };
                    let boxes = if args.iter().any(|a| a == "--boxes") { " boxes" } else { "" };
                    let reply = Hands::open()?.say(&format!("A {} tree since {v}{boxes}", target(&p)))?;
                    keep_version(&p, &reply);
                    print_changes(&reply, json)?;
                }
                Err(_) => {
                    let all = args.iter().skip(2).any(|a| a == "all");
                    let reply = Hands::open()?.say(&format!("A {} tree{}", target(&p), if all { " all" } else { "" }))?;
                    if !all {
                        keep_version(&p, &reply);
                    }
                    print_nodes(&reply, json)?;
                }
            }
        }
        "read" => {
            let p = pid()?;
            let line = match args.get(2).map(String::as_str) {
                Some("focus") => "focus".to_owned(),
                Some("at") => format!("at {} {}", number(args.get(3), "x")? as i32, number(args.get(4), "y")? as i32),
                Some("find") => format!("find {}", hex(&args.get(3..).map(|w| w.join(" ")).filter(|w| !w.is_empty()).ok_or("find what")?)),
                _ => return Err("read PID focus | at X Y | find TEXT (a terminal: read PID [N])".into()),
            };
            let reply = Hands::open()?.say(&format!("A {} {line}", target(&p)))?;
            print_nodes(&reply, args.iter().skip(2).any(|a| a == "json"))?;
        }
        "press" => {
            let p = pid()?;
            args.get(2).ok_or("press what: @ID, or its name, as `tree` gives it")?;
            match scene_of(&p) {
                // The scene presses it by name, and before that takes the agent's
                // cursor there itself: it is never behind the press.
                Ok((_, sock)) if element(args.get(2)).is_none() => print!("{}", ask(&sock, &format!("press {}", args[2..].join(" ")))?),
                // Any other window: the element its tree numbered, or named.
                _ => Hands::open()?.press_element(&p, &args[2..])?,
            }
        }
        // Any other window: until its tree, or the window manager, says so.
        "wait" if scene_of(&pid()?).is_err() => {
            let (condition, secs) = wait_line(&args[2..])?;
            let mut h = Hands::open()?;
            let _ = h.reader.get_ref().set_read_timeout(Some(std::time::Duration::from_secs_f64(secs + 1.0)));
            print_waited(&h.say(&format!("W {} {} {condition}", target(&pid()?), (secs * 1000.0) as u64))?)?;
        }
        // Any other window: a line for each thing it says, as it comes.
        "listen" => {
            let p = pid()?;
            let secs = match args.get(2) {
                None => 30.0,
                Some(s) => s.parse::<f64>().ok().filter(|s| *s > 0.0 && *s <= 600.0).ok_or(format!("{s}: seconds, a number up to 600"))?,
            };
            let mut h = Hands::open()?;
            let _ = h.reader.get_ref().set_read_timeout(Some(std::time::Duration::from_secs_f64(secs + 5.0)));
            writeln!(h.writer, "O {} {}", target(&p), (secs * 1000.0) as u64).map_err(|e| e.to_string())?;
            let start = std::time::Instant::now();
            loop {
                let mut line = String::new();
                if h.reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                    return Err("the session hung up".into());
                }
                let line = line.trim_end();
                if line.starts_with("err ") {
                    return Err(tree_err(line));
                }
                let (why, heard, read) = match listened(line) {
                    Listened::Say(s) => {
                        println!("{s}");
                        let _ = std::io::stdout().flush();
                        continue;
                    }
                    Listened::End(why, heard, read) => (why, heard, read),
                };
                let took = start.elapsed().as_secs_f64();
                eprintln!("listened {took:.1} s: {heard} events from it ({:.0} a second), {read} elements read", heard as f64 / took.max(0.001));
                return match why.as_str() {
                    "stopped-by-user" => Err(STOPPED.to_owned()),
                    "private" => Err(tree_err("err private")),
                    "gone" => Ok(println!("(the window closed)")),
                    _ => Ok(()),
                };
            }
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
        "pace" => match args.get(1) {
            None => println!("{}", Hands::open()?.pace.name()),
            Some(p) => {
                let pace = Pace::parse(p).ok_or_else(|| format!("a pace is default, fast or instant: {p} is not one"))?;
                Hands::open()?.act(&format!("T {}", pace.name()))?;
            }
        },
        "focus" => {
            let p = pid()?;
            match element(args.get(2)) {
                Some(id) => Hands::open()?.focus_element(&p, id)?,
                None => Hands::open()?.act(&format!("f {p}"))?,
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_is_looked_at_in_the_deepest_that_says_it() {
        let found = format!("nodes 3 | 1 0 frame 0 0 800 600 - {} - | 31 1 label 105 280 144 19 - {} - | 32 2 check_box 109 282 14 14 focusable {} - | 40 1 paragraph 10 10 80 20 - - {}", hex("Remember me - Firefox"), hex("Remember me"), hex("Remember me"), hex("remember me later"));
        assert_eq!(first_match(&found, "remember ME"), Some(32));
        assert_eq!(first_match(&found, "later"), Some(40));
        assert_eq!(first_match(&found, "nowhere"), None);
    }

    #[test]
    fn each_pace_moves_as_its_table_says() {
        // As a hand: from 260 to 620 ms, 8 to 24 steps, and a moment still.
        assert_eq!(glide(Pace::Default, 10.0), Some((260.0, 8, 70)));
        assert_eq!(glide(Pace::Default, 2000.0), Some((620.0, 24, 70)));
        // Quicker, and no stop.
        assert_eq!(glide(Pace::Fast, 10.0), Some((80.0, 4, 0)));
        assert_eq!(glide(Pace::Fast, 2000.0), Some((150.0, 6, 0)));
        for far in [5.0, 300.0, 900.0] {
            let (ms, steps, _) = glide(Pace::Fast, far).unwrap();
            assert!((80.0..=150.0).contains(&ms) && (4..=6).contains(&steps));
        }
        assert_eq!(glide(Pace::Instant, 500.0), None);
        assert_eq!(strokes(Pace::Default), (24, 60));
        assert_eq!(strokes(Pace::Fast), (8, 20));
    }

    #[test]
    fn an_element_is_its_number_or_its_name() {
        assert_eq!(element(Some(&"@24".to_owned())), Some(24));
        // A text that starts with @ is still text to type.
        assert_eq!(element(Some(&"@john".to_owned())), None);
        assert_eq!(element(Some(&"24".to_owned())), None);
        // Its name, case folded, before its text; what it is in is not it.
        let reply = format!("nodes 3 | 1 0 frame 0 0 800 600 - {} - | 5 1 push_button 10 10 80 30 focusable {} - | 6 1 label 10 50 80 20 - - {}", hex("Save as"), hex("Save"), hex("save"));
        assert_eq!(named(&reply, "save"), Some(5));
        assert_eq!(named(&reply, "Save as"), Some(1));
        assert_eq!(named(&reply, "Open"), None);
    }

    #[test]
    fn a_listen_is_read_a_line_at_a_time() {
        let say = |s: &str| match listened(s) {
            Listened::Say(s) => s,
            end => panic!("{end:?}"),
        };
        assert_eq!(say(&format!("focus 12 0 push_button 10 20 80 30 focused,focusable {} -", hex("Close"))), "focus push_button @12 \"Close\" 10,20 80x30 focused,focusable");
        assert_eq!(say(&format!("text 5 0 entry 0 0 200 30 focused,editable - {}", hex("12*3"))), "text entry @5 text \"12*3\" 0,0 200x30 focused,editable");
        assert_eq!(say(&format!("announce {} 3 0 label - - - - - - -", hex("Saved"))), "announce \"Saved\" (label @3)");
        assert_eq!(say(&format!("announce {}", hex("Saved"))), "announce \"Saved\"");
        assert_eq!(say(&format!("dialog 4521 3 {}", hex("About"))), "dialog \"About\" (window 4521.3)");
        assert!(say("no-tree").contains("only dialogs"));
        assert_eq!(listened("end time 812 40"), Listened::End("time".into(), 812, 40));
    }

    #[test]
    fn a_wait_says_its_condition_as_the_socket_does() {
        let w = |s: &[&str]| wait_line(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(w(&["has", "Saved"]), Ok(("has 5361766564".into(), 30.0)));
        assert_eq!(w(&["@24", "gone", "5"]), Ok(("24 gone".into(), 5.0)));
        assert_eq!(w(&["@24", "not", "busy"]), Ok(("24 not busy".into(), 30.0)));
        assert_eq!(w(&["focus", "@3"]), Ok(("3 focused".into(), 30.0)));
        assert_eq!(w(&["focus", "OK"]), Ok(("focus 4f4b".into(), 30.0)));
        assert_eq!(w(&["settled", "500", "10"]), Ok(("settled 500".into(), 10.0)));
        assert_eq!(w(&["dialog"]), Ok(("dialog".into(), 30.0)));
        assert!(w(&["settled", "soon"]).is_err());
        assert!(w(&["dialog", "90"]).is_err());
        assert!(w(&["24", "gone"]).is_err());
    }
}
