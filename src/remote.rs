//! `pleamar-wm remote …`: this desktop from a browser somewhere else —the
//! monitors seen, the mouse and the keyboard used— to work from there.
//!
//! - `pleamar-wm remote setup` makes a password and a key for six-digit
//!   codes (TOTP, an authenticator app on the phone), kept in
//!   `~/.config/pleamar/remote.conf`.
//! - `pleamar-wm remote` serves the page on 127.0.0.1 (port 8765, or `port N`
//!   in that file). Something in front makes it reachable and encrypted:
//!   `tailscale funnel --bg --https=8443 http://127.0.0.1:8765`.
//!
//! The picture: the monitor is taken with grim (wlr-screencopy) and only the
//! squares that changed travel, as JPEG, each time the page says it has
//! painted the last ones. The hands: a pointer and a keyboard made with
//! uinput, which the session takes as any other mouse and keyboard plugged
//! in —its shortcuts, its bar, everything—. Not the agent's: these are yours.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Digest;
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

const PAGE: &str = include_str!("remote.html");
/// The page's version: a page left open —an app on a phone's home screen
/// lives for days— that hears another one from its server is older than it.
const PAGE_VERSION: u64 = fnv(PAGE.as_bytes());
/// Its icon on a home screen (the orb it greets with).
const ICON_180: &[u8] = include_bytes!("icons/icon-180.png");
const ICON_192: &[u8] = include_bytes!("icons/icon-192.png");
const ICON_512: &[u8] = include_bytes!("icons/icon-512.png");
const PORT: u16 = 8765;
const TILE: usize = 64;
/// How long a session lasts without being used, and at most.
const IDLE: Duration = Duration::from_secs(8 * 3600);
const LIFE: Duration = Duration::from_secs(24 * 3600);
/// A page connected with no mouse or key from it for this long is signed
/// out: a laptop closed with the page open (its browser kept it connected,
/// and kept the picture coming) is not someone using this desktop.
const UNUSED: Duration = Duration::from_secs(30 * 60);
/// How long the session waits on the phone with no page of the phone
/// connected (switched to another app, the network gone a moment) before it
/// goes back to the desk.
const PHONE_GRACE: Duration = Duration::from_secs(3 * 60);

const HELP: &str = "pleamar-wm remote — this desktop from a browser elsewhere

  setup        a new password and a key for six-digit codes (an authenticator app):
               written to ~/.config/pleamar/remote.conf, shown once
  (nothing)    serve the page on 127.0.0.1:8765 (`port N` in remote.conf changes it)
  --view-only  the same, but only to watch: the mouse and keys there do nothing here
  stop         from this computer: everyone using it from elsewhere out, now, and every
               session ended (Super+Shift+Escape in pleamar-wm's keys)
  send FILE…   offer files or folders to the page connected now, to download there
               (the same as copying them in a file manager: it puts them on the clipboard)

Files dropped on the page, or sent with its button, land in «Remote» in your
downloads folder; dropped on a window, that window is given them too.

Make it reachable with something that encrypts it, for example:
  tailscale funnel --bg --https=8443 http://127.0.0.1:8765";

pub fn run(args: &[String]) -> i32 {
    let result = match args.first().map(String::as_str) {
        None | Some("serve") => serve(false),
        Some("--view-only") => serve(true),
        Some("setup") => setup(),
        Some("stop") => stop(),
        Some("send") => send(&args[1..]),
        Some("help") | Some("--help") | Some("-h") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => Err(format!("unknown: {other}\n\n{HELP}")),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("pleamar-wm remote: {e}");
            1
        }
    }
}

// ---------------------------------------------------------------- settings

struct Config {
    password: [u8; 32],
    totp: Vec<u8>,
    port: u16,
}

fn config_path() -> String {
    let base = std::env::var("XDG_CONFIG_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    format!("{base}/pleamar/remote.conf")
}

fn load() -> Result<Config, String> {
    let path = config_path();
    let text = std::fs::read_to_string(&path).map_err(|_| format!("no {path}: run `pleamar-wm remote setup` first"))?;
    let (mut password, mut totp, mut port) = (None, None, PORT);
    for line in text.lines() {
        let mut f = line.split_whitespace();
        match (f.next(), f.next()) {
            (Some("password-sha256"), Some(v)) => password = unhex(v).and_then(|b| b.try_into().ok()),
            (Some("totp"), Some(v)) => totp = unbase32(v),
            (Some("port"), Some(v)) => port = v.parse().map_err(|_| format!("{path}: port {v}?"))?,
            _ => {}
        }
    }
    Ok(Config {
        password: password.ok_or(format!("{path}: no password-sha256"))?,
        totp: totp.ok_or(format!("{path}: no totp"))?,
        port,
    })
}

fn setup() -> Result<(), String> {
    // About 100 bits, in pieces that can be read out and typed.
    const LETTERS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let bytes = random(20);
    let mut password = String::new();
    for (k, b) in bytes.iter().enumerate() {
        if k > 0 && k % 5 == 0 {
            password.push('-');
        }
        password.push(LETTERS[*b as usize % LETTERS.len()] as char);
    }
    let secret = random(20);
    let key = base32(&secret);
    let path = config_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let port = load().map(|c| c.port).unwrap_or(PORT);
    let text = format!(
        "# pleamar-wm remote: made by `pleamar-wm remote setup` (run it again for new ones).\npassword-sha256 {}\ntotp {key}\nport {port}\n",
        hex(&sha2::Sha256::digest(password.as_bytes()))
    );
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| format!("{path}: {e}"))?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let host = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_owned()).unwrap_or_else(|_| "pleamar".into());
    println!("Written to {path}. Shown only now:\n");
    println!("  password  {password}");
    println!("  code key  {key}");
    println!("  for an authenticator app: otpauth://totp/pleamar:{host}?secret={key}&issuer=pleamar");
    println!("\nA running `pleamar-wm remote` reads them when it starts again.");
    Ok(())
}

// ---------------------------------------------------------------- the server

struct Gate {
    config: Config,
    /// Sessions, by their cookie's hash: when they began and when they were
    /// last used (seconds since 1970). Kept in a file, so that starting the
    /// server again does not sign anyone out.
    sessions: HashMap<String, (u64, u64)>,
    saved: u64,
    /// Failed sign-ins, by address and all together.
    failures: HashMap<String, Vec<Instant>>,
    all_failures: Vec<Instant>,
    /// The last code taken: one is good once.
    last_step: u64,
    /// The pages connected now: which monitor each looks at, and from where.
    present: HashMap<u64, (usize, String)>,
    next_page: u64,
    /// Raised by `pleamar-wm remote stop`: every page connected goes.
    kicked: u64,
    /// The session is on the phone (docs/phone.md): how many of the phone's
    /// pages are connected, and since when none is.
    phone_on: bool,
    phone_pages: u32,
    phone_left: Option<Instant>,
    /// A newer program is installed: the pages are told, and one asks to start again.
    newer: bool,
}

impl Gate {
    fn valid(&mut self, token: &str) -> bool {
        let now = now_secs();
        let before = self.sessions.len();
        self.sessions.retain(|_, (born, used)| now.saturating_sub(*used) < IDLE.as_secs() && now.saturating_sub(*born) < LIFE.as_secs());
        let found = match self.sessions.get_mut(&hex(&sha2::Sha256::digest(token.as_bytes()))) {
            Some((_, used)) => {
                *used = now;
                true
            }
            None => false,
        };
        if self.sessions.len() != before || now.saturating_sub(self.saved) > 60 {
            self.save();
        }
        found
    }

    fn sign_out(&mut self, token: &str) {
        self.sessions.remove(&hex(&sha2::Sha256::digest(token.as_bytes())));
        self.save();
    }

    fn sessions_path() -> String {
        let base = std::env::var("XDG_STATE_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{}/.local/state", std::env::var("HOME").unwrap_or_default()));
        format!("{base}/pleamar/remote-sessions")
    }

    /// The sessions as the file has them (only hashes: the file does not let anyone in).
    fn restore(&mut self) {
        let Ok(text) = std::fs::read_to_string(Self::sessions_path()) else { return };
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            match f[..] {
                ["step", n] => self.last_step = n.parse().unwrap_or(0),
                [hash, born, used] => {
                    if let (Ok(b), Ok(u)) = (born.parse(), used.parse()) {
                        self.sessions.insert(hash.to_owned(), (b, u));
                    }
                }
                _ => {}
            }
        }
    }

    fn save(&mut self) {
        use std::os::unix::fs::OpenOptionsExt;
        self.saved = now_secs();
        let mut text = format!("step {}\n", self.last_step);
        for (hash, (born, used)) in &self.sessions {
            text.push_str(&format!("{hash} {born} {used}\n"));
        }
        let path = Self::sessions_path();
        if let Some(dir) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let new = format!("{path}.new");
        let written = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new).and_then(|mut f| f.write_all(text.as_bytes()));
        if written.is_ok() {
            let _ = std::fs::rename(&new, &path);
        }
    }

    fn blocked(&mut self, who: &str) -> bool {
        let window = Duration::from_secs(15 * 60);
        let now = Instant::now();
        self.all_failures.retain(|t| now.duration_since(*t) < window);
        let mine = self.failures.entry(who.to_owned()).or_default();
        mine.retain(|t| now.duration_since(*t) < window);
        mine.len() >= 5 || self.all_failures.len() >= 20
    }

    fn sign_in(&mut self, who: &str, password: &str, code: &str) -> Option<String> {
        let typed = sha2::Sha256::digest(password.trim().as_bytes());
        let password_ok = same(&typed, &self.config.password);
        let step = now_secs() / 30;
        let code = code.trim().replace(' ', "");
        let mut code_step = None;
        for s in [step - 1, step, step + 1] {
            if s > self.last_step && same(totp(&self.config.totp, s).as_bytes(), code.as_bytes()) {
                code_step = Some(s);
            }
        }
        match (password_ok, code_step) {
            (true, Some(s)) => {
                self.last_step = s;
                let token = hex(&random(32));
                let now = now_secs();
                self.sessions.insert(hex(&sha2::Sha256::digest(token.as_bytes())), (now, now));
                self.save();
                Some(token)
            }
            _ => {
                self.failures.entry(who.to_owned()).or_default().push(Instant::now());
                self.all_failures.push(Instant::now());
                None
            }
        }
    }
}

fn serve(view_only: bool) -> Result<(), String> {
    let config = load()?;
    let port = config.port;
    // One left from before (a session that ended, one started by hand) goes:
    // this is the one the session started now.
    take_over(port);
    let mut gate = Gate { config, sessions: HashMap::new(), saved: 0, failures: HashMap::new(), all_failures: Vec::new(), last_step: 0, present: HashMap::new(), next_page: 0, kicked: 0, phone_on: false, phone_pages: 0, phone_left: None, newer: false };
    gate.restore();
    // Started again (an update) with the session on the phone: it waits there
    // for the phone's page as if it had just left.
    if crate::agent_cli::monitors().is_ok_and(|m| phone_index(&m).is_some()) {
        gate.phone_on = true;
        gate.phone_left = Some(Instant::now());
    }
    if let Ok(path) = std::env::current_exe() {
        let id = exe_id(&path);
        let _ = EXE.set((path, id));
    }
    let gate = Arc::new(Mutex::new(gate));
    // The session is told who is in (it marks it on the monitors), and this
    // computer can send everyone away (`pleamar-wm remote stop`).
    let told = gate.clone();
    std::thread::spawn(move || tell_the_session(told));
    let door = gate.clone();
    std::thread::spawn(move || control(door));
    let keeper = gate.clone();
    std::thread::spawn(move || keep_phone(keeper));
    let watcher = gate.clone();
    std::thread::spawn(move || watch_binary(watcher));
    let hands = Arc::new(Mutex::new(None::<Hands>));
    let mut tries = 0;
    let listener = loop {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => break l,
            // The one before still letting go of it.
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(format!("127.0.0.1:{port}: {e}")),
        }
    };
    println!("pleamar-wm remote · on http://127.0.0.1:{port}{}", if view_only { " · only to watch" } else { "" });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let (gate, hands) = (gate.clone(), hands.clone());
        std::thread::spawn(move || {
            // A connection opened ahead and never used (browsers do) is not news.
            if let Err(e) = connection(stream, &gate, &hands, view_only).map_err(|e| e.to_string()) {
                if e != "bad request" && !e.contains("os error 11") {
                    eprintln!("remote · {e}");
                }
            }
        });
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    /// A file sent up (`POST /upload`): its body is read from here as it comes.
    rest: Option<BufReader<TcpStream>>,
}

fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    stream.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut first = String::new();
    reader.read_line(&mut first).map_err(|e| e.to_string())?;
    let mut parts = first.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("").to_owned(), parts.next().unwrap_or("/").to_owned());
    let mut headers = HashMap::new();
    let mut size = first.len();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        size += n;
        if n == 0 || size > 32 * 1024 {
            return Err("bad request".into());
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    if method == "POST" && path.starts_with("/upload?") {
        return Ok(Request { method, path, headers, body: Vec::new(), rest: Some(reader) });
    }
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0).min(16 * 1024);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Request { method, path, headers, body, rest: None })
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, extra: &str, body: &[u8]) -> Result<(), String> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self'; frame-ancestors 'none'\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body)).map_err(|e| e.to_string())
}

fn cookie(req: &Request) -> Option<String> {
    req.headers.get("cookie")?.split(';').map(str::trim).find_map(|c| c.strip_prefix("pleamar_remote=")).map(str::to_owned)
}

/// Who is asking: the address the proxy in front says (Tailscale's), or the socket's.
fn who(req: &Request, stream: &TcpStream) -> String {
    req.headers
        .get("x-forwarded-for")
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_owned())
        .unwrap_or_else(|| stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default())
}

/// Whose relay an address is, if it is one of those that hide a browser's
/// own (Safari's «Hide IP Address», iCloud Private Relay, Cloudflare WARP):
/// a page behind one cannot be reached by the direct way, and goes the slow
/// one. Seen from an iPhone on the same Wi-Fi where a computer went direct.
fn relay_of(address: &str) -> Option<&'static str> {
    let ip: std::net::IpAddr = address.trim().parse().ok()?;
    let within = |net: &str, bits: u32| -> bool {
        match (ip, net.parse::<std::net::IpAddr>()) {
            (std::net::IpAddr::V4(a), Ok(std::net::IpAddr::V4(n))) => bits == 0 || (u32::from(a) ^ u32::from(n)) >> (32 - bits) == 0,
            (std::net::IpAddr::V6(a), Ok(std::net::IpAddr::V6(n))) => bits == 0 || (u128::from(a) ^ u128::from(n)) >> (128 - bits) == 0,
            _ => false,
        }
    };
    [
        ("2a09:bac0::", 29, "Cloudflare"),
        ("104.28.0.0", 16, "Cloudflare"),
        ("2a02:26f7::", 32, "Akamai"),
        ("172.224.0.0", 12, "Akamai"),
    ]
    .iter()
    .find(|(net, bits, _)| within(net, *bits))
    .map(|(_, _, who)| *who)
}

fn page(signed_in: bool, error: &str, relay: &str) -> String {
    let (login, app) = if signed_in { ("none", "block") } else { ("flex", "none") };
    PAGE.replace("{{LOGIN}}", login).replace("{{APP}}", app).replace("{{ERROR}}", error).replace("{{RELAY}}", relay).replace("{{VERSION}}", &format!("{PAGE_VERSION:016x}"))
}

const fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
        i += 1;
    }
    h
}

/// What makes the page an app on a phone's home screen: all of the screen,
/// held any way, with its own icon.
fn manifest() -> String {
    let host = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_owned()).unwrap_or_default();
    let name = if host.is_empty() { "pleamar".to_owned() } else { format!("pleamar · {}", host.replace(['"', '\\'], "")) };
    format!(
        r##"{{"name":"{name}","short_name":"pleamar","id":"/","start_url":"/","scope":"/","display":"fullscreen","display_override":["fullscreen","standalone"],"orientation":"any","background_color":"#071014","theme_color":"#071014","icons":[{{"src":"/icon-192.png","sizes":"192x192","type":"image/png","purpose":"any maskable"}},{{"src":"/icon-512.png","sizes":"512x512","type":"image/png","purpose":"any maskable"}}]}}"##
    )
}

/// This program on disk, as it was when it started: when it changes (an
/// update installed), the one running starts again from it —at once if no
/// page is connected, or when a page asks (`update`)—.
static EXE: std::sync::OnceLock<(std::path::PathBuf, Option<(u64, u64, SystemTime)>)> = std::sync::OnceLock::new();

fn exe_id(path: &std::path::Path) -> Option<(u64, u64, SystemTime)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.ino(), m.len(), m.modified().ok()?))
}

fn watch_binary(gate: Arc<Mutex<Gate>>) {
    let Some((path, Some(first))) = EXE.get() else { return };
    let mut last = *first;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let Some(now) = exe_id(path) else { continue };
        // Changed, and the same for a moment: written whole.
        let settled = now == last;
        last = now;
        if now == *first || !settled {
            continue;
        }
        let mut g = gate.lock().unwrap();
        if !g.newer {
            g.newer = true;
            println!("remote · a newer pleamar-wm is installed");
        }
        if g.present.is_empty() {
            drop(g);
            again();
        }
    }
}

/// Started again from the program on disk now, in place (the same process,
/// its arguments, its environment). The pages come back to it on their own
/// —their sessions are on disk—, and a phone keeps its monitor meanwhile.
fn again() {
    use std::os::unix::process::CommandExt;
    let Some((path, _)) = EXE.get() else { return };
    println!("remote · starting again from the new one");
    let e = std::process::Command::new(path).args(std::env::args_os().skip(1)).exec();
    eprintln!("remote · could not start again: {e}");
}

fn connection(mut stream: TcpStream, gate: &Arc<Mutex<Gate>>, hands: &Arc<Mutex<Option<Hands>>>, view_only: bool) -> Result<(), String> {
    let mut req = read_request(&mut stream)?;
    let token = cookie(&req);
    let signed_in = token.as_deref().is_some_and(|t| gate.lock().unwrap().valid(t));
    let html = "text/html; charset=utf-8";
    match (req.method.as_str(), req.path.split('?').next().unwrap_or("/")) {
        ("GET", "/") => {
            // Back at the door after a while unused: why.
            let why = if !signed_in && req.path.ends_with("?unused") { "Signed out after half an hour without use." } else { "" };
            let relay = relay_of(&who(&req, &stream)).unwrap_or("");
            respond(&mut stream, "200 OK", html, "", page(signed_in, why, relay).as_bytes())
        }
        ("POST", "/login") => {
            let who = who(&req, &stream);
            let form = form(&String::from_utf8_lossy(&req.body));
            let (password, code) = (form.get("password").cloned().unwrap_or_default(), form.get("code").cloned().unwrap_or_default());
            if gate.lock().unwrap().blocked(&who) {
                std::thread::sleep(Duration::from_secs(2));
                return respond(&mut stream, "429 Too Many Requests", html, "", page(false, "Too many tries. Wait a quarter of an hour.", "").as_bytes());
            }
            let signed = gate.lock().unwrap().sign_in(&who, &password, &code);
            match signed {
                Some(token) => {
                    notify(&format!("Someone signed in to this desktop from {who}"));
                    if let Some(relay) = relay_of(&who) {
                        println!("remote · {who} is {relay}'s relay (a browser hiding its address): the direct way will not reach that page");
                    }
                    let set = format!("Set-Cookie: pleamar_remote={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}\r\nLocation: /\r\n", LIFE.as_secs());
                    respond(&mut stream, "303 See Other", html, &set, b"")
                }
                None => {
                    std::thread::sleep(Duration::from_secs(1));
                    respond(&mut stream, "401 Unauthorized", html, "", page(false, "That password or code is not right.", "").as_bytes())
                }
            }
        }
        ("POST", "/logout") => {
            if let Some(t) = token {
                gate.lock().unwrap().sign_out(&t);
            }
            respond(&mut stream, "303 See Other", html, "Set-Cookie: pleamar_remote=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0\r\nLocation: /\r\n", b"")
        }
        // A home screen's app: what it is and its icon. Nothing of this desktop in
        // them, so before signing in (a browser asks for them without the cookie).
        ("GET", "/manifest.webmanifest") => respond(&mut stream, "200 OK", "application/manifest+json", "", manifest().as_bytes()),
        ("GET", "/icon-180.png") => respond(&mut stream, "200 OK", "image/png", "", ICON_180),
        ("GET", "/icon-192.png") => respond(&mut stream, "200 OK", "image/png", "", ICON_192),
        ("GET", "/icon-512.png") => respond(&mut stream, "200 OK", "image/png", "", ICON_512),
        ("GET", "/ws") => {
            if !signed_in {
                return respond(&mut stream, "401 Unauthorized", "text/plain", "", b"sign in first");
            }
            // Only from this page: a page elsewhere cannot use the cookie here.
            let host = req.headers.get("host").cloned().unwrap_or_default();
            let origin = req.headers.get("origin").cloned().unwrap_or_default();
            if origin != format!("https://{host}") && origin != format!("http://{host}") {
                return respond(&mut stream, "403 Forbidden", "text/plain", "", b"wrong origin");
            }
            let key = req.headers.get("sec-websocket-key").ok_or("no websocket key")?;
            let mut sha = sha1::Sha1::new();
            sha.update(key.as_bytes());
            sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            let accept = base64(&sha.finalize());
            let head = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
            stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
            let from = who(&req, &stream);
            viewer(stream, token.unwrap_or_default(), from, gate, hands, view_only)
        }
        // A file from the page, into the folder for them here.
        ("POST", "/upload") => {
            let host = req.headers.get("host").cloned().unwrap_or_default();
            let origin = req.headers.get("origin").cloned().unwrap_or_default();
            if !signed_in || view_only {
                return respond(&mut stream, "403 Forbidden", "text/plain", "", b"sign in first");
            }
            if origin != format!("https://{host}") && origin != format!("http://{host}") {
                return respond(&mut stream, "403 Forbidden", "text/plain", "", b"wrong origin");
            }
            let Some(length) = req.headers.get("content-length").and_then(|v| v.parse::<u64>().ok()) else {
                return respond(&mut stream, "411 Length Required", "text/plain", "", b"how long is it?");
            };
            // The body comes slowly over a long way: more time for each piece of it.
            let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
            let q = req.path.split_once('?').map_or("", |(_, q)| q).to_owned();
            let session = session_of(token.as_deref().unwrap_or_default());
            let Some(mut body) = req.rest.take() else { return Err("bad request".into()) };
            match crate::remote_files::receive(&mut body, &q, length, &session) {
                Ok(saved) => respond(&mut stream, "200 OK", "text/plain; charset=utf-8", "", saved.as_bytes()),
                Err((status, why)) => {
                    println!("remote · a file from {} not saved: {why}", who(&req, &stream));
                    respond(&mut stream, status, "text/plain; charset=utf-8", "", why.as_bytes())
                }
            }
        }
        // Something offered here (copied in a file manager), downloaded there.
        ("GET", p) if p.starts_with("/file/") => {
            if !signed_in || view_only {
                return respond(&mut stream, "403 Forbidden", "text/plain", "", b"sign in first");
            }
            let session = session_of(token.as_deref().unwrap_or_default());
            let from = who(&req, &stream);
            match crate::remote_files::serve(&mut stream, &p["/file/".len()..], &session) {
                Ok(what) => {
                    println!("remote · {what} downloaded from {from}");
                    Ok(())
                }
                Err((status, why)) => respond(&mut stream, status, "text/plain; charset=utf-8", "", why.as_bytes()),
            }
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", "", b"not here"),
    }
}

/// A signed-in session as the files offered to it know it: its cookie's hash.
fn session_of(token: &str) -> String {
    hex(&sha2::Sha256::digest(token.as_bytes()))
}

fn form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (unescape(k), unescape(v)))
        .collect()
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'+' => out.push(b' '),
            b'%' if k + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[k + 1..k + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        k += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        k += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every couple of seconds, the session hears who is in (the last page to
/// arrive: which monitor it looks at, from where), or that no one is.
fn tell_the_session(gate: Arc<Mutex<Gate>>) {
    let mut last = String::new();
    let mut said = Instant::now();
    loop {
        let now = {
            let g = gate.lock().unwrap();
            g.present.iter().max_by_key(|(k, _)| **k).map(|(_, (m, who))| (*m, who.clone()))
        };
        let line = match &now {
            Some((m, who)) => format!("R {m} {}", hex(who.as_bytes())),
            None => "R off".to_owned(),
        };
        if line != last || (now.is_some() && said.elapsed() > Duration::from_secs(4)) {
            if crate::agent_cli::tell(&line).is_ok() {
                last = line;
                said = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(700));
    }
}

/// The session back at the desk once no page of the phone has been there
/// for a while; and told the phone is gone if the desk took it back.
fn keep_phone(gate: Arc<Mutex<Gate>>) {
    loop {
        std::thread::sleep(Duration::from_secs(2));
        // The phone as this computer's output while the session is on it.
        let on = gate.lock().unwrap().phone_on;
        crate::remote_rtc::phone_sink(on);
        let mut g = gate.lock().unwrap();
        if !g.phone_on {
            continue;
        }
        if g.phone_pages == 0 && g.phone_left.is_some_and(|t| t.elapsed() > PHONE_GRACE) {
            g.phone_on = false;
            drop(g);
            println!("remote · no phone for a while: the session goes back to the desk");
            let _ = crate::agent_cli::tell("P off");
        }
    }
}

/// Text as keys of the desk's own layout: each character, the key that
/// writes it (with Shift if it is on the second level). For the lock screen,
/// which is no window to type into otherwise.
fn type_keys(h: &mut Hands, text: &str) {
    static TABLE: std::sync::OnceLock<HashMap<char, (u16, bool)>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = HashMap::new();
        let Ok(km) = crate::session::keymap() else { return t };
        use smithay::input::keyboard::xkb;
        for level in [1u32, 0] {
            for code in km.min_keycode().raw()..=km.max_keycode().raw() {
                for sym in km.key_get_syms_by_level(code.into(), 0, level) {
                    if let Some(c) = char::from_u32(xkb::keysym_to_utf32(*sym)).filter(|c| *c != '\0') {
                        if code >= 8 {
                            t.insert(c, ((code - 8) as u16, level == 1));
                        }
                    }
                }
            }
        }
        t
    });
    for c in text.chars() {
        let (code, shift) = match c {
            '\n' => (28, false),
            _ => match table.get(&c) {
                Some(k) => *k,
                None => continue,
            },
        };
        if shift {
            h.key(42, true);
        }
        h.key(code, true);
        h.key(code, false);
        if shift {
            h.key(42, false);
        }
    }
}

/// Whether the phone's monitor is up (the desk may have taken the session back).
fn phone_index(monitors: &[(String, f64, f64, f64, f64)]) -> Option<usize> {
    monitors.iter().position(|m| m.0 == crate::layers::PHONE_NAME)
}

/// Where `stop` finds the server: one per port, so two never meet.
fn control_path(port: u16) -> String {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    format!("{dir}/pleamar-remote-{port}.sock")
}

/// `pleamar-wm remote send FILE…`: on the clipboard as files, which every
/// page connected is offered to download.
fn send(files: &[String]) -> Result<(), String> {
    if files.is_empty() {
        return Err("which files?".into());
    }
    let paths: Vec<std::path::PathBuf> = files.iter().map(|f| std::fs::canonicalize(f).map_err(|e| format!("{f}: {e}"))).collect::<Result<_, _>>()?;
    let mut child = std::process::Command::new("wl-copy").args(["-t", "text/uri-list"]).stdin(std::process::Stdio::piped()).spawn().map_err(|e| format!("wl-copy: {e}"))?;
    if let Some(mut input) = child.stdin.take() {
        input.write_all(crate::remote_files::uris_of_paths(&paths).as_bytes()).map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string())?;
    println!("offered to the page: {} (a «Download» there, for a quarter of an hour)", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
    Ok(())
}

/// `pleamar-wm remote stop`, from this computer: everyone out, now, and
/// every session forgotten (signing in again needs the password and a code).
fn control(gate: Arc<Mutex<Gate>>) {
    use std::os::unix::net::UnixListener;
    let path = control_path(gate.lock().unwrap().config.port);
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    for stream in listener.incoming().flatten() {
        let mut line = String::new();
        let _ = BufReader::new(&stream).read_line(&mut line);
        if line.trim() == "stop" {
            let mut g = gate.lock().unwrap();
            let pages = g.present.len();
            g.kicked += 1;
            g.sessions.clear();
            g.save();
            drop(g);
            notify(&format!("Remote desktop: sent away ({pages} connected), and every session ended"));
            let _ = (&stream).write_all(b"ok\n");
        } else if line.trim() == "leave" {
            // A newer one takes the port: the pages it has come back to it
            // (their sessions are kept on disk), the hands go with this process.
            println!("remote · a newer one takes over");
            let _ = (&stream).write_all(b"ok\n");
            std::process::exit(0);
        }
    }
}

/// Asks a `pleamar-wm remote` already serving this port to leave.
fn take_over(port: u16) {
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(control_path(port)) else { return };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream.write_all(b"leave\n").is_ok() {
        let mut reply = String::new();
        let _ = BufReader::new(&stream).read_line(&mut reply);
        println!("remote · the one before was told to leave");
    }
}

fn stop() -> Result<(), String> {
    let port = load()?.port;
    let mut stream = std::os::unix::net::UnixStream::connect(control_path(port)).map_err(|_| "no remote desktop running here".to_owned())?;
    stream.write_all(b"stop\n").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    let _ = BufReader::new(&stream).read_line(&mut reply);
    println!("Everyone using this desktop from elsewhere is out, and must sign in again.");
    Ok(())
}

fn notify(text: &str) {
    println!("remote · {text}");
    let _ = std::process::Command::new("notify-send").args(["-a", "pleamar", "-u", "critical", "Remote desktop", text]).spawn();
}

// ---------------------------------------------------------------- a viewer

enum Want {
    Frame { monitor: usize, whole: bool },
}

/// The encoders tried, in order: the card's (NVIDIA, then VAAPI: AMD,
/// Intel), and the processor's. Each with what makes it answer at once:
/// no frames held back to compare with later ones.
const ENCODERS: &[(&str, &[&str])] = &[
    ("h264_nvenc", &["preset=p1", "tune=ull", "zerolatency=1", "bf=0", "g=1800", "rc=vbr"]),
    ("libx264", &["preset=ultrafast", "tune=zerolatency", "bf=0", "g=1800"]),
];

/// A monitor as video: `pleamar-wm-stream` (ours, next to this program:
/// each change out as soon as it is encoded, told a new bitrate or to send a
/// whole frame while it runs), or else wf-recorder taking it 60 (or 30)
/// times a second into H.264, cut here into frames.
struct Video {
    child: std::process::Child,
    frames: mpsc::Receiver<(bool, Vec<u8>)>,
    /// The page fell behind and frames were dropped: start again from a whole one.
    behind: Arc<std::sync::atomic::AtomicBool>,
    /// Ours: what it is told while it runs.
    orders: Option<std::process::ChildStdin>,
}

impl Drop for Video {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Video {
    /// At that many kilobits a second (more in a burst: a page scrolled).
    fn start(monitor: &str, kbps: u32, fps: u32) -> Result<Video, String> {
        match Video::ours(monitor, kbps, fps) {
            Ok(v) => return Ok(v),
            Err(e) => println!("remote · {e}: wf-recorder instead"),
        }
        Video::recorder(monitor, kbps, fps)
    }

    fn ours(monitor: &str, kbps: u32, fps: u32) -> Result<Video, String> {
        use std::os::fd::AsRawFd;
        let path = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("pleamar-wm-stream"))).filter(|p| p.exists()).ok_or("no pleamar-wm-stream next to pleamar-wm")?;
        let mut child = std::process::Command::new(&path)
            .args([monitor, &fps.to_string(), &kbps.to_string()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(|e| format!("pleamar-wm-stream: {e}"))?;
        let out = child.stdout.take().ok_or("no output")?;
        let orders = child.stdin.take();
        let mut poll = libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut poll, 1, 4000) } > 0 && poll.revents & libc::POLLIN != 0;
        if !ready || child.try_wait().ok().flatten().is_some() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("pleamar-wm-stream did not start".into());
        }
        let (tx, rx) = mpsc::sync_channel(24);
        let behind = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let late = behind.clone();
        std::thread::spawn(move || read_frames(out, tx, late));
        Ok(Video { child, frames: rx, behind, orders })
    }

    /// A new bitrate, while it runs; false if this one cannot (start it again).
    fn rate(&mut self, kbps: u32, fps: u32) -> bool {
        self.orders.as_mut().is_some_and(|o| writeln!(o, "rate {kbps} {fps}").is_ok())
    }

    /// A whole frame next; false if this one cannot (start it again).
    fn whole(&mut self) -> bool {
        self.behind.store(false, std::sync::atomic::Ordering::Relaxed);
        self.orders.as_mut().is_some_and(|o| writeln!(o, "key").is_ok())
    }

    fn recorder(monitor: &str, kbps: u32, fps: u32) -> Result<Video, String> {
        use std::os::fd::AsRawFd;
        let mut why = String::new();
        for (codec, params) in ENCODERS {
            let fps = fps.to_string();
            let mut args: Vec<String> = ["-D", "--no-dmabuf", "-o", monitor, "-r", &fps, "-c", codec].iter().map(|v| v.to_string()).collect();
            let rate = [format!("b={kbps}k"), format!("maxrate={}k", kbps * 3 / 2), format!("bufsize={}k", kbps / 2)];
            for p in params.iter().map(|p| p.to_string()).chain(rate) {
                args.push("-p".into());
                args.push(p);
            }
            args.extend(["-m", "h264", "-f", "pipe:1"].iter().map(|v| v.to_string()));
            let mut child = match std::process::Command::new("wf-recorder").args(&args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() {
                Ok(c) => c,
                Err(e) => return Err(format!("wf-recorder: {e} (install wf-recorder)")),
            };
            let out = child.stdout.take().ok_or("no output")?;
            // The first frame within a few seconds, or the next encoder.
            let mut poll = libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let ready = unsafe { libc::poll(&mut poll, 1, 4000) } > 0 && poll.revents & libc::POLLIN != 0;
            if !ready || child.try_wait().ok().flatten().is_some() {
                let _ = child.kill();
                let _ = child.wait();
                why.push_str(&format!("{codec} did not start; "));
                continue;
            }
            println!("remote · {monitor} as video with {codec}, {kbps} kb/s, {fps} a second");
            let (tx, rx) = mpsc::sync_channel(24);
            let behind = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let late = behind.clone();
            std::thread::spawn(move || cut_frames(out, tx, late));
            return Ok(Video { child, frames: rx, behind, orders: None });
        }
        Err(format!("no encoder worked: {why}"))
    }
}

/// Ours, read as it comes: each frame says how long it is.
fn read_frames(mut out: std::process::ChildStdout, tx: mpsc::SyncSender<(bool, Vec<u8>)>, behind: Arc<std::sync::atomic::AtomicBool>) {
    let mut head = [0u8; 5];
    while out.read_exact(&mut head).is_ok() {
        let n = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let mut data = vec![0u8; n];
        if out.read_exact(&mut data).is_err() {
            return;
        }
        trace(&format!("encoded {n} bytes{}", if head[4] & 1 != 0 { " (key)" } else { "" }));
        match tx.try_send((head[4] & 1 != 0, data)) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => behind.store(true, std::sync::atomic::Ordering::Relaxed),
            Err(mpsc::TrySendError::Disconnected(_)) => return,
        }
    }
}

/// The stream, read as it comes: when it stops for a moment, what came is
/// whole frames (the encoder writes one at a time), each sent on.
fn cut_frames(out: std::process::ChildStdout, tx: mpsc::SyncSender<(bool, Vec<u8>)>, behind: Arc<std::sync::atomic::AtomicBool>) {
    use std::os::fd::AsRawFd;
    let fd = out.as_raw_fd();
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    let mut chunk = vec![0u8; 1 << 18];
    loop {
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let wait = if buf.is_empty() { 2000 } else { 3 };
        let n = unsafe { libc::poll(&mut poll, 1, wait) };
        if n > 0 {
            let got = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
            if got <= 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..got as usize]);
            continue;
        }
        if buf.is_empty() {
            continue;
        }
        for frame in access_units(&buf) {
            trace(&format!("encoded {} bytes{}", frame.1.len(), if frame.0 { " (key)" } else { "" }));
            match tx.try_send(frame) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => behind.store(true, std::sync::atomic::Ordering::Relaxed),
                Err(mpsc::TrySendError::Disconnected(_)) => return,
            }
        }
        buf.clear();
    }
    drop(out);
}

/// What went to a page as video lately, written every 10 s while it goes:
/// enough to see afterwards how it went (and what went wrong) without a trace.
#[derive(Default)]
struct Tally {
    frames: u32,
    keys: u32,
    bytes: usize,
    /// Whole frames asked (the page, the way, or this end fallen behind).
    wholes: u32,
    /// This end fell behind (frames dropped before sending).
    behind: u32,
    /// The direct way was full (a frame not sent).
    full: u32,
    since: Option<Instant>,
}

impl Tally {
    fn tell(&mut self, way: &str, flow: &Flow) {
        let since = *self.since.get_or_insert_with(Instant::now);
        let secs = since.elapsed().as_secs_f64();
        if secs < 10.0 {
            return;
        }
        if self.frames > 0 || self.wholes > 0 {
            let mut more = String::new();
            if self.behind > 0 {
                more.push_str(&format!(" · fell behind {}×", self.behind));
            }
            if self.full > 0 {
                more.push_str(&format!(" · way full {}×", self.full));
            }
            println!(
                "remote · {:.0} s by {way}: {} frames ({:.0} a second, {} whole, {} asked), {:.1} Mb/s of {:.1}{more}",
                secs,
                self.frames,
                self.frames as f64 / secs,
                self.keys,
                self.wholes,
                self.bytes as f64 * 8.0 / 1e6 / secs,
                flow.kbps as f64 / 1000.0,
            );
        }
        *self = Tally { since: Some(Instant::now()), ..Tally::default() };
    }
}

/// `PLEAMAR_REMOTE_TRACE=1`: each step of a key and a frame, with the time
/// (ms, monotonic), to see where the time goes between a key and its picture.
fn trace(what: &str) {
    static ON: std::sync::OnceLock<Option<Instant>> = std::sync::OnceLock::new();
    if let Some(t0) = ON.get_or_init(|| std::env::var_os("PLEAMAR_REMOTE_TRACE").map(|_| Instant::now())) {
        println!("trace {:.1} {what}", t0.elapsed().as_secs_f64() * 1000.0);
    }
}

/// H.264 (Annex B) cut into frames: a frame is the parameters and notes
/// before a picture, and the picture. Whether it is a whole one (key).
fn access_units(data: &[u8]) -> Vec<(bool, Vec<u8>)> {
    // Where each unit starts (after its 00 00 01) and its start code's start.
    let mut starts = Vec::new();
    let mut k = 0;
    while k + 3 <= data.len() {
        if data[k] == 0 && data[k + 1] == 0 && data[k + 2] == 1 {
            let code = if k > 0 && data[k - 1] == 0 { k - 1 } else { k };
            starts.push((code, k + 3));
            k += 3;
        } else {
            k += 1;
        }
    }
    let mut out: Vec<(bool, Vec<u8>)> = Vec::new();
    let mut current: Option<(usize, bool, bool)> = None; // from, has a picture, key
    for (n, (code, body)) in starts.iter().enumerate() {
        let kind = data.get(*body).map_or(0, |b| b & 0x1f);
        let picture = kind == 1 || kind == 5;
        if let Some((from, has, key)) = current {
            if has && (picture || matches!(kind, 6 | 7 | 8 | 9)) {
                out.push((key, data[from..*code].to_vec()));
                current = None;
            }
        }
        let c = current.get_or_insert((*code, false, false));
        c.1 |= picture;
        c.2 |= kind == 5;
        if n + 1 == starts.len() {
            out.push((c.2, data[c.0..].to_vec()));
        }
    }
    out
}

/// How the video is doing on the way: each frame the page says it has
/// shown (so a computer slow to decode counts as much as a slow network),
/// how long that took against how long it usually takes there and back,
/// and how much actually arrives each second.
struct Flow {
    seq: u32,
    on_the_way: std::collections::VecDeque<(u32, Instant, usize)>,
    /// Round trip, smoothed, and the usual one of this way (the least of
    /// late), in ms: above that, frames are waiting in line somewhere.
    rtt: f64,
    base: f64,
    base_at: Instant,
    /// What arrives, in kb/s (smoothed), and the bytes counted for it.
    arriving: f64,
    arrived: usize,
    kbps: u32,
    fps: u32,
    waiting: bool,
    /// A new start sends a whole frame (big): its time on the way is not trouble.
    grace: Instant,
    last_trouble: Instant,
    last_raise: Instant,
    last_stats: Instant,
}

enum Pace {
    Go,
    /// Too much on the way: send nothing until it arrives.
    Wait,
    /// Start again (a whole frame): lighter after trouble, or richer after calm.
    Again,
}

const KBPS_START: u32 = 8000;
// Down to what a slow way carries (a phone through somebody's relay brought
// 0.4–1.9 Mb/s): with a floor of 1500 it could only wait and start again.
const KBPS_MIN: u32 = 500;
const KBPS_MAX: u32 = 20000;

impl Flow {
    fn new() -> Flow {
        let now = Instant::now();
        Flow {
            seq: 0,
            on_the_way: Default::default(),
            rtt: 0.0,
            base: 0.0,
            base_at: now,
            arriving: 0.0,
            arrived: 0,
            kbps: KBPS_START,
            fps: 60,
            waiting: false,
            grace: now,
            last_trouble: now,
            last_raise: now,
            last_stats: now,
        }
    }

    fn sent(&mut self, bytes: usize) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        self.on_the_way.push_back((self.seq, Instant::now(), bytes));
        self.seq
    }

    fn got(&mut self, seq: u32) {
        while let Some((s, at, bytes)) = self.on_the_way.front().copied() {
            if s > seq {
                break;
            }
            self.on_the_way.pop_front();
            self.arrived += bytes;
            if s == seq {
                let ms = at.elapsed().as_secs_f64() * 1000.0;
                self.rtt = if self.rtt == 0.0 { ms } else { self.rtt * 0.85 + ms * 0.15 };
                if self.base == 0.0 || ms < self.base {
                    self.base = ms;
                    self.base_at = Instant::now();
                }
            }
        }
        // The way may have become longer for good: what is usual follows, slowly.
        if self.base_at.elapsed() > Duration::from_secs(20) {
            self.base = (self.base * 1.2).min(self.rtt.max(self.base));
            self.base_at = Instant::now();
        }
    }

    fn state(&mut self) -> Pace {
        let now = Instant::now();
        let oldest = self.on_the_way.front().map_or(Duration::ZERO, |(_, at, _)| at.elapsed());
        if self.waiting {
            // Arrived (or lost for good): again, at what actually arrives.
            if self.on_the_way.is_empty() || oldest > Duration::from_secs(4) {
                self.waiting = false;
                // What arrived says what fits only if the way was full: a
                // still screen sends little, and that is no measure of it.
                let full = self.arriving > self.kbps as f64 * 0.5;
                let fits = if full { (self.arriving * 0.85) as u32 } else { self.kbps };
                self.kbps = fits.min(self.kbps * 85 / 100).max(KBPS_MIN);
                // Little room: fewer frames, each sharper.
                if self.kbps < 1500 {
                    self.fps = 15;
                } else if self.kbps < 4000 {
                    self.fps = self.fps.min(30);
                }
                return Pace::Again;
            }
            return Pace::Wait;
        }
        let limit = Duration::from_millis((self.base * 2.0 + 250.0).max(400.0) as u64);
        if now > self.grace && oldest > limit {
            println!(
                "remote · {} ms on the way (usually {:.0}, now {:.0}), {} frames, arriving {:.0} kb/s of {}: lighter",
                oldest.as_millis(),
                self.base,
                self.rtt,
                self.on_the_way.len(),
                self.arriving,
                self.kbps
            );
            self.waiting = true;
            self.last_trouble = now;
            return Pace::Wait;
        }
        // A while with nothing waiting in line: a little more.
        let calm = Duration::from_secs(20);
        let free = self.rtt > 0.0 && self.rtt < self.base * 1.5 + 60.0;
        if (self.kbps < KBPS_MAX || self.fps < 60) && free && self.last_trouble.elapsed() > calm && self.last_raise.elapsed() > calm {
            self.last_raise = now;
            if self.fps < 60 && self.kbps >= 4000 {
                self.fps = 60;
            } else if self.fps < 30 && self.kbps >= 1500 {
                self.fps = 30;
            } else {
                self.kbps = (self.kbps * 5 / 4).min(KBPS_MAX);
            }
            return Pace::Again;
        }
        Pace::Go
    }

    fn restarted(&mut self) {
        self.on_the_way.clear();
        self.grace = Instant::now() + Duration::from_millis(2500);
    }

    /// Once a second: what arrived in it, and time to tell the page.
    fn stats_due(&mut self) -> bool {
        let elapsed = self.last_stats.elapsed();
        if elapsed < Duration::from_secs(1) {
            return false;
        }
        let kbps = self.arrived as f64 * 8.0 / 1000.0 / elapsed.as_secs_f64();
        self.arriving = if self.arriving == 0.0 { kbps } else { self.arriving * 0.7 + kbps * 0.3 };
        self.arrived = 0;
        self.last_stats = Instant::now();
        true
    }
}

/// One connected page: what it says goes to the hands; the picture comes
/// as video (or, for a browser without a video decoder, as squares of JPEG
/// each time the page has painted the last ones).
fn viewer(stream: TcpStream, token: String, from: String, gate: &Arc<Mutex<Gate>>, hands: &Arc<Mutex<Option<Hands>>>, view_only: bool) -> Result<(), String> {
    // In the list of who is in, while this lasts, and out of it however it ends.
    let (page, kicked) = {
        let mut g = gate.lock().unwrap();
        g.next_page += 1;
        let page = g.next_page;
        g.present.insert(page, (0, from.clone()));
        (page, g.kicked)
    };
    struct Leave<'a>(&'a Arc<Mutex<Gate>>, u64);
    impl Drop for Leave<'_> {
        fn drop(&mut self) {
            self.0.lock().unwrap().present.remove(&self.1);
        }
    }
    let _leave = Leave(gate, page);
    stream.set_read_timeout(Some(Duration::from_millis(4))).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    let mut ws = WebSocket::from_raw_socket(stream, Role::Server, None);
    if !view_only {
        let mut h = hands.lock().unwrap();
        if h.is_none() {
            *h = Some(Hands::new()?);
        }
    }
    // The clipboard here, watched while this page is in: what it held when
    // the page came is not sent (it would replace yours on connecting).
    let clip_watch = ClipWatch::start();
    let mut clip_known = wl_paste();
    let mut files_known: Vec<std::path::PathBuf> = Vec::new();
    let mut monitors = crate::agent_cli::monitors()?;
    let mons = |monitors: &[(String, f64, f64, f64, f64)]| {
        let mut list = String::from("mons");
        for (name, x, y, w, h) in monitors {
            list.push_str(&format!(" {name},{x},{y},{w},{h}"));
        }
        list
    };
    ws.send(Message::Text(format!("version {PAGE_VERSION:016x}").into())).map_err(|e| e.to_string())?;
    ws.send(Message::Text(mons(&monitors).into())).map_err(|e| e.to_string())?;
    // The session still on the phone (its page closed by accident): a page of
    // the phone takes it back by itself; at the desk, it offers to bring it.
    let waits = gate.lock().unwrap().phone_on;
    ws.send(Message::Text(if waits { "phonewaits" } else { "deskhas" }.into())).map_err(|e| e.to_string())?;

    let (mut want_tx, want_rx) = mpsc::channel::<Want>();
    let (frame_tx, mut frame_rx) = mpsc::channel::<Result<(usize, u32, u32, Vec<Vec<u8>>), String>>();
    let names: Vec<String> = monitors.iter().map(|m| m.0.clone()).collect();
    std::thread::spawn(move || pictures(names, want_rx, frame_tx));
    // A page on a phone, with the session on it (docs/phone.md).
    let mut on_phone = false;
    let mut parked = false;
    // Asked for while the session is locked: the desk is shown to unlock it,
    // and the phone is asked for again until it comes.
    let mut unlocking: Option<String> = None;
    let mut unlock_tried = Instant::now();
    let mut phone_checked = Instant::now();
    struct PhoneLeave<'a>(&'a Arc<Mutex<Gate>>, std::rc::Rc<std::cell::Cell<bool>>);
    impl Drop for PhoneLeave<'_> {
        fn drop(&mut self) {
            if self.1.get() {
                let mut g = self.0.lock().unwrap();
                g.phone_pages = g.phone_pages.saturating_sub(1);
                g.phone_left = Some(Instant::now());
            }
        }
    }
    let phone_flag = std::rc::Rc::new(std::cell::Cell::new(false));
    let _phone_leave = PhoneLeave(gate, phone_flag.clone());
    // `parked`, for the direct way: no sound while it is.
    let hush = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut monitor = 0usize;
    let mut flow = Flow::new();
    let mut video_wanted = false;
    let mut video: Option<Video> = None;
    let mut restart = false;
    // Why it starts (again), for the log.
    let mut why = String::new();
    // What went to the page lately, told every 10 s (see Tally).
    let mut tally = Tally::default();
    // What the page itself says (its errors, its decoder…), at most so many a minute.
    let (mut page_lines, mut page_minute) = (0u32, Instant::now());
    // Without starting again (ours can): a whole frame next, a new bitrate.
    let mut whole = false;
    let mut retune = false;
    let mut video_started = Instant::now();
    let mut size = (1u32, 1u32);
    let mut waiting = false;
    let mut pending: Option<bool> = None;
    let mut last_ask = Instant::now() - Duration::from_secs(1);
    let mut checked = Instant::now();
    // The direct way (WebRTC), once the page has offered it and it opened.
    let mut peer: Option<crate::remote_rtc::Peer> = None;
    let mut direct = false;
    let mut hands_direct = false;
    let mut last_rate = Instant::now();
    // What the direct way is actually carrying, in kb/s (each second).
    let mut sending_kbps = 0.0f64;
    let (mut sent_bytes, mut sent_since) = (0usize, Instant::now());
    let mut low_since: Option<Instant> = None;
    // The frames' channel: since when too much waits in it, and when it last did.
    let mut high_since: Option<Instant> = None;
    let mut channel_trouble = Instant::now();
    // How long the frames wait on the way, as the page measures it (`q`), and
    // since when it is too long.
    let (mut waits, mut waits_at, mut waits_since) = (0.0f64, Instant::now() - Duration::from_secs(60), None::<Instant>);
    // A frame was lost on its way out: none but a whole one may follow (the
    // others would be decoded against what the page never had).
    let mut gap = false;
    // The encoder could not start on the phone's monitor (it was going): again in a moment.
    let mut retry: Option<Instant> = None;
    let mut used = Instant::now();
    // (`PLEAMAR_REMOTE_UNUSED`, in seconds: to check it without waiting.)
    let unused = std::env::var("PLEAMAR_REMOTE_UNUSED").ok().and_then(|v| v.parse().ok()).map_or(UNUSED, Duration::from_secs);
    let mut told_newer = false;
    loop {
        // Sent away from this computer: at once.
        if gate.lock().unwrap().kicked != kicked {
            let _ = ws.send(Message::Text("bye".into()));
            break;
        }
        // A newer program installed: the page offers to start it.
        if !told_newer && gate.lock().unwrap().newer {
            told_newer = true;
            let _ = ws.send(Message::Text("newer".into()));
        }
        // Nobody's hands for a long while: signed out (see UNUSED).
        if used.elapsed() > unused {
            gate.lock().unwrap().sign_out(&token);
            println!("remote · signed out: half an hour without a mouse or a key from there");
            let _ = ws.send(Message::Text("bye unused".into()));
            break;
        }
        // Every so often: the session is still good (not signed out elsewhere).
        if checked.elapsed() > Duration::from_secs(30) {
            checked = Instant::now();
            if !gate.lock().unwrap().valid(&token) {
                let _ = ws.send(Message::Text("bye".into()));
                break;
            }
        }
        // What the page says, by its socket or by the direct way.
        let mut lines: Vec<String> = Vec::new();
        match ws.read() {
            Ok(Message::Text(t)) => lines.push(t.to_string()),
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        if let Some(ask) = unlocking.as_ref().filter(|_| unlock_tried.elapsed() > Duration::from_millis(1500)) {
            unlock_tried = Instant::now();
            lines.push(ask.clone());
        }
        // Something copied here, by any means: to your clipboard too. Files
        // (copied in a file manager) are offered to download instead.
        if clip_watch.as_ref().is_some_and(|w| w.changed.try_iter().count() > 0) && !view_only {
            let kinds = std::process::Command::new("wl-paste").arg("-l").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            if kinds.lines().any(|k| k.trim() == "text/uri-list") {
                let list = std::process::Command::new("wl-paste").args(["-n", "-t", "text/uri-list"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
                let paths = crate::remote_files::paths_of_uris(&list);
                if !paths.is_empty() && paths != files_known {
                    files_known = paths.clone();
                    let (t, count, bytes, name) = crate::remote_files::offer(paths, &session_of(&token), hex(&random(32)));
                    println!("remote · offered to {from}: {name} ({count} files, {})", crate::remote_files::size(bytes));
                    let _ = ws.send(Message::Text(format!("files {t} {count} {bytes} {name}").into()));
                }
            } else if let Some(text) = wl_paste().filter(|t| clip_known.as_ref() != Some(t) && t.len() <= CLIP_MOST) {
                clip_known = Some(text.clone());
                let _ = ws.send(Message::Text(format!("clipped {text}").into()));
            }
        }
        let mut gone = false;
        while let Some(event) = peer.as_ref().and_then(|p| p.events.try_recv().ok()) {
            use crate::remote_rtc::PeerEvent;
            match event {
                PeerEvent::Line(l) => {
                    if !hands_direct {
                        hands_direct = true;
                        println!("remote · the hands come by the direct way");
                    }
                    lines.push(l);
                }
                PeerEvent::Connected => {
                    // The picture goes this way now, from a whole frame.
                    direct = true;
                    whole = true;
                    println!("remote · the direct way is open");
                    let _ = ws.send(Message::Text("direct".into()));
                }
                PeerEvent::WholeFrame => whole = true,
                PeerEvent::Estimate(k) if direct => {
                    // What the way takes. Less only when it says so for a
                    // while AND it was being filled: the estimate only grows
                    // with what is sent, so a still screen (which sends next
                    // to nothing) made it fall, and the first scroll after it
                    // came blurred and at 30 a second. More when it says so.
                    let k = k.clamp(KBPS_MIN, KBPS_MAX);
                    let filled = sending_kbps > k as f64 * 0.6;
                    if k < flow.kbps * 7 / 10 && filled {
                        let since = *low_since.get_or_insert_with(Instant::now);
                        if since.elapsed() > Duration::from_secs(3) {
                            low_since = None;
                            flow.kbps = (k * 9 / 10).max(KBPS_MIN);
                            // Fewer frames only on a way really poor.
                            flow.fps = if k < KBPS_MIN * 3 / 2 { 30 } else { 60 };
                            last_rate = Instant::now();
                            retune = true;
                            println!("remote · the direct way takes less ({k} kb/s, {sending_kbps:.0} sent): {} kb/s, {} a second", flow.kbps, flow.fps);
                        }
                    } else {
                        if !filled || k >= flow.kbps * 7 / 10 {
                            low_since = None;
                        }
                        if k > flow.kbps * 13 / 10 && last_rate.elapsed() > Duration::from_secs(5) {
                            flow.kbps = (k * 9 / 10).min(KBPS_MAX);
                            flow.fps = 60;
                            last_rate = Instant::now();
                            retune = true;
                            println!("remote · the direct way takes more ({k} kb/s): {} kb/s, {} a second", flow.kbps, flow.fps);
                        }
                    }
                }
                PeerEvent::Estimate(_) => {}
                PeerEvent::Backlog(bytes) if direct => {
                    // The frames' own channel: what waits in it says how the
                    // way is doing. Over 300 ms of it for a while: less (it
                    // would only grow, and every frame come later). Little
                    // waiting while it carries plenty, or calm for long: more.
                    let ms = bytes as f64 * 8.0 / flow.kbps as f64;
                    if ms > 300.0 {
                        let since = *high_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= Duration::from_millis(500) && last_rate.elapsed() > Duration::from_secs(1) {
                            high_since = None;
                            flow.kbps = (flow.kbps * 7 / 10).max(KBPS_MIN);
                            flow.fps = if flow.kbps < KBPS_MIN * 3 / 2 { 30 } else { 60 };
                            last_rate = Instant::now();
                            channel_trouble = Instant::now();
                            retune = true;
                            println!("remote · {ms:.0} ms waiting on the frames' channel: {} kb/s, {} a second", flow.kbps, flow.fps);
                        }
                    } else {
                        high_since = None;
                        // More only while it carries plenty and the frames do not
                        // wait on the way (a still screen proves nothing: the
                        // first scroll at a rate the wifi cannot take came late).
                        let busy = sending_kbps > flow.kbps as f64 * 0.4;
                        let quick = waits_at.elapsed() < Duration::from_secs(1) && waits < 25.0;
                        let calm = channel_trouble.elapsed() > Duration::from_secs(4);
                        // (In few steps: each new bitrate costs a whole frame, the
                        // encoder's own doing.)
                        if ms < 60.0 && busy && quick && calm && flow.kbps < KBPS_MAX && last_rate.elapsed() > Duration::from_secs(3) {
                            flow.kbps = (flow.kbps * 3 / 2).min(KBPS_MAX);
                            flow.fps = 60;
                            last_rate = Instant::now();
                            retune = true;
                            println!("remote · the frames' channel keeps up: {} kb/s, {} a second", flow.kbps, flow.fps);
                        }
                    }
                }
                PeerEvent::Backlog(_) => {}
                PeerEvent::Gone => gone = true,
            }
        }
        if gone {
            // Back by the socket.
            peer = None;
            if direct {
                direct = false;
                whole = true;
                println!("remote · the direct way closed: by the socket again");
                let _ = ws.send(Message::Text("indirect".into()));
            }
        }
        for t in lines {
            {
                let mut f = t.splitn(2, ' ');
                let (verb, rest) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
                let n: Vec<f64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                let mut guard = hands.lock().unwrap();
                // Only watching: the hands are not there, and what would use them is let go.
                let mut idle = None;
                let h = match guard.as_mut() {
                    Some(h) if !view_only => h,
                    _ => idle.insert(Hands::none()),
                };
                if matches!(verb, "m" | "b" | "w" | "s" | "se" | "sh" | "k" | "paste" | "copy" | "mon" | "phone" | "unphone" | "g" | "type") {
                    used = Instant::now();
                }
                match (verb, &n[..]) {
                    // Asked to start the newer program installed.
                    ("update", _) if !view_only && gate.lock().unwrap().newer => {
                        drop(guard);
                        again();
                        break;
                    }
                    // A phone: the session comes to a monitor of its size.
                    ("phone", [w, hh, scale]) if !view_only => {
                        // Already there and turned on its side: it takes the new
                        // shape in its place (its size, until then, the old one).
                        let before = phone_index(&monitors).filter(|_| on_phone).map(|i| (monitors[i].3, monitors[i].4));
                        let asked = crate::agent_cli::tell(&format!("P {} {} {:.3}", *w as u32, *hh as u32, scale));
                        // Up within a moment (the scene's copies are given again).
                        let mut k = None;
                        for tries in 0..30 {
                            if let Ok(m) = crate::agent_cli::monitors() {
                                if let Some(i) = phone_index(&m).filter(|&i| before.is_none_or(|b| b != (m[i].3, m[i].4)) || tries >= 15) {
                                    k = Some(i);
                                    monitors = m;
                                    break;
                                }
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        match (asked, k) {
                            (Err(e), _) if e.contains("locked") => {
                                // Shown the desk —its lock screen— to unlock it from here.
                                if unlocking.is_none() {
                                    println!("remote · the session is locked at the desk: unlocked from the phone first");
                                    let _ = ws.send(Message::Text("phonelocked".into()));
                                    monitor = 0;
                                    parked = false;
                                    if video_wanted {
                                        restart = true;
                                        why = "the desk's lock screen".into();
                                    } else {
                                        pending = Some(true);
                                        waiting = false;
                                    }
                                }
                                unlocking = Some(t.clone());
                            }
                            (Ok(_), Some(k)) => {
                                unlocking = None;
                                if !on_phone {
                                    on_phone = true;
                                    phone_flag.set(true);
                                    let mut g = gate.lock().unwrap();
                                    g.phone_on = true;
                                    g.phone_pages += 1;
                                    g.phone_left = None;
                                    drop(g);
                                    // Its output at once (not at keep_phone's next look).
                                    std::thread::spawn(|| crate::remote_rtc::phone_sink(true));
                                }
                                println!("remote · the session goes to the phone ({}×{} at {:.2})", *w as u32, *hh as u32, scale);
                                let _ = ws.send(Message::Text(mons(&monitors).into()));
                                let _ = ws.send(Message::Text(format!("phoneon {k}").into()));
                                // The pictures, of the new row of monitors.
                                let (wt, wr) = mpsc::channel::<Want>();
                                let (ft, fr) = mpsc::channel();
                                let names: Vec<String> = monitors.iter().map(|m| m.0.clone()).collect();
                                std::thread::spawn(move || pictures(names, wr, ft));
                                want_tx = wt;
                                frame_rx = fr;
                                monitor = k;
                                if let Some(p) = gate.lock().unwrap().present.get_mut(&page) {
                                    p.0 = monitor;
                                }
                                phone_checked = Instant::now();
                                parked = false;
                                if video_wanted {
                                    restart = true;
                                    why = "the phone's monitor".into();
                                } else {
                                    pending = Some(true);
                                    waiting = false;
                                }
                            }
                            (Err(e), _) => {
                                let _ = ws.send(Message::Text(format!("nophone {e}").into()));
                            }
                            (_, None) => {
                                let _ = ws.send(Message::Text("nophone the session did not put up the phone's monitor".into()));
                            }
                        }
                    }
                    // Given back from the phone: the session goes to the desk now.
                    ("unphone", _) if on_phone => {
                        on_phone = false;
                        phone_flag.set(false);
                        parked = true;
                        video = None;
                        {
                            let mut g = gate.lock().unwrap();
                            g.phone_pages = g.phone_pages.saturating_sub(1);
                            g.phone_on = g.phone_pages > 0;
                        }
                        println!("remote · the phone gives the session back to the desk");
                        let _ = crate::agent_cli::tell("P off");
                    }
                    // A gesture from the phone's bottom edge.
                    ("g", _) if on_phone && matches!(rest.trim(), "cards" | "next" | "prev") => {
                        let _ = crate::agent_cli::tell(&format!("E phone_{}", rest.trim()));
                    }
                    // What the phone's keyboard writes.
                    // Typing at the lock screen (no window to type into): its
                    // keys, as the desk's own layout has them.
                    ("type", _) if !on_phone && unlocking.is_some() && !rest.is_empty() => type_keys(h, rest),
                    ("type", _) if on_phone && !rest.is_empty() => {
                        if let Err(e) = crate::agent_cli::tell(&format!("U {}", hex(rest.as_bytes()))) {
                            println!("remote · the phone's text did not go: {e}");
                        }
                    }
                    ("hello", _) => {
                        video_wanted = rest.trim() == "video";
                        // (Already started for the phone a moment ago: that one goes on.)
                        if video_wanted && video.is_none() {
                            restart = true;
                            why = "the page says hello".into();
                        } else if video_wanted {
                            whole = true;
                        } else {
                            pending = Some(true);
                        }
                    }
                    ("got", [seq]) => flow.got(*seq as u32),
                    // How long the frames wait on the way now (ms over the
                    // way's own time): over 80 for half a second, less.
                    // Ten seconds or more is no line on the way but a clock
                    // that changed under the page (an older page, a way that
                    // started again): not believed.
                    ("q", [ms]) if direct && *ms < 10_000.0 => {
                        waits = *ms;
                        waits_at = Instant::now();
                        if waits > 80.0 {
                            let since = *waits_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= Duration::from_millis(500) && last_rate.elapsed() > Duration::from_secs(1) && flow.kbps > KBPS_MIN {
                                waits_since = None;
                                flow.kbps = (flow.kbps * 7 / 10).max(KBPS_MIN);
                                flow.fps = if flow.kbps < KBPS_MIN * 3 / 2 { 30 } else { 60 };
                                last_rate = Instant::now();
                                channel_trouble = Instant::now();
                                retune = true;
                                println!("remote · frames wait {waits:.0} ms on the way: {} kb/s, {} a second", flow.kbps, flow.fps);
                            }
                        } else {
                            waits_since = None;
                        }
                    }
                    // What happens in the page (its errors, its decoder, its way), in this log.
                    ("log", _) => {
                        if page_minute.elapsed() > Duration::from_secs(60) {
                            page_minute = Instant::now();
                            page_lines = 0;
                        }
                        page_lines += 1;
                        if page_lines <= 60 {
                            let clean: String = rest.chars().map(|c| if c.is_control() { ' ' } else { c }).take(400).collect();
                            println!("remote · page · {clean}");
                        } else if page_lines == 61 {
                            println!("remote · page · (more this minute, not written)");
                        }
                    }
                    ("rtc", _) => {
                        // The page offers the direct way (again): by the socket until it opens.
                        if direct {
                            direct = false;
                            whole = true;
                            let _ = ws.send(Message::Text("indirect".into()));
                        }
                        match crate::remote_rtc::answer(rest, flow.kbps, hush.clone()) {
                            Ok((sdp, p)) => {
                                peer = Some(p);
                                let _ = ws.send(Message::Text(format!("rtcanswer {sdp}").into()));
                            }
                            Err(e) => {
                                println!("remote · no direct way: {e}");
                                let _ = ws.send(Message::Text("rtcno".into()));
                            }
                        }
                    }
                    ("ping", _) => {
                        let _ = ws.send(Message::Text(format!("pong {rest}").into()));
                    }
                    ("ack", _) => {
                        waiting = false;
                        if pending.is_none() {
                            pending = Some(false);
                        }
                    }
                    ("mon", [k]) if (*k as usize) < monitors.len() => {
                        let same = monitor == *k as usize && (video.is_some() || restart);
                        monitor = *k as usize;
                        if let Some(p) = gate.lock().unwrap().present.get_mut(&page) {
                            p.0 = monitor;
                        }
                        if video_wanted {
                            // The same one, already coming: nothing to start again.
                            if !same {
                                restart = true;
                                why = format!("monitor {monitor} asked");
                            }
                        } else {
                            pending = Some(true);
                            waiting = false;
                        }
                    }
                    ("full", _) => {
                        if video_wanted {
                            whole = true;
                        } else {
                            pending = Some(true);
                            waiting = false;
                        }
                    }
                    ("m", [fx, fy]) => {
                        // A point of the monitor shown, as a fraction of it.
                        let m = &monitors[monitor];
                        h.point(&monitors, m.1 + fx.clamp(0.0, 1.0) * m.3, m.2 + fy.clamp(0.0, 1.0) * m.4);
                    }
                    ("b", [b, d]) => h.button(*b as u32, *d != 0.0),
                    ("w", [dx, dy]) => h.wheel(*dx, *dy),
                    // A finger on the phone's glass: so far, lifted at that
                    // speed, down again.
                    ("s", [x, y]) => h.finger(Finger::Moved(*x, *y)),
                    ("se", [vx, vy]) => h.finger(Finger::Lifted(*vx, *vy)),
                    ("sh", _) => h.finger(Finger::Halt),
                    ("k", [c, d]) => {
                        trace(&format!("key {c} {d}"));
                        h.key(*c as u16, *d != 0.0)
                    }
                    ("rel", _) => h.release(),
                    ("paste", _) if !view_only => {
                        // `S` or `-` (Shift held with it: a terminal's
                        // Ctrl+Shift+V), then your text, if your clipboard
                        // changed since both held the same. Without it, the
                        // one here is newer —copied here with the mouse— or
                        // holds what a text cannot (a picture, files): kept.
                        // Then Ctrl+V pressed from here, right after: pressed
                        // from the page once this answered, the Ctrl had often
                        // been let go on the way —the keys come by the direct
                        // way, this by the socket— and nothing was pasted.
                        let shift = rest.starts_with('S');
                        let ok = match rest.get(2..) {
                            Some(text) => {
                                clip_known = Some(text.to_owned());
                                wl_copy(text)
                            }
                            None => true,
                        };
                        if ok {
                            h.paste_keys(shift);
                        }
                        let _ = ws.send(Message::Text(if ok { "pasted".into() } else { "nopaste".into() }));
                    }
                    // The files of a sending have all come: say so here, and
                    // if they were dropped on a window, give it them there.
                    ("received", _) if !view_only => {
                        let mut f = rest.split_whitespace();
                        let batch = f.next().unwrap_or_default();
                        let at: Vec<f64> = f.filter_map(|v| v.parse().ok()).collect();
                        if let Some((saved, files, bytes, took)) = crate::remote_files::finished(batch, &session_of(&token)) {
                            let inbox = crate::remote_files::inbox();
                            let what = match (&saved[..], files) {
                                ([one], 1) => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                (_, n) => format!("{n} files"),
                            };
                            let _ = std::process::Command::new("notify-send").args(["-a", "pleamar", "Remote desktop", &format!("{what} ({}) from {from}, in {}", crate::remote_files::size(bytes), inbox.display())]).spawn();
                            println!("remote · received {what} ({}) from {from} in {:.1} s", crate::remote_files::size(bytes), took.as_secs_f64());
                            // Only a moment after it was let go: a long sending later
                            // dropped on whatever is there by then would surprise.
                            let mut dropped = false;
                            if let ([m, fx, fy], false, true) = (&at[..], saved.is_empty(), took < Duration::from_secs(20))
                                && let Some(mm) = monitors.get(*m as usize)
                            {
                                h.point(&monitors, mm.1 + fx.clamp(0.0, 1.0) * mm.3, mm.2 + fy.clamp(0.0, 1.0) * mm.4);
                                // The session finds the window under it first.
                                std::thread::sleep(Duration::from_millis(150));
                                let uris = crate::remote_files::uris_of_paths(&saved);
                                dropped = crate::agent_cli::tell(&format!("D {}", hex(uris.as_bytes()))).is_ok_and(|r| r.starts_with("ok"));
                            }
                            let _ = ws.send(Message::Text(format!("saved {files} {} {}", dropped as u8, inbox.display()).into()));
                        }
                    }
                    ("copy", _) if !view_only => {
                        let text = wl_paste().unwrap_or_default();
                        clip_known = Some(text.clone());
                        let _ = ws.send(Message::Text(format!("clip {text}").into()));
                    }
                    _ => {}
                }
            }
        }

        // The desk took the session back: the phone is told, and nothing more is sent.
        if on_phone && phone_checked.elapsed() > Duration::from_secs(1) {
            phone_checked = Instant::now();
            if let Ok(m) = crate::agent_cli::monitors() {
                if phone_index(&m).is_none() {
                    on_phone = false;
                    phone_flag.set(false);
                    {
                        let mut g = gate.lock().unwrap();
                        g.phone_pages = g.phone_pages.saturating_sub(1);
                        g.phone_on = g.phone_pages > 0;
                    }
                    println!("remote · the session went back to the desk");
                    video = None;
                    parked = true;
                    monitors = m;
                    monitor = 0;
                    let _ = ws.send(Message::Text(mons(&monitors).into()));
                    let _ = ws.send(Message::Text("deskback".into()));
                }
            }
        }
        // Parked (the desk has the session): nothing to send until the phone asks again.
        hush.store(parked, std::sync::atomic::Ordering::SeqCst);
        if parked {
            continue;
        }
        // As video: frames as they come; fallen behind, from a whole one again.
        if video_wanted {
            if video.as_ref().is_some_and(|v| v.behind.load(std::sync::atomic::Ordering::Relaxed)) {
                if !whole {
                    tally.behind += 1;
                }
                whole = true;
                gap = true;
            }
            // Its monitor went (a phone turned: put up again in its new shape),
            // or it stopped: again, for the monitor there is now.
            if let Some(end) = video.as_mut().and_then(|v| v.child.try_wait().ok().flatten()).filter(|_| video_started.elapsed() > Duration::from_millis(700)) {
                restart = true;
                why = format!("the encoder stopped ({end})");
            }
            if retry.is_some_and(|t| Instant::now() >= t) && video.is_none() && !restart {
                retry = None;
                restart = true;
                why = "trying again".into();
            }
            // The way there filling up (a slower moment of the network, the
            // buffers of whoever is in between): stop sending until it has
            // emptied, and go on with less. (The direct way measures itself.)
            if !direct {
                match flow.state() {
                    Pace::Go => {}
                    Pace::Wait => {
                        while let Some(Ok(_)) = video.as_ref().map(|v| v.frames.try_recv()) {}
                        continue;
                    }
                    // (What waited was dropped: a whole frame too.)
                    Pace::Again => {
                        retune = true;
                        whole = true;
                    }
                }
            }
            if flow.stats_due() {
                let _ = ws.send(Message::Text(format!("stats {} {} {}", flow.rtt.round(), flow.kbps, flow.fps).into()));
            }
            // Told to the one running, if it can be; started again if not.
            if !restart && video.is_some() {
                if std::mem::take(&mut retune) && !video.as_mut().is_some_and(|v| v.rate(flow.kbps, flow.fps)) {
                    restart = true;
                    why = format!("{} kb/s, {} a second", flow.kbps, flow.fps);
                }
                if std::mem::take(&mut whole) {
                    tally.wholes += 1;
                    if video.as_mut().is_some_and(|v| v.whole()) {
                        flow.restarted();
                    } else {
                        restart = true;
                        why = "a whole frame".into();
                    }
                }
            }
            if restart {
                println!("remote · the video {}: {}", if video.is_some() { "starts again" } else { "starts" }, if why.is_empty() { "asked" } else { &why });
                why.clear();
                restart = false;
                retune = false;
                whole = false;
                video = None;
                flow.restarted();
                video_started = Instant::now();
                match Video::start(&monitors[monitor].0, flow.kbps, flow.fps) {
                    Ok(v) => {
                        video = Some(v);
                        let _ = ws.send(Message::Text(format!("video {monitor}").into()));
                    }
                    // On the phone, its monitor going or coming (the desk took
                    // it, it turned): not pictures for good, the video again in
                    // a moment (or parked, when the desk has it).
                    Err(e) if on_phone => {
                        println!("remote · no video for now: {e}");
                        retry = Some(Instant::now() + Duration::from_secs(1));
                    }
                    Err(e) => {
                        println!("remote · no video: {e}");
                        // No video here: squares of JPEG, as for an old browser.
                        let _ = ws.send(Message::Text(format!("novideo {e}").into()));
                        video_wanted = false;
                        pending = Some(true);
                    }
                }
            }
            if sent_since.elapsed() >= Duration::from_secs(1) {
                sending_kbps = sent_bytes as f64 * 8.0 / 1000.0 / sent_since.elapsed().as_secs_f64();
                sent_bytes = 0;
                sent_since = Instant::now();
            }
            let mut sent = false;
            while let Some(Ok((key, data))) = video.as_ref().map(|v| v.frames.try_recv()) {
                tally.frames += 1;
                tally.keys += key as u32;
                tally.bytes += data.len();
                if direct {
                    if gap && !key {
                        continue;
                    }
                    gap = false;
                    sent_bytes += data.len();
                    if let Some(p) = &peer {
                        if !p.send(key, data) {
                            // The way is full: from a whole frame again.
                            tally.full += 1;
                            whole = true;
                            gap = true;
                        }
                    }
                    continue;
                }
                let seq = flow.sent(data.len());
                let mut message = Vec::with_capacity(data.len() + 6);
                message.extend_from_slice(&[2, key as u8]);
                message.extend_from_slice(&seq.to_be_bytes());
                message.extend_from_slice(&data);
                ws.write(Message::Binary(message.into())).map_err(|e| e.to_string())?;
                sent = true;
            }
            if sent {
                ws.flush().map_err(|e| e.to_string())?;
            }
            tally.tell(if direct { "the direct way" } else { "the socket" }, &flow);
            continue;
        }

        // As squares: a new batch when the page painted the last one, at most ~15 a second.
        if !waiting && pending.is_some() && last_ask.elapsed() > Duration::from_millis(66) {
            let whole = pending.take().unwrap_or(false);
            let _ = want_tx.send(Want::Frame { monitor, whole });
            last_ask = Instant::now();
            waiting = true;
        }
        while let Ok(done) = frame_rx.try_recv() {
            match done {
                Ok((k, w, h, tiles)) => {
                    if k != monitor {
                        continue;
                    }
                    if (w, h) != size {
                        size = (w, h);
                        let _ = ws.send(Message::Text(format!("size {k} {w} {h}").into()));
                    }
                    if tiles.is_empty() {
                        // Nothing changed: ask again in a moment.
                        waiting = false;
                        pending = Some(false);
                        continue;
                    }
                    for t in tiles {
                        ws.write(Message::Binary(t.into())).map_err(|e| e.to_string())?;
                    }
                    ws.send(Message::Text("frame".into())).map_err(|e| e.to_string())?;
                    // `ack` asks for the next.
                    pending = None;
                }
                Err(e) => {
                    let _ = ws.send(Message::Text(format!("error {e}").into()));
                    std::thread::sleep(Duration::from_millis(500));
                    waiting = false;
                    pending = Some(true);
                }
            }
        }
    }
    if let Some(h) = hands.lock().unwrap().as_mut() {
        h.release();
    }
    Ok(())
}

/// The pictures of a viewer: each monitor taken, compared with the last
/// time in squares, and the changed squares (joined along each row) as JPEG.
fn pictures(names: Vec<String>, want: mpsc::Receiver<Want>, out: mpsc::Sender<Result<(usize, u32, u32, Vec<Vec<u8>>), String>>) {
    let mut last: HashMap<usize, (u32, u32, Vec<u8>)> = HashMap::new();
    while let Ok(Want::Frame { monitor, whole }) = want.recv() {
        let result = (|| {
            let name = names.get(monitor).ok_or("no such monitor")?;
            let shot = std::process::Command::new("grim").args(["-o", name, "-t", "ppm", "-"]).output().map_err(|e| format!("grim: {e}"))?;
            if !shot.status.success() {
                return Err(format!("grim: {}", String::from_utf8_lossy(&shot.stderr).trim()));
            }
            let (w, h, rgb) = ppm(&shot.stdout).ok_or("grim: not a picture")?;
            let before = last.get(&monitor).filter(|(lw, lh, _)| !whole && (*lw, *lh) == (w, h)).map(|l| &l.2);
            let tiles = changed(w as usize, h as usize, &rgb, before);
            last.insert(monitor, (w, h, rgb));
            Ok((monitor, w, h, tiles))
        })();
        if out.send(result).is_err() {
            break;
        }
    }
}

fn ppm(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    // P6, width, height, maxval, then one whitespace and the pixels.
    let mut fields = Vec::new();
    let mut k = 0;
    while fields.len() < 4 && k < data.len() {
        while k < data.len() && data[k].is_ascii_whitespace() {
            k += 1;
        }
        if data.get(k) == Some(&b'#') {
            while k < data.len() && data[k] != b'\n' {
                k += 1;
            }
            continue;
        }
        let start = k;
        while k < data.len() && !data[k].is_ascii_whitespace() {
            k += 1;
        }
        fields.push(std::str::from_utf8(&data[start..k]).ok()?.to_owned());
    }
    if fields.first()? != "P6" || fields.get(3)? != "255" {
        return None;
    }
    let (w, h): (u32, u32) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let pixels = data.get(k + 1..k + 1 + (w * h * 3) as usize)?;
    Some((w, h, pixels.to_vec()))
}

/// The squares that differ from `before` (all, without it), each row's
/// neighbours joined: `x y w h` (u16, big-endian) and the JPEG.
fn changed(w: usize, h: usize, rgb: &[u8], before: Option<&Vec<u8>>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for ty in (0..h).step_by(TILE) {
        let th = TILE.min(h - ty);
        let mut run: Option<(usize, usize)> = None;
        let mut tx = 0;
        loop {
            let dirty = tx < w && {
                let tw = TILE.min(w - tx);
                match before {
                    None => true,
                    Some(b) => (ty..ty + th).any(|y| {
                        let at = (y * w + tx) * 3;
                        rgb[at..at + tw * 3] != b[at..at + tw * 3]
                    }),
                }
            };
            match (dirty, run) {
                (true, None) => run = Some((tx, tx)),
                (true, Some((s, _))) => run = Some((s, tx)),
                (false, Some((s, e))) => {
                    let x1 = (e + TILE).min(w);
                    if let Some(t) = jpeg(w, rgb, s, ty, x1 - s, th) {
                        out.push(t);
                    }
                    run = None;
                }
                (false, None) => {}
            }
            if tx >= w {
                break;
            }
            tx += TILE;
        }
    }
    out
}

fn jpeg(w: usize, rgb: &[u8], x: usize, y: usize, cw: usize, ch: usize) -> Option<Vec<u8>> {
    let mut cut = Vec::with_capacity(cw * ch * 3);
    for row in y..y + ch {
        let at = (row * w + x) * 3;
        cut.extend_from_slice(&rgb[at..at + cw * 3]);
    }
    let mut out = Vec::with_capacity(16 + cw * ch / 4);
    out.push(1);
    for v in [x, y, cw, ch] {
        out.extend_from_slice(&(v as u16).to_be_bytes());
    }
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode(&cut, cw as u32, ch as u32, image::ExtendedColorType::Rgb8).ok()?;
    Some(out)
}

/// What the clipboard here holds as text, if it holds text.
fn wl_paste() -> Option<String> {
    std::process::Command::new("wl-paste").args(["-n", "-t", "text/plain"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Past this, a copy here is not sent to the page: a whole file's text.
const CLIP_MOST: usize = 1 << 20;

/// `wl-paste --watch`: a line each time the clipboard here changes.
struct ClipWatch {
    child: std::process::Child,
    changed: mpsc::Receiver<()>,
}

impl ClipWatch {
    fn start() -> Option<ClipWatch> {
        let mut child = std::process::Command::new("wl-paste").args(["--watch", "echo"]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
        let out = child.stdout.take()?;
        let (tx, changed) = mpsc::channel();
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                for _ in BufReader::new(out).lines().map_while(Result::ok) {
                    if tx.send(()).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(ClipWatch { child, changed })
    }
}

impl Drop for ClipWatch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wl_copy(text: &str) -> bool {
    let Ok(mut child) = std::process::Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() else { return false };
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(text.as_bytes());
    }
    child.wait().is_ok_and(|s| s.success())
}

// ---------------------------------------------------------------- the hands (uinput)

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 11;
const REL_HWHEEL_HI_RES: u16 = 12;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const ABS_MAX: i32 = 65535;

const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_ABS_SETUP: libc::c_ulong = 0x401c_5504;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;

#[repr(C)]
struct UinputSetup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct AbsSetup {
    code: u16,
    _pad: u16,
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
struct InputEvent {
    sec: i64,
    usec: i64,
    kind: u16,
    code: u16,
    value: i32,
}

/// A device of our own: what it presses reaches the session as from a
/// mouse or keyboard plugged in.
struct Device {
    fd: std::os::fd::OwnedFd,
}

impl Device {
    fn new(name: &str, product: u16, setup: impl Fn(i32) -> bool) -> Result<Device, String> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let path = std::ffi::CString::new("/dev/uinput").unwrap();
        let raw = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if raw < 0 {
            return Err(format!("/dev/uinput: {}", std::io::Error::last_os_error()));
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
        if !setup(fd.as_raw_fd()) {
            return Err(format!("/dev/uinput: {}", std::io::Error::last_os_error()));
        }
        let mut s = UinputSetup { bustype: 0x06, vendor: 0x1d6b, product, version: 1, name: [0; 80], ff_effects_max: 0 };
        s.name[..name.len().min(79)].copy_from_slice(&name.as_bytes()[..name.len().min(79)]);
        unsafe {
            if libc::ioctl(fd.as_raw_fd(), UI_DEV_SETUP, &s) < 0 || libc::ioctl(fd.as_raw_fd(), UI_DEV_CREATE) < 0 {
                return Err(format!("/dev/uinput: {}", std::io::Error::last_os_error()));
            }
        }
        Ok(Device { fd })
    }

    fn emit(&self, events: &[(u16, u16, i32)]) {
        use std::os::fd::AsRawFd;
        let mut all: Vec<InputEvent> = events.iter().map(|(kind, code, value)| InputEvent { sec: 0, usec: 0, kind: *kind, code: *code, value: *value }).collect();
        all.push(InputEvent { sec: 0, usec: 0, kind: EV_SYN, code: 0, value: 0 });
        let bytes = unsafe { std::slice::from_raw_parts(all.as_ptr() as *const u8, all.len() * std::mem::size_of::<InputEvent>()) };
        unsafe {
            libc::write(self.fd.as_raw_fd(), bytes.as_ptr() as *const libc::c_void, bytes.len());
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::ioctl(self.fd.as_raw_fd(), UI_DEV_DESTROY);
        }
    }
}

struct Hands {
    pointer: Option<Device>,
    keyboard: Option<Device>,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<u16>,
    wheel: (f64, f64),
    /// Instead of devices, steps down a pipe to a headless desktop
    /// (`PLEAMAR_REMOTE_HANDS_TO`, its `PLEAMAR_HEADLESS_INPUT_FIFO`): to try
    /// the page without touching the real session's input.
    to: Option<std::fs::File>,
    /// A phone's fingers moving what is under them: told to the session
    /// itself (`fingers`), which no device here could say. And whether the
    /// session refused them (an older one, still running): the wheel then.
    fingers: Option<mpsc::Sender<Finger>>,
    refused: Arc<std::sync::atomic::AtomicBool>,
}

/// What a finger on a phone's glass does to what is under it.
enum Finger {
    /// It went so far, in units of the desktop.
    Moved(f64, f64),
    /// Lifted, going that fast (units a millisecond): it goes on by itself.
    Lifted(f64, f64),
    /// Down again: what was going on by itself stops there.
    Halt,
}

/// The phone's fingers, to the session down its socket (`F`): a program
/// follows them to the pixel, as a touchpad's. Lifted while moving, what was
/// under them goes on by itself, slowing down: from here, at this computer's
/// own pace (from the phone, each step came when the network let it).
fn fingers(from: mpsc::Receiver<Finger>, refused: Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    let mut line: Option<crate::agent_cli::Line> = None;
    let mut say = |text: String| {
        if line.is_none() {
            line = crate::agent_cli::Line::open().ok();
        }
        match line.as_mut().map(|l| l.say(&text)) {
            Some(Ok(reply)) => refused.store(reply.starts_with("err"), Ordering::Relaxed),
            // Hung up (the session started again): asked again next time.
            Some(Err(_)) => line = None,
            None => refused.store(true, Ordering::Relaxed),
        }
    };
    let mut glide: Option<(f64, f64, Instant)> = None;
    loop {
        let said = match glide {
            Some(_) => match from.recv_timeout(Duration::from_millis(8)) {
                Ok(f) => Some(f),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match from.recv() {
                Ok(f) => Some(f),
                Err(_) => return,
            },
        };
        match said {
            Some(Finger::Moved(x, y)) => {
                glide = None;
                say(format!("F {x:.2} {y:.2}"));
            }
            Some(Finger::Lifted(vx, vy)) => {
                if vx.hypot(vy) > 0.05 {
                    glide = Some((vx, vy, Instant::now()));
                } else {
                    glide = None;
                    say("F end".into());
                }
            }
            Some(Finger::Halt) => {
                if glide.take().is_some() {
                    say("F end".into());
                }
            }
            None => {
                let Some((vx, vy, last)) = glide.as_mut() else { continue };
                let ms = last.elapsed().as_secs_f64().min(0.04) * 1000.0;
                *last = Instant::now();
                let (x, y) = (*vx * ms, *vy * ms);
                let slower = 0.9965f64.powf(ms);
                *vx *= slower;
                *vy *= slower;
                let stopped = vx.hypot(*vy) < 0.03;
                say(format!("F {x:.2} {y:.2}"));
                if stopped {
                    glide = None;
                    say("F end".into());
                }
            }
        }
    }
}

impl Hands {
    fn new() -> Result<Hands, String> {
        if let Ok(path) = std::env::var("PLEAMAR_REMOTE_HANDS_TO") {
            let to = std::fs::OpenOptions::new().write(true).open(&path).map_err(|e| format!("{path}: {e}"))?;
            println!("remote · the hands go down {path}, not to devices");
            return Ok(Hands { to: Some(to), ..Hands::none() }.with_fingers());
        }
        // A pointer that says where it is (as a tablet, or a virtual
        // machine's mouse): over all of the desktop, 0…65535.
        let pointer = Device::new("pleamar remote pointer", 0x0701, |fd| unsafe {
            let mut ok = libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_int) >= 0
                && libc::ioctl(fd, UI_SET_EVBIT, EV_ABS as libc::c_int) >= 0
                && libc::ioctl(fd, UI_SET_EVBIT, EV_REL as libc::c_int) >= 0;
            for b in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
                ok &= libc::ioctl(fd, UI_SET_KEYBIT, b as libc::c_int) >= 0;
            }
            for r in [REL_WHEEL, REL_HWHEEL, REL_WHEEL_HI_RES, REL_HWHEEL_HI_RES] {
                ok &= libc::ioctl(fd, UI_SET_RELBIT, r as libc::c_int) >= 0;
            }
            for a in [ABS_X, ABS_Y] {
                ok &= libc::ioctl(fd, UI_SET_ABSBIT, a as libc::c_int) >= 0;
                let s = AbsSetup { code: a, _pad: 0, value: 0, minimum: 0, maximum: ABS_MAX, fuzz: 0, flat: 0, resolution: 0 };
                ok &= libc::ioctl(fd, UI_ABS_SETUP, &s) >= 0;
            }
            ok
        })?;
        let keyboard = Device::new("pleamar remote keyboard", 0x0702, |fd| unsafe {
            let mut ok = libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_int) >= 0;
            for k in 1..=248 {
                ok &= libc::ioctl(fd, UI_SET_KEYBIT, k as libc::c_int) >= 0;
            }
            ok
        })?;
        // The session finds them a moment later.
        std::thread::sleep(Duration::from_millis(300));
        Ok(Hands { pointer: Some(pointer), keyboard: Some(keyboard), ..Hands::none() }.with_fingers())
    }

    fn with_fingers(mut self) -> Hands {
        let (tx, rx) = mpsc::channel();
        let refused = self.refused.clone();
        std::thread::spawn(move || fingers(rx, refused));
        self.fingers = Some(tx);
        self
    }

    /// Hands that do nothing: only watching.
    fn none() -> Hands {
        Hands { pointer: None, keyboard: None, held_keys: HashSet::new(), held_buttons: HashSet::new(), wheel: (0.0, 0.0), to: None, fingers: None, refused: Default::default() }
    }

    /// A step down the pipe, when there is one instead of devices.
    fn piped(&mut self, step: &str) -> bool {
        match self.to.as_mut() {
            Some(f) => {
                let _ = writeln!(f, "{step}");
                true
            }
            None => false,
        }
    }

    fn press(&self, pointer: bool, events: &[(u16, u16, i32)]) {
        if let Some(d) = if pointer { &self.pointer } else { &self.keyboard } {
            d.emit(events);
        }
    }

    /// The pointer to a point of the desktop, in units.
    fn point(&mut self, monitors: &[(String, f64, f64, f64, f64)], x: f64, y: f64) {
        if self.piped(&format!("{x:.1},{y:.1}")) {
            return;
        }
        let x0 = monitors.iter().map(|m| m.1).fold(f64::MAX, f64::min);
        let y0 = monitors.iter().map(|m| m.2).fold(f64::MAX, f64::min);
        let x1 = monitors.iter().map(|m| m.1 + m.3).fold(f64::MIN, f64::max);
        let y1 = monitors.iter().map(|m| m.2 + m.4).fold(f64::MIN, f64::max);
        let ax = (((x - x0) / (x1 - x0).max(1.0)) * (ABS_MAX as f64 + 1.0)).round().clamp(0.0, ABS_MAX as f64) as i32;
        let ay = (((y - y0) / (y1 - y0).max(1.0)) * (ABS_MAX as f64 + 1.0)).round().clamp(0.0, ABS_MAX as f64) as i32;
        self.press(true, &[(EV_ABS, ABS_X, ax), (EV_ABS, ABS_Y, ay)]);
    }

    fn button(&mut self, which: u32, down: bool) {
        let code = match which {
            0 => BTN_LEFT,
            1 => BTN_MIDDLE,
            2 => BTN_RIGHT,
            _ => return,
        };
        if down {
            self.held_buttons.insert(code);
        } else {
            self.held_buttons.remove(&code);
        }
        let step = match (which, down) {
            (2, true) => "rdown",
            (2, false) => "rup",
            (_, true) => "down",
            (_, false) => "up",
        };
        if self.piped(step) {
            return;
        }
        self.press(true, &[(EV_KEY, code, down as i32)]);
    }

    /// The wheel, in notches (a fraction of one too: a touchpad).
    fn wheel(&mut self, dx: f64, dy: f64) {
        if self.to.is_some() {
            if dy != 0.0 {
                self.piped(&format!("wheel:{:.3}", -dy));
            }
            if dx != 0.0 {
                self.piped(&format!("wheelx:{:.3}", -dx));
            }
            return;
        }
        let mut events = Vec::new();
        let (hx, hy) = ((dx * 120.0).round() as i32, (-dy * 120.0).round() as i32);
        if hy != 0 {
            events.push((EV_REL, REL_WHEEL_HI_RES, hy));
        }
        if hx != 0 {
            events.push((EV_REL, REL_HWHEEL_HI_RES, hx));
        }
        // The old notches too, for whoever reads only those.
        self.wheel.0 += dx;
        self.wheel.1 -= dy;
        for (sum, code) in [(&mut self.wheel.0, REL_HWHEEL), (&mut self.wheel.1, REL_WHEEL)] {
            let whole = sum.trunc();
            if whole != 0.0 {
                events.push((EV_REL, code, whole as i32));
                *sum -= whole;
            }
        }
        if !events.is_empty() {
            self.press(true, &events);
        }
    }

    /// A phone's finger: how far it went on the glass, in units (what is
    /// under it goes the other way round, as Wayland counts it), lifted, or
    /// down again. A session that does not know of fingers (an older one,
    /// still running) is given the wheel, a notch every 50 units, as before.
    fn finger(&mut self, f: Finger) {
        let known = !self.refused.load(std::sync::atomic::Ordering::Relaxed);
        match (self.fingers.clone(), f) {
            (Some(to), Finger::Moved(x, y)) if known => drop(to.send(Finger::Moved(-x, -y))),
            (Some(to), Finger::Lifted(x, y)) if known => drop(to.send(Finger::Lifted(-x, -y))),
            (Some(to), Finger::Halt) if known => drop(to.send(Finger::Halt)),
            (Some(_), Finger::Moved(x, y)) => self.wheel(-x / 50.0, -y / 50.0),
            _ => {}
        }
    }

    fn key(&mut self, code: u16, down: bool) {
        if code == 0 || code > 248 {
            return;
        }
        if down {
            self.held_keys.insert(code);
        } else if !self.held_keys.remove(&code) {
            return;
        }
        if self.to.is_some() {
            let name = match code {
                14 => "BackSpace",
                28 => "Return",
                57 => "space",
                1 => "Escape",
                // (Letters too: a keyboard of its own on a tablet.)
                16..=25 | 30..=38 | 44..=50 => {
                    let row = match code {
                        16..=25 => &"qwertyuiop"[(code - 16) as usize..],
                        30..=38 => &"asdfghjkl"[(code - 30) as usize..],
                        _ => &"zxcvbnm"[(code - 44) as usize..],
                    };
                    &row[..1]
                }
                _ => "",
            };
            if !name.is_empty() {
                self.piped(&format!("{}:{name}:{code}", if down { "keydown" } else { "keyup" }));
            }
            return;
        }
        self.press(false, &[(EV_KEY, code, down as i32)]);
    }

    /// Ctrl+V (Ctrl+Shift+V), with what of it is not held already: the
    /// page's own Ctrl may be down still, or let go on the way.
    fn paste_keys(&mut self, shift: bool) {
        let held = |h: &Hands, a: u16, b: u16| h.held_keys.contains(&a) || h.held_keys.contains(&b);
        let mut pressed = Vec::new();
        if !held(self, 29, 97) {
            pressed.push(29);
        }
        if shift && !held(self, 42, 54) {
            pressed.push(42);
        }
        for &k in &pressed {
            self.key(k, true);
        }
        self.key(47, true);
        self.key(47, false);
        for &k in pressed.iter().rev() {
            self.key(k, false);
        }
    }

    /// Nothing left pressed (the page lost focus, or went away).
    fn release(&mut self) {
        for code in std::mem::take(&mut self.held_keys) {
            self.press(false, &[(EV_KEY, code, 0)]);
        }
        for code in std::mem::take(&mut self.held_buttons) {
            self.press(true, &[(EV_KEY, code, 0)]);
        }
    }
}

// ---------------------------------------------------------------- little things

fn random(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    let mut f = std::fs::File::open("/dev/urandom").expect("/dev/urandom");
    f.read_exact(&mut out).expect("/dev/urandom");
    out
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// RFC 6238: six digits, from HMAC-SHA1 of the 30-second step.
fn totp(key: &[u8], step: u64) -> String {
    let mut mac = <Hmac<sha1::Sha1> as Mac>::new_from_slice(key).expect("any key size");
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let at = (h[19] & 0x0f) as usize;
    let v = u32::from_be_bytes([h[at] & 0x7f, h[at + 1], h[at + 2], h[at + 3]]) % 1_000_000;
    format!("{v:06}")
}

/// Equal, taking as long whatever the difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|k| u8::from_str_radix(&s[k..k + 2], 16).ok()).collect()
}

const B32: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32(b: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for &v in b {
        buffer = (buffer << 8) | v as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn unbase32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = B32.iter().position(|x| *x == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

pub(crate) fn base64(b: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_the_rfc() {
        // RFC 6238, appendix B (SHA1, 8 digits there: the last six here).
        let key = b"12345678901234567890";
        assert_eq!(totp(key, 59 / 30), "287082");
        assert_eq!(totp(key, 1111111109 / 30), "081804");
        assert_eq!(totp(key, 2000000000 / 30), "279037");
    }

    #[test]
    fn base32_both_ways() {
        let b = random(20);
        assert_eq!(unbase32(&base32(&b)).unwrap(), b);
        assert_eq!(base64(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn frames_of_a_stream() {
        // SPS, PPS, IDR · a picture · AUD, a picture.
        let stream = [0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 1, 0x65, 3, 3, 0, 0, 0, 1, 0x41, 4, 0, 0, 0, 1, 0x09, 5, 0, 0, 1, 0x41, 6];
        let f = access_units(&stream);
        assert_eq!(f.len(), 3);
        assert!(f[0].0 && !f[1].0 && !f[2].0);
        assert_eq!(f[0].1, &stream[..18]);
        assert_eq!(f[1].1, [0, 0, 0, 1, 0x41, 4]);
        assert_eq!(f[2].1, [0, 0, 0, 1, 0x09, 5, 0, 0, 1, 0x41, 6]);
    }

    #[test]
    fn squares_that_changed() {
        let (w, h) = (130usize, 70usize);
        let a = vec![0u8; w * h * 3];
        let mut b = a.clone();
        assert_eq!(changed(w, h, &b, Some(&a)).len(), 0);
        b[(10 * w + 100) * 3] = 255;
        let t = changed(w, h, &b, Some(&a));
        assert_eq!(t.len(), 1);
        assert_eq!(&t[0][..9], &[1, 0, 64, 0, 0, 0, 64, 0, 64]);
        // All of it: two rows of three squares, joined along each row.
        assert_eq!(changed(w, h, &a, None).len(), 2);
    }
}

#[cfg(test)]
mod relay_tests {
    use super::relay_of;

    #[test]
    fn the_relays_seen_are_told_apart_from_a_home() {
        assert_eq!(relay_of("2a09:bac3:3137:3632::566:5f"), Some("Cloudflare"));
        assert_eq!(relay_of("2a02:26f7:ef0c:5905:0:9000:0:c"), Some("Akamai"));
        assert_eq!(relay_of("172.226.1.2"), Some("Akamai"));
        assert_eq!(relay_of("213.194.159.80"), None);
        assert_eq!(relay_of("192.168.3.56"), None);
        assert_eq!(relay_of("not an address"), None);
    }
}
