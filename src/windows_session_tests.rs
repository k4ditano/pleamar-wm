//! Integration with a separate real daemon. Only this process's test HWNDs
//! on a verified secondary display are ever managed.
use super::*;
use super::tests::{OwnWindows, ThreadDpi, fixture_proc};
use std::{os::windows::process::CommandExt, process::{Child, Command, Stdio}, sync::mpsc};

struct OwnedDaemon(Child);
impl Drop for OwnedDaemon { fn drop(&mut self) { let _=self.0.kill(); let _=self.0.wait(); } }

thread_local! { static FOCUS_EVENTS:std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) }; }
unsafe extern "system" fn foreground_event(_:windows::Win32::UI::Accessibility::HWINEVENTHOOK,_:u32,hwnd:HWND,_:i32,_:i32,_:u32,_:u32) {
    let mut pid=0;
    unsafe { GetWindowThreadProcessId(hwnd,Some(&mut pid)); }
    FOCUS_EVENTS.with(|events|events.borrow_mut().push(pid));
}
pub(super) struct FocusWatch(windows::Win32::UI::Accessibility::HWINEVENTHOOK);
impl FocusWatch {
    pub(super) fn new() -> Result<Self> {
        use windows::Win32::UI::Accessibility::*;
        FOCUS_EVENTS.with(|events|events.borrow_mut().clear());
        let hook=unsafe { SetWinEventHook(EVENT_SYSTEM_FOREGROUND,EVENT_SYSTEM_FOREGROUND,None,Some(foreground_event),0,0,WINEVENT_OUTOFCONTEXT) };
        if hook.is_invalid() { return Err("could not observe foreground events".into()); }
        Ok(Self(hook))
    }
    pub(super) fn events(&self) -> Vec<u32> { pump(); FOCUS_EVENTS.with(|events|events.borrow().clone()) }
}
impl Drop for FocusWatch { fn drop(&mut self) { let _=unsafe { windows::Win32::UI::Accessibility::UnhookWinEvent(self.0) }; } }

fn last_input_tick() -> u32 {
    #[repr(C)]
    struct LastInput { size:u32, tick:u32 }
    #[link(name="user32")]
    unsafe extern "system" { fn GetLastInputInfo(info:*mut LastInput) -> i32; }
    let mut info=LastInput { size:size_of::<LastInput>() as u32,tick:0 };
    assert_ne!(unsafe { GetLastInputInfo(&mut info) },0);
    info.tick
}

#[test]
#[ignore = "child-process fixture used by native_session_lifecycle"]
fn native_owner_fixture() -> Result<()> {
    if std::env::var("PLEAMAR_WM_OWNER_FIXTURE").as_deref() != Ok("1") {
        return Err("this helper must be started by its parent test".into());
    }
    std::io::copy(&mut std::io::stdin(), &mut std::io::sink())?;
    Ok(())
}

fn request(endpoint:&ipc::Endpoint, line:&str) -> Result<Value> {
    let endpoint=endpoint.clone();
    let line=line.to_owned();
    let (send, receive)=mpsc::channel();
    let task=std::thread::spawn(move || { let _=send.send(endpoint.ask(&line).map_err(|e|e.to_string())); });
    let result=loop {
        pump();
        match receive.try_recv() {
            Ok(result)=>break result,
            Err(mpsc::TryRecvError::Empty)=>std::thread::sleep(Duration::from_millis(5)),
            Err(e)=>return Err(e.into()),
        }
    };
    task.join().map_err(|_|"WM client thread panicked")?;
    result.map_err(Into::into)
}

fn until(mut check:impl FnMut()->bool) {
    let start=Instant::now();
    loop { pump(); if check() { break; } assert!(start.elapsed()<Duration::from_secs(10),"native WM state timed out"); std::thread::sleep(Duration::from_millis(15)); }
}

fn resources(pid:u32) -> Result<(u64,usize,usize)> {
    use windows::Win32::System::ProcessStatus::*;
    let handle=unsafe { OpenProcess(PROCESS_QUERY_INFORMATION|PROCESS_VM_READ,false,pid) }?;
    let result=(||->Result<_> {
        let mut created=FILETIME::default(); let mut exit=FILETIME::default();
        let mut kernel=FILETIME::default(); let mut user=FILETIME::default();
        unsafe { GetProcessTimes(handle,&mut created,&mut exit,&mut kernel,&mut user) }?;
        let mut memory=PROCESS_MEMORY_COUNTERS_EX::default();
        memory.cb=size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        unsafe { GetProcessMemoryInfo(handle,&mut memory as *mut _ as _,memory.cb) }?;
        let ticks=|v:FILETIME| ((v.dwHighDateTime as u64)<<32)|v.dwLowDateTime as u64;
        Ok((ticks(kernel)+ticks(user),memory.WorkingSetSize,memory.PrivateUsage))
    })();
    let _=unsafe { CloseHandle(handle) };
    result
}

#[test]
#[ignore = "starts a scoped daemon and its own test windows on a non-primary monitor"]
fn native_session_lifecycle() -> Result<()> {
    let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    if !requested.starts_with(r"\\.\DISPLAY") { return Err("explicit secondary monitor required".into()); }
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());
    let _dpi=ThreadDpi(dpi);
    let monitor=select_monitor(&requested)?;
    assert!(!monitor.primary);
    let foreground=unsafe { GetForegroundWindow() };
    let initial_input=last_input_tick();
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW { lpfnWndProc:Some(fixture_proc), hInstance:module.into(), lpszClassName:w!("pleamar-wm-session-fixture"), ..Default::default() };
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());
    let mut originals=Vec::new();
    let create=|i:i32|->Result<HWND> {
        let rect=Bounds { x:monitor.work.x+40+i*30,y:monitor.work.y+50+i*30,width:400,height:280 };
        assert!(monitor.work.contains(&rect));
        let title:Vec<u16>=format!("pleamar-wm session test ñ {i}").encode_utf16().chain([0]).collect();
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,PCWSTR(title.as_ptr()),WS_OVERLAPPEDWINDOW,
            rect.x,rect.y,rect.width,rect.height,None,None,Some(module.into()),None) }?;
        let _=unsafe { ShowWindow(hwnd,SW_SHOWNOACTIVATE) };
        pump();
        Ok(hwnd)
    };
    for i in 0..3 {
        let hwnd=create(i)?;
        owned.0.push(hwnd);
        let w=inspect(hwnd).unwrap();
        originals.push(SavedWindow { id:w.id,bounds:w.bounds });
    }
    let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let out=std::env::var_os("PLEAMAR_WM_TEST_OUTPUT").map(std::path::PathBuf::from)
        .unwrap_or_else(||std::env::temp_dir().join(format!("wm session ñ {nonce}")));
    std::fs::create_dir(&out)?;
    let rules_file=out.join("empty session.conf");
    std::fs::write(&rules_file,"")?;
    let state_file=out.join("session ñ.json");
    let namespace=format!("test-{}-{nonce}",std::process::id());
    let endpoint=ipc::Endpoint::new(&namespace)?;
    let binary=std::path::PathBuf::from(std::env::var_os("PLEAMAR_WM_TEST_BINARY").ok_or("set PLEAMAR_WM_TEST_BINARY")?).canonicalize()?;
    let host=std::env::var_os("PLEAMAR_WM_TEST_HOST").map(std::path::PathBuf::from);
    let start=|round:usize, owner:Option<u32>|->Result<OwnedDaemon> {
        let log=std::fs::File::create(out.join(format!("daemon-{round}.log")))?;
        let mut command=Command::new(host.as_ref().unwrap_or(&binary));
        if host.is_none() { command.arg("session"); }
        if let Some(pid) = owner { command.args(["--owner", &pid.to_string()]); }
        let child=command.args(["--monitor",&requested,"--process",&std::process::id().to_string(),
            "--namespace",&namespace,"--state",state_file.to_str().unwrap(),"--rules",rules_file.to_str().unwrap(),"--seconds","120"])
            .stdout(log.try_clone()?).stderr(Stdio::from(log)).creation_flags(0x08000000|0x00004000).spawn()?;
        Ok(OwnedDaemon(child))
    };
    let mut daemon=start(1,None)?;
    until(|| { assert!(daemon.0.try_wait().unwrap().is_none()); request(&endpoint,"status").is_ok() });
    let status=request(&endpoint,"status")?;
    assert_eq!(status["monitors"][0]["tiled"],false);
    let free_event=create(4)?;
    owned.0.push(free_event);
    let settle=Instant::now();
    while settle.elapsed()<Duration::from_millis(250) { pump(); std::thread::sleep(Duration::from_millis(10)); }
    assert_eq!(request(&endpoint,"status")?["catalog_scans"],0,"free mode scanned unrelated window events");
    let _=unsafe { DestroyWindow(owned.0.pop().unwrap()) };
    for old in &originals { assert_eq!(target(&old.id)?.1.bounds,old.bounds); }
    request(&endpoint,&format!("layout {requested} grid"))?;
    assert_eq!(request(&endpoint,"status")?["saved_windows"],3);
    for old in &originals { assert!(monitor.work.contains(&target(&old.id)?.1.bounds)); }
    let added=create(3)?;
    owned.0.push(added);
    until(||request(&endpoint,"status").unwrap()["monitors"][0]["windows"]==4);
    let _=unsafe { DestroyWindow(owned.0.pop().unwrap()) };
    until(||request(&endpoint,"status").unwrap()["monitors"][0]["windows"]==3);
    state(&originals[0].id,true)?;
    until(||request(&endpoint,"status").unwrap()["monitors"][0]["windows"]==2);
    request(&endpoint,&format!("free {requested}"))?;
    assert!(target(&originals[0].id)?.1.minimized);
    assert_eq!(request(&endpoint,"status")?["pending_recovery"],0);
    state(&originals[0].id,false)?;
    for old in &originals { assert_eq!(target(&old.id)?.1.bounds,old.bounds); }
    unsafe { SetWindowLongPtrW(owned.0[1],GWLP_USERDATA,1); }
    assert!(request(&endpoint,&format!("layout {requested} columns")).is_err());
    let failed=request(&endpoint,"status")?;
    assert_eq!(failed["monitors"][0]["tiled"],false);
    assert!(failed["monitors"][0]["error"].is_string());
    for old in &originals { assert_eq!(target(&old.id)?.1.bounds,old.bounds); }
    unsafe { SetWindowLongPtrW(owned.0[1],GWLP_USERDATA,0); }
    request(&endpoint,&format!("layout {requested} rows"))?;
    daemon.0.kill()?;
    daemon.0.wait()?;
    drop(daemon);
    let mut daemon=start(2,None)?;
    until(|| { assert!(daemon.0.try_wait().unwrap().is_none()); request(&endpoint,"status").is_ok() });
    for old in &originals { assert_eq!(target(&old.id)?.1.bounds,old.bounds); }
    let settled=Instant::now();
    while settled.elapsed()<Duration::from_millis(400) { pump(); std::thread::sleep(Duration::from_millis(10)); }
    let before=request(&endpoint,"status")?;
    let used_before=resources(daemon.0.id())?;
    let idle=Instant::now();
    while idle.elapsed()<Duration::from_secs(5) { pump(); std::thread::sleep(Duration::from_millis(25)); }
    let after=request(&endpoint,"status")?;
    let elapsed=idle.elapsed().as_secs_f64();
    let used_after=resources(daemon.0.id())?;
    assert_eq!(after["catalog_scans"],before["catalog_scans"],"idle WM repeatedly scanned the desktop");
    assert_eq!(after["geometry_changes"],before["geometry_changes"]);
    assert!(request(&endpoint,"emit toggle_rain").is_err());
    request(&endpoint,&format!("layout {requested} left"))?;
    let done=request(&endpoint,"quit")?;
    assert_eq!(done["pending_recovery"],0);
    until(||daemon.0.try_wait().unwrap().is_some());
    assert!(daemon.0.wait()?.success());
    for old in &originals { assert_eq!(target(&old.id)?.1.bounds,old.bounds); }
    for (round, kill) in [(3,false),(4,true)] {
        let mut owner=OwnedDaemon(Command::new(std::env::current_exe()?)
            .args(["--ignored","--exact","windows_backend::session_tests::native_owner_fixture","--nocapture"])
            .env("PLEAMAR_WM_OWNER_FIXTURE","1").stdin(Stdio::piped())
            .stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(0x08000000|0x00004000).spawn()?);
        let mut child=start(round,Some(owner.0.id()))?;
        until(|| { assert!(child.0.try_wait().unwrap().is_none()); request(&endpoint,"status").is_ok() });
        request(&endpoint,&format!("layout {requested} grid"))?;
        if kill { owner.0.kill()?; } else { drop(owner.0.stdin.take()); }
        until(||owner.0.try_wait().unwrap().is_some());
        if !kill { assert!(owner.0.wait()?.success()); }
        until(||child.0.try_wait().unwrap().is_some());
        assert!(child.0.wait()?.success());
        for old in &originals { assert_eq!(target(&old.id)?.1.bounds,old.bounds); }
        let journal:Value=serde_json::from_slice(&std::fs::read(&state_file)?)?;
        assert_eq!(journal["windows"].as_array().unwrap().len(),0);
    }
    let focus_unchanged=unsafe { GetForegroundWindow() }==foreground;
    let external_input=last_input_tick()!=initial_input;
    drop(owned);
    unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    let report=json!({"passed":focus_unchanged,"monitor":requested,"primary":false,"physical_input":false,
        "external_input_during_test":external_input,
        "focus_unchanged":focus_unchanged,"automatic_create_close":true,"minimize_restore":true,
        "free_while_minimized":true,"resize_rejection_rollback":true,"crash_recovery":true,
        "unicode_journal":true,"idle_seconds":5,"idle_catalog_scans":0,"quit_restores":true,
        "owner_exit_restores":true,"owner_kill_restores":true,"free_mode_avoids_catalog_scans":true});
    let mut report=report;
    report["idle_one_core_cpu_percent"]=json!((used_after.0-used_before.0) as f64/10000000.0/elapsed*100.0);
    report["daemon_working_set_bytes"]=json!(used_after.1);
    report["daemon_private_commit_bytes"]=json!(used_after.2);
    report["gui_host"]=json!(host.is_some());
    std::fs::write(out.join("report.json"),serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    assert!(focus_unchanged,"foreground changed; external input during test: {external_input}");
    Ok(())
}

#[test]
#[ignore = "applies native rules only to owned HWNDs on an explicit non-primary display"]
fn native_window_rules() -> Result<()> {
    let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());
    let _dpi=ThreadDpi(dpi);
    let monitor=select_monitor(&requested)?;
    assert!(!monitor.primary);
    let outside=monitors()?.into_iter().find(|m|m.primary).ok_or("primary monitor required for scope rejection")?;
    let foreground=unsafe { GetForegroundWindow() };
    let initial_input=last_input_tick();
    let focus=FocusWatch::new()?;
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW { lpfnWndProc:Some(fixture_proc),hInstance:module.into(),hbrBackground:HBRUSH((COLOR_WINDOW.0+1) as usize as _),
        lpszClassName:w!("pleamar-wm-rules-fixture"),..Default::default() };
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let create=|title:&str,i:i32|->Result<HWND> {
        let rect=Bounds{x:monitor.work.x+60+25*i,y:monitor.work.y+70+25*i,width:400,height:280};
        assert!(monitor.work.contains(&rect));
        let title:Vec<u16>=title.encode_utf16().chain([0]).collect();
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,PCWSTR(title.as_ptr()),WS_OVERLAPPEDWINDOW,
            rect.x,rect.y,rect.width,rect.height,None,None,Some(module.into()),None) }?;
        let _=unsafe { ShowWindow(hwnd,SW_SHOWNOACTIVATE) };
        pump();
        Ok(hwnd)
    };
    let mut owned=OwnWindows(Vec::new());
    for (i,title) in ["Float ñ #1","Sized ñ","Loading ñ","Reject ñ","Outside ñ"].iter().enumerate() {
        owned.0.push(create(title,i as i32)?);
    }
    let originals:Vec<_>=owned.0.iter().map(|h|inspect(*h).unwrap()).collect();
    let app=std::env::current_exe()?.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(originals[0].app,app);
    unsafe { SetWindowLongPtrW(owned.0[3],GWLP_USERDATA,1); }
    let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let out=std::env::var_os("PLEAMAR_WM_TEST_OUTPUT").map(std::path::PathBuf::from)
        .unwrap_or_else(||std::env::temp_dir().join(format!("wm rules ñ {nonce}")));
    std::fs::create_dir(&out)?;
    let rules=out.join("session ñ.conf");
    let journal=out.join("state ñ.json");
    std::fs::write(&rules,format!("window app={app} title=\"Float ñ #1\" float size 360x260\n\
        window app={app} title=\"Sized ñ\" size 400x300 monitor {requested}\n\
        window app={app} title=\"Float after title ñ\" float\n\
        window app={app} title=\"Reject ñ\" size 600x400\n\
        window app={app} title=\"Outside ñ\" monitor {}\n\
        window app={app} title=\"New float ñ\" float\n\
        window app=wrong-executable.exe title=\"Sized ñ\" float\n",outside.name))?;
    let namespace=format!("rules-{}-{nonce}",std::process::id());
    let endpoint=ipc::Endpoint::new(&namespace)?;
    let binary=std::path::PathBuf::from(std::env::var_os("PLEAMAR_WM_TEST_BINARY").ok_or("set PLEAMAR_WM_TEST_BINARY")?).canonicalize()?;
    let start=||->Result<OwnedDaemon> {
        let log=std::fs::OpenOptions::new().create(true).append(true).open(out.join("daemon.log"))?;
        let child=Command::new(&binary).args(["session","--monitor",&requested,"--process",&std::process::id().to_string(),
            "--namespace",&namespace,"--state",journal.to_str().unwrap(),"--rules",rules.to_str().unwrap(),"--seconds","120"])
            .stdout(log.try_clone()?).stderr(Stdio::from(log)).creation_flags(0x08000000|0x00004000).spawn()?;
        Ok(OwnedDaemon(child))
    };
    let mut daemon=start()?;
    until(|| { assert!(daemon.0.try_wait().unwrap().is_none()); request(&endpoint,"status").is_ok() });
    until(||request(&endpoint,"status").unwrap()["ruled_windows"]==2);
    let status=request(&endpoint,"status")?;
    assert_eq!(status["rule_errors"].as_object().unwrap().len(),2);
    assert_eq!(status["saved_windows"],0);
    let floated=target(&originals[0].id)?.1;
    let sized=target(&originals[1].id)?.1;
    assert_eq!(floated.bounds.width,(360.0*monitor.scale).round() as i32);
    assert_eq!(floated.bounds.height,(260.0*monitor.scale).round() as i32);
    assert_eq!(sized.bounds.width,(400.0*monitor.scale).round() as i32);
    assert_eq!(sized.bounds.height,(300.0*monitor.scale).round() as i32);
    for i in [3,4] { assert_eq!(target(&originals[i].id)?.1.bounds,originals[i].bounds); }
    assert_eq!(target(&originals[4].id)?.1.monitor,requested,"out-of-scope rule moved a test window");
    request(&endpoint,&format!("layout {requested} grid"))?;
    assert_eq!(request(&endpoint,"status")?["monitors"][0]["windows"],2);
    assert_eq!(target(&floated.id)?.1.bounds,floated.bounds);
    // Capture only an owned HWND, never the user's desktop or another app.
    let device=capture::Device::new(None)?;
    let mut captured=capture::Capture::new(device,owned.0[0],16_777_216)?;
    let deadline=Instant::now()+Duration::from_secs(8);
    let picture=loop {
        pump();
        if let Some(picture)=captured.next(16_777_216)? { break picture; }
        if Instant::now()>=deadline { return Err("owned rule fixture capture timed out".into()); }
        std::thread::sleep(Duration::from_millis(10));
    };
    std::fs::write(out.join("floating-window.bgra"),&picture.pixels)?;
    std::fs::write(out.join("floating-window.json"),serde_json::to_vec(&json!({"size":picture.size}))?)?;
    drop(captured);
    assert_ne!(target(&sized.id)?.1.bounds,sized.bounds);
    unsafe { SetWindowTextW(owned.0[2],w!("Float after title ñ")) }?;
    until(||request(&endpoint,"status").unwrap()["monitors"][0]["windows"]==1);
    assert_eq!(target(&originals[2].id)?.1.bounds,originals[2].bounds,"late floating rule lost the free position");
    // Once a matching rule applies, ordinary title changes do not rearrange it.
    unsafe { SetWindowTextW(owned.0[2],w!("New ordinary title")) }?;
    owned.0.push(create("New float ñ",5)?);
    until(||request(&endpoint,"status").unwrap()["ruled_windows"]==4);
    assert_eq!(request(&endpoint,"status")?["monitors"][0]["windows"],1);
    request(&endpoint,&format!("free {requested}"))?;
    assert_eq!(target(&sized.id)?.1.bounds,sized.bounds,"free mode lost the initial rule size");
    assert_eq!(target(&floated.id)?.1.bounds,floated.bounds);
    let settle=Instant::now();
    while settle.elapsed()<Duration::from_millis(300) { pump(); std::thread::sleep(Duration::from_millis(10)); }
    let before=request(&endpoint,"status")?;
    let idle=Instant::now();
    while idle.elapsed()<Duration::from_secs(3) { pump(); std::thread::sleep(Duration::from_millis(25)); }
    let after=request(&endpoint,"status")?;
    assert_eq!(before["catalog_scans"],after["catalog_scans"],"rules polled an unchanged desktop");
    request(&endpoint,"quit")?;
    until(||daemon.0.try_wait().unwrap().is_some());
    assert!(daemon.0.wait()?.success());
    assert_eq!(target(&floated.id)?.1.bounds,floated.bounds,"shutdown undid the explicit initial rule");
    assert_eq!(target(&sized.id)?.1.bounds,sized.bounds);
    let saved:Value=serde_json::from_slice(&std::fs::read(&journal)?)?;
    assert_eq!(saved["windows"].as_array().unwrap().len(),0);
    std::fs::write(&rules,"")?;
    let mut daemon_pids=vec![daemon.0.id()];
    // Model the on-disk state left between a rule mutation and its commit.
    // The recovery runs in a separate native daemon, including legacy journals.
    for version in [1,2] {
        let mut placement=WINDOWPLACEMENT{length:size_of::<WINDOWPLACEMENT>() as u32,..Default::default()};
        unsafe { GetWindowPlacement(owned.0[0],&mut placement) }?;
        let normal=placement.rcNormalPosition;
        let mut item=json!({"id":floated.id,"monitor":requested,"bounds":floated.bounds,
            "normal":[normal.left,normal.top,normal.right,normal.bottom]});
        if version==2 { item["pending_monitor"]=json!(requested); }
        std::fs::write(&journal,serde_json::to_vec(&json!({"version":version,"windows":[item]}))?)?;
        let changed=Bounds{width:floated.bounds.width+50,..floated.bounds.clone()};
        assert!(monitor.work.contains(&changed));
        place(&floated.id,&changed)?;
        let mut recovery=start()?;
        daemon_pids.push(recovery.0.id());
        until(|| { assert!(recovery.0.try_wait().unwrap().is_none()); request(&endpoint,"status").is_ok() });
        assert_eq!(target(&floated.id)?.1.bounds,floated.bounds);
        assert_eq!(request(&endpoint,"status")?["saved_windows"],0);
        request(&endpoint,"quit")?;
        until(||recovery.0.try_wait().unwrap().is_some());
        assert!(recovery.0.wait()?.success());
    }
    let focus_unchanged=unsafe { GetForegroundWindow() }==foreground;
    let foreground_processes=focus.events();
    let test_activated=foreground_processes.iter().any(|pid|*pid==std::process::id() || daemon_pids.contains(pid));
    let report=json!({"passed":!test_activated,"monitor":requested,"primary":false,"physical_input":false,
        "focus_unchanged":focus_unchanged,"external_input_during_test":last_input_tick()!=initial_input,
        "foreground_processes":foreground_processes,"owned_process_activated":test_activated,
        "executable_and_title_match":true,"dpi_rule_size":true,"late_title_float":true,"new_window_float":true,
        "once_per_window":true,"free_restores_rule_size":true,"rejected_size_rollback":true,"outside_scope_rejected":true,
        "unfinished_rule_recovery":true,"legacy_journal_recovery":true,
        "idle_seconds":3,"idle_catalog_scans":0,"cross_monitor_transfer_tested":false,"rule_errors":status["rule_errors"]});
    std::fs::write(out.join("report.json"),serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    drop(owned);
    unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    assert!(!test_activated,"a test-owned process received foreground focus");
    Ok(())
}
