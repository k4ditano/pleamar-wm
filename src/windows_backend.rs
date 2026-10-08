//! A desktop companion on Windows. DWM continues to own composition and input.
use crate::layout::{self, Layout, Rect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, mem::size_of, path::Path, time::{Duration, Instant}};
use windows::{core::*, Win32::{Foundation::*, Graphics::{Dwm::*, Gdi::*},
    System::Threading::*, UI::{HiDpi::*, WindowsAndMessaging::*}}};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[path = "windows_ipc.rs"]
mod ipc;
#[path = "windows_session.rs"]
mod session;
#[path = "windows_rules.rs"]
mod rules;
#[path = "windows_fullscreen.rs"]
mod fullscreen;

#[path = "windows_capture.rs"]
mod capture;

#[path = "windows_wait.rs"]
mod wait;

#[path = "windows_preview.rs"]
mod preview;
#[path = "windows_transfer.rs"]
mod transfer;

#[path = "windows_agent.rs"]
mod agent;

#[path = "windows_launch.rs"]
mod launch;
#[path = "windows_dock.rs"]
mod dock;

const HELP: &str = "pleamar-wm — experimental native Windows desktop companion

  capabilities                     machine-readable support status
  agent help                       inspect and use pleamar scenes by name
  monitors                         connected displays and physical work areas (JSON)
  windows                          ordinary application windows (JSON)
  session --monitor NAME|all       start in free mode; --state FILE selects its recovery journal
          [--owner PID]            restore windows and exit when the owner exits
          [--rules FILE]           window rules; defaults to pleamar's session.conf
                                   --process PID scopes automatic management to one application
  --say wm COMMAND                status, toggle MONITOR, layout MONITOR KIND, free MONITOR, quit
                                   emit minimize, emit restore_last, emit toggle_free
                                   emit focus_next, emit focus_previous, emit close, emit fullscreen
                                   fullscreen ID toggles a catalog window without taking focus
  hyprctl monitors|activewindow     compatibility queries for existing scenes
  tile MONITOR LAYOUT --save FILE ID...
                                   tile these normal windows on their current monitor;
                                   save their positions to a new undo file first
  restore-layout FILE              restore those positions without taking focus
  window ID minimize|restore        act on one window from the current catalog
  --scene FILE [OPTIONS]            a native pleamar scene, including Luau and live reload
          --preview-monitor NAME   view-only live native windows in the scene (experimental)
          --preview-process PID    restrict those pictures to one current process
          --window-actions         allow native focus, close, minimize, restore, scene size, send and launch
                                   fullscreen uses the session's recovery journal

Layouts: left, right, columns, rows, grid. MONITOR is a display name or number
from `monitors`. IDs come from `windows`. Coordinates are physical pixels.
Compositor effects, pools and remote/agent seats are not implemented here yet.
Unsupported commands fail instead of pretending to work.";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Bounds { x: i32, y: i32, width: i32, height: i32 }
impl From<RECT> for Bounds {
    fn from(r: RECT) -> Self { Self { x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top } }
}
impl From<Rect> for Bounds {
    fn from(r: Rect) -> Self { Self { x: r.x, y: r.y, width: r.width, height: r.height } }
}
impl From<&Bounds> for Rect {
    fn from(r: &Bounds) -> Self { Self { x: r.x, y: r.y, width: r.width, height: r.height } }
}
impl Bounds {
    fn contains(&self, other: &Self) -> bool {
        other.width > 0 && other.height > 0 && other.x >= self.x && other.y >= self.y
            && i64::from(other.x) + i64::from(other.width) <= i64::from(self.x) + i64::from(self.width)
            && i64::from(other.y) + i64::from(other.height) <= i64::from(self.y) + i64::from(self.height)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
struct Monitor { name: String, bounds: Bounds, work: Bounds, scale: f64, primary: bool, refresh_hz: u32 }

fn wide_text(text: &[u16]) -> String {
    String::from_utf16_lossy(&text[..text.iter().position(|c| *c == 0).unwrap_or(text.len())])
}

fn monitor(handle: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if !unsafe { GetMonitorInfoW(handle, &mut info.monitorInfo) }.as_bool() { return None; }
    let mut x = 96;
    let mut y = 96;
    // An unavailable DPI is an error, not a silently assumed 100% scale.
    unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.ok()?;
    let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    if !unsafe { EnumDisplaySettingsW(PCWSTR(info.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool() { return None; }
    Some(Monitor { name: wide_text(&info.szDevice), bounds: info.monitorInfo.rcMonitor.into(),
        work: info.monitorInfo.rcWork.into(), scale: x as f64 / 96.0,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0, refresh_hz: mode.dmDisplayFrequency })
}

fn monitors() -> Result<Vec<Monitor>> {
    unsafe extern "system" fn visit(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let result = unsafe { &mut *(data.0 as *mut (Vec<Monitor>, bool)) };
        if let Some(m) = monitor(handle) { result.0.push(m); } else { result.1 = true; }
        true.into()
    }
    let mut result = (Vec::<Monitor>::new(), false);
    if !unsafe { EnumDisplayMonitors(None, None, Some(visit), LPARAM(&mut result as *mut _ as isize)) }.as_bool()
        || result.1 || result.0.is_empty() { return Err("could not read all active monitors".into()); }
    result.0.sort_by(|a, b| (a.bounds.x, a.bounds.y, &a.name).cmp(&(b.bounds.x, b.bounds.y, &b.name)));
    Ok(result.0)
}

#[derive(Clone, Copy, Debug)]
struct Identity { hwnd: HWND, pid: u32, thread: u32, created: u64 }
impl Identity {
    fn read(hwnd: HWND) -> Option<Self> {
        Self::details(hwnd, false).map(|(identity,_)|identity)
    }
    fn details(hwnd: HWND, with_app: bool) -> Option<(Self, String)> {
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if thread == 0 { return None; }
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut created = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let result = unsafe { GetProcessTimes(process, &mut created, &mut exit, &mut kernel, &mut user) };
        let app = if with_app { process_app(process).unwrap_or_default() } else { String::new() };
        let _ = unsafe { CloseHandle(process) };
        result.ok()?;
        Some((Self { hwnd, pid, thread, created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime) }, app))
    }
    fn token(self) -> String { format!("{}:{}:{:x}:{:x}", self.pid, self.thread, self.hwnd.0 as usize, self.created) }
}

fn process_app(process: HANDLE) -> Option<String> {
    for capacity in [512,32768] {
        let mut path = vec![0u16;capacity];
        let mut size = capacity as u32;
        match unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut size) } {
            Ok(()) => return String::from_utf16_lossy(&path[..size as usize]).rsplit(['\\','/']).next().map(str::to_owned),
            Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => continue,
            Err(_) => return None,
        }
    }
    None
}

#[derive(Clone, Debug, Serialize)]
struct Window { id: String, title: String, app: String, class: String, process: u32, monitor: String,
    bounds: Bounds, minimized: bool, maximized: bool, resizable: bool }

fn inspect(hwnd: HWND) -> Option<Window> { inspect_kind(hwnd, false) }
fn catalog_style(ex: WINDOW_EX_STYLE, owned: bool, include_owned: bool) -> bool {
    if ex.contains(WS_EX_NOACTIVATE) { return false; }
    if ex.contains(WS_EX_APPWINDOW) { return true; }
    if owned { include_owned } else { !ex.contains(WS_EX_TOOLWINDOW) }
}
fn inspect_kind(hwnd: HWND, include_owned: bool) -> Option<Window> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() { return None; }
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, size_of::<u32>() as u32).ok()?;
        if cloaked != 0 { return None; }
        let ex = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        if !catalog_style(ex, !GetWindow(hwnd, GW_OWNER).unwrap_or_default().is_invalid(), include_owned) { return None; }
        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") { return None; }
        let mut title = [0u16; 4096];
        let n = GetWindowTextW(hwnd, &mut title).max(0) as usize;
        if n == 0 { return None; }
        let (identity,app) = Identity::details(hwnd, true)?;
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let m = monitor(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL))?;
        let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
        Some(Window { id: identity.token(), title: String::from_utf16_lossy(&title[..n]), app, class,
            process: identity.pid, monitor: m.name, bounds: rect.into(), minimized: IsIconic(hwnd).as_bool(),
            maximized: IsZoomed(hwnd).as_bool(), resizable: style.contains(WS_THICKFRAME | WS_CAPTION) })
    }
}

fn windows() -> Result<Vec<Window>> { catalog(false) }
fn catalog(include_owned: bool) -> Result<Vec<Window>> {
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let (found, include_owned) = unsafe { &mut *(data.0 as *mut (Vec<Window>, bool)) };
        if let Some(window) = inspect_kind(hwnd, *include_owned) { found.push(window); }
        true.into()
    }
    let mut found = (Vec::<Window>::new(), include_owned);
    unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut _ as isize)) }?;
    Ok(found.0)
}

fn target(id: &str) -> Result<(HWND, Window)> { target_kind(id, false) }
fn target_kind(id: &str, include_owned: bool) -> Result<(HWND, Window)> {
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 4 { return Err("use an ID from `pleamar-wm windows`".into()); }
    let hwnd = HWND(usize::from_str_radix(parts[2], 16)? as _);
    let window = inspect_kind(hwnd, include_owned).ok_or("window closed, is hidden, or is outside this window catalog")?;
    if window.id != id { return Err("window identity changed; list windows again".into()); }
    Ok((hwnd, window))
}

fn normal(window: &Window) -> Result<()> {
    if window.minimized || window.maximized || !window.resizable {
        return Err(format!("{} must be a normal, resizable window before arranging it", window.id).into());
    }
    Ok(())
}

fn pump() {
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn place(id: &str, bounds: &Bounds) -> Result<()> {
    let (hwnd, current) = target(id)?;
    normal(&current)?;
    unsafe { SetWindowPos(hwnd, None, bounds.x, bounds.y, bounds.width, bounds.height,
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS) }?;
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        pump();
        let (_, current) = target(id)?;
        if current.bounds == *bounds { return Ok(()); }
        if Instant::now() >= until { return Err(format!("{id} did not accept the requested geometry").into()); }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedWindow { id: String, bounds: Bounds }
#[derive(Debug, Serialize, Deserialize)]
struct Snapshot { version: u32, monitor: String, windows: Vec<SavedWindow> }

fn tile(monitor: &Monitor, layout: Layout, ids: &[String], save: &Path) -> Result<Value> {
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() { return Err("duplicate window ID".into()); }
    let boxes = layout::arrange((&monitor.work).into(), ids.len(), layout, (8.0 * monitor.scale).round() as i32)?;
    let mut snapshot = Snapshot { version: 1, monitor: monitor.name.clone(), windows: Vec::new() };
    for id in ids {
        let (_, window) = target(id)?;
        normal(&window)?;
        if window.monitor != monitor.name { return Err(format!("{id} belongs to a different monitor").into()); }
        if !monitor.bounds.contains(&window.bounds) { return Err("bring each window wholly onto this monitor before tiling".into()); }
        snapshot.windows.push(SavedWindow { id: id.clone(), bounds: window.bounds });
    }
    // Never move a window before its recovery information is safely on disk.
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(save)?;
    serde_json::to_writer_pretty(&mut file, &snapshot)?;
    file.sync_all()?;
    for (i, (id, rect)) in ids.iter().zip(boxes).enumerate() {
        if let Err(error) = place(id, &rect.into()) {
            let mut failures = Vec::new();
            for old in snapshot.windows[..=i].iter().rev() {
                if let Err(e) = place(&old.id, &old.bounds) { failures.push(e.to_string()); }
            }
            return Err(format!("{error}; rollback errors: {failures:?}; recovery file: {}", save.display()).into());
        }
    }
    Ok(json!({"tiled": ids.len(), "monitor": monitor.name, "undo": save}))
}

fn restore_layout(path: &Path) -> Result<Value> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > 65536 { return Err("invalid layout snapshot size".into()); }
    let snapshot: Snapshot = serde_json::from_reader(file)?;
    if snapshot.version != 1 || snapshot.windows.is_empty() || snapshot.windows.len() > 64
        || snapshot.windows.iter().map(|w| &w.id).collect::<HashSet<_>>().len() != snapshot.windows.len() {
        return Err("invalid layout snapshot".into());
    }
    let m = monitors()?.into_iter().find(|m| m.name == snapshot.monitor).ok_or("the original monitor is disconnected")?;
    for old in &snapshot.windows {
        let (_, current) = target(&old.id)?;
        normal(&current)?;
        if current.monitor != m.name || !m.bounds.contains(&old.bounds) {
            return Err("a window or display moved since this snapshot; no positions were restored".into());
        }
    }
    let mut errors = Vec::new();
    for old in &snapshot.windows { if let Err(error) = place(&old.id, &old.bounds) { errors.push(error.to_string()); } }
    if !errors.is_empty() { return Err(format!("some positions could not be restored: {errors:?}").into()); }
    Ok(json!({"restored": snapshot.windows.len()}))
}

fn state(id: &str, minimize: bool) -> Result<Value> {
    window_state(id, minimize, false)
}
fn window_state(id: &str, minimize: bool, activate: bool) -> Result<Value> {
    let (hwnd, _) = target(id)?;
    let mode = match (minimize, activate) {
        (true, true) => SW_MINIMIZE,
        (false, true) => SW_RESTORE,
        (true, false) => SW_SHOWMINNOACTIVE,
        (false, false) => SW_SHOWNOACTIVATE,
    };
    if !unsafe { ShowWindowAsync(hwnd, mode) }.as_bool() {
        return Err("Windows rejected the window state change".into());
    }
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        pump();
        // During asynchronous restore, Windows can briefly report geometry
        // outside all monitors. Keep the exact identity while the catalog settles.
        if Identity::read(hwnd).is_none_or(|current| current.token() != id) {
            return Err("window identity changed during the state transition".into());
        }
        if let Some(current) = inspect(hwnd) {
            if current.id == id && current.minimized == minimize { return Ok(json!({"id": id, "minimized": minimize})); }
        }
        if Instant::now() >= until { return Err("window did not confirm the requested state".into()); }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn select_monitor(name: &str) -> Result<Monitor> {
    let screens = monitors()?;
    screens.iter().enumerate().find(|(i, m)| m.name == name || i.to_string() == name)
        .map(|(_, m)| m.clone()).ok_or_else(|| "monitor is not connected; use `pleamar-wm monitors`".into())
}

fn execute(args: &[String]) -> Result<Option<Value>> {
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => { println!("{HELP}"); Ok(None) }
        ["--version"] => Ok(Some(json!({"version": env!("CARGO_PKG_VERSION"), "platform": "windows", "experimental": true}))),
        ["capabilities"] => Ok(Some(json!({"platform": "windows", "experimental": true,
            "monitors": true, "windows": true, "explicit_layouts": true, "minimize_restore": true,
            "native_scenes_luau": true, "read_only_window_previews": true, "visible_window_capture": true,
            "preview_window_actions": ["focus", "close", "minimize", "restore", "configure", "send", "fullscreen"], "preview_redirected_input": false,
            "scene_launch": true, "agent_background_launch": false,
            "agent_window_capture": true, "agent_window_send": true, "agent_native_input": true, "agent_input_mode": "opt-in-foreground",
            "window_rules": ["app", "title", "float", "size", "monitor"], "private_window_rules": false, "workspace_window_rules": false,
            "window_scene_provider": false, "automatic_session": true, "rain": false, "snow": false,
            "session_shortcuts": ["minimize", "restore_last", "toggle_free", "focus_next", "focus_previous", "close", "fullscreen"],
            "application_dock": true, "ride": false, "dock": false, "pools": false, "remote": false, "phone_monitor": false, "independent_agent_seat": false,
            "agent_scene_commands": ["scenes", "tree", "press", "wait", "watch", "say"]}))),
        ["agent", rest @ ..] => agent::execute(rest),
        ["monitors"] => Ok(Some(serde_json::to_value(monitors()?)?)),
        ["windows"] => Ok(Some(serde_json::to_value(windows()?)?)),
        ["session", rest @ ..] => session::run(&rest.iter().map(|s|(*s).to_owned()).collect::<Vec<_>>()).map(Some),
        ["--say", "wm", command] => {
            let endpoint = ipc::Endpoint::current()?;
            let words:Vec<_>=command.split_whitespace().collect();
            if matches!(words.as_slice(),["emit","restore_last"|"focus_next"|"focus_previous"]) { endpoint.ask_with_focus(command).map(Some) }
            else { endpoint.ask(command).map(Some) }
        },
        ["window", id, "minimize"] => state(id, true).map(Some),
        ["window", id, "restore"] => state(id, false).map(Some),
        ["restore-layout", path] => restore_layout(Path::new(path)).map(Some),
        ["tile", screen, kind, "--save", path, ids @ ..] if !ids.is_empty() =>
            tile(&select_monitor(screen)?, kind.parse()?, &ids.iter().map(|id| (*id).into()).collect::<Vec<_>>(), Path::new(path)).map(Some),
        ["hyprctl", "monitors"] => {
            let focused = unsafe { GetForegroundWindow() };
            let active = monitor(unsafe { MonitorFromWindow(focused, MONITOR_DEFAULTTONULL) }).map(|m| m.name);
            for (id, m) in monitors()?.iter().enumerate() {
                println!("Monitor {} (ID {id}):\n\t{}x{}@{:.5} at {}x{}\n\tscale: {:.2}\n\tfocused: {}\n",
                    m.name, m.bounds.width, m.bounds.height, m.refresh_hz as f64, m.bounds.x, m.bounds.y,
                    m.scale, if active.as_deref() == Some(&m.name) { "yes" } else { "no" });
            }
            Ok(None)
        }
        ["hyprctl", "activewindow"] => {
            if let Some(w) = inspect(unsafe { GetForegroundWindow() }) {
                let title = w.title.replace(['\r', '\n', '\t'], " ");
                println!("Window {} -> {title}:\n\tat: {},{}\n\tsize: {},{}\n\tclass: {}\n\ttitle: {title}\n",
                    w.id, w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height, w.class);
            } else { println!("Invalid"); }
            Ok(None)
        }
        ["--scene" | "--check" | "--grammar" | "--docs" | "--say", ..] => {
            let args=args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
            let args=if args.first().is_some_and(|v|v=="--scene") { preview::prepare(&args)? } else { args };
            pleamar::run_with(args); Ok(None)
        }
        _ => Err(format!("unsupported Windows command: {}\nUse `pleamar-wm capabilities` or `--help`.", args.join(" ")).into()),
    }
}

pub fn run(args: Vec<String>) -> i32 {
    if let Some(code) = pleamar::windows_desktop::capture_helper(&args) { return code; }
    if args.first().map(String::as_str) == Some("session") { return run_session(args[1..].to_vec()); }
    // No windows or threads have been created yet; coordinates remain physical
    // when displays have different scaling and origins.
    if let Err(error) = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        eprintln!("pleamar-wm: could not enable per-monitor DPI: {error}"); return 1;
    }
    match execute(&args) {
        Ok(Some(value)) => { println!("{value}"); 0 }
        Ok(None) => 0,
        Err(error) => { eprintln!("pleamar-wm: {error}"); 1 }
    }
}

pub fn run_session(args: Vec<String>) -> i32 {
    use std::io::Write;
    if let Err(error) = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        eprintln!("pleamar-wm: could not enable per-monitor DPI: {error}"); return 1;
    }
    match session::run(&args) {
        // A GUI supervisor may have exited before us and closed its log pipe.
        Ok(value) => { let _ = writeln!(std::io::stdout().lock(), "{value}"); 0 }
        Err(error) => { let _ = writeln!(std::io::stderr().lock(), "pleamar-wm session: {error}"); 1 }
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "windows_session_tests.rs"]
mod session_tests;

#[cfg(test)]
#[path = "windows_capture_tests.rs"]
mod capture_tests;

#[cfg(test)]
#[path = "windows_preview_tests.rs"]
mod preview_tests;
