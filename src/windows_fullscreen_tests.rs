//! Native fullscreen acceptance on owned windows, never on personal applications.
use super::*;
use super::super::tests::{OwnWindows, ThreadDpi, fixture_proc};

#[test]
#[ignore = "owned secondary-monitor fullscreen; maximized/focus cases require disposable CI"]
fn native_fullscreen_and_recovery() -> Result<()> {
    let ci=std::env::var("GITHUB_ACTIONS").as_deref()==Ok("true")
        && std::env::var("RUNNER_ENVIRONMENT").as_deref()==Ok("github-hosted")
        && std::env::var("PLEAMAR_WM_CI_FULLSCREEN").as_deref()==Ok("1");
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());let _dpi=ThreadDpi(dpi);
    let screen=if ci { monitors()?.into_iter().find(|m|m.primary).ok_or("CI monitor missing")? }
        else { let m=select_monitor(&std::env::var("PLEAMAR_WM_TEST_MONITOR")?)?;assert!(!m.primary);m };
    let out=PathBuf::from(std::env::var_os("PLEAMAR_WM_TEST_OUTPUT").ok_or("set PLEAMAR_WM_TEST_OUTPUT")?);
    std::fs::create_dir(&out)?;
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW {lpfnWndProc:Some(fixture_proc),hInstance:module.into(),lpszClassName:w!("pleamar-wm-fullscreen-test"),..Default::default()};
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());
    for title in [w!("Owned fullscreen target"),w!("Owned fullscreen focus sentinel")] {
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW|WS_EX_CLIENTEDGE,class.lpszClassName,title,WS_OVERLAPPEDWINDOW,
            screen.work.x+80,screen.work.y+90,400,280,None,None,Some(module.into()),None) }?;
        owned.0.push(hwnd);let _=unsafe { ShowWindow(hwnd,SW_SHOWNOACTIVATE) };
    }
    pump();
    let watch=super::super::session_tests::FocusWatch::new()?;
    let (hwnd,sentinel)=(owned.0[0],owned.0[1]);
    let id=inspect(hwnd).ok_or("owned window missing")?.id;
    let free=original(&id)?;
    let regular=unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) };
    let extended=unsafe { GetWindowLongPtrW(hwnd,GWL_EXSTYLE) };
    let rules=out.join("empty.conf");std::fs::write(&rules,"")?;
    let options=Options {monitors:BTreeSet::from([screen.name.clone()]),all:false,process:Some(std::process::id()),owner:None,
        state:out.join("fullscreen ñ.json"),namespace:String::new(),seconds:None,rules,explicit_rules:true};
    let mut stages=Vec::new();
    for restart in [false,true] {
        for tiled in [false,true] {
            let mut manager=Manager::open(&options)?;
            if tiled {manager.set_mode(&screen.name,true,Some(Layout::Grid))?;}
            let before=target(&id)?.1.bounds;
            manager.toggle_fullscreen(&id)?;
            assert_eq!(target(&id)?.1.bounds,screen.bounds);
            assert!(fullscreen::active(&target(&id)?.1,std::slice::from_ref(&screen)));
            assert_eq!(unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) } as u32 & (WS_CAPTION|WS_THICKFRAME).0,0);
            manager.reconcile()?;
            assert_eq!(target(&id)?.1.bounds,screen.bounds,"layout resized fullscreen");
            let journal:Value=serde_json::from_reader(std::fs::File::open(&options.state)?)?;
            assert_eq!(journal["version"],3);
            assert!(journal["windows"].as_array().unwrap().iter().any(|w|w["id"]==id && w["fullscreen"].is_object()));
            if restart {
                drop(manager);manager=Manager::open(&options)?;
                assert_eq!(target(&id)?.1.bounds,free.bounds);
            } else {
                manager.toggle_fullscreen(&id)?;
                assert_eq!(target(&id)?.1.bounds,before);
                manager.set_mode(&screen.name,false,None)?;
                assert_eq!(target(&id)?.1.bounds,free.bounds);
            }
            assert!(manager.fullscreen.is_empty());
            assert!(!fullscreen::active(&target(&id)?.1,std::slice::from_ref(&screen)));
            assert_eq!(unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) },regular);
            assert_eq!(unsafe { GetWindowLongPtrW(hwnd,GWL_EXSTYLE) },extended);
            assert_eq!(rect_array(placement(hwnd)?.rcNormalPosition),free.normal);
            stages.push(json!({"restart":restart,"tiled":tiled,"frame_and_geometry_restored":true}));
        }
    }
    // Refusing an oversized geometry must restore the frame and leave no active fullscreen.
    let mut manager=Manager::open(&options)?;
    unsafe { SetWindowLongPtrW(hwnd,GWLP_USERDATA,1); }
    assert!(manager.toggle_fullscreen(&id).is_err());
    assert!(manager.fullscreen.is_empty() && manager.originals.is_empty());
    assert_eq!(target(&id)?.1.bounds,free.bounds);
    assert_eq!(unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) },regular);
    unsafe { SetWindowLongPtrW(hwnd,GWLP_USERDATA,0); }
    stages.push(json!({"geometry_refusal_rolls_back":true}));
    let original_scope=manager.process;
    manager.process=Some(u32::MAX);
    assert!(manager.toggle_fullscreen(&id).is_err());
    manager.process=original_scope;
    assert_eq!(target(&id)?.1.bounds,free.bounds);
    unsafe { let _=windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(hwnd,false); }
    assert!(manager.toggle_fullscreen(&id).is_err());
    unsafe { let _=windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(hwnd,true); }
    assert!(manager.originals.is_empty());
    stages.push(json!({"scope_and_modal_refusal_do_not_change_geometry":true}));
    if ci {
        assert!(unsafe { SetForegroundWindow(hwnd) }.as_bool());pump();
        manager.command("emit fullscreen")?;
        assert_eq!(target(&id)?.1.bounds,screen.bounds);
        manager.command("emit fullscreen")?;
        assert_eq!(target(&id)?.1.bounds,free.bounds);
        stages.push(json!({"active_window_command_roundtrip":true}));
        for restart in [false,true] {
            let _=unsafe { ShowWindow(hwnd,SW_SHOWMAXIMIZED) };pump();
            assert!(unsafe { SetForegroundWindow(sentinel) }.as_bool());pump();
            assert_eq!(unsafe { GetForegroundWindow() },sentinel);
            manager.toggle_fullscreen(&id)?;
            assert_eq!(unsafe { GetForegroundWindow() },sentinel,"entering fullscreen stole focus");
            if restart {drop(manager);manager=Manager::open(&options)?;}
            else {manager.toggle_fullscreen(&id)?;}
            assert_eq!(unsafe { GetForegroundWindow() },sentinel,"fullscreen restoration stole focus");
            assert!(target(&id)?.1.maximized);
            let _=unsafe { ShowWindow(hwnd,SW_SHOWNOACTIVATE) };pump();
            assert_eq!(target(&id)?.1.bounds,free.bounds);
            stages.push(json!({"maximized_restored_without_focus":true,"restart":restart}));
        }
    } else {
        assert!(!watch.events().contains(&std::process::id()),"owned test took foreground");
    }
    manager.toggle_fullscreen(&id)?;
    manager.command("quit")?;
    assert!(manager.originals.is_empty());
    assert_eq!(target(&id)?.1.bounds,free.bounds);
    assert_eq!(unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) },regular);
    if !ci { assert!(!watch.events().contains(&std::process::id()),"owned test took foreground during shutdown"); }
    stages.push(json!({"normal_session_exit_restores_fullscreen":true}));
    drop(owned);unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    let report=json!({"passed":true,"monitor":screen,"os_input":false,"maximized_ci_cases":ci,"stages":stages});
    std::fs::write(out.join("report.json"),serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");Ok(())
}
