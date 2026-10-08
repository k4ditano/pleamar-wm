use super::*;

#[test]
fn unsupported_capabilities_do_not_report_success() {
    for args in [vec!["session"], vec!["agent", "click", "0", "1", "2"], vec!["remote"],
        vec!["tile", "0", "grid", "--save", "undo.json"]] {
        assert!(execute(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
    }
    let caps = execute(&["capabilities".into()]).unwrap().unwrap();
    assert_eq!(caps["scene_launch"],true);
    assert_eq!(caps["agent_background_launch"],false);
    assert_eq!(caps["agent_window_capture"],true);
    assert_eq!(caps["agent_window_send"],true);
    assert_eq!(caps["agent_native_input"],true);
    assert_eq!(caps["agent_input_mode"],"opt-in-foreground");
    for feature in ["window_scene_provider", "rain", "snow", "ride", "dock", "pools", "remote", "phone_monitor", "independent_agent_seat"] {
        assert_eq!(caps[feature], false);
    }
}

#[test]
fn agent_catalog_includes_owned_dialogs_without_tiling_them() {
    assert!(catalog_style(WS_EX_TOOLWINDOW,true,true));
    assert!(!catalog_style(WS_EX_TOOLWINDOW,true,false));
    assert!(!catalog_style(WS_EX_TOOLWINDOW,false,true));
    assert!(catalog_style(WINDOW_EX_STYLE(0),true,true));
    assert!(!catalog_style(WINDOW_EX_STYLE(0),true,false));
    assert!(!catalog_style(WS_EX_NOACTIVATE,true,true));
    assert!(catalog_style(WS_EX_APPWINDOW,true,false));
}

#[test]
fn bounds_do_not_wrap_or_accept_empty_windows() {
    let desktop = Bounds { x: -1920, y: -200, width: 1920, height: 1080 };
    assert!(desktop.contains(&Bounds { x: -1910, y: -190, width: 1900, height: 1000 }));
    assert!(!desktop.contains(&Bounds { x: -1910, y: -190, width: 0, height: 1000 }));
    assert!(!desktop.contains(&Bounds { x: i32::MAX, y: 0, width: 100, height: 100 }));
}

#[test]
fn rejects_invalid_window_identifiers() {
    for id in ["", "1", "a:b:c", "1:2:0:3", "1:2:ffffffffffffffffffffffff:3"] {
        assert!(target(id).is_err());
    }
}

pub(super) struct OwnWindows(pub(super) Vec<HWND>);
impl Drop for OwnWindows {
    fn drop(&mut self) {
        for hwnd in self.0.drain(..) { let _ = unsafe { DestroyWindow(hwnd) }; }
    }
}

pub(super) struct ThreadDpi(pub(super) DPI_AWARENESS_CONTEXT);
impl Drop for ThreadDpi {
    fn drop(&mut self) { unsafe { SetThreadDpiAwarenessContext(self.0); } }
}

pub(super) unsafe extern "system" fn fixture_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // Model an application that rejects resizing. Only these owned test HWNDs
    // use this procedure; no application is subclassed or injected into.
    if message == WM_WINDOWPOSCHANGING && unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 1 {
        let position = unsafe { &mut *(l.0 as *mut WINDOWPOS) };
        if !position.flags.contains(SWP_NOSIZE) { position.cx = 400; }
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}

#[test]
#[ignore = "creates only test windows on an explicitly named non-primary monitor"]
fn native_layouts_and_restore_on_secondary_monitor() -> Result<()> {
    let requested = std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    if !requested.starts_with(r"\\.\DISPLAY") { return Err("name the secondary display explicitly".into()); }
    let old = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // -1 is DPI_AWARENESS_CONTEXT_UNAWARE, a valid previous context.
    assert!(!old.0.is_null());
    let _dpi = ThreadDpi(old);
    let monitor = select_monitor(&requested)?;
    if monitor.primary { return Err("refusing to test on the primary monitor".into()); }
    let foreground = unsafe { GetForegroundWindow() };
    let module = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class = WNDCLASSW { lpfnWndProc: Some(fixture_proc), hInstance: module.into(),
        lpszClassName: w!("pleamar-wm-owned-acceptance"), ..Default::default() };
    assert_ne!(unsafe { RegisterClassW(&class) }, 0);
    let mut owned = OwnWindows(Vec::new());
    let mut original = Vec::new();
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let temp = std::env::temp_dir().join(format!("pleamar-wm test ñ {} {nonce}", std::process::id()));
    std::fs::create_dir(&temp)?;
    for i in 0..3 {
        let rect = Bounds { x: monitor.work.x + 30 + i * 40, y: monitor.work.y + 40 + i * 45,
            width: 400, height: 280 };
        assert!(monitor.work.contains(&rect));
        let caption: Vec<u16> = format!("pleamar-wm native test · ñ · {i}").encode_utf16().chain([0]).collect();
        let hwnd = unsafe { CreateWindowExW(WS_EX_APPWINDOW, class.lpszClassName, PCWSTR(caption.as_ptr()),
            WS_OVERLAPPEDWINDOW, rect.x, rect.y, rect.width, rect.height, None, None, Some(module.into()), None) }?;
        owned.0.push(hwnd);
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        pump();
        let window = inspect(hwnd).ok_or("test window is missing from the native catalog")?;
        assert!(window.title.contains('ñ'));
        assert_eq!(window.monitor, requested);
        original.push(SavedWindow { id: window.id, bounds: rect });
    }
    let ids: Vec<_> = original.iter().map(|w| w.id.clone()).collect();
    let seen = windows()?;
    for id in &ids { assert!(seen.iter().any(|w| &w.id == id)); }
    unsafe { SetWindowLongPtrW(owned.0[1], GWLP_USERDATA, 1); }
    let rollback = temp.join("rejected layout ñ.json");
    assert!(tile(&monitor, Layout::Columns, &ids, &rollback).is_err());
    assert!(rollback.exists());
    for old in &original { assert_eq!(target(&old.id)?.1.bounds, old.bounds); }
    unsafe { SetWindowLongPtrW(owned.0[1], GWLP_USERDATA, 0); }
    std::fs::remove_file(rollback)?;
    let mut completed = Vec::new();
    for kind in ["left", "right", "columns", "rows", "grid"] {
        let before = unsafe { GetForegroundWindow() };
        let live = select_monitor(&requested)?;
        assert!(!live.primary && live.bounds == monitor.bounds);
        let save = temp.join(format!("undo {kind} ñ.json"));
        tile(&live, kind.parse()?, &ids, &save)?;
        let expected = layout::arrange((&monitor.work).into(), 3, kind.parse()?, (8.0 * monitor.scale).round() as i32)?;
        for (id, expected) in ids.iter().zip(expected) {
            let (hwnd, found) = target(id)?;
            assert_eq!(found.process, std::process::id());
            assert_eq!(found.bounds, Bounds::from(expected));
            assert!(monitor.work.contains(&found.bounds));
            assert!(!WINDOW_EX_STYLE(unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32).contains(WS_EX_TOPMOST));
        }
        // Overwriting recovery data must fail before moving anything.
        assert!(tile(&monitor, Layout::Columns, &ids, &save).is_err());
        restore_layout(&save)?;
        for old in &original { assert_eq!(target(&old.id)?.1.bounds, old.bounds); }
        assert_eq!(unsafe { GetForegroundWindow() }, before);
        std::fs::remove_file(save)?;
        completed.push(kind);
    }
    state(&ids[0], true)?;
    assert!(target(&ids[0])?.1.minimized);
    state(&ids[0], false)?;
    assert!(!target(&ids[0])?.1.minimized);
    let wrong_monitor = Monitor { name: "disconnected".into(), ..monitor.clone() };
    let bad = temp.join("must not exist.json");
    assert!(tile(&wrong_monitor, Layout::Grid, &ids, &bad).is_err());
    assert!(!bad.exists());
    assert!(tile(&monitor, Layout::Grid, &[ids[0].clone(), ids[0].clone()], &bad).is_err());
    assert!(!bad.exists());
    drop(owned);
    for id in &ids { assert!(target(id).is_err()); }
    unsafe { UnregisterClassW(class.lpszClassName, Some(module.into())) }?;
    assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    std::fs::remove_dir(temp)?;
    println!("{}", json!({"monitor": requested, "primary": false, "layouts": completed,
        "windows": 3, "undo": true, "minimize_restore": true, "focus_unchanged": true,
        "physical_input_sent": false, "closed_window_rejected": true, "unicode_paths": true,
        "rejected_resize_rolled_back": true}));
    Ok(())
}
