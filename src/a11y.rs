//! The accessibility bus: what programs publish for screen readers (AT-SPI2,
//! their trees of buttons, links and text) goes here instead of to
//! at-spi2-core's bus, and only pleamar-wm and the readers the user allows
//! can read it. Turning it on does not hand the desktop's text to everything
//! that runs as the user.
//!
//! It is a D-Bus broker for that one purpose, on a thread of its own: the
//! part of the bus (`org.freedesktop.DBus`) these programs use, the registry
//! they embed their trees in (`org.a11y.atspi.Registry`), and a policy. A
//! program reaches the bus and the registry, never another program; a reply
//! goes through only to the call it answers; a connection that calls into
//! another program's objects is a reader, and is let through only if
//! session.conf names it (`a11y reader orca`).
//!
//! pleamar-wm's own reader has no socket: `call` and `listen` go straight to
//! the programs' connections, from any thread.
//!
//! Programs find it by `AT_SPI_BUS_ADDRESS` (what the session starts, and
//! dbus and systemd) and by `org.a11y.Bus` on the session bus (the rest).
//! `PLEAMAR_A11Y_LOG=1` says every message a connection sends.

use futures_lite::StreamExt;
use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::os::fd::AsRawFd;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use zbus::message::{Flags, Header, Message, Type};
use zbus::zvariant::{OwnedObjectPath, Value};

const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const DEC_PATH: &str = "/org/a11y/atspi/registry/deviceeventcontroller";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const NULL: &str = "/org/a11y/atspi/null";
/// pleamar-wm on its own bus: the registry and the reader are one connection.
const OWN: &str = ":1.0";

const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
const NOT_SUPPORTED: &str = "org.freedesktop.DBus.Error.NotSupported";
const NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
const LIMITS: &str = "org.freedesktop.DBus.Error.LimitsExceeded";
const NO_REPLY: &str = "org.freedesktop.DBus.Error.NoReply";
const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// The largest message taken: a tree's whole text fits many times over.
const MAX_SIZE: usize = 8 << 20;
/// Calls a connection may have waiting for an answer at once.
const MAX_CALLS: usize = 256;
/// Match rules a connection may keep.
const MAX_RULES: usize = 512;
/// Messages in a second past which a connection is put out.
const FLOOD: u32 = 10_000;
/// What a connection that does not read its socket may have queued to it.
const QUEUE: usize = 4096;
/// How long a call of pleamar-wm's own is waited for at most.
const OWN_DEADLINE: Duration = Duration::from_secs(60);
/// How long a reader is held for the user's answer.
const ASK_DEADLINE: Duration = Duration::from_secs(60);

/// A connection to the bus, as pleamar-wm's reader sees it.
#[derive(Clone, Debug)]
pub struct Program {
    /// Its unique name (`:1.N`).
    pub name: String,
    pub pid: u32,
    /// `/proc/PID/exe`.
    pub exe: String,
    /// The root of its tree, as it embedded it: (bus name, object path).
    pub root: Option<(String, String)>,
    /// A reader that was let in, not a program publishing.
    pub reader: bool,
}

/// Why one of pleamar-wm's calls got no answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The bus is not running.
    Off,
    /// No such connection, or it went away before answering.
    Gone,
    Timeout,
    /// The program answered with an error: its name and its message.
    Failed(String, String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Off => write!(f, "the accessibility bus is off"),
            Error::Gone => write!(f, "gone"),
            Error::Timeout => write!(f, "timeout"),
            Error::Failed(name, text) => write!(f, "{name}: {text}"),
        }
    }
}

/// What other threads ask of the broker's.
enum Cmd {
    Call(Message, async_channel::Sender<Result<Message, Error>>),
    /// Events pleamar-wm's reader wants (true) or no longer wants.
    Events(Vec<String>, bool),
    Listen(async_channel::Sender<Message>),
    Answer(u32, bool),
}

static TO_BROKER: OnceLock<async_channel::Sender<Cmd>> = OnceLock::new();
static ADDRESS: OnceLock<String> = OnceLock::new();
static PROGRAMS: Mutex<Vec<Program>> = Mutex::new(Vec::new());
/// Who asks the user about a reader that is not in session.conf; without
/// one, such a reader is refused at once.
static ASK: Mutex<Option<Box<dyn Fn(&Program) + Send>>> = Mutex::new(None);

fn tell(c: Cmd) -> bool {
    TO_BROKER.get().is_some_and(|tx| tx.try_send(c).is_ok())
}

fn logging() -> bool {
    static LOG: OnceLock<bool> = OnceLock::new();
    *LOG.get_or_init(|| std::env::var_os("PLEAMAR_A11Y_LOG").is_some_and(|v| v != "0"))
}

/// Where the socket goes, for the display it serves.
pub fn socket_path(display: &str) -> String {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    format!("{dir}/pleamar-a11y-{display}")
}

/// The bus's address (`unix:path=...`), once it listens.
pub fn address() -> Option<&'static str> {
    ADDRESS.get().map(String::as_str)
}

/// What the programs the session starts are told, so they publish their
/// trees here and turn them on. Without at-spi2-core NixOS says
/// `NO_AT_BRIDGE=1` and `GTK_A11Y=none` to everything, which keeps GTK 3's
/// and GTK 4's trees off: said back.
pub fn environment() -> Vec<(&'static str, String)> {
    let Some(a) = address() else { return Vec::new() };
    vec![
        ("AT_SPI_BUS_ADDRESS", a.to_owned()),
        ("GNOME_ACCESSIBILITY", "1".into()),
        ("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1".into()),
        ("ACCESSIBILITY_ENABLED", "1".into()),
        ("NO_AT_BRIDGE", "0".into()),
        ("GTK_A11Y", "atspi".into()),
    ]
}

/// Every connection on the bus now, readers too.
pub fn programs() -> Vec<Program> {
    PROGRAMS.lock().unwrap().clone()
}

/// The connection of that process that embedded a tree, if any did.
pub fn program_of(pid: u32) -> Option<Program> {
    PROGRAMS.lock().unwrap().iter().find(|p| p.pid == pid && p.root.is_some() && !p.reader).cloned()
}

/// Calls a method on a program's object and waits for its answer: `name` is
/// its connection (`Program::name`, or a bus name its tree gave), `path`
/// the object.
pub async fn call<B>(name: &str, path: &str, interface: &str, member: &str, body: &B) -> Result<Message, Error>
where
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
{
    let tx = TO_BROKER.get().ok_or(Error::Off)?;
    let bad = |e: zbus::Error| Error::Failed(INVALID_ARGS.into(), e.to_string());
    let msg = Message::method_call(path, member)
        .and_then(|b| b.interface(interface))
        .and_then(|b| b.destination(name))
        .and_then(|b| b.sender(OWN))
        .and_then(|b| b.build(body))
        .map_err(bad)?;
    let (reply, answer) = async_channel::bounded(1);
    tx.try_send(Cmd::Call(msg, reply)).map_err(|_| Error::Off)?;
    answer.recv().await.unwrap_or(Err(Error::Gone))
}

/// The same, from a thread that is not async, waiting at most `deadline`.
pub fn call_blocking<B>(name: &str, path: &str, interface: &str, member: &str, body: &B, deadline: Duration) -> Result<Message, Error>
where
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
{
    futures_lite::future::block_on(futures_lite::future::or(call(name, path, interface, member, body), async {
        async_io::Timer::after(deadline).await;
        Err(Error::Timeout)
    }))
}

/// Events pleamar-wm's reader is sent while this lives (`object:state-changed`,
/// `focus:`...): the registry tells the programs, which emit only events
/// someone registered. Every program's signals come down `signals`, these and
/// any other it emits.
pub struct Listener {
    pub signals: async_channel::Receiver<Message>,
    events: Vec<String>,
}

pub fn listen(events: &[&str]) -> Option<Listener> {
    let (tx, signals) = async_channel::bounded(1024);
    let events: Vec<String> = events.iter().map(|e| e.to_string()).collect();
    (tell(Cmd::Listen(tx)) && tell(Cmd::Events(events.clone(), true))).then_some(Listener { signals, events })
}

impl Drop for Listener {
    fn drop(&mut self) {
        tell(Cmd::Events(std::mem::take(&mut self.events), false));
    }
}

/// Who asks the user when a reader that session.conf does not name wants
/// in: it is held until `answer`, or refused after a minute.
pub fn on_ask(f: impl Fn(&Program) + Send + 'static) {
    *ASK.lock().unwrap() = Some(Box::new(f));
}

/// The user's answer about the reader of that process: allowed (for this
/// session: `config::write_a11y_reader` keeps it), or refused until it
/// connects again.
pub fn answer(pid: u32, allow: bool) {
    tell(Cmd::Answer(pid, allow));
}

/// The bus, if session.conf says `agent on`: its socket now, its thread, and
/// `org.a11y.Bus` on the session bus.
pub fn start(display: &str) {
    if !crate::config::get().agent {
        return;
    }
    if let Some(address) = broker_at(&socket_path(display)) {
        let _ = std::thread::Builder::new().name("a11y-session".into()).spawn(move || publish(address));
    }
}

/// The broker, listening at that path: its address.
fn broker_at(path: &str) -> Option<String> {
    let _ = std::fs::remove_file(&path);
    let listener = match std::os::unix::net::UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("a11y: cannot listen at {path}: {e}");
            return None;
        }
    };
    // Yours only: the policy is the boundary, but nobody else needs it.
    let _ = std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    let address = format!("unix:path={path}");
    let _ = ADDRESS.set(address.clone());
    let (tx, rx) = async_channel::unbounded();
    let _ = TO_BROKER.set(tx);
    let started = std::thread::Builder::new().name("a11y".into()).spawn(move || {
        let ex: &'static async_executor::LocalExecutor<'static> = Box::leak(Box::default());
        let broker = Rc::new(RefCell::new(Broker::new(ex)));
        ex.spawn(commands(broker.clone(), rx)).detach();
        futures_lite::future::block_on(ex.run(serve(ex, broker, listener)));
    });
    if started.is_err() {
        eprintln!("a11y: no thread for the bus");
        return None;
    }
    println!("a11y: programs publish their trees at AT_SPI_BUS_ADDRESS={address}");
    Some(address)
}

// The session bus: where programs started elsewhere look for the bus.

struct BusName(String);

#[zbus::interface(name = "org.a11y.Bus")]
impl BusName {
    fn get_address(&self) -> String {
        self.0.clone()
    }
}

struct Status {
    screen_reader: bool,
}

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    /// Always: a program that sees it off would not publish.
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn set_is_enabled(&mut self, _on: bool) {}
    #[zbus(property)]
    fn screen_reader_enabled(&self) -> bool {
        self.screen_reader
    }
    /// Orca says it runs.
    #[zbus(property)]
    fn set_screen_reader_enabled(&mut self, on: bool) {
        self.screen_reader = on;
    }
}

fn publish(address: String) {
    // A test session on the user's own session bus would point their
    // programs at a bus that is gone when it ends.
    if crate::portal::HEADLESS.load(std::sync::atomic::Ordering::Relaxed) {
        let own = std::env::var("XDG_RUNTIME_DIR").map(|d| format!("unix:path={d}/bus")).ok();
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        if bus.is_none() || bus == own {
            println!("a11y: a test session: org.a11y.Bus left to the user's session bus");
            return;
        }
    }
    const NAME: &str = "org.a11y.Bus";
    let built = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at("/org/a11y/bus", BusName(address)))
        .and_then(|b| b.serve_at("/org/a11y/bus", Status { screen_reader: false }))
        .and_then(|b| b.build())
        .and_then(|c| match c.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())? {
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => Ok(c),
            _ => Err(zbus::Error::NameTaken),
        });
    match built {
        Ok(connection) => {
            println!("a11y: {NAME} on the session bus is this session's");
            loop {
                std::thread::park();
                let _ = &connection;
            }
        }
        Err(zbus::Error::NameTaken) => eprintln!("a11y: {NAME} is taken (at-spi2-core runs): only programs with AT_SPI_BUS_ADDRESS find this bus"),
        Err(e) => eprintln!("a11y: not on the session bus ({e}): only programs with AT_SPI_BUS_ADDRESS find this bus"),
    }
}

// The broker.

#[derive(Clone, Copy, PartialEq, Debug)]
enum Reader {
    Asking,
    Allowed,
    Denied,
}

struct Peer {
    /// `:1.N`, from its `Hello`; empty before it.
    name: String,
    pid: u32,
    exe: String,
    /// What the process calls itself (`/proc/PID/comm`): a script's name,
    /// where `exe` is its interpreter.
    comm: String,
    root: Option<(String, String)>,
    reader: Option<Reader>,
    /// Calls held while the user is asked.
    held: Vec<Message>,
    /// Well-known names it owns (a reader's).
    names: Vec<String>,
    rules: Vec<(String, Rule)>,
    /// Events it registered with the registry (a reader's).
    events: Vec<String>,
    /// Its calls still waiting for an answer.
    calls: usize,
    /// Messages since `since`, for floods.
    since: Instant,
    count: u32,
    out: async_channel::Sender<Message>,
    conn: zbus::Connection,
}

/// Who waits for the answer to a call that was passed on.
enum Waiter {
    /// A connection, and the serial of its call.
    Peer(u64, NonZeroU32),
    Own(async_channel::Sender<Result<Message, Error>>, Instant),
}

enum Gate {
    Pass,
    Held,
    Deny,
}

struct Broker {
    peers: HashMap<u64, Peer>,
    /// Unique and well-known names, to their connection.
    names: HashMap<String, u64>,
    /// By the connection called and the serial the call was sent with.
    waiting: HashMap<(u64, u32), Waiter>,
    /// The events pleamar-wm's reader wants, and how many want each.
    own_events: HashMap<String, usize>,
    listeners: Vec<async_channel::Sender<Message>>,
    /// Readers let in, by name: session.conf's and the user's answers.
    allowed: Vec<String>,
    guid: zbus::OwnedGuid,
    next: u64,
    ex: &'static async_executor::LocalExecutor<'static>,
    /// Methods answered `NotSupported` that were said already.
    unsupported: std::collections::HashSet<String>,
}

/// The connections, as they come; each read on a task of its own.
async fn serve(ex: &'static async_executor::LocalExecutor<'static>, broker: Rc<RefCell<Broker>>, listener: std::os::unix::net::UnixListener) {
    let listener = match async_io::Async::new(listener) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("a11y: {e}");
            return;
        }
    };
    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                eprintln!("a11y: {e}");
                async_io::Timer::after(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(stream) = stream.into_inner() else { continue };
        ex.spawn(connection(ex, broker.clone(), stream)).detach();
    }
}

/// The peer's process and user, from the socket (`SO_PEERCRED`).
fn credentials(stream: &std::os::unix::net::UnixStream) -> Option<(u32, u32)> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: a ucred, and its size.
    let r = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    (r == 0).then_some((cred.pid as u32, cred.uid))
}

async fn connection(ex: &'static async_executor::LocalExecutor<'static>, broker: Rc<RefCell<Broker>>, stream: std::os::unix::net::UnixStream) {
    let Some((pid, uid)) = credentials(&stream) else { return };
    // SAFETY: getuid cannot fail.
    if uid != unsafe { libc::getuid() } {
        eprintln!("a11y: pid {pid} is another user's ({uid}): refused");
        return;
    }
    let guid = broker.borrow().guid.clone();
    let built = futures_lite::future::or(
        async {
            zbus::connection::Builder::async_io_unix_stream(stream)
                .server(guid)
                .map(|b| b.p2p().internal_executor(false).max_queued(256))
                .map_err(|e| e.to_string())?
                .build_message_stream()
                .await
                .map_err(|e| e.to_string())
        },
        async {
            async_io::Timer::after(Duration::from_secs(5)).await;
            Err("it did not authenticate in time".to_owned())
        },
    );
    // The connection's own tasks (its socket's reader) run on this thread.
    let mut stream = match built.await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("a11y: pid {pid}: {e}");
            return;
        }
    };
    let conn = zbus::Connection::from(&stream);
    let inner = conn.executor().clone();
    ex.spawn(async move {
        while !inner.is_empty() {
            inner.tick().await;
        }
    })
    .detach();
    let (out, queue) = async_channel::bounded::<Message>(QUEUE);
    let writer = conn.clone();
    ex.spawn(async move {
        while let Ok(m) = queue.recv().await {
            if writer.send(&m).await.is_err() {
                break;
            }
        }
    })
    .detach();
    let id = broker.borrow_mut().add(pid, out, conn.clone());
    while let Some(m) = stream.next().await {
        let Ok(m) = m else { break };
        if !broker.borrow_mut().incoming(id, m) {
            break;
        }
    }
    broker.borrow_mut().forget(id);
    let _ = conn.close().await;
}

/// What other threads ask, in order.
async fn commands(broker: Rc<RefCell<Broker>>, rx: async_channel::Receiver<Cmd>) {
    while let Ok(c) = rx.recv().await {
        let mut b = broker.borrow_mut();
        match c {
            Cmd::Call(m, reply) => b.own_call(m, reply),
            Cmd::Events(events, on) => b.own_events(events, on),
            Cmd::Listen(tx) => b.listeners.push(tx),
            Cmd::Answer(pid, allow) => b.answered(pid, allow),
        }
    }
}

fn read_proc(pid: u32, what: &str) -> String {
    std::fs::read_link(format!("/proc/{pid}/{what}")).map(|p| p.display().to_string()).unwrap_or_else(|_| std::fs::read_to_string(format!("/proc/{pid}/{what}")).unwrap_or_default().trim().to_owned())
}

/// A message passed on: the same body, from `sender`.
fn relay<'a>(m: &'a Message, hdr: &'a Header<'a>, sender: &'a str, reply_serial: Option<NonZeroU32>, destination: Option<&'a str>) -> zbus::Result<Message> {
    let endian = m.primary_header().endian_sig().into();
    let mut b = match m.message_type() {
        Type::MethodCall => {
            let mut b = Message::method_call(hdr.path().ok_or(zbus::Error::MissingField)?.clone(), hdr.member().ok_or(zbus::Error::MissingField)?.clone())?;
            if let Some(i) = hdr.interface() {
                b = b.interface(i.clone())?;
            }
            if hdr.primary().flags().contains(Flags::NoReplyExpected) {
                b = b.with_flags(Flags::NoReplyExpected)?;
            }
            b
        }
        Type::Signal => Message::signal(hdr.path().ok_or(zbus::Error::MissingField)?.clone(), hdr.interface().ok_or(zbus::Error::MissingField)?.clone(), hdr.member().ok_or(zbus::Error::MissingField)?.clone())?,
        Type::MethodReturn => Message::method_return(hdr)?.reply_serial(reply_serial),
        Type::Error => Message::error(hdr, hdr.error_name().ok_or(zbus::Error::MissingField)?.clone())?.reply_serial(reply_serial),
    };
    b = b.endian(endian).sender(sender)?;
    if let Some(d) = destination.or(hdr.destination().map(|d| d.as_str())) {
        b = b.destination(d)?;
    }
    let body = m.body();
    // SAFETY: the body as it came, which zbus read and checked against its signature.
    unsafe { b.build_raw_body(body.data(), body.signature().clone(), vec![]) }
}

impl Broker {
    fn new(ex: &'static async_executor::LocalExecutor<'static>) -> Broker {
        Broker {
            ex,
            peers: HashMap::new(),
            names: HashMap::from([(BUS.to_owned(), 0), (REGISTRY.to_owned(), 0), (OWN.to_owned(), 0)]),
            waiting: HashMap::new(),
            own_events: HashMap::new(),
            listeners: Vec::new(),
            allowed: crate::config::get().a11y_readers.clone(),
            guid: zbus::Guid::generate().into(),
            next: 1,
            unsupported: Default::default(),
        }
    }

    fn add(&mut self, pid: u32, out: async_channel::Sender<Message>, conn: zbus::Connection) -> u64 {
        let id = self.next;
        self.next += 1;
        let (exe, comm) = (read_proc(pid, "exe"), read_proc(pid, "comm"));
        self.peers.insert(id, Peer { name: String::new(), pid, exe, comm, root: None, reader: None, held: Vec::new(), names: Vec::new(), rules: Vec::new(), events: Vec::new(), calls: 0, since: Instant::now(), count: 0, out, conn });
        id
    }

    /// What pleamar-wm's reader sees: every connection with its name.
    fn snapshot(&self) {
        let list = self.peers.values().filter(|p| !p.name.is_empty()).map(|p| Program { name: p.name.clone(), pid: p.pid, exe: p.exe.clone(), root: p.root.clone(), reader: p.reader == Some(Reader::Allowed) }).collect();
        *PROGRAMS.lock().unwrap() = list;
    }

    fn label(&self, id: u64) -> String {
        self.peers.get(&id).map(|p| format!("{} (pid {}, {})", if p.name.is_empty() { "-" } else { &p.name }, p.pid, short(&p.exe))).unwrap_or_default()
    }

    fn send(&self, to: u64, m: Message) {
        if let Some(p) = self.peers.get(&to) {
            if p.out.try_send(m).is_err() {
                eprintln!("a11y: {} does not read what it is sent: dropped", self.label(to));
            }
        }
    }

    fn name_of(&self, id: u64) -> &str {
        if id == 0 { OWN } else { self.peers.get(&id).map(|p| p.name.as_str()).unwrap_or("") }
    }

    fn allowed_reader(&self, id: u64) -> bool {
        id == 0 || self.peers.get(&id).is_some_and(|p| p.reader == Some(Reader::Allowed))
    }

    /// Whether `id` may know `name` is on the bus: a program, only the bus,
    /// the registry, itself and the readers; a reader, everything.
    fn visible(&self, id: u64, name: &str) -> Option<u64> {
        let &owner = self.names.get(name)?;
        (owner == 0 || owner == id || self.allowed_reader(id) || self.allowed_reader(owner)).then_some(owner)
    }

    fn reply<B>(&self, to: u64, hdr: &Header<'_>, from: &str, body: &B)
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    {
        if hdr.primary().flags().contains(Flags::NoReplyExpected) {
            return;
        }
        let built = Message::method_return(hdr).and_then(|b| b.sender(from)).and_then(|b| b.destination(self.name_of(to).to_owned())).and_then(|b| b.build(body));
        match built {
            Ok(m) => self.send(to, m),
            Err(e) => eprintln!("a11y: an answer to {} could not be made: {e}", self.label(to)),
        }
    }

    fn error(&self, to: u64, hdr: &Header<'_>, from: &str, name: &str, text: &str) {
        if hdr.primary().flags().contains(Flags::NoReplyExpected) {
            return;
        }
        let built = Message::error(hdr, name).and_then(|b| b.sender(from)).and_then(|b| b.destination(self.name_of(to).to_owned())).and_then(|b| b.build(&(text,)));
        if let Ok(m) = built {
            self.send(to, m);
        }
    }

    /// A signal from the bus or the registry, to the connections whose
    /// rules want it and that `to` lets have it.
    fn emit<B>(&self, from: &str, path: &str, iface: &str, member: &str, body: &B, to: impl Fn(u64, &Peer) -> bool)
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    {
        let Ok(m) = Message::signal(path, iface, member).and_then(|b| b.sender(from)).and_then(|b| b.build(body)) else { return };
        for (&id, p) in &self.peers {
            if !p.name.is_empty() && to(id, p) && self.wants(p, &m) {
                self.send(id, m.clone());
            }
        }
    }

    /// Whether one of its match rules takes that message.
    fn wants(&self, p: &Peer, m: &Message) -> bool {
        let sender = m.header().sender().map(|s| s.to_string());
        // A rule's sender may be a well-known name: its owner's messages.
        let owner = |n: &str| self.names.get(n).map(|&o| self.name_of(o));
        p.rules.iter().any(|(_, r)| r.matches(m, sender.as_deref(), &owner))
    }

    /// A name came or went (`old` to `new`, either empty).
    fn owner_changed(&self, name: &str, old: &str, new: &str, of_reader: bool) {
        self.emit(BUS, BUS_PATH, BUS, "NameOwnerChanged", &(name, old, new), |id, _| of_reader || self.allowed_reader(id));
    }

    fn incoming(&mut self, id: u64, m: Message) -> bool {
        let Some(p) = self.peers.get_mut(&id) else { return false };
        if m.data().len() > MAX_SIZE {
            eprintln!("a11y: {} sent {} bytes at once: put out", self.label(id), m.data().len());
            return false;
        }
        if p.since.elapsed() > Duration::from_secs(1) {
            (p.since, p.count) = (Instant::now(), 0);
        }
        p.count += 1;
        if p.count > FLOOD {
            eprintln!("a11y: {} sent over {FLOOD} messages in a second: put out", self.label(id));
            return false;
        }
        let hdr = m.header();
        if logging() {
            println!("a11y: {} {:?} {} {} {}.{} {}", self.label(id), m.message_type(), hdr.destination().map(|d| d.as_str()).unwrap_or("-"), hdr.path().map(|d| d.as_str()).unwrap_or("-"), hdr.interface().map(|d| d.as_str()).unwrap_or("-"), hdr.member().map(|d| d.as_str()).or(hdr.error_name().map(|e| e.as_str())).unwrap_or("-"), hdr.signature());
        }
        let hello = m.message_type() == Type::MethodCall && hdr.member().is_some_and(|n| n == "Hello");
        if self.peers[&id].name.is_empty() && !hello {
            eprintln!("a11y: {} spoke before its Hello: put out", self.label(id));
            return false;
        }
        if !m.data().fds().is_empty() {
            self.error(id, &hdr, BUS, NOT_SUPPORTED, "file descriptors are not passed on this bus");
            return true;
        }
        match m.message_type() {
            Type::MethodCall => self.call(id, &m),
            Type::MethodReturn | Type::Error => self.answered_by(id, &m),
            Type::Signal => self.signal(id, &m),
        }
        true
    }

    fn call(&mut self, id: u64, m: &Message) {
        let hdr = m.header();
        let destination = hdr.destination().map(|d| d.to_string());
        match destination.as_deref() {
            None | Some(BUS) => self.bus_call(id, m),
            Some(REGISTRY) | Some(OWN) => self.registry_call(id, m),
            Some(d) => {
                let Some(to) = self.names.get(d).copied() else {
                    return self.error(id, &hdr, BUS, "org.freedesktop.DBus.Error.ServiceUnknown", &format!("the name {d} is not on this bus"));
                };
                // A program calling into its own objects is not reading.
                if to != id {
                    match self.gate(id, m) {
                        Gate::Pass => (),
                        Gate::Held => return,
                        Gate::Deny => return self.error(id, &hdr, BUS, ACCESS_DENIED, "a program cannot read another program's accessibility tree"),
                    }
                }
                if self.peers[&id].calls >= MAX_CALLS {
                    return self.error(id, &hdr, BUS, LIMITS, "too many calls waiting for an answer");
                }
                let Ok(out) = relay(m, &hdr, &self.peers[&id].name, None, None) else {
                    return self.error(id, &hdr, BUS, INVALID_ARGS, "a call without a path or a member");
                };
                if !hdr.primary().flags().contains(Flags::NoReplyExpected) {
                    self.waiting.insert((to, out.primary_header().serial_num().get()), Waiter::Peer(id, hdr.primary().serial_num()));
                    self.peers.get_mut(&id).unwrap().calls += 1;
                }
                self.send(to, out);
            }
        }
    }

    /// Whether this connection may read: it is a program until it does
    /// what only a reader does (call into a program, register for events).
    fn gate(&mut self, id: u64, m: &Message) -> Gate {
        let allowed = &self.allowed;
        let p = self.peers.get_mut(&id).unwrap();
        match p.reader {
            Some(Reader::Allowed) => Gate::Pass,
            Some(Reader::Denied) => Gate::Deny,
            Some(Reader::Asking) => {
                p.held.push(m.clone());
                Gate::Held
            }
            None => {
                let named = names_of(&p.exe, &p.comm).iter().any(|n| allowed.contains(n));
                let ask = ASK.lock().unwrap();
                if named {
                    p.reader = Some(Reader::Allowed);
                } else if ask.is_some() {
                    p.reader = Some(Reader::Asking);
                    p.held.push(m.clone());
                } else {
                    p.reader = Some(Reader::Denied);
                }
                let (pid, reader) = (p.pid, p.reader);
                let label = self.label(id);
                match reader {
                    Some(Reader::Allowed) => {
                        println!("a11y: {label} reads the windows (session.conf allows it)");
                        self.snapshot();
                        Gate::Pass
                    }
                    Some(Reader::Asking) => {
                        println!("a11y: {label} wants to read the windows: the user is asked");
                        if let Some(f) = ask.as_ref() {
                            f(&Program { name: self.peers[&id].name.clone(), pid, exe: self.peers[&id].exe.clone(), root: None, reader: false });
                        }
                        self.ex.spawn(async move {
                            async_io::Timer::after(ASK_DEADLINE).await;
                            tell(Cmd::Answer(pid, false));
                        })
                        .detach();
                        Gate::Held
                    }
                    _ => {
                        let name = names_of(&self.peers[&id].exe, &self.peers[&id].comm).pop().unwrap_or_default();
                        eprintln!("a11y: {label} tried to read another program's tree: refused (`a11y reader {name}` in session.conf allows it)");
                        Gate::Deny
                    }
                }
            }
        }
    }

    /// The user answered about the reader of that process.
    fn answered(&mut self, pid: u32, allow: bool) {
        let asking: Vec<u64> = self.peers.iter().filter(|(_, p)| p.pid == pid && p.reader == Some(Reader::Asking)).map(|(&id, _)| id).collect();
        for id in asking {
            let p = self.peers.get_mut(&id).unwrap();
            p.reader = Some(if allow { Reader::Allowed } else { Reader::Denied });
            let held = std::mem::take(&mut p.held);
            if allow {
                if let Some(n) = names_of(&p.exe, &p.comm).pop() {
                    self.allowed.push(n);
                }
            }
            println!("a11y: {} {}", self.label(id), if allow { "allowed to read the windows" } else { "refused" });
            self.snapshot();
            for m in held {
                if allow {
                    self.call(id, &m);
                } else {
                    self.error(id, &m.header(), BUS, ACCESS_DENIED, "the user did not allow this program to read the windows");
                }
            }
        }
    }

    /// A reply, or an error, to a call it was sent: passed on only to the
    /// connection that made that call.
    fn answered_by(&mut self, id: u64, m: &Message) {
        let hdr = m.header();
        let Some(serial) = hdr.reply_serial() else { return };
        match self.waiting.remove(&(id, serial.get())) {
            Some(Waiter::Peer(from, their)) => {
                if let Some(p) = self.peers.get_mut(&from) {
                    p.calls = p.calls.saturating_sub(1);
                }
                let to = self.name_of(from).to_owned();
                match relay(m, &hdr, self.name_of(id), Some(their), Some(&to)) {
                    Ok(out) => self.send(from, out),
                    Err(e) => eprintln!("a11y: an answer from {} could not be passed on: {e}", self.label(id)),
                }
            }
            Some(Waiter::Own(tx, _)) => {
                let _ = tx.try_send(if m.message_type() == Type::Error {
                    Err(Error::Failed(hdr.error_name().map(|e| e.to_string()).unwrap_or_default(), m.body().deserialize::<(String,)>().map(|t| t.0).unwrap_or_default()))
                } else {
                    Ok(m.clone())
                });
            }
            // An answer nobody asked for: dropped.
            None => (),
        }
    }

    /// A program's signals go to pleamar-wm's reader and to the readers let
    /// in; a reader's, nowhere.
    fn signal(&mut self, id: u64, m: &Message) {
        if self.allowed_reader(id) {
            return;
        }
        let hdr = m.header();
        let Ok(out) = relay(m, &hdr, self.name_of(id), None, None) else { return };
        match hdr.destination().map(|d| d.to_string()) {
            Some(d) if d == OWN || d == REGISTRY => self.to_listeners(&out),
            Some(d) => {
                if let Some(to) = self.names.get(&d).copied().filter(|&to| self.allowed_reader(to)) {
                    self.send(to, out);
                }
            }
            None => {
                self.to_listeners(&out);
                for (&to, p) in &self.peers {
                    if p.reader == Some(Reader::Allowed) && self.wants(p, &out) {
                        self.send(to, out.clone());
                    }
                }
            }
        }
    }

    fn to_listeners(&mut self, m: &Message) {
        self.listeners.retain(|tx| !tx.is_closed());
        for tx in &self.listeners {
            let _ = tx.try_send(m.clone());
        }
    }

    fn own_call(&mut self, m: Message, reply: async_channel::Sender<Result<Message, Error>>) {
        // Calls whose callers stopped waiting.
        self.waiting.retain(|_, w| !matches!(w, Waiter::Own(tx, at) if tx.is_closed() || at.elapsed() > OWN_DEADLINE));
        let to = m.header().destination().and_then(|d| self.names.get(d.as_str()).copied()).filter(|&to| to != 0);
        let Some(to) = to else {
            let _ = reply.try_send(Err(Error::Gone));
            return;
        };
        self.waiting.insert((to, m.primary_header().serial_num().get()), Waiter::Own(reply, Instant::now()));
        self.send(to, m);
    }

    fn own_events(&mut self, events: Vec<String>, on: bool) {
        for e in events {
            let n = self.own_events.entry(e.clone()).or_insert(0);
            if on {
                *n += 1;
                if *n == 1 {
                    self.emit(OWN, REGISTRY_PATH, REGISTRY, "EventListenerRegistered", &(OWN, e.as_str(), Vec::<String>::new()), |_, _| true);
                }
            } else {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    self.own_events.remove(&e);
                    self.emit(OWN, REGISTRY_PATH, REGISTRY, "EventListenerDeregistered", &(OWN, e.as_str()), |_, _| true);
                }
            }
        }
    }

    /// A connection went: what it owned, what it was asked and what it asked.
    fn forget(&mut self, id: u64) {
        let Some(p) = self.peers.remove(&id) else { return };
        let reader = p.reader == Some(Reader::Allowed);
        let gone: Vec<(u64, u32)> = self.waiting.keys().filter(|(to, _)| *to == id).copied().collect();
        for key in gone {
            match self.waiting.remove(&key) {
                Some(Waiter::Peer(from, their)) => {
                    if let Some(f) = self.peers.get_mut(&from) {
                        f.calls = f.calls.saturating_sub(1);
                    }
                    let to = self.name_of(from).to_owned();
                    let built = zbus::message::Message::method_call(BUS_PATH, "Gone").and_then(|b| b.serial(their).build(&())).and_then(|call| Message::error(&call.header(), NO_REPLY).and_then(|b| b.sender(BUS)).and_then(|b| b.destination(to)).and_then(|b| b.build(&("the program went away before answering",))));
                    if let Ok(m) = built {
                        self.send(from, m);
                    }
                }
                Some(Waiter::Own(tx, _)) => {
                    let _ = tx.try_send(Err(Error::Gone));
                }
                None => (),
            }
        }
        self.waiting.retain(|_, w| !matches!(w, Waiter::Peer(from, _) if *from == id));
        for n in p.names.iter().chain(std::iter::once(&p.name)) {
            if self.names.get(n) == Some(&id) {
                self.names.remove(n);
                if !p.name.is_empty() {
                    self.owner_changed(n, &p.name, "", reader);
                }
            }
        }
        for e in &p.events {
            self.emit(OWN, REGISTRY_PATH, REGISTRY, "EventListenerDeregistered", &(p.name.as_str(), e.as_str()), |_, _| true);
        }
        if logging() {
            println!("a11y: {} (pid {}) left", p.name, p.pid);
        }
        self.snapshot();
        drop(p.conn);
    }

    /// The programs' roots, the desktop's children, in the order they came.
    fn children(&self) -> Vec<(String, OwnedObjectPath)> {
        let mut roots: Vec<(u64, &(String, String))> = self.peers.iter().filter_map(|(&k, p)| p.root.as_ref().map(|r| (k, r))).collect();
        roots.sort_by_key(|(k, _)| *k);
        roots.into_iter().filter_map(|(_, (n, p))| Some((n.clone(), OwnedObjectPath::try_from(p.as_str()).ok()?))).collect()
    }

    fn not_supported(&mut self, id: u64, hdr: &Header<'_>, from: &str) {
        let what = format!("{} {}.{}", hdr.path().map(|p| p.as_str()).unwrap_or("-"), hdr.interface().map(|i| i.as_str()).unwrap_or("-"), hdr.member().map(|m| m.as_str()).unwrap_or("-"));
        if self.unsupported.insert(what.clone()) {
            println!("a11y: {} called {what}, which this bus does not have", self.label(id));
        }
        self.error(id, hdr, from, if from == BUS { NOT_SUPPORTED } else { UNKNOWN_METHOD }, &format!("{what} is not on this bus"));
    }

    fn bus_call(&mut self, id: u64, m: &Message) {
        let hdr = m.header();
        let body = m.body();
        let arg = || body.deserialize::<(String,)>().map(|t| t.0).ok();
        if hdr.interface().is_some_and(|i| i != BUS) {
            return self.not_supported(id, &hdr, BUS);
        }
        match hdr.member().map(|m| m.as_str()).unwrap_or("") {
            "Hello" => {
                if !self.peers[&id].name.is_empty() {
                    return self.error(id, &hdr, BUS, "org.freedesktop.DBus.Error.Failed", "Hello was said already");
                }
                let name = format!(":1.{id}");
                self.peers.get_mut(&id).unwrap().name = name.clone();
                self.names.insert(name.clone(), id);
                self.reply(id, &hdr, BUS, &(name.as_str(),));
                if let Ok(s) = Message::signal(BUS_PATH, BUS, "NameAcquired").and_then(|b| b.sender(BUS)).and_then(|b| b.destination(name.as_str())).and_then(|b| b.build(&(name.as_str(),))) {
                    self.send(id, s);
                }
                self.owner_changed(&name, "", &name, false);
                self.snapshot();
            }
            "AddMatch" => {
                let Some(rule) = arg() else { return self.error(id, &hdr, BUS, INVALID_ARGS, "a match rule is a string") };
                if self.peers[&id].rules.len() >= MAX_RULES {
                    return self.error(id, &hdr, BUS, LIMITS, "too many match rules");
                }
                match Rule::parse(&rule) {
                    Some(r) => self.peers.get_mut(&id).unwrap().rules.push((rule, r)),
                    None => {
                        eprintln!("a11y: {} asked for a rule this bus cannot read: {rule}", self.label(id));
                        return self.error(id, &hdr, BUS, "org.freedesktop.DBus.Error.MatchRuleInvalid", "a rule is key='value' pairs");
                    }
                }
                self.reply(id, &hdr, BUS, &());
            }
            "RemoveMatch" => {
                let rule = arg().unwrap_or_default();
                let rules = &mut self.peers.get_mut(&id).unwrap().rules;
                if let Some(k) = rules.iter().position(|(r, _)| *r == rule) {
                    rules.remove(k);
                }
                self.reply(id, &hdr, BUS, &());
            }
            "GetNameOwner" => match arg().and_then(|n| self.visible(id, &n)) {
                Some(owner) => self.reply(id, &hdr, BUS, &(self.name_of(owner),)),
                None => self.error(id, &hdr, BUS, NO_OWNER, "no such name"),
            },
            "NameHasOwner" => self.reply(id, &hdr, BUS, &(arg().and_then(|n| self.visible(id, &n)).is_some(),)),
            "GetId" => self.reply(id, &hdr, BUS, &(self.guid.as_str(),)),
            // Nothing is started by name here.
            "ListActivatableNames" => self.reply(id, &hdr, BUS, &(Vec::<&str>::new(),)),
            "ListNames" => {
                let names: Vec<&str> = self.names.keys().filter(|n| self.visible(id, n).is_some()).map(String::as_str).collect();
                self.reply(id, &hdr, BUS, &(names,));
            }
            "GetConnectionUnixProcessID" | "GetConnectionCredentials" => {
                let Some(owner) = arg().and_then(|n| self.visible(id, &n)) else { return self.error(id, &hdr, BUS, NO_OWNER, "no such name") };
                let pid = if owner == 0 { std::process::id() } else { self.peers[&owner].pid };
                if hdr.member().is_some_and(|m| m == "GetConnectionUnixProcessID") {
                    self.reply(id, &hdr, BUS, &(pid,));
                } else {
                    // SAFETY: getuid cannot fail.
                    let creds = HashMap::from([("ProcessID", Value::U32(pid)), ("UnixUserID", Value::U32(unsafe { libc::getuid() }))]);
                    self.reply(id, &hdr, BUS, &(creds,));
                }
            }
            // A name of its own is a reader's (a screen reader's service).
            "RequestName" => {
                let Ok((name, _flags)) = body.deserialize::<(String, u32)>() else { return self.error(id, &hdr, BUS, INVALID_ARGS, "RequestName takes a name and flags") };
                if name.starts_with(':') || name == BUS || name == REGISTRY || zbus::names::WellKnownName::try_from(name.as_str()).is_err() {
                    return self.error(id, &hdr, BUS, INVALID_ARGS, "that name cannot be owned");
                }
                match self.gate(id, m) {
                    Gate::Pass => (),
                    Gate::Held => return,
                    Gate::Deny => return self.error(id, &hdr, BUS, ACCESS_DENIED, "only a reader owns a name on this bus"),
                }
                match self.names.get(&name).copied() {
                    Some(o) if o == id => self.reply(id, &hdr, BUS, &(4u32,)),
                    Some(_) => self.reply(id, &hdr, BUS, &(3u32,)),
                    None => {
                        self.names.insert(name.clone(), id);
                        let p = self.peers.get_mut(&id).unwrap();
                        p.names.push(name.clone());
                        let own = p.name.clone();
                        self.reply(id, &hdr, BUS, &(1u32,));
                        if let Ok(s) = Message::signal(BUS_PATH, BUS, "NameAcquired").and_then(|b| b.sender(BUS)).and_then(|b| b.destination(own.as_str())).and_then(|b| b.build(&(name.as_str(),))) {
                            self.send(id, s);
                        }
                        self.owner_changed(&name, "", &own, true);
                    }
                }
            }
            "ReleaseName" => {
                let name = arg().unwrap_or_default();
                match self.names.get(&name).copied() {
                    Some(o) if o == id && !name.starts_with(':') => {
                        self.names.remove(&name);
                        let p = self.peers.get_mut(&id).unwrap();
                        p.names.retain(|n| *n != name);
                        let own = p.name.clone();
                        self.reply(id, &hdr, BUS, &(1u32,));
                        self.owner_changed(&name, &own, "", true);
                    }
                    Some(_) => self.reply(id, &hdr, BUS, &(3u32,)),
                    None => self.reply(id, &hdr, BUS, &(2u32,)),
                }
            }
            _ => self.not_supported(id, &hdr, BUS),
        }
    }

    fn registry_call(&mut self, id: u64, m: &Message) {
        let hdr = m.header();
        let body = m.body();
        let path = hdr.path().map(|p| p.as_str()).unwrap_or("");
        let iface = hdr.interface().map(|i| i.as_str()).unwrap_or("");
        let member = hdr.member().map(|m| m.as_str()).unwrap_or("");
        let root = || (OWN, OwnedObjectPath::try_from(ROOT).unwrap());
        match (path, iface, member) {
            (ROOT, "org.a11y.atspi.Socket", "Embed") => {
                let plug = body.deserialize::<((String, OwnedObjectPath),)>().map(|t| t.0).or_else(|_| body.deserialize::<(String, OwnedObjectPath)>());
                let Ok((_, plug)) = plug else { return self.error(id, &hdr, OWN, INVALID_ARGS, "Embed takes (so)") };
                let p = self.peers.get_mut(&id).unwrap();
                // Its own tree only, whatever name it gave.
                p.root = Some((p.name.clone(), plug.to_string()));
                println!("a11y: {} embedded its tree", self.label(id));
                self.reply(id, &hdr, OWN, &(root(),));
                self.snapshot();
            }
            (ROOT, "org.a11y.atspi.Socket", "Unembed") => {
                self.peers.get_mut(&id).unwrap().root = None;
                self.reply(id, &hdr, OWN, &());
                self.snapshot();
            }
            (REGISTRY_PATH, REGISTRY, "GetRegisteredEvents") => {
                let mut events: Vec<(String, String)> = self.own_events.keys().map(|e| (OWN.to_owned(), e.clone())).collect();
                for p in self.peers.values().filter(|p| p.reader == Some(Reader::Allowed)) {
                    events.extend(p.events.iter().map(|e| (p.name.clone(), e.clone())));
                }
                self.reply(id, &hdr, OWN, &(events,));
            }
            (REGISTRY_PATH, REGISTRY, "RegisterEvent" | "DeregisterEvent") => {
                // `s`, `sas`, `sass` or `ss` by version: the event first.
                let event = body.deserialize::<(String,)>().map(|t| t.0).or_else(|_| body.deserialize::<zbus::zvariant::Structure>().map_err(|_| ()).and_then(|s| match s.fields().first() {
                    Some(Value::Str(e)) => Ok(e.to_string()),
                    _ => Err(()),
                }));
                let Ok(event) = event else { return self.error(id, &hdr, OWN, INVALID_ARGS, "an event is a string") };
                match self.gate(id, m) {
                    Gate::Pass => (),
                    Gate::Held => return,
                    Gate::Deny => return self.error(id, &hdr, OWN, ACCESS_DENIED, "only a reader registers for events"),
                }
                let p = self.peers.get_mut(&id).unwrap();
                let name = p.name.clone();
                if member == "RegisterEvent" {
                    p.events.push(event.clone());
                    self.emit(OWN, REGISTRY_PATH, REGISTRY, "EventListenerRegistered", &(name.as_str(), event.as_str(), Vec::<String>::new()), |_, _| true);
                } else if let Some(k) = p.events.iter().position(|e| *e == event) {
                    p.events.remove(k);
                    self.emit(OWN, REGISTRY_PATH, REGISTRY, "EventListenerDeregistered", &(name.as_str(), event.as_str()), |_, _| true);
                }
                self.reply(id, &hdr, OWN, &());
            }
            // Key events from programs are not taken: nobody listens.
            (DEC_PATH, "org.a11y.atspi.DeviceEventController", "GetKeystrokeListeners") => {
                self.reply(id, &hdr, OWN, &(Vec::<(String, OwnedObjectPath, u32, u32, Vec<(i32, i32, String, i32)>, u32, (bool, bool, bool))>::new(),))
            }
            (DEC_PATH, "org.a11y.atspi.DeviceEventController", "GetDeviceEventListeners") => self.reply(id, &hdr, OWN, &(Vec::<(String, OwnedObjectPath, u32)>::new(),)),
            (DEC_PATH, "org.a11y.atspi.DeviceEventController", "NotifyListenersSync" | "RegisterKeystrokeListener" | "RegisterDeviceEventListener") => self.reply(id, &hdr, OWN, &(false,)),
            (DEC_PATH, "org.a11y.atspi.DeviceEventController", "NotifyListenersAsync" | "DeregisterKeystrokeListener" | "DeregisterDeviceEventListener") => self.reply(id, &hdr, OWN, &()),
            // The desktop: its children are the programs' trees, for readers.
            (ROOT, "org.a11y.atspi.Accessible", "GetChildren" | "GetChildAtIndex") => {
                match self.gate(id, m) {
                    Gate::Pass => (),
                    Gate::Held => return,
                    Gate::Deny => return self.error(id, &hdr, OWN, ACCESS_DENIED, "only a reader lists the programs"),
                }
                let all = self.children();
                if member == "GetChildren" {
                    self.reply(id, &hdr, OWN, &(all,));
                } else {
                    let k = body.deserialize::<(i32,)>().map(|t| t.0).unwrap_or(-1);
                    let child = usize::try_from(k).ok().and_then(|k| all.get(k).cloned()).unwrap_or((OWN.to_owned(), OwnedObjectPath::try_from(NULL).unwrap()));
                    self.reply(id, &hdr, OWN, &(child,));
                }
            }
            (ROOT, "org.a11y.atspi.Accessible", "GetRole") => self.reply(id, &hdr, OWN, &(14u32,)),
            (ROOT, "org.a11y.atspi.Accessible", "GetRoleName" | "GetLocalizedRoleName") => self.reply(id, &hdr, OWN, &("desktop frame",)),
            (ROOT, "org.a11y.atspi.Accessible", "GetState") => self.reply(id, &hdr, OWN, &(vec![0u32, 0u32],)),
            (ROOT, "org.a11y.atspi.Accessible", "GetInterfaces") => self.reply(id, &hdr, OWN, &(vec!["org.a11y.atspi.Accessible", "org.a11y.atspi.Component"],)),
            (ROOT, "org.a11y.atspi.Accessible", "GetAttributes") => self.reply(id, &hdr, OWN, &(HashMap::<String, String>::new(),)),
            (ROOT, "org.freedesktop.DBus.Properties", "Get" | "GetAll") => {
                let count = self.peers.values().filter(|p| p.root.is_some()).count() as i32;
                let null = || Value::from((OWN.to_owned(), OwnedObjectPath::try_from(NULL).unwrap()));
                let props = HashMap::from([("Name", Value::from("main")), ("Description", Value::from("")), ("ChildCount", Value::I32(count)), ("Parent", null()), ("Locale", Value::from("")), ("AccessibleId", Value::from(""))]);
                if member == "GetAll" {
                    self.reply(id, &hdr, OWN, &(props,));
                } else {
                    let name = body.deserialize::<(String, String)>().map(|t| t.1).unwrap_or_default();
                    match props.get(name.as_str()) {
                        Some(v) => self.reply(id, &hdr, OWN, &(v,)),
                        None => self.error(id, &hdr, OWN, INVALID_ARGS, "no such property"),
                    }
                }
            }
            // A reader's library asks every connection for its objects at once.
            ("/org/a11y/atspi/cache", "org.a11y.atspi.Cache", "GetItems") => {
                type Item = ((String, OwnedObjectPath), (String, OwnedObjectPath), (String, OwnedObjectPath), i32, i32, Vec<String>, String, u32, String, Vec<u32>);
                self.reply(id, &hdr, OWN, &(Vec::<Item>::new(),))
            }
            (_, "org.freedesktop.DBus.Peer", "Ping") => self.reply(id, &hdr, OWN, &()),
            _ => self.not_supported(id, &hdr, OWN),
        }
    }
}

fn short(exe: &str) -> &str {
    exe.rsplit('/').next().unwrap_or(exe)
}

/// What a reader may be named by in session.conf: its executable's name,
/// and its process's (a script's, and Nix's `.orca-wrapped` as `orca`).
fn names_of(exe: &str, comm: &str) -> Vec<String> {
    let mut out = vec![short(exe).to_owned()];
    let plain = comm.trim_start_matches('.').trim_end_matches("-wrapped");
    for n in [comm, plain] {
        if !n.is_empty() && !out.iter().any(|o| o == n) {
            out.push(n.to_owned());
        }
    }
    out
}

/// A match rule (`AddMatch`), read as dbus-daemon reads it: spaces between
/// its pairs, and `argNpath` values that are not object paths (`add/`), which
/// at-spi's readers use and zbus's rules refuse.
#[derive(Default, Debug)]
struct Rule {
    kind: Option<Type>,
    sender: Option<String>,
    interface: Option<String>,
    member: Option<String>,
    path: Option<String>,
    path_namespace: Option<String>,
    destination: Option<String>,
    args: Vec<(usize, String)>,
    arg_paths: Vec<(usize, String)>,
    arg0namespace: Option<String>,
}

impl Rule {
    fn parse(text: &str) -> Option<Rule> {
        let mut r = Rule::default();
        let mut rest = text.trim();
        while !rest.is_empty() {
            let (key, after) = rest.split_once('=')?;
            let key = key.trim();
            // The value: quoted parts and `\'` outside them, up to a comma.
            let (mut value, mut chars) = (String::new(), after.char_indices().peekable());
            let mut end = after.len();
            let mut quoted = false;
            while let Some((k, c)) = chars.next() {
                match c {
                    '\'' => quoted = !quoted,
                    '\\' if !quoted && chars.peek().is_some_and(|(_, n)| *n == '\'') => {
                        chars.next();
                        value.push('\'');
                    }
                    ',' if !quoted => {
                        end = k + 1;
                        break;
                    }
                    c => value.push(c),
                }
            }
            if quoted {
                return None;
            }
            rest = after[end.min(after.len())..].trim_start();
            match key {
                "type" => {
                    r.kind = Some(match value.as_str() {
                        "signal" => Type::Signal,
                        "method_call" => Type::MethodCall,
                        "method_return" => Type::MethodReturn,
                        "error" => Type::Error,
                        _ => return None,
                    })
                }
                "sender" => r.sender = Some(value),
                "interface" => r.interface = Some(value),
                "member" => r.member = Some(value),
                "path" => r.path = Some(value),
                "path_namespace" => r.path_namespace = Some(value),
                "destination" => r.destination = Some(value),
                "arg0namespace" => r.arg0namespace = Some(value),
                "eavesdrop" => (),
                k => {
                    let n = k.strip_prefix("arg")?;
                    let (n, path) = n.strip_suffix("path").map_or((n, false), |n| (n, true));
                    let n: usize = n.parse().ok().filter(|n| *n < 64)?;
                    if path { r.arg_paths.push((n, value)) } else { r.args.push((n, value)) }
                }
            }
        }
        Some(r)
    }

    /// Whether it takes that message, from `sender`; `owner` says who owns a
    /// well-known name.
    fn matches<'a>(&self, m: &Message, sender: Option<&str>, owner: &dyn Fn(&str) -> Option<&'a str>) -> bool {
        let hdr = m.header();
        let same = |want: &Option<String>, got: Option<&str>| want.as_ref().is_none_or(|w| Some(w.as_str()) == got);
        if self.kind.is_some_and(|k| k != m.message_type()) {
            return false;
        }
        if let Some(s) = &self.sender {
            let s = if s.starts_with(':') || s == BUS { Some(s.as_str()) } else { owner(s) };
            if s.is_none() || s != sender {
                return false;
            }
        }
        if !same(&self.interface, hdr.interface().map(|i| i.as_str()))
            || !same(&self.member, hdr.member().map(|i| i.as_str()))
            || !same(&self.path, hdr.path().map(|i| i.as_str()))
            || !same(&self.destination, hdr.destination().map(|i| i.as_str()))
        {
            return false;
        }
        if let Some(ns) = &self.path_namespace {
            let p = hdr.path().map(|p| p.as_str()).unwrap_or("");
            if !(ns == "/" || p == ns || p.strip_prefix(ns.as_str()).is_some_and(|r| r.starts_with('/'))) {
                return false;
            }
        }
        if self.args.is_empty() && self.arg_paths.is_empty() && self.arg0namespace.is_none() {
            return true;
        }
        let body = m.body();
        let Ok(body) = body.deserialize::<zbus::zvariant::Structure>() else { return false };
        let arg = |n: usize| match body.fields().get(n) {
            Some(Value::Str(s)) => Some(s.as_str().to_owned()),
            Some(Value::ObjectPath(p)) => Some(p.as_str().to_owned()),
            _ => None,
        };
        let path_like = |want: &str, got: &str| want == got || (want.ends_with('/') && got.starts_with(want)) || (got.ends_with('/') && want.starts_with(got));
        self.args.iter().all(|(n, want)| matches!(body.fields().get(*n), Some(Value::Str(s)) if s.as_str() == want))
            && self.arg_paths.iter().all(|(n, want)| arg(*n).is_some_and(|got| path_like(want, &got)))
            && self.arg0namespace.as_ref().is_none_or(|ns| arg(0).is_some_and(|a| a == *ns || a.strip_prefix(ns.as_str()).is_some_and(|r| r.starts_with('.'))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_is_named_by_its_program_or_its_script() {
        assert_eq!(names_of("/usr/bin/python3.12", "orca"), ["python3.12", "orca"]);
        assert_eq!(names_of("/nix/store/x/bin/python3.12", ".orca-wrapped"), ["python3.12", ".orca-wrapped", "orca"]);
        assert_eq!(names_of("/usr/bin/orca", "orca"), ["orca"]);
    }

    #[test]
    fn rules_as_readers_write_them() {
        let none = |_: &str| None;
        let changed = Message::signal(ROOT, "org.a11y.atspi.Event.Object", "ChildrenChanged").unwrap().sender(":1.4").unwrap().build(&("add/system", 0i32, 0i32, Value::I32(0), HashMap::<String, Value>::new())).unwrap();
        let r = Rule::parse("type='signal',interface='org.a11y.atspi.Event.Object',member='ChildrenChanged',arg0path='add/'").unwrap();
        assert!(r.matches(&changed, Some(":1.4"), &none));
        assert!(!Rule::parse("type='signal',member='ChildrenChanged',arg0path='remove/'").unwrap().matches(&changed, Some(":1.4"), &none));
        let owner = Message::signal(BUS_PATH, BUS, "NameOwnerChanged").unwrap().sender(BUS).unwrap().build(&(":1.7", ":1.7", "")).unwrap();
        let r = Rule::parse("type='signal', interface='org.freedesktop.DBus', member='NameOwnerChanged'").unwrap();
        assert!(r.matches(&owner, Some(BUS), &none), "spaces between pairs");
        assert!(Rule::parse("sender='org.freedesktop.DBus',arg0=':1.7'").unwrap().matches(&owner, Some(BUS), &none));
        assert!(!Rule::parse("arg0=':1.8'").unwrap().matches(&owner, Some(BUS), &none));
        let registry = |n: &str| (n == REGISTRY).then_some(OWN);
        let registered = Message::signal(REGISTRY_PATH, REGISTRY, "EventListenerRegistered").unwrap().sender(OWN).unwrap().build(&(OWN, "focus:", Vec::<String>::new())).unwrap();
        assert!(Rule::parse("type='signal',sender='org.a11y.atspi.Registry',interface='org.a11y.atspi.Registry'").unwrap().matches(&registered, Some(OWN), &registry));
        assert!(!Rule::parse("sender='org.a11y.atspi.Registry'").unwrap().matches(&changed, Some(":1.4"), &registry));
        assert!(Rule::parse("path_namespace='/org/a11y'").unwrap().matches(&changed, Some(":1.4"), &none));
        assert!(Rule::parse("type='nonsense'").is_none() && Rule::parse("member='open").is_none());
    }

    struct Thing;

    #[zbus::interface(name = "org.a11y.atspi.Accessible")]
    impl Thing {
        fn get_role(&self) -> u32 {
            43
        }
    }

    fn denied(r: zbus::Result<Message>) -> bool {
        matches!(r, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == ACCESS_DENIED)
    }

    #[test]
    fn programs_publish_and_only_pleamar_wm_reads() {
        let path = std::env::temp_dir().join(format!("pleamar-a11y-test-{}", std::process::id()));
        let address = broker_at(path.to_str().unwrap()).unwrap();
        let connect = || zbus::blocking::connection::Builder::address(address.as_str()).unwrap();
        let program = connect().serve_at(ROOT, Thing).unwrap().build().unwrap();
        let me = program.unique_name().unwrap().to_string();
        let r = program.call_method(Some(REGISTRY), ROOT, Some("org.a11y.atspi.Socket"), "Embed", &((me.as_str(), OwnedObjectPath::try_from(ROOT).unwrap()),)).unwrap();
        assert_eq!(r.body().deserialize::<((String, OwnedObjectPath),)>().unwrap().0 .0, OWN, "the registry's root answers");
        let listener = listen(&["object:state-changed"]).unwrap();

        // pleamar-wm reads it, by its process.
        let mut found = None;
        for _ in 0..100 {
            found = program_of(std::process::id());
            if found.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let found = found.expect("the program is known by its pid");
        assert_eq!(found.root, Some((me.clone(), ROOT.to_owned())));
        let r = call_blocking(&found.name, ROOT, "org.a11y.atspi.Accessible", "GetRole", &(), Duration::from_secs(2)).unwrap();
        assert_eq!(r.body().deserialize::<(u32,)>().unwrap().0, 43);
        let events = program.call_method(Some(REGISTRY), REGISTRY_PATH, Some(REGISTRY), "GetRegisteredEvents", &()).unwrap();
        assert!(events.body().deserialize::<(Vec<(String, String)>,)>().unwrap().0.contains(&(OWN.to_owned(), "object:state-changed".to_owned())));
        program.emit_signal(None::<&str>, ROOT, "org.a11y.atspi.Event.Object", "StateChanged", &("focused", 1i32, 0i32, Value::I32(0), HashMap::<String, Value>::new())).unwrap();
        let s = futures_lite::future::block_on(futures_lite::future::or(async { listener.signals.recv().await.ok() }, async {
            async_io::Timer::after(Duration::from_secs(2)).await;
            None
        }));
        assert_eq!(s.expect("its events come to pleamar-wm").header().sender().unwrap().as_str(), me, "with who sent them");

        // Another program cannot, nor see it.
        let other = connect().build().unwrap();
        assert!(denied(other.call_method(Some(me.as_str()), ROOT, Some("org.a11y.atspi.Accessible"), "GetRole", &())));
        assert!(denied(other.call_method(Some(REGISTRY), ROOT, Some("org.a11y.atspi.Accessible"), "GetChildren", &())));
        let names = other.call_method(Some(BUS), BUS_PATH, Some(BUS), "ListNames", &()).unwrap().body().deserialize::<(Vec<String>,)>().unwrap().0;
        assert!(!names.contains(&me) && names.contains(&REGISTRY.to_owned()));
        let r = other.call_method(Some(BUS), BUS_PATH, Some(BUS), "GetConnectionUnixProcessID", &(me.as_str(),));
        assert!(matches!(r, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == NO_OWNER));
        assert!(matches!(other.call_method(Some(BUS), BUS_PATH, Some(BUS), "StartServiceByName", &("x.y", 0u32)), Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == NOT_SUPPORTED));

        // A reader the user allows, once asked: its call held, then let through.
        on_ask(|p| answer(p.pid, true));
        let reader = connect().build().unwrap();
        let r = reader.call_method(Some(me.as_str()), ROOT, Some("org.a11y.atspi.Accessible"), "GetRole", &()).unwrap();
        assert_eq!(r.body().deserialize::<(u32,)>().unwrap().0, 43);
        let names = reader.call_method(Some(BUS), BUS_PATH, Some(BUS), "ListNames", &()).unwrap().body().deserialize::<(Vec<String>,)>().unwrap().0;
        assert!(names.contains(&me));
        assert!(denied(other.call_method(Some(me.as_str()), ROOT, Some("org.a11y.atspi.Accessible"), "GetRole", &())), "a refused one stays refused");

        // Too large a message puts its sender out.
        let big = connect().build().unwrap();
        assert!(big.call_method(Some(REGISTRY), REGISTRY_PATH, Some(REGISTRY), "RegisterEvent", &("x".repeat(MAX_SIZE + 1),)).is_err());
        drop(program);
        let gone = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            program_of(std::process::id()).is_none()
        });
        assert!(gone, "a program that leaves is forgotten");
        assert_eq!(call_blocking(&me, ROOT, "org.a11y.atspi.Accessible", "GetRole", &(), Duration::from_secs(2)).unwrap_err(), Error::Gone);
        let _ = std::fs::remove_file(path);
    }
}
