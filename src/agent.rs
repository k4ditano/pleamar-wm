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
//! | `C [PID]` | `can LOCKED` and, `|` between them, per window (every window without PID; with it, the one TARGET `root:PID` names) `PID SLOT TREE PRIVATE POPUPS`: 1 or 0, its program published a tree on the accessibility bus (asked of the broker, no walk), it is private; POPUPS its menus open, `X,Y,W,H` from the root's corner, `;` between them, `-` with none. LOCKED: the screen is locked. `err unknown-root-pid` |
//! | `l` | `windows` and, per window, `PID X Y W H VISIBLE FOCUSED APP TITLE` (app and title hex encoded), windows separated by `|`: what `pleamar-wm agent windows` shows |
//! | `r PID` | `rect X Y W H VISIBLE ZOOM`: the root surface's box on the desktop, as the scene shows it, whether it is seen (1), and how much the scene scales it: `m`'s coordinates are the window's own, W / ZOOM wide |
//! | `c TARGET [hidpi] [X Y W H]` | `picture PATH W H AGE [SCALE]`: a PNG of the window as it drew itself, with its menus, in `m`'s coordinates (or at its own density, a pixel over SCALE), cut to that box; AGE the ms since it last drew; whole, with a menu above or left of the root's corner: `... [SCALE] X Y`, the picture's corner in `m`'s coordinates; `err private` for a private window or a locked screen, `err no-box` for a box that misses the window, `err too-big` for a picture past 16384 pixels a side |
//! | `c TARGET [hidpi] @ID [PAD] [HEX]` | `picture PATH W H AGE [SCALE] X Y`: the same, cut to an element `A` gave (scrolled into view first if it is not in the window), or to the run of its text that says HEX, grown by PAD; X Y its corner in `m`'s coordinates; `err gone`, `err no-box` |
//! | `c TARGET [hidpi] marks [all]` | `picture PATH W H AGE [SCALE] V` and, `|` between them, the records of the elements marked on it: outlined, their ids on labels; what is acted on (`all`: and texts and headings), from a read `V` of its tree taken with the picture |
//! | `A TARGET tree [all]` · `A TARGET focus` · `A TARGET at X Y` · `A TARGET find HEX` | `nodes V` and, `|` between them, records `ID DEPTH ROLE X Y W H STATES NAME TEXT` (name and text hex) of what the window's program published on the accessibility bus: what is shown (`all`: and what is not), the focused element, the one at that point, or those that say that, each with those it is in; `err no-tree`, `err private`. Only `tree` and `tree since` reads are kept (the last 16 of a window) |
//! | `A TARGET within ID [all]` | `nodes V` and the records of what is shown of that element as `A` numbered it and what it holds (`all`: and what is not), it at depth 0; not kept; `err gone` once it is no longer there |
//! | `e TARGET ID box|press|focus|scroll`, `e TARGET ID action N` | that element of the window, as `A` numbered it: `box X Y W H shown|hidden` (`- - - -` without one), read now; its default action, the keyboard's focus in its window, scrolled into view, its action N: `ok`, or `err no-action`, `err no-focus`, `err no-scroll`; `err gone` once it is no longer there. Refused while stopped |
//! | `S TARGET ID HEX` | that element's whole text set to that (`EditableText.SetTextContents`): `ok`, `err not-editable`, `err gone`. Refused while stopped |
//! | `A TARGET tree since V [boxes]` | `changes V2` and, `|` between them, `+ RECORD` (given now, not in read V), `- ID` (in V, not now), `~ RECORD` (its role, name, text or states changed; `boxes`: its box too); `full | nodes V2 | ...`, the whole tree, when V is no longer kept or the frame is another |
//! | `i PID [w]` | the agent reads (`w`: types into) that window's terminal through its layer: `ok`, and its outline lit; `err private` for a private window or a locked session, `err stopped-by-user` for `w` while stopped; while stopped, `ok stopped MS`, MS since the Stop |
//! | `W TARGET MS CONDITION` | once it holds: `ok`, and for `has HEX`, `focus HEX` and `ID [not] STATE` the record that met it; `err timeout` after MS (60000 at most), `err stopped-by-user` at a Stop meanwhile, `err no-tree`. On the tree: `has HEX`, `gone HEX`, `ID gone`, `ID STATE`, `ID not STATE` (a record's states, `enabled`), `focus HEX`, `changed`; on the window: `settled MS` (neither its tree nor its picture changed for MS), `drawn MS` (it drew since the wait began, then nothing new for MS: its picture alone, no tree walked; `ok drawing` when it does not stop: at the deadline, or after 2 MS for a window that drew without a pause as the wait began, a spinner; `err timeout` when it drew nothing), `dialog` (`ok PID SLOT TITLE`, title hex) |
//! | `O TARGET MS` | what a screen reader would say of it, a line each as it happens: `focus RECORD` (the focus it starts on, then each it moves to), `text RECORD` (the focused element's text changed), `live RECORD` (a live region's news), `announce HEX [RECORD]`, `dialog PID SLOT TITLE`, `no-tree` (only dialogs are heard); last `end WHY HEARD READ`, WHY `time` after MS (600000 at most), `gone`, `stopped-by-user`, `private`, with the program's events heard and the elements read for them |
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
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
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
    /// What the last command left to finish after its line: its answer
    /// waits for that (see `agent_lines`).
    pending: Option<Pending>,
    /// Typing for the person, not for an agent (the phone's keyboard, `U`):
    /// the same hands, without the agent's marks on the monitor.
    quiet: bool,
    /// How fast the hands go (`T`), for every agent and shell: the CLI asks
    /// it when it opens the socket.
    pace: crate::config::Pace,
    pub path: String,
}

/// A command whose answer comes later than its line.
enum Pending {
    /// A paste the window has not read yet, and what was copied before, to
    /// give back then.
    Paste(std::sync::Arc<super::Kept>, Option<std::sync::Arc<super::Kept>>),
    /// Work done off the loop: its answer, or none by the deadline.
    Work(std::sync::mpsc::Receiver<String>, Instant),
    /// A wait (`W`): until its condition holds, its deadline, or the user's Stop.
    Wait(Box<crate::a11y_wait::Wait>),
    /// A listen (`O`): its lines as they come, until its time, the window
    /// closing or the user's Stop.
    Listen(Box<crate::a11y_wait::Listen>),
}

/// What `c` makes of a window's picture.
enum Look {
    /// It, or that box of it.
    Cut(Option<[i32; 4]>),
    /// An element's box grown by a margin, or the run of its text that says that.
    Element(u32, i32, Option<String>),
    /// It with its elements marked (`all`: what is read too).
    Marks(bool),
}

/// A connection to the socket: what it sent that is not answered yet,
/// whether an answer is being waited for (`Pending`), and what is still to
/// go down it.
struct Conn {
    stream: UnixStream,
    buffer: Vec<u8>,
    hello: bool,
    waiting: bool,
    /// Answers not written yet, in order: written as the socket takes them,
    /// never waiting for it on the loop (a picture's records are hundreds of
    /// KB, and an agent that does not read held the whole session).
    out: Vec<u8>,
    /// A source writes `out` as the socket takes more.
    writing: bool,
    /// Over: the agent went, or let too much go unread.
    closed: bool,
}

/// What may wait unread down a connection before it is closed.
const UNREAD: usize = 16 << 20;

impl Conn {
    fn new(stream: UnixStream) -> Conn {
        Conn { stream, buffer: Vec::new(), hello: false, waiting: false, out: Vec::new(), writing: false, closed: false }
    }

    /// Bytes after the ones before them, written what the socket takes now.
    /// false: the connection is over.
    fn queue(&mut self, bytes: &[u8]) -> bool {
        if self.closed {
            return false;
        }
        if self.out.len() + bytes.len() > UNREAD {
            self.close();
            return false;
        }
        self.out.extend_from_slice(bytes);
        self.flush()
    }

    /// What the socket takes of `out` without waiting.
    fn flush(&mut self) -> bool {
        while !self.out.is_empty() {
            match (&self.stream).write(&self.out) {
                Ok(0) => {
                    self.close();
                    return false;
                }
                Ok(n) => drop(self.out.drain(..n)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.close();
                    return false;
                }
            }
        }
        !self.closed
    }

    /// Ended both ways: its read source hears the end, the agent too.
    fn close(&mut self) {
        self.closed = true;
        self.out = Vec::new();
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Whether the agent has gone: closed here, or hung up (it closed, not
    /// only its writing half).
    fn gone(&mut self) -> bool {
        if !self.closed && hung_up(&self.stream) {
            self.close();
        }
        self.closed
    }
}

fn hung_up(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut p = libc::pollfd { fd: stream.as_raw_fd(), events: 0, revents: 0 };
    // SAFETY: one pollfd of an open socket, not waited on.
    unsafe { libc::poll(&mut p, 1, 0) > 0 && p.revents & (libc::POLLHUP | libc::POLLERR) != 0 }
}

type Shared = std::rc::Rc<std::cell::RefCell<Conn>>;

/// An answer, down a connection, after the ones before it. false: the
/// connection is over.
fn answer(handle: &LoopHandle<'static, State>, conn: &Shared, reply: &str) -> bool {
    let ok = conn.borrow_mut().queue(format!("{reply}\n").as_bytes());
    write_later(handle, conn);
    ok
}

/// Lines of an answer that goes on (`O`), down a connection, whole.
fn answer_soon(handle: &LoopHandle<'static, State>, conn: &Shared, lines: &[String]) -> bool {
    let ok = conn.borrow_mut().queue(format!("{}\n", lines.join("\n")).as_bytes());
    write_later(handle, conn);
    ok
}

/// What the socket did not take yet, written as it takes more.
fn write_later(handle: &LoopHandle<'static, State>, conn: &Shared) {
    let mut c = conn.borrow_mut();
    if c.out.is_empty() || c.writing || c.closed {
        return;
    }
    let Ok(writing) = c.stream.try_clone() else {
        c.close();
        return;
    };
    c.writing = true;
    drop(c);
    let source = Generic::new(writing, Interest::WRITE, Mode::Level);
    let conn2 = conn.clone();
    let inserted = handle.insert_source(source, move |_, _, _: &mut State| {
        let mut c = conn2.borrow_mut();
        if c.flush() && !c.out.is_empty() {
            return Ok(PostAction::Continue);
        }
        c.writing = false;
        Ok(PostAction::Remove)
    });
    if inserted.is_err() {
        let mut c = conn.borrow_mut();
        c.writing = false;
        c.close();
    }
}

#[cfg(test)]
mod conn_tests {
    use super::Conn;
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    fn pair() -> (Conn, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        (Conn::new(ours), theirs)
    }

    /// An agent that does not read: a long answer waits, whole and in
    /// order, without the loop waiting for it; read later, it all comes.
    #[test]
    fn a_long_answer_never_waits_for_the_agent() {
        let (mut c, mut theirs) = pair();
        let long: Vec<u8> = (0..4 << 20).map(|k| b'a' + (k % 26) as u8).collect();
        let start = std::time::Instant::now();
        assert!(c.queue(&long));
        assert!(c.queue(b"\nok\n"));
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        assert!(!c.out.is_empty(), "the socket took it all at once");
        let reader = std::thread::spawn(move || {
            let mut all = Vec::new();
            theirs.read_to_end(&mut all).unwrap();
            all
        });
        while !c.out.is_empty() {
            assert!(c.flush());
            std::thread::yield_now();
        }
        drop(c);
        let all = reader.join().unwrap();
        assert_eq!(&all[..long.len()], &long[..]);
        assert_eq!(&all[long.len()..], b"\nok\n");
    }

    /// Too much left unread closes it: the agent hears the end.
    #[test]
    fn too_much_unread_closes_it() {
        let (mut c, mut theirs) = pair();
        let lot = vec![b'x'; 1 << 20];
        let mut over = false;
        for _ in 0..(super::UNREAD >> 20) + 4 {
            if !c.queue(&lot) {
                over = true;
                break;
            }
        }
        assert!(over && c.closed && c.out.is_empty());
        let mut all = Vec::new();
        theirs.read_to_end(&mut all).unwrap();
        assert!(!c.queue(b"ok\n"));
    }

    /// A picture that starts at a menu above and left of the root's corner:
    /// the window where it is from there, and a box asked for in look
    /// coordinates cut from the same place.
    #[test]
    fn a_picture_from_a_menu_above_and_left() {
        let red = [0u8, 0, 255, 255].repeat(10 * 8);
        let p = pleamar::scene::WindowPicture { size: (10, 8), pixels: std::sync::Arc::new(red) };
        let (g, extent) = ([4, 6, 10, 8], [-5, -3, 19, 17]);
        let (whole, _) = super::look_picture(usize::MAX, &p, g, extent, false, None).unwrap();
        assert_eq!(whole.dimensions(), (19, 17));
        assert_eq!(whole.get_pixel(9, 9).0, [255, 0, 0, 255]);
        assert_eq!(whole.get_pixel(8, 8).0[3], 0);
        let (part, _) = super::look_picture(usize::MAX, &p, g, extent, false, Some([4, 6, 2, 2])).unwrap();
        assert!(part.pixels().all(|c| c.0 == [255, 0, 0, 255]));
        let (corner, _) = super::look_picture(usize::MAX, &p, g, extent, false, Some([-5, -3, 4, 4])).unwrap();
        assert!(corner.dimensions() == (4, 4) && corner.pixels().all(|c| c.0[3] == 0));
    }

    /// One that hung up is gone; one that only stopped writing is not.
    #[test]
    fn gone_once_hung_up() {
        let (mut c, theirs) = pair();
        theirs.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(!c.gone());
        drop(theirs);
        assert!(c.gone());
        assert!(!c.queue(b"ok\n"));
    }
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
    state.agent = Some(Agent { seats, on_monitor: [None; CURSORS], raw_entered: [None, None], raw_real: [None, None], kb_entered: None, kb_slot: None, kb_real: None, opening: Vec::new(), seen: [0; CURSORS], busy: 0, at: [(0.0, 0.0).into(); CURSORS], path, peer: 0, last: None, active: false, stopped: None, global: [None; CURSORS], working: None, remote: None, pending: None, quiet: false, pace: crate::config::get().pace });
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
        let conn = std::rc::Rc::new(std::cell::RefCell::new(Conn::new(stream)));
        let source = Generic::new(reading, Interest::READ, Mode::Level);
        let _ = self.handle.insert_source(source, move |_, stream, state: &mut State| {
            let stream: &mut UnixStream = unsafe { stream.get_mut() };
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    // The end of what it says: what it said before is still
                    // answered, unless it has gone altogether (not only done
                    // writing); then what it asked is ended (`agent_lines`).
                    Ok(0) => {
                        if !conn.borrow_mut().gone() {
                            state.agent_lines(&conn);
                        }
                        return Ok(PostAction::Remove);
                    }
                    Ok(n) => conn.borrow_mut().buffer.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        conn.borrow_mut().close();
                        return Ok(PostAction::Remove);
                    }
                }
                // A line longer than any command is not one.
                if conn.borrow().buffer.len() > 64 * 1024 {
                    conn.borrow_mut().close();
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
    /// first—. Work done off the loop (`Pending::Work`) is answered when it
    /// is done, or `err timeout` at its deadline. The lines after either
    /// wait meanwhile. false: the connection is over.
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
                    answer(&self.handle, conn, "err unsupported-version");
                    return false;
                }
            } else {
                self.agent_command(&line)
            };
            if let Some(mut pending) = self.agent.as_mut().and_then(|a| a.pending.take()) {
                conn.borrow_mut().waiting = true;
                let (conn, since) = (conn.clone(), Instant::now());
                let mut reply = Some(reply);
                let timer = Timer::from_duration(Duration::from_millis(5));
                let _ = self.handle.insert_source(timer, move |_, _, state: &mut State| {
                    // The agent went: what it asked ends now (a listen's or a
                    // wait's reading stops), and a paste gives back what you
                    // had copied all the same.
                    if conn.borrow_mut().gone() {
                        if let Pending::Paste(pasted, back) = &mut pending {
                            state.agent_give_back(&pasted.clone(), back.take());
                        }
                        conn.borrow_mut().waiting = false;
                        return TimeoutAction::Drop;
                    }
                    let done = match &mut pending {
                        Pending::Paste(pasted, back) => {
                            if !pasted.read.load(std::sync::atomic::Ordering::Relaxed) && since.elapsed() < Duration::from_millis(1500) {
                                return TimeoutAction::ToDuration(Duration::from_millis(5));
                            }
                            state.agent_give_back(&pasted.clone(), back.take());
                            reply.take().unwrap_or_default()
                        }
                        Pending::Work(work, deadline) => match work.try_recv() {
                            Ok(done) => done,
                            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < *deadline => {
                                return TimeoutAction::ToDuration(Duration::from_millis(5));
                            }
                            Err(_) => "err timeout".to_owned(),
                        },
                        Pending::Wait(w) => match state.agent_waited(w) {
                            Some(done) => done,
                            None => return TimeoutAction::ToDuration(Duration::from_millis(5)),
                        },
                        // Its lines as they come; once they cannot go (the
                        // agent went, or left too much unread) the connection
                        // is closed and the listen ends with it.
                        Pending::Listen(l) => {
                            let (said, end) = state.agent_listened(l);
                            if !said.is_empty() && !answer_soon(&state.handle, &conn, &said) {
                                conn.borrow_mut().waiting = false;
                                return TimeoutAction::Drop;
                            }
                            match end {
                                Some(end) => end,
                                None => return TimeoutAction::ToDuration(Duration::from_millis(5)),
                            }
                        }
                    };
                    conn.borrow_mut().waiting = false;
                    if answer(&state.handle, &conn, &done) {
                        state.agent_lines(&conn);
                    }
                    TimeoutAction::Drop
                });
                return true;
            }
            if !answer(&self.handle, conn, &reply) {
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
            ["c", target, rest @ ..] => self.agent_picture(target, rest),
            ["A", target, rest @ ..] => self.agent_tree(target, rest),
            ["W", target, ms, condition @ ..] => self.agent_wait(target, ms, condition),
            ["O", target, ms] => self.agent_listen(target, ms),
            ["i", pid, rest @ ..] => self.agent_text(pid, rest == ["w"]),
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
                crate::a11y_act::stop();
                eprintln!("agent · stopped by the user");
                Ok("ok".to_owned())
            }
            // Looking stays possible; doing anything does not.
            [verb, ..] if matches!(*verb, "f" | "m" | "b" | "a" | "t" | "k" | "h" | "d" | "v" | "L" | "M" | "B" | "e" | "S") && self.agent_stopped() => Err("stopped-by-user"),
            ["e", target, id, act @ ..] => match (id.parse(), crate::a11y_act::Act::parse(act)) {
                (Ok(id), Some(act)) => self.agent_element(target, id, act),
                _ => Err("bad-args"),
            },
            // A whole text, longer than `t`'s: `-` for none.
            ["S", target, id, hex] if hex.len() % 2 == 0 || *hex == "-" => {
                let bytes = if *hex == "-" { Ok(Vec::new()) } else { unhex_long(hex) };
                match (id.parse(), bytes.ok().and_then(|b| String::from_utf8(b).ok())) {
                    (Ok(id), Some(text)) => self.agent_element(target, id, crate::a11y_act::Act::Set(text)),
                    _ => Err("bad-args"),
                }
            }
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
        let (r, zoom) = (self.agent_rect_of(slot), self.agent_place(slot).map_or(1.0, |p| p.2));
        Ok(format!("rect {} {} {} {} {} {zoom:.4}", r.0, r.1, r.2, r.3, u8::from(r.4)))
    }

    /// The agent at a window's text through its terminal's layer, which the
    /// compositor does not see: refused as a picture of it would be (a
    /// private window, a locked session), typing also while stopped; and the
    /// window's outline lit, as for anything it does with it.
    fn agent_text(&mut self, pid: &str, types: bool) -> Result<String, &'static str> {
        let slot = self.agent_pick(pid).map_err(|_| "target-not-found")?;
        let process = self.pid_of(slot);
        if layers::locked() || self.slots_open().any(|s| (s == slot || self.pid_of(s) == process) && layers::private(s)) {
            return Err("private");
        }
        let stopped = self.agent_stopped();
        if types && stopped {
            return Err("stopped-by-user");
        }
        self.agent_busy(slot);
        // Reading while stopped: how long ago, so that a wait begun before it ends.
        Ok(match self.agent.as_ref().and_then(|a| a.stopped).filter(|_| stopped) {
            Some(at) => format!("ok stopped {}", at.elapsed().as_millis()),
            None => "ok".to_owned(),
        })
    }

    fn agent_rect_of(&self, slot: usize) -> (i64, i64, i64, i64, bool) {
        let Some((_, root, zoom, _)) = self.agent_place(slot) else { return (0, 0, 0, 0, false) };
        let e = self.agent_extent(slot);
        let (width, height) = (((e[0] + e[2]) as f64 * zoom).round(), ((e[1] + e[3]) as f64 * zoom).round());
        (root[0].round() as i64, root[1].round() as i64, width as i64, height as i64, true)
    }

    /// How far a window reaches from its root surface's corner, in its units:
    /// to the window's far edges (the scene draws the window only, not a
    /// program's own shadow, and starting at the root's corner keeps a pixel
    /// of a picture a point `m` takes), and its menus that hang out of it (a
    /// right-click menu near its edge), whole: X Y W H. X and Y are 0 but
    /// for a menu above or left of the root's corner (one a window at the
    /// bottom of the monitor opens upwards), where they are that menu's.
    fn agent_extent(&self, slot: usize) -> [i32; 4] {
        let Some(Some(w)) = self.slots.get(slot) else { return [0; 4] };
        let g = w.geometry;
        let (mut left, mut top, mut right, mut bottom) = (0, 0, g[0] + g[2], g[1] + g[3]);
        for (popup, offset) in smithay::desktop::PopupManager::popups_for_surface(&w.surface) {
            let size = popup.geometry().size;
            let (x, y) = (g[0] + offset.x, g[1] + offset.y);
            (left, top) = (left.min(x), top.min(y));
            (right, bottom) = (right.max(x + size.w), bottom.max(y + size.h));
        }
        [left, top, right - left, bottom - top]
    }

    /// `A TARGET tree [all] | tree since V [boxes] | within ID [all] | focus | at X Y | find HEX`:
    /// what the window's program published of it on the accessibility bus
    /// (`a11y_tree.rs`), read off the loop: `nodes V` and its records, or
    /// `changes V` and what changed.
    fn agent_tree(&mut self, target: &str, rest: &[&str]) -> Result<String, &'static str> {
        use crate::a11y_tree::Ask;
        let slot = self.agent_target(target)?;
        if layers::private(slot) || layers::locked() {
            return Err("private");
        }
        let ask = match rest {
            ["tree"] => Ask::Tree(false),
            ["tree", "all"] => Ask::Tree(true),
            ["tree", "since", v, boxes @ ..] if boxes.is_empty() || boxes == ["boxes"] => Ask::Since(v.parse().map_err(|_| "bad-args")?, !boxes.is_empty()),
            ["within", id, all @ ..] if all.is_empty() || all == ["all"] => Ask::Within(id.parse().map_err(|_| "bad-args")?, !all.is_empty()),
            ["focus"] => Ask::Focus,
            ["at", x, y] => match (x.parse(), y.parse()) {
                (Ok(x), Ok(y)) => Ask::At(x, y),
                _ => return Err("bad-args"),
            },
            ["find", hex] => Ask::Find(String::from_utf8_lossy(&unhex(hex).map_err(|_| "bad-args")?).into_owned()),
            _ => return Err("bad-args"),
        };
        let pid = self.pid_of(slot);
        let title = self.slots.get(slot).and_then(Option::as_ref).map(|w| w.title.clone()).unwrap_or_default();
        let guarded = self.agent_guarded(pid);
        self.agent_busy(slot);
        let (answer, work) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(crate::a11y_tree::read(slot, pid, &title, guarded, |p| in_family(p, pid) || in_family(pid, p), ask));
        });
        if let Some(a) = self.agent.as_mut() {
            a.pending = Some(Pending::Work(work, Instant::now() + Duration::from_secs(3)));
        }
        Ok(String::new())
    }

    /// Whether a window of that program, or of its family, is private: its
    /// tree's frame is then taken only by the window's title.
    fn agent_guarded(&self, pid: u32) -> bool {
        self.slots_open().any(|s| {
            let p = self.pid_of(s);
            (p == pid || in_family(p, pid) || in_family(pid, p)) && layers::private(s)
        })
    }

    /// `e TARGET ID ...`, `S TARGET ID HEX`: that element of the window, as
    /// `A` numbered it, acted on off the loop (`a11y_act.rs`); a Stop
    /// meanwhile refuses it.
    fn agent_element(&mut self, target: &str, id: u32, act: crate::a11y_act::Act) -> Result<String, &'static str> {
        let slot = self.agent_target(target)?;
        if layers::private(slot) || layers::locked() {
            return Err("private");
        }
        let pid = self.pid_of(slot);
        let stops = crate::a11y_act::stops();
        self.agent_busy(slot);
        let (answer, work) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(crate::a11y_act::act(pid, slot, id, act, stops).unwrap_or_else(|e| format!("err {e}")));
        });
        if let Some(a) = self.agent.as_mut() {
            a.pending = Some(Pending::Work(work, Instant::now() + Duration::from_secs(3)));
        }
        Ok(String::new())
    }

    /// `W TARGET MS CONDITION`: answered once the condition holds
    /// (`a11y_wait.rs`), `err timeout` after MS. Allowed while stopped: it
    /// does nothing, and a Stop meanwhile ends it.
    fn agent_wait(&mut self, target: &str, ms: &str, condition: &[&str]) -> Result<String, &'static str> {
        let slot = self.agent_target(target)?;
        if layers::private(slot) || layers::locked() {
            return Err("private");
        }
        let ms: u64 = ms.parse().map_err(|_| "bad-args")?;
        let condition = crate::a11y_wait::Condition::parse(condition, unhex)?;
        let pid = self.pid_of(slot);
        let title = self.slots.get(slot).and_then(Option::as_ref).map(|w| w.title.clone()).unwrap_or_default();
        self.agent_busy(slot);
        let mut wait = crate::a11y_wait::Wait::start(condition, slot, pid, title, self.agent_guarded(pid), move |p| in_family(p, pid) || in_family(pid, p), ms, self.slots_open().collect());
        if let Some(w) = self.slots.get(slot).and_then(Option::as_ref) {
            wait.animating = w.drew.elapsed() <= crate::a11y_wait::PAUSE && w.steady.elapsed() >= crate::a11y_wait::STEADY;
        }
        if let Some(a) = self.agent.as_mut() {
            a.pending = Some(Pending::Wait(Box::new(wait)));
        }
        Ok(String::new())
    }

    /// A wait's turn on the loop: its answer, once it has one. What the
    /// window manager knows (a Stop, the window gone or drawing, a dialog)
    /// is decided here; the tree's answer comes from its thread.
    fn agent_waited(&self, w: &crate::a11y_wait::Wait) -> Option<String> {
        use crate::a11y_wait::Condition;
        if self.agent.as_ref().and_then(|a| a.stopped).is_some_and(|at| at > w.since) {
            return Some("err stopped-by-user".into());
        }
        if layers::locked() || layers::private(w.slot) {
            return Some("err private".into());
        }
        let Some(window) = self.slots.get(w.slot).and_then(Option::as_ref).filter(|_| self.pid_of(w.slot) == w.pid) else { return Some("err gone".into()) };
        w.retitle(&window.title);
        if let Some(done) = w.answered() {
            return Some(done);
        }
        match w.condition {
            Condition::Dialog => {
                if let Some((_, d)) = self.agent_dialog(w.slot, w.pid, &w.open) {
                    return Some(format!("ok {d}"));
                }
            }
            Condition::Settled(ms) if w.quiet_since(window.drew).elapsed() >= Duration::from_millis(ms) => return Some("ok".into()),
            // Drawing all the time: what it drew for the action is not told apart.
            Condition::Drawn(ms) if w.animating && w.since.elapsed() >= Duration::from_millis(2 * ms) => return Some("ok drawing".into()),
            Condition::Drawn(ms) if window.drew > w.since && window.drew.elapsed() >= Duration::from_millis(ms) => return Some("ok".into()),
            Condition::Drawn(_) if window.drew > w.since && Instant::now() >= w.deadline => return Some("ok drawing".into()),
            _ => (),
        }
        (Instant::now() >= w.deadline).then(|| "err timeout".into())
    }

    /// A window not among those `open`, for this one or its program's (a
    /// portal's chooser is the browser's): its slot, and `PID SLOT TITLE`
    /// (title hex).
    fn agent_dialog(&self, slot: usize, pid: u32, open: &[usize]) -> Option<(usize, String)> {
        let ours = |s: usize| {
            let mut at = s;
            let under = (0..8).any(|_| match self.parent_slot(at) {
                Some(p) => {
                    at = p;
                    p == slot || self.pid_of(p) == pid
                }
                None => false,
            });
            under || (self.pid_of(s) == pid && self.slots[s].as_ref().is_some_and(|d| d.dialog))
        };
        let d = self.slots_open().find(|s| !open.contains(s) && ours(*s))?;
        let title = self.slots[d].as_ref().map_or(String::new(), |d| d.title.bytes().map(|b| format!("{b:02x}")).collect());
        Some((d, format!("{} {d} {}", self.pid_of(d), if title.is_empty() { "-".into() } else { title })))
    }

    /// `O TARGET MS`: what a screen reader would say of the window, a line
    /// each as it happens (`a11y_wait.rs`), then `end WHY HEARD READ`. Allowed
    /// while stopped, as `W`.
    fn agent_listen(&mut self, target: &str, ms: &str) -> Result<String, &'static str> {
        let slot = self.agent_target(target)?;
        if layers::private(slot) || layers::locked() {
            return Err("private");
        }
        let ms: u64 = ms.parse().map_err(|_| "bad-args")?;
        let pid = self.pid_of(slot);
        let title = self.slots.get(slot).and_then(Option::as_ref).map(|w| w.title.clone()).unwrap_or_default();
        self.agent_busy(slot);
        let listen = crate::a11y_wait::Listen::start(slot, pid, title, self.agent_guarded(pid), move |p| in_family(p, pid) || in_family(pid, p), ms, self.slots_open().collect());
        if let Some(a) = self.agent.as_mut() {
            a.pending = Some(Pending::Listen(Box::new(listen)));
        }
        Ok(String::new())
    }

    /// A listen's turn on the loop: the lines heard since the last, the
    /// dialogs opened, and its last line once it is over.
    fn agent_listened(&self, l: &mut crate::a11y_wait::Listen) -> (Vec<String>, Option<String>) {
        let mut said = l.said();
        while let Some((d, which)) = self.agent_dialog(l.slot, l.pid, &l.open) {
            l.open.push(d);
            said.push(format!("dialog {which}"));
        }
        let end = if self.agent.as_ref().and_then(|a| a.stopped).is_some_and(|at| at > l.since) {
            Some("stopped-by-user")
        } else if layers::locked() {
            Some("private")
        } else if self.slots.get(l.slot).and_then(Option::as_ref).is_none() || self.pid_of(l.slot) != l.pid {
            Some("gone")
        } else {
            (Instant::now() >= l.deadline).then_some("time")
        };
        (said, end.map(|why| l.end(why)))
    }

    /// `c TARGET [hidpi] [X Y W H]`: a picture of the window from what it
    /// drew, not from the screen: covered, on another workspace, with its
    /// menus and nothing else over it. In look coordinates (its root
    /// surface's units: a pixel is the point `m` takes), or at the window's
    /// own density with `hidpi` (a pixel over SCALE is that point). Put
    /// together off the loop; answered `picture PATH W H AGE [SCALE]`, AGE
    /// the milliseconds since the window last drew. `@ID [PAD] [HEX]`: cut to
    /// that element (or to the run of its text that says HEX), grown by PAD,
    /// and its corner after: `... X Y`. `marks [all]`: its elements marked
    /// on it, and their records after: `... V | records`.
    fn agent_picture(&mut self, target: &str, rest: &[&str]) -> Result<String, &'static str> {
        let slot = self.agent_target(target)?;
        if layers::private(slot) || layers::locked() {
            return Err("private");
        }
        let hidpi = rest.first() == Some(&"hidpi");
        let look = match &rest[usize::from(hidpi)..] {
            [] => Look::Cut(None),
            [x, y, w, h] => match (x.parse::<i32>(), y.parse::<i32>(), w.parse::<i32>(), h.parse::<i32>()) {
                (Ok(x), Ok(y), Ok(w), Ok(h)) if w > 0 && h > 0 => Look::Cut(Some([x, y, w, h])),
                _ => return Err("bad-args"),
            },
            ["marks"] => Look::Marks(false),
            ["marks", "all"] => Look::Marks(true),
            [at, more @ ..] if at.starts_with('@') && more.len() <= 2 => {
                let id = at[1..].parse().map_err(|_| "bad-args")?;
                let pad = more.first().map_or(Ok(0), |p| p.parse::<i32>()).map_err(|_| "bad-args")?.clamp(0, 400);
                let run = more.get(1).map(|h| unhex(h).map(|b| String::from_utf8_lossy(&b).into_owned())).transpose()?;
                Look::Element(id, pad, run)
            }
            _ => return Err("bad-args"),
        };
        let Some(Some(w)) = self.slots.get(slot) else { return Err("unknown-pid") };
        let (geometry, age, extent, title) = (w.geometry, w.drew.elapsed().as_millis(), self.agent_extent(slot), w.title.clone());
        let (pid, peer) = (self.pid_of(slot), self.agent.as_ref().map_or(0, |a| a.peer));
        // Stopped, it is looked at where it is: not scrolled.
        let (guarded, stops) = (self.agent_guarded(pid), (!self.agent_stopped()).then(crate::a11y_act::stops));
        self.agent_busy(slot);
        static WHO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 32);
        let who = WHO.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (answer, work) = std::sync::mpsc::channel();
        let to_render = self.to_render.clone();
        // A read of the tree as well may take its own 2.5 s; menus read back
        // from their monitors, a second more.
        let deadline = Duration::from_secs(if matches!(look, Look::Marks(_)) { 4 } else { 3 + u64::from(layers::has_menus(slot)) });
        std::thread::spawn(move || {
            let (tx, pictures) = std::sync::mpsc::channel::<pleamar::scene::WindowPicture>();
            let _ = to_render.send(ToRender::WatchWindow(slot, who, Some(tx)));
            let shot = Shot { slot, who, to_render, pictures, geometry, extent, hidpi, peer, age };
            // A cut is what of it is in the window, and says its own size.
            let within = |c: [i32; 4]| crate::marks::meet(c, extent);
            let reply = match look {
                Look::Cut(Some(cut)) if within(cut).is_none() => "err no-box".to_owned(),
                Look::Cut(cut) => match shot.first() {
                    Some(p) => {
                        drop(shot);
                        let cut = cut.and_then(within);
                        // Whole, from a menu above or left of the root's corner: where it starts.
                        let corner = if cut.is_none() && extent[..2] != [0, 0] { format!(" {} {}", extent[0], extent[1]) } else { String::new() };
                        match look_picture(slot, &p, geometry, extent, hidpi, cut) {
                            Ok((picture, scale)) => kept(&picture, peer, cut.map_or((extent[2], extent[3]), |c| (c[2], c[3])), age, hidpi.then_some(scale), &corner),
                            Err(e) => format!("err {e}"),
                        }
                    }
                    None => "err no-picture".to_owned(),
                },
                Look::Element(id, pad, run) => look_element(shot, pid, id, pad, run.as_deref(), stops),
                Look::Marks(all) => look_marks(shot, pid, title, guarded, all),
            };
            let _ = answer.send(reply);
        });
        if let Some(a) = self.agent.as_mut() {
            a.pending = Some(Pending::Work(work, Instant::now() + deadline));
        }
        Ok(String::new())
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

    /// `C [PID]`: what can be asked of a window, or of each: whether its
    /// program (or, as `A` takes it, one of its family) published a tree,
    /// from what the broker knows without walking it; whether it is private;
    /// and where its menus are, for a scene's boxes in them.
    fn agent_can(&self, spec: Option<&str>) -> Result<String, &'static str> {
        let slots: Vec<usize> = match spec {
            Some(p) => vec![self.agent_pick(p)?],
            None => self.slots_open().collect(),
        };
        let mut programs = None;
        let mut out = format!("can {}", u8::from(layers::locked()));
        for slot in slots {
            let Some(w) = self.slots[slot].as_ref() else { continue };
            let pid = self.pid_of(slot);
            let tree = crate::a11y::program_of(pid).is_some()
                || programs.get_or_insert_with(crate::a11y::programs).iter().any(|p| p.root.is_some() && !p.reader && (in_family(p.pid, pid) || in_family(pid, p.pid)));
            let g = w.geometry;
            let popups: Vec<String> = smithay::desktop::PopupManager::popups_for_surface(&w.surface)
                .map(|(popup, at)| {
                    let pg = popup.geometry();
                    format!("{},{},{},{}", g[0] + at.x - pg.loc.x, g[1] + at.y - pg.loc.y, pg.size.w, pg.size.h)
                })
                .collect();
            let popups = if popups.is_empty() { "-".to_owned() } else { popups.join(";") };
            out.push_str(&format!(" | {pid} {slot} {} {} {popups}", u8::from(tree), u8::from(layers::private(slot))));
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
                    a.pending = Some(Pending::Paste(pasted, back));
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

/// A window's picture from the render (cut to its geometry, at its own
/// density, BGRA premultiplied) laid out from its root surface's corner,
/// with its menus over it, over the `extent` box (from that corner), at a
/// unit a pixel (or at its density, `hidpi`), cut to `cut` (in units from
/// the root's corner): RGBA, and its pixels a unit. `too-big` past `SIDE`
/// pixels a side or `PIXELS` in all.
fn look_picture(slot: usize, p: &pleamar::scene::WindowPicture, g: [i32; 4], extent: [i32; 4], hidpi: bool, cut: Option<[i32; 4]>) -> Result<(image::RgbaImage, f64), &'static str> {
    const SIDE: u32 = 16384;
    const PIXELS: u64 = 1 << 25;
    let s = if g[2] > 0 { p.size.0 as f64 / g[2] as f64 } else { 1.0 };
    let d = if hidpi { s } else { 1.0 };
    let px = |v: i32| (v as f64 * d).round() as i32;
    let small = |(w, h): (u32, u32)| w <= SIDE && h <= SIDE && u64::from(w) * u64::from(h) <= PIXELS;
    let size = (px(extent[2]).max(1) as u32, px(extent[3]).max(1) as u32);
    // The window's corner on the canvas, in units.
    let (gx, gy) = (g[0] - extent[0], g[1] - extent[1]);
    // Its pixels to the canvas's: resampled premultiplied, as they are.
    let fit = (px(g[2]).max(1) as u32, px(g[3]).max(1) as u32);
    if !small(size) || !small(fit) {
        return Err("too-big");
    }
    let mut canvas = vec![0u8; size.0 as usize * size.1 as usize * 4];
    let own = image::RgbaImage::from_raw(p.size.0, p.size.1, p.pixels.to_vec());
    let own = own.map(|o| if o.dimensions() == fit { o } else { image::imageops::resize(&o, fit.0, fit.1, image::imageops::FilterType::Triangle) });
    if let Some(own) = own {
        let (ox, oy) = (px(gx) as i64, px(gy) as i64);
        for (x, y, c) in own.enumerate_pixels() {
            let (tx, ty) = (ox + x as i64, oy + y as i64);
            if tx >= 0 && ty >= 0 && tx < size.0 as i64 && ty < size.1 as i64 {
                let k = (ty as usize * size.0 as usize + tx as usize) * 4;
                canvas[k..k + 4].copy_from_slice(&c.0);
            }
        }
    }
    if let Some((_, menus)) = layers::window_menus(slot) {
        for (menu, its) in &menus {
            layers::lay_menu(&mut canvas, size, (px(gx), px(gy)), d, menu, its);
        }
    }
    // BGRA premultiplied to RGBA as it is seen.
    for c in canvas.chunks_exact_mut(4) {
        let a = c[3] as u32;
        let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a / 2) / a).min(255) as u8 };
        let (b, g, r) = (un(c[0]), un(c[1]), un(c[2]));
        c[0] = r;
        c[1] = g;
        c[2] = b;
    }
    let picture = image::RgbaImage::from_raw(size.0, size.1, canvas).unwrap_or_default();
    let picture = match cut {
        Some([x, y, w, h]) => {
            let (x0, y0) = (px(x - extent[0]).clamp(0, size.0 as i32) as u32, px(y - extent[1]).clamp(0, size.1 as i32) as u32);
            let (w, h) = ((px(w).max(0) as u32).min(size.0 - x0).max(1), (px(h).max(0) as u32).min(size.1 - y0).max(1));
            image::imageops::crop_imm(&picture, x0, y0, w, h).to_image()
        }
        None => picture,
    };
    Ok((picture, d))
}

/// Where the agent's pictures are kept (yours only): the last few, each
/// connection's numbered, the older ones let go.
fn keep_picture(picture: &image::RgbaImage, peer: u32) -> Result<String, String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = format!("{}/pleamar-agent", std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let path = format!("{dir}/look-{peer}-{}.png", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
    picture.write_to(&mut file, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let mut kept: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("look-")).filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect())
        .unwrap_or_default();
    kept.sort();
    for (_, old) in kept.iter().rev().skip(16) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// A window being looked at for `c`: its pictures, each time it draws,
/// while this lives.
struct Shot {
    slot: usize,
    who: u64,
    to_render: std::sync::mpsc::Sender<ToRender>,
    pictures: std::sync::mpsc::Receiver<pleamar::scene::WindowPicture>,
    geometry: [i32; 4],
    extent: [i32; 4],
    hidpi: bool,
    peer: u32,
    age: u128,
}

impl Shot {
    /// What it shows now.
    fn first(&self) -> Option<pleamar::scene::WindowPicture> {
        self.pictures.recv_timeout(Duration::from_millis(2500)).ok()
    }

    /// What it shows once it has drawn again and stopped (a scroll that
    /// glides), within that long: none if it did not draw.
    fn redrawn(&self, within: Duration) -> Option<pleamar::scene::WindowPicture> {
        let until = Instant::now() + within;
        let mut last = self.pictures.recv_timeout(within).ok()?;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match self.pictures.recv_timeout(left.min(Duration::from_millis(120))) {
                Ok(p) => last = p,
                Err(_) => break,
            }
        }
        Some(last)
    }
}

impl Drop for Shot {
    fn drop(&mut self) {
        let _ = self.to_render.send(ToRender::WatchWindow(self.slot, self.who, None));
    }
}

/// A picture kept, and `c`'s answer: `picture PATH W H AGE [SCALE]`, and
/// what follows.
fn kept(picture: &image::RgbaImage, peer: u32, (w, h): (i32, i32), age: u128, scale: Option<f64>, after: &str) -> String {
    match keep_picture(picture, peer) {
        Ok(path) => format!("picture {path} {w} {h} {age}{}{after}", scale.map_or(String::new(), |s| format!(" {s:.3}"))),
        Err(e) => format!("err {e}"),
    }
}

/// `c TARGET @ID`: the element's box read, the element scrolled into view
/// when it is not in the window (and its box read again once the window has
/// drawn), and the picture cut to it there: nothing moves between the box
/// and the picture. Its corner after, in look coordinates. `stops`: the
/// user's Stops when it was asked, none while stopped: then it is not
/// scrolled, and one not shown is refused.
fn look_element(shot: Shot, pid: u32, id: u32, pad: i32, run: Option<&str>, stops: Option<u64>) -> String {
    let whole = shot.extent;
    let Some(mut picture) = shot.first() else { return "err no-picture".to_owned() };
    let mut age = shot.age;
    let read = || crate::marks::element_box(pid, shot.slot, id, run);
    let (mut b, showing) = match read() {
        Ok(b) => b,
        Err(e) => return format!("err {e}"),
    };
    let away = !showing || !b.is_some_and(|b| crate::marks::inside(b, whole));
    if away && stops.is_none() && (!showing || !b.is_some_and(|b| crate::marks::meet(b, whole).is_some())) {
        return "err stopped-by-user".to_owned();
    }
    if away && stops.is_some_and(|s| crate::marks::scroll_to(pid, shot.slot, id, s)) {
        if let Some(p) = shot.redrawn(Duration::from_millis(1200)) {
            (picture, age) = (p, 0);
        }
        b = match read() {
            Ok((b, _)) => b,
            Err(e) => return format!("err {e}"),
        };
    }
    let Some(cut) = b.and_then(|b| crate::marks::meet(crate::marks::grow(b, pad), whole)) else { return "err no-box".to_owned() };
    match look_picture(shot.slot, &picture, shot.geometry, shot.extent, shot.hidpi, Some(cut)) {
        Ok((image, scale)) => kept(&image, shot.peer, (cut[2], cut[3]), age, shot.hidpi.then_some(scale), &format!(" {} {}", cut[0], cut[1])),
        Err(e) => format!("err {e}"),
    }
}

/// `c TARGET marks [all]`: the window's tree read while its picture is
/// taken, and the elements marked on the picture at its density: the
/// answer is the picture, the read's version and the marked elements'
/// records. What is not in the window (a row scrolled out) is not marked.
fn look_marks(shot: Shot, pid: u32, title: String, guarded: bool, all: bool) -> String {
    let slot = shot.slot;
    let tree = std::thread::spawn(move || crate::a11y_tree::read(slot, pid, &title, guarded, |p| in_family(p, pid) || in_family(pid, p), crate::a11y_tree::Ask::Tree(false)));
    let picture = shot.first();
    let read = tree.join().unwrap_or_else(|_| "err no-tree".to_owned());
    let Some(picture) = picture else { return "err no-picture".to_owned() };
    let mut parts = read.split(" | ");
    let Some(version) = parts.next().and_then(|h| h.strip_prefix("nodes ")) else { return read };
    let records: Vec<&str> = parts.collect();
    // From the root's corner, as the marks are drawn.
    let e = shot.extent;
    let from_root = [0, 0, e[0] + e[2], e[1] + e[3]];
    let mut marked = crate::marks::pick(pid, slot, &records, all);
    marked.retain(|(m, _)| crate::marks::meet(m.rect, from_root).is_some());
    let (mut image, scale) = match look_picture(slot, &picture, shot.geometry, from_root, shot.hidpi, None) {
        Ok(got) => got,
        Err(e) => return format!("err {e}"),
    };
    crate::marks::draw(&mut image, &marked.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>(), scale);
    let more = records.iter().filter(|r| r.starts_with("... more"));
    let after: String = marked.iter().map(|(_, r)| *r).chain(more.copied()).fold(format!(" {version}"), |s, r| s + " | " + r);
    kept(&image, shot.peer, (from_root[2], from_root[3]), shot.age, shot.hidpi.then_some(scale), &after)
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

    #[test]
    fn a_picture_too_big_is_refused() {
        let p = pleamar::scene::WindowPicture { size: (4000, 10), pixels: std::sync::Arc::new(vec![0; 4000 * 10 * 4]) };
        // A buffer 4000 times the window's units, at its density.
        assert_eq!(super::look_picture(usize::MAX, &p, [0, 0, 1, 10], [0, 0, 100, 10], true, None).err(), Some("too-big"));
        assert_eq!(super::look_picture(usize::MAX, &p, [0, 0, 1, 10], [0, 0, i32::MAX, i32::MAX], false, None).err(), Some("too-big"));
        let (small, d) = super::look_picture(usize::MAX, &p, [0, 0, 1, 10], [0, 0, 100, 10], false, Some([90, 0, 10, 10])).unwrap();
        assert_eq!((small.dimensions(), d), ((10, 10), 1.0));
    }
}
