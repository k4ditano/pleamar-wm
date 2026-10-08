//! Real focus and polite close requests on owned disposable CI windows.
use super::*;
use super::super::tests::{OwnWindows, ThreadDpi};
use windows::{core::HSTRING, Win32::UI::Input::KeyboardAndMouse::EnableWindow};

unsafe extern "system" fn procedure(hwnd:HWND,message:u32,w:WPARAM,l:LPARAM) -> LRESULT {
    if message==WM_CLOSE && unsafe { GetWindowLongPtrW(hwnd,GWLP_USERDATA) }==1 {
        // Model an application keeping unsaved work instead of closing.
        unsafe { SetWindowLongPtrW(hwnd,GWLP_USERDATA,2); }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd,message,w,l) }
}

#[test]
#[ignore = "changes focus and closes only owned windows on a disposable CI desktop"]
fn native_navigation_and_close() -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref()!=Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref()!=Ok("github-hosted")
        || std::env::var("PLEAMAR_WM_CI_NAVIGATION").as_deref()!=Ok("1") {
        return Err("requires the explicit navigation step on a disposable GitHub-hosted runner".into());
    }
    let out=PathBuf::from(std::env::var_os("PLEAMAR_WM_CI_OUTPUT").ok_or("missing evidence path")?);
    let temp=std::fs::canonicalize(std::env::var_os("RUNNER_TEMP").ok_or("missing runner temp")?)?;
    if std::fs::canonicalize(out.parent().ok_or("invalid evidence path")?)?!=temp || out.exists() {
        return Err("evidence must be a new direct child of RUNNER_TEMP".into());
    }
    std::fs::create_dir(&out)?;
    let previous=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!previous.0.is_null());let _dpi=ThreadDpi(previous);
    let screen=monitors()?.into_iter().find(|m|m.primary).ok_or("CI monitor missing")?;
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW {lpfnWndProc:Some(procedure),hInstance:module.into(),
        lpszClassName:w!("pleamar-wm-navigation-ci"),..Default::default()};
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());let mut ids=Vec::new();
    for i in 0..3 {
        let title=HSTRING::from(format!("WM navigation {i} · Español 日本語"));
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,&title,WS_OVERLAPPEDWINDOW,
            screen.work.x+40+i*30,screen.work.y+50+i*30,360,240,None,None,Some(module.into()),None) }?;
        owned.0.push(hwnd);let _=unsafe { ShowWindow(hwnd,SW_SHOWNOACTIVATE) };pump();
        ids.push(inspect(hwnd).ok_or("owned window missing")?.id);
    }
    let rules=out.join("empty rules.conf");std::fs::write(&rules,"")?;
    let options=Options {monitors:BTreeSet::from([screen.name.clone()]),all:false,process:Some(std::process::id()),
        owner:None,state:out.join("navigation ñ.json"),namespace:String::new(),seconds:None,rules,explicit_rules:true};
    let mut manager=Manager::open(&options)?;
    assert!(unsafe { SetForegroundWindow(owned.0[0]) }.as_bool());pump();
    let mut visits=Vec::new();
    for _ in 0..3 {
        let result=manager.command("emit focus_next")?.0;
        let current=inspect(unsafe { GetForegroundWindow() }).ok_or("foreground missing")?;
        assert_eq!(result["focused_window"],current.id);assert!(ids.contains(&current.id));
        visits.push(current.id);
    }
    assert_eq!(visits.iter().collect::<BTreeSet<_>>(),ids.iter().collect());
    assert_eq!(visits.last(),Some(&ids[0]));
    let previous=manager.command("emit focus_previous")?.0;
    assert_eq!(previous["focused_window"],visits[1]);

    let _=unsafe { ShowWindow(owned.0[2],SW_SHOWMINNOACTIVE) };pump();
    assert!(unsafe { SetForegroundWindow(owned.0[0]) }.as_bool());pump();
    assert_eq!(manager.command("emit focus_next")?.0["focused_window"],ids[1]);
    assert_eq!(manager.command("emit focus_next")?.0["focused_window"],ids[0]);
    let _=unsafe { EnableWindow(owned.0[1],false) };
    assert_eq!(manager.command("emit focus_next")?.0["focused_window"],ids[0]);
    let _=unsafe { EnableWindow(owned.0[1],true) };
    let _=unsafe { ShowWindow(owned.0[2],SW_SHOWNOACTIVATE) };pump();

    manager.set_mode(&screen.name,true,Some(Layout::Grid))?;
    let order=manager.modes[&screen.name].order.clone();
    assert!(unsafe { SetForegroundWindow(owned.0[0]) }.as_bool());pump();
    let at=order.iter().position(|id|id==&ids[0]).unwrap();
    assert_eq!(manager.command("emit focus_next")?.0["focused_window"],order[(at+1)%order.len()]);
    manager.set_mode(&screen.name,false,None)?;

    let foreground=unsafe { GetForegroundWindow() };
    manager.process=Some(u32::MAX);
    for command in ["emit focus_next","emit focus_previous","emit close"] {
        assert!(manager.command(command).unwrap_err().to_string().contains("outside this WM session"));
        assert_eq!(unsafe { GetForegroundWindow() },foreground);
    }
    manager.process=options.process;
    let modes=std::mem::take(&mut manager.modes);
    assert!(manager.command("emit focus_next").is_err());assert!(manager.command("emit close").is_err());
    manager.modes=modes;

    assert!(unsafe { SetForegroundWindow(owned.0[0]) }.as_bool());pump();
    unsafe { SetWindowLongPtrW(owned.0[0],GWLP_USERDATA,1); }
    assert_eq!(manager.command("emit close")?.0["close_requested"],ids[0]);pump();
    assert!(unsafe { IsWindow(Some(owned.0[0])) }.as_bool());
    assert_eq!(unsafe { GetWindowLongPtrW(owned.0[0],GWLP_USERDATA) },2,"application did not receive its close request");
    unsafe { SetWindowLongPtrW(owned.0[0],GWLP_USERDATA,0); }

    let dialog=unsafe { CreateWindowExW(WS_EX_DLGMODALFRAME,class.lpszClassName,w!("Owned navigation dialog"),WS_OVERLAPPEDWINDOW,
        screen.work.x+100,screen.work.y+120,300,180,Some(owned.0[0]),None,Some(module.into()),None) }?;
    owned.0.push(dialog);let _=unsafe { EnableWindow(owned.0[0],false) };
    let _=unsafe { ShowWindow(dialog,SW_SHOWNOACTIVATE) };
    assert!(unsafe { SetForegroundWindow(dialog) }.as_bool());pump();
    let dialog_id=inspect_kind(dialog,true).ok_or("owned dialog missing")?.id;
    assert!(manager.command("emit focus_next").is_err());
    assert_eq!(manager.command("emit close")?.0["close_requested"],dialog_id);pump();
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool());
    assert!(unsafe { IsWindow(Some(owned.0[0])) }.as_bool());
    let _=unsafe { EnableWindow(owned.0[0],true) };
    assert!(unsafe { SetForegroundWindow(owned.0[0]) }.as_bool());pump();
    assert_eq!(manager.command("emit close")?.0["close_requested"],ids[0]);pump();
    assert!(!unsafe { IsWindow(Some(owned.0[0])) }.as_bool());
    assert!(unsafe { SetForegroundWindow(owned.0[1]) }.as_bool());pump();
    assert_eq!(manager.command("emit focus_next")?.0["focused_window"],ids[2]);
    assert!(!manager.modes[&screen.name].navigation.contains(&ids[0]));
    assert!(manager.originals.is_empty());
    let report=json!({"passed":true,"environment":"github-hosted","only_owned_targets":true,
        "physical_input":false,"physical_key_dispatch":false,"whole_product_acceptance":false,"visits":visits,
        "checks":["stable free-window cycle despite Z-order changes","previous and tiled order",
            "minimized and disabled windows skipped","monitor/process scope refusal","polite close can be declined",
            "explicit modal close leaves owner open","destroyed identities retired"]});
    drop(manager);drop(owned);
    unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    std::fs::write(out.join("report.json"),serde_json::to_vec_pretty(&report)?)?;println!("{report}");Ok(())
}
