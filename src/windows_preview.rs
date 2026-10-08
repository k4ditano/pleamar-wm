//! Live native window pictures, with optional native window actions. Input
//! inside an application still goes through its original Windows window.
use super::*;
use pleamar::scene::{NestEvent, PieceContent, ToNest, ToRender, WindowPiece};
use std::{cell::Cell, sync::{OnceLock, mpsc::{self, Sender, TryRecvError}}};
use windows::Win32::UI::Accessibility::*;

#[path = "windows_configure.rs"]
mod configure;

#[derive(Clone)]
struct Scope { monitor:String, process:Option<u32>, actions:bool }
impl Scope {
    fn dock_actions(&self) -> Result<()> {
        if !self.actions { return Err("dock changes require --window-actions; this scene is view-only".into()); }
        if self.process.is_some() { return Err("dock changes are unavailable with a single-process preview scope".into()); }
        Ok(())
    }
    fn launch(&self, commands:&mut launch::Launches, command:&str) -> Result<u32> {
        if !self.actions { return Err("scene launch requires --window-actions; this scene is view-only".into()); }
        if self.process.is_some() { return Err("scene launch is unavailable with a single-process preview scope".into()); }
        commands.start(command)
    }
    fn allows(&self, window:&Window, created:Option<u64>) -> bool {
        (self.monitor=="all" || window.monitor==self.monitor) && self.process.is_none_or(|pid|window.process==pid)
            && created.is_none_or(|stamp|window.id.ends_with(&format!(":{stamp:x}")))
    }
}

#[derive(Default)]
struct Outputs(Vec<(usize,String)>);
impl Outputs {
    fn new(mut copies:Vec<(usize,String)>) -> Result<Self> {
        copies.sort();copies.dedup();
        if copies.len()>64 || copies.iter().any(|(index,name)|*index>=4 || name.is_empty())
            || copies.iter().any(|(i,name)|copies.iter().any(|(j,other)|i!=j && name==other)) {
            return Err("ambiguous native scene outputs; use distinct screens: each copies".into());
        }
        Ok(Self(copies))
    }
    fn destination(&self, index:usize, scope:&Scope) -> Result<&str> {
        if !scope.actions { return Err("window actions require --window-actions; this scene is view-only".into()); }
        let mut names=self.0.iter().filter(|(i,_)|*i==index).map(|(_,name)|name.as_str());
        let name=names.next().ok_or("the destination has no live scene output")?;
        if names.any(|other|other!=name) { return Err("the destination scene output is ambiguous".into()); }
        if scope.monitor!="all" && scope.monitor!=name {
            return Err("sending to another monitor requires --preview-monitor all".into());
        }
        Ok(name)
    }
    fn screen(&self, source:&str, scope:&Scope) -> Option<usize> {
        self.0.iter().find(|(_,name)|name==source).map(|(index,_)|*index)
            // An explicitly chosen source can be previewed on a different
            // display. It belongs to the first live copy of that scene.
            .or_else(||(scope.monitor!="all").then(||self.0.first().map(|(i,_)|*i)).flatten())
    }
}
static SCOPE:OnceLock<Scope> = OnceLock::new();
thread_local! { static CATALOG_DIRTY:Cell<bool> = const { Cell::new(true) }; }
unsafe extern "system" fn changed(_:HWINEVENTHOOK,_:u32,_:HWND,object:i32,child:i32,_:u32,_:u32) {
    if object == 0 && child == 0 { CATALOG_DIRTY.set(true); }
}
struct Hooks(Vec<HWINEVENTHOOK>);
impl Hooks {
    fn add(&mut self, first:u32,last:u32,pid:u32) -> Result<()> {
        let hook=unsafe { SetWinEventHook(first,last,None,Some(changed),pid,0,WINEVENT_OUTOFCONTEXT|WINEVENT_SKIPOWNPROCESS) };
        if hook.is_invalid() { return Err("native window event subscription failed".into()); }
        self.0.push(hook);Ok(())
    }
}
impl Drop for Hooks { fn drop(&mut self) { for hook in self.0.drain(..) { let _=unsafe { UnhookWinEvent(hook) }; } } }

pub(super) fn prepare(args:&[String]) -> Result<Vec<String>> {
    let mut monitor=std::env::var("PLEAMAR_WM_PREVIEW_MONITOR").ok();
    let mut process=std::env::var("PLEAMAR_WM_PREVIEW_PROCESS").ok();
    let mut actions=false;
    let mut forwarded=Vec::new(); let mut it=args.iter();
    while let Some(argument)=it.next() {
        match argument.as_str() {
            "--preview-monitor" => monitor=Some(it.next().ok_or("preview monitor missing")?.clone()),
            "--preview-process" => process=Some(it.next().ok_or("preview process missing")?.clone()),
            "--window-actions" => actions=true,
            _ => forwarded.push(argument.clone()),
        }
    }
    let Some(monitor)=monitor else {
        if process.is_some() || actions { return Err("window previews and actions need an explicit preview monitor".into()); }
        return Ok(forwarded);
    };
    let monitor=if monitor=="all" { monitor } else { select_monitor(&monitor)?.name };
    let process=process.map(|v|v.parse::<u32>()).transpose()?;
    if process==Some(0) || process==Some(std::process::id()) { return Err("invalid preview process".into()); }
    SCOPE.set(Scope { monitor,process,actions }).map_err(|_|"window preview is already configured")?;
    pleamar::provide_windows(start);
    Ok(forwarded)
}

fn start(max:usize,to_render:Sender<ToRender>) -> Option<pleamar::NestSender> {
    let scope=SCOPE.get()?.clone();
    let (commands,receive)=mpsc::channel();
    let (ready,started)=mpsc::sync_channel(1);
    let wake=wait::Wake::new().ok()?;
    let command_wake=wake.clone();
    std::thread::Builder::new().name("native-window-pictures".into()).spawn(move || {
        match Preview::new(max,scope,to_render.clone(),wake) {
            Ok(mut preview) => {
                let _=ready.send(Ok(()));
                if let Err(error)=preview.run(receive) { eprintln!("windows preview: {error}"); }
            },
            Err(error) => { let _=ready.send(Err(error.to_string())); },
        }
    }).ok()?;
    match started.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Some(Box::new(move |message| { let _=commands.send(message); command_wake.signal(); })),
        result => { eprintln!("windows preview could not start: {result:?}"); None },
    }
}

#[derive(Default)]
struct Retry { failures:u32, at:Option<Instant> }
impl Retry {
    fn failed(&mut self, now:Instant) {
        let seconds=(1u64<<self.failures.min(5)).min(30);
        self.failures=self.failures.saturating_add(1);
        self.at=Some(now+Duration::from_secs(seconds));
    }
    fn wait(&self, now:Instant) -> Duration { self.at.map_or(Duration::ZERO,|at|at.saturating_duration_since(now)) }
    fn reset(&mut self) { *self=Self::default(); }
}

struct Slot { window:Window, capture:Option<capture::Capture>, received:bool, pixels:u64, born:Instant, next_frame:Instant,
    configure:configure::Configure, visible:bool, retry:Retry, screen:usize, scale:f64, last_size:Option<(u32,u32)>, fullscreen:bool }
impl Slot {
    // A minimized window still has its last texture in the renderer. It must
    // count towards the same bound even after its capture resources are freed.
    fn pixels(&self) -> u64 {
        self.pixels.max(self.configure.pixels())
            .max(self.capture.as_ref().and_then(|c|c.size().ok()).map_or(0,|(w,h)|w as u64*h as u64))
    }
    fn retry_wait(&self, now:Instant) -> Option<Duration> {
        (self.visible && !self.window.minimized && self.capture.is_none()).then(||self.retry.wait(now))
    }
}
struct Preview {
    scope:Scope, created:Option<u64>, device:capture::DeviceCache,
    slots:Vec<Option<Slot>>, send:Sender<ToRender>, _hooks:Hooks,
    consumed:Option<mpsc::Receiver<String>>, waiter:wait::Waiter, warned:HashSet<&'static str>,
    focused:Option<usize>,
    visible:HashSet<usize>,
    screens:Vec<Monitor>,
    outputs:Outputs,
    launches:launch::Launches,
    dock:dock::Dock,
    programs:std::collections::BTreeMap<String,Option<dock::Program>>,
    metadata:dock::Lookup,
    activations:dock::Activations,
}
impl Preview {
    fn new(max:usize,scope:Scope,send:Sender<ToRender>,wake:std::sync::Arc<wait::Wake>) -> Result<Self> {
        if !(1..=64).contains(&max) { return Err("invalid native window slot count".into()); }
        let created=match scope.process {
            Some(pid) => Some(windows()?.iter().find(|w|w.process==pid).and_then(|w|w.id.rsplit(':').next())
                .and_then(|v|u64::from_str_radix(v,16).ok()).ok_or("preview process has no eligible window")?),
            None => None,
        };
        let mut hooks=Hooks(Vec::new());
        hooks.add(EVENT_OBJECT_CREATE,EVENT_OBJECT_NAMECHANGE,scope.process.unwrap_or(0))?;
        hooks.add(EVENT_SYSTEM_MINIMIZESTART,EVENT_SYSTEM_MINIMIZEEND,scope.process.unwrap_or(0))?;
        hooks.add(EVENT_OBJECT_CLOAKED,EVENT_OBJECT_UNCLOAKED,scope.process.unwrap_or(0))?;
        // A different process taking focus clears this scene's focused slot.
        hooks.add(EVENT_SYSTEM_FOREGROUND,EVENT_SYSTEM_FOREGROUND,0)?;
        let waiter=wait::Waiter::new(wake.clone())?;
        let dock=dock::Dock::new(pleamar::config_dir().ok_or("Windows configuration directory unavailable")?.join("wm/windows-dock.json"))?;
        let metadata=dock::Lookup::new(wake.clone())?;
        let activations=dock::Activations::new(wake.clone())?;
        Ok(Self { scope,created,device:capture::DeviceCache::new(Some(wake)),slots:(0..max).map(|_|None).collect(),
            send,_hooks:hooks,consumed:None,waiter,warned:HashSet::new(),focused:None,visible:HashSet::new(),screens:Vec::new(),outputs:Outputs::default(),launches:launch::Launches::default(),dock,programs:Default::default(),metadata,activations })
    }
    fn tell(&self,event:NestEvent) -> Result<()> { self.send.send(ToRender::Nest(event))?; Ok(()) }
    fn order(&self) -> Result<()> {
        self.tell(NestEvent::Order(self.slots.iter().enumerate()
            .filter_map(|(i,s)|s.as_ref().map(|_|i)).collect()))
    }
    fn focus(&mut self) -> Result<()> {
        let foreground=unsafe { GetForegroundWindow() };
        let root=unsafe { GetAncestor(foreground,GA_ROOTOWNER) };
        let id=Identity::read(root).map(Identity::token);
        let now=self.slots.iter().position(|s|s.as_ref().is_some_and(|s|id.as_ref()==Some(&s.window.id)));
        if now!=self.focused { self.tell(NestEvent::Focused(now))?;self.focused=now; }
        Ok(())
    }
    fn capture(&mut self,i:usize) {
        let existing:u64=self.slots.iter().enumerate().filter(|(k,_)|*k!=i)
            .filter_map(|(_,s)|s.as_ref()).map(Slot::pixels).sum();
        let Some(slot)=self.slots[i].as_mut() else { return; };
        if slot.window.minimized || !slot.visible { return; }
        let result=(|| -> Result<_> { let (hwnd,_)=target(&slot.window.id)?;
            Ok(capture::Capture::new(self.device.get()?,hwnd,16_777_216u64.saturating_sub(existing))?) })();
        match result {
            Ok(capture) => { slot.capture=Some(capture);slot.received=false;slot.born=Instant::now();slot.next_frame=Instant::now(); },
            Err(error) => {
                slot.retry.failed(Instant::now());
                eprintln!("windows preview: could not capture {}: {error}; retry scheduled",slot.window.id);
            },
        }
    }
    fn refresh(&mut self) -> Result<()> {
        let screens=monitors()?;
        self.screens=screens.clone();
        let live:Vec<_>=windows()?.into_iter().filter(|w| w.process!=std::process::id()
            && self.scope.allows(w,self.created)).filter_map(|window| {
                let screen=self.outputs.screen(&window.monitor,&self.scope)?;
                let scale=screens.iter().find(|m|m.name==window.monitor)?.scale;
                Some((window,screen,scale))
            }).collect();
        self.programs.retain(|id,_|live.iter().any(|(window,_,_)|&window.id==id));
        for i in 0..self.slots.len() {
            let remove=self.slots[i].as_ref().is_some_and(|s|!live.iter().any(|(w,_,_)|w.id==s.window.id));
            if remove {
                self.slots[i]=None;
                self.tell(NestEvent::Closed(i))?;
            }
        }
        for (window,screen,scale) in live {
            let fullscreen=fullscreen::active(&window,&screens);
            let previous_app=self.programs.get(&window.id).and_then(Option::as_ref).map(|p|p.key()).unwrap_or_else(||window.app.clone());
            if self.slots.iter().flatten().any(|slot|slot.window.id==window.id
                && (slot.window.title!=window.title || slot.window.app!=window.app)) { self.programs.remove(&window.id); }
            if !self.programs.contains_key(&window.id) && self.metadata.request(&window) { self.programs.insert(window.id.clone(),None); }
            let program=self.programs.get(&window.id).cloned().flatten();
            let app_id=program.as_ref().map(|p|p.key()).unwrap_or_else(||window.app.clone());
            if let Some(program)=&program { self.dock.remember(program.clone()); }
            if let Some(i)=self.slots.iter().position(|s|s.as_ref().is_some_and(|s|s.window.id==window.id)) {
                let slot=self.slots[i].as_mut().unwrap();
                let title=slot.window.title!=window.title;
                let app=previous_app!=app_id;
                let state=slot.window.minimized!=window.minimized;
                let minimized=window.minimized;
                let moved=slot.screen!=screen;
                let resized=slot.scale!=scale;
                let fullscreen_changed=slot.fullscreen!=fullscreen;
                if minimized { slot.capture=None; }
                slot.window=window;
                slot.screen=screen;slot.scale=scale;
                slot.fullscreen=fullscreen;
                slot.configure.wake();
                let retained=if resized { slot.last_size.map(|size|native_frame(i,size,scale,PieceContent::Kept)) } else { None };
                if moved { self.tell(NestEvent::Screen(i,screen))?; }
                if fullscreen_changed { self.tell(NestEvent::Fullscreen(i,fullscreen))?; }
                if let Some(frame)=retained { self.tell(frame)?; }
                if title { self.tell(NestEvent::Title(i,self.slots[i].as_ref().unwrap().window.title.clone()))?; }
                if app { self.tell(NestEvent::App(i,app_id))?;if let Some(program)=&program {self.tell(program.event(i))?;} }
                if state {
                    self.tell(NestEvent::Minimized(i,minimized))?;
                    if !minimized { self.slots[i].as_mut().unwrap().retry.reset(); self.capture(i); }
                }
                continue;
            }
            let Some(i)=self.slots.iter().position(Option::is_none) else { break; };
            self.tell(NestEvent::Opened {slot:i,title:window.title.clone(),app:app_id,screen})?;
            if let Some(program)=&program { self.tell(program.event(i))?; }
            self.tell(NestEvent::Minimized(i,window.minimized))?;
            self.tell(NestEvent::Fullscreen(i,fullscreen))?;
            self.slots[i]=Some(Slot {window,capture:None,received:false,pixels:0,born:Instant::now(),next_frame:Instant::now(),
                configure:configure::Configure::default(),visible:self.visible.contains(&i),retry:Retry::default(),screen,scale,last_size:None,fullscreen});
            self.capture(i);
        }
        self.order()?;
        self.dock.retain(&self.programs.values().flatten().map(|p|p.key()).collect());
        self.focus()?;
        Ok(())
    }
    fn frames(&mut self) -> Result<()> {
        let mut sent=false;
        for i in 0..self.slots.len() {
            let others:u64=self.slots.iter().enumerate().filter(|(k,_)|*k!=i).filter_map(|(_,s)|s.as_ref())
                .map(Slot::pixels).sum();
            let Some(slot)=self.slots[i].as_mut() else { continue; };
            let Some(capture)=slot.capture.as_mut() else { continue; };
            let now=Instant::now();
            if !capture.pending() && ((!capture.ready() && (slot.received || slot.born.elapsed()<Duration::from_secs(5))) || now<slot.next_frame) { continue; }
            // Throttle starting copies, not finishing a copy already on the GPU.
            if !capture.pending() { slot.next_frame=now+Duration::from_secs_f64(1.0/30.0); }
            match capture.next(16_777_216u64.saturating_sub(others)) {
                Ok(Some(picture)) => {
                    slot.received=true;
                    slot.retry.reset();
                    slot.pixels=picture.size.0 as u64*picture.size.1 as u64;
                    slot.last_size=Some(picture.size);
                    let frame=native_frame(i,picture.size,slot.scale,
                        picture.shared.map(PieceContent::Windows).unwrap_or_else(||PieceContent::Pixels(picture.pixels)));
                    self.tell(frame)?;
                    sent=true;
                },
                Ok(None) if slot.received || slot.born.elapsed()<Duration::from_secs(5) => {},
                result => {
                    eprintln!("windows preview: capture stopped for {}: {}",slot.window.id,
                        result.err().map(|e|e.to_string()).unwrap_or_else(||"no capture frame received".into()));
                    slot.capture=None;
                    slot.retry.failed(Instant::now());
                },
            }
        }
        if sent {
            // A query on the same FIFO confirms this batch was consumed. The
            // generic FrameDone message can belong to an earlier repaint.
            let (reply,consumed)=mpsc::channel();
            self.send.send(ToRender::Query("screen.width",reply))?;
            self.consumed=Some(consumed);
        }
        Ok(())
    }
    fn visible(&mut self,slots:Vec<usize>) -> Result<()> {
        self.visible=slots.into_iter().filter(|i|*i<self.slots.len()).collect();
        // Retire every hidden image before starting the newly visible captures,
        // so an old page cannot consume the next page's capture budget.
        for i in 0..self.slots.len() {
            let Some(slot)=self.slots[i].as_mut() else { continue; };
            if slot.visible && !self.visible.contains(&i) {
                slot.visible=false;slot.capture=None;slot.pixels=0;slot.received=false;slot.retry.reset();slot.last_size=None;
                self.tell(NestEvent::Frame {slot:i,geometry:[0,0,0,0],pieces:Vec::new()})?;
            }
        }
        for i in 0..self.slots.len() {
            let Some(slot)=self.slots[i].as_mut() else { continue; };
            if !slot.visible && self.visible.contains(&i) { slot.visible=true;slot.retry.reset();self.capture(i); }
        }
        Ok(())
    }
    fn gpu(&mut self, shared:Option<pleamar::windows_texture::SharedDevice>) {
        // A diagnostic override for driver problems and comparisons using the
        // same executable, scene and capture workload.
        if shared.is_some() && std::env::var("PLEAMAR_WM_CAPTURE_CPU").as_deref()==Ok("1") {
            eprintln!("windows preview: capture transport = CPU readback (requested)");
            return;
        }
        match self.device.renderer(shared) {
            Ok(true) => {
                for slot in self.slots.iter_mut().flatten() { slot.capture=None;slot.retry.reset(); }
                for i in 0..self.slots.len() { self.capture(i); }
            },
            Ok(false) => {},
            Err(error) => eprintln!("windows preview: retaining current capture transport: {error}"),
        }
    }
    fn run(&mut self,commands:mpsc::Receiver<ToNest>) -> Result<()> {
        self.tell(self.dock.events())?;
        let mut topology=Instant::now();
        loop {
            pump();
            for (id,result) in self.metadata.poll() {
                let Some(slot)=self.slots.iter().position(|s|s.as_ref().is_some_and(|s|s.window.id==id)) else {continue;};
                match result {
                    Ok(program)=>{
                        self.tell(NestEvent::App(slot,program.key()))?;self.tell(program.event(slot))?;
                        self.dock.remember(program.clone());self.programs.insert(id,Some(program));
                    },
                    // Catalog races are diagnostics, not failures of a requested dock action.
                    Err(error)=>eprintln!("windows dock metadata: {id}: {error}"),
                }
            }
            for error in self.activations.errors() { eprintln!("windows dock: {error}"); }
            for _ in 0..256 {
                match commands.try_recv() {
                    Ok(ToNest::Quit)|Err(TryRecvError::Disconnected) => return Ok(()),
                    Err(TryRecvError::Empty) => break,
                    Ok(ToNest::FrameDone) => {},
                    Ok(ToNest::WindowsGpu(shared)) => self.gpu(shared),
                    Ok(ToNest::WindowsScreens(copies)) => {
                        self.outputs=Outputs::new(copies).unwrap_or_else(|error| { eprintln!("windows preview: {error}"); Outputs::default() });
                        CATALOG_DIRTY.set(true);
                    },
                    Ok(ToNest::Visible(slots)) => self.visible(slots)?,
                    Ok(ToNest::Launch(command)) => {
                        if let Err(error)=self.scope.launch(&mut self.launches,&command) { eprintln!("windows launch: {error}"); }
                    },
                    Ok(ToNest::Pin(key,yes)) => {
                        let result=self.scope.dock_actions().and_then(|_|self.dock.pin(&key,yes));
                        match result { Ok(())=>self.tell(self.dock.events())?,Err(error)=>eprintln!("windows dock: {error}") }
                    },
                    Ok(ToNest::OpenProgram {key,files}) => {
                        if let Err(error)=self.scope.dock_actions().and_then(|_|self.dock.open(&key,&files,&self.activations)) {
                            eprintln!("windows dock: {error}");
                        }
                    },
                    Ok(ToNest::Size(..)|ToNest::Shown {..}|ToNest::OnScreen(..)|ToNest::Gpu {..}|ToNest::Released(..)|ToNest::PointerOut|ToNest::HostFocus(..)) => {},
                    Ok(ToNest::Configure {slot,w,h}) => {
                        if !self.scope.actions && (w,h)==(0,0) { continue; }
                        let result=if !self.scope.actions { Err("scene resizing requires --window-actions; this scene is view-only".into()) }
                            else { self.slots.get_mut(slot).and_then(Option::as_mut)
                                .ok_or_else(||Box::<dyn std::error::Error>::from("window slot is no longer open"))
                                .and_then(|slot|slot.configure.ask(w,h)) };
                        if let Err(error)=result { eprintln!("windows preview: {error}"); }
                    },
                    Ok(message @ (ToNest::Focus(_)|ToNest::Close(_)|ToNest::Minimize(..)|ToNest::Send(..)|ToNest::Fullscreen(_))) => {
                        if let Err(error)=self.action(message) { eprintln!("windows preview: {error}"); }
                        CATALOG_DIRTY.set(true);
                    },
                    Ok(message) => {
                        let kind=match message {
                            ToNest::Pointer {..}|ToNest::Button {..}|ToNest::Wheel(..)|ToNest::Key {..} => "input",
                            _ => "window actions",
                        };
                        if self.warned.insert(kind) { eprintln!("windows preview: {kind} are unavailable; use the original native window"); }
                    },
                }
            }
            let mut refresh=CATALOG_DIRTY.replace(false);
            if topology.elapsed()>=Duration::from_secs(2) {
                // Display polling also detects DPI/work-area changes. Ordinary
                // windows are enumerated only on events or a topology change.
                refresh |= monitors()?!=self.screens;
                match self.dock.refresh() {
                    Ok(true)=>self.tell(self.dock.events())?,Ok(false)=>{},
                    Err(error)=>if self.warned.insert("dock configuration") {eprintln!("windows dock: {error}");},
                }
                topology=Instant::now();
            }
            if refresh { self.refresh()?; }
            for i in 0..self.slots.len() {
                if self.slots[i].as_ref().and_then(|s|s.retry_wait(Instant::now())).is_some_and(|d|d.is_zero()) { self.capture(i); }
                if !self.slots[i].as_ref().is_some_and(|s|s.configure.wait(Instant::now()).is_some_and(|d|d.is_zero())) { continue; }
                let others:u64=self.slots.iter().enumerate().filter(|(k,_)|*k!=i).filter_map(|(_,s)|s.as_ref())
                    .map(Slot::pixels).sum();
                if let Some(slot)=self.slots[i].as_mut() {
                    if let Err(error)=slot.configure.tick(&self.scope,self.created,&slot.window.id,&self.outputs,16_777_216u64.saturating_sub(others)) {
                        eprintln!("windows preview: {error}");
                    }
                }
            }
            if let Some(consumed)=&self.consumed {
                match consumed.try_recv() {
                    Ok(_) => self.consumed=None,
                    Err(TryRecvError::Disconnected) => return Err("render frame acknowledgement closed".into()),
                    Err(TryRecvError::Empty) => {},
                }
            }
            if self.consumed.is_none() { self.frames()?; }
            let now=Instant::now();
            self.device.idle(self.consumed.is_none() && self.slots.iter().flatten()
                .all(|slot|slot.capture.is_none() && (!slot.visible || slot.window.minimized)),now);
            if let Err(error)=self.launches.poll(now) { eprintln!("windows launch: {error}"); }
            let mut wait=(topology+Duration::from_secs(2)).saturating_duration_since(now);
            if let Some(retire)=self.device.wait(now) { wait=wait.min(retire); }
            if let Some(launch)=self.launches.wait(now) { wait=wait.min(launch); }
            for slot in self.slots.iter().flatten() {
                if let Some(resize)=slot.configure.wait(now) { wait=wait.min(resize); }
                if let Some(retry)=slot.retry_wait(now) { wait=wait.min(retry); }
            }
            if self.consumed.is_some() { wait=wait.min(Duration::from_millis(2)); }
            else {
                for slot in self.slots.iter().flatten() {
                    let Some(capture)=&slot.capture else { continue; };
                    if capture.pending() { wait=wait.min(Duration::from_millis(2)); }
                    else if capture.ready() { wait=wait.min(slot.next_frame.saturating_duration_since(now)); }
                }
            }
            self.waiter.wait(wait)?;
        }
    }
    fn action(&mut self,message:ToNest) -> Result<()> {
        if let ToNest::Send(i,screen)=message {
            let name=self.outputs.destination(screen,&self.scope)?;
            let slot=self.slots.get_mut(i).and_then(Option::as_mut).ok_or("window slot is no longer open")?;
            slot.configure.send(name.to_owned());
            return Ok(());
        }
        let (i,action)=match message {
            ToNest::Focus(i)=>(i,Action::Focus),
            ToNest::Close(i)=>(i,Action::Close),
            ToNest::Minimize(i,yes)=>(i,Action::Minimize(yes)),
            ToNest::Fullscreen(i)=>(i,Action::Fullscreen),
            _=>return Err("unsupported native window action".into()),
        };
        let slot=self.slots.get(i).and_then(Option::as_ref).ok_or("window slot is no longer open")?;
        act(&self.scope,self.created,&slot.window.id,action)
    }
}

fn native_frame(slot:usize,px:(u32,u32),scale:f64,content:PieceContent) -> NestEvent {
    let (w,h)=px;
    let size=((w as f64/scale).round().max(1.0) as u32,(h as f64/scale).round().max(1.0) as u32);
    NestEvent::Frame {slot,geometry:[0,0,size.0 as i32,size.1 as i32],pieces:vec![WindowPiece {
        id:slot as u64+1,at:(0,0),size,px,src:[0.0,0.0,w as f32,h as f32],content
    }]}
}

enum Action { Focus, Close, Minimize(bool), Fullscreen }
fn act(scope:&Scope,created:Option<u64>,id:&str,action:Action) -> Result<()> {
    if !scope.actions { return Err("window actions require --window-actions; this scene is view-only".into()); }
    let (hwnd,window)=target(id)?;
    if !scope.allows(&window,created) { return Err("window left the selected monitor or process scope".into()); }
    match action {
        Action::Fullscreen => { ipc::Endpoint::current()?.ask(&format!("fullscreen {id}"))?; },
        Action::Minimize(yes) => { state(id,yes)?; },
        Action::Close => {
            // An application may cancel closing or show an unsaved-work dialog.
            // Only its actual disappearance removes its slot.
            unsafe { PostMessageW(Some(hwnd),WM_CLOSE,WPARAM(0),LPARAM(0)) }?;
        },
        Action::Focus => {
            if window.minimized { state(id,false)?; }
            let (hwnd,window)=target(id)?;
            if !scope.allows(&window,created) { return Err("window left the selected scope while restoring".into()); }
            if !unsafe { SetForegroundWindow(hwnd) }.as_bool() {
                return Err("Windows denied foreground activation; select the scene and try again".into());
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn view_only_and_process_scopes_cannot_launch_programs() {
        let mut commands=launch::Launches::default();
        for scope in [Scope {monitor:"all".into(),process:None,actions:false},
            Scope {monitor:"all".into(),process:Some(123),actions:true}] {
            assert!(scope.launch(&mut commands,"exit 0").is_err());
            assert!(scope.dock_actions().is_err());
        }
        assert!(commands.wait(Instant::now()).is_none());
    }
    #[test]
    fn source_outputs_follow_scene_names_and_keep_cross_display_single_previews() {
        let all=Scope {monitor:"all".into(),process:None,actions:false};
        let single=Scope {monitor:"SOURCE".into(),process:None,actions:false};
        let outputs=Outputs::new(vec![(1,"RIGHT".into()),(0,"LEFT".into()),(1,"RIGHT".into())]).unwrap();
        assert_eq!(outputs.screen("RIGHT",&all),Some(1));
        assert_eq!(outputs.screen("LEFT",&all),Some(0));
        assert_eq!(outputs.screen("UNREPRESENTED",&all),None);
        assert_eq!(outputs.screen("SOURCE",&single),Some(0));
        let removed=Outputs::new(vec![(1,"RIGHT".into())]).unwrap();
        assert_eq!(removed.screen("RIGHT",&all),Some(1));
        assert_eq!(removed.screen("LEFT",&all),None);
        assert_eq!(removed.screen("SOURCE",&single),Some(1));
        assert_eq!(Outputs::default().screen("SOURCE",&single),None);
        assert!(Outputs::new(vec![(0,"SAME".into()),(1,"SAME".into())]).is_err());
        assert!(Outputs::new(vec![(4,"FIFTH".into())]).is_err());
    }
    #[test]
    fn send_resolves_scene_indices_and_refuses_view_only_or_ambiguous_destinations() {
        let mut scope=Scope {monitor:"all".into(),process:None,actions:true};
        let outputs=Outputs::new(vec![(2,"LEFT".into()),(0,"RIGHT".into())]).unwrap();
        assert_eq!(outputs.destination(0,&scope).unwrap(),"RIGHT");
        assert_eq!(outputs.destination(2,&scope).unwrap(),"LEFT");
        assert!(outputs.destination(1,&scope).is_err());
        scope.monitor="RIGHT".into();
        assert_eq!(outputs.destination(0,&scope).unwrap(),"RIGHT");
        assert!(outputs.destination(2,&scope).is_err());
        scope.actions=false;
        assert!(outputs.destination(0,&scope).unwrap_err().to_string().contains("--window-actions"));
        scope.actions=true;scope.monitor="all".into();
        let ambiguous=Outputs::new(vec![(0,"LEFT".into()),(0,"RIGHT".into())]).unwrap();
        assert!(ambiguous.destination(0,&scope).is_err());
        assert!(Outputs::default().destination(0,&scope).is_err());
    }
    #[test]
    fn each_native_window_uses_its_source_dpi_even_for_a_retained_frame() {
        for (pixels,scale,logical) in [((2560,1440),1.25,(2048,1152)),((1920,1080),1.0,(1920,1080)),
            ((1920,1080),1.5,(1280,720)),((2560,1440),2.0,(1280,720))] {
            let NestEvent::Frame {slot,geometry,pieces}=native_frame(3,pixels,scale,PieceContent::Kept) else { panic!() };
            assert_eq!(slot,3);
            assert_eq!(geometry,[0,0,logical.0,logical.1]);
            assert_eq!(pieces[0].size,(logical.0 as u32,logical.1 as u32));
            assert_eq!(pieces[0].px,pixels);
            assert_eq!(pieces[0].id,4);
            assert!(matches!(pieces[0].content,PieceContent::Kept));
        }
    }
    #[test]
    fn preview_actions_need_explicit_opt_in() {
        let scope=Scope {monitor:"unused".into(),process:None,actions:false};
        for action in [Action::Focus,Action::Close,Action::Minimize(true),Action::Minimize(false)] {
            assert!(act(&scope,None,"invalid",action).unwrap_err().to_string().contains("--window-actions"));
        }
        assert!(prepare(&["--window-actions".into()]).is_err());
    }
    #[test]
    fn actions_remain_scoped_after_catalog_changes() {
        let scope=Scope {monitor:"secondary".into(),process:Some(12),actions:true};
        let mut window=Window {id:"12:13:a:ff".into(),title:"fixture".into(),app:"fixture.exe".into(),
            class:"fixture".into(),process:12,monitor:scope.monitor.clone(),
            bounds:Bounds {x:0,y:0,width:400,height:300},minimized:true,maximized:false,resizable:true};
        assert!(scope.allows(&window,Some(255)));
        assert!(!scope.allows(&window,Some(256)));
        window.process=13;assert!(!scope.allows(&window,Some(255)));
        window.process=12;window.monitor="primary".into();assert!(!scope.allows(&window,Some(255)));
        let mut slot=Slot {window,capture:None,received:true,pixels:1_000_000,born:Instant::now(),next_frame:Instant::now(),
            configure:configure::Configure::default(),visible:false,retry:Retry::default(),screen:0,scale:1.0,last_size:None,fullscreen:false};
        assert_eq!(slot.pixels(),1_000_000,"suspended capture must retain its renderer memory budget");
        let now=Instant::now();
        slot.retry.failed(now);
        assert_eq!(slot.retry_wait(now),None,"a hidden page must not wake for capture retries");
        slot.visible=true;
        assert_eq!(slot.retry_wait(now),None,"a minimized source must not wake for capture retries");
        slot.window.minimized=false;
        assert_eq!(slot.retry_wait(now),Some(Duration::from_secs(1)));
        assert_eq!(slot.retry_wait(now+Duration::from_secs(1)),Some(Duration::ZERO));
    }
    #[test]
    fn capture_failures_back_off_until_pixels_arrive_or_the_view_reopens() {
        let mut retry=Retry::default();
        let mut now=Instant::now();
        for seconds in [1,2,4,8,16,30,30,30] {
            retry.failed(now);
            assert_eq!(retry.wait(now),Duration::from_secs(seconds));
            assert!(!retry.wait(now+Duration::from_millis(500)).is_zero());
            now+=Duration::from_secs(seconds);
            assert!(retry.wait(now).is_zero());
            // Merely reaching the deadline or creating a new capture must not
            // reset backoff: some drivers start successfully but send no pixels.
        }
        retry.reset();
        assert!(retry.wait(now).is_zero());
        retry.failed(now);
        assert_eq!(retry.wait(now),Duration::from_secs(1));
        retry.failures=u32::MAX;
        retry.failed(now);
        assert_eq!(retry.wait(now),Duration::from_secs(30));
    }
}
