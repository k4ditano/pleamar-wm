//! Same-user local IPC. Idle listeners sleep in the kernel, with bounded I/O
//! and one serialized command in flight on the desktop thread.
use super::*;
use std::{fs::{File, OpenOptions}, os::windows::{fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle}}, sync::{Arc, mpsc, atomic::{AtomicBool, Ordering}}};
use windows::Win32::{Security::{*, Authorization::*}, Storage::FileSystem::*, System::{IO::*, Pipes::*, RemoteDesktop::*}};

const LIMIT: usize = 65536;
const TIMEOUT: Duration = Duration::from_secs(15);

pub(super) struct Event(OwnedHandle);
impl Event {
    pub fn new() -> Result<Self> {
        let handle = unsafe { CreateEventW(None, true, false, None) }?;
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle.0) }))
    }
    pub fn handle(&self) -> HANDLE { HANDLE(self.0.as_raw_handle()) }
    pub fn signal(&self) { let _ = unsafe { SetEvent(self.handle()) }; }
    pub fn reset(&self) { let _ = unsafe { ResetEvent(self.handle()) }; }
    fn signaled(&self) -> bool { unsafe { WaitForSingleObject(self.handle(), 0) == WAIT_OBJECT_0 } }
}

fn user_sid() -> Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }?;
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let mut needed = 0;
    let _ = unsafe { GetTokenInformation(HANDLE(token.as_raw_handle()), TokenUser, None, 0, &mut needed) };
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    unsafe { GetTokenInformation(HANDLE(token.as_raw_handle()), TokenUser, Some(buffer.as_mut_ptr() as _), needed, &mut needed) }?;
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }?;
    let value = unsafe { text.to_string() };
    let _ = unsafe { LocalFree(Some(HLOCAL(text.0 as _))) };
    Ok(value?)
}

#[derive(Clone)]
pub(super) struct Endpoint { path: String, sid: String }
impl Endpoint {
    pub fn new(namespace: &str) -> Result<Self> {
        if namespace.len() > 64 || namespace.bytes().any(|b| !b.is_ascii_alphanumeric() && !b"-_".contains(&b)) {
            return Err("invalid WM namespace".into());
        }
        let sid = user_sid()?;
        let mut session = 0;
        unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }?;
        Ok(Self { path: format!(r"\\.\pipe\pleamar-wm-{sid}-{session}-{namespace}"), sid })
    }
    pub fn current() -> Result<Self> { Self::new(&std::env::var("PLEAMAR_WM_NAMESPACE").unwrap_or_default()) }
    pub fn service(mut self, name: &str) -> Result<Self> {
        if !matches!(name, "agent" | "agent-control") { return Err("unknown WM service".into()); }
        // A dot is forbidden in user namespaces, so these cannot alias a layout session.
        self.path.push('.'); self.path.push_str(name); Ok(self)
    }
    pub fn exists(&self) -> Result<bool> {
        let path: Vec<u16> = self.path.encode_utf16().chain([0]).collect();
        if unsafe { WaitNamedPipeW(PCWSTR(path.as_ptr()), 1) }.as_bool() { return Ok(true); }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(2) => Ok(false),
            Some(121 | 231) => Ok(true), // Existing, busy pipe: never fall back to unguarded look.
            _ => Err(error.into()),
        }
    }
    fn bind(&self) -> Result<File> {
        let descriptor: Vec<u16> = format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", self.sid).encode_utf16().chain([0]).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(descriptor.as_ptr()), SDDL_REVISION_1, &mut sd, None) }?;
        let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
        let path: Vec<u16> = self.path.encode_utf16().chain([0]).collect();
        let handle = unsafe { CreateNamedPipeW(PCWSTR(path.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1, LIMIT as u32, LIMIT as u32, 1000, Some(&attributes)) };
        let error = std::io::Error::last_os_error();
        let _ = unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        if handle.is_invalid() { return Err(format!("WM session already running or pipe unavailable: {error}").into()); }
        Ok(unsafe { File::from_raw_handle(handle.0) })
    }
    pub fn ask(&self, command: &str) -> Result<Value> { self.ask_mode(command, false) }
    pub fn ask_with_focus(&self, command: &str) -> Result<Value> { self.ask_mode(command, true) }
    fn ask_mode(&self, command: &str, foreground: bool) -> Result<Value> {
        if command.len() > LIMIT - 128 { return Err("WM command exceeds the protocol limit".into()); }
        let until = Instant::now() + TIMEOUT;
        let file = loop {
            match OpenOptions::new().read(true).write(true).custom_flags(FILE_FLAG_OVERLAPPED.0).open(&self.path) {
                Ok(file) => break file,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) && Instant::now() < until =>
                    std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(format!("WM session unavailable: {e}").into()),
            }
        };
        if foreground {
            // Only an explicit focus command forwards the caller's foreground
            // eligibility, and only to the actual connected server, never ASFW_ANY.
            let mut pid = 0;
            unsafe { GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut pid) }?;
            if pid == 0 { return Err("native focus server has no process identity".into()); }
            let _ = unsafe { AllowSetForegroundWindow(pid) };
        }
        let cancel = Event::new()?;
        send(&file, &cancel, &json!({"version":1,"command":command}), until)?;
        let answer = receive(&file, &cancel, until)?;
        transfer(&file, &cancel, &mut [1], true, until)?;
        if answer["ok"] != true { return Err(answer["error"].as_str().unwrap_or("invalid WM response").to_owned().into()); }
        Ok(answer["result"].clone())
    }
}

fn operation(file: &File, cancel: &Event, until: Option<Instant>, start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>) -> Result<usize> {
    let event = Event::new()?;
    let mut overlap = OVERLAPPED { hEvent: event.handle(), ..Default::default() };
    let handle = HANDLE(file.as_raw_handle());
    match start(&mut overlap) {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => return Ok(0),
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            let ms = until.map(|t| t.saturating_duration_since(Instant::now()).as_millis().min(u32::MAX as u128 - 1) as u32).unwrap_or(INFINITE);
            let wait = unsafe { WaitForMultipleObjects(&[cancel.handle(), event.handle()], false, ms) };
            if wait != WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                // OVERLAPPED and its buffer must outlive cancellation completion.
                let _ = unsafe { CancelIoEx(handle, Some(&overlap)) };
                let mut bytes = 0;
                let _ = unsafe { GetOverlappedResult(handle, &overlap, &mut bytes, true) };
                return Err("WM pipe cancelled or timed out".into());
            }
        }
        Err(e) => return Err(e.into()),
    }
    let mut bytes = 0;
    unsafe { GetOverlappedResult(handle, &overlap, &mut bytes, false) }?;
    Ok(bytes as usize)
}

fn transfer(file: &File, cancel: &Event, bytes: &mut [u8], write: bool, until: Instant) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if Instant::now() >= until || cancel.signaled() { return Err("WM pipe transfer cancelled or timed out".into()); }
        let count = operation(file, cancel, Some(until), |overlap| unsafe {
            if write { WriteFile(HANDLE(file.as_raw_handle()), Some(&bytes[offset..]), None, Some(overlap)) }
            else { ReadFile(HANDLE(file.as_raw_handle()), Some(&mut bytes[offset..]), None, Some(overlap)) }
        })?;
        if count == 0 { return Err("WM pipe closed during transfer".into()); }
        offset += count;
    }
    Ok(())
}

fn send(file: &File, cancel: &Event, value: &Value, until: Instant) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > LIMIT { return Err("WM response exceeds the protocol limit".into()); }
    transfer(file, cancel, &mut (bytes.len() as u32).to_le_bytes(), true, until)?;
    transfer(file, cancel, &mut bytes, true, until)
}

fn receive(file: &File, cancel: &Event, until: Instant) -> Result<Value> {
    let mut header = [0u8; 4];
    transfer(file, cancel, &mut header, false, until)?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > LIMIT { return Err("invalid WM protocol frame length".into()); }
    let mut bytes = vec![0u8; length];
    transfer(file, cancel, &mut bytes, false, until)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) struct Request {
    pub command: String,
    pub expired: Arc<AtomicBool>,
    reply: mpsc::Sender<Value>,
    done: mpsc::Receiver<()>,
}
impl Request {
    pub fn finish(self, result: Result<Value>) {
        let answer = match result { Ok(value) => json!({"ok":true,"result":value}), Err(e) => json!({"ok":false,"error":e.to_string()}) };
        let _ = self.reply.send(answer);
        // In particular, quit is delivered before the listener is torn down.
        let _ = self.done.recv_timeout(Duration::from_secs(2));
    }
}

pub(super) struct Server {
    pub wake: Arc<Event>,
    pub requests: mpsc::Receiver<Request>,
    stop: Arc<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
}
struct ListenerExit(Arc<Event>);
impl Drop for ListenerExit { fn drop(&mut self) { self.0.signal(); } }
impl Server {
    pub fn start(endpoint: Endpoint) -> Result<Self> {
        let file = endpoint.bind()?;
        let wake = Arc::new(Event::new()?);
        let stop = Arc::new(Event::new()?);
        let (tx, requests) = mpsc::sync_channel(1);
        let (notify, cancel) = (wake.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let _finished = ListenerExit(cancel.clone());
            while !cancel.signaled() {
                if let Err(error) = operation(&file, &cancel, None, |overlap| unsafe { ConnectNamedPipe(HANDLE(file.as_raw_handle()), Some(overlap)) }) {
                    if cancel.signaled() { break; }
                    if error.downcast_ref::<windows::core::Error>().is_some_and(|e| e.code() == ERROR_NO_DATA.to_hresult()) {
                        let _ = unsafe { DisconnectNamedPipe(HANDLE(file.as_raw_handle())) };
                        continue;
                    }
                    eprintln!("wm · command listener: {error}");
                    break;
                }
                let handle = HANDLE(file.as_raw_handle());
                let exchange = || -> Result<()> {
                    let request = receive(&file, &cancel, Instant::now() + Duration::from_secs(2))?;
                    let command = request["command"].as_str().ok_or("missing WM command")?;
                    if request["version"] != 1 { return Err("unknown WM protocol version".into()); }
                    let (reply, answer) = mpsc::channel();
                    let (finished, done) = mpsc::channel();
                    let expired = Arc::new(AtomicBool::new(false));
                    tx.send(Request { command:command.into(), expired:expired.clone(), reply, done })?;
                    notify.signal();
                    let until = Instant::now() + TIMEOUT;
                    loop {
                        match answer.recv_timeout(Duration::from_millis(50)) {
                            Ok(value) => {
                                let sent = send(&file, &cancel, &value, Instant::now() + Duration::from_secs(2));
                                if sent.is_ok() { let _ = transfer(&file, &cancel, &mut [0], false, Instant::now() + Duration::from_secs(1)); }
                                let _ = finished.send(());
                                return sent;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < until && !cancel.signaled() => {}
                            _ => { expired.store(true, Ordering::Release); return Err("WM command expired".into()); }
                        }
                    }
                };
                let _ = exchange();
                let _ = unsafe { DisconnectNamedPipe(handle) };
            }
        });
        Ok(Self { wake, requests, stop, thread:Some(thread) })
    }
    pub fn running(&self) -> bool { self.thread.as_ref().is_some_and(|t| !t.is_finished()) }
    pub fn stopped(&self) -> HANDLE { self.stop.handle() }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.signal();
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint() -> Endpoint {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        Endpoint::new(&format!("test-{}-{nonce}", std::process::id())).unwrap()
    }
    #[test]
    fn agent_endpoints_do_not_alias_layout_namespaces() {
        let layout=endpoint();
        let input=layout.clone().service("agent").unwrap();
        let control=layout.clone().service("agent-control").unwrap();
        assert_ne!(input.path,control.path);assert_ne!(input.path,layout.path);
        assert!(!input.exists().unwrap());
        let server=Server::start(input.clone()).unwrap();
        assert!(input.exists().unwrap());
        drop(server);assert!(!input.exists().unwrap());
        assert!(layout.service("arbitrary").is_err());
    }
    #[test]
    fn rejects_invalid_names_and_oversized_commands() {
        for name in ["bad/name", "..", "bad\\name", "bad\nname", &"a".repeat(65)] { assert!(Endpoint::new(name).is_err()); }
        assert!(endpoint().ask(&"x".repeat(LIMIT)).is_err());
    }
    #[test]
    fn pipe_roundtrip_errors_single_owner_and_shutdown() {
        let endpoint = endpoint();
        let server = Server::start(endpoint.clone()).unwrap();
        assert!(Server::start(endpoint.clone()).is_err());
        for fail in [false,true] {
            let client = endpoint.clone();
            let worker = std::thread::spawn(move || client.ask("status España").map_err(|e|e.to_string()));
            let request = server.requests.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(request.command,"status España");
            request.finish(if fail { Err("native operation refused".into()) } else { Ok(json!({"title":"café ñ"})) });
            let result = worker.join().unwrap();
            if fail { assert_eq!(result.unwrap_err(),"native operation refused"); }
            else { assert_eq!(result.unwrap()["title"],"café ñ"); }
        }
        let before = Instant::now();
        drop(server);
        assert!(before.elapsed() < Duration::from_secs(5));
        let _rebound = Server::start(endpoint).unwrap();
    }
    #[test]
    fn malformed_client_does_not_kill_the_listener() {
        let endpoint = endpoint();
        let server = Server::start(endpoint.clone()).unwrap();
        let cancel = Event::new().unwrap();
        let file = OpenOptions::new().read(true).write(true).custom_flags(FILE_FLAG_OVERLAPPED.0).open(&endpoint.path).unwrap();
        transfer(&file,&cancel,&mut (LIMIT as u32 + 1).to_le_bytes(),true,Instant::now()+Duration::from_secs(2)).unwrap();
        assert!(receive(&file,&cancel,Instant::now()+Duration::from_secs(3)).is_err());
        drop(file);
        let worker = std::thread::spawn(move || endpoint.ask("status").map_err(|e|e.to_string()));
        let request = server.requests.recv_timeout(Duration::from_secs(5)).unwrap();
        request.finish(Ok(json!({"alive":true})));
        assert_eq!(worker.join().unwrap().unwrap()["alive"],true);
        assert!(server.running());
    }
}
