//! A computer-use agent's hands: `cua-inject v1`, the line protocol Cua
//! Driver speaks to a compositor of its own (its nested `cua-compositor`),
//! here spoken by the session's. Cua Driver uses it whenever
//! `CUA_INJECT_SOCKET` is set, so the programs this session starts find it.
//!
//! What stock Wayland forbids a program —typing into a window without the
//! keyboard, clicking one that is under another— the compositor can do: the
//! agent has a seat of its own (`cua-agent`, one per cursor), so its pointer
//! and its keyboard are not yours, and your mouse and your focus stay where
//! they are while it works. A program that only listens to one seat
//! (Chromium, GLFW's) gets the agent's events on the usual one instead,
//! straight to its own objects, without moving anybody's focus.
//!
//! Where each cursor is goes to the scene (`agent.$k.x`…), which draws it:
//! a second cursor, moving on its own.
//!
//! The protocol: the client sends `cua-inject v1`, which is echoed back; then
//! one command a line, each answered by `ok` or `err <reason>` once it has
//! been delivered:
//!
//! | command | |
//! | --- | --- |
//! | `q PID` | `state FOCUSED_PID foreground|background_visible|background_occluded|not_found` |
//! | `g PID` | `geometry AX AY SX SY`: where the window (AX, AY) and its root surface (SX, SY) are on the desktop |
//! | `x` | the agent has finished: the monitor's light goes out now, not when it would have stopped waiting for its next step |
//! | `C [PID]` | `can LOCKED` and, `|` between them, per window (every window without PID; with it, the one TARGET `root:PID` names) `PID SLOT PRIVATE POPUPS`: 1 or 0, it is private; POPUPS its menus open, `X,Y,W,H` from the root's corner, `;` between them, `-` with none. LOCKED: the screen is locked. `err unknown-root-pid` |
//! | `l` | `windows` and, per window, `PID X Y W H VISIBLE FOCUSED APP TITLE` (app and title hex encoded), windows separated by `|`: what `pleamar-wm agent windows` shows |
//! | `r PID` | `rect X Y W H VISIBLE`: the root surface's box on the desktop, as the scene shows it, and whether it is seen (1) — what a screenshot of the window is cut from, and what `m`'s coordinates count in |
//! | `f PID` | the keyboard to that process' only window (and its workspace shown) |
//! | `m TARGET IDX X Y` | cursor IDX to X, Y of the window (its root surface's coordinates) |
//! | `b TARGET IDX BUTTON PRESSED` | an evdev button (272 left), 1 down, 0 up |
//! | `a TARGET IDX AXIS VALUE` | the wheel: axis 0 vertical, 1 horizontal |
//! | `t TARGET HEX` | ASCII text, hex encoded, typed into the window |
//! | `k TARGET KEY` | a named key: enter, tab, escape, backspace, space, arrows, f1–f12 |
//! | `h TARGET MODS KEY` | a chord: `ctrl,shift` and a key |
//! | `d X Y COUNT BUTTON` | a click at a point of the desktop |
//! | `T`, `T PACE` | `pace PACE`: how fast the hands go, for every agent (`default`, `fast`, `instant`); `T PACE` sets it. The CLI moves at it, and Stop does not refuse it |
//! | `M PID W H IDX X Y` | cursor IDX to X, Y of that process' surface of W × H that is no window (a panel: Marea, a bar) |
//! | `B PID W H IDX BUTTON PRESSED` | a button there |
//! | `R MONITOR HEX` · `R off` | `pleamar-wm remote`: someone uses this desktop from elsewhere, looking at that monitor, from that address (hex encoded) — said again every few seconds while it lasts, and forgotten 15 s after the last |
//! | `E phone_NAME` | a gesture on the phone: the scene's event of that name |
//! | `F X Y` · `F end` | a finger on the phone's glass: what is under the pointer moves X, Y units (down and to the right, positive), as under a touchpad's fingers; and the finger lifted |
//! | `U HEX` | text from the phone's keyboard (hex encoded), typed into the window with the keyboard as the person's own: no agent marks |
//! | `P W H SCALE` · `P off` | `pleamar-wm remote` from a phone: a monitor of W × H pixels at that scale is put up for it (`PHONE-1`), or taken down (docs/phone.md) |
//!
//! A TARGET is `pid:N` (that process' only window), `root:N` (the only window
//! of that process or a child of it: a browser's), or an app_id.

use super::{State, Toplevel};
use crate::layers;
use pleamar::scene::{NestEvent, ToNest, ToRender};
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::input::keyboard::{xkb, FilterResult, KeyboardHandle, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerHandle};
use smithay::input::Seat;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::reexports::wayland_server::protocol::{wl_keyboard, wl_pointer, wl_surface::WlSurface};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use std::os::unix::net::{UnixListener, UnixStream};

const HELLO: &str = "cua-inject v1";
/// How long after `agent open` a program's first windows are still its.
const OPENING: Duration = Duration::from_secs(30);

/// A program the agent opened: its process, the name it was started by (a
/// program that was already running opens the window from its own process,
/// Discord, Firefox: it is known by its name), and the monitor.
struct Opening {
    pid: u32,
    name: String,
    screen: usize,
    until: Instant,
}
/// Cursors the agent may drive at once.
const CURSORS: usize = 2;

/// The agent's seats and what each cursor last did.
pub struct Agent {
    seats: Vec<(Seat<State>, PointerHandle<State>, KeyboardHandle<State>)>,
    /// Per cursor, the monitor and the point (in its units) it was last drawn
    /// at over a panel.
    on_monitor: [Option<(usize, (f64, f64))>; CURSORS],
    /// Per cursor, on the usual seat (for the programs that only hear that
    /// one): the surface it entered, straight on its objects.
    raw_entered: [Option<WlSurface>; CURSORS],
    /// Where your own pointer was when that was said: it speaks through the
    /// same objects, so if it has gone in or out since, what the program
    /// thinks is not what was said.
    raw_real: [Option<WlSurface>; CURSORS],
    /// The window its keys went to straight on its objects (one that is not
    /// yours): it stays entered while the agent works with it, as a window
    /// you type in stays focused. Entering and leaving around every key
    /// command made a program think it lost focus between the agent's
    /// Ctrl+K and its letters, and Discord's search box got none of them.
    kb_entered: Option<WlSurface>,
    /// And its slot, shown as active (`activated`) for that while, as a
    /// window you type in is. A Chromium program takes a character typed by
    /// number (Ctrl+Shift+U, an emoji) only in an active window: in one that
    /// was not, Discord took Ctrl+Shift+U as its «upload a file».
    pub(super) kb_slot: Option<usize>,
    /// Where your keyboard was when that was said: if it has been anywhere
    /// since (that window too), the program may have been told `leave`.
    kb_real: Option<WlSurface>,
    /// Programs the agent opened (`agent open`): their windows go to the
    /// monitor it works on, and none takes your keyboard.
    opening: Vec<Opening>,
    /// Per cursor: how many things it has done, for the scene to know it moved.
    seen: [u32; CURSORS],
    /// And all of them, keys too: the scene knows the agent is at work.
    busy: u32,
    /// The process that last spoke to the socket, when it last did
    /// something, and whether the scene was told the agent is at work.
    peer: u32,
    last: Option<Instant>,
    active: bool,
    /// And where it last was, in its window's root surface.
    at: [Point<f64, Logical>; CURSORS],
    /// And on the desktop, in units (for `p`: the next move glides from there).
    global: [Option<(f64, f64)>; CURSORS],
    /// The program it works with now (the process its target names) and the
    /// monitor that window is on.
    working: Option<(u32, usize)>,
    /// Stopped by the user (the «Stop» on the monitor's pill, or
    /// `pleamar-wm agent stop`): until the agent says it is done, or for a
    /// minute, everything it tries to do is refused, and it hears why.
    stopped: Option<Instant>,
    /// Someone at this desktop from elsewhere (`pleamar-wm remote`): when
    /// that was last said.
    remote: Option<Instant>,
    /// What the last command pasted, until the window reads it, and what
    /// was copied before, to give back then: its answer waits for that (see
    /// `agent_lines`).
    pasting: Option<(std::sync::Arc<super::Kept>, Option<std::sync::Arc<super::Kept>>)>,
    /// Typing for the person, not for an agent (the phone's keyboard, `U`):
    /// the same hands, without the agent's marks on the monitor.
    quiet: bool,
    /// How fast the hands go (`T`), for every agent and shell: the CLI asks
    /// it when it opens the socket.
    pace: crate::config::Pace,
    pub path: String,
}

/// A connection to the socket: what it sent that is not answered yet, and
/// whether an answer is being waited for (a paste the window has not read).
struct Conn {
    stream: UnixStream,
    buffer: Vec<u8>,
    hello: bool,
    waiting: bool,
}

/// An answer, down a connection. Blocking for it: it is one short line.
fn answer(conn: &std::rc::Rc<std::cell::RefCell<Conn>>, reply: &str) -> bool {
    let c = conn.borrow_mut();
    let _ = c.stream.set_nonblocking(false);
    let ok = (&c.stream).write_all(format!("{reply}\n").as_bytes()).is_ok();
    let _ = c.stream.set_nonblocking(true);
    ok
}

/// Where the socket goes: beside the session's other ones.
pub fn socket_path(display: &str) -> String {
    crate::agent_socket_path(display)
}

/// The agent's seats and its socket, if session.conf says `agent on`.
pub fn start(state: &mut State) {
    if !crate::config::get().agent {
        return;
    }
    let path = socket_path(&state.socket);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("agent · cannot listen at {path}: {e}");
            return;
        }
    };
    // Yours only.
    let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    let _ = listener.set_nonblocking(true);
    let mut seats = Vec::new();
    for k in 0..CURSORS {
        let name = if k == 0 { "cua-agent".to_owned() } else { format!("cua-agent-{}", k + 1) };
        let mut seat = state.seats.new_wl_seat(&state.dh, name);
        // A plain US keyboard: what it types is what was asked, whatever yours is.
        let us = XkbConfig { layout: "us", ..Default::default() };
        let Ok(keyboard) = seat.add_keyboard(us, 600, 25) else { continue };
        let pointer = seat.add_pointer();
        seats.push((seat, pointer, keyboard));
    }
    let source = Generic::new(listener, Interest::READ, Mode::Level);
    let inserted = state.handle.insert_source(source, |_, listener, state: &mut State| {
        while let Ok((stream, _)) = listener.accept() {
            // Only the same user (the socket is 0600; this says it again).
            let Some(pid) = same_user(&stream) else { continue };
            if let Some(a) = state.agent.as_mut() {
                a.peer = pid;
            }
            let _ = stream.set_nonblocking(true);
            state.agent_connection(stream);
        }
        Ok(PostAction::Continue)
    });
    if let Err(e) = inserted {
        eprintln!("agent · {e}");
        return;
    }
    println!("agent · computer use: cua-inject v1 at {path}");
    state.agent = Some(Agent { seats, on_monitor: [None; CURSORS], raw_entered: [None, None], raw_real: [None, None], kb_entered: None, kb_slot: None, kb_real: None, opening: Vec::new(), seen: [0; CURSORS], busy: 0, at: [(0.0, 0.0).into(); CURSORS], path, peer: 0, last: None, active: false, stopped: None, global: [None; CURSORS], working: None, remote: None, pasting: None, quiet: false, pace: crate::config::get().pace });
    // Whether it is at work, looked at every second: between one action and
    // the next an agent thinks, and that is still working.
    let timer = Timer::from_duration(Duration::from_secs(1));
    let _ = state.handle.insert_source(timer, |_, _, state: &mut State| {
        state.agent_still_working();
        TimeoutAction::ToDuration(Duration::from_secs(1))
    });
}

impl State {
    fn agent_connection(&mut self, stream: UnixStream) {
        let Ok(reading) = stream.try_clone() else { return };
        let conn = std::rc::Rc::new(std::cell::RefCell::new(Conn { stream, buffer: Vec::new(), hello: false, waiting: false }));
        let source = Generic::new(reading, Interest::READ, Mode::Level);
        let _ = self.handle.insert_source(source, move |_, stream, state: &mut State| {
            let stream: &mut UnixStream = unsafe { stream.get_mut() };
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => return Ok(PostAction::Remove),
                    Ok(n) => conn.borrow_mut().buffer.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => return Ok(PostAction::Remove),
                }
                // A line longer than any command is not one.
                if conn.borrow().buffer.len() > 64 * 1024 {
                    return Ok(PostAction::Remove);
                }
            }
            Ok(if state.agent_lines(&conn) { PostAction::Continue } else { PostAction::Remove })
        });
    }

    /// The lines a connection sent, answered in order. One that pasted
    /// (`agent_paste_with_yours`) is answered once the window has read what
    /// was pasted, or after a second and a half: a program reads it when it
    /// gets round to the Ctrl+V, and something pasted next could otherwise
    /// get there first —Chromium pasted the second emoji in place of the
    /// first—. The lines after it wait meanwhile. false: the connection is over.
    fn agent_lines(&mut self, conn: &std::rc::Rc<std::cell::RefCell<Conn>>) -> bool {
        loop {
            let line = {
                let mut c = conn.borrow_mut();
                if c.waiting {
                    return true;
                }
                let Some(end) = c.buffer.iter().position(|b| *b == b'\n') else { return true };
                let line: Vec<u8> = c.buffer.drain(..=end).collect();
                String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']).to_owned()
            };
            let reply = if !conn.borrow().hello {
                if line == HELLO {
                    conn.borrow_mut().hello = true;
                    HELLO.to_owned()
                } else {
                    let _ = conn.borrow_mut().stream.write_all(b"err unsupported-version\n");
                    return false;
                }
            } else {
                self.agent_command(&line)
            };
            if let Some((pasted, back)) = self.agent.as_mut().and_then(|a| a.pasting.take()) {
                conn.borrow_mut().waiting = true;
                let (conn, since) = (conn.clone(), Instant::now());
                let mut back = Some(back);
                let timer = Timer::from_duration(Duration::from_millis(5));
                let _ = self.handle.insert_source(timer, move |_, _, state: &mut State| {
                    if !pasted.read.load(std::sync::atomic::Ordering::Relaxed) && since.elapsed() < Duration::from_millis(1500) {
                        return TimeoutAction::ToDuration(Duration::from_millis(5));
                    }
                    if let Some(back) = back.take() {
                        state.agent_give_back(&pasted, back);
                    }
                    conn.borrow_mut().waiting = false;
                    if answer(&conn, &reply) {
                        state.agent_lines(&conn);
                    }
                    TimeoutAction::Drop
                });
                return true;
            }
            if !answer(conn, &reply) {
                return false;
            }
        }
    }

    /// One command, answered once it has been delivered.
    fn agent_command(&mut self, line: &str) -> String {
        let w: Vec<&str> = line.split_whitespace().collect();
        let result = match w.as_slice() {
            ["q", pid] => return self.agent_query(pid.parse().unwrap_or(0)),
            ["g", pid] => self.agent_geometry(pid),
            ["r", pid] => self.agent_rect(pid),
            ["C"] => self.agent_can(None),
            ["C", pid] => self.agent_can(Some(pid)),
            ["l"] => Ok(self.agent_list()),
            ["o"] => Ok(self.agent_monitors()),
            ["y"] => Ok(self.agent_layers()),
            ["h", id, on] => self.agent_hide_layer(id.parse().unwrap_or(0), *on == "1").map(|_| "ok".to_owned()),
            // Who the agent is, for its cursor's label (`PLEAMAR_AGENT_NAME`).
            ["n", hex] => {
                let name: String = String::from_utf8_lossy(&unhex(hex).unwrap_or_default()).chars().filter(|c| !c.is_control()).take(16).collect();
                let name = if name.trim().is_empty() { "agent".to_owned() } else { name };
                let _ = self.to_render.send(ToRender::Text(pleamar::scene::intern("agent.name"), name));
                Ok("ok".to_owned())
            }
            ["p", idx] => Ok(self.agent_where(idx.parse().unwrap_or(0))),
            // The pace: how the hands move, not something they do, so the
            // user's Stop does not refuse it.
            ["T"] => Ok(format!("pace {}", self.agent.as_ref().map_or_else(Default::default, |a| a.pace).name())),
            ["T", pace] => match (crate::config::Pace::parse(pace), self.agent.as_mut()) {
                (Some(p), Some(a)) => {
                    a.pace = p;
                    let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern("agent.pace"), p as u8 as f32));
                    Ok("ok".to_owned())
                }
                _ => Err("bad-args"),
            },
            ["v", pid, monitor] => self.agent_send(pid, monitor.parse().unwrap_or(usize::MAX)).map(|_| "ok".to_owned()),
            ["x"] => {
                self.agent_done();
                let _ = self.to_render.send(ToRender::Text(pleamar::scene::intern("agent.name"), "agent".to_owned()));
                if let Some(agent) = self.agent.as_mut() {
                    agent.stopped = None;
                }
                Ok("ok".to_owned())
            }
            // The remote desktop says who is in, and where they look: the
            // scene marks it on the monitors (and only there, not in what
            // they see from elsewhere).
            ["R", "off"] => {
                self.remote_mark(None);
                Ok("ok".to_owned())
            }
            // What the phone's keyboard writes, into the window with the
            // keyboard: the person's own typing, any text.
            ["U", hex] => (|| {
                let text = unhex(hex)?;
                let focus = self.keyboard.current_focus();
                let slot = self.slots.iter().position(|w| w.as_ref().is_some_and(|w| focus.as_ref() == Some(&w.surface))).ok_or("no-window")?;
                if let Some(a) = self.agent.as_mut() {
                    a.quiet = true;
                }
                let done = self.agent_type(slot, &text);
                if let Some(a) = self.agent.as_mut() {
                    a.quiet = false;
                }
                done.map(|_| "ok".to_owned())
            })(),
            // A gesture on the phone, to the scene as the event of that name
            // (`phone_cards`, `phone_next`…): only the phone's.
            ["E", name] if name.starts_with("phone_") && name.len() < 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                let _ = self.to_render.send(ToRender::ExternalSignal(pleamar::scene::intern(name), None));
                Ok("ok".to_owned())
            }
            // A finger on the phone's glass moving what is under the pointer,
            // so many units (down and to the right, positive), and lifted: the
            // person's own, as their touchpad's would be.
            ["F", "end"] => {
                if layers::fingers(layers::Fingers::Up) {
                    Ok("ok".to_owned())
                } else {
                    Err("no-session")
                }
            }
            ["F", x, y] => match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(x), Ok(y)) if x.is_finite() && y.is_finite() && x.abs() <= 4096.0 && y.abs() <= 4096.0 => {
                    if layers::fingers(layers::Fingers::Moved(x, y)) {
                        Ok("ok".to_owned())
                    } else {
                        Err("no-session")
                    }
                }
                _ => Err("bad-args"),
            },
            // Files the remote page received, dropped on the window under
            // the pointer (a `text/uri-list`, in hex).
            ["D", uris] if uris.len() % 2 == 0 && uris.len() <= 60_000 => {
                match unhex_long(uris).ok().and_then(|b| String::from_utf8(b).ok()) {
                    Some(list) if list.starts_with("file://") => self.drop_files(list),
                    _ => Err("bad-args"),
                }
            }
            // A phone asks for a monitor of its own, and lets it go.
            ["P", "off"] => {
                if layers::request_phone(None) {
                    Ok("ok".to_owned())
                } else {
                    Err("no-session")
                }
            }
            ["P", w, h, scale] => match (w.parse::<u32>(), h.parse::<u32>(), scale.parse::<f64>()) {
                // Locked, it stays at the desk: the lock screen is only there,
                // and it is unlocked there (from the phone too, typing).
                _ if layers::locked() => Err("locked"),
                (Ok(w), Ok(h), Ok(scale)) if (200..=4096).contains(&w) && (200..=4096).contains(&h) && (0.5..=4.0).contains(&scale) => {
                    if layers::request_phone(Some(layers::PhoneWish { size: (w, h), scale })) {
                        Ok("ok".to_owned())
                    } else {
                        Err("no-session")
                    }
                }
                _ => Err("bad-args"),
            },
            ["R", monitor, who] => match (monitor.parse::<i32>(), unhex(who)) {
                (Ok(monitor), Ok(who)) => {
                    let who: String = String::from_utf8_lossy(&who).chars().filter(|c| !c.is_control()).take(64).collect();
                    self.remote_mark(Some((monitor, who)));
                    Ok("ok".to_owned())
                }
                _ => Err("bad-args"),
            },
            ["s"] => {
                self.agent_done();
                if let Some(agent) = self.agent.as_mut() {
                    agent.stopped = Some(Instant::now());
                }
                eprintln!("agent · stopped by the user");
                Ok("ok".to_owned())
            }
            // Looking stays possible; doing anything does not.
            [verb, ..] if matches!(*verb, "f" | "m" | "b" | "a" | "t" | "k" | "h" | "d" | "v" | "L" | "M" | "B") && self.agent_stopped() => Err("stopped-by-user"),
            // A program opened by the agent, on a monitor (-1: the one it
            // works on, or one you are not on): «opened PID MONITOR».
            ["L", monitor, command] => match monitor.parse::<i64>() {
                Ok(m) => unhex(command).and_then(|c| String::from_utf8(c).map_err(|_| "bad-utf8")).and_then(|c| self.agent_open(m, &c)),
                Err(_) => Err("bad-args"),
            },
            ["f", pid] => self.agent_activate(pid).map(|_| "ok".to_owned()),
            ["m", target, idx, x, y] => self.agent_target(target).and_then(|s| self.agent_motion(s, idx.parse().unwrap_or(99), num(x)?, num(y)?)).map(|_| "ok".into()),
            ["b", target, idx, button, pressed] => self.agent_target(target).and_then(|s| self.agent_button(s, idx.parse().unwrap_or(99), button.parse().map_err(|_| "bad-args")?, *pressed != "0")).map(|_| "ok".into()),
            ["a", target, idx, axis, value] => self.agent_target(target).and_then(|s| self.agent_axis(s, idx.parse().unwrap_or(99), *axis == "1", num(value)?)).map(|_| "ok".into()),
            ["t", target, hex] => self.agent_target(target).and_then(|s| self.agent_type(s, &unhex(hex)?)).map(|_| "ok".into()),
            ["k", target, key] => self.agent_target(target).and_then(|s| {
                let code = named_key(key).ok_or("unknown-key")?;
                self.agent_keys(s, &[], code)
            }).map(|_| "ok".into()),
            ["h", target, mods, key] => self.agent_target(target).and_then(|s| {
                let mut held = Vec::new();
                for m in mods.split(',') {
                    held.push(match m.to_ascii_lowercase().as_str() {
                        "ctrl" | "control" => 29,
                        "shift" => 42,
                        "alt" | "option" => 56,
                        "meta" | "super" | "win" | "cmd" => 125,
                        _ => return Err("unknown-hotkey"),
                    });
                }
                let code = named_key(key).or_else(|| (key.len() == 1).then(|| us_key(key.as_bytes()[0]).map(|(c, _)| c)).flatten()).ok_or("unknown-hotkey")?;
                self.agent_keys(s, &held, code)
            }).map(|_| "ok".into()),
            ["M", pid, w, h, idx, x, y] => match (pid.parse::<u32>(), w.parse::<i32>(), h.parse::<i32>()) {
                (Ok(pid), Ok(w), Ok(h)) => num(x).and_then(|x| Ok((x, num(y)?))).and_then(|(x, y)| self.agent_panel_motion(pid, (w, h), idx.parse().unwrap_or(99), x, y)).map(|_| "ok".into()),
                _ => Err("bad-args"),
            },
            ["B", pid, w, h, idx, button, pressed] => match (pid.parse::<u32>(), w.parse::<i32>(), h.parse::<i32>(), button.parse::<u32>()) {
                (Ok(pid), Ok(w), Ok(h), Ok(button)) => self.agent_panel_button(pid, (w, h), idx.parse().unwrap_or(99), button, *pressed != "0").map(|_| "ok".into()),
                _ => Err("bad-args"),
            },
            ["d", x, y, count, button] => num(x).and_then(|x| Ok((x, num(y)?))).and_then(|(x, y)| self.agent_desktop_click(x, y, count.parse().unwrap_or(1), button.parse().unwrap_or(272))).map(|_| "ok".into()),
            [] => Err("empty"),
            _ => Err("unknown-command"),
        };
        match result {
            Ok(reply) => reply,
            Err(e) => format!("err {e}"),
        }
    }

    pub(super) fn pid_of(&self, slot: usize) -> u32 {
        let Some(Some(w)) = self.slots.get(slot) else { return 0 };
        if let Toplevel::X11(x) = &w.toplevel {
            if let Some(pid) = x.pid() {
                return pid;
            }
        }
        w.surface.client().and_then(|c| c.get_credentials(&self.dh).ok()).map_or(0, |c| c.pid as u32)
    }

    fn slots_open(&self) -> impl Iterator<Item = usize> + '_ {
        self.slots.iter().enumerate().filter(|(_, w)| w.is_some()).map(|(k, _)| k)
    }

    /// The window a target names: refused if it names none, or more than one.
    fn agent_target(&self, target: &str) -> Result<usize, &'static str> {
        let found: Vec<usize> = if let Some(pid) = target.strip_prefix("root:") {
            return self.agent_pick(pid);
        } else if let Some(pid) = target.strip_prefix("pid:") {
            let pid: u32 = pid.parse().map_err(|_| "bad-pid")?;
            self.slots_open().filter(|s| self.pid_of(*s) == pid).collect()
        } else {
            self.slots_open().filter(|s| self.slots[*s].as_ref().is_some_and(|w| w.app == target)).collect()
        };
        match found.as_slice() {
            [one] => Ok(*one),
            [] => Err(if target.starts_with("root:") { "unknown-root-pid" } else if target.starts_with("pid:") { "unknown-pid" } else { "unknown-app-id" }),
            _ => Err(if target.starts_with("root:") { "ambiguous-root-pid" } else if target.starts_with("pid:") { "ambiguous-pid" } else { "ambiguous-app-id" }),
        }
    }

    /// The window of a program the agent means: a dialog open for one of its
    /// windows first (its own «Save as», or a portal's file chooser that
    /// belongs to it) —that is where its input goes, and what a picture of
    /// it should show—; else its only window; else the one with the
    /// keyboard; else one that is seen.
    ///
    /// `PID.N` names one window of a program that has several (`windows`
    /// shows them so): that one, or the dialog it has open.
    fn agent_pick(&self, spec: &str) -> Result<usize, &'static str> {
        let (pid, one) = match spec.split_once('.') {
            Some((p, n)) => (p.parse::<u32>().map_err(|_| "bad-root-pid")?, Some(n.parse::<usize>().map_err(|_| "bad-window")?)),
            None => (spec.parse::<u32>().map_err(|_| "bad-root-pid")?, None),
        };
        // That process's own windows; those of the processes it started only
        // if it has none (a launcher's): a browser Discord opened a link in
        // is not Discord.
        let mine = |s: &usize| self.pid_of(*s) == pid;
        let kin = |s: &usize| in_family(self.pid_of(*s), pid);
        let of = |test: &dyn Fn(&usize) -> bool| -> Vec<usize> {
            match one {
                Some(slot) => self.slots_open().filter(|s| *s == slot && test(s)).collect(),
                None => self.slots_open().filter(|s| test(s)).collect(),
            }
        };
        let family = match of(&mine) {
            own if !own.is_empty() => own,
            _ => of(&kin),
        };
        if family.is_empty() {
            return Err("unknown-root-pid");
        }
        // Dialogs of dialogs too («Replace it?» over a «Save as»): the
        // deepest is the one waiting for an answer.
        let depth = |s: usize| -> Option<usize> {
            let mut at = s;
            for d in 1..8 {
                at = self.parent_slot(at)?;
                if family.contains(&at) {
                    return Some(d);
                }
            }
            None
        };
        let dialogs: Vec<(usize, usize)> = self.slots_open().filter(|s| !family.contains(s)).filter_map(|s| depth(s).map(|d| (s, d))).filter(|(s, _)| self.slots[*s].as_ref().is_some_and(|w| !w.minimized)).collect();
        if let Some(d) = self.focus.filter(|f| dialogs.iter().any(|(s, _)| s == f)).or_else(|| dialogs.iter().max_by_key(|(_, d)| *d).map(|(s, _)| *s)) {
            return Ok(d);
        }
        // Its own dialogs (a program's «Save as» of its own): the one with
        // the keyboard, or the deepest.
        let own: Vec<usize> = family.iter().copied().filter(|s| self.parent_slot(*s).is_some()).collect();
        if let Some(d) = self.focus.filter(|f| own.contains(f)).or_else(|| own.last().copied()) {
            return Ok(d);
        }
        if let [one] = family[..] {
            return Ok(one);
        }
        if let Some(f) = self.focus.filter(|f| family.contains(f)) {
            return Ok(f);
        }
        family.iter().copied().find(|s| self.agent_place(*s).is_some()).or(family.first().copied()).ok_or("unknown-root-pid")
    }

    /// Where a window is on the desktop, in units: the window itself, and its
    /// root surface (a program's own shadow included), which is what the
    /// agent's coordinates count from. None if the scene does not show it.
    fn agent_place(&self, slot: usize) -> Option<([f64; 2], [f64; 2], f64, usize)> {
        let w = self.slots.get(slot)?.as_ref()?;
        let (name, r) = w.shown.clone()?;
        let monitors = layers::monitors();
        let k = monitors.iter().position(|m| m.name == name)?;
        let m = &monitors[k];
        // How much the scene scales it (1 when laid out at its own size).
        let g = w.geometry;
        let zoom = if g[2] > 0 { r[2] as f64 / m.scale / g[2] as f64 } else { 1.0 };
        let ax = m.x as f64 + r[0] as f64 / m.scale;
        let ay = m.y as f64 + r[1] as f64 / m.scale;
        Some(([ax, ay], [ax - g[0] as f64 * zoom, ay - g[1] as f64 * zoom], zoom, k))
    }

    fn agent_query(&self, pid: u32) -> String {
        let focused = self.focus.map_or(0, |s| self.pid_of(s));
        let target = self.slots_open().find(|s| self.pid_of(*s) == pid);
        let state = match target {
            None => "not_found",
            Some(s) if Some(s) == self.focus => "foreground",
            Some(s) => {
                if self.agent_place(s).is_some() {
                    "background_visible"
                } else {
                    "background_occluded"
                }
            }
        };
        format!("state {focused} {state}")
    }

    fn agent_geometry(&self, pid: &str) -> Result<String, &'static str> {
        let slot = self.agent_pick(pid).map_err(|_| "target-not-found")?;
        let (window, root, _, _) = self.agent_place(slot).ok_or("unmapped-target")?;
        Ok(format!("geometry {} {} {} {}", window[0].round(), window[1].round(), root[0].round(), root[1].round()))
    }

    /// The box a picture of the window is cut from: its root surface (a
    /// program's own shadow included) where the scene shows it, so that a
    /// pixel of that picture is a point `m` understands. Said by the
    /// compositor, which is what draws it: an agent can trust it.
    fn agent_rect(&self, pid: &str) -> Result<String, &'static str> {
        let slot = self.agent_pick(pid).map_err(|_| "target-not-found")?;
        let r = self.agent_rect_of(slot);
        Ok(format!("rect {} {} {} {} {}", r.0, r.1, r.2, r.3, u8::from(r.4)))
    }

    fn agent_rect_of(&self, slot: usize) -> (i64, i64, i64, i64, bool) {
        let Some(Some(w)) = self.slots.get(slot) else { return (0, 0, 0, 0, false) };
        let Some((_, root, zoom, _)) = self.agent_place(slot) else { return (0, 0, 0, 0, false) };
        // From the root surface's corner to the window's far edges: the scene
        // draws the window only (not a program's own shadow), and starting at
        // the root's corner keeps a pixel of the picture a point `m` takes.
        let g = w.geometry;
        let (mut right, mut bottom) = (g[0] + g[2], g[1] + g[3]);
        // And its menus that hang out of it (a right-click menu near its
        // edge): a picture of the window shows them whole.
        for (popup, offset) in smithay::desktop::PopupManager::popups_for_surface(&w.surface) {
            let size = popup.geometry().size;
            right = right.max(g[0] + offset.x + size.w);
            bottom = bottom.max(g[1] + offset.y + size.h);
        }
        let (width, height) = ((right as f64 * zoom).round(), (bottom as f64 * zoom).round());
        (root[0].round() as i64, root[1].round() as i64, width as i64, height as i64, true)
    }

    /// Every window: its process, its box (as `r` says it), whether it is
    /// seen and whether it has the keyboard, its program and its title.
    fn agent_list(&self) -> String {
        // An empty title or program is «-»: a field of its own all the same.
        let hex = |s: &str| if s.is_empty() { "-".to_owned() } else { s.bytes().map(|b| format!("{b:02x}")).collect::<String>() };
        let mut out = String::from("windows");
        for slot in self.slots_open() {
            let Some(w) = self.slots[slot].as_ref() else { continue };
            let pid = self.pid_of(slot);
            let rect = self.agent_rect_of(slot);
            let focused = self.focus == Some(slot);
            let sep = if out.len() > "windows".len() { " |" } else { "" };
            // And, after the old fields: the monitor it is seen on (-1: not
            // seen) and, for a dialog, the process of the window it belongs to.
            let monitor = self.agent_place(slot).map_or(-1, |p| p.3 as i64);
            let parent = self.parent_slot(slot).map_or(0, |p| self.pid_of(p));
            out.push_str(&format!("{sep} {pid} {} {} {} {} {} {} {} {} {monitor} {parent} {slot}", rect.0, rect.1, rect.2, rect.3, u8::from(rect.4), u8::from(focused), hex(&w.app), hex(&w.title)));
        }
        out
    }

    /// `C [PID]`: what can be asked of a window, or of each: whether it is
    /// private, and where its menus are, for a scene's boxes in them.
    fn agent_can(&self, spec: Option<&str>) -> Result<String, &'static str> {
        let slots: Vec<usize> = match spec {
            Some(p) => vec![self.agent_pick(p)?],
            None => self.slots_open().collect(),
        };
        let mut out = format!("can {}", u8::from(layers::locked()));
        for slot in slots {
            let Some(w) = self.slots[slot].as_ref() else { continue };
            let pid = self.pid_of(slot);
            let g = w.geometry;
            let popups: Vec<String> = smithay::desktop::PopupManager::popups_for_surface(&w.surface)
                .map(|(popup, at)| {
                    let pg = popup.geometry();
                    format!("{},{},{},{}", g[0] + at.x - pg.loc.x, g[1] + at.y - pg.loc.y, pg.size.w, pg.size.h)
                })
                .collect();
            let popups = if popups.is_empty() { "-".to_owned() } else { popups.join(";") };
            out.push_str(&format!(" | {pid} {slot} {} {popups}", u8::from(layers::private(slot))));
        }
        Ok(out)
    }

    /// Where a new window of this process goes while the agent works with
    /// its program: on the monitor it works on (a window the program opened
    /// for it, a «New window»), not where your mouse happens to be.
    pub(super) fn agent_screen_for(&self, pid: u32) -> Option<usize> {
        let agent = self.agent.as_ref()?;
        let (root, screen) = agent.working?;
        let recent = agent.last.is_some_and(|t| t.elapsed() < Duration::from_secs(20));
        (recent && pid != 0 && in_family(pid, root)).then_some(screen)
    }

    /// A window just made: if it is of a program the agent opened, the
    /// monitor it goes to (and it does not take your keyboard).
    pub(super) fn agent_opened(&mut self, pid: u32, app: &str) -> Option<usize> {
        let agent = self.agent.as_mut()?;
        agent.opening.retain(|o| o.until > Instant::now());
        if agent.opening.is_empty() {
            return None;
        }
        // What the window is called: its app id (often not said yet), and
        // its process's program, as it runs and by its file.
        let mut names = words(app);
        if pid != 0 {
            names.extend(std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| words(&c)).unwrap_or_default());
            names.extend(std::fs::read_link(format!("/proc/{pid}/exe")).ok().and_then(|e| e.file_name().map(|f| words(&f.to_string_lossy()))).unwrap_or_default());
        }
        let o = agent.opening.iter().find(|o| (pid != 0 && in_family(pid, o.pid)) || words(&o.name).iter().any(|w| names.contains(w)))?;
        let screen = o.screen;
        // And what it opens next (a dialog, another window) with it.
        agent.working = Some((pid, screen));
        agent.last = Some(Instant::now());
        Some(screen)
    }

    /// `agent open`: the monitor is lit first, so you see where it will
    /// work, then the program starts there, without your keyboard. The
    /// monitor: the one asked for; else the one it is working on; else one
    /// your pointer is not on.
    fn agent_open(&mut self, monitor: i64, command: &str) -> Result<String, &'static str> {
        let n = self.outputs.len().max(1);
        let agent = self.agent.as_ref().ok_or("off")?;
        let screen = if monitor >= 0 && (monitor as usize) < n {
            monitor as usize
        } else if let Some((_, s)) = agent.working.filter(|_| agent.active && agent.last.is_some_and(|t| t.elapsed() < Duration::from_secs(90))) {
            s.min(n - 1)
        } else {
            (0..n).find(|k| *k != self.on_screen).unwrap_or(0)
        };
        // Lit before anything opens.
        if let Some(agent) = self.agent.as_mut() {
            agent.working = Some((0, screen));
            agent.last = Some(Instant::now());
            agent.busy = agent.busy.wrapping_add(1);
            let busy = agent.busy;
            for (name, v) in [("agent.win", -1.0), ("agent.screen", screen as f64), ("agent.seen", busy as f64)] {
                let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern(name), v as f32));
            }
        }
        self.agent_still_working();
        let pid = self.launch(command).ok_or("could-not-start")?;
        // Known by the name it was started with: the first word that is not
        // `env`, a variable or a wrapper, without its folder.
        let name = command
            .split_whitespace()
            .find(|w| !w.contains('=') && !matches!(*w, "env" | "exec" | "setsid" | "nohup" | "--"))
            .map(|w| w.rsplit('/').next().unwrap_or(w).to_lowercase())
            .unwrap_or_default();
        if let Some(agent) = self.agent.as_mut() {
            agent.working = Some((pid, screen));
            agent.opening.retain(|o| o.until > Instant::now());
            agent.opening.push(Opening { pid, name, screen, until: Instant::now() + OPENING });
        }
        println!("agent · opened «{command}» on monitor {screen}");
        Ok(format!("opened {pid} {screen}"))
    }

    /// Whether a window of this process must leave your keyboard where it is:
    /// it is of the program the agent is working with this moment (it did
    /// something with it in the last few seconds), and your keyboard is in
    /// another program. A dialog its keys opened, a window asking to come
    /// forward after its click, did take it.
    pub(super) fn agent_keeps_your_keyboard(&self, pid: u32) -> bool {
        self.agent_keeps_your_keyboard_for(pid, self.focus)
    }

    /// The same, with your keyboard in `yours`.
    pub(super) fn agent_keeps_your_keyboard_for(&self, pid: u32, yours: Option<usize>) -> bool {
        let Some(a) = self.agent.as_ref() else { return false };
        let Some((root, _)) = a.working else { return false };
        let recent = a.last.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        if !recent || pid == 0 || root == 0 || !(in_family(pid, root) || in_family(root, pid)) {
            return false;
        }
        let yours = yours.map_or(0, |s| self.pid_of(s));
        !(yours != 0 && (in_family(yours, root) || in_family(root, yours)))
    }

    /// Text pasted into a Chromium program (Discord, a browser): put on the
    /// clipboard, Ctrl+V with your keyboard lent to the window for that key
    /// —what is copied goes with the keyboard: only the window that has it is
    /// offered it—, and what you had copied given back at once. Typed by
    /// number instead (Ctrl+Shift+U), an emoji was written, and Discord took
    /// the Ctrl+Shift+U as its own «upload a file» all the same.
    /// Ok(false): what you had copied could not be kept to give it back (an
    /// X11 program's, or one that did not hand it over in time), and nothing
    /// was done.
    fn agent_paste_with_yours(&mut self, slot: usize, text: &str) -> Result<bool, &'static str> {
        use smithay::wayland::selection::data_device::set_data_device_selection;
        let back = match &self.clipboard {
            super::Clipboard::Nothing => None,
            super::Clipboard::Kept(k) => Some(k.clone()),
            super::Clipboard::X11 => return Ok(false),
            super::Clipboard::Program(kinds) => {
                let kinds = kinds.clone();
                match super::read_copied(self, &kinds) {
                    Some(k) => Some(super::Kept::new(k)),
                    None => return Ok(false),
                }
            }
        };
        let seat = self.seat.clone();
        let kinds = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];
        let pasted = super::Kept::new(kinds.iter().map(|k| (k.to_string(), text.as_bytes().to_vec())).collect());
        set_data_device_selection(&self.dh, &seat, kinds.iter().map(|k| k.to_string()).collect(), super::Copied::Kept(pasted.clone()));
        self.clipboard = super::Clipboard::Kept(pasted.clone());
        // An X11 program pastes through XWayland: it is told too.
        super::x11::wayland_copied(self, smithay::wayland::selection::SelectionTarget::Clipboard, Some(pasted.kinds()));
        let lent = self.agent_with_your_keyboard(slot, &[(vec![29], 47)]);
        // What you had copied comes back once the window has read the text
        // (`agent_lines`): before, one that has your keyboard is offered it at
        // once, and pasted that instead.
        match lent {
            Ok(()) => {
                if let Some(a) = self.agent.as_mut() {
                    a.pasting = Some((pasted, back));
                }
            }
            Err(_) => self.agent_give_back(&pasted, back),
        }
        lent.map(|_| true)
    }

    /// What was copied before the agent pasted, back on the clipboard (or
    /// nothing, if nothing was) —unless something else was copied meanwhile:
    /// that one stays—.
    fn agent_give_back(&mut self, pasted: &std::sync::Arc<super::Kept>, back: Option<std::sync::Arc<super::Kept>>) {
        use smithay::wayland::selection::data_device::{clear_data_device_selection, current_data_device_selection_userdata, set_data_device_selection};
        use smithay::wayland::selection::SelectionTarget;
        let seat = self.seat.clone();
        let still = current_data_device_selection_userdata(&seat).is_some_and(|c| matches!(&*c, super::Copied::Kept(k) if std::sync::Arc::ptr_eq(k, pasted)));
        if !still {
            return;
        }
        match back {
            Some(k) => {
                let kinds = k.kinds();
                set_data_device_selection(&self.dh, &seat, kinds.clone(), super::Copied::Kept(k.clone()));
                self.clipboard = super::Clipboard::Kept(k);
                super::x11::wayland_copied(self, SelectionTarget::Clipboard, Some(kinds));
            }
            None => {
                clear_data_device_selection(&self.dh, &seat);
                self.clipboard = super::Clipboard::Nothing;
                super::x11::wayland_copied(self, SelectionTarget::Clipboard, None);
            }
        }
    }

    /// A character typed by number (Ctrl+Shift+U, its code, a space) into a
    /// Chromium program, with your keyboard lent to it for those keys: what
    /// is left when what you copied cannot be kept to give it back.
    fn agent_by_number_with_yours(&mut self, slot: usize, c: char) -> Result<(), &'static str> {
        let mut presses: Vec<(Vec<u32>, u32)> = vec![(vec![29, 42], 22)];
        for d in format!("{:x}", c as u32).bytes() {
            presses.push((Vec::new(), us_key(d).map_or(57, |(code, _)| code)));
        }
        presses.push((Vec::new(), 57));
        self.agent_with_your_keyboard(slot, &presses)
    }

    /// Keys pressed into a window with your keyboard lent to it, and given
    /// back at once, without telling anyone. Chromium keeps them to itself
    /// then (with the agent's own keys, Discord saw a Ctrl+Shift+U as its
    /// shortcut), and the window is offered what is copied. The window is
    /// entered again for the agent's keys after: lending yours told it
    /// `leave` when it came back.
    fn agent_with_your_keyboard(&mut self, slot: usize, presses: &[(Vec<u32>, u32)]) -> Result<(), &'static str> {
        let root = self.slots.get(slot).and_then(Option::as_ref).ok_or("gone")?.surface.clone();
        let client = root.client().ok_or("gone")?;
        let keyboard = self.keyboard.clone();
        let before = keyboard.current_focus();
        keyboard.set_focus(self, Some(root.clone()), SERIAL_COUNTER.next_serial());
        for (held, code) in presses {
            for (code, down) in held.iter().map(|c| (*c, true)).chain(std::iter::once((*code, true))).chain(std::iter::once((*code, false))).chain(held.iter().rev().map(|c| (*c, false))) {
                let state = if down { KeyState::Pressed } else { KeyState::Released };
                let time = self.time();
                keyboard.input::<(), _>(self, (code + 8).into(), state, SERIAL_COUNTER.next_serial(), time, |_, _, _| FilterResult::Forward);
            }
        }
        keyboard.set_focus(self, before.clone(), SERIAL_COUNTER.next_serial());
        // In again for the agent's keys, where it was.
        if before.as_ref() != Some(&root) {
            for k in self.keyboard.client_keyboards(&client) {
                k.enter(SERIAL_COUNTER.next_serial().into(), &root, Vec::new());
                k.modifiers(SERIAL_COUNTER.next_serial().into(), 0, 0, 0, 0);
            }
            if let Some(a) = self.agent.as_mut() {
                a.kb_entered = Some(root);
                a.kb_slot = Some(slot);
                a.kb_real = before;
            }
        }
        Ok(())
    }

    /// The monitors, in the order `windows` numbers them.
    fn agent_monitors(&self) -> String {
        let mut out = String::from("monitors");
        for (k, m) in layers::monitors().iter().enumerate() {
            let sep = if k > 0 { " |" } else { "" };
            let (w, h) = ((m.size.0 as f64 / m.scale).round(), (m.size.1 as f64 / m.scale).round());
            out.push_str(&format!("{sep} {k} {} {} {w} {h} {}", m.x, m.y, m.name));
        }
        out
    }

    /// Where a cursor is on the desktop, in units.
    fn agent_where(&self, idx: usize) -> String {
        match self.agent.as_ref().and_then(|a| a.global.get(idx).copied().flatten()) {
            Some((x, y)) => format!("at {x:.1} {y:.1}"),
            None => "at none".to_owned(),
        }
    }

    /// A window to another monitor, as the user asked («open it on the other one»).
    fn agent_send(&mut self, pid: &str, monitor: usize) -> Result<(), &'static str> {
        let slot = self.agent_pick(pid)?;
        if monitor >= self.outputs.len() {
            return Err("unknown-monitor");
        }
        self.agent_busy(slot);
        self.handle(ToNest::Send(slot, monitor));
        Ok(())
    }

    fn agent_activate(&mut self, pid: &str) -> Result<(), &'static str> {
        let slot = self.agent_pick(pid)?;
        self.set_focus(Some(slot));
        // On another workspace, the scene goes there.
        self.tell(NestEvent::Reveal(slot));
        Ok(())
    }

    /// The scene is told the agent is at work, on which window and monitor:
    /// it lights that monitor's edges and that window's outline while it is.
    fn agent_busy(&mut self, slot: usize) {
        if self.agent.as_ref().is_some_and(|a| a.quiet) {
            return;
        }
        let pid = self.pid_of(slot);
        let screen = self.slots.get(slot).and_then(Option::as_ref).map(|w| w.screen);
        let Some(agent) = self.agent.as_mut() else { return };
        if let Some(screen) = screen {
            agent.working = Some((pid, screen));
        }
        agent.last = Some(Instant::now());
        agent.busy = agent.busy.wrapping_add(1);
        let busy = agent.busy;
        let monitor = self.agent_place(slot).map_or(-1.0, |p| p.3 as f64);
        for (name, v) in [("agent.win", slot as f64), ("agent.screen", monitor), ("agent.seen", busy as f64)] {
            let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern(name), v as f32));
        }
        self.agent_still_working();
    }

    /// Finished, as the agent says: nothing to wait for.
    fn agent_stopped(&mut self) -> bool {
        let Some(agent) = self.agent.as_mut() else { return false };
        match agent.stopped {
            Some(at) if at.elapsed() < Duration::from_secs(60) => true,
            Some(_) => {
                agent.stopped = None;
                false
            }
            None => false,
        }
    }

    fn agent_done(&mut self) {
        let Some(agent) = self.agent.as_mut() else { return };
        agent.last = None;
        agent.peer = 0;
        self.agent_still_working();
        self.agent_keyboard_leave();
    }

    /// The window its keys went to is left (unless it is yours now: then
    /// its focus is your keyboard's).
    fn agent_keyboard_leave(&mut self) {
        // Not active any more, unless it is the window you are in.
        if let Some(slot) = self.agent.as_mut().and_then(|a| a.kb_slot.take()) {
            if self.focus != Some(slot) {
                if let Some(w) = self.slots.get(slot).and_then(Option::as_ref) {
                    w.toplevel.set_activated(false);
                }
            }
        }
        let Some(old) = self.agent.as_mut().and_then(|a| a.kb_entered.take()) else { return };
        if !old.is_alive() || self.keyboard.current_focus().as_ref() == Some(&old) {
            return;
        }
        let Some(client) = old.client() else { return };
        for k in self.keyboard.client_keyboards(&client) {
            k.leave(SERIAL_COUNTER.next_serial().into(), &old);
        }
    }

    /// At work, as long as it may be thinking what to do next: a minute and a
    /// half after its last action, or ten minutes while the process that
    /// acted (a daemon such as `cua-driver serve`) is still there. The scene
    /// keeps the monitor's light on meanwhile, and lets it go after.
    fn remote_mark(&mut self, on: Option<(i32, String)>) {
        let Some(agent) = self.agent.as_mut() else { return };
        let was = agent.remote.is_some();
        agent.remote = on.as_ref().map(|_| Instant::now());
        let fact = |name: &str, v: f32| ToRender::Fact(pleamar::scene::intern(name), v);
        match on {
            Some((monitor, who)) => {
                if !was {
                    println!("remote · this desktop is being used from {who}");
                }
                let _ = self.to_render.send(fact("remote.screen", monitor as f32));
                let _ = self.to_render.send(ToRender::Text(pleamar::scene::intern("remote.who"), who));
                let _ = self.to_render.send(fact("remote.on", 1.0));
            }
            None => {
                if was {
                    println!("remote · no one is using this desktop from elsewhere now");
                }
                let _ = self.to_render.send(fact("remote.on", 0.0));
            }
        }
    }

    fn agent_still_working(&mut self) {
        // Not heard from the remote desktop for a while: it is not there.
        if self.agent.as_ref().and_then(|a| a.remote).is_some_and(|t| t.elapsed() > Duration::from_secs(15)) {
            self.remote_mark(None);
        }
        let Some(agent) = self.agent.as_mut() else { return };
        let idle = agent.last.map_or(Duration::MAX, |t| t.elapsed());
        let alive = agent.peer != 0 && std::path::Path::new(&format!("/proc/{}", agent.peer)).exists();
        let working = idle < Duration::from_secs(90) || (alive && idle < Duration::from_secs(600));
        if working != agent.active {
            agent.active = working;
            // The pill says the pace, from the session's start too.
            let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern("agent.pace"), agent.pace as u8 as f32));
            let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern("agent.active"), if working { 1.0 } else { 0.0 }));
            if !working {
                self.agent_keyboard_leave();
            }
        }
    }

    /// The scene is told where cursor `idx` is, on which monitor, and that
    /// it did something (it draws it while it does).
    fn agent_show(&mut self, slot: usize, idx: usize, at: Point<f64, Logical>, down: Option<bool>) {
        self.agent_busy(slot);
        let Some(agent) = self.agent.as_mut() else { return };
        agent.seen[idx] = agent.seen[idx].wrapping_add(1);
        let seen = agent.seen[idx];
        let Some((_, root, zoom, monitor)) = self.agent_place(slot) else { return };
        let m = layers::monitors().get(monitor).map(|m| (m.x as f64, m.y as f64)).unwrap_or_default();
        let fact = |name: String, v: f64| {
            let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern(&name), v as f32));
        };
        if let Some(a) = self.agent.as_mut() {
            a.global[idx] = Some((root[0] + at.x * zoom, root[1] + at.y * zoom));
        }
        fact(format!("agent.{idx}.x"), root[0] + at.x * zoom - m.0);
        fact(format!("agent.{idx}.y"), root[1] + at.y * zoom - m.1);
        fact(format!("agent.{idx}.screen"), monitor as f64);
        if let Some(down) = down {
            fact(format!("agent.{idx}.down"), if down { 1.0 } else { 0.0 });
        }
        fact(format!("agent.{idx}.seen"), seen as f64);
    }

    fn agent_motion(&mut self, slot: usize, idx: usize, x: f64, y: f64) -> Result<(), &'static str> {
        if idx >= CURSORS {
            return Err("bad-cursor");
        }
        let w = self.slots[slot].as_ref().ok_or("gone")?;
        let (root, g) = (w.surface.clone(), w.geometry);
        let at: Point<f64, Logical> = (x, y).into();
        let (surface, origin) = self.surface_under(&root, g, at).map(|(s, o)| (s, o.to_f64())).ok_or("no-surface-at-point")?;
        let client = surface.client().ok_or("gone")?;
        let (serial, time) = (SERIAL_COUNTER.next_serial(), self.time());
        let agent = self.agent.as_ref().ok_or("off")?;
        let own = agent.seats.get(idx).map(|s| s.1.clone()).filter(|p| p.client_pointers(&client).next().is_some());
        if let Some(pointer) = own {
            if let Some(a) = self.agent.as_mut() {
                a.raw_entered[idx] = None;
            }
            let entering = pointer.current_focus().as_ref() != Some(&surface);
            pointer.motion(self, Some((surface.clone(), origin)), &MotionEvent { location: at, serial, time });
            pointer.frame(self);
            // Entering a surface is said with `enter` alone; a program's menu
            // lights the item under the pointer (and takes its press) on
            // `motion`, which a hand always makes after entering: so it is
            // made here too.
            if entering {
                pointer.motion(self, Some((surface, origin)), &MotionEvent { location: at, serial: SERIAL_COUNTER.next_serial(), time: time + 1 });
                pointer.frame(self);
            }
        } else {
            let local = at - origin;
            let pointers: Vec<wl_pointer::WlPointer> = self.pointer.client_pointers(&client).collect();
            if pointers.is_empty() {
                return Err("no-pointer-resource");
            }
            // Your own pointer speaks through these same objects: if it has
            // gone into the program or out of it since the agent last said
            // where it was, the program no longer thinks that (it was told
            // `leave`, and went on ignoring the agent's motions and presses:
            // Discord, after your pointer had crossed it).
            let real = self.pointer.current_focus();
            let mut entered = self.agent.as_ref().and_then(|a| a.raw_entered[idx].clone());
            if self.agent.as_ref().is_some_and(|a| a.raw_real[idx] != real) {
                entered = None;
            }
            if entered.as_ref() != Some(&surface) {
                if let Some(old) = entered.filter(|s| s.is_alive()) {
                    if let Some(c) = old.client() {
                        for p in self.pointer.client_pointers(&c) {
                            p.leave(SERIAL_COUNTER.next_serial().into(), &old);
                            frame(&p);
                        }
                    }
                }
                // Yours in that program now: the agent takes over from it
                // (yours leaves, the agent's enters), or, on the very same
                // surface, is already in.
                let yours = real.as_ref().filter(|s| s.client().as_ref() == Some(&client) && s.is_alive());
                if yours != Some(&surface) {
                    for p in &pointers {
                        if let Some(s) = yours {
                            p.leave(SERIAL_COUNTER.next_serial().into(), s);
                        }
                        p.enter(SERIAL_COUNTER.next_serial().into(), &surface, local.x, local.y);
                        frame(p);
                    }
                }
                if let Some(a) = self.agent.as_mut() {
                    a.raw_entered[idx] = Some(surface.clone());
                    a.raw_real[idx] = real.clone();
                }
            }
            for p in &pointers {
                p.motion(time, local.x, local.y);
                frame(p);
            }
        }
        if let Some(a) = self.agent.as_mut() {
            a.at[idx] = at;
        }
        self.agent_show(slot, idx, at, None);
        Ok(())
    }

    fn agent_button(&mut self, slot: usize, idx: usize, button: u32, pressed: bool) -> Result<(), &'static str> {
        if idx >= CURSORS || !(272..=279).contains(&button) {
            return Err("bad-args");
        }
        let (serial, time) = (SERIAL_COUNTER.next_serial(), self.time());
        let agent = self.agent.as_ref().ok_or("off")?;
        let (_, pointer, _) = agent.seats.get(idx).cloned().ok_or("bad-cursor")?;
        let raw = agent.raw_entered[idx].clone();
        if let Some(surface) = raw {
            let client = surface.client().ok_or("gone")?;
            let state = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
            let mut any = false;
            for p in self.pointer.client_pointers(&client) {
                p.button(serial.into(), time, button, state);
                frame(&p);
                any = true;
            }
            if !any {
                return Err("no-pointer-resource");
            }
        } else if pointer.current_focus().is_some() {
            let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
            pointer.button(self, &ButtonEvent { serial, time, button, state });
            pointer.frame(self);
        } else {
            return Err("no-pointer-resource");
        }
        // A press of the agent's does not change whose the keyboard is: it stays yours.
        let at = self.agent.as_ref().map_or((0.0, 0.0).into(), |a| a.at[idx]);
        self.agent_show(slot, idx, at, Some(pressed));
        Ok(())
    }

    fn agent_axis(&mut self, slot: usize, idx: usize, horizontal: bool, value: f64) -> Result<(), &'static str> {
        if idx >= CURSORS || !value.is_finite() {
            return Err("bad-args");
        }
        self.agent_busy(slot);
        let time = self.time();
        let agent = self.agent.as_ref().ok_or("off")?;
        let (_, pointer, _) = agent.seats.get(idx).cloned().ok_or("bad-cursor")?;
        let raw = agent.raw_entered[idx].clone();
        let step = if value < 0.0 { -1 } else { 1 };
        if raw.is_none() && pointer.current_focus().is_some() {
            let axis = if horizontal { Axis::Horizontal } else { Axis::Vertical };
            let frame = AxisFrame::new(time).source(AxisSource::Wheel).value(axis, value).v120(axis, step * 120);
            pointer.axis(self, frame);
            pointer.frame(self);
        } else {
            let surface = raw.ok_or("no-pointer-resource")?;
            let client = surface.client().ok_or("gone")?;
            let axis = if horizontal { wl_pointer::Axis::HorizontalScroll } else { wl_pointer::Axis::VerticalScroll };
            let mut any = false;
            for p in self.pointer.client_pointers(&client) {
                if p.version() >= 5 {
                    p.axis_source(wl_pointer::AxisSource::Wheel);
                }
                if p.version() >= 8 {
                    p.axis_value120(axis, step * 120);
                } else if p.version() >= 5 {
                    p.axis_discrete(axis, step);
                }
                p.axis(time, axis, value);
                frame(&p);
                any = true;
            }
            if !any {
                return Err("no-pointer-resource");
            }
        }
        Ok(())
    }

    /// Keys into a window: held modifiers, then the key, all let go.
    fn agent_keys(&mut self, slot: usize, held: &[u32], code: u32) -> Result<(), &'static str> {
        self.agent_press(slot, &[(held.to_vec(), code)])
    }

    /// ASCII text, each character as the keys that make it: on a US
    /// keyboard, or on yours if it goes through yours.
    fn agent_type(&mut self, slot: usize, text: &[u8]) -> Result<(), &'static str> {
        // Anything but ASCII (an accent, a ñ, an emoji): with a keymap made
        // for the text, as wtype does.
        if !text.is_ascii() {
            let text = std::str::from_utf8(text).map_err(|_| "bad-utf8")?;
            return self.agent_type_any(slot, text);
        }
        let yours = self.agent_through_yours(slot);
        let table = if yours { Some(self.your_keymap(|k| char_table(k))) } else { None };
        let mut presses = Vec::with_capacity(text.len());
        for &c in text {
            let key = match &table {
                Some(t) => t.get(c as usize).copied().flatten(),
                None => us_key(c),
            };
            // One the layout has no plain key for (a backtick is a dead key
            // in Spanish): the whole text with a keymap made for it.
            let Some((code, shift)) = key else {
                let text = std::str::from_utf8(text).map_err(|_| "bad-utf8")?;
                return self.agent_type_any(slot, text);
            };
            presses.push((if shift { vec![42] } else { vec![] }, code));
        }
        self.agent_press(slot, &presses)
    }

    /// Any text: a keymap with a key of its own for each character in it
    /// (alone on the key, no modifier needed), handed to the program for
    /// the while, and the usual one back after. The keys are ones that only
    /// write: a browser takes what a key is from where it sits, and a
    /// character put on Escape, Backspace or Control was lost or erased the
    /// one before. So in pieces of as many different characters as there are
    /// such keys.
    fn agent_type_any(&mut self, slot: usize, text: &str) -> Result<(), &'static str> {
        // Chromium (a browser, Discord, any Electron program) cuts what a key
        // writes to 16 bits: an emoji came out as a blank. A text with one is
        // pasted whole, at once (a line break in it is a line break, not an
        // Enter); typed by number, Ctrl+Shift+U, only if what you copied
        // cannot be kept to give it back.
        let chromium = is_chromium(self.pid_of(slot));
        if chromium && text.chars().any(|c| c as u32 > 0xFFFF) {
            self.agent_busy(slot);
            if self.agent_paste_with_yours(slot, text)? {
                return Ok(());
            }
        }
        let mut run = String::new();
        for c in text.chars() {
            if chromium && c as u32 > 0xFFFF {
                if !run.is_empty() {
                    self.agent_type_keys(slot, &run)?;
                    run.clear();
                }
                if self.agent_through_yours(slot) {
                    self.agent_press(slot, &[(vec![29, 42], 22)])?;
                    self.agent_type(slot, format!("{:x} ", c as u32).as_bytes())?;
                } else {
                    self.agent_busy(slot);
                    self.agent_by_number_with_yours(slot, c)?;
                }
            } else {
                run.push(c);
            }
        }
        if run.is_empty() { Ok(()) } else { self.agent_type_keys(slot, &run) }
    }

    fn agent_type_keys(&mut self, slot: usize, text: &str) -> Result<(), &'static str> {
        let chars: Vec<char> = text.chars().collect();
        let mut at = 0;
        while at < chars.len() {
            let mut keys: Vec<(char, u32)> = Vec::new();
            let mut free = WRITING_KEYS.iter();
            let mut end = at;
            while end < chars.len() {
                let c = chars[end];
                if !keys.iter().any(|(k, _)| *k == c) {
                    let code = match c {
                        ' ' => 57,
                        '\n' => 28,
                        '\t' => 15,
                        _ => match free.next() {
                            Some(code) => *code,
                            None => break,
                        },
                    };
                    keys.push((c, code));
                }
                end += 1;
            }
            let keymap = text_keymap(&keys).ok_or("no-keymap")?;
            let presses: Vec<(Vec<u32>, u32)> = chars[at..end].iter().map(|c| (Vec::new(), keys.iter().find(|(k, _)| k == c).map_or(57, |(_, code)| *code))).collect();
            self.agent_press_with(slot, &presses, Some(&keymap))?;
            at = end;
        }
        Ok(())
    }

    /// Something read from your keyboard's layout.
    fn your_keymap<T>(&mut self, f: impl Fn(&xkb::Keymap) -> T) -> T {
        let keyboard = self.keyboard.clone();
        keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            f(unsafe { xkb.keymap() })
        })
    }

    /// Whether keys for that window go through your keyboard: the program
    /// does not hear the agent's, and the window is the one you are typing in.
    fn agent_through_yours(&self, slot: usize) -> bool {
        let Some(Some(w)) = self.slots.get(slot) else { return false };
        let Some(client) = w.surface.client() else { return false };
        let own = self.agent.as_ref().and_then(|a| a.seats.first()).is_some_and(|s| s.2.client_keyboards(&client).next().is_some());
        // Your keyboard only if it is on that window now: a program's surface
        // of its own (Marea's chat) may have it, and the keys went there.
        !own && self.keyboard.current_focus().as_ref() == Some(&w.surface)
    }

    fn agent_press(&mut self, slot: usize, presses: &[(Vec<u32>, u32)]) -> Result<(), &'static str> {
        self.agent_press_with(slot, presses, None)
    }

    /// Keys, with the agent's keymap or (`special`) one made for them.
    fn agent_press_with(&mut self, slot: usize, presses: &[(Vec<u32>, u32)], special: Option<&str>) -> Result<(), &'static str> {
        self.agent_busy(slot);
        let w = self.slots[slot].as_ref().ok_or("gone")?;
        let root = w.surface.clone();
        let client = root.client().ok_or("gone")?;
        let agent = self.agent.as_ref().ok_or("off")?;
        let own = agent.seats.first().map(|s| s.2.clone()).filter(|k| k.client_keyboards(&client).next().is_some());
        let mine = self.agent_through_yours(slot);
        // The agent's own keyboard (US), which the program hears apart from yours.
        if let Some(keyboard) = own {
            if keyboard.current_focus().as_ref() != Some(&root) {
                keyboard.set_focus(self, Some(root.clone()), SERIAL_COUNTER.next_serial());
            }
            if let Some(map) = special {
                keyboard.set_keymap_from_string(self, map.to_owned()).map_err(|_| "bad-keymap")?;
            }
            for (held, code) in presses {
                for (code, down) in held.iter().map(|c| (*c, true)).chain(std::iter::once((*code, true))).chain(std::iter::once((*code, false))).chain(held.iter().rev().map(|c| (*c, false))) {
                    let state = if down { KeyState::Pressed } else { KeyState::Released };
                    let time = self.time();
                    keyboard.input::<(), _>(self, (code + 8).into(), state, SERIAL_COUNTER.next_serial(), time, |_, _, _| FilterResult::Forward);
                }
            }
            if special.is_some() {
                let _ = keyboard.set_keymap_from_string(self, us_keymap_string());
            }
            return Ok(());
        }
        // If it is the window you are typing in, as if you typed it (the
        // characters were already looked up in your layout).
        if mine && special.is_none() {
            if self.agent.as_ref().is_some_and(|a| a.kb_entered.as_ref() != Some(&root)) {
                self.agent_keyboard_leave();
            }
            let keyboard = self.keyboard.clone();
            for (held, code) in presses {
                for (code, down) in held.iter().map(|c| (*c, true)).chain(std::iter::once((*code, true))).chain(std::iter::once((*code, false))).chain(held.iter().rev().map(|c| (*c, false))) {
                    let state = if down { KeyState::Pressed } else { KeyState::Released };
                    let time = self.time();
                    keyboard.input::<(), _>(self, (code + 8).into(), state, SERIAL_COUNTER.next_serial(), time, |_, _, _| FilterResult::Forward);
                }
            }
            return Ok(());
        }
        // Not yours: straight to its keyboard objects, with the US keymap for
        // the while and yours back after. Entered once and left when the
        // agent goes to another window or is done (`agent_keyboard_leave`).
        let keyboards: Vec<wl_keyboard::WlKeyboard> = self.keyboard.client_keyboards(&client).collect();
        if keyboards.is_empty() {
            return Err("no-keyboard-resource");
        }
        let real = self.keyboard.current_focus();
        let entered = self.agent.as_ref().and_then(|a| a.kb_entered.clone());
        if entered.as_ref() != Some(&root) {
            self.agent_keyboard_leave();
        }
        let still = !mine && entered.as_ref() == Some(&root) && self.agent.as_ref().is_some_and(|a| a.kb_real == real);
        // Active while the agent types in it, said before the keys.
        if !mine {
            if let Some(w) = self.slots.get(slot).and_then(Option::as_ref) {
                w.toplevel.set_activated(true);
            }
        }
        let keymap = self.your_keymap(|k| k.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1));
        let us_keymap = special.map_or_else(us_keymap_string, str::to_owned);
        let us = keymap_fd(&us_keymap).ok_or("no-keymap")?;
        let yours = keymap_fd(&keymap);
        let time = self.time();
        for k in &keyboards {
            k.keymap(wl_keyboard::KeymapFormat::XkbV1, us.0.as_fd(), us.1);
            // The window you are typing in already has its keyboard entered,
            // and so has the one the agent is typing in.
            if !mine && !still {
                k.enter(SERIAL_COUNTER.next_serial().into(), &root, Vec::new());
            }
            k.modifiers(SERIAL_COUNTER.next_serial().into(), 0, 0, 0, 0);
        }
        for (held, code) in presses {
            let mask = held.iter().fold(0u32, |m, c| m | modifier_mask(*c));
            for k in &keyboards {
                if mask != 0 {
                    k.modifiers(SERIAL_COUNTER.next_serial().into(), mask, 0, 0, 0);
                }
                k.key(SERIAL_COUNTER.next_serial().into(), time, *code, wl_keyboard::KeyState::Pressed);
                k.key(SERIAL_COUNTER.next_serial().into(), time, *code, wl_keyboard::KeyState::Released);
                if mask != 0 {
                    k.modifiers(SERIAL_COUNTER.next_serial().into(), 0, 0, 0, 0);
                }
            }
        }
        for k in &keyboards {
            if let Some(y) = &yours {
                k.keymap(wl_keyboard::KeymapFormat::XkbV1, y.0.as_fd(), y.1);
            }
        }
        if let Some(a) = self.agent.as_mut() {
            a.kb_entered = (!mine).then(|| root.clone());
            a.kb_slot = (!mine).then_some(slot);
            a.kb_real = real;
        }
        Ok(())
    }

    /// A click at a point of the desktop: on the window the scene shows there.
    /// The agent's hand on a program's surface that is no window (a panel):
    /// its own seat's pointer goes there, as on a window, and its cursor is
    /// drawn where it is, with the monitor lit.
    fn agent_panel_motion(&mut self, pid: u32, size: (i32, i32), idx: usize, x: f64, y: f64) -> Result<(), &'static str> {
        if idx >= CURSORS {
            return Err("bad-cursor");
        }
        let (_, root, monitor, (px, py)) = self.panel_of(pid, size).ok_or("no-surface")?;
        let (_, pointer, _) = self.agent.as_ref().ok_or("off")?.seats.get(idx).cloned().ok_or("bad-cursor")?;
        let (serial, time) = (SERIAL_COUNTER.next_serial(), self.time());
        let at: Point<f64, Logical> = (x, y).into();
        pointer.motion(self, Some((root, (0.0, 0.0).into())), &MotionEvent { location: at, serial, time });
        pointer.frame(self);
        self.agent_show_on(monitor, idx, (px as f64 + x, py as f64 + y), None);
        Ok(())
    }

    fn agent_panel_button(&mut self, pid: u32, size: (i32, i32), idx: usize, button: u32, pressed: bool) -> Result<(), &'static str> {
        if idx >= CURSORS || !(272..=279).contains(&button) {
            return Err("bad-args");
        }
        let (_, root, monitor, _) = self.panel_of(pid, size).ok_or("no-surface")?;
        let (_, pointer, _) = self.agent.as_ref().ok_or("off")?.seats.get(idx).cloned().ok_or("bad-cursor")?;
        if pointer.current_focus().as_ref() != Some(&root) {
            return Err("no-pointer-resource");
        }
        let (serial, time) = (SERIAL_COUNTER.next_serial(), self.time());
        let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
        pointer.button(self, &ButtonEvent { serial, time, button, state });
        pointer.frame(self);
        let at = self.agent.as_ref().and_then(|a| a.on_monitor[idx]).map_or((0.0, 0.0), |(_, p)| p);
        self.agent_show_on(monitor, idx, at, Some(pressed));
        Ok(())
    }

    /// The agent's cursor drawn at a point of a monitor, in its units, and the
    /// monitor lit: what `agent_show` does for a window, for anything.
    fn agent_show_on(&mut self, monitor: usize, idx: usize, at: (f64, f64), down: Option<bool>) {
        let Some(agent) = self.agent.as_mut() else { return };
        agent.last = Some(Instant::now());
        agent.busy = agent.busy.wrapping_add(1);
        agent.seen[idx] = agent.seen[idx].wrapping_add(1);
        let (busy, seen) = (agent.busy, agent.seen[idx]);
        agent.on_monitor[idx] = Some((monitor, at));
        let m = layers::monitors().get(monitor).map(|m| (m.x as f64, m.y as f64)).unwrap_or_default();
        agent.global[idx] = Some((m.0 + at.0, m.1 + at.1));
        let facts = [
            ("agent.win".to_owned(), -1.0),
            ("agent.screen".to_owned(), monitor as f64),
            ("agent.seen".to_owned(), busy as f64),
            (format!("agent.{idx}.x"), at.0),
            (format!("agent.{idx}.y"), at.1),
            (format!("agent.{idx}.screen"), monitor as f64),
            (format!("agent.{idx}.seen"), seen as f64),
        ];
        for (name, v) in facts {
            let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern(&name), v as f32));
        }
        if let Some(down) = down {
            let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern(&format!("agent.{idx}.down")), if down { 1.0 } else { 0.0 }));
        }
        self.agent_still_working();
    }

    fn agent_desktop_click(&mut self, x: f64, y: f64, count: u32, button: u32) -> Result<(), &'static str> {
        let mut hit = None;
        for slot in self.slots_open().collect::<Vec<_>>() {
            let Some((window, root, zoom, _)) = self.agent_place(slot) else { continue };
            let g = self.slots[slot].as_ref().map_or([0; 4], |w| w.geometry);
            let inside = x >= window[0] && y >= window[1] && x < window[0] + g[2] as f64 * zoom && y < window[1] + g[3] as f64 * zoom;
            // The one with the keyboard first, if several are there.
            if inside && (hit.is_none() || self.focus == Some(slot)) {
                hit = Some((slot, (x - root[0]) / zoom, (y - root[1]) / zoom));
            }
        }
        let (slot, lx, ly) = hit.ok_or("no-surface-at-point")?;
        self.agent_motion(slot, 0, lx, ly)?;
        for _ in 0..count.clamp(1, 3) {
            self.agent_button(slot, 0, button, true)?;
            self.agent_button(slot, 0, button, false)?;
        }
        Ok(())
    }
}

/// The process at the other end of the socket, if it is this user's.
fn same_user(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: u32::MAX, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let got = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    (got == 0 && cred.uid == unsafe { libc::getuid() }).then_some(cred.pid as u32)
}

fn frame(p: &wl_pointer::WlPointer) {
    if p.version() >= 5 {
        p.frame();
    }
}

fn num(s: &str) -> Result<f64, &'static str> {
    s.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or("bad-args")
}

fn unhex(hex: &str) -> Result<Vec<u8>, &'static str> {
    if hex.len() > 8192 {
        return Err("bad-args");
    }
    unhex_long(hex)
}

/// The same, of any length: two hex digits a byte, nothing else.
fn unhex_long(hex: &str) -> Result<Vec<u8>, &'static str> {
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8).ok_or("bad-args");
    let b = hex.as_bytes();
    if b.len() % 2 != 0 {
        return Err("bad-args");
    }
    b.chunks_exact(2).map(|p| Ok(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

/// Whether `pid` is `root` or one of its children: a browser's windows may
/// belong to a process it started.
/// The words of a program's name that say which it is: «google-chrome-canary»
/// and «chrome» share «chrome»; not the ones every program could have.
fn words(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !matches!(*w, "bin" | "app" | "desktop" | "browser" | "electron" | "client" | "org" | "com" | "the"))
        .map(str::to_owned)
        .collect()
}

fn in_family(mut pid: u32, root: u32) -> bool {
    if pid == 0 || root == 0 {
        return false;
    }
    for _ in 0..64 {
        if pid == root {
            return true;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
        // After the name, which is in parentheses and may hold anything.
        let Some(after) = stat.rfind(')').map(|k| &stat[k + 2..]) else { return false };
        let Some(parent) = after.split_whitespace().nth(1).and_then(|p| p.parse::<u32>().ok()) else { return false };
        if parent <= 1 || parent == pid {
            return false;
        }
        pid = parent;
    }
    false
}

/// The keys the protocol names, as evdev codes.
fn named_key(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    Some(match n.as_str() {
        "enter" | "return" => 28,
        "tab" => 15,
        "escape" | "esc" => 1,
        "backspace" => 14,
        "space" => 57,
        "up" => 103,
        "down" => 108,
        "left" => 105,
        "right" => 106,
        "delete" => 111,
        "home" => 102,
        "end" => 107,
        "pageup" => 104,
        "pagedown" => 109,
        _ => {
            let f: u32 = n.strip_prefix('f')?.parse().ok()?;
            match f {
                1..=10 => 58 + f,
                11 => 87,
                12 => 88,
                _ => return None,
            }
        }
    })
}

/// An ASCII character on a US keyboard: its evdev key, and whether with shift.
fn us_key(c: u8) -> Option<(u32, bool)> {
    const LOWER: &[(u8, u32)] = &[
        (b'1', 2), (b'2', 3), (b'3', 4), (b'4', 5), (b'5', 6), (b'6', 7), (b'7', 8), (b'8', 9), (b'9', 10), (b'0', 11),
        (b'-', 12), (b'=', 13), (b'q', 16), (b'w', 17), (b'e', 18), (b'r', 19), (b't', 20), (b'y', 21), (b'u', 22),
        (b'i', 23), (b'o', 24), (b'p', 25), (b'[', 26), (b']', 27), (b'a', 30), (b's', 31), (b'd', 32), (b'f', 33),
        (b'g', 34), (b'h', 35), (b'j', 36), (b'k', 37), (b'l', 38), (b';', 39), (b'\'', 40), (b'`', 41), (b'\\', 43),
        (b'z', 44), (b'x', 45), (b'c', 46), (b'v', 47), (b'b', 48), (b'n', 49), (b'm', 50), (b',', 51), (b'.', 52),
        (b'/', 53), (b' ', 57), (b'\n', 28), (b'\t', 15), (0x08, 14),
    ];
    const UPPER: &[(u8, u8)] = &[
        (b'!', b'1'), (b'@', b'2'), (b'#', b'3'), (b'$', b'4'), (b'%', b'5'), (b'^', b'6'), (b'&', b'7'), (b'*', b'8'),
        (b'(', b'9'), (b')', b'0'), (b'_', b'-'), (b'+', b'='), (b'{', b'['), (b'}', b']'), (b':', b';'), (b'"', b'\''),
        (b'~', b'`'), (b'|', b'\\'), (b'<', b','), (b'>', b'.'), (b'?', b'/'),
    ];
    if let Some((_, k)) = LOWER.iter().find(|(ch, _)| *ch == c) {
        return Some((*k, false));
    }
    if c.is_ascii_uppercase() {
        return us_key(c.to_ascii_lowercase()).map(|(k, _)| (k, true));
    }
    UPPER.iter().find(|(ch, _)| *ch == c).and_then(|(_, base)| us_key(*base)).map(|(k, _)| (k, true))
}

/// The modifier bit a key holds down, in a US keymap (Shift, Control, Mod1, Mod4).
fn modifier_mask(code: u32) -> u32 {
    match code {
        42 => 1,
        29 => 4,
        56 => 8,
        125 => 64,
        _ => 0,
    }
}

/// Each ASCII character in a keymap: its evdev key and whether with shift
/// (the first two levels only: what needs AltGr is left out).
fn char_table(keymap: &xkb::Keymap) -> Vec<Option<(u32, bool)>> {
    let mut table = vec![None; 128];
    for code in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
        for level in 0..2 {
            for sym in keymap.key_get_syms_by_level(code.into(), 0, level) {
                let c = xkb::keysym_to_utf32(*sym);
                if (1..128).contains(&c) && table[c as usize].is_none() && code >= 8 {
                    table[c as usize] = Some((code - 8, level == 1));
                }
            }
        }
    }
    table[b'\n' as usize] = Some((28, false));
    table[b'\t' as usize] = Some((15, false));
    table[0x08] = Some((14, false));
    table
}

fn us_keymap_string() -> String {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(&context, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
        .map(|k| k.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
        .unwrap_or_default()
}

/// Whether that process is Chromium or an Electron program: its folder has
/// Chromium's own files.
fn is_chromium(pid: u32) -> bool {
    let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) else { return false };
    let Some(dir) = exe.parent() else { return false };
    ["v8_context_snapshot.bin", "snapshot_blob.bin", "chrome_100_percent.pak"].iter().any(|f| dir.join(f).exists())
}

/// The keys that only write, in evdev codes: the rows of a keyboard's
/// letters, figures and signs.
const WRITING_KEYS: [u32; 47] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40,
    41, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53,
];

/// A keymap with a key for each of these characters, alone on it (on its
/// evdev key). Checked by compiling it.
fn text_keymap(chars: &[(char, u32)]) -> Option<String> {
    let mut codes = String::new();
    let mut symbols = String::new();
    for (c, code) in chars {
        let name = match c {
            '\n' => "Return".to_owned(),
            '\t' => "Tab".to_owned(),
            ' ' => "space".to_owned(),
            c => xkb::keysym_get_name(xkb::utf32_to_keysym(*c as u32)),
        };
        if name.is_empty() || name == "NoSymbol" {
            return None;
        }
        codes.push_str(&format!("        <K{code}> = {};\n", code + 8));
        symbols.push_str(&format!("        key <K{code}> {{ [ {name} ] }};\n"));
    }
    let text = format!(
        "xkb_keymap {{\n    xkb_keycodes \"agent\" {{\n        minimum = 8;\n        maximum = 255;\n{codes}    }};\n    xkb_types \"agent\" {{ include \"complete\" }};\n    xkb_compatibility \"agent\" {{ include \"complete\" }};\n    xkb_symbols \"agent\" {{\n{symbols}    }};\n}};\n"
    );
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_string(&context, text.clone(), xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS)?;
    Some(text)
}

/// A keymap in shared memory, as wl_keyboard.keymap hands it over.
fn keymap_fd(text: &str) -> Option<(std::os::fd::OwnedFd, u32)> {
    use std::os::fd::FromRawFd;
    let name = std::ffi::CString::new("cua-keymap").ok()?;
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let mut file = std::fs::File::from(fd);
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    file.write_all(&bytes).ok()?;
    Some((file.into(), bytes.len() as u32))
}

/// Taken by the agent's seats out of what the session's seat does.
pub fn is_agent_seat(seat: &Seat<State>) -> bool {
    seat.name().starts_with("cua-agent")
}

#[cfg(test)]
mod tests {
    use super::words;

    fn same(a: &str, b: &str) -> bool {
        let b = words(b);
        words(a).iter().any(|w| b.contains(w))
    }

    #[test]
    fn a_program_known_by_its_name() {
        // What the agent started it with, and what its window or process says.
        assert!(same("google-chrome-canary", "chrome"));
        assert!(same("discord", "Discord"));
        assert!(same("zen-browser", "zen-bin"));
        assert!(same("firefox", "org.mozilla.firefox"));
        // Not by the words any program could have.
        assert!(!same("zen-browser", "brave-browser"));
        assert!(!same("code", "electron"));
        assert!(!same("kitty", "foot"));
    }

    #[test]
    fn hex_is_two_digits_a_byte_and_nothing_else() {
        use super::{unhex, unhex_long};
        assert_eq!(unhex("4f6b"), Ok(b"Ok".to_vec()));
        assert_eq!(unhex("4F6B"), Ok(b"Ok".to_vec()));
        assert_eq!(unhex(""), Ok(Vec::new()));
        // Not a character boundary, a sign, an odd length, too long.
        assert_eq!(unhex("a\u{e9}b"), Err("bad-args"));
        assert_eq!(unhex("\u{e9}\u{e9}"), Err("bad-args"));
        assert_eq!(unhex("+a"), Err("bad-args"));
        assert_eq!(unhex("abc"), Err("bad-args"));
        assert_eq!(unhex(&"a".repeat(8194)), Err("bad-args"));
        assert_eq!(unhex_long(&"61".repeat(5000)).map(|b| b.len()), Ok(5000));
    }
}
