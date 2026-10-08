//! Borderless native fullscreen; the session journals the original frame before
//! changing it so an interrupted transition can be recovered after a restart.
use super::*;

const FRAME: u32 = WS_CAPTION.0 | WS_THICKFRAME.0;
const EDGES: u32 = WS_EX_WINDOWEDGE.0 | WS_EX_CLIENTEDGE.0 | WS_EX_STATICEDGE.0 | WS_EX_DLGMODALFRAME.0;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Saved {
    style: u32, extended: u32, normal: [i32; 4], maximized: bool,
    monitor: String, bounds: Bounds, work: Bounds, scale:f64,
    /// A tiled window also has an earlier free position in the session journal.
    pub prior_layout: bool,
}

fn placement(hwnd: HWND) -> Result<WINDOWPLACEMENT> {
    let mut p = WINDOWPLACEMENT { length:size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    unsafe { GetWindowPlacement(hwnd, &mut p) }?;
    Ok(p)
}
fn frame(current:u32, saved:u32, mask:u32) -> u32 { (current & !mask) | (saved & mask) }
pub(super) fn active(window:&Window,screens:&[Monitor]) -> bool {
    if window.minimized || !screens.iter().any(|m|m.name==window.monitor && m.bounds==window.bounds) { return false; }
    target(&window.id).is_ok_and(|(hwnd,current)|current.bounds==window.bounds
        && unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) } as u32 & FRAME==0)
}
fn style(hwnd:HWND, index:WINDOW_LONG_PTR_INDEX, value:u32) -> Result<()> {
    unsafe {
        SetLastError(ERROR_SUCCESS);
        let previous = SetWindowLongPtrW(hwnd, index, value as isize);
        if previous == 0 && GetLastError() != ERROR_SUCCESS { return Err(windows::core::Error::from_thread().into()); }
    }
    Ok(())
}
fn change_frame(hwnd:HWND, regular:u32, extended:u32) -> Result<()> {
    style(hwnd,GWL_STYLE,frame(unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) } as u32,regular,FRAME))?;
    style(hwnd,GWL_EXSTYLE,frame(unsafe { GetWindowLongPtrW(hwnd,GWL_EXSTYLE) } as u32,extended,EDGES))?;
    Ok(())
}
fn wait(id:&str, check:impl Fn(HWND,&Window)->Result<bool>) -> Result<()> {
    let until = Instant::now()+Duration::from_secs(1);
    loop {
        pump();
        let (hwnd,window)=target(id)?;
        if check(hwnd,&window)? { return Ok(()); }
        if Instant::now()>=until { return Err("the application did not confirm its fullscreen transition".into()); }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Saved {
    pub fn read(id:&str, prior_layout:bool) -> Result<Self> {
        let (hwnd,window)=target(id)?;
        if window.minimized || !unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd) }.as_bool()
            || unsafe { IsHungAppWindow(hwnd) }.as_bool() {
            return Err("fullscreen requires a responsive window without an active modal dialog".into());
        }
        let p=placement(hwnd)?;
        let screen=monitors()?.into_iter().find(|m|m.name==window.monitor).ok_or("fullscreen monitor disconnected")?;
        Ok(Self {style:unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) } as u32,
            extended:unsafe { GetWindowLongPtrW(hwnd,GWL_EXSTYLE) } as u32,
            normal:[p.rcNormalPosition.left,p.rcNormalPosition.top,p.rcNormalPosition.right,p.rcNormalPosition.bottom],
            maximized:window.maximized,monitor:window.monitor,bounds:window.bounds,work:screen.work,scale:screen.scale,prior_layout})
    }
    pub fn enter(&self,id:&str,screen:&Monitor) -> Result<()> {
        let (hwnd,window)=target(id)?;
        if window.monitor!=screen.name || window.minimized { return Err("fullscreen target changed monitor or show state".into()); }
        if window.maximized {
            let mut p=placement(hwnd)?;
            p.flags|=WPF_ASYNCWINDOWPLACEMENT;
            p.showCmd=SW_SHOWNOACTIVATE.0 as u32;
            unsafe { SetWindowPlacement(hwnd,&p) }?;
            wait(id,|_,w|Ok(!w.maximized && !w.minimized))?;
        }
        let (hwnd,window)=target(id)?;
        if window.monitor!=screen.name { return Err("fullscreen target moved during transition".into()); }
        change_frame(hwnd,0,0)?;
        let b=&screen.bounds;
        unsafe { SetWindowPos(hwnd,None,b.x,b.y,b.width,b.height,
            SWP_NOACTIVATE|SWP_NOZORDER|SWP_NOOWNERZORDER|SWP_ASYNCWINDOWPOS|SWP_FRAMECHANGED) }?;
        wait(id,|h,w|Ok(w.bounds==screen.bounds && unsafe { GetWindowLongPtrW(h,GWL_STYLE) } as u32 & FRAME==0
            && unsafe { GetWindowLongPtrW(h,GWL_EXSTYLE) } as u32 & EDGES==0))
    }
    pub fn restore(&self,id:&str) -> Result<()> {
        let (hwnd,window)=target(id)?;
        let exact_geometry=monitors()?.iter().any(|m|m.name==self.monitor && m.bounds.contains(&self.bounds));
        change_frame(hwnd,self.style,self.extended)?;
        let mut p=placement(hwnd)?;
        p.rcNormalPosition=RECT {left:self.normal[0],top:self.normal[1],right:self.normal[2],bottom:self.normal[3]};
        p.flags=WPF_ASYNCWINDOWPLACEMENT;
        if window.minimized && self.maximized { p.flags|=WPF_RESTORETOMAXIMIZED; }
        // SW_SHOWMAXIMIZED also activates a background window. Restore its
        // maximized style and geometry with explicitly non-activating operations.
        if self.maximized && !window.minimized {
            style(hwnd,GWL_STYLE,unsafe { GetWindowLongPtrW(hwnd,GWL_STYLE) } as u32 | WS_MAXIMIZE.0)?;
        }
        p.showCmd=if window.minimized {SW_SHOWMINNOACTIVE} else if self.maximized {SW_SHOWNA} else {SW_SHOWNOACTIVATE}.0 as u32;
        unsafe { SetWindowPlacement(hwnd,&p) }?;
        let maximum=if self.maximized && !window.minimized {
            let (_,current)=target(id)?;
            let screen=monitors()?.into_iter().find(|m|m.name==current.monitor).ok_or("fullscreen recovery monitor disconnected")?;
            let ratio=screen.scale/self.scale;
            let scale=|n:i32|(f64::from(n)*ratio).round() as i32;
            Some(Bounds {x:screen.work.x+scale(self.bounds.x-self.work.x),y:screen.work.y+scale(self.bounds.y-self.work.y),
                width:screen.work.width+scale(self.bounds.width-self.work.width),
                height:screen.work.height+scale(self.bounds.height-self.work.height)})
        } else {None};
        let b=maximum.clone().unwrap_or(Bounds {x:0,y:0,width:0,height:0});
        let position=if maximum.is_some() {SET_WINDOW_POS_FLAGS(0)} else {SWP_NOMOVE|SWP_NOSIZE};
        unsafe { SetWindowPos(hwnd,None,b.x,b.y,b.width,b.height,position|
            SWP_NOACTIVATE|SWP_NOZORDER|SWP_NOOWNERZORDER|SWP_ASYNCWINDOWPOS|SWP_FRAMECHANGED) }?;
        wait(id,|h,w|Ok(w.minimized==window.minimized && (window.minimized || w.maximized==self.maximized)
            && maximum.as_ref().is_none_or(|b|w.bounds==*b)
            && (!exact_geometry || window.minimized || self.maximized || w.bounds==self.bounds)
            && (unsafe { GetWindowLongPtrW(h,GWL_STYLE) } as u32 & FRAME)==self.style & FRAME
            && (unsafe { GetWindowLongPtrW(h,GWL_EXSTYLE) } as u32 & EDGES)==self.extended & EDGES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restoring_the_frame_preserves_unrelated_application_styles() {
        let changed=WS_VISIBLE.0|WS_DISABLED.0|WS_CLIPCHILDREN.0;
        assert_eq!(frame(changed,WS_OVERLAPPEDWINDOW.0,FRAME),changed|FRAME);
        assert_eq!(frame(WS_EX_TOPMOST.0|WS_EX_APPWINDOW.0,WS_EX_CLIENTEDGE.0,EDGES),
            WS_EX_TOPMOST.0|WS_EX_APPWINDOW.0|WS_EX_CLIENTEDGE.0);
    }
}
