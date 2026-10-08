//! Owned non-activating app for dock launch/file-argument acceptance.
#[cfg(not(target_os="windows"))]
fn main() { eprintln!("this fixture requires Windows"); }
#[cfg(target_os="windows")]
fn main() {
    if let Err(error)=fixture::run() {eprintln!("{error}");std::process::exit(1);}
}
#[cfg(target_os="windows")]
mod fixture {
    use windows::{core::{w,BOOL},Win32::{Foundation::*,Graphics::Gdi::*,System::LibraryLoader::*,UI::{HiDpi::*,WindowsAndMessaging::*}}};
    use std::{mem::size_of,path::PathBuf};
    struct Find {name:String,work:Option<RECT>,package:bool}
    unsafe extern "system" fn monitor(handle:HMONITOR,_:HDC,_:*mut RECT,data:LPARAM) -> BOOL {
        let find=unsafe {&mut *(data.0 as *mut Find)};
        let mut info=MONITORINFOEXW::default();info.monitorInfo.cbSize=size_of::<MONITORINFOEXW>() as u32;
        if unsafe {GetMonitorInfoW(handle,&mut info.monitorInfo)}.as_bool() {
            let name=String::from_utf16_lossy(&info.szDevice[..info.szDevice.iter().position(|c|*c==0).unwrap_or(info.szDevice.len())]);
            if name==find.name && (find.package || info.monitorInfo.dwFlags&MONITORINFOF_PRIMARY==0) {find.work=Some(info.monitorInfo.rcWork);}
        }
        true.into()
    }
    unsafe extern "system" fn window(hwnd:HWND,message:u32,w:WPARAM,l:LPARAM) -> LRESULT {
        unsafe {
            match message {
                WM_PAINT=>{
                    let mut paint=PAINTSTRUCT::default();let dc=BeginPaint(hwnd,&mut paint);
                    let mut area=RECT {left:18,top:18,right:300,bottom:150};
                    let mut text:Vec<u16>="Owned dock application — ñ 海\nNo keyboard or mouse input.".encode_utf16().collect();
                    DrawTextW(dc,&mut text,&mut area,DT_LEFT|DT_WORDBREAK);let _=EndPaint(hwnd,&paint);LRESULT(0)
                },
                WM_TIMER=>{let _=DestroyWindow(hwnd);LRESULT(0)},
                WM_DESTROY=>{PostQuitMessage(0);LRESULT(0)},
                _=>DefWindowProcW(hwnd,message,w,l),
            }
        }
    }
    pub fn run() -> Result<(),Box<dyn std::error::Error>> {
        let mut buffer=vec![0u16;1024];let mut length=buffer.len() as u32;
        let identified=unsafe {windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName(&mut length,Some(windows::core::PWSTR(buffer.as_mut_ptr())))}.is_ok();
        let package=identified.then(||String::from_utf16_lossy(&buffer[..buffer.iter().position(|v|*v==0).unwrap_or(buffer.len())]));
        let (name,root,ci_package)=if package.as_ref().is_some_and(|p|p.starts_with("Pleamar.NativeDockTest_")) {
            // Package activation comes from the OS broker, not the test's
            // environment. Only this separately registered CI package reads
            // its immutable adjacent configuration and may use a primary output.
            let config=std::env::current_exe()?.with_file_name("dock-fixture.json");
            let config:serde_json::Value=serde_json::from_slice(&std::fs::read(config)?)?;
            if config["disposable_github_runner"]!=true {return Err("invalid packaged fixture configuration".into());}
            (config["monitor"].as_str().ok_or("missing packaged monitor")?.to_owned(),
             PathBuf::from(config["output"].as_str().ok_or("missing packaged output")?),true)
        } else {
            (std::env::var("PLEAMAR_DOCK_FIXTURE_MONITOR")?,
             PathBuf::from(std::env::var_os("PLEAMAR_DOCK_FIXTURE_ROOT").ok_or("missing fixture output directory")?),false)
        };
        if !root.is_absolute() || !root.is_dir() {return Err("fixture output must be an existing absolute directory".into());}
        unsafe {SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)}?;
        let ci_desktop=disposable_desktop(std::env::var("GITHUB_ACTIONS").ok().as_deref(),
            std::env::var("RUNNER_ENVIRONMENT").ok().as_deref(),
            std::env::var("PLEAMAR_WM_CI_DOCK").ok().as_deref(),
            std::env::var("PLEAMAR_WM_CI_DOCK_DROP").ok().as_deref());
        let mut find=Find {name,work:None,package:ci_package || ci_desktop};
        if !unsafe {EnumDisplayMonitors(None,None,Some(monitor),LPARAM(&mut find as *mut _ as isize))}.as_bool() {return Err("monitor enumeration failed".into());}
        let area=find.work.ok_or("the explicit non-primary fixture monitor is unavailable")?;
        if area.right-area.left<400 || area.bottom-area.top<260 {return Err("secondary work area is too small".into());}
        let module=unsafe {GetModuleHandleW(None)}?;
        let class=WNDCLASSW {lpfnWndProc:Some(window),hInstance:module.into(),lpszClassName:w!("pleamar-owned-dock-app"),
            hbrBackground:unsafe {HBRUSH(GetStockObject(WHITE_BRUSH).0)},..Default::default()};
        if unsafe {RegisterClassW(&class)}==0 {return Err("fixture registration failed".into());}
        let hwnd=unsafe {CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,w!("Owned dock app ñ"),WS_OVERLAPPEDWINDOW,
            area.left+24,area.top+24,340,220,None,None,Some(module.into()),None)}?;
        unsafe {let _=ShowWindow(hwnd,SW_SHOWNOACTIVATE);SetTimer(Some(hwnd),1,60_000,None);}
        let report=serde_json::json!({"pid":std::process::id(),"hwnd":hwnd.0 as usize,"monitor":find.name,"package":package,"args":std::env::args().skip(1).collect::<Vec<_>>()});
        use std::io::Write;
        let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(root.join(format!("launch-{}.json",std::process::id())))?;
        file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;drop(file);
        let mut message=MSG::default();
        while unsafe {GetMessageW(&mut message,None,0,0)}.0>0 {unsafe {let _=TranslateMessage(&message);DispatchMessageW(&message);}}
        Ok(())
    }

    fn disposable_desktop(actions:Option<&str>,host:Option<&str>,dock:Option<&str>,drop:Option<&str>) -> bool {
        actions==Some("true") && host==Some("github-hosted") && (dock==Some("1") || drop==Some("1"))
    }
    #[cfg(test)]
    mod tests {
        use super::disposable_desktop as allowed;
        #[test]
        fn local_or_unrequested_primary_output_is_refused() {
            assert!(!allowed(None,None,Some("1"),Some("1")));
            assert!(!allowed(Some("true"),Some("self-hosted"),Some("1"),None));
            assert!(!allowed(Some("true"),Some("github-hosted"),None,None));
            assert!(allowed(Some("true"),Some("github-hosted"),Some("1"),None));
            assert!(allowed(Some("true"),Some("github-hosted"),None,Some("1")));
        }
    }
}
