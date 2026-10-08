//! Scene size requests must not block capture while an application handles them.
use super::*;

struct Pending { bounds:Bounds, ask:Option<(i32,i32)>, monitor:String, transfer:bool, until:Instant }
pub(super) struct Configure {
    wanted:Option<(i32,i32)>, destination:Option<String>, pending:Option<Pending>, next:Instant, deferred:bool,
}
impl Default for Configure {
    fn default() -> Self { Self { wanted:None,destination:None,pending:None,next:Instant::now(),deferred:false } }
}
impl Configure {
    pub(super) fn send(&mut self, destination:String) {
        self.destination=Some(destination);
        self.deferred=false;
    }
    pub(super) fn ask(&mut self,w:i32,h:i32) -> Result<()> {
        if w<0 || h<0 { return Err("negative native window size".into()); }
        // Match Linux's protection against a scene's transient animation sizes.
        if (w>0 && w<32) || (h>0 && h<32) { return Ok(()); }
        self.wanted=if (w,h)==(0,0) { None } else { Some((w,h)) };
        // A posted request cannot be retracted, but releasing size control
        // must also prevent replaying it after a minimize/restore cycle.
        if self.wanted.is_none() { if let Some(pending)=&mut self.pending { pending.ask=None; } }
        self.deferred=false;
        Ok(())
    }
    pub(super) fn wake(&mut self) { self.deferred=false; }
    pub(super) fn pixels(&self) -> u64 {
        self.pending.as_ref().map_or(0,|p|p.bounds.width as u64*p.bounds.height as u64)
    }
    pub(super) fn wait(&self,now:Instant) -> Option<Duration> {
        (self.pending.is_some() || self.destination.is_some() || (self.wanted.is_some() && !self.deferred))
            .then(||self.next.saturating_duration_since(now))
    }
    pub(super) fn tick(&mut self,scope:&Scope,created:Option<u64>,id:&str,outputs:&Outputs,budget:u64) -> Result<()> {
        let now=Instant::now();
        if self.wait(now).is_none_or(|wait|!wait.is_zero()) { return Ok(()); }
        self.next=now+Duration::from_secs_f64(1.0/30.0);
        let eligible=|| -> Result<(HWND,Window)> {
            let (hwnd,window)=target(id)?;
            if !scope.actions || !scope.allows(&window,created) { return Err("resize left its authorized window scope".into()); }
            Ok((hwnd,window))
        };
        if let Some(pending)=self.pending.take() {
            let (_,window)=eligible()?;
            if window.minimized || window.maximized {
                if pending.transfer { return Err("window state changed while sending it to another monitor".into()); }
                if let Some(ask)=pending.ask { self.wanted.get_or_insert(ask); }
                self.deferred=true;
                return Ok(());
            }
            if window.bounds!=pending.bounds || window.monitor!=pending.monitor {
                if now>=pending.until { return Err(format!("{id} did not accept the requested scene placement").into()); }
                self.pending=Some(pending);
                return Ok(());
            }
        }
        if let Some(name)=self.destination.take() {
            // Pin the physical display name at the time of the action. A
            // reordered or removed output must never send it somewhere else.
            let index=outputs.0.iter().find(|(_,n)|n==&name).map(|(i,_)|*i)
                .ok_or("the destination scene output was removed")?;
            if outputs.destination(index,scope)?!=name { return Err("the destination scene output changed".into()); }
            let (hwnd,window)=eligible()?;
            normal(&window)?;
            let screens=monitors()?;
            let from=screens.iter().find(|m|m.name==window.monitor).ok_or("source monitor disconnected")?;
            let to=screens.iter().find(|m|m.name==name).ok_or("destination monitor disconnected")?;
            if from.name!=to.name {
                let bounds=transfer::geometry(&window.bounds,from,to,budget)?;
                unsafe { SetWindowPos(hwnd,None,bounds.x,bounds.y,bounds.width,bounds.height,
                    SWP_NOACTIVATE|SWP_NOZORDER|SWP_NOOWNERZORDER|SWP_ASYNCWINDOWPOS) }?;
                self.pending=Some(Pending {bounds,ask:None,monitor:name,transfer:true,until:now+Duration::from_secs(1)});
                return Ok(());
            }
        }
        let Some(ask)=self.wanted.take() else { return Ok(()); };
        let (hwnd,window)=eligible()?;
        if window.minimized || window.maximized {
            self.wanted=Some(ask);self.deferred=true;return Ok(());
        }
        normal(&window)?;
        let monitor=select_monitor(&window.monitor)?;
        if !monitor.bounds.contains(&window.bounds) { return Err("bring the window wholly onto its monitor before resizing".into()); }
        let bounds=geometry(&window.bounds,&monitor.work,monitor.scale,ask,budget)?;
        if bounds==window.bounds { return Ok(()); }
        unsafe { SetWindowPos(hwnd,None,bounds.x,bounds.y,bounds.width,bounds.height,
            SWP_NOACTIVATE|SWP_NOZORDER|SWP_NOOWNERZORDER|SWP_ASYNCWINDOWPOS) }?;
        self.pending=Some(Pending {bounds,ask:Some(ask),monitor:window.monitor,transfer:false,until:now+Duration::from_secs(1)});
        Ok(())
    }
}

fn geometry(current:&Bounds,work:&Bounds,scale:f64,ask:(i32,i32),budget:u64) -> Result<Bounds> {
    if !scale.is_finite() || scale<=0.0 { return Err("invalid monitor scale".into()); }
    let dimension=|wanted:i32,old:i32| -> Result<i32> {
        let pixels=if wanted==0 { f64::from(old) } else { (f64::from(wanted)*scale).round() };
        if wanted<0 || !(1.0..=8192.0).contains(&pixels) { return Err("native window size exceeds capture limits".into()); }
        Ok(pixels as i32)
    };
    let width=dimension(ask.0,current.width)?;
    let height=dimension(ask.1,current.height)?;
    if width>work.width || height>work.height { return Err("requested scene size exceeds this monitor's work area".into()); }
    if width as u64*height as u64>budget { return Err("requested scene size exceeds the shared capture budget".into()); }
    let last_x=i32::try_from(i64::from(work.x)+i64::from(work.width)-i64::from(width))?;
    let last_y=i32::try_from(i64::from(work.y)+i64::from(work.height)-i64::from(height))?;
    Ok(Bounds {x:current.x.clamp(work.x,last_x),y:current.y.clamp(work.y,last_y),width,height})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scene_sizes_keep_physical_bounds_on_their_monitor() {
        let current=Bounds {x:-400,y:700,width:380,height:280};
        let work=Bounds {x:-1920,y:210,width:1920,height:1020};
        assert_eq!(geometry(&current,&work,1.25,(640,400),16_777_216).unwrap(),
            Bounds {x:-800,y:700,width:800,height:500});
        assert_eq!(geometry(&current,&work,1.5,(0,300),16_777_216).unwrap(),
            Bounds {x:-400,y:700,width:380,height:450});
        assert_eq!(geometry(&current,&work,2.0,(500,400),16_777_216).unwrap(),
            Bounds {x:-1000,y:430,width:1000,height:800});
        for ask in [(2000,400),(600,1000),(-1,200),(i32::MAX,200)] {
            assert!(geometry(&current,&work,1.25,ask,16_777_216).is_err());
        }
        assert!(geometry(&current,&work,1.25,(640,400),399_999).is_err());
        assert!(geometry(&current,&work,f64::NAN,(640,400),16_777_216).is_err());
    }
    #[test]
    fn animation_sizes_coalesce_without_a_request_queue() {
        let mut configure=Configure::default();
        for width in 200..2000 { configure.ask(width,400).unwrap(); }
        assert_eq!(configure.wanted,Some((1999,400)));
        configure.ask(1,300).unwrap();assert_eq!(configure.wanted,Some((1999,400)));
        assert!(configure.ask(-1,300).is_err());
        configure.deferred=true;assert!(configure.wait(Instant::now()).is_none());
        configure.wake();assert!(configure.wait(Instant::now()).is_some());
        configure.ask(0,0).unwrap();assert!(configure.wait(Instant::now()).is_none());
    }
    #[test]
    fn send_coalesces_by_display_name_without_losing_the_scene_size() {
        let mut configure=Configure::default();
        configure.ask(640,400).unwrap();
        configure.send("LEFT".into());
        configure.send("RIGHT".into());
        assert_eq!(configure.destination.as_deref(),Some("RIGHT"));
        assert_eq!(configure.wanted,Some((640,400)));
        configure.ask(0,0).unwrap();
        assert!(configure.wanted.is_none());
        assert_eq!(configure.destination.as_deref(),Some("RIGHT"));
        assert!(configure.wait(Instant::now()).is_some());
    }
}
