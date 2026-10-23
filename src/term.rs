//! The layer between a terminal and its shell: what lets a window manager
//! read and write in any terminal, whichever it is.
//!
//! In a pleamar-wm session `SHELL` names a link to this program called as the
//! user's shell (`~/.local/share/pleamar-wm/shell/fish`), and the real one is
//! in `PLEAMAR_REAL_SHELL`. A terminal that starts `$SHELL` —kitty, Alacritty,
//! foot, Konsole, WezTerm…— starts this instead, which starts the real shell
//! on a terminal of its own (a pty) and passes everything through, both ways,
//! untouched: the terminal draws what the shell says, as ever. On the way it
//! keeps what is on the screen, as a terminal would (`vt100`), and listens on
//! a socket of its own (`$XDG_RUNTIME_DIR/pleamar/term/PID.sock`): what is
//! there, its title, what runs in it, and text or keys to type. What runs in
//! it knows it from `PLEAMAR_TERMINAL` (this process's id).
//!
//! It never stands in the way: asked to run a command (`-c`), not on a
//! terminal, inside another one of these, or failing to set itself up, it is
//! the real shell at once.

use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::sync::{Arc, Mutex};

/// History kept beyond the screen, in lines.
pub const HISTORY: usize = 2000;

/// The folder of the links, next to Build's: `~/.local/share/pleamar-wm/shell`.
fn link_dir() -> String {
    format!("{}/shell", crate::build::dir().trim_end_matches("/build"))
}

/// Where the layers listen.
/// Each session's apart, in its own place (`PLEAMAR_SOCKETS`, which the
/// session gives what it starts): a nested or screenless session never sees
/// the terminals of the one it runs in, nor they its own.
pub fn socket_dir() -> String {
    match std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()) {
        Some(d) => format!("{d}/term"),
        None => {
            let run = std::env::var("XDG_RUNTIME_DIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/tmp".into());
            format!("{run}/pleamar/term")
        }
    }
}

/// The user's shell, as the system says it: `SHELL`, or the passwd entry.
fn users_shell() -> Option<String> {
    if let Some(s) = std::env::var("SHELL").ok().filter(|s| !s.is_empty() && !s.contains("/pleamar-wm/shell/")) {
        return Some(s);
    }
    // SAFETY: getpwuid's answer is read at once, before anything else asks.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_shell.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned()).filter(|s| !s.is_empty())
    }
}

static SESSION: std::sync::OnceLock<Option<(String, String)>> = std::sync::OnceLock::new();

/// The session's: made once (`shell_link`), unless `terminals off`.
pub fn session_shell() -> Option<&'static (String, String)> {
    SESSION.get_or_init(|| if crate::config::get().terminals_off { None } else { shell_link() }).as_ref()
}

/// For the session: the link that stands for the user's shell, and the real
/// one, to give every program it starts (`SHELL`, `PLEAMAR_REAL_SHELL`).
/// Named as the shell is, so what looks at `$SHELL`'s name still sees fish.
fn shell_link() -> Option<(String, String)> {
    let real = users_shell()?;
    let name = std::path::Path::new(&real).file_name()?.to_str()?.to_owned();
    let dir = link_dir();
    let link = format!("{dir}/{name}");
    let me = std::env::current_exe().ok()?;
    if std::fs::read_link(&link).ok().as_deref() != Some(me.as_path()) {
        std::fs::create_dir_all(&dir).ok()?;
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&me, &link).ok()?;
    }
    Some((link, real))
}

/// The link that stands for that real shell in the session's terminals, if
/// the session made one: what a terminal started from inside a layer (whose
/// `SHELL` is the real one again) needs to get a layer of its own.
pub fn link_of(real: &str) -> Option<String> {
    let name = std::path::Path::new(real).file_name()?.to_str()?;
    let link = format!("{}/{name}", link_dir());
    std::fs::symlink_metadata(&link).is_ok().then_some(link)
}

/// In a program's environment: its window opens without taking the
/// keyboard (`link spawn`'s terminal). The layer takes it out before the
/// shell starts, so what is started from that terminal takes it as ever.
pub const QUIET: &str = "PLEAMAR_OPEN_QUIET";

/// Whether that process was started with `QUIET`.
pub fn started_quiet(pid: u32) -> bool {
    pid != 0 && std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|e| e.split(|b| *b == 0).any(|v| v == format!("{QUIET}=1").as_bytes()))
}

/// Called as the shell (`argv[0]` is the real shell's name, `-fish` for a
/// login one): the layer, or the shell itself. Never returns then.
pub fn as_shell() {
    let Some(real) = std::env::var("PLEAMAR_REAL_SHELL").ok().filter(|s| !s.is_empty()) else { return };
    // SAFETY: called first thing in main, before any other thread starts.
    unsafe { std::env::remove_var(QUIET) };
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let Some(zero) = args.first().and_then(|a| std::path::Path::new(a).file_name()).and_then(|n| n.to_str()) else { return };
    let real_name = std::path::Path::new(&real).file_name().and_then(|n| n.to_str()).unwrap_or("");
    if zero.trim_start_matches('-') != real_name || real_name.is_empty() {
        return;
    }
    // SAFETY: isatty only asks.
    let on_terminal = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    let a_command = args.iter().skip(1).any(|a| a == "-c" || a == "--command");
    // Inside a layer already: on the very terminal one made (`PLEAMAR_TTY`).
    // Not just because the variable came down from a terminal further up —a
    // program started there, which starts a terminal of its own—.
    // SAFETY: ttyname's answer is read at once.
    let here = unsafe {
        let t = libc::ttyname(0);
        (!t.is_null()).then(|| std::ffi::CStr::from_ptr(t).to_string_lossy().into_owned())
    };
    let inside = here.is_some() && std::env::var("PLEAMAR_TTY").ok() == here;
    if on_terminal && !a_command && !inside && std::env::var_os("PLEAMAR_NO_TERM").is_none() {
        if let Err(e) = layer(&real, &args) {
            eprintln!("pleamar-wm · the terminal layer could not start ({e}): the shell as it is");
        }
    }
    exec(&real, &args, &[]);
}

/// The real shell in this process's place: its own name in front (`-fish`
/// stays a login shell), the rest of the words as they came.
fn exec(real: &str, args: &[std::ffi::OsString], env: &[(&str, String)]) -> ! {
    use std::os::unix::ffi::OsStrExt;
    let zero = args.first().map(|a| a.as_bytes().starts_with(b"-")).unwrap_or(false);
    let name = std::path::Path::new(real).file_name().map(|n| n.as_bytes().to_vec()).unwrap_or_default();
    let mut argv: Vec<std::ffi::CString> = vec![std::ffi::CString::new(if zero { [b"-".as_slice(), &name].concat() } else { name }).unwrap_or_default()];
    argv.extend(args.iter().skip(1).filter_map(|a| std::ffi::CString::new(a.as_bytes()).ok()));
    let path = std::ffi::CString::new(real).unwrap_or_default();
    // SAFETY: before exec, this process only sets its own environment.
    unsafe {
        std::env::set_var("SHELL", real);
        for (k, v) in env {
            std::env::set_var(k, v);
        }
    }
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    // SAFETY: argv is null-terminated and outlives the call.
    unsafe { libc::execv(path.as_ptr(), ptrs.as_ptr()) };
    eprintln!("pleamar-wm · {real}: {}", std::io::Error::last_os_error());
    std::process::exit(127);
}

/// What it keeps of the screen: the terminal's contents, what the program
/// said of itself (its title, the bell), and the history a redraw erased.
#[derive(Default)]
struct Seen {
    title: String,
    bells: u64,
    /// What `CSI 3 J` would have erased of the history (and the screen with
    /// it), each line with its number: older than vt100's own history.
    kept: std::collections::VecDeque<(u64, String)>,
    /// The number of the first line of vt100's history: every line before
    /// it is kept or gone. Lines count from 1.
    next: u64,
    /// The screen as `CSI 2 J` found it, and vt100's history then (its
    /// length, `next`): what a `CSI 3 J` right after keeps.
    cleared: Option<(Vec<String>, usize, u64)>,
}

impl vt100::Callbacks for Seen {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bells += 1;
    }
    // A line off the top of a full history: gone, and the numbers go on.
    fn on_scroll(&mut self, _: vt100::capture::RowContents<'_>, alternate: bool) {
        if !alternate {
            self.next += 1;
        }
    }
}

fn parser(rows: u16, cols: u16) -> vt100::Parser<Seen> {
    vt100::Parser::new_with_callbacks(nz(rows), nz(cols), HISTORY, Seen { next: 1, ..Seen::default() })
}

/// The erases a full redraw sends (`ESC[2J ESC[3J ESC[H`: the screen, then
/// the history), found as the bytes come (a sequence may be cut between
/// two reads): what comes before each, and each.
#[derive(Default)]
struct Erases {
    // A sequence that may be one, begun: from its ESC.
    held: Vec<u8>,
}

#[derive(Debug, PartialEq)]
enum Piece {
    Bytes(Vec<u8>),
    /// `CSI 2 J` or `CSI 3 J` (`CSI ? n J` too).
    Erase(u8),
}

impl Erases {
    fn split(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let (mut out, mut text) = (Vec::new(), Vec::new());
        for &b in bytes {
            if self.held.is_empty() && b != 0x1b {
                text.push(b);
                continue;
            }
            // An ESC begins another.
            if b == 0x1b {
                text.append(&mut self.held);
            }
            self.held.push(b);
            let erase = match self.held.as_slice() {
                [0x1b] | [0x1b, b'['] | [0x1b, b'[', b'?'] | [0x1b, b'[', b'2' | b'3'] | [0x1b, b'[', b'?', b'2' | b'3'] => continue,
                [0x1b, b'[', n @ (b'2' | b'3'), b'J'] | [0x1b, b'[', b'?', n @ (b'2' | b'3'), b'J'] => Some(n - b'0'),
                _ => None,
            };
            match erase {
                Some(n) => {
                    if !text.is_empty() {
                        out.push(Piece::Bytes(std::mem::take(&mut text)));
                    }
                    out.push(Piece::Erase(n));
                    self.held.clear();
                }
                None => text.append(&mut self.held),
            }
        }
        if !text.is_empty() {
            out.push(Piece::Bytes(text));
        }
        out
    }
}

/// What came out, read as a terminal would, except that what `CSI 3 J`
/// erases goes to the kept history first.
fn take(p: &mut vt100::Parser<Seen>, pieces: Vec<Piece>) {
    for piece in pieces {
        let alternate = p.screen().alternate_screen();
        match piece {
            Piece::Bytes(b) => p.process(&b),
            Piece::Erase(2) => {
                if !alternate {
                    let screen = p.screen().rows(0, p.screen().size().1.get()).collect();
                    let (len, next) = (p.screen().scrollback_len(), p.callbacks().next);
                    p.callbacks_mut().cleared = Some((screen, len, next));
                }
                p.process(b"\x1b[2J");
            }
            Piece::Erase(_) => {
                if !alternate {
                    keep_erased(p);
                }
                p.screen_mut().clear_scrollback();
            }
        }
    }
    // The same lines in all: the oldest kept go as vt100's history grows.
    let room = HISTORY.saturating_sub(p.screen().scrollback_len());
    let kept = &mut p.callbacks_mut().kept;
    while kept.len() > room {
        kept.pop_front();
    }
}

/// vt100's history to the kept history, and the screen `CSI 2 J` erased
/// just before if nothing has moved since (`ESC[2J ESC[3J`, `clear`): the
/// screen still shown stays where it is. What was kept is cut where they
/// print it again.
fn keep_erased(p: &mut vt100::Parser<Seen>) {
    let (mut lines, rows) = lines_of(p);
    lines.truncate(lines.len() - rows);
    let len = lines.len();
    let seen = p.callbacks_mut();
    if let Some((mut before, _, _)) = seen.cleared.take().filter(|c| c.1 == len && c.2 == seen.next) {
        while before.last().is_some_and(|l| l.trim().is_empty()) {
            before.pop();
        }
        lines.extend(before);
    }
    let at = reprint_at(seen.kept.iter().map(|(_, l)| l.as_str()), lines.iter().map(String::as_str));
    seen.kept.truncate(at.unwrap_or(seen.kept.len()));
    for line in lines {
        seen.kept.push_back((seen.next, line));
        seen.next += 1;
    }
}

/// Where a program that prints everything again after a clear (its
/// transcript) begins to do it in what was there before: the first place
/// from which all that was kept comes again: three lines that say something
/// at least, one after the other from where it starts, and what does not
/// follow them (a box it draws again under what is new), whole, further
/// down. Output that differs before that (a build run again) is not one,
/// nor a reprint not done yet: what was kept is never hidden.
fn reprint_at<'a>(before: impl Iterator<Item = &'a str>, again: impl Iterator<Item = &'a str>) -> Option<usize> {
    let again: Vec<&str> = again.map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if again.len() < 3 {
        return None;
    }
    let said: Vec<(usize, &str)> = before.map(str::trim_end).enumerate().filter(|(_, l)| !l.trim().is_empty()).collect();
    (0..said.len())
        .find(|&p| {
            let tail = &said[p..];
            let same = tail.iter().zip(&again).take_while(|((_, a), b)| a == *b).count();
            let rest: Vec<&str> = tail[same..].iter().map(|(_, l)| *l).collect();
            same >= 3 && (rest.is_empty() || again[same..].windows(rest.len()).any(|w| w == rest.as_slice()))
        })
        .map(|p| said[p].0)
}

/// Kitty's text sizing (`OSC 66 ; w=1 ; á ST`), which `vt100` does not know
/// and drops, text and all: opencode, in kitty, writes every letter that is
/// not ASCII that way, and its screen was read without them, each line moved
/// a cell to the left from there. Turned into the text it says, as the
/// bytes come (a sequence may be cut between two reads).
#[derive(Default)]
struct SizedText {
    // A sequence begun and not ended yet: from its ESC.
    held: Vec<u8>,
}

impl SizedText {
    fn plain(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            if self.held.is_empty() {
                if b == 0x1b {
                    self.held.push(b);
                } else {
                    out.push(b);
                }
                continue;
            }
            self.held.push(b);
            let h = &self.held;
            // Only an OSC is held: anything else goes on at once.
            if h.len() == 2 && h[1] != b']' {
                out.append(&mut self.held);
                continue;
            }
            let end = match h.len() {
                0..=2 => None,
                _ if b == 0x07 => Some(h.len() - 1),
                _ if b == b'\\' && h[h.len() - 2] == 0x1b => Some(h.len() - 2),
                _ => None,
            };
            // An ESC that does not end it: not one to read either.
            if end.is_none() && h.len() > 3 && h[h.len() - 2] == 0x1b {
                out.append(&mut self.held);
                continue;
            }
            if let Some(end) = end {
                let body = &h[2..end];
                match body.strip_prefix(b"66;").and_then(|r| r.iter().position(|c| *c == b';').map(|k| &r[k + 1..])) {
                    Some(text) => out.extend_from_slice(text),
                    None => out.extend_from_slice(h),
                }
                self.held.clear();
            } else if h.len() > 4096 {
                // No end in sight: not one to read, then.
                out.append(&mut self.held);
            }
        }
        out
    }
}

struct Shared {
    screen: Mutex<vt100::Parser<Seen>>,
    // Writes to the shell's terminal, from the keyboard and from outside,
    // one at a time: a paste is not cut by a key in its middle.
    input: Mutex<()>,
    master: RawFd,
    shell: libc::pid_t,
    // When something last came out, in ms since the start.
    last_out: std::sync::atomic::AtomicU64,
    // The person has typed something they have not sent yet (see `drafting`).
    draft: std::sync::atomic::AtomicBool,
    // What the agent's hooks said (`link hook`): how it is, and its answers
    // (how many, and the last one, exactly as it gave it).
    hook: Mutex<String>,
    answers: Mutex<(u64, String)>,
    started: std::time::Instant,
    // The last reads given with their marks (`lines`), to tell what changed since.
    given: Mutex<std::collections::VecDeque<Given>>,
}

/// Whether the person is in the middle of writing something, after these keys:
/// a letter (or a paste) starts it; Enter, Ctrl+C or Ctrl+U ends it. Escape
/// sequences —arrows, the mouse, the window taking the focus— are not writing,
/// nor what the terminal answers a program that asked it something (its
/// colours, its name: `ESC ] 11;rgb:… ESC \`, `ESC P >|kitty… ESC \`).
fn drafting(mut draft: bool, keys: &[u8]) -> bool {
    let mut k = 0;
    while k < keys.len() {
        match keys[k] {
            b'\r' | b'\n' | 0x03 | 0x15 => draft = false,
            0x1b => {
                // CSI (`ESC [` … a final byte) or SS3 (`ESC O` and one more), skipped;
                // a paste's own text, between its brackets, is still writing.
                match keys.get(k + 1) {
                    Some(b'[') => {
                        k += 2;
                        while k < keys.len() && !(0x40..=0x7e).contains(&keys[k]) {
                            k += 1;
                        }
                    }
                    Some(b'O') => k += 2,
                    // A string, up to BEL or ST (`ESC \`).
                    Some(b']' | b'P' | b'X' | b'^' | b'_') => {
                        k += 2;
                        while k < keys.len() && keys[k] != 0x07 && !(keys[k] == 0x1b && keys.get(k + 1) == Some(&b'\\')) {
                            k += 1;
                        }
                        if keys.get(k) == Some(&0x1b) {
                            k += 1;
                        }
                    }
                    _ => {}
                }
            }
            c if c >= 0x20 && c != 0x7f => draft = true,
            _ => {}
        }
        k += 1;
    }
    draft
}

fn size_of(fd: RawFd) -> libc::winsize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: ws is a winsize the kernel fills.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 || ws.ws_row == 0 || ws.ws_col == 0 {
        ws.ws_row = 24;
        ws.ws_col = 80;
    }
    ws
}

fn nz(v: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(v.max(1)).unwrap_or(std::num::NonZeroU16::MIN)
}

/// The layer: the shell on a pty of its own, everything passed through.
/// Returns only if it could not set itself up (then the shell runs as it is).
fn layer(real: &str, args: &[std::ffi::OsString]) -> Result<(), String> {
    // SAFETY: plain calls on this process's own terminal and a new pty; the
    // child only sets up its terminal and execs.
    unsafe {
        let mut outer: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut outer) != 0 {
            return Err("not a terminal".into());
        }
        let ws = size_of(0);
        let (mut master, mut slave) = (-1, -1);
        if libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), &outer, &ws) != 0 {
            return Err(format!("openpty: {}", std::io::Error::last_os_error()));
        }
        // The signals that matter are waited for by a thread of their own:
        // blocked here, before any thread is made, so they all inherit it.
        let mut wanted: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut wanted);
        for s in [libc::SIGWINCH, libc::SIGHUP, libc::SIGTERM, libc::SIGCHLD] {
            libc::sigaddset(&mut wanted, s);
        }
        let mut was: libc::sigset_t = std::mem::zeroed();
        libc::pthread_sigmask(libc::SIG_BLOCK, &wanted, &mut was);
        let me = libc::getpid();
        let child = libc::fork();
        if child < 0 {
            libc::pthread_sigmask(libc::SIG_SETMASK, &was, std::ptr::null_mut());
            libc::close(master);
            libc::close(slave);
            return Err(format!("fork: {}", std::io::Error::last_os_error()));
        }
        if child == 0 {
            libc::pthread_sigmask(libc::SIG_SETMASK, &was, std::ptr::null_mut());
            libc::close(master);
            libc::setsid();
            libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            if slave > 2 {
                libc::close(slave);
            }
            let tty = {
                let t = libc::ttyname(0);
                if t.is_null() { String::new() } else { std::ffi::CStr::from_ptr(t).to_string_lossy().into_owned() }
            };
            exec(real, args, &[("PLEAMAR_TERMINAL", me.to_string()), ("PLEAMAR_TTY", tty)]);
        }
        libc::close(slave);
        // This terminal raw: every byte, Ctrl+C included, goes to the shell's,
        // whose own rules make of it what they make.
        let mut raw = outer;
        libc::cfmakeraw(&mut raw);
        libc::tcsetattr(0, libc::TCSANOW, &raw);
        let shared = Arc::new(Shared {
            screen: Mutex::new(parser(ws.ws_row, ws.ws_col)),
            input: Mutex::new(()),
            master,
            shell: child,
            last_out: std::sync::atomic::AtomicU64::new(0),
            draft: std::sync::atomic::AtomicBool::new(false),
            hook: Mutex::new(String::new()),
            answers: Mutex::new((0, String::new())),
            started: std::time::Instant::now(),
            given: Mutex::new(std::collections::VecDeque::new()),
        });
        let socket = format!("{}/{me}.sock", socket_dir());
        // Typed: to the shell.
        {
            let s = shared.clone();
            std::thread::spawn(move || {
                let mut keyboard = std::fs::File::from_raw_fd(0);
                let mut buf = [0u8; 4096];
                loop {
                    match keyboard.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let was = s.draft.load(std::sync::atomic::Ordering::Relaxed);
                            s.draft.store(drafting(was, &buf[..n]), std::sync::atomic::Ordering::Relaxed);
                            let _g = s.input.lock();
                            write_all(s.master, &buf[..n]);
                        }
                    }
                }
                std::mem::forget(keyboard);
            });
        }
        // The size, followed; and leaving with the shell or the terminal.
        {
            let s = shared.clone();
            let socket = socket.clone();
            std::thread::spawn(move || loop {
                let mut sig: libc::c_int = 0;
                if libc::sigwait(&wanted, &mut sig) != 0 {
                    continue;
                }
                match sig {
                    libc::SIGWINCH => {
                        let ws = size_of(0);
                        libc::ioctl(s.master, libc::TIOCSWINSZ, &ws);
                        if let Ok(mut p) = s.screen.lock() {
                            p.set_size(nz(ws.ws_row), nz(ws.ws_col));
                        }
                    }
                    libc::SIGHUP | libc::SIGTERM => {
                        let _ = std::fs::remove_file(&socket);
                        libc::kill(s.shell, libc::SIGHUP);
                        libc::_exit(129);
                    }
                    _ => {}
                }
            });
        }
        // Asked from outside.
        if let Some(listener) = listen(&socket) {
            let s = shared.clone();
            std::thread::spawn(move || {
                for c in listener.incoming().flatten() {
                    let s = s.clone();
                    std::thread::spawn(move || answer(&s, c));
                }
            });
        }
        // What the shell says: to the terminal at once, and kept by another
        // thread (reading it as a terminal would is slower than passing it).
        // Up to 16 MB behind; past that, the shell waits for it.
        let (keep, kept) = std::sync::mpsc::channel::<Vec<u8>>();
        let behind = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let s = shared.clone();
            let behind = behind.clone();
            std::thread::spawn(move || {
                let (mut sized, mut erases) = (SizedText::default(), Erases::default());
                while let Ok(mut bytes) = kept.recv() {
                    // All that has come meanwhile, at once.
                    while let Ok(more) = kept.try_recv() {
                        bytes.extend_from_slice(&more);
                    }
                    if let Ok(mut p) = s.screen.lock() {
                        take(&mut p, erases.split(&sized.plain(&bytes)));
                    }
                    behind.fetch_sub(bytes.len(), std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
        let mut out = std::fs::File::from_raw_fd(1);
        let mut buf = [0u8; 65536];
        loop {
            let n = libc::read(master, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if n <= 0 {
                break;
            }
            let bytes = &buf[..n as usize];
            let _ = out.write_all(bytes);
            while behind.load(std::sync::atomic::Ordering::Relaxed) > 16 << 20 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            behind.fetch_add(bytes.len(), std::sync::atomic::Ordering::Relaxed);
            let _ = keep.send(bytes.to_vec());
            shared.last_out.store(shared.started.elapsed().as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        std::mem::forget(out);
        let _ = std::fs::remove_file(&socket);
        libc::tcsetattr(0, libc::TCSANOW, &outer);
        let mut status = 0;
        libc::waitpid(child, &mut status, 0);
        let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) };
        libc::_exit(code);
    }
}

fn write_all(fd: RawFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: fd is the pty's master, open for the layer's whole life.
        let n = unsafe { libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        bytes = &bytes[n as usize..];
    }
}

fn listen(path: &str) -> Option<std::os::unix::net::UnixListener> {
    let dir = std::path::Path::new(path).parent()?;
    std::fs::create_dir_all(dir).ok()?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let _ = std::fs::remove_file(path);
    std::os::unix::net::UnixListener::bind(path).ok()
}

/// What runs in the foreground of the shell's terminal: its process group's
/// leader, by name and the words it was started with, and where it is.
fn foreground(s: &Shared) -> (i32, String, String) {
    // SAFETY: asks the pty which group has it.
    let pg = unsafe { libc::tcgetpgrp(s.master) };
    let pid = if pg > 0 { pg } else { s.shell };
    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let words: Vec<String> = cmd.split(|b| *b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).map(|p| p.display().to_string()).unwrap_or_default();
    (pid, words.join(" "), cwd)
}

/// Which coding agent that is, if one: by its name or the script it runs.
pub fn agent_of(command: &str) -> Option<&'static str> {
    const KNOWN: [(&str, &str); 10] = [
        ("claude", "claude"),
        ("codex", "codex"),
        ("gemini", "gemini"),
        ("opencode", "opencode"),
        ("copilot", "copilot"),
        ("qwen", "qwen"),
        ("aider", "aider"),
        ("goose", "goose"),
        ("crush", "crush"),
        ("pi", "pi"),
    ];
    let words: Vec<&str> = command.split_whitespace().take(3).collect();
    for w in words {
        let base = std::path::Path::new(w).file_name().and_then(|n| n.to_str()).unwrap_or(w);
        if let Some((_, a)) = KNOWN.iter().find(|(k, _)| base == *k || base.starts_with(&format!("{k}-")) || w.contains(&format!("/{k}/")) || w.contains(&format!("/{k}-")) || w.contains(&format!("@{k}"))) {
            return Some(a);
        }
    }
    None
}

/// What an agent's title says of it: Claude Code turns a half-moon while it
/// works (◐◓◑◒) and shows ✳ when it waits for the person; others spin
/// braille dots.
pub fn state_of(title: &str) -> &'static str {
    match title.chars().next() {
        Some('◐' | '◓' | '◑' | '◒') => "working",
        Some('⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏') => "working",
        Some('✳') => "idle",
        _ => "",
    }
}

/// The last `n` lines, history and screen, without the blank ones at the end.
fn last_lines(p: &mut vt100::Parser<Seen>, n: usize) -> String {
    let (lines, _) = numbered(p);
    lines[lines.len().saturating_sub(n)..].iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n")
}

/// What is read, each line with its number: the history kept (cut where
/// what came after it prints it again), vt100's history and its screen,
/// without the blank lines at the end; and the number of the screen's
/// first line. A program on the alternate screen: its screen only.
fn numbered(p: &mut vt100::Parser<Seen>) -> (Vec<(u64, String)>, u64) {
    let (lines, rows) = lines_of(p);
    let seen = p.callbacks();
    let next = seen.next;
    let top = next + (lines.len() - rows) as u64;
    let kept = if p.screen().alternate_screen() { 0 } else { reprint_at(seen.kept.iter().map(|(_, l)| l.as_str()), lines.iter().map(String::as_str)).unwrap_or(seen.kept.len()) };
    let mut out: Vec<(u64, String)> = seen.kept.iter().take(kept).cloned().collect();
    out.extend(lines.into_iter().enumerate().map(|(k, l)| (next + k as u64, l.trim_end().to_owned())));
    while out.last().is_some_and(|(_, l)| l.trim().is_empty()) {
        out.pop();
    }
    (out, top)
}

/// A read given with its mark: the number of its last line, of the
/// screen's first then, what the lines from there to the last said (a
/// hash), and the last one.
struct Given {
    mark: u64,
    top: u64,
    hash: u64,
    last: String,
}

fn hash_of<'a>(lines: impl Iterator<Item = &'a (u64, String)>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.for_each(|l| l.hash(&mut h));
    h.finish()
}

/// `lines N [M] [peek]`: `mark` and the newest line's number, then the last N
/// lines (after line M). Lines read up to M that were written over since (a
/// TUI's screen, a redraw): `screen` and the screen instead. The line M
/// itself, only written on (a prompt being typed at), comes again. `peek`
/// (a wait looking again and again): the read is not remembered as given.
fn lines_after(lines: &[(u64, String)], top: u64, after: Option<u64>, n: usize, given: &mut std::collections::VecDeque<Given>, remember: bool) -> String {
    let mark = lines.last().map_or(top.saturating_sub(1), |l| l.0);
    let mut shown: Vec<&(u64, String)> = lines.iter().filter(|l| after.is_none_or(|m| l.0 > m)).collect();
    let mut head = format!("mark {mark}\n");
    if let Some(g) = after.and_then(|m| given.iter().rev().find(|g| g.mark == m)) {
        let last = lines.iter().find(|l| l.0 == g.mark);
        let same = hash_of(lines.iter().filter(|l| l.0 >= g.top && l.0 < g.mark)) == g.hash;
        match last {
            Some(l) if same && l.1 == g.last => {}
            Some(l) if same && l.1.starts_with(&g.last) => shown.insert(0, l),
            _ => {
                head.push_str("screen\n");
                shown = lines.iter().filter(|l| l.0 >= top).collect();
            }
        }
    }
    if remember {
        if given.len() >= 64 {
            given.pop_front();
        }
        given.push_back(Given { mark, top, hash: hash_of(lines.iter().filter(|l| l.0 >= top && l.0 < mark)), last: lines.last().map(|l| l.1.clone()).unwrap_or_default() });
    }
    let from = shown.len().saturating_sub(n);
    head + &shown[from..].iter().map(|(_, l)| format!("{l}\n")).collect::<String>()
}

/// vt100's history and its screen, every row, and how many are the screen's.
fn lines_of(p: &mut vt100::Parser<Seen>) -> (Vec<String>, usize) {
    let (rows, cols) = p.screen().size();
    let (rows, cols) = (rows.get() as usize, cols.get());
    let mut lines: Vec<String> = Vec::new();
    let back = p.screen().scrollback_capacity();
    // From the oldest still kept, a screen at a time.
    p.screen_mut().set_scrollback(back);
    let history = p.screen().scrollback();
    let mut offset = history;
    loop {
        p.screen_mut().set_scrollback(offset);
        let shown: Vec<String> = p.screen().rows(0, cols).collect();
        // A page `offset` back starts `offset` lines above the screen.
        let take = if offset >= rows { rows } else { offset };
        if offset == 0 {
            lines.extend(shown);
            break;
        }
        lines.extend(shown.into_iter().take(take));
        offset = offset.saturating_sub(take);
    }
    p.screen_mut().set_scrollback(0);
    (lines, rows)
}

/// A key by its name, as the program asked for it (its cursor keys in
/// application mode or not).
fn key_bytes(name: &str, app_cursor: bool) -> Option<Vec<u8>> {
    let arrow = |c: u8| if app_cursor { vec![0x1b, b'O', c] } else { vec![0x1b, b'[', c] };
    Some(match name {
        "enter" | "return" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "shift+tab" => b"\x1b[Z".to_vec(),
        "escape" | "esc" => b"\x1b".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "space" => b" ".to_vec(),
        "up" => arrow(b'A'),
        "down" => arrow(b'B'),
        "right" => arrow(b'C'),
        "left" => arrow(b'D'),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "page_up" | "pageup" => b"\x1b[5~".to_vec(),
        "page_down" | "pagedown" => b"\x1b[6~".to_vec(),
        _ if name.len() >= 2 && name.starts_with('f') => match name[1..].parse::<usize>().ok().filter(|n| (1..=12).contains(n))? {
            n @ 1..=4 => vec![0x1b, b'O', b"PQRS"[n - 1]],
            n => format!("\x1b[{}~", [15, 17, 18, 19, 20, 21, 23, 24][n - 5]).into_bytes(),
        },
        _ if name.starts_with("ctrl+") && name.len() == 6 => {
            let c = name.as_bytes()[5].to_ascii_lowercase();
            if !c.is_ascii_lowercase() {
                return None;
            }
            vec![c - b'a' + 1]
        }
        _ if name.chars().count() == 1 => name.as_bytes().to_vec(),
        _ => return None,
    })
}

/// One question, one answer, and the connection closes. `info`, `read N`,
/// `lines N [M]`, `screen`, `paste HEX`, `keys NAME...`.
fn answer(s: &Shared, mut c: std::os::unix::net::UnixStream) {
    let mut line = String::new();
    {
        let mut r = std::io::BufReader::new(&c);
        use std::io::BufRead;
        if r.read_line(&mut line).is_err() {
            return;
        }
    }
    let mut words = line.trim_end().splitn(2, ' ');
    let verb = words.next().unwrap_or("");
    let rest = words.next().unwrap_or("");
    let reply = match verb {
        "info" => {
            let (title, bells, rows, cols, alt, cursor, before) = match s.screen.lock() {
                Ok(p) => {
                    let (r, c) = p.screen().size();
                    let (row, col) = p.screen().cursor_position();
                    // The number of the cursor's line, as `lines` numbers them.
                    let cursor = p.callbacks().next + (p.screen().scrollback_len() + row as usize) as u64;
                    // What its line says before it: a prompt, when a shell waits.
                    let before = p.screen().contents_between(row, 0, row, col);
                    (p.callbacks().title.clone(), p.callbacks().bells, r.get(), c.get(), p.screen().alternate_screen(), cursor, before)
                }
                Err(_) => return,
            };
            let (pid, cmd, cwd) = foreground(s);
            let quiet = s.started.elapsed().as_millis() as u64 - s.last_out.load(std::sync::atomic::Ordering::Relaxed);
            format!(
                "layer {}\nterminal {}\nshell {}\nforeground {pid}\ncommand {cmd}\ncwd {cwd}\nagent {}\ntitle {title}\nstate {}\nsize {cols}x{rows}\nfullscreen {}\nbells {bells}\nquiet {quiet}\ncursor {cursor}\nhook {}\nanswers {}\ndraft {}\nbefore {before}\n",
                // SAFETY: plain questions about this process.
                unsafe { libc::getpid() },
                unsafe { libc::getppid() },
                s.shell,
                agent_of(&cmd).unwrap_or(""),
                state_of(&title),
                alt as u8,
                // (Only while the agent its hooks spoke for still runs.)
                if agent_of(&cmd).is_some() { s.hook.lock().map(|h| h.clone()).unwrap_or_default() } else { String::new() },
                s.answers.lock().map(|a| a.0).unwrap_or(0),
                s.draft.load(std::sync::atomic::Ordering::Relaxed) as u8,
            )
        }
        "read" => {
            let n = rest.trim().parse::<usize>().unwrap_or(200).clamp(1, HISTORY);
            match s.screen.lock() {
                Ok(mut p) => last_lines(&mut p, n) + "\n",
                Err(_) => return,
            }
        }
        // The same with their marks: `lines N [M] [peek]` (see `lines_after`).
        "lines" => {
            let mut w = rest.split_whitespace();
            let n = w.next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(200).clamp(1, HISTORY);
            let after = w.next().and_then(|m| m.parse::<u64>().ok());
            let Ok((lines, top)) = s.screen.lock().map(|mut p| numbered(&mut p)) else { return };
            match s.given.lock() {
                Ok(mut given) => lines_after(&lines, top, after, n, &mut given, w.next() != Some("peek")),
                Err(_) => return,
            }
        }
        "screen" => match s.screen.lock() {
            Ok(p) => p.screen().rows(0, p.screen().size().1.get()).map(|l| l.trim_end().to_owned() + "\n").collect::<String>().trim_end().to_owned() + "\n",
            Err(_) => return,
        },
        // Text as a paste: whole, bracketed if the program asked for that, so
        // an agent takes it as one thing and not as many lines typed.
        "paste" => match hex(rest.trim()) {
            Some(text) => {
                let bracketed = s.screen.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
                let _g = s.input.lock();
                if bracketed {
                    write_all(s.master, b"\x1b[200~");
                }
                write_all(s.master, &text);
                if bracketed {
                    write_all(s.master, b"\x1b[201~");
                }
                "ok\n".into()
            }
            None => "error: paste HEX\n".into(),
        },
        // Keys one by one, a little apart: an Escape and the key after it
        // are not read as Alt and that key.
        "keys" => {
            let app = s.screen.lock().map(|p| p.screen().application_cursor()).unwrap_or(false);
            let mut bad = None;
            let keys: Vec<Vec<u8>> = rest.split_whitespace().filter_map(|k| key_bytes(&k.to_lowercase(), app).or_else(|| { bad = Some(k.to_owned()); None })).collect();
            match bad {
                Some(k) => format!("error: no key called '{k}'\n"),
                None => {
                    for (i, k) in keys.iter().enumerate() {
                        if i > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        let _g = s.input.lock();
                        write_all(s.master, k);
                    }
                    "ok\n".into()
                }
            }
        }
        // What the agent's hooks say: `note state working|asking|done`, `note answer HEX`.
        "note" => match rest.split_once(' ') {
            Some(("state", v)) => {
                if let Ok(mut h) = s.hook.lock() {
                    *h = v.trim().to_owned();
                }
                "ok\n".into()
            }
            Some(("answer", v)) => match hex(v.trim()) {
                Some(b) => {
                    if let Ok(mut a) = s.answers.lock() {
                        *a = (a.0 + 1, String::from_utf8_lossy(&b).into_owned());
                    }
                    "ok\n".into()
                }
                None => "error: note answer HEX\n".into(),
            },
            _ => "error: note state S | note answer HEX\n".into(),
        },
        // How many answers the hooks told, and the last one.
        "answer" => s.answers.lock().map(|a| format!("{}\n{}", a.0, a.1)).unwrap_or_default(),
        _ => "error: info, read N, lines N [M], screen, paste HEX, keys NAME...\n".into(),
    };
    let _ = c.write_all(reply.as_bytes());
}

fn hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Asks the layer at that socket one thing.
pub fn ask(socket: &str, question: &str) -> Result<String, String> {
    let mut c = std::os::unix::net::UnixStream::connect(socket).map_err(|e| e.to_string())?;
    let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    c.write_all(format!("{question}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    c.read_to_string(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The layers running now: their id and what each says of itself. A socket
/// left by one that is gone is taken away.
pub fn layers() -> Vec<(i32, std::collections::BTreeMap<String, String>)> {
    layers_in(&socket_dir())
}

/// The same, of the session whose layers listen in that folder.
pub fn layers_in(folder: &str) -> Vec<(i32, std::collections::BTreeMap<String, String>)> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(folder) else { return out };
    for e in dir.flatten() {
        let path = e.path();
        let Some(id) = path.file_stem().and_then(|n| n.to_str()).and_then(|n| n.parse::<i32>().ok()) else { continue };
        match info(&path.to_string_lossy()) {
            Ok(info) => out.push((id, info)),
            // SAFETY: kill with 0 only asks whether it is there.
            Err(_) if unsafe { libc::kill(id, 0) } != 0 => {
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }
    out.sort_by_key(|(id, _)| *id);
    out
}

/// What the layer at that socket says of itself (`info`), by its words.
pub fn info(socket: &str) -> Result<std::collections::BTreeMap<String, String>, String> {
    Ok(ask(socket, "info")?.lines().filter_map(|l| l.split_once(' ').map(|(k, v)| (k.to_owned(), v.to_owned())).or_else(|| Some((l.to_owned(), String::new())))).collect())
}

/// The layer whose terminal is that process, if one. A terminal with
/// several shells (tabs) speaks for the one that moved last.
pub fn layer_of(pid: u32, layers: &[(i32, std::collections::BTreeMap<String, String>)]) -> Option<(i32, std::collections::BTreeMap<String, String>)> {
    shells_of(pid, layers).into_iter().next()
}

/// Every layer whose terminal is that process: its tabs, or the windows of
/// a terminal that serves many (foot's server). The one that moved last first.
pub fn shells_of(pid: u32, layers: &[(i32, std::collections::BTreeMap<String, String>)]) -> Vec<(i32, std::collections::BTreeMap<String, String>)> {
    let mut of: Vec<(i32, std::collections::BTreeMap<String, String>)> = layers.iter().filter(|(_, i)| i.get("terminal").and_then(|t| t.parse::<u32>().ok()) == Some(pid)).cloned().collect();
    of.sort_by_key(|(_, i)| i.get("quiet").and_then(|q| q.parse::<u64>().ok()).unwrap_or(u64::MAX));
    of
}

/// Whether the layer types that key by its name (`keys`).
pub fn knows_key(name: &str) -> bool {
    key_bytes(&name.to_lowercase(), false).is_some()
}

/// A question an agent's screen asks the person: what it says, and its
/// options —numbered, one of them under the cursor— or a yes/no.
#[derive(Debug, PartialEq)]
pub struct Question {
    pub text: String,
    pub options: Vec<(u32, String)>,
    /// Which of `options` the cursor is on.
    pub cursor: usize,
    pub yes_no: bool,
}

/// A line without the box an agent draws round its dialogs.
fn unboxed(l: &str) -> &str {
    l.trim().trim_matches(|c| matches!(c, '│' | '┃' | '║' | '|')).trim()
}

/// `❯ 2. Yes, and don't ask again`: whether the cursor is on it, its number, what it says.
fn option_line(l: &str) -> Option<(bool, u32, String)> {
    let t = unboxed(l);
    let (cursor, rest) = match t.chars().next() {
        Some(c @ ('❯' | '›' | '>' | '▶' | '→')) => (true, t[c.len_utf8()..].trim_start()),
        _ => (false, t),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: u32 = digits.parse().ok()?;
    let after = rest[digits.len()..].strip_prefix(['.', ')'])?;
    let text = after.strip_prefix(' ')?.trim();
    (!text.is_empty()).then(|| (cursor, n, text.to_owned()))
}

/// What a selection menu or a yes/no at the bottom of an agent's screen
/// asks, if one is there. A menu is numbered options counting from 1 with
/// a cursor on one of them (a numbered list in an answer has no cursor).
pub fn question(screen: &str) -> Option<Question> {
    let lines: Vec<&str> = screen.lines().collect();
    let tail = &lines[lines.len().saturating_sub(40)..];
    // The last run of options 1, 2, 3… (lines that describe one may come between).
    let mut best: Option<(usize, Vec<(usize, bool, u32, String)>)> = None;
    let mut run: Vec<(usize, bool, u32, String)> = Vec::new();
    for (k, l) in tail.iter().enumerate() {
        if let Some((c, n, t)) = option_line(l) {
            if n == 1 {
                run = vec![(k, c, n, t)];
            } else if run.last().is_some_and(|o| o.2 + 1 == n) {
                run.push((k, c, n, t));
            } else {
                run.clear();
            }
            if run.len() >= 2 && run.iter().any(|o| o.1) {
                best = Some((run[0].0, run.clone()));
            }
        }
    }
    // Only one still being asked: below its last option nothing but the box's
    // edge or how to answer («Enter to select · Esc to cancel»). One already
    // answered, with the agent's work after it, asks nothing.
    let footer = |l: &&str| {
        let t = unboxed(l).trim_matches(|c| matches!(c, '─' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘'));
        let low = t.to_lowercase();
        t.is_empty() || low.contains("esc") || low.contains("enter") || low.contains("navigate") || low.contains("select") || low.contains("confirm")
    };
    let best = best.filter(|(_, opts)| {
        let last = opts.last().map_or(0, |o| o.0);
        // (Lines that only go on describing the last option, indented, are its.)
        tail[last + 1..].iter().all(|l| footer(l) || (l.starts_with("     ") && !unboxed(l).is_empty() && option_line(l).is_none()))
    });
    if let Some((first, opts)) = best {
        // What it asks: the lines just above the options, up to a blank one.
        let mut text: Vec<&str> = Vec::new();
        for l in tail[..first].iter().rev() {
            let t = unboxed(l).trim_matches(|c| matches!(c, '─' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘'));
            if t.is_empty() {
                if text.is_empty() {
                    continue;
                }
                break;
            }
            text.push(t);
            if text.len() == 3 {
                break;
            }
        }
        text.reverse();
        let cursor = opts.iter().position(|o| o.1).unwrap_or(0);
        return Some(Question { text: text.join(" "), options: opts.into_iter().map(|o| (o.2, o.3)).collect(), cursor, yes_no: false });
    }
    if let Some(q) = unnumbered(tail) {
        return Some(q);
    }
    // A yes/no, on the last line that says something.
    let last = tail.iter().rev().map(|l| unboxed(l)).find(|l| !l.is_empty())?;
    let yn = ["[y/N]", "[Y/n]", "(y/n)", "[y/n]", "(Y/n)", "(y/N)"];
    yn.iter().any(|m| last.contains(m)).then(|| Question { text: last.to_owned(), options: Vec::new(), cursor: 0, yes_no: true })
}

/// A menu with no numbers, as Claude Code asks whether to trust a folder
/// (`❯ No, exit` / `  Yes, I trust this folder`): the lines just above how
/// to answer it («Enter to confirm · Esc to cancel»), one under the cursor.
/// Its options are numbered here from 1, in order.
fn unnumbered(tail: &[&str]) -> Option<Question> {
    let edge = |l: &str| unboxed(l).trim_matches(|c| matches!(c, '─' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘')).to_owned();
    let cursor = |t: &str| matches!(t.chars().next(), Some('❯' | '›' | '>' | '▶' | '→'));
    let said: Vec<usize> = (0..tail.len()).filter(|&k| !edge(tail[k]).is_empty()).collect();
    let &how = said.last()?;
    let low = edge(tail[how]).to_lowercase();
    if !(low.contains("esc") && (low.contains("enter") || low.contains("confirm") || low.contains("select"))) {
        return None;
    }
    // The options: the run of lines that say something just above it.
    let mut end = how;
    while end > 0 && edge(tail[end - 1]).is_empty() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && !edge(tail[start - 1]).is_empty() {
        start -= 1;
    }
    let opts: Vec<String> = tail[start..end].iter().map(|l| edge(l)).collect();
    let at: Vec<usize> = (0..opts.len()).filter(|&k| cursor(&opts[k])).collect();
    if !(2..=9).contains(&opts.len()) || at.len() != 1 || opts.iter().any(|o| option_line(o).is_some()) {
        return None;
    }
    // What it asks: what it says above them, up to a rule (a few lines at most).
    let mut text: Vec<String> = Vec::new();
    for l in tail[..start].iter().rev() {
        let t = l.trim();
        if t.chars().count() >= 10 && t.chars().all(|c| matches!(c, '─' | '━' | '═')) {
            break;
        }
        let t = edge(l);
        if !t.is_empty() {
            text.push(t);
        }
        if text.len() == 8 {
            break;
        }
    }
    text.reverse();
    let options = opts.iter().enumerate().map(|(k, o)| (k as u32 + 1, o.trim_start_matches(['❯', '›', '>', '▶', '→']).trim().to_owned())).collect();
    Some(Question { text: text.join(" "), options, cursor: at[0], yes_no: false })
}

/// For the scene, every second: in which windows (`windows`, `slot pid`, as
/// pleamar-wm writes it) an agent runs in a terminal through a layer, and
/// how it is —`ag.N` 1 working, 2 ready, 3 waiting for the person; `agt.N`
/// «claude · ~/Proyectos/pleamar»—. Said only when it changes. In a thread of
/// its own: a terminal slow to answer never holds the compositor.
pub fn watch_agents(windows: String, to_render: std::sync::mpsc::Sender<pleamar::scene::ToRender>) {
    use pleamar::scene::{intern, ToRender};
    // Its session's layers: next to the windows file, as what it starts is told.
    let folder = std::path::Path::new(&windows).parent().map(|p| format!("{}/term", p.display())).unwrap_or_else(socket_dir);
    std::thread::spawn(move || {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut said: std::collections::HashMap<usize, (u8, String)> = std::collections::HashMap::new();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let text = std::fs::read_to_string(&windows).unwrap_or_default();
            let slots: Vec<(usize, u32)> = text.lines().filter_map(|l| {
                let mut w = l.split('\t').next()?.split_whitespace();
                Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
            }).collect();
            let all = if slots.is_empty() { Vec::new() } else { layers_in(&folder) };
            let mut now: std::collections::HashMap<usize, (u8, String)> = std::collections::HashMap::new();
            for (slot, pid) in &slots {
                let mine: Vec<&(i32, std::collections::BTreeMap<String, String>)> = all.iter().filter(|(_, i)| i.get("terminal").and_then(|t| t.parse::<u32>().ok()) == Some(*pid)).collect();
                // A terminal with several shells: the one with an agent, the busiest.
                let Some((id, info)) = mine.iter().filter(|(_, i)| i.get("agent").is_some_and(|a| !a.is_empty())).min_by_key(|(_, i)| i.get("quiet").and_then(|q| q.parse::<u64>().ok()).unwrap_or(u64::MAX)) else { continue };
                let agent = info.get("agent").cloned().unwrap_or_default();
                let cwd = info.get("cwd").cloned().unwrap_or_default();
                let cwd = if !home.is_empty() && cwd.starts_with(&home) { format!("~{}", &cwd[home.len()..]) } else { cwd };
                let hook = info.get("hook").map(String::as_str).unwrap_or("");
                let working = if hook.is_empty() { info.get("state").map(String::as_str) == Some("working") } else { hook == "working" };
                let state = if hook == "asking" {
                    3
                } else if working {
                    1
                } else if ask(&format!("{folder}/{id}.sock"), "read 40").is_ok_and(|s| question(&s).is_some()) {
                    3
                } else {
                    2
                };
                now.insert(*slot, (state, format!("{agent} · {cwd}")));
            }
            let slots_known: std::collections::HashSet<usize> = said.keys().chain(now.keys()).copied().collect();
            for slot in slots_known {
                let new = now.get(&slot).cloned().unwrap_or((0, String::new()));
                if said.get(&slot) != Some(&new) {
                    let _ = to_render.send(ToRender::ExternalFact(intern(&format!("ag.{slot}")), new.0.to_string()));
                    let _ = to_render.send(ToRender::Text(intern(&format!("agt.{slot}")), new.1.clone()));
                    if new.0 == 0 {
                        said.remove(&slot);
                    } else {
                        said.insert(slot, new);
                    }
                }
            }
        }
    });
}

/// `pleamar-wm term list | read ID [N] | paste ID TEXT | keys ID NAME…`:
/// the layers, by hand (what `link` builds on).
pub fn cli(args: &[String]) -> i32 {
    let socket = |id: &str| format!("{}/{id}.sock", socket_dir());
    match args.first().map(String::as_str) {
        Some("list") | None => {
            for (id, info) in layers() {
                let g = |k: &str| info.get(k).cloned().unwrap_or_default();
                let agent = g("agent");
                println!(
                    "{id:>8}  terminal {}  {}{}  «{}»  {}",
                    g("terminal"),
                    if agent.is_empty() { g("command") } else { agent },
                    match g("state").as_str() {
                        "" => String::new(),
                        s => format!(" ({s})"),
                    },
                    g("title"),
                    g("cwd"),
                );
            }
            0
        }
        Some("info") if args.len() == 2 => print_reply(ask(&socket(&args[1]), "info")),
        Some("read") if args.len() >= 2 => print_reply(ask(&socket(&args[1]), &format!("read {}", args.get(2).map_or("200", String::as_str)))),
        Some("paste") if args.len() >= 3 => print_reply(ask(&socket(&args[1]), &format!("paste {}", to_hex(args[2..].join(" ").as_bytes())))),
        Some("keys") if args.len() >= 3 => print_reply(ask(&socket(&args[1]), &format!("keys {}", args[2..].join(" ")))),
        _ => {
            eprintln!("pleamar-wm term list | info ID | read ID [LINES] | paste ID TEXT | keys ID enter|tab|escape|up|ctrl+c…");
            2
        }
    }
}

fn print_reply(r: Result<String, String>) -> i32 {
    match r {
        Ok(text) if text.starts_with("error: ") => {
            eprint!("{text}");
            1
        }
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => {
            eprintln!("no such terminal layer: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writing_is_letters_until_enter_and_never_escapes() {
        assert!(drafting(false, b"hel"));
        assert!(!drafting(true, b"lo\r"));
        assert!(drafting(false, b"one\rtwo"));
        assert!(!drafting(true, &[0x03]));
        // Arrows, the focus coming in, a mouse click: not writing.
        assert!(!drafting(false, b"\x1b[A\x1b[I\x1b[<0;10;5M\x1bOP"));
        // A paste is.
        assert!(drafting(false, b"\x1b[200~some text\x1b[201~"));
        // What the terminal answers a program's questions is nobody writing.
        assert!(!drafting(false, b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\\x1bP>|kitty(0.42.1)\x1b\\\x1b]10;rgb:ffff/ffff/ffff\x07"));
        assert!(drafting(false, b"\x1b]11;rgb:0/0/0\x07hola"));
    }

    #[test]
    fn an_agents_title_says_whether_it_works() {
        assert_eq!(state_of("◑ Alternativa a quickshell"), "working");
        assert_eq!(state_of("✳ Modo Pokémon con chapas"), "idle");
        assert_eq!(state_of("⠹ codex"), "working");
        assert_eq!(state_of("abel@abel: ~"), "");
    }

    #[test]
    fn an_agent_is_known_by_what_it_runs() {
        assert_eq!(agent_of("claude --dangerously-skip-permissions"), Some("claude"));
        assert_eq!(agent_of("/usr/bin/codex"), Some("codex"));
        assert_eq!(agent_of("node /usr/lib/node_modules/@google/gemini-cli/dist/index.js"), Some("gemini"));
        assert_eq!(agent_of("vim notes.md"), None);
        assert_eq!(agent_of("pipewire"), None, "a name that starts like one is not it");
    }

    #[test]
    fn keys_are_sent_as_the_program_asked() {
        assert_eq!(key_bytes("up", false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("up", true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes("ctrl+c", false).unwrap(), [3]);
        assert_eq!(key_bytes("enter", false).unwrap(), b"\r");
        assert!(key_bytes("hyper+q", false).is_none());
    }

    #[test]
    fn a_permission_dialog_is_a_question() {
        let screen = "● I'll remove it.

╭──────────────────────────────────────────────────────╮
│ Bash command                                         │
│                                                      │
│   rm notes.tmp                                       │
│   Remove the scratch file                            │
│                                                      │
│ Do you want to proceed?                              │
│ ❯ 1. Yes                                             │
│   2. Yes, and don't ask again for rm commands        │
│   3. No, and tell Claude what to do differently (esc) │
╰──────────────────────────────────────────────────────╯";
        let q = question(screen).unwrap();
        assert_eq!(q.text, "Do you want to proceed?");
        assert_eq!(q.options.len(), 3);
        assert_eq!(q.options[1], (2, "Yes, and don't ask again for rm commands".to_owned()));
        assert_eq!(q.cursor, 0);
    }

    #[test]
    fn a_choice_with_descriptions_between_is_a_question() {
        let screen = "☐ Colour

Which colour should the bar be?

  1. Mint
     The default
❯ 2. Coral
     Warmer
  3. Type something.

Enter to select · ↑/↓ to navigate · Esc to cancel";
        let q = question(screen).unwrap();
        assert_eq!(q.text, "Which colour should the bar be?");
        assert_eq!(q.options.iter().map(|o| o.0).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(q.cursor, 1);
    }

    #[test]
    fn whether_to_trust_a_folder_is_a_question_with_no_numbers() {
        let screen = "~/Proyectos/pleamar-wm main ≡
❯ claude --model claude-opus-5-5

──────────────────────────────────────────────────────────────────────────────
 Accessing workspace:

 /home/abel/Proyectos/pleamar-wm

 Quick safety check: Is this a project you created or one you trust?

 Security guide

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";
        let q = question(screen).unwrap();
        assert!(q.text.starts_with("Accessing workspace: /home/abel/Proyectos/pleamar-wm Quick safety check"), "{}", q.text);
        assert_eq!(q.options, [(1, "No, exit".to_owned()), (2, "Yes, I trust this folder".to_owned())]);
        assert_eq!(q.cursor, 0);
        // Claude Code's own box, waiting for the person, asks nothing.
        assert_eq!(question("● Done.\n────────────────────\n❯ \n────────────────────\n  ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt"), None);
    }

    #[test]
    fn a_question_answered_already_is_no_question() {
        let screen = "│ Do you want to proceed?   │\n│ ❯ 1. Yes                  │\n│   2. No                   │\n╰───────────────────────────╯\nchosen: 1\nANSWER: DONE\n> ";
        assert_eq!(question(screen), None);
    }

    #[test]
    fn a_numbered_list_in_an_answer_is_not_a_question() {
        assert_eq!(question("● Steps:\n  1. Build it\n  2. Test it\n\n❯ "), None);
        assert!(question("Overwrite notes.md? [y/N]").unwrap().yes_no);
    }

    #[test]
    fn text_goes_through_as_hex() {
        assert_eq!(hex(&to_hex("echo «hola»\n".as_bytes())).unwrap(), "echo «hola»\n".as_bytes());
        assert!(hex("abc").is_none());
    }

    #[test]
    fn the_last_lines_come_from_the_history_and_the_screen() {
        let mut p = parser(3, 20);
        p.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
        assert_eq!(last_lines(&mut p, 10), "one\ntwo\nthree\nfour\nfive");
        assert_eq!(last_lines(&mut p, 2), "four\nfive");
        p.process(b"\x1b]2;\xe2\x9c\xb3 here\x07");
        assert_eq!(p.callbacks().title, "✳ here");
    }

    #[test]
    fn kittys_sized_text_is_read_as_its_text() {
        let mut p = parser(3, 30);
        let mut sized = SizedText::default();
        // As opencode writes «Salúdame qué», cut in the middle of a sequence,
        // one ended by BEL and one by ESC \.
        let all = "Sal\x1b]66;w=1;ú\x07dame qu\x1b]66;w=1;é\x1b\\ ok".as_bytes();
        for part in all.chunks(7) {
            p.process(&sized.plain(part));
        }
        p.process(&sized.plain(b"\r\n\x1b]2;title\x07\x1b[1mbold\x1b[0m"));
        assert_eq!(last_lines(&mut p, 2), "Salúdame qué ok\nbold");
        assert_eq!(p.callbacks().title, "title");
    }

    /// As the layer reads what comes: in pieces of that many bytes.
    fn feed(p: &mut vt100::Parser<Seen>, erases: &mut Erases, bytes: &[u8], cut: usize) {
        for part in bytes.chunks(cut) {
            take(p, erases.split(part));
        }
    }

    #[test]
    fn the_erases_are_found_wherever_they_are_cut() {
        let all = b"a\x1b[2J\x1b[3J\x1b[Hb\x1b[?3Jc\x1b[1Jd\x1b[33Je";
        for cut in 1..all.len() {
            let mut erases = Erases::default();
            let pieces: Vec<Piece> = all.chunks(cut).flat_map(|p| erases.split(p)).collect();
            let (mut bytes, mut found) = (Vec::new(), Vec::new());
            for p in pieces {
                match p {
                    Piece::Bytes(b) => bytes.extend(b),
                    Piece::Erase(n) => {
                        found.push((bytes.len(), n));
                    }
                }
            }
            assert_eq!(bytes, b"a\x1b[Hbc\x1b[1Jd\x1b[33Je", "cut every {cut}");
            assert_eq!(found, [(1, 2), (1, 3), (5, 3)], "cut every {cut}");
        }
    }

    #[test]
    fn a_full_redraw_keeps_the_history() {
        let mut p = parser(5, 30);
        let mut erases = Erases::default();
        let before: String = (1..=40).map(|k| format!("line {k}\r\n")).collect();
        feed(&mut p, &mut erases, before.as_bytes(), 7);
        feed(&mut p, &mut erases, b"\x1b[2J\x1b[3J\x1b[Hafter one\r\nafter two", 3);
        let text = last_lines(&mut p, HISTORY);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 42, "{text}");
        assert_eq!((lines[0], lines[39], lines[40], lines[41]), ("line 1", "line 40", "after one", "after two"));
        // `clear`'s: the same, the cursor home first.
        feed(&mut p, &mut erases, b"\r\nthird\x1b[H\x1b[2J\x1b[3J", 64);
        assert_eq!(last_lines(&mut p, 3), "after one\nafter two\nthird");
        // The history alone erased: the screen stays, read once.
        feed(&mut p, &mut erases, b"fourth\r\nfifth\x1b[3J", 64);
        assert_eq!(last_lines(&mut p, 5), "after one\nafter two\nthird\nfourth\nfifth");
        // The alternate screen: its screen only, as ever.
        feed(&mut p, &mut erases, b"\x1b[?1049h\x1b[Hvim\x1b[2J\x1b[3J\x1b[Hstill vim", 64);
        assert_eq!(last_lines(&mut p, HISTORY), "still vim");
        feed(&mut p, &mut erases, b"\x1b[?1049l", 64);
        assert_eq!(last_lines(&mut p, 3), "third\nfourth\nfifth");
    }

    #[test]
    fn a_transcript_printed_again_after_a_clear_is_read_once() {
        let mut p = parser(6, 30);
        let mut erases = Erases::default();
        let transcript = "> hello\r\n\r\nHi there.\r\n\r\n> list the files\r\n\r\na.txt b.txt\r\n";
        feed(&mut p, &mut erases, format!("{transcript}[input]").as_bytes(), 5);
        // Claude Code's redraw: everything again, and a little more.
        feed(&mut p, &mut erases, format!("\x1b[2J\x1b[3J\x1b[H{transcript}> thanks\r\n\r\nYou are welcome.\r\n[input]").as_bytes(), 9);
        assert_eq!(last_lines(&mut p, HISTORY).matches("> hello").count(), 1, "{}", last_lines(&mut p, HISTORY));
        // Twice: still once, nothing lost.
        feed(&mut p, &mut erases, format!("\x1b[2J\x1b[3J\x1b[H{transcript}> thanks\r\n\r\nYou are welcome.\r\n> bye\r\n[input]").as_bytes(), 64);
        let text = last_lines(&mut p, HISTORY);
        assert_eq!((text.matches("> hello").count(), text.matches("You are welcome.").count(), text.matches("[input]").count()), (1, 1, 1), "{text}");
        assert!(text.ends_with("> bye\n[input]"), "{text}");
        // What is not a reprint is kept whole before it.
        feed(&mut p, &mut erases, b"\x1b[2J\x1b[3J\x1b[Hsomething\r\nelse\r\nentirely\r\n", 64);
        let text = last_lines(&mut p, HISTORY);
        assert!(text.starts_with("> hello") && text.contains("[input]\nsomething\nelse\nentirely"), "{text}");
    }

    #[test]
    fn a_build_run_again_after_clear_keeps_both_runs() {
        let mut p = parser(8, 40);
        let mut erases = Erases::default();
        let run = |error: &str| format!("$ make\r\nCompiling x\r\nwarning: unused\r\nerror: {error}\r\nfailed\r\n");
        // `clear` (ncurses: `ESC[H ESC[2J ESC[3J`), and the same build again.
        feed(&mut p, &mut erases, format!("{}$ clear\r\n\x1b[H\x1b[2J\x1b[3J", run("A")).as_bytes(), 7);
        // Three lines as the first run's: not one yet.
        feed(&mut p, &mut erases, b"$ make\r\nCompiling x\r\nwarning: unused\r\n", 64);
        assert_eq!(last_lines(&mut p, HISTORY).matches("$ make").count(), 2);
        feed(&mut p, &mut erases, b"error: B\r\nfailed\r\n$ ", 64);
        let text = last_lines(&mut p, HISTORY);
        assert!(text.contains("error: A") && text.contains("error: B") && text.matches("Compiling x").count() == 2, "{text}");
        // Kept so after the next clear too.
        feed(&mut p, &mut erases, format!("clear\r\n\x1b[H\x1b[2J\x1b[3J{}$ ", run("C")).as_bytes(), 64);
        let text = last_lines(&mut p, HISTORY);
        assert_eq!((text.matches("error: A").count(), text.matches("error: B").count(), text.matches("error: C").count()), (1, 1, 1), "{text}");
    }

    #[test]
    fn a_reprint_is_one_only_once_all_that_was_kept_came_again() {
        let kept = ["> hello", "", "Hi there.", "> list", "a b", "[input]"];
        let read = |again: &[&str]| reprint_at(kept.iter().copied(), again.iter().copied());
        // The transcript again, more after it, its box last: from the start.
        assert_eq!(read(&["> hello", "Hi there.", "> list", "a b", "> thanks", "[input]"]), Some(0));
        assert_eq!(read(&["> hello", "Hi there.", "> list", "a b", "[input]"]), Some(0));
        // Its box with more under it: once it exits, the shell's prompt.
        assert_eq!(read(&["> hello", "Hi there.", "> list", "a b", "> thanks", "[input]", "$"]), Some(0));
        // Not done yet, or its box another: kept whole.
        assert_eq!(read(&["> hello", "Hi there.", "> list"]), None);
        assert_eq!(read(&["> hello", "Hi there.", "> list", "a b", "> thanks", "[busy]"]), None);
        // From further down: only from there.
        assert_eq!(read(&["> list", "a b", "[input]"]), Some(3));
    }

    #[test]
    fn the_history_kept_is_the_same_lines_in_all() {
        let mut p = parser(5, 20);
        let mut erases = Erases::default();
        let lines: String = (1..=1500).map(|k| format!("{k}\r\n")).collect();
        feed(&mut p, &mut erases, lines.as_bytes(), 4096);
        feed(&mut p, &mut erases, b"\x1b[2J\x1b[3J\x1b[H", 64);
        feed(&mut p, &mut erases, lines.replace("\r\n", "b\r\n").as_bytes(), 4096);
        // The history (kept and vt100's) and the screen's four lines.
        let (all, _) = numbered(&mut p);
        assert_eq!(all.len(), HISTORY + 4);
        assert_eq!((all[0].1.as_str(), all.last().unwrap().1.as_str()), ("997", "1500b"));
        // Numbered on from the first line ever, whatever went.
        assert_eq!(all.last().unwrap().0, 3000);
        assert!(all.windows(2).all(|w| w[1].0 == w[0].0 + 1));
        // vt100's own history full: lines off its top go, the numbers on.
        feed(&mut p, &mut erases, lines.replace("\r\n", "c\r\n").as_bytes(), 4096);
        let (all, _) = numbered(&mut p);
        assert_eq!((all.len(), all.last().unwrap().0, all.last().unwrap().1.as_str()), (HISTORY + 4, 4500, "1500c"));
        assert!(all.windows(2).all(|w| w[1].0 == w[0].0 + 1));
    }

    #[test]
    fn lines_after_a_mark_are_what_is_new_or_the_screen_written_over() {
        let mut p = parser(4, 20);
        let mut erases = Erases::default();
        let mut given = std::collections::VecDeque::new();
        let mut lines = |p: &mut vt100::Parser<Seen>, after: Option<u64>| {
            let (l, top) = numbered(p);
            lines_after(&l, top, after, HISTORY, &mut given, true)
        };
        feed(&mut p, &mut erases, b"one\r\ntwo\r\nthree\r\nfour\r\n$ ", 64);
        assert_eq!(lines(&mut p, None), "mark 5\none\ntwo\nthree\nfour\n$\n");
        // Typed at the prompt read last: it comes again, with what followed.
        feed(&mut p, &mut erases, b"ls\r\na b\r\n$ ", 64);
        assert_eq!(lines(&mut p, Some(5)), "mark 7\n$ ls\na b\n$\n");
        assert_eq!(lines(&mut p, Some(7)), "mark 7\n");
        // A TUI that redraws its screen in place: the screen again.
        feed(&mut p, &mut erases, b"\x1b[H\x1b[2Ktop\x1b[4;1H\x1b[2Kbottom", 64);
        assert_eq!(lines(&mut p, Some(7)), "mark 7\nscreen\ntop\n$ ls\na b\nbottom\n");
        // A mark it does not remember: the lines after it.
        assert_eq!(lines(&mut p, Some(6)), "mark 7\nbottom\n");
    }
}
