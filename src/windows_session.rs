//! Event-driven per-monitor layouts. The recovery journal is flushed before
//! changing a window; ending a session restores free positions, not focus.
use super::*;
use std::{cell::{Cell, RefCell}, collections::{BTreeMap, BTreeSet}, io::Write, os::windows::{fs::OpenOptionsExt, ffi::OsStrExt, io::{AsRawHandle, FromRawHandle, OwnedHandle}},
    path::PathBuf, sync::atomic::Ordering};
use windows::Win32::{Storage::FileSystem::*, UI::Accessibility::*};

thread_local! {
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
    static MINIMIZE_EVENTS: RefCell<Vec<(isize, bool)>> = const { RefCell::new(Vec::new()) };
}
unsafe extern "system" fn changed(_: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, child: i32, _: u32, _: u32) {
    if event == EVENT_SYSTEM_MOVESIZESTART { DRAGGING.set(true); }
    if event == EVENT_SYSTEM_MOVESIZEEND { DRAGGING.set(false); }
    if event < EVENT_OBJECT_CREATE || (object == 0 && child == 0) { DIRTY.set(true); }
    if matches!(event, EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND) || (event == EVENT_OBJECT_DESTROY && object == 0 && child == 0) {
        MINIMIZE_EVENTS.with(|pending| {
            let mut pending = pending.borrow_mut();
            if pending.len() == 256 { pending.remove(0); }
            pending.push((hwnd.0 as isize, event == EVENT_SYSTEM_MINIMIZESTART));
        });
    }
}
struct Hooks(Vec<HWINEVENTHOOK>);
impl Hooks {
    fn new(process: u32) -> Result<Self> {
        let mut hooks = Self(Vec::new());
        for (first, last) in [(EVENT_OBJECT_CREATE, EVENT_OBJECT_NAMECHANGE),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND), (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND)] {
            let hook = unsafe { SetWinEventHook(first, last, None, Some(changed), process, 0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS) };
            if hook.is_invalid() { return Err("could not subscribe to native window events".into()); }
            hooks.0.push(hook);
        }
        Ok(hooks)
    }
}
impl Drop for Hooks { fn drop(&mut self) { for hook in self.0.drain(..) { let _ = unsafe { UnhookWinEvent(hook) }; } } }

#[derive(Clone, Serialize, Deserialize)]
struct Original {
    id: String, monitor: String, bounds: Bounds, normal: [i32; 4],
    // Only an unfinished rule transaction may restore across displays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_monitor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fullscreen: Option<fullscreen::Saved>,
}
#[derive(Serialize, Deserialize)]
struct Recovery { version: u32, windows: Vec<Original> }

struct Journal { path: PathBuf, _lock: std::fs::File }
impl Journal {
    fn open(path: PathBuf) -> Result<(Self, BTreeMap<String, Original>)> {
        let parent = path.parent().ok_or("WM state file needs a parent directory")?;
        std::fs::create_dir_all(parent)?;
        let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .share_mode(0).open(path.with_extension("lock"))?;
        let mut windows = BTreeMap::new();
        if path.exists() {
            let file = std::fs::File::open(&path)?;
            if file.metadata()?.len() > 65536 { return Err("WM recovery journal is too large".into()); }
            let saved: Recovery = serde_json::from_reader(file)?;
            if !matches!(saved.version,1|2|3) || saved.windows.len() > 64 { return Err("unknown WM recovery journal".into()); }
            for item in saved.windows {
                if item.bounds.width <= 0 || item.bounds.height <= 0 || windows.insert(item.id.clone(), item).is_some() {
                    return Err("invalid WM recovery journal".into());
                }
            }
        }
        Ok((Self { path, _lock:lock }, windows))
    }
    fn save(&self, windows: &BTreeMap<String, Original>) -> Result<()> {
        if windows.len() > 64 { return Err("WM recovery supports at most 64 managed windows".into()); }
        let temporary = self.path.with_extension(format!("{}.pending", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        let result = (|| -> Result<()> {
            let bytes = serde_json::to_vec(&Recovery { version:3, windows:windows.values().cloned().collect() })?;
            if bytes.len() > 65536 { return Err("WM recovery journal exceeds its limit".into()); }
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            let from: Vec<u16> = temporary.as_os_str().encode_wide().chain([0]).collect();
            let to: Vec<u16> = self.path.as_os_str().encode_wide().chain([0]).collect();
            unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) }?;
            Ok(())
        })();
        if result.is_err() { let _ = std::fs::remove_file(&temporary); }
        result
    }
}

struct Mode { tiled: bool, layout: Layout, order: Vec<String>, navigation: Vec<String>, error: Option<String> }
impl Default for Mode { fn default() -> Self { Self { tiled:false, layout:Layout::Left, order:Vec::new(), navigation:Vec::new(), error:None } } }

fn neighbor(order:&mut Vec<String>, live:&[String], current:&str, forward:bool) -> Result<String> {
    if live.len()>256 { return Err("window navigation supports at most 256 windows per monitor".into()); }
    order.retain(|id|live.contains(id));
    // Activation changes EnumWindows' Z order. Preserve the cycle so repeated
    // next commands reach every window instead of bouncing between two.
    let mut added:Vec<_> = live.iter().filter(|id|!order.contains(id)).cloned().collect();
    added.sort();added.dedup();order.extend(added);
    let at=order.iter().position(|id|id==current).ok_or("active window left the navigation scope")?;
    let next=if forward {(at+1)%order.len()} else {(at+order.len()-1)%order.len()};
    Ok(order[next].clone())
}

#[derive(Default)]
struct Minimized(Vec<String>);
impl Minimized {
    fn remember(&mut self, id: String) {
        self.0.retain(|old| old != &id);
        if self.0.len() == 64 { self.0.remove(0); }
        self.0.push(id);
    }
    fn destroyed(&mut self, hwnd: isize) {
        self.0.retain(|id| id.split(':').nth(2).and_then(|h| usize::from_str_radix(h, 16).ok()) != Some(hwnd as usize));
    }
}

fn placement(hwnd: HWND) -> Result<WINDOWPLACEMENT> {
    let mut p = WINDOWPLACEMENT { length:size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    unsafe { GetWindowPlacement(hwnd, &mut p) }?;
    Ok(p)
}
fn rect_array(rect: RECT) -> [i32; 4] { [rect.left, rect.top, rect.right, rect.bottom] }
fn original(id: &str) -> Result<Original> {
    let (hwnd, w) = target(id)?;
    Ok(Original { id:id.into(), monitor:w.monitor, bounds:w.bounds, normal:rect_array(placement(hwnd)?.rcNormalPosition), pending_monitor:None, fullscreen:None })
}

fn restore(old: &Original, screens: &[Monitor]) -> Result<bool> {
    let handle = old.id.split(':').nth(2).and_then(|s| usize::from_str_radix(s, 16).ok())
        .ok_or("invalid recovery window id")?;
    let hwnd = HWND(handle as _);
    if !Identity::read(hwnd).is_some_and(|i| i.token() == old.id) { return Ok(true); }
    if let Some(saved)=&old.fullscreen {
        saved.restore(&old.id)?;
        if !saved.prior_layout { return Ok(true); }
    }
    let Some(screen) = screens.iter().find(|m| m.name == old.monitor) else { return Ok(false); };
    let Ok((hwnd, current)) = target(&old.id) else { return Ok(false); };
    // A move to another still-connected monitor is the user's new free position.
    if current.monitor != old.monitor && old.pending_monitor.as_deref() != Some(&current.monitor) { return Ok(true); }
    if !screen.bounds.contains(&old.bounds) { return Ok(false); }
    if !current.minimized && !current.maximized { place(&old.id, &old.bounds)?; return Ok(true); }
    let mut p = placement(hwnd)?;
    p.rcNormalPosition = RECT { left:old.normal[0], top:old.normal[1], right:old.normal[2], bottom:old.normal[3] };
    p.flags |= WPF_ASYNCWINDOWPLACEMENT;
    p.showCmd = if current.minimized { SW_SHOWMINNOACTIVE.0 as u32 } else { SW_SHOWNA.0 as u32 };
    unsafe { SetWindowPlacement(hwnd, &p) }?;
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        pump();
        let (_, now) = target(&old.id)?;
        if rect_array(placement(hwnd)?.rcNormalPosition) == old.normal
            && now.minimized == current.minimized && now.maximized == current.maximized { return Ok(true); }
        if Instant::now() >= until { return Err("window did not confirm its restored free position".into()); }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Manager {
    modes: BTreeMap<String, Mode>, originals:BTreeMap<String, Original>, journal:Journal,
    process:Option<u32>, owner:Option<u32>, creation:Option<u64>, all:bool, scans:u64, changes:u64,
    rules:rules::Rules,
    minimized:Minimized,
    fullscreen:BTreeSet<String>,
}
impl Manager {
    fn owns(&self, window: &Window) -> bool {
        self.process.is_none_or(|pid| window.process == pid)
            && self.creation.is_none_or(|created| window.id.ends_with(&format!(":{created:x}")))
    }
    fn open(options: &Options) -> Result<Self> {
        let screens = monitors()?;
        let modes: BTreeMap<_, _> = screens.iter().filter(|m| options.all || options.monitors.contains(&m.name))
            .map(|m| (m.name.clone(), Mode::default())).collect();
        if !options.all && modes.len() != options.monitors.len() { return Err("a requested monitor is not connected".into()); }
        let rules = rules::Rules::read(options.rules.clone(),options.explicit_rules)?;
        let (journal, originals) = Journal::open(options.state.clone())?;
        let mut manager = Self { modes, originals, journal, process:options.process, owner:options.owner, creation:None, all:options.all, scans:0, changes:0, rules, minimized:Minimized::default(), fullscreen:BTreeSet::new() };
        if let Some(pid) = options.process {
            // Store a creation stamp as well: a recycled PID must not widen a fixture's scope.
            manager.creation = windows()?.iter().find(|w| w.process == pid)
                .and_then(|w| w.id.rsplit(':').next()).and_then(|s| u64::from_str_radix(s, 16).ok());
            if manager.creation.is_none() { return Err("scoped process has no eligible window".into()); }
        }
        manager.release(None)?;
        Ok(manager)
    }
    fn release(&mut self, screen:Option<&str>) -> Result<()> {
        let screens = monitors()?;
        let before = self.originals.len();
        let mut errors = Vec::new();
        let ids: Vec<_> = self.originals.values().filter(|w| screen.is_none_or(|s| w.monitor == s)).map(|w| w.id.clone()).collect();
        for id in ids {
            if self.fullscreen.contains(&id) { continue; }
            let old = &self.originals[&id];
            if !self.modes.contains_key(&old.monitor) { continue; }
            if old.pending_monitor.as_ref().is_some_and(|m|!self.modes.contains_key(m)) { continue; }
            if let Ok((_, w)) = target(&id) { if !self.owns(&w) { continue; } }
            match restore(old, &screens) {
                Ok(true) => { self.originals.remove(&id); }
                Ok(false) => {}
                Err(e) => errors.push(e.to_string()),
            }
        }
        if before != self.originals.len() || !self.journal.path.exists() { self.journal.save(&self.originals)?; }
        if !errors.is_empty() { return Err(errors.join("; ").into()); }
        Ok(())
    }
    fn status(&self) -> Value {
        json!({"running":true,"automatic_layouts":true,"window_overview":true,"application_dock":true,"rain":false,"snow":false,"ride":false,"dock":false,
            "pools":false,"process":self.process,"owner":self.owner,"saved_windows":self.originals.len(),
            "pending_recovery":self.originals.values().filter(|w|!self.fullscreen.contains(&w.id) && self.modes.get(&w.monitor).is_none_or(|m|!m.tiled)).count(),
            "catalog_scans":self.scans,"geometry_changes":self.changes,
            "minimize_shortcuts":true,"last_minimized":self.minimized.0.last(),
            "navigation_shortcuts":true,"close_shortcut":true,"fullscreen_shortcut":true,"fullscreen_windows":self.fullscreen,
            "rules_file":self.rules.path,"window_rules":self.rules.entries.len(),
            "ruled_windows":self.rules.applied.len(),"rule_errors":self.rules.errors,
            "rule_limit_reached":self.rules.applied.len()+self.rules.errors.len()>=256,
            "monitors":self.modes.iter().map(|(name,m)| json!({"name":name,"tiled":m.tiled,
                "layout":format!("{:?}",m.layout).to_lowercase(),"windows":m.order.len(),"error":m.error})).collect::<Vec<_>>()})
    }
    fn set_mode(&mut self, name:&str, tiled:bool, layout:Option<Layout>) -> Result<Value> {
        let mode = self.modes.get_mut(name).ok_or("monitor is outside this WM session")?;
        mode.tiled = tiled;
        mode.error = None;
        if let Some(layout) = layout { mode.layout = layout; }
        if tiled { self.reconcile()?; }
        else { mode.order.clear(); self.release(Some(name))?; }
        if let Some(e) = &self.modes[name].error { return Err(e.clone().into()); }
        Ok(self.status())
    }
    fn reconcile(&mut self) -> Result<()> {
        let screens = monitors()?;
        if self.all { for screen in &screens { self.modes.entry(screen.name.clone()).or_default(); } }
        if self.rules.entries.is_empty() && self.originals.is_empty() && self.modes.values().all(|m| !m.tiled) { return Ok(()); }
        self.scans += 1;
        let mut live: Vec<_> = windows()?.into_iter().filter(|w| self.owns(w)).collect();
        // Forget only destroyed identities or deliberate moves; hidden/minimized
        // windows retain their recovery position until they return or we exit.
        let previous = self.originals.len();
        self.originals.retain(|id, old| {
            let handle = id.split(':').nth(2).and_then(|s| usize::from_str_radix(s,16).ok()).unwrap_or(0);
            Identity::read(HWND(handle as _)).is_some_and(|i| i.token() == *id)
                && (old.fullscreen.is_some() || !live.iter().any(|w| w.id == *id && w.monitor != old.monitor
                    && old.pending_monitor.as_deref() != Some(&w.monitor) && screens.iter().any(|m| m.name == old.monitor)))
        });
        self.fullscreen.retain(|id|self.originals.contains_key(id));
        // Tiling and initial window rules must not resize an active fullscreen.
        live.retain(|w|!self.fullscreen.contains(&w.id));
        if self.originals.len() != previous { self.journal.save(&self.originals)?; }
        self.apply_rules(&mut live,&screens)?;
        let names:Vec<_> = self.modes.keys().cloned().collect();
        for name in names {
            if !self.modes[&name].tiled {
                if self.originals.values().any(|w| w.monitor == name) { self.release(Some(&name))?; }
                continue;
            }
            let Some(screen) = screens.iter().find(|m| m.name == name) else { continue; };
            let eligible:Vec<_> = live.iter().filter(|w| w.monitor == name && !w.minimized && !w.maximized && w.resizable
                && !self.rules.floats(&w.id) && !self.rules.errors.contains_key(&w.id)).collect();
            let mode = self.modes.get_mut(&name).unwrap();
            mode.order.retain(|id| eligible.iter().any(|w| w.id == *id));
            let mut added:Vec<_> = eligible.iter().filter(|w| !mode.order.contains(&w.id)).map(|w|w.id.clone()).collect();
            added.sort();
            mode.order.extend(added);
            if mode.order.is_empty() { continue; }
            let order = mode.order.clone();
            let layout = mode.layout;
            let changed = (|| -> Result<()> {
                let boxes = layout::arrange((&screen.work).into(), order.len(), layout, (8.0*screen.scale).round() as i32)?;
                let added = order.iter().filter(|id| !self.originals.contains_key(*id)).count();
                if self.originals.len() + added > 64 { return Err("WM recovery supports at most 64 managed windows across monitors".into()); }
                let mut saved = false;
                for id in &order {
                    if !self.originals.contains_key(id) {
                        let value = original(id)?;
                        if !screen.bounds.contains(&value.bounds) { return Err("bring new windows wholly onto their monitor before tiling".into()); }
                        self.originals.insert(id.clone(), value); saved = true;
                    }
                }
                if saved { self.journal.save(&self.originals)?; }
                for (id, rect) in order.iter().zip(boxes) {
                    let bounds:Bounds = rect.into();
                    if target(id)?.1.bounds != bounds { place(id, &bounds)?; self.changes += 1; }
                }
                Ok(())
            })();
            if let Err(error) = changed {
                self.modes.get_mut(&name).unwrap().tiled = false;
                let recovery = self.release(Some(&name)).err().map(|e|format!("; restore: {e}")).unwrap_or_default();
                self.modes.get_mut(&name).unwrap().error = Some(format!("{error}{recovery}"));
            }
        }
        Ok(())
    }
    fn apply_rules(&mut self, live:&mut [Window], screens:&[Monitor]) -> Result<()> {
        if self.rules.entries.is_empty() { return Ok(()); }
        self.rules.forget_closed();
        for window in live {
            if !self.modes.contains_key(&window.monitor) || self.rules.applied.contains_key(&window.id)
                || self.rules.errors.contains_key(&window.id) || self.rules.applied.len()+self.rules.errors.len()>=256 { continue; }
            let rule = self.rules.for_window(window);
            if rule == crate::window_rules::ForWindow::default() { continue; }
            // Wait for a normal state instead of restoring or unmaximizing an application.
            if (rule.size.is_some() || rule.monitor.is_some() || self.originals.contains_key(&window.id))
                && (window.minimized || window.maximized) { continue; }
            let result = (|| -> Result<()> {
                let destination = rules::destination(&rule,window,screens)?;
                if !self.modes.contains_key(&destination.name) { return Err("rule destination is outside this WM session".into()); }
                let geometry = rule.size.is_some() || rule.monitor.is_some();
                if geometry || self.originals.contains_key(&window.id) {
                    normal(window)?;
                    if !self.originals.contains_key(&window.id) {
                        if self.originals.len()>=64 { return Err("WM recovery supports at most 64 windows".into()); }
                        let mut old = original(&window.id)?;
                        let source = screens.iter().find(|m|m.name==old.monitor).ok_or("source display disconnected")?;
                        if !source.bounds.contains(&old.bounds) { return Err("bring ruled windows wholly onto their monitor first".into()); }
                        old.pending_monitor = Some(destination.name.clone());
                        self.originals.insert(window.id.clone(),old);
                    } else {
                        self.originals.get_mut(&window.id).unwrap().pending_monitor = Some(destination.name.clone());
                    }
                    self.journal.save(&self.originals)?;
                    // A late title may match after tiling: derive the rule from its
                    // saved free position, never from the temporary layout rectangle.
                    let old = &self.originals[&window.id];
                    if !restore(old,screens)? { return Err("rule is waiting for its original window position".into()); }
                    *window = target(&window.id)?.1;
                    if geometry {
                        let bounds = rules::bounds(&rule,window,destination)?;
                        if window.bounds != bounds { place(&window.id,&bounds)?; self.changes+=1; }
                        *window = target(&window.id)?.1;
                    }
                    // An initial rule defines the new free position. Future tiling
                    // saves that position; shutdown does not undo the user's rule.
                    let old = self.originals.remove(&window.id).unwrap();
                    if let Err(error) = self.journal.save(&self.originals) {
                        self.originals.insert(window.id.clone(),old);
                        return Err(error);
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => { self.rules.applied.insert(window.id.clone(),rule.float); }
                Err(error) => {
                    let mut error = error.to_string();
                    if let Some(old) = self.originals.get(&window.id) {
                        match restore(old,screens) {
                            Ok(true) => { self.originals.remove(&window.id); self.journal.save(&self.originals)?; }
                            Ok(false) => error.push_str("; rollback pending"),
                            Err(e) => error.push_str(&format!("; rollback: {e}")),
                        }
                    }
                    if let Ok((_,current)) = target(&window.id) { *window = current; }
                    eprintln!("window rule · {}: {error}",window.id);
                    self.rules.errors.insert(window.id.clone(),error);
                }
            }
        }
        Ok(())
    }
    fn minimized_events(&mut self) {
        let events = MINIMIZE_EVENTS.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        for (hwnd, minimized) in events {
            if !minimized { self.minimized.destroyed(hwnd); continue; }
            if let Some(window) = inspect(HWND(hwnd as _)) {
                if self.owns(&window) && self.modes.contains_key(&window.monitor) && window.minimized {
                    self.minimized.remember(window.id);
                }
            }
        }
    }
    fn leave_fullscreen(&mut self,id:&str) -> Result<()> {
        let old=self.originals.get(id).cloned().ok_or("fullscreen recovery is missing")?;
        let saved=old.fullscreen.as_ref().ok_or("fullscreen placement is missing")?;
        saved.restore(id)?;
        if saved.prior_layout { self.originals.get_mut(id).unwrap().fullscreen=None; }
        else { self.originals.remove(id); }
        if let Err(error)=self.journal.save(&self.originals) {
            self.originals.insert(id.into(),old);
            return Err(error);
        }
        self.fullscreen.remove(id);
        Ok(())
    }
    fn toggle_fullscreen(&mut self,id:&str) -> Result<Value> {
        let (_,window)=target(id)?;
        if !self.owns(&window) || !self.modes.contains_key(&window.monitor) {
            return Err("fullscreen target is outside this WM session".into());
        }
        if self.fullscreen.contains(id) { self.leave_fullscreen(id)?; }
        else {
            let screen=monitors()?.into_iter().find(|m|m.name==window.monitor).ok_or("fullscreen monitor disconnected")?;
            let previous=self.originals.get(id).cloned();
            let saved=fullscreen::Saved::read(id,previous.is_some())?;
            let mut old=match &previous {Some(old)=>old.clone(),None=>original(id)?};
            old.fullscreen=Some(saved.clone());
            self.originals.insert(id.into(),old);
            if let Err(error)=self.journal.save(&self.originals) {
                match previous {Some(old)=>{self.originals.insert(id.into(),old);},None=>{self.originals.remove(id);}}
                return Err(error);
            }
            self.fullscreen.insert(id.into());
            if let Err(error)=saved.enter(id,&screen) {
                let rollback=self.leave_fullscreen(id);
                return Err(format!("{error}; fullscreen rollback: {}",rollback.err().map_or("restored".into(),|e|e.to_string())).into());
            }
        }
        DIRTY.set(true);
        Ok(self.status())
    }
    fn active(&self, include_owned:bool) -> Result<(HWND,Window)> {
        let hwnd = unsafe { GetForegroundWindow() };
        let window = inspect_kind(hwnd,include_owned).ok_or("the active window is not an eligible application window")?;
        if !self.owns(&window) || !self.modes.contains_key(&window.monitor) {
            return Err("the active window is outside this WM session".into());
        }
        if window.minimized || !unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd) }.as_bool() {
            return Err("the active window is minimized or blocked by a dialog".into());
        }
        if !monitors()?.iter().any(|m|m.name==window.monitor) { return Err("the active window's monitor disconnected".into()); }
        Ok((hwnd,window))
    }
    fn focus_relative(&mut self, forward:bool) -> Result<Value> {
        let (source,current)=self.active(false)?;
        let live:Vec<_>=windows()?.into_iter().filter(|w|self.owns(w) && w.monitor==current.monitor && !w.minimized)
            .filter(|w|target(&w.id).is_ok_and(|(hwnd,_)|unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd) }.as_bool()))
            .map(|w|w.id).collect();
        let mode=self.modes.get_mut(&current.monitor).unwrap();
        if mode.tiled { mode.navigation=mode.order.clone(); }
        let id=neighbor(&mut mode.navigation,&live,&current.id,forward)?;
        let (hwnd,window)=target(&id)?;
        if !self.owns(&window) || window.monitor!=current.monitor || window.minimized
            || !unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd) }.as_bool()
            || unsafe { GetForegroundWindow() }!=source || target(&current.id).is_err() {
            return Err("window or foreground changed during navigation".into());
        }
        if !unsafe { SetForegroundWindow(hwnd) }.as_bool() {
            return Err("Windows denied foreground activation".into());
        }
        let until=Instant::now()+Duration::from_millis(500);
        loop {
            pump();
            if unsafe { GetForegroundWindow() }==hwnd && target(&id).is_ok() { break; }
            if Instant::now()>=until { return Err("window did not confirm foreground activation".into()); }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut status=self.status();status["focused_window"]=json!(id);Ok(status)
    }
    fn close_focused(&self) -> Result<Value> {
        let (hwnd,window)=self.active(true)?;
        if target_kind(&window.id,true).is_err() || unsafe { GetForegroundWindow() }!=hwnd {
            return Err("window or foreground changed before closing".into());
        }
        // Ask the application to close: it retains its unsaved-work prompt and
        // can decline. A posted request is not proof that its window disappeared.
        unsafe { PostMessageW(Some(hwnd),WM_CLOSE,WPARAM(0),LPARAM(0)) }?;
        let mut status=self.status();status["close_requested"]=json!(window.id);Ok(status)
    }
    fn minimize_focused(&mut self) -> Result<Value> {
        let (_,window)=self.active(false)?;
        window_state(&window.id, true, true)?;
        self.minimized.remember(window.id);
        self.minimized_events();
        // Minimized windows leave the layout but retain their recovery entry.
        // Reflow before returning so the command's status matches the desktop.
        self.reconcile()?;
        Ok(self.status())
    }
    fn restore_last(&mut self) -> Result<Value> {
        while let Some(id) = self.minimized.0.last().cloned() {
            let window = target(&id).ok().map(|(_, window)| window);
            if window.as_ref().is_none_or(|w| !w.minimized || !self.owns(w) || !self.modes.contains_key(&w.monitor)) {
                self.minimized.0.pop(); continue;
            }
            // Keep a failed restoration retryable; remove it only after native readback.
            window_state(&id, false, true)?;
            self.minimized.0.pop();
            self.reconcile()?;
            return Ok(self.status());
        }
        Err("no recently minimized window remains in this WM session".into())
    }
    fn command(&mut self, line:&str) -> Result<(Value,bool)> {
        self.minimized_events();
        let words:Vec<_> = line.split_whitespace().collect();
        let value = match words.as_slice() {
            ["status"] | ["capabilities"] => self.status(),
            ["quit"] => {
                for mode in self.modes.values_mut() { mode.tiled=false; mode.order.clear(); }
                self.fullscreen.clear();
                self.release(None)?;
                return Ok((json!({"stopped":true,"pending_recovery":self.originals.len()}),true));
            }
            ["toggle", name] => {
                let tiled = !self.modes.get(*name).ok_or("unknown WM monitor")?.tiled;
                self.set_mode(name,tiled,None)?
            }
            ["layout", name, kind] => self.set_mode(name,true,Some(kind.parse()?))?,
            ["free", name] => self.set_mode(name,false,None)?,
            ["emit", "minimize"] => self.minimize_focused()?,
            ["emit", "restore_last"] => self.restore_last()?,
            ["emit", "focus_next"] => self.focus_relative(true)?,
            ["emit", "focus_previous"] => self.focus_relative(false)?,
            ["emit", "close"] => self.close_focused()?,
            ["fullscreen", id] => self.toggle_fullscreen(id)?,
            ["emit", "fullscreen"] => {
                let (_,window)=self.active(false)?;
                self.toggle_fullscreen(&window.id)?
            },
            ["emit", "toggle_free"] => {
                let mut point = POINT::default();
                unsafe { GetCursorPos(&mut point) }?;
                let m = monitor(unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) }).ok_or("pointer has no active monitor")?;
                let tiled = !self.modes.get(&m.name).ok_or("pointer is outside this WM session")?.tiled;
                self.set_mode(&m.name,tiled,None)?
            }
            _ => return Err(format!("unsupported Windows WM command: {line}").into()),
        };
        Ok((value,false))
    }
}

// Hold the process object, not its PID: a recycled PID cannot keep a session alive.
struct Owner(OwnedHandle);
impl Owner {
    fn open(pid:u32) -> Result<Self> {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }?;
        let owner = Self(unsafe { OwnedHandle::from_raw_handle(handle.0) });
        if owner.exited()? { return Err("WM owner has already exited".into()); }
        Ok(owner)
    }
    fn handle(&self) -> HANDLE { HANDLE(self.0.as_raw_handle()) }
    fn exited(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(std::io::Error::last_os_error().into()),
        }
    }
}

struct Options { monitors:BTreeSet<String>, all:bool, process:Option<u32>, owner:Option<u32>, state:PathBuf, namespace:String, seconds:Option<u64>,
    rules:PathBuf, explicit_rules:bool }
impl Options {
    fn parse(args:&[String]) -> Result<Self> {
        let config = pleamar::config_dir().ok_or("Windows configuration directory unavailable")?;
        let rule_override = std::env::var_os("PLEAMAR_WM_CONFIG").filter(|s|!s.is_empty());
        let mut options = Self { monitors:BTreeSet::new(), all:false, process:None, owner:None,
            state:config.join("wm/windows-session.json"),
            rules:rule_override.clone().map(PathBuf::from).unwrap_or_else(||config.join("session.conf")), explicit_rules:rule_override.is_some(),
            namespace:std::env::var("PLEAMAR_WM_NAMESPACE").unwrap_or_default(), seconds:None };
        let mut explicit_state = false;
        let mut it = args.iter();
        while let Some(key) = it.next() {
            let value = it.next().ok_or("session options need values")?;
            match key.as_str() {
                "--monitor" => if value == "all" { options.all = true; } else { options.monitors.insert(value.clone()); },
                "--process" => options.process = Some(value.parse()?),
                "--owner" => options.owner = Some(value.parse()?),
                "--state" => { options.state = std::path::absolute(value)?; explicit_state = true; },
                "--rules" => { options.rules = std::path::absolute(value)?; options.explicit_rules = true; },
                "--namespace" => options.namespace = value.clone(),
                "--seconds" => { let seconds = value.parse()?; if !(1..=86400).contains(&seconds) { return Err("invalid session duration".into()); } options.seconds=Some(seconds); },
                _ => return Err(format!("unknown session option: {key}").into()),
            }
        }
        if !options.all && options.monitors.is_empty() { return Err("session requires --monitor NAME or --monitor all".into()); }
        if options.process == Some(0) { return Err("process scope must be a nonzero PID".into()); }
        if options.owner == Some(0) { return Err("owner must be a nonzero PID".into()); }
        if !explicit_state && !options.namespace.is_empty() {
            // Validate before allowing the namespace to become part of a filename.
            ipc::Endpoint::new(&options.namespace)?;
            options.state.set_file_name(format!("windows-session-{}.json", options.namespace));
        }
        Ok(options)
    }
}

pub(super) fn run(args:&[String]) -> Result<Value> {
    let options = Options::parse(args)?;
    let owner = options.owner.map(Owner::open).transpose()?;
    let server = ipc::Server::start(ipc::Endpoint::new(&options.namespace)?)?;
    let mut manager = Manager::open(&options)?;
    let _hooks = Hooks::new(options.process.unwrap_or(0))?;
    let started = Instant::now();
    let mut dirty_since = (!manager.rules.entries.is_empty()).then(Instant::now);
    let mut last_topology = Instant::now();
    let mut topology = serde_json::to_string(&monitors()?)?;
    let mut handles = vec![server.wake.handle()];
    if let Some(owner) = &owner { handles.push(owner.handle()); }
    let result = (|| -> Result<()> {
        loop {
            pump();
            manager.minimized_events();
            if owner.as_ref().map(Owner::exited).transpose()?.unwrap_or(false) { break; }
            if !server.running() { return Err("WM command listener stopped unexpectedly".into()); }
            server.wake.reset();
            let mut quit = false;
            while let Ok(request) = server.requests.try_recv() {
                if request.expired.load(Ordering::Acquire) { continue; }
                let response = manager.command(&request.command).map(|(value,stop)| { quit |= stop; value });
                request.finish(response);
                if quit { break; }
            }
            if quit || options.seconds.is_some_and(|s| started.elapsed() >= Duration::from_secs(s)) { break; }
            if DIRTY.replace(false) { dirty_since.get_or_insert_with(Instant::now); }
            if last_topology.elapsed() >= Duration::from_secs(2) {
                let current = serde_json::to_string(&monitors()?)?;
                if current != topology { dirty_since.get_or_insert_with(Instant::now); topology=current; }
                last_topology=Instant::now();
            }
            if !DRAGGING.get() && dirty_since.is_some_and(|t| t.elapsed() >= Duration::from_millis(60)) {
                dirty_since=None;
                manager.reconcile()?;
            }
            let wait = if dirty_since.is_some() { 60 } else { 500 };
            let result = unsafe { MsgWaitForMultipleObjectsEx(Some(&handles),wait,QS_ALLINPUT,MWMO_INPUTAVAILABLE) };
            if result == WAIT_FAILED { return Err(std::io::Error::last_os_error().into()); }
        }
        Ok(())
    })();
    manager.fullscreen.clear();
    let restore = manager.release(None);
    result?;
    restore?;
    Ok(json!({"stopped":true,"pending_recovery":manager.originals.len()}))
}

#[cfg(test)]
#[path = "windows_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
#[path = "windows_fullscreen_tests.rs"]
mod fullscreen_tests;

#[cfg(test)]
#[path = "windows_navigation_tests.rs"]
mod navigation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_reaches_each_window_despite_z_order_changes_and_retires_old_identities() {
        let mut order=Vec::new();
        let live=|names:&[&str]|names.iter().map(|s|s.to_string()).collect::<Vec<_>>();
        assert_eq!(neighbor(&mut order,&live(&["a","b","c"]),"a",true).unwrap(),"b");
        assert_eq!(neighbor(&mut order,&live(&["b","a","c"]),"b",true).unwrap(),"c");
        assert_eq!(neighbor(&mut order,&live(&["c","b","a"]),"c",true).unwrap(),"a");
        assert_eq!(neighbor(&mut order,&live(&["a","c","b"]),"a",false).unwrap(),"c");
        assert_eq!(neighbor(&mut order,&live(&["a","new"]),"a",true).unwrap(),"new");
        assert_eq!(order,live(&["a","new"]));
        assert_eq!(neighbor(&mut order,&live(&["a"]),"a",false).unwrap(),"a");
        assert!(neighbor(&mut order,&[],"a",true).is_err());
        assert!(order.is_empty());
        assert!(neighbor(&mut order,&(0..257).map(|n|n.to_string()).collect::<Vec<_>>(),"0",true).is_err());
    }

    #[test]
    fn minimized_history_is_ordered_bounded_and_forgets_reused_handles() {
        let mut history = Minimized::default();
        for handle in 1..=70 { history.remember(format!("1:2:{handle:x}:3")); }
        assert_eq!(history.0.len(), 64);
        assert_eq!(history.0.first().unwrap(), "1:2:7:3");
        history.remember("1:2:7:3".into());
        assert_eq!(history.0.len(), 64);
        assert_eq!(history.0.last().unwrap(), "1:2:7:3");
        history.destroyed(7);
        assert_eq!(history.0.len(), 63);
        assert_eq!(history.0.last().unwrap(), "1:2:46:3");
        history.remember("8:9:7:10".into());
        assert_eq!(history.0.last().unwrap(), "8:9:7:10");
        history.destroyed(7);
        assert!(!history.0.iter().any(|id| id.split(':').nth(2) == Some("7")));
    }

    #[test]
    fn session_names_separate_default_journals_and_reject_path_injection() {
        let parse = |args:&[&str]| Options::parse(&args.iter().map(|v|v.to_string()).collect::<Vec<_>>());
        let one = parse(&["--monitor","all","--namespace","one"]).unwrap();
        let two = parse(&["--monitor","all","--namespace","two"]).unwrap();
        assert_ne!(one.state,two.state);
        assert_eq!(one.state.file_name().unwrap(),"windows-session-one.json");
        assert!(parse(&["--monitor","all","--namespace","../escape"]).is_err());
        assert!(parse(&["--monitor","all","--owner","0"]).is_err());
        let explicit = parse(&["--monitor","all","--namespace","one","--state","explicit.json"]).unwrap();
        assert_eq!(explicit.state,std::path::absolute("explicit.json").unwrap());
    }
}
