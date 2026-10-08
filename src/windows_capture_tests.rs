use super::*;
use super::tests::{OwnWindows, ThreadDpi};

pub(super) unsafe extern "system" fn paint_fixture(hwnd:HWND,message:u32,w:WPARAM,l:LPARAM) -> LRESULT {
    if message==WM_PAINT { unsafe {
        let mut paint=PAINTSTRUCT::default();
        let dc=BeginPaint(hwnd,&mut paint);
        let color=GetWindowLongPtrW(hwnd,GWLP_USERDATA) as u32;
        let brush=CreateSolidBrush(COLORREF(color));
        let mut rect=RECT::default();
        let _=GetClientRect(hwnd,&mut rect);
        FillRect(dc,&rect,brush);
        let _=DeleteObject(brush.into());
        let _=EndPaint(hwnd,&paint);
        return LRESULT(0);
    } }
    unsafe { DefWindowProcW(hwnd,message,w,l) }
}

fn next(capture:&mut capture::Capture, expected:[u8;4]) -> Result<capture::Picture> {
    let until=Instant::now()+Duration::from_secs(8);
    let mut received=0; let mut last=None;
    while Instant::now()<until {
        pump();
        if let Some(picture)=capture.next(16_777_216)? {
            let middle=((picture.size.1/2 * picture.size.0 + picture.size.0/2)*4) as usize;
            received+=1;last=Some((picture.size,picture.pixels[middle..middle+4].to_vec()));
            if picture.pixels[middle..middle+4]==expected { return Ok(picture); }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(format!("source pixels did not arrive: expected {expected:?}, received {received} frames, last {last:?}").into())
}

#[test]
#[ignore = "WGC of only owned colored test windows on an explicit non-primary display"]
fn native_persistent_window_capture() -> Result<()> {
    let requested=std::env::var("PLEAMAR_WM_TEST_MONITOR")?;
    let monitor=select_monitor(&requested)?;
    assert!(!monitor.primary);
    let dpi=unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!dpi.0.is_null());
    let _dpi=ThreadDpi(dpi);
    let foreground=unsafe { GetForegroundWindow() };
    let module=unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
    let class=WNDCLASSW { lpfnWndProc:Some(paint_fixture),hInstance:module.into(),
        lpszClassName:w!("pleamar-wm-window-pixels"),..Default::default() };
    assert_ne!(unsafe { RegisterClassW(&class) },0);
    let mut owned=OwnWindows(Vec::new());
    for i in 0..2 {
        let x=monitor.work.x+40+i*510; let y=monitor.work.y+60;
        assert!(monitor.work.contains(&Bounds {x,y,width:500,height:360}));
        let hwnd=unsafe { CreateWindowExW(WS_EX_APPWINDOW,class.lpszClassName,w!("Native captured pixels ñ"),WS_OVERLAPPEDWINDOW,
            x,y,400,280,None,None,Some(module.into()),None) }?;
        owned.0.push(hwnd);
        unsafe { SetWindowLongPtrW(hwnd,GWLP_USERDATA,0x0020c060); let _=ShowWindow(hwnd,SW_SHOWNOACTIVATE); }
    }
    pump();
    let mut sizes=Vec::new();
    // Reopening after every apartment is destroyed exercises factory lifetime.
    for round in 0..3 {
        eprintln!("capture round {round}: opening two sources");
        let device=capture::Device::new(None)?;
        assert!(capture::Capture::new(device.clone(),owned.0[0],1).is_err());
        let mut first=capture::Capture::new(device.clone(),owned.0[0],16_777_216)?;
        let mut second=capture::Capture::new(device,owned.0[1],16_777_216)?;
        let picture=next(&mut first,[0x20,0xc0,0x60,255])?;
        let initial=picture.size;
        assert_eq!(picture.pixels.len(),initial.0 as usize*initial.1 as usize*4);
        drop(picture);
        let second_size=next(&mut second,[0x20,0xc0,0x60,255])?.size;
        // Rendering can pause while the source keeps drawing. Resuming must
        // deliver its final static repaint, not require another source update.
        for color in [0x003080e0,0x00e07030,0x00d03080] {
            unsafe {
                SetWindowLongPtrW(owned.0[0],GWLP_USERDATA,color);
                let _=InvalidateRect(Some(owned.0[0]),None,false);
            }
            let until=Instant::now()+Duration::from_millis(100);
            while Instant::now()<until { pump();std::thread::sleep(Duration::from_millis(10)); }
        }
        next(&mut first,[0xd0,0x30,0x80,255])?;
        unsafe {
            SetWindowLongPtrW(owned.0[0],GWLP_USERDATA,0x0020c060);
            let _=InvalidateRect(Some(owned.0[0]),None,false);
        }
        next(&mut first,[0x20,0xc0,0x60,255])?;
        for (color,bgra) in [(0x00d03080,[0xd0,0x30,0x80,255]),(0x0020c060,[0x20,0xc0,0x60,255])] {
            eprintln!("capture round {round}: repaint {bgra:?}");
            unsafe {
                SetWindowLongPtrW(owned.0[0],GWLP_USERDATA,color);
                let _=InvalidateRect(Some(owned.0[0]),None,false);
            }
            assert_eq!(next(&mut first,bgra)?.size,initial);
            // A static source need not produce another frame. Repaint the other
            // window too, so this checks independent live updates, not polling.
            unsafe {
                SetWindowLongPtrW(owned.0[1],GWLP_USERDATA,color);
                let _=InvalidateRect(Some(owned.0[1]),None,false);
            }
            assert_eq!(next(&mut second,bgra)?.size,second_size);
        }
        if round==0 {
            eprintln!("capture round {round}: resizing source");
            unsafe { SetWindowPos(owned.0[0],None,0,0,500,360,SWP_NOMOVE|SWP_NOZORDER|SWP_NOACTIVATE) }?;
            let mut resized=false;let until=Instant::now()+Duration::from_secs(8);
            while Instant::now()<until {
                let frame=next(&mut first,[0x20,0xc0,0x60,255])?;
                if frame.size!=initial { sizes.push(frame.size);resized=true;break; }
            }
            assert!(resized);
            unsafe { SetWindowPos(owned.0[0],None,0,0,400,280,SWP_NOMOVE|SWP_NOZORDER|SWP_NOACTIVATE) }?;
            let until=Instant::now()+Duration::from_secs(8);
            loop {
                if next(&mut first,[0x20,0xc0,0x60,255])?.size==initial { break; }
                assert!(Instant::now()<until,"capture did not return to the original size");
            }
            assert!(first.next(1).is_err(),"capture accepted an exhausted memory budget");
        }
        if round==2 {
            let hwnd=owned.0.pop().unwrap();
            unsafe { DestroyWindow(hwnd) }?;
            let until=Instant::now()+Duration::from_secs(5);
            while !second.closed()&&Instant::now()<until { pump();std::thread::sleep(Duration::from_millis(10)); }
            assert!(second.closed());assert!(second.next(16_777_216).is_err());
        }
    }
    let focus_unchanged=unsafe { GetForegroundWindow() }==foreground;
    drop(owned);
    unsafe { UnregisterClassW(class.lpszClassName,Some(module.into())) }?;
    println!("{}",json!({"actual_wgc_pixels":true,"shared_device_two_windows":true,"pixel_updates":true,
        "resize":sizes,"reopened_apartments":3,"closed_capture_rejected":true,"budget_rejected":true,
        "slow_consumer_latest_static_frame":true,
        "monitor":requested,"primary":false,"physical_input":false,"focus_unchanged":focus_unchanged}));
    assert!(focus_unchanged);
    Ok(())
}
