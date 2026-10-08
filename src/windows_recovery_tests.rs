//! Show-state acceptance on disposable GitHub-hosted Windows runners. This
//! test changes focus between its own windows; it must never run on a user's PC.
use super::*;
use super::super::tests::{OwnWindows, ThreadDpi, fixture_proc};

#[test]
#[ignore = "changes show states and focus on a disposable GitHub-hosted runner only"]
fn native_recovery_preserves_show_states() -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
        || std::env::var("PLEAMAR_WM_CI_RECOVERY").as_deref() != Ok("1") {
        return Err("requires the explicit recovery step on a disposable GitHub-hosted runner".into());
    }
    let dpi = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());
    let _dpi = ThreadDpi(dpi);
    let screen = monitors()?.into_iter().find(|m| m.primary).ok_or("CI monitor missing")?;
    let module = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class = WNDCLASSW { lpfnWndProc:Some(fixture_proc), hInstance:module.into(),
        lpszClassName:w!("pleamar-wm-recovery-ci"), ..Default::default() };
    assert_ne!(unsafe { RegisterClassW(&class) }, 0);
    let mut owned = OwnWindows(Vec::new());
    for (i, title) in [w!("WM recovery target"), w!("WM recovery focus sentinel")].into_iter().enumerate() {
        let hwnd = unsafe { CreateWindowExW(WS_EX_APPWINDOW, class.lpszClassName, title, WS_OVERLAPPEDWINDOW,
            screen.work.x + 40 + i as i32 * 20, screen.work.y + 50 + i as i32 * 20,
            360, 240, None, None, Some(module.into()), None) }?;
        owned.0.push(hwnd);
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    }
    pump();
    let target_hwnd = owned.0[0];
    let sentinel = owned.0[1];
    let id = inspect(target_hwnd).ok_or("CI target missing")?.id;
    let free = original(&id)?;
    let out = PathBuf::from(std::env::var_os("PLEAMAR_WM_CI_OUTPUT").ok_or("set PLEAMAR_WM_CI_OUTPUT")?);
    std::fs::create_dir(&out)?;
    let rules = out.join("empty rules.conf");
    std::fs::write(&rules, "")?;
    let options = Options { monitors:BTreeSet::from([screen.name.clone()]), all:false,
        process:Some(std::process::id()), owner:None, state:out.join("recovery ñ.json"),
        namespace:String::new(), seconds:None, rules, explicit_rules:true };
    let mut completed = Vec::new();
    for restart in [false, true] {
        for (name, shows, minimized, maximized_after) in [
            ("maximized", vec![SW_SHOWMAXIMIZED], false, true),
            ("minimized", vec![SW_SHOWMINNOACTIVE], true, false),
            ("minimized-from-maximized", vec![SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE], true, true),
        ] {
            let mut manager = Manager::open(&options)?;
            manager.set_mode(&screen.name, true, Some(Layout::Grid))?;
            assert_ne!(target(&id)?.1.bounds, free.bounds);
            for show in shows { let _ = unsafe { ShowWindow(target_hwnd, show) }; pump(); }
            assert_eq!(unsafe { IsIconic(target_hwnd) }.as_bool(), minimized);
            let was_maximized = unsafe { IsZoomed(target_hwnd) }.as_bool();
            let _ = unsafe { SetForegroundWindow(sentinel) };
            pump();
            let foreground = unsafe { GetForegroundWindow() };
            assert_eq!(foreground, sentinel, "CI could not establish its own focus sentinel");
            if restart {
                // The journal survives a lost manager; opening the replacement
                // must restore free positions even when no layout is enabled.
                drop(manager);
                manager = Manager::open(&options)?;
            } else {
                manager.set_mode(&screen.name, false, None)?;
            }
            assert_eq!(manager.originals.len(), 0, "pending {name}, restart={restart}");
            assert_eq!(rect_array(placement(target_hwnd)?.rcNormalPosition), free.normal,
                "wrong free position for {name}, restart={restart}");
            assert_eq!(unsafe { IsIconic(target_hwnd) }.as_bool(), minimized);
            assert_eq!(unsafe { IsZoomed(target_hwnd) }.as_bool(), was_maximized);
            assert_eq!(unsafe { GetForegroundWindow() }, foreground, "recovery took focus");
            if minimized {
                let _ = unsafe { ShowWindow(target_hwnd, SW_RESTORE) };
                pump();
            }
            assert_eq!(unsafe { IsZoomed(target_hwnd) }.as_bool(), maximized_after,
                "lost restore-to-maximized state for {name}, restart={restart}");
            let _ = unsafe { ShowWindow(target_hwnd, SW_SHOWNOACTIVATE) };
            pump();
            assert_eq!(target(&id)?.1.bounds, free.bounds, "wrong restored bounds for {name}");
            completed.push(json!({"state":name, "restart":restart}));
        }
    }
    drop(owned);
    unsafe { UnregisterClassW(class.lpszClassName, Some(module.into())) }?;
    let report = json!({"passed":true, "environment":"github-hosted", "physical_input":false,
        "graphical_acceptance":false, "recovery_preserves_focus":true, "cases":completed});
    std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    Ok(())
}
