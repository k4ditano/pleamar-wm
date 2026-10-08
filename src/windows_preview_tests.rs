//! A native renderer regression: a static preview must repaint when only the
//! captured program's pixels change. No synthetic input or third-party window.
use super::*;
use super::tests::{OwnWindows, ThreadDpi};
use std::{os::windows::process::CommandExt, process::{Child, Command, Stdio}};
use std::sync::atomic::{AtomicBool,Ordering};

static REFUSE_CLOSE:AtomicBool=AtomicBool::new(true);
static REFUSE_SIZE:AtomicBool=AtomicBool::new(false);
unsafe extern "system" fn action_fixture(hwnd:HWND,message:u32,w:WPARAM,l:LPARAM) -> LRESULT {
    if message==WM_CLOSE && REFUSE_CLOSE.load(Ordering::Relaxed) { return LRESULT(0); }
    unsafe { super::capture_tests::paint_fixture(hwnd,message,w,l) }
}

unsafe extern "system" fn size_fixture(hwnd:HWND,message:u32,w:WPARAM,l:LPARAM) -> LRESULT {
    if message==WM_WINDOWPOSCHANGING && REFUSE_SIZE.load(Ordering::Relaxed) {
        let position=unsafe { &mut *(l.0 as *mut WINDOWPOS) };
        if !position.flags.contains(SWP_NOSIZE) { position.cx=400; }
    }
    unsafe { super::capture_tests::paint_fixture(hwnd,message,w,l) }
}

struct Scene(Child);
impl Drop for Scene { fn drop(&mut self) { let _=self.0.kill();let _=self.0.wait(); } }

#[test]
#[ignore = "Marea overview pagination on owned secondary-monitor windows without input or activation"]
fn native_marea_overview_pages() -> Result<()> {
    let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    let binary=std::fs::canonicalize(std::env::var_os("PLEAMAR_WM_TEST_BINARY").ok_or("set PLEAMAR_WM_TEST_BINARY")?)?;
    let product=std::path::PathBuf::from(std::env::var_os("PLEAMAR_WM_TEST_OVERVIEW").ok_or("set PLEAMAR_WM_TEST_OVERVIEW")?);
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());let _dpi=ThreadDpi(dpi);
    let monitor=select_monitor(&requested)?;assert!(!monitor.primary);
    let watch=super::session_tests::FocusWatch::new()?;
    let foreground=unsafe { GetForegroundWindow() };
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW {lpfnWndProc:Some(super::capture_tests::paint_fixture),hInstance:module.into(),
        lpszClassName:w!("pleamar-wm-marea-pages"),..Default::default()};
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());
    let colors=[0x0020c060,0x00d03080,0x006040e0,0x00c09020,0x0070c030,0x003070d0];
    for i in 0..6 {
        let title:Vec<_>=format!("Overview ñ {i}").encode_utf16().chain([0]).collect();
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,PCWSTR(title.as_ptr()),WS_OVERLAPPEDWINDOW,
            monitor.work.x+20+i as i32*35,monitor.work.y+40+i as i32*30,400,280,None,None,Some(module.into()),None) }?;
        owned.0.push(hwnd);
        unsafe { SetWindowLongPtrW(hwnd,GWLP_USERDATA,colors[i]);let _=ShowWindow(hwnd,SW_SHOWNOACTIVATE); }
        pump();assert!(monitor.work.contains(&inspect(hwnd).unwrap().bounds));
    }
    let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let directory=std::path::PathBuf::from(std::env::var_os("PLEAMAR_WM_TEST_OUTPUT").ok_or("set PLEAMAR_WM_TEST_OUTPUT")?);
    std::fs::create_dir(&directory)?;
    let path=directory.join("windows-overview.plm");
    let source=std::fs::read_to_string(&product)?;
    let surface="surface { size: full, full; level: overlay; keyboard: on_demand; rate: 30 }";
    assert!(source.contains(surface));
    std::fs::write(&path,source.replace(surface,"surface { size: 960, 640; anchor: center; keyboard: none; reserve: 0; rate: 30 }"))?;
    std::fs::copy(product.with_extension("luau"),path.with_extension("luau"))?;
    let namespace=format!("wm-marea-pages-{nonce}");
    let log=std::fs::File::create(directory.join("scene.log"))?;
    let mut paths=vec![binary.parent().unwrap().to_owned()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let mut scene=Scene(Command::new(&binary).arg("--scene").arg(&path)
        .args(["--screen",&requested,"--preview-monitor",&requested,"--preview-process",&std::process::id().to_string(),
            "--window-actions","--no-hud","--stall","0","--seconds","120"])
        .env("PLEAMAR_SOCKET_DIR",&namespace).env("PLEAMAR_TEST_WINDOWS","1").env("PLEAMAR_NO_RELAUNCH","1")
        .env("MAREA_LOCALE","es").env("PATH",std::env::join_paths(paths)?)
        .stdout(log.try_clone()?).stderr(Stdio::from(log)).creation_flags(0x08000000|0x00004000).spawn()?);
    let query=|line:&str|ask(&binary,&path,&namespace,line);
    let wait_for=|line:&str,predicate:fn(&str)->bool| -> Result<String> {
        let start=Instant::now();
        loop {
            pump();let result=query(line);
            if let Ok(value)=&result { if predicate(value) { return Ok(value.clone()); } }
            if start.elapsed()>Duration::from_secs(15) { return Err(format!("{line}: {result:?}").into()); }
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    wait_for("get win.count",|s|s=="6")?;
    assert_eq!(query("get locale")?,"es");
    let mut slots=Vec::new();
    for i in 0..6 {
        let title=query(&format!("get win.{i}.title"))?;
        let fixture=title.strip_prefix("Overview ñ ").ok_or("unexpected window title")?.parse::<usize>()?;
        let place=query(&format!("get win.{i}.place"))?.parse::<usize>()?;
        slots.push((place,i,fixture));
    }
    slots.sort_unstable();
    let hwnd=canvas(scene.0.id(),&monitor)?.ok_or("overview canvas missing")?;
    let mut capture=capture::Capture::new(capture::Device::new(None)?,hwnd,16_777_216)?;
    let mut stages=Vec::new();
    for page in [0,1,0,1] {
        query(if page==0 { "emit overview_previous" } else { "emit overview_next" })?;
        for &(place,slot,_) in &slots {
            let visible=place/4==page;
            wait_for(&format!("get win.{slot}.width"),if visible { |s|s.parse::<f64>().is_ok_and(|v|v>0.0) } else { |s|s=="0" })?;
            assert!(matches!(query(&format!("get win.{slot}.open"))?.as_str(),"1"|"true"));
        }
        let fixture=slots[page*4].2;
        let color=colors[fixture] as u32;
        let pixels=[(color>>16) as u8,(color>>8) as u8,color as u8,255];
        let frame=picture(&mut capture,&mut scene,pixels,true)?;
        std::fs::write(directory.join(format!("page-{page}.bgra")),frame.pixels)?;
        std::fs::write(directory.join(format!("page-{page}-size.json")),serde_json::to_string(&frame.size)?)?;
        stages.push(json!({"page":page,"visible_captures":if page==0 {4} else {2},"hidden_geometry_cleared":true,"catalog_count":6}));
    }
    // Hidden sources keep their identity, but their next page must show fresh pixels.
    let (..,fixture)=slots[0];
    unsafe { SetWindowLongPtrW(owned.0[fixture],GWLP_USERDATA,0x00e060a0);let _=InvalidateRect(Some(owned.0[fixture]),None,false); }
    query("emit overview_previous")?;
    picture(&mut capture,&mut scene,[0xe0,0x60,0xa0,255],true)?;
    let child=scene.0.id();
    query("emit overview_close")?;
    let start=Instant::now();
    while scene.0.try_wait()?.is_none()&&start.elapsed()<Duration::from_secs(10) { pump();std::thread::sleep(Duration::from_millis(15)); }
    assert_eq!(scene.0.try_wait()?.and_then(|s|s.code()),Some(0));
    drop(capture);
    let activated=watch.events().iter().any(|pid|*pid==std::process::id()||*pid==child);
    let report=json!({"result":"passed","monitor":monitor.name,"primary":monitor.primary,"dpi_scale":monitor.scale,
        "product_scene":product,"owned_windows":6,"stages":stages,"fresh_pixels_on_return":true,
        "luau_close":true,"physical_input":false,"foreground_unchanged":foreground==unsafe{GetForegroundWindow()},"activated":activated});
    std::fs::write(directory.join("report.json"),serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");assert!(!activated);
    drop(owned);unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    Ok(())
}

fn canvas(pid:u32,monitor:&Monitor) -> Result<Option<HWND>> {
    struct Find { pid:u32,window:Option<HWND> }
    unsafe extern "system" fn visit(hwnd:HWND,data:LPARAM) -> BOOL { unsafe {
        let find=&mut *(data.0 as *mut Find);
        let mut pid=0;GetWindowThreadProcessId(hwnd,Some(&mut pid));
        if pid==find.pid&&IsWindowVisible(hwnd).as_bool() { find.window=Some(hwnd); }
        true.into()
    } }
    let mut find=Find {pid,window:None};
    unsafe { EnumWindows(Some(visit),LPARAM(&mut find as *mut _ as isize)) }?;
    if let Some(hwnd)=find.window {
        let mut rect=RECT::default();unsafe { GetWindowRect(hwnd,&mut rect) }?;
        assert!(monitor.work.contains(&rect.into()),"owned preview left its secondary work area");
    }
    Ok(find.window)
}

fn ask(binary:&Path,scene:&Path,namespace:&str,line:&str) -> Result<String> {
    let mut request=Scene(Command::new(binary).args(["--say",scene.file_stem().unwrap().to_str().unwrap(),line])
        .env("PLEAMAR_SOCKET_DIR",namespace).stdout(Stdio::piped()).stderr(Stdio::piped())
        .creation_flags(0x08000000|0x00004000).spawn()?);
    let start=Instant::now();
    while request.0.try_wait()?.is_none() {
        pump();
        if start.elapsed()>Duration::from_secs(5) { return Err("scene query timed out".into()); }
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut output=String::new();
    std::io::Read::read_to_string(request.0.stdout.as_mut().unwrap(),&mut output)?;
    if !request.0.try_wait()?.unwrap().success() { return Err("scene query failed".into()); }
    Ok(output.trim().to_owned())
}

#[test]
#[ignore = "native overview actions on owned secondary-monitor windows; no foreground activation or physical input"]
fn native_window_preview_actions() -> Result<()> {
    let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    let binary=std::fs::canonicalize(std::env::var_os("PLEAMAR_WM_TEST_BINARY").ok_or("set PLEAMAR_WM_TEST_BINARY")?)?;
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());let _dpi=ThreadDpi(dpi);
    let monitor=select_monitor(&requested)?;assert!(!monitor.primary);
    let watch=super::session_tests::FocusWatch::new()?;
    let foreground=unsafe { GetForegroundWindow() };
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW {lpfnWndProc:Some(action_fixture),hInstance:module.into(),
        lpszClassName:w!("pleamar-wm-preview-actions"),..Default::default()};
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());
    for i in 0..2 {
        let title:Vec<_>=format!("Owned overview ñ {i}").encode_utf16().chain([0]).collect();
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,PCWSTR(title.as_ptr()),WS_OVERLAPPEDWINDOW,
            monitor.work.x+30+i*450,monitor.work.y+40,400,280,None,None,Some(module.into()),None) }?;
        owned.0.push(hwnd);
        let mut rect=RECT::default();unsafe { GetWindowRect(hwnd,&mut rect) }?;
        assert!(monitor.work.contains(&rect.into()));
        unsafe { SetWindowLongPtrW(hwnd,GWLP_USERDATA,0x0020c060);let _=ShowWindow(hwnd,SW_SHOWNOACTIVATE); }
    }
    pump();
    let second_id=inspect(owned.0[1]).unwrap().id;
    state(&second_id,true)?;
    let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let directory=std::env::var_os("PLEAMAR_WM_TEST_OUTPUT").map(std::path::PathBuf::from)
        .unwrap_or_else(||std::env::temp_dir().join(format!("wm overview ñ {nonce}")));
    std::fs::create_dir(&directory)?;
    let path=directory.join("native-overview.plm");
    let example=include_str!("../examples/windows-overview.plm");
    // Keep the product scene's cards and handlers, but make this test canvas
    // non-activating. Commands below exercise its real renderer/action route.
    let surface="surface { size: 960, 640; kind: window; title: \"pleamar · native windows\" }";
    assert!(example.contains(surface));
    let mut source=example.replace(surface,"surface { size: 960, 640; anchor: center; keyboard: none; reserve: 0 }");
    source.truncate(source.rfind('}').unwrap());
    for i in 0..6 { source.push_str(&format!("\nevent hide{i}\nevent restore{i}\nevent close{i}\non hide{i} {{ minimize win.{i} }}\non restore{i} {{ restore win.{i} }}\non close{i} {{ close win.{i} }}\n")); }
    source.push_str("}\n");std::fs::write(&path,source)?;
    let mut child_pids=Vec::new();let mut stages=Vec::new();
    for actions in [false,true] {
        let namespace=format!("wm-overview-{nonce}-{actions}");
        let log=std::fs::File::create(directory.join(format!("scene-{actions}.log")))?;
        let mut command=Command::new(&binary);
        command.arg("--scene").arg(&path).args(["--screen",&requested,"--preview-monitor",&requested,
            "--preview-process",&std::process::id().to_string(),"--no-hud","--stall","0","--seconds","90"]);
        if actions { command.arg("--window-actions"); }
        let mut scene=Scene(command.env("PLEAMAR_SOCKET_DIR",&namespace).env("PLEAMAR_TEST_WINDOWS","1")
            .env("PLEAMAR_NO_RELAUNCH","1").stdout(log.try_clone()?).stderr(Stdio::from(log))
            .creation_flags(0x08000000|0x00004000).spawn()?);
        child_pids.push(scene.0.id());
        let query=|line:&str|ask(&binary,&path,&namespace,line);
        let wait_for=|line:&str,value:&str| -> Result<()> {
            let start=Instant::now();
            loop {
                pump();
                let result=query(line);
                if result.as_ref().is_ok_and(|v|v==value||matches!((v.as_str(),value),("true","1")|("false","0"))) { return Ok(()); }
                if start.elapsed()>Duration::from_secs(15) { return Err(format!("{line} did not become {value}: {result:?}").into()); }
                std::thread::sleep(Duration::from_millis(30));
            }
        };
        wait_for("get win.count","2")?;
        let mut indexes=[usize::MAX;2];
        for i in 0..6 { for (n,index) in indexes.iter_mut().enumerate() {
            if query(&format!("get win.{i}.title"))?==format!("Owned overview ñ {n}") { *index=i; }
        } }
        assert!(indexes.iter().all(|i|*i<6));
        let (a,b)=(indexes[0],indexes[1]);
        wait_for(&format!("get win.{b}.minimized"),"1")?;
        assert!(query(&format!("get win.{a}.app"))?.ends_with(".exe"));
        let hwnd=canvas(scene.0.id(),&monitor)?.ok_or("overview canvas missing")?;
        let mut capture=capture::Capture::new(capture::Device::new(None)?,hwnd,16_777_216)?;
        picture(&mut capture,&mut scene,[0x20,0xc0,0x60,255],true)?;
        if !actions {
            for line in [format!("emit hide{a}"),format!("emit restore{b}"),format!("emit close{a}")] { query(&line)?; }
            let until=Instant::now()+Duration::from_millis(300);
            while Instant::now()<until { pump();std::thread::sleep(Duration::from_millis(10)); }
            assert!(!inspect(owned.0[0]).unwrap().minimized);
            assert!(inspect(owned.0[1]).unwrap().minimized);
            assert_eq!(query("get win.count")?,"2");
            stages.push("view-only-rejects-actions");
        } else {
            query(&format!("emit restore{b}"))?;
            wait_for(&format!("get win.{b}.minimized"),"0")?;
            assert!(!inspect(owned.0[1]).unwrap().minimized);
            query(&format!("emit hide{a}"))?;
            wait_for(&format!("get win.{a}.minimized"),"1")?;
            assert!(inspect(owned.0[0]).unwrap().minimized);
            assert!(matches!(query(&format!("get win.{a}.open"))?.as_str(),"1"|"true"));
            assert_eq!(query("get win.count")?,"2");
            query(&format!("emit restore{a}"))?;
            wait_for(&format!("get win.{a}.minimized"),"0")?;
            unsafe { SetWindowLongPtrW(owned.0[0],GWLP_USERDATA,0x00d03080);let _=InvalidateRect(Some(owned.0[0]),None,false); }
            let frame=picture(&mut capture,&mut scene,[0xd0,0x30,0x80,255],true)?;
            std::fs::write(directory.join("overview.bgra"),frame.pixels)?;
            std::fs::write(directory.join("overview-size.json"),serde_json::to_string(&frame.size)?)?;
            query(&format!("emit close{a}"))?;
            let until=Instant::now()+Duration::from_millis(300);
            while Instant::now()<until { pump();std::thread::sleep(Duration::from_millis(10)); }
            assert!(inspect(owned.0[0]).is_some());assert_eq!(query("get win.count")?,"2");
            REFUSE_CLOSE.store(false,Ordering::Relaxed);
            query(&format!("emit close{a}"))?;
            wait_for(&format!("get win.{a}.open"),"0")?;
            assert!(inspect(owned.0[0]).is_none());owned.0.remove(0);
            picture(&mut capture,&mut scene,[0xd0,0x30,0x80,255],false)?;
            query(&format!("emit restore{a}"))?;
            assert_eq!(query("get win.count")?,"1");
            stages.extend(["initially-minimized-listed","minimize-keeps-identity","restore-resumes-real-capture",
                "cancelled-close-keeps-window","confirmed-close-removes-window","closed-slot-rejected"]);
        }
        drop(capture);query("quit")?;
        let start=Instant::now();
        while scene.0.try_wait()?.is_none()&&start.elapsed()<Duration::from_secs(10) { pump();std::thread::sleep(Duration::from_millis(15)); }
        assert_eq!(scene.0.try_wait()?.and_then(|s|s.code()),Some(0));
    }
    let events=watch.events();
    let activated=events.iter().any(|pid|*pid==std::process::id()||child_pids.contains(pid));
    let report=json!({"passed":!activated,"monitor":requested,"stages":stages,"physical_input":false,
        "foreground_activation_tested":false,"own_process_activated":activated,"foreground_events":events,
        "focus_unchanged":unsafe { GetForegroundWindow() }==foreground,"actual_wgc_to_d3d12":true});
    std::fs::write(directory.join("report.json"),serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");assert!(!activated);
    drop(owned);unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    Ok(())
}

fn picture(capture:&mut capture::Capture, scene:&mut Scene,color:[u8;4],present:bool) -> Result<capture::Picture> {
    let start=Instant::now();
    loop {
        pump();assert!(scene.0.try_wait()?.is_none(),"owned scene exited early");
        if let Some(picture)=capture.next(16_777_216)? {
            let count=picture.pixels.chunks_exact(4).filter(|p|*p==color).count();
            if (present&&count>10_000)||(!present&&count==0) { return Ok(picture); }
        }
        if start.elapsed()>Duration::from_secs(10) { return Err("native preview did not repaint its actual source pixels".into()); }
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[test]
#[ignore = "real scene size requests to owned DISPLAY2 windows without activation or input"]
fn native_window_scene_sizes() -> Result<()> {
    let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    let binary=std::fs::canonicalize(std::env::var_os("PLEAMAR_WM_TEST_BINARY").ok_or("set PLEAMAR_WM_TEST_BINARY")?)?;
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());let _dpi=ThreadDpi(dpi);
    let monitor=select_monitor(&requested)?;assert!(!monitor.primary);
    let watch=super::session_tests::FocusWatch::new()?;
    let foreground=unsafe { GetForegroundWindow() };
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW {lpfnWndProc:Some(size_fixture),hInstance:module.into(),
        lpszClassName:w!("pleamar-wm-preview-sizes"),..Default::default()};
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());let mut ids=Vec::new();
    for i in 0..2 {
        let title:Vec<_>=format!("Owned size ñ {i}").encode_utf16().chain([0]).collect();
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,PCWSTR(title.as_ptr()),WS_OVERLAPPEDWINDOW,
            monitor.work.x+monitor.work.width-440,monitor.work.y+50+i*310,400,280,None,None,Some(module.into()),None) }?;
        owned.0.push(hwnd);
        unsafe { SetWindowLongPtrW(hwnd,GWLP_USERDATA,0x0020c060);let _=ShowWindow(hwnd,SW_SHOWNOACTIVATE); }
        pump();let window=inspect(hwnd).unwrap();assert!(monitor.work.contains(&window.bounds));ids.push(window.id);
    }
    let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let directory=std::env::var_os("PLEAMAR_WM_TEST_OUTPUT").map(std::path::PathBuf::from)
        .unwrap_or_else(||std::env::temp_dir().join(format!("wm sizes ñ {nonce}")));
    std::fs::create_dir(&directory)?;
    let path=directory.join("native-sizes.plm");
    let example=include_str!("../examples/windows-resize.plm");
    let surface="surface { size: 960, 560; kind: window; title: \"pleamar · native sizes\" }";
    assert!(example.contains(surface));
    let mut source=example.replace(surface,"surface { size: 960, 560; anchor: center; keyboard: none; reserve: 0 }");
    source.truncate(source.rfind('}').unwrap());
    for i in 0..2 {
        for (label,w,h) in [("height",0,320),("tiny",1,1),("oversize",20000,20000),("rejected",500,300),("deferred",560,350)] {
            source.push_str(&format!("\nevent {label}.{i}\non {label}.{i} {{ width.{i}: {w} ~0ms; height.{i}: {h} ~0ms }}\n"));
        }
    }
    source.push_str("}\n");std::fs::write(&path,source)?;
    let settle=|ms:u64| { let until=Instant::now()+Duration::from_millis(ms);
        while Instant::now()<until { pump();std::thread::sleep(Duration::from_millis(10)); }
    };
    let sized=|id:&str,w:i32,h:i32| -> Result<()> {
        let start=Instant::now();
        loop {
            pump();let (_,window)=target(id)?;assert!(monitor.work.contains(&window.bounds));
            if window.bounds.width==w && window.bounds.height==h { return Ok(()); }
            if start.elapsed()>Duration::from_secs(5) { return Err(format!("size {w}x{h} not observed: {:?}",window.bounds).into()); }
            std::thread::sleep(Duration::from_millis(15));
        }
    };
    let mut child_pids=Vec::new();let mut stages=Vec::new();let mut repaint_ms=None;
    for actions in [false,true] {
        let namespace=format!("wm-sizes-{nonce}-{actions}");
        let log_path=directory.join(format!("scene-{actions}.log"));
        let log=std::fs::File::create(&log_path)?;
        let mut command=Command::new(&binary);
        command.arg("--scene").arg(&path).args(["--screen",&requested,"--preview-monitor",&requested,
            "--preview-process",&std::process::id().to_string(),"--no-hud","--stall","0","--seconds","90"]);
        if actions { command.arg("--window-actions"); }
        let mut scene=Scene(command.env("PLEAMAR_SOCKET_DIR",&namespace).env("PLEAMAR_TEST_WINDOWS","1")
            .env("PLEAMAR_NO_RELAUNCH","1").stdout(log.try_clone()?).stderr(Stdio::from(log))
            .creation_flags(0x08000000|0x00004000).spawn()?);
        child_pids.push(scene.0.id());
        let query=|line:&str|ask(&binary,&path,&namespace,line);
        let until=Instant::now()+Duration::from_secs(15);
        while !query("get win.count").is_ok_and(|v|v=="2") {
            pump();assert!(Instant::now()<until);std::thread::sleep(Duration::from_millis(30));
        }
        let a=if query("get win.0.title")?=="Owned size ñ 0" {0} else {1};
        let hwnd=canvas(scene.0.id(),&monitor)?.ok_or("sizes canvas missing")?;
        let mut capture=capture::Capture::new(capture::Device::new(None)?,hwnd,16_777_216)?;
        picture(&mut capture,&mut scene,[0x20,0xc0,0x60,255],true)?;
        let before=target(&ids[0])?.1.bounds;
        query(&format!("emit large.{a}"))?;
        if !actions {
            settle(350);assert_eq!(target(&ids[0])?.1.bounds,before);
            assert!(std::fs::read_to_string(&log_path)?.contains("resizing requires --window-actions"));
            stages.push("view-only-size-rejected");
        } else {
            let px=|n:i32| (n as f64*monitor.scale).round() as i32;
            sized(&ids[0],px(640),px(400))?;
            assert!(target(&ids[0])?.1.bounds.x<before.x,"growth must stay inside this monitor");
            query(&format!("emit height.{a}"))?;sized(&ids[0],px(640),px(320))?;
            let before=target(&ids[0])?.1.bounds;
            query(&format!("emit tiny.{a}"))?;settle(200);assert_eq!(target(&ids[0])?.1.bounds,before);
            query(&format!("emit oversize.{a}"))?;settle(200);assert_eq!(target(&ids[0])?.1.bounds,before);
            query(&format!("emit own.{a}"))?;settle(100);
            let free=Bounds {width:500,height:300,..before};place(&ids[0],&free)?;
            settle(200);assert_eq!(target(&ids[0])?.1.bounds,free);
            query(&format!("emit compact.{a}"))?;sized(&ids[0],px(320),px(240))?;
            REFUSE_SIZE.store(true,Ordering::Relaxed);
            query(&format!("emit rejected.{a}"))?;settle(50);
            let changed=Instant::now();
            unsafe { SetWindowLongPtrW(owned.0[1],GWLP_USERDATA,0x00d03080);let _=InvalidateRect(Some(owned.0[1]),None,false); }
            let frame=picture(&mut capture,&mut scene,[0xd0,0x30,0x80,255],true)?;
            repaint_ms=Some(changed.elapsed().as_millis());
            assert!(changed.elapsed()<Duration::from_millis(950),"a refused resize blocked the other preview");
            std::fs::write(directory.join("sizes.bgra"),frame.pixels)?;
            std::fs::write(directory.join("sizes-size.json"),serde_json::to_string(&frame.size)?)?;
            settle(1200);
            assert_eq!(std::fs::read_to_string(&log_path)?.matches("did not accept the requested scene size").count(),1);
            settle(200);REFUSE_SIZE.store(false,Ordering::Relaxed);
            query(&format!("emit large.{a}"))?;sized(&ids[0],px(640),px(400))?;
            state(&ids[0],true)?;
            query(&format!("emit deferred.{a}"))?;settle(200);assert!(target(&ids[0])?.1.minimized);
            query(&format!("emit own.{a}"))?;settle(100);
            state(&ids[0],false)?;settle(200);sized(&ids[0],px(640),px(400))?;
            state(&ids[0],true)?;
            query(&format!("emit deferred.{a}"))?;settle(200);assert!(target(&ids[0])?.1.minimized);
            state(&ids[0],false)?;sized(&ids[0],px(560),px(350))?;
            unsafe { SetWindowLongPtrW(owned.0[0],GWLP_USERDATA,0x003080e0);let _=InvalidateRect(Some(owned.0[0]),None,false); }
            picture(&mut capture,&mut scene,[0x30,0x80,0xe0,255],true)?;
            stages.extend(["dpi-scaled-size","growth-keeps-monitor","zero-axis-preserved","tiny-and-oversize-rejected",
                "app-size-releases-control","rejected-size-reported-once","other-preview-continues-during-refusal",
                "later-request-recovers","released-deferred-size-cancelled","minimized-size-deferred","restored-size-and-real-pixels"]);
        }
        drop(capture);query("quit")?;
        let until=Instant::now()+Duration::from_secs(10);
        while scene.0.try_wait()?.is_none() && Instant::now()<until { settle(15); }
        assert_eq!(scene.0.try_wait()?.and_then(|s|s.code()),Some(0));
    }
    let events=watch.events();
    let activated=events.iter().any(|pid|*pid==std::process::id()||child_pids.contains(pid));
    let report=json!({"passed":!activated,"monitor":requested,"scale":monitor.scale,"stages":stages,
        "repaint_during_refusal_ms":repaint_ms,"physical_input":false,"own_process_activated":activated,
        "foreground_events":events,"focus_unchanged":unsafe { GetForegroundWindow() }==foreground,"actual_wgc_to_d3d12":true});
    std::fs::write(directory.join("report.json"),serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");assert!(!activated);
    drop(owned);unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;Ok(())
}

#[test]
#[ignore = "renders and captures only an owned source and scene on an explicit secondary monitor"]
fn native_window_preview_repaints() -> Result<()> {
    let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    let binary=std::fs::canonicalize(std::env::var_os("PLEAMAR_WM_TEST_BINARY").ok_or("set PLEAMAR_WM_TEST_BINARY")?)?;
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());let _dpi=ThreadDpi(dpi);
    let monitor=select_monitor(&requested)?;assert!(!monitor.primary);
    let foreground=unsafe { GetForegroundWindow() };
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW {lpfnWndProc:Some(super::capture_tests::paint_fixture),hInstance:module.into(),
        lpszClassName:w!("pleamar-wm-preview-regression"),..Default::default()};
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());
    let source=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,w!("Owned preview source ñ"),WS_OVERLAPPEDWINDOW,
        monitor.work.x+30,monitor.work.y+40,400,280,None,None,Some(module.into()),None) }?;
    owned.0.push(source);
    let mut rect=RECT::default();unsafe { GetWindowRect(source,&mut rect) }?;
    assert!(monitor.work.contains(&rect.into()));
    unsafe { SetWindowLongPtrW(source,GWLP_USERDATA,0x0020c060);let _=ShowWindow(source,SW_SHOWNOACTIVATE); }
    pump();
    let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let directory=std::env::temp_dir().join(format!("pleamar-wm preview ñ {} {nonce}",std::process::id()));
    std::fs::create_dir(&directory)?;
    let path=directory.join("native-preview.plm");
    std::fs::write(&path,r##"scene PreviewRegression {
        surface { size: 640, 400; anchor: center; keyboard: none; reserve: 0 }
        windows win max 1
        box { from: 0, 0; size: 640, 400; color: #101820 }
        window win.0 { at: 40, 40; size: 560, 320; ask: -1, -1; show: win.0.open }
    }"##)?;
    let log=std::fs::File::create(directory.join("scene.log"))?;
    let mut scene=Scene(Command::new(binary).arg("--scene").arg(path).args(["--screen",&requested,
        "--preview-monitor",&requested,"--preview-process",&std::process::id().to_string(),
        "--no-hud","--stall","0","--seconds","45"])
        .env("PLEAMAR_TEST_WINDOWS","1").env("PLEAMAR_NO_RELAUNCH","1")
        .env("PLEAMAR_SOCKET_DIR",format!("wm-preview-{nonce}"))
        .stdout(log.try_clone()?).stderr(Stdio::from(log)).creation_flags(0x08000000|0x00004000).spawn()?);
    let start=Instant::now();
    let canvas=loop {
        pump();assert!(scene.0.try_wait()?.is_none());
        if let Some(hwnd)=canvas(scene.0.id(),&monitor)? { break hwnd; }
        assert!(start.elapsed()<Duration::from_secs(12),"native scene never appeared");
        std::thread::sleep(Duration::from_millis(15));
    };
    let mut capture=capture::Capture::new(capture::Device::new(None)?,canvas,16_777_216)?;
    picture(&mut capture,&mut scene,[0x20,0xc0,0x60,255],true)?;
    unsafe { SetWindowLongPtrW(source,GWLP_USERDATA,0x00d03080);let _=InvalidateRect(Some(source),None,false); }
    picture(&mut capture,&mut scene,[0xd0,0x30,0x80,255],true)?;
    unsafe { DestroyWindow(source) }?;owned.0.clear();
    picture(&mut capture,&mut scene,[0xd0,0x30,0x80,255],false)?;
    drop(capture);
    unsafe { PostMessageW(Some(canvas),WM_CLOSE,WPARAM(0),LPARAM(0)) }?;
    let start=Instant::now();
    while scene.0.try_wait()?.is_none()&&start.elapsed()<Duration::from_secs(10) {
        pump();std::thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(scene.0.try_wait()?.and_then(|s|s.code()),Some(0));
    unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    let focus_unchanged=unsafe { GetForegroundWindow() }==foreground;
    println!("{}",json!({"native_window_preview":true,"actual_pixels_changed":true,"source_close":true,
        "view_only":true,"physical_input":false,"focus_unchanged":focus_unchanged,"monitor":requested,"log":directory}));
    assert!(focus_unchanged);Ok(())
}
