//! Explicit, short-lived foreground input. The compositor's input remains shared.
use super::*;
use pleamar::windows_desktop::{Cancellation, Session, Value as DesktopValue};
use std::{sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, thread,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle}};

struct Process { handle: OwnedHandle, created: u64 }
impl Process {
    fn open(pid: u32) -> Result<Self> {
        let handle=unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) }?;
        let handle=unsafe { OwnedHandle::from_raw_handle(handle.0) };
        let (mut created,mut exited,mut kernel,mut user)=(FILETIME::default(),FILETIME::default(),FILETIME::default(),FILETIME::default());
        unsafe { GetProcessTimes(HANDLE(handle.as_raw_handle()),&mut created,&mut exited,&mut kernel,&mut user) }?;
        let process=Self { handle, created:(u64::from(created.dwHighDateTime)<<32)|u64::from(created.dwLowDateTime) };
        if !process.alive() { return Err("agent process scope has already exited".into()); }
        Ok(process)
    }
    fn alive(&self) -> bool { unsafe { WaitForSingleObject(HANDLE(self.handle.as_raw_handle()),0)==WAIT_TIMEOUT } }
}

fn endpoint(control: bool) -> Result<ipc::Endpoint> {
    ipc::Endpoint::current()?.service(if control { "agent-control" } else { "agent" })
}

struct Options { monitor: String, process: Option<u32>, seconds: u64, process_birth: Option<u64> }
impl Options {
    fn parse(args: &[&str]) -> Result<Self> {
        let mut monitor = None;
        let mut process = None;
        let mut seconds = 300;
        let mut foreground = false;
        let mut seen = HashSet::new();
        for pair in args.chunks(2) {
            let [key, value] = pair else { return Err("agent serve options require values".into()); };
            if !seen.insert(*key) { return Err("duplicate agent serve option".into()); }
            match *key {
                "--input" if *value == "foreground" => foreground = true,
                "--monitor" if value.starts_with(r"\\.\DISPLAY") => monitor = Some(value.to_string()),
                "--process" => process = Some(value.parse()?),
                "--seconds" => seconds = value.parse()?,
                _ => return Err("use agent serve --input foreground --monitor NAME [--process PID] [--seconds N]".into()),
            }
        }
        if !foreground || monitor.is_none() || process == Some(0) || !(1..=3600).contains(&seconds) {
            return Err("foreground input needs an explicit display name, nonzero process if given, and a 1..3600 second lease".into());
        }
        Ok(Self { monitor: monitor.unwrap(), process, seconds, process_birth:None })
    }
}

struct Action<'a> { name: &'a str, selector: &'a str, args: Vec<DesktopValue> }
fn action<'a>(words: &'a [&str]) -> Result<Action<'a>> {
    use DesktopValue::{Num, Text};
    let number = |s: &str| -> Result<DesktopValue> {
        let n: f64 = s.parse()?;
        if !n.is_finite() || n < 0.0 || n > 8192.0 { return Err("invalid picture coordinate".into()); }
        Ok(Num(n))
    };
    let count = |s: &str, limit: u32| -> Result<DesktopValue> {
        let n: u32 = s.parse()?;
        if !(1..=limit).contains(&n) { return Err("invalid gesture count".into()); }
        Ok(Num(n as f64))
    };
    let (name, selector, args) = match words {
        ["focus", selector] => ("focus", *selector, vec![]),
        ["move", selector, x, y] => ("move", *selector, vec![number(x)?,number(y)?]),
        ["type", selector, text] if !text.contains('\0') && text.chars().count() <= 4000 =>
            ("type", *selector, vec![Text(text.to_string())]),
        [name @ ("key" | "hotkey"), selector, value] if value.len() <= 32 =>
            (*name, *selector, vec![Text(value.to_ascii_lowercase())]),
        ["click", selector, x, y, rest @ ..] if rest.len() <= 2 => {
            let button = rest.first().copied().unwrap_or("left");
            if !matches!(button, "left" | "right" | "middle") { return Err("invalid mouse button".into()); }
            ("click", *selector, vec![number(x)?, number(y)?, Text(button.into()), count(rest.get(1).copied().unwrap_or("1"), 3)?])
        },
        ["drag", selector, x1, y1, x2, y2] =>
            ("drag", *selector, vec![number(x1)?, number(y1)?, number(x2)?, number(y2)?]),
        ["scroll", selector, x, y, direction, rest @ ..] if rest.len() <= 1 => {
            if !matches!(*direction, "up" | "down" | "left" | "right") { return Err("invalid wheel direction".into()); }
            ("scroll", *selector, vec![number(x)?, number(y)?, Text(direction.to_string()), count(rest.first().copied().unwrap_or("1"), 30)?])
        },
        _ => return Err("invalid native input command; use agent help".into()),
    };
    if selector.len() > 100 || !(selector.parse::<u32>().is_ok_and(|pid| pid > 0) || selector.split(':').count() == 4) {
        return Err("use a PID or exact native window.id".into());
    }
    Ok(Action { name, selector, args })
}

fn scope(window: &Window, monitor: &Monitor, process: Option<u32>) -> Result<()> {
    if window.minimized || window.monitor != monitor.name || !monitor.bounds.contains(&window.bounds)
        || process.is_some_and(|pid| pid != window.process) {
        return Err("target must be visible and wholly inside the agent's display/process scope".into());
    }
    Ok(())
}

fn selected(selector: &str, options: &Options) -> Result<(HWND, Window)> {
    let catalog = windows()?;
    let window = select_window(&catalog, selector)?;
    let screen = monitors()?.into_iter().find(|m| m.name == options.monitor).ok_or("agent display disconnected")?;
    let (hwnd, mut current) = target(&window.id)?;
    let identity=Identity::read(hwnd).ok_or("agent target closed")?;
    if options.process.is_some_and(|pid|pid!=identity.pid) { return Err("target is outside the agent process scope".into()); }
    if options.process_birth.is_some_and(|birth|birth!=identity.created) {
        return Err("the scoped process exited or its PID was reused".into());
    }
    // DWM's visible frame matches the input picture. A maximized window's
    // invisible resize border may extend beyond the monitor's physical bounds.
    let mut frame = RECT::default();
    unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut _ as _, size_of::<RECT>() as u32) }?;
    current.bounds = frame.into();
    scope(&current, &screen, options.process)?;
    Ok((hwnd, current))
}

fn native_id(session: &mut Session, hwnd: HWND) -> Result<String> {
    let identity = Identity::read(hwnd).ok_or("input target closed")?;
    session.windows()?;
    Ok(session.window_id(hwnd.0 as usize, identity.pid, identity.thread)?)
}

/// Share the bounded capture implementation without constructing an input session.
/// Disabled owners remain readable, with no implicit modal redirection.
pub(super) fn read_only_picture(window: &str) -> Result<CapturedPicture> {
    let window = window.to_owned();
    let result = thread::spawn(move || -> std::result::Result<CapturedPicture, String> {
        (|| -> Result<CapturedPicture> {
            let (hwnd, _) = target(&window)?;
            let identity = Identity::read(hwnd).ok_or("capture target closed")?;
            decode_picture(pleamar::windows_desktop::read_only_picture(
                hwnd.0 as usize, identity.pid, identity.thread)?)
        })().map_err(|error| error.to_string())
    }).join().map_err(|_| "read-only capture worker failed")?;
    result.map_err(Into::into)
}

fn picture(session: &mut Session, options: &Options, selector: &str, path: &str) -> Result<Value> {
    if !Path::new(path).is_absolute() { return Err("input capture requires an absolute new file path".into()); }
    let (hwnd, before) = selected(selector, options)?;
    let id = native_id(session, hwnd)?;
    let captured = session.look(&id)?;
    let (_, after) = selected(&before.id, options)?;
    if before.bounds != after.bounds { return Err("window moved during look; try again".into()); }
    let picture = decode_picture(captured)?;
    write_picture(Path::new(path), &picture.png)?;
    Ok(json!({"path":path,"width":picture.size.0,"height":picture.size.1,"window":before.id,"input":"foreground",
        "capture_method":picture.method,"coordinates":"picture physical pixels","permit":"one action within 30 seconds"}))
}

struct Pending { cancel: Cancellation, expired: Option<Arc<AtomicBool>> }
struct Controller {
    stop: Arc<AtomicBool>, pending: Arc<Mutex<Pending>>, worker: Option<thread::JoinHandle<()>>,
    changed: Arc<ipc::Event>, stopped: Arc<ipc::Event>,
}
struct ControlExit { stop: Arc<AtomicBool>, pending: Arc<Mutex<Pending>>, event: Arc<ipc::Event> }
impl Drop for ControlExit {
    fn drop(&mut self) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).cancel.cancel();
        // Observing stopped must also observe revoked input permits.
        self.stop.store(true, Ordering::Release);
        self.event.signal();
    }
}
fn control_wait(remaining: Duration, pending: bool) -> u32 {
    // Only a live request needs to observe IPC's atomic expiration flag.
    let wait = if pending { remaining.min(Duration::from_millis(25)) } else { remaining };
    wait.as_millis().clamp(1, u32::MAX as u128 - 1) as u32
}
impl Controller {
    fn new(server: ipc::Server, cancel: Cancellation, seconds: u64, process: Option<Process>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(Mutex::new(Pending { cancel, expired: None }));
        let changed = Arc::new(ipc::Event::new()?);
        let stopped = Arc::new(ipc::Event::new()?);
        let (halt, active) = (stop.clone(), pending.clone());
        let (update, finished) = (changed.clone(), stopped.clone());
        let worker = thread::spawn(move || {
            let _exit = ControlExit { stop:halt.clone(), pending:active.clone(), event:finished.clone() };
            let deadline = Instant::now() + Duration::from_secs(seconds);
            let mut handles = vec![update.handle(), server.wake.handle(), server.stopped()];
            if let Some(process) = &process { handles.push(HANDLE(process.handle.as_raw_handle())); }
            loop {
                // Reset before checking the associated state/queue so a concurrent
                // publication is either observed below or leaves the event signalled.
                update.reset();
                server.wake.reset();
                let (expired, busy) = {
                    let pending = active.lock().unwrap();
                    (pending.expired.as_ref().is_some_and(|v| v.load(Ordering::Acquire)), pending.expired.is_some())
                };
                if halt.load(Ordering::Acquire) || expired || process.as_ref().is_some_and(|p|!p.alive())
                    || Instant::now() >= deadline || !server.running() { break; }
                match server.requests.try_recv() {
                    Ok(request) if !request.expired.load(Ordering::Acquire) => {
                        if request.command == "stop" {
                            active.lock().unwrap().cancel.cancel();
                            halt.store(true, Ordering::Release);
                            finished.signal();
                            request.finish(Ok(json!({"cancelled":true})));
                            break;
                        }
                        request.finish(Err("only stop is accepted on the cancellation endpoint".into()));
                        continue;
                    },
                    Ok(_) => continue,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {},
                    Err(_) => break,
                }
                let wait = control_wait(deadline.saturating_duration_since(Instant::now()), busy);
                if unsafe { WaitForMultipleObjects(&handles, false, wait) } == WAIT_FAILED { break; }
            }
        });
        Ok(Self { stop, pending, worker: Some(worker), changed, stopped })
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.pending.lock().unwrap().cancel.cancel();
        self.stop.store(true, Ordering::Release);
        self.changed.signal();
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

fn serve(args: &[&str]) -> Result<Value> {
    let mut options = Options::parse(args)?;
    let process=options.process.map(Process::open).transpose()?;
    options.process_birth=process.as_ref().map(|p|p.created);
    if !monitors()?.iter().any(|m| m.name == options.monitor) { return Err("agent display is not connected".into()); }
    let server = ipc::Server::start(endpoint(false)?)?;
    let control = ipc::Server::start(endpoint(true)?)?;
    let mut session = Some(Session::new()?);
    let controller = Controller::new(control, session.as_ref().unwrap().cancellation(), options.seconds, process)?;
    while !controller.stop.load(Ordering::Acquire) {
        server.wake.reset();
        pump();
        if !server.running() { return Err("native input listener stopped".into()); }
        let request = match server.requests.try_recv() {
            Ok(request) if !request.expired.load(Ordering::Acquire) => request,
            Ok(_) => continue,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                let handles = [server.wake.handle(), controller.stopped.handle(), server.stopped()];
                if unsafe { MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) } == WAIT_FAILED {
                    return Err(windows::core::Error::from_thread().into());
                }
                continue;
            },
            Err(e) => return Err(e.into()),
        };
        controller.pending.lock().unwrap().expired = Some(request.expired.clone());
        controller.changed.signal();
        let response = (|| -> Result<Value> {
            if controller.stop.load(Ordering::Acquire) { return Err("native input stopped".into()); }
            let words: Vec<String> = serde_json::from_str(&request.command)?;
            let words: Vec<_> = words.iter().map(String::as_str).collect();
            match words.as_slice() {
                ["input-status"] => Ok(json!({"input":"foreground","independent_seat":false,"monitor":options.monitor,
                    "process":options.process,"lease_seconds":options.seconds})),
                ["done"] => {
                    // Revoke every picture before allowing a new catalog on this worker.
                    drop(session.take());
                    let fresh = Session::new()?;
                    let mut pending = controller.pending.lock().unwrap();
                    if controller.stop.load(Ordering::Acquire) { return Err("native input stopped".into()); }
                    pending.cancel = fresh.cancellation();
                    session = Some(fresh);
                    Ok(json!({"pictures_forgotten":true}))
                },
                ["look", selector, file] => picture(session.as_mut().ok_or("native input stopped")?, &options, selector, file),
                _ => {
                    let action = action(&words)?;
                    let (hwnd, window) = selected(action.selector, &options)?;
                    let session = session.as_mut().ok_or("native input stopped")?;
                    let id = native_id(session, hwnd)?;
                    session.action(action.name, &id, &action.args)?;
                    Ok(json!({"submitted":true,"window":window.id,"input":"foreground"}))
                },
            }
        })();
        // A failed capture/write must not leave an unseen picture usable for input.
        if response.is_err() { if let Some(session) = &mut session { session.forget(); } }
        request.finish(response);
        controller.pending.lock().unwrap().expired = None;
        controller.changed.signal();
    }
    drop(controller);
    drop(session);
    Ok(json!({"stopped":true,"input":"foreground"}))
}

pub(super) fn execute(args: &[&str]) -> Result<Value> {
    match args {
        ["serve", rest @ ..] => serve(rest),
        ["stop"] => endpoint(true)?.ask("stop"),
        ["done" | "input-status"] => endpoint(false)?.ask(&serde_json::to_string(args)?),
        _ => {
            let mut owned = args.iter().map(|s|s.to_string()).collect::<Vec<_>>();
            if let ["type", _, "-"] = args {
                use std::io::Read;
                let mut input = String::new();
                std::io::stdin().take(16001).read_to_string(&mut input)?;
                if input.len() > 16000 { return Err("text exceeds the native input limit".into()); }
                owned[2] = input;
            }
            let words=owned.iter().map(String::as_str).collect::<Vec<_>>();
            let action=action(&words)?;
            let command=serde_json::to_string(&owned)?;
            if action.name=="focus" { endpoint(false)?.ask_with_focus(&command) }
            else { endpoint(false)?.ask(&command) }
        },
    }
}

pub(super) fn look_if_running(selector: &str, output: Option<&str>) -> Result<bool> {
    let endpoint = endpoint(false)?;
    if !endpoint.exists()? { return Ok(false); }
    let path = match output {
        Some(path) => std::path::absolute(path)?,
        None => {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
            std::env::temp_dir().join(format!("pleamar-agent-input-{}-{stamp}.png",std::process::id()))
        },
    };
    let path = path.to_str().ok_or("capture path is not Unicode")?;
    let result = endpoint.ask(&serde_json::to_string(&["look",selector,path])?)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_stop_cancels_a_worker_without_windows_or_input() {
        thread::spawn(|| {
            let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let endpoint=ipc::Endpoint::new(&format!("cancel-{}-{nonce}",std::process::id())).unwrap().service("agent-control").unwrap();
            let server=ipc::Server::start(endpoint.clone()).unwrap();
            let mut session=Session::new().unwrap();
            let controller=Controller::new(server,session.cancellation(),10,None).unwrap();
            assert_eq!(endpoint.ask("stop").unwrap()["cancelled"],true);
            assert!(controller.stop.load(Ordering::Acquire));
            assert!(session.look("1").unwrap_err().contains("cancelled"));
            drop(controller);
        }).join().unwrap();
    }
    #[test]
    fn process_exit_revokes_the_worker_even_if_a_pid_could_be_reused() {
        use std::{process::{Command,Stdio},os::windows::process::CommandExt};
        struct Child(std::process::Child);
        impl Drop for Child { fn drop(&mut self) { let _=self.0.kill();let _=self.0.wait(); } }
        thread::spawn(|| {
            let child=Command::new(std::env::current_exe().unwrap())
                .args(["--ignored","--exact","windows_backend::session_tests::native_owner_fixture"])
                .env("PLEAMAR_WM_OWNER_FIXTURE","1").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
                .creation_flags(0x08000000|0x00004000).spawn().unwrap();
            let mut child=Child(child);
            let process=Process::open(child.0.id()).unwrap();assert!(process.alive() && process.created!=0);
            let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let endpoint=ipc::Endpoint::new(&format!("process-{}-{nonce}",std::process::id())).unwrap().service("agent-control").unwrap();
            let mut session=Session::new().unwrap();
            let controller=Controller::new(ipc::Server::start(endpoint).unwrap(),session.cancellation(),10,Some(process)).unwrap();
            drop(child.0.stdin.take());assert!(child.0.wait().unwrap().success());
            let began=Instant::now();
            while !controller.stop.load(Ordering::Acquire) { assert!(began.elapsed()<Duration::from_secs(3));thread::sleep(Duration::from_millis(10)); }
            assert!(session.look("1").unwrap_err().contains("cancelled"));
            drop(controller);
        }).join().unwrap();
    }
    #[test]
    fn idle_controller_waits_for_events_but_live_requests_keep_their_cancellation_bound() {
        assert_eq!(control_wait(Duration::from_secs(300), false), 300_000);
        assert_eq!(control_wait(Duration::from_secs(300), true), 25);
        assert_eq!(control_wait(Duration::from_millis(8), true), 8);
        assert_eq!(control_wait(Duration::from_nanos(1), false), 1);
    }
    #[test]
    fn foreground_scope_and_lease_are_explicit() {
        for args in [vec![], vec!["--input","foreground"], vec!["--monitor",r"\\.\DISPLAY2"],
            vec!["--input","foreground","--monitor","all"],
            vec!["--input","foreground","--monitor",r"\\.\DISPLAY2","--seconds","0"],
            vec!["--input","foreground","--monitor",r"\\.\DISPLAY2","--process","0"]] {
            assert!(Options::parse(&args).is_err());
        }
        let parsed = Options::parse(&["--input","foreground","--monitor",r"\\.\DISPLAY2","--seconds","15"]).unwrap();
        assert_eq!(parsed.seconds,15);
    }
    #[test]
    fn malformed_gestures_fail_without_opening_a_pipe_or_sending_input() {
        for args in [vec!["click","123","NaN","0"], vec!["click","123","0","0","left","4"],
            vec!["drag","123","0","0","-1","20"],vec!["scroll","123","0","0","up","31"],
            vec!["focus","123.1"],vec!["type","123","a\0b"],vec!["open","app.exe"]] {
            assert!(action(&args).is_err());
        }
        let typed = action(&["type","123","Café 海"]).unwrap();
        assert!(matches!(&typed.args[0], DesktopValue::Text(text) if text=="Café 海"));
        assert_eq!(action(&["click","123","2","3"]).unwrap().args.len(),4);
    }
    #[test]
    fn process_monitor_and_whole_window_scope_refuse_cross_display_input() {
        let m=Monitor{name:"secondary".into(),bounds:Bounds{x:-1920,y:0,width:1920,height:1080},
            work:Bounds{x:-1920,y:0,width:1920,height:1040},scale:1.0,primary:false,refresh_hz:60};
        let mut w=Window{id:"1:2:3:4".into(),title:"test".into(),app:"test.exe".into(),class:"test".into(),process:1,
            monitor:m.name.clone(),bounds:Bounds{x:-900,y:100,width:500,height:400},minimized:false,maximized:false,resizable:true};
        assert!(scope(&w,&m,Some(1)).is_ok());
        assert!(scope(&w,&m,Some(2)).is_err());
        w.bounds.x=-100;assert!(scope(&w,&m,None).is_err());
        w.bounds.x=-900;w.minimized=true;assert!(scope(&w,&m,None).is_err());
    }
}
