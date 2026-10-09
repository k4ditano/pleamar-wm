//! pleamar-wm's own session: no compositor underneath. It is started from a
//! TTY of its own (`pleamar-wm session scene.plm`), takes the monitors and the
//! input through the seat (libseat: logind or seatd, no root), and paints each
//! monitor straight into buffers of the card that go to the screen with a page
//! flip. Programs connect to the compositor inside the scene as before.
//!
//! Ctrl+Alt+Backspace leaves at once; Ctrl+Alt+F1…F12 go to another TTY and
//! back. Everything it does is said on its output: run it with that going to
//! a file, so that if the screen stays black there is something to read.

use pleamar::scene::{Cursor, Mods, Screens, Surface, ToRender};
use pleamar::wgpu;
use crate::config;
use crate::layers::{self, MonitorInfo, ToLayers};
use crate::route::Route;
use crate::screen::{self, Hit, LayerFrames, LayerWindow, Output, Screen};
use pleamar::{NewSheet, Target, View};
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session as _};
use smithay::backend::udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode as LoopMode, PostAction};
use smithay::reexports::drm::control::{connector, crtc, framebuffer, Device as ControlDevice, Event as DrmEvent, FbCmd2Flags, Mode, ModeTypeFlags, PageFlipFlags};
use smithay::reexports::gbm;
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::DeviceFd;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use xkbcommon::xkb;

/// The platform that drives the monitors itself.
pub struct Session;

impl pleamar::Platform for Session {
    fn run(self: Box<Self>, surfaces: Vec<Surface>, _extra_height: u32, _instance: wgpu::Instance, to_render: Sender<ToRender>) {
        keep_ahead();
        if let Err(e) = run(surfaces, to_render.clone()) {
            eprintln!("session · {e}");
            let _ = to_render.send(ToRender::Quit);
            std::thread::sleep(Duration::from_millis(300));
            std::process::exit(1);
        }
        // Done: pleamar stops the render and leaves (see `provide_before_quit`).
    }
}

/// Which of a monitor's buffers is on screen and which is on its way.
#[derive(Default)]
struct Flips {
    on_screen: Option<usize>,
    pending: Option<usize>,
    /// Whether the monitor has been given its mode with a first frame.
    set: bool,
}

struct Monitor {
    name: String,
    /// What drives it on the card; none on the phone's monitor, which has no
    /// screen (see phone.rs).
    crtc: Option<crtc::Handle>,
    connector: Option<connector::Handle>,
    /// Lit, or dark (idle, or a program turned it off).
    on: bool,
    /// Its size upright, as it is seen: a monitor on its side is taller than wide.
    size: (u32, u32),
    /// Quarter turns it stands at (`transform`): its cursor is turned with it.
    turn: u8,
    /// Where it is on the desktop: its top left corner, in units.
    x: i32,
    y: i32,
    /// How many pixels a unit is on it (1, 1.25, 1.5, 2): from the configuration.
    scale: f64,
    /// What is shown on it: its surfaces, put together.
    screen: Screen,
    flips: Arc<Mutex<Flips>>,
    mhz: i32,
}

/// A monitor's buffers on the card: three, what each frame is put together
/// in and shown with a page flip.
struct DrmOutput {
    drm: DrmDeviceFd,
    gbm: Arc<Mutex<gbm::Device<DrmDeviceFd>>>,
    connector: connector::Handle,
    crtc: crtc::Handle,
    mode: Mode,
    size: (u32, u32),
    name: String,
    buffers: Vec<Buffer>,
    failed: bool,
    flips: Arc<Mutex<Flips>>,
}

struct Buffer {
    _bo: gbm::BufferObject<()>,
    fb: framebuffer::Handle,
    texture: wgpu::Texture,
}

impl Monitor {
    /// Its size in units of the desktop.
    fn units(&self) -> (i32, i32) {
        ((self.size.0 as f64 / self.scale).round() as i32, (self.size.1 as f64 / self.scale).round() as i32)
    }
}

impl DrmOutput {
    fn make_buffers(&mut self, device: &wgpu::Device, modifiers: &[u64]) -> Result<(), String> {
        let (w, h) = self.size;
        let wanted: Vec<u64> = if modifiers.is_empty() { vec![0] } else { modifiers.to_vec() };
        for _ in 0..3 {
            let bo = self
                .gbm
                .lock()
                .unwrap()
                .create_buffer_object_with_modifiers2::<()>(w, h, gbm::Format::Xrgb8888, wanted.iter().map(|m| gbm::Modifier::from(*m)), gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING)
                .map_err(|e| format!("the card did not make a buffer of {w}×{h}: {e}"))?;
            let fd = bo.fd_for_plane(0).map_err(|e| format!("no handle for the buffer: {e}"))?;
            let modifier: u64 = bo.modifier().into();
            let texture = pleamar::gpu::Gpu::import_dmabuf(
                device,
                fd,
                (w, h),
                modifier,
                bo.stride_for_plane(0),
                bo.offset(0),
                wgpu::TextureUses::COLOR_TARGET,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                wgpu::TextureUses::UNINITIALIZED,
            )?;
            let flags = if bo.modifier() == gbm::Modifier::Invalid { FbCmd2Flags::empty() } else { FbCmd2Flags::MODIFIERS };
            let fb = self.drm.add_planar_framebuffer(&bo, flags).map_err(|e| format!("the monitor does not take the buffer: {e}"))?;
            self.buffers.push(Buffer { _bo: bo, fb, texture });
        }
        println!("session · {}: {w}×{h} at {} Hz, three buffers of the card (modifier {:#x})", self.name, self.mode.vrefresh(), wanted[0]);
        Ok(())
    }
}

impl Output for DrmOutput {
    fn buffer(&mut self, device: &wgpu::Device, modifiers: &[u64]) -> Option<(usize, wgpu::Texture)> {
        if self.failed {
            return None;
        }
        if self.buffers.is_empty() {
            if let Err(e) = self.make_buffers(device, modifiers) {
                eprintln!("session · {}: {e}", self.name);
                self.failed = true;
                return None;
            }
        }
        let f = self.flips.lock().unwrap();
        (0..self.buffers.len()).find(|k| f.on_screen != Some(*k) && f.pending != Some(*k)).map(|k| (k, self.buffers[k].texture.clone()))
    }

    fn show(&mut self, which: usize, done: pleamar::Sent, device: &wgpu::Device, _: &wgpu::Queue, anew: bool) -> bool {
        // The monitor shows what is in the buffer when it flips: it has to be put together by then.
        done.wait(device, Duration::from_millis(100));
        let mut f = self.flips.lock().unwrap();
        if anew {
            *f = Flips::default();
        }
        let fb = self.buffers[which].fb;
        if !f.set {
            match self.drm.set_crtc(self.crtc, Some(fb), (0, 0), &[self.connector], Some(self.mode)) {
                Ok(()) => {
                    f.set = true;
                    f.on_screen = Some(which);
                    if config::get().monitor(&self.name).is_some_and(|r| r.vrr) {
                        set_vrr(&self.drm, self.connector, self.crtc, &self.name);
                    }
                }
                Err(e) => eprintln!("session · {}: the monitor did not take its first frame: {e}", self.name),
            }
            false
        } else {
            match self.drm.page_flip(self.crtc, fb, PageFlipFlags::EVENT, None) {
                Ok(()) => {
                    f.pending = Some(which);
                    true
                }
                Err(e) => {
                    eprintln!("session · {}: a frame did not go to the screen: {e}", self.name);
                    false
                }
            }
        }
    }
}

/// Everything the loop holds.
struct State {
    /// Where the cursor plane goes, moved by a thread of its own.
    mover: CursorMover,
    session: LibSeatSession,
    drm: DrmDeviceFd,
    monitors: Vec<Monitor>,
    libinput: Libinput,
    to_render: Sender<ToRender>,
    keymap: xkb::State,
    pointer: (f64, f64),
    /// The cursor's shapes on the card, each with its hot spot, and which is shown.
    /// Each shape, for each way a monitor stands (quarter turns): its image
    /// on the card and its tip, both turned.
    cursors: Vec<(Cursor, u8, gbm::BufferObject<()>, (i32, i32))>,
    shown: Option<Cursor>,
    /// What the scene asks for, and what a program's surface under the pointer does.
    scene_cursor: Cursor,
    program_cursor: Cursor,
    scroll: f64,
    scroll_sideways: f64,
    /// A swipe on the touchpad under way: how many fingers, and how far they went;
    /// a pinch: how many, and how much bigger or smaller.
    swipe: Option<(u32, f64, f64)>,
    pinch: Option<(u32, f64)>,
    last_touch: std::time::Instant,
    /// The last input, and whether the monitors went dark for lack of it.
    last_input: std::time::Instant,
    dark_for_idle: bool,
    /// Where the pointer and the keys go: the scene or a program's surface.
    route: Route,
    /// Each shape's picture as it is on the card, unturned, with its tip: for
    /// whoever puts the pointer into a picture (sharing the screen).
    cursor_pictures: Vec<(Cursor, std::sync::Arc<(Vec<u8>, (i32, i32))>)>,
    /// The loop's handle: to start the timer of a key binding that repeats.
    handle: smithay::reexports::calloop::LoopHandle<'static, State>,
    /// What is needed to put monitors up when they are plugged in: the card's
    /// buffers, the scene's surfaces, and the sheets given to the render.
    gbm: Arc<Mutex<gbm::Device<DrmDeviceFd>>>,
    surfaces: Vec<Surface>,
    cursor_kind: Arc<Mutex<Cursor>>,
    sheets: Vec<u32>,
    next_sheet: u32,
    quit: bool,
    /// The mouse of this desk moved while the session was on the phone: how
    /// far in one go, to tell a hand on it from a table being bumped.
    desk_moved: f64,
    desk_moved_at: std::time::Instant,
}

fn run(surfaces: Vec<Surface>, to_render: Sender<ToRender>) -> Result<(), String> {
    let (mut session, notifier) = LibSeatSession::new().map_err(|e| format!("there is no seat to take ({e:?}): start it from a TTY of its own, logged in there"))?;
    let seat = session.seat();
    println!("session · seat {seat}");
    let card = primary_gpu(&seat)
        .ok()
        .flatten()
        .or_else(|| all_gpus(&seat).ok()?.into_iter().next())
        .ok_or("there is no graphics card")?;
    let fd = session.open(&card, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK).map_err(|e| format!("{} could not be opened: {e:?}", card.display()))?;
    let drm = DrmDeviceFd::new(DeviceFd::from(fd));
    println!("session · card {}", card.display());
    let gbm = Arc::new(Mutex::new(gbm::Device::new(drm.clone()).map_err(|e| format!("no buffers on the card (gbm): {e}"))?));

    // The monitors that are connected, each with its preferred mode and a
    // controller of its own, left to right as `PLEAMAR_MONITORS` says.
    let mut monitors: Vec<Monitor> = Vec::new();
    for (name, conn, mode, crtcs) in connected(&drm) {
        let used: Vec<crtc::Handle> = monitors.iter().filter_map(|m| m.crtc).collect();
        let Some(crtc) = crtcs.into_iter().find(|c| !used.contains(c)) else { continue };
        monitors.push(make_monitor(&drm, &gbm, name, conn, mode, crtc));
    }
    if monitors.is_empty() {
        return Err("no monitor is connected".into());
    }
    place_monitors(&mut monitors);

    // The scene's surfaces on the monitors.
    let cursor_kind = Arc::new(Mutex::new(Cursor::Normal));
    let mut next_sheet = 1000;
    let sheets = give_sheets(&monitors, &surfaces, &to_render, &cursor_kind, &mut next_sheet);
    // A surface that changes level or edge while running (Marea's).
    {
        let screens: Vec<Screen> = monitors.iter().map(|m| m.screen.clone()).collect();
        let again = screens.clone();
        pleamar::provide_layer_hooks(pleamar::LayerHooks {
            relayer: Box::new(move |which, level| {
                for sc in &screens {
                    let mut st = sc.0.lock().unwrap();
                    let mut any = false;
                    for l in st.layers.iter_mut().filter(|l| l.surface == which) {
                        l.level = level;
                        any = true;
                    }
                    if any {
                        st.dirty = true;
                        st.changed_all = true;
                        sc.1.notify_all();
                    }
                }
            }),
            reanchor: Box::new(move |which, anchor| {
                for sc in &again {
                    let mut st = sc.0.lock().unwrap();
                    let size = st.size;
                    let mut any = false;
                    for l in st.layers.iter_mut().filter(|l| l.surface == which && !l.main) {
                        l.anchor = anchor;
                        l.rect = screen::placed(l.units, anchor, l.margin, size, l.scale).1;
                        any = true;
                    }
                    if any {
                        st.dirty = true;
                        st.changed_all = true;
                        sc.1.notify_all();
                    }
                }
            }),
        });
    }

    // The keyboard as the system has it set up, for the scene and for its windows.
    let keymap = keymap()?;
    pleamar::set_host_keymap(keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1));
    let keymap = xkb::State::new(&keymap);
    let _ = to_render.send(ToRender::KeyboardFocus(true));
    // How keys repeat, for the scene's fields (the programs are told by the compositor).
    let (rate, delay) = config::get().repeat();
    let _ = to_render.send(ToRender::KeyRepeat(Some((delay, (1000 / rate).max(1)))));

    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput.udev_assign_seat(&seat).map_err(|()| "the input devices could not be taken")?;
    let input = LibinputInputBackend::new(libinput.clone());

    let mut event_loop: EventLoop<State> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let h = event_loop.handle();
    h.insert_source(input, |event, _, state: &mut State| state.input(event)).map_err(|e| e.to_string())?;
    // A monitor plugged in or out: the card's device changes.
    match UdevBackend::new(&seat) {
        Ok(udev) => {
            h.insert_source(udev, |event, _, state: &mut State| {
                if let UdevEvent::Changed { .. } = event {
                    state.rescan();
                }
            })
            .map_err(|e| e.to_string())?;
        }
        Err(e) => eprintln!("session · monitors plugged in later will not be seen ({e})"),
    }
    h.insert_source(notifier, |event, _, state: &mut State| state.session_event(event)).map_err(|e| e.to_string())?;
    h.insert_source(Generic::new(drm.clone(), Interest::READ, LoopMode::Level), |_, _, state: &mut State| {
        state.flipped();
        Ok(PostAction::Continue)
    })
    .map_err(|e| e.to_string())?;

    let first = monitors.first().map_or((0.0, 0.0), |m| (m.x as f64 + m.units().0 as f64 / 2.0, m.y as f64 + m.units().1 as f64 / 2.0));
    layers::set_card(drm.clone());
    layers::register(monitors.iter().map(|m| (MonitorInfo { name: m.name.clone(), size: m.size, x: m.x, y: m.y, mhz: m.mhz, scale: m.scale }, m.screen.clone())).collect());
    let mover = CursorMover::new(drm.clone());
    let mut state = State { mover, session, drm, monitors, libinput, to_render: to_render.clone(), keymap, pointer: first, cursors: Vec::new(), shown: None, scene_cursor: Cursor::Normal, program_cursor: Cursor::Normal, scroll: 0.0, scroll_sideways: 0.0, swipe: None, pinch: None, last_touch: std::time::Instant::now(), last_input: std::time::Instant::now(), dark_for_idle: false, route: Route::new(to_render), cursor_pictures: Vec::new(), handle: event_loop.handle(), gbm: gbm.clone(), surfaces, cursor_kind, sheets, next_sheet, quit: false, desk_moved: 0.0, desk_moved_at: std::time::Instant::now() };
    state.make_cursors(&gbm);
    // The cursor the scene and the programs ask for, whenever it changes.
    let (cursor_tx, cursor_rx) = smithay::reexports::calloop::channel::channel::<(bool, Cursor)>();
    layers::set_cursor_sink(cursor_tx);
    event_loop
        .handle()
        .insert_source(cursor_rx, |event, _, state: &mut State| {
            if let smithay::reexports::calloop::channel::Event::Msg((from_program, c)) = event {
                if from_program {
                    state.program_cursor = c;
                } else {
                    state.scene_cursor = c;
                }
                state.show_cursor();
            }
        })
        .map_err(|e| e.to_string())?;
    // Monitors on and off as a program asks (hypridle, wlopm).
    let (power_tx, power_rx) = smithay::reexports::calloop::channel::channel::<(Option<usize>, bool)>();
    layers::set_power_sink(power_tx);
    event_loop
        .handle()
        .insert_source(power_rx, |event, _, state: &mut State| {
            if let smithay::reexports::calloop::channel::Event::Msg((which, on)) = event {
                state.power(which, on);
            }
        })
        .map_err(|e| e.to_string())?;
    // The phone's monitor, as `pleamar-wm remote` asks (docs/phone.md).
    let (phone_tx, phone_rx) = smithay::reexports::calloop::channel::channel::<Option<layers::PhoneWish>>();
    layers::set_phone_sink(Box::new(move |wish| phone_tx.send(wish).is_ok()));
    event_loop
        .handle()
        .insert_source(phone_rx, |event, _, state: &mut State| {
            if let smithay::reexports::calloop::channel::Event::Msg(wish) = event {
                state.phone(wish);
            }
        })
        .map_err(|e| e.to_string())?;
    // Idleness, looked at once a second.
    event_loop
        .handle()
        .insert_source(smithay::reexports::calloop::timer::Timer::from_duration(Duration::from_secs(1)), |_, _, state: &mut State| {
            state.idle_check();
            smithay::reexports::calloop::timer::TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .map_err(|e| e.to_string())?;
    layers::set_powered(state.monitors.iter().map(|_| true).collect());
    state.move_pointer(0.0, 0.0);
    println!("session · running: Ctrl+Alt+Backspace leaves");
    while !state.quit {
        event_loop.dispatch(Some(Duration::from_millis(500)), &mut state).map_err(|e| e.to_string())?;
    }
    println!("session · leaving");
    Ok(())
}

/// The monitors connected to the card now and not turned off in the
/// configuration: their name (`DP-3`), connector, the mode asked for (or the
/// one they prefer) and the controllers that can drive them.
fn connected(drm: &DrmDeviceFd) -> Vec<(String, connector::Handle, Mode, Vec<crtc::Handle>)> {
    let Ok(res) = drm.resource_handles() else { return Vec::new() };
    let mut out = Vec::new();
    for &conn in res.connectors() {
        let Ok(info) = drm.get_connector(conn, true) else { continue };
        if info.state() != connector::State::Connected {
            continue;
        }
        let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
        let rule = config::get().monitor(&name);
        if rule.is_some_and(|r| r.off) {
            continue;
        }
        let Some(mode) = choose_mode(info.modes(), rule.map(|r| &r.mode)) else { continue };
        let crtcs: Vec<crtc::Handle> = info.encoders().iter().filter_map(|e| drm.get_encoder(*e).ok()).flat_map(|e| res.filter_crtcs(e.possible_crtcs())).collect();
        out.push((name, conn, mode, crtcs));
    }
    out
}

/// Its refresh in mHz, exactly (`vrefresh` rounds 164.997 up to 165).
fn refresh_mhz(m: &Mode) -> i32 {
    let (h, v) = (m.hsync().2 as i64, m.vsync().2 as i64);
    if h == 0 || v == 0 {
        return m.vrefresh() as i32 * 1000;
    }
    (m.clock() as i64 * 1_000_000 / (h * v)) as i32
}

/// A property of a connector or a controller by its name: its handle and value.
fn property(drm: &DrmDeviceFd, object: impl smithay::reexports::drm::control::ResourceHandle, name: &str) -> Option<(smithay::reexports::drm::control::property::Handle, u64)> {
    let props = drm.get_properties(object).ok()?;
    let (handles, values) = props.as_props_and_values();
    handles.iter().zip(values).find_map(|(h, v)| drm.get_property(*h).ok().filter(|p| p.name().to_str() == Ok(name)).map(|_| (*h, *v)))
}

/// Variable refresh, if the monitor can: the screen waits for the frame
/// instead of the frame for the screen (a game that does not reach 165).
fn set_vrr(drm: &DrmDeviceFd, conn: connector::Handle, crtc: crtc::Handle, name: &str) {
    if property(drm, conn, "vrr_capable").is_none_or(|(_, v)| v == 0) {
        println!("session · {name}: it cannot vary its refresh (vrr)");
        return;
    }
    match property(drm, crtc, "VRR_ENABLED").map(|(h, _)| drm.set_property(crtc, h, 1)) {
        Some(Ok(())) => println!("session · {name}: variable refresh on"),
        _ => eprintln!("session · {name}: variable refresh could not be turned on"),
    }
}

/// The mode asked for, among the ones the monitor has: that size at the
/// refresh closest to the one asked (the most it has, if none is asked), or
/// its biggest at its most, or the one it prefers.
fn choose_mode(modes: &[Mode], wish: Option<&config::ModeWish>) -> Option<Mode> {
    let preferred = modes.iter().find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED)).or(modes.first()).copied();
    match wish {
        Some(config::ModeWish::Exact(w, h, hz)) => {
            let same = modes.iter().filter(|m| m.size() == (*w as u16, *h as u16));
            let best = if *hz > 0.0 { same.min_by_key(|m| (refresh_mhz(m) - (hz * 1000.0) as i32).abs()) } else { same.max_by_key(|m| refresh_mhz(m)) };
            if best.is_none() {
                eprintln!("session · there is no {w}×{h} mode: the preferred one instead");
            }
            best.copied().or(preferred)
        }
        Some(config::ModeWish::Highest) => modes.iter().max_by_key(|m| (m.size().0 as u32 * m.size().1 as u32, refresh_mhz(m))).copied(),
        _ => preferred,
    }
}

/// Where a point of a monitor seen upright (`size` its upright size) falls
/// on it as it really is, turned `turn` quarter turns: the same turn
/// `screen::Turned` gives what is put together.
fn turned_point(turn: u8, (x, y): (f64, f64), (w, h): (f64, f64)) -> (f64, f64) {
    match turn % 4 {
        1 => (y, w - x),
        2 => (w - x, h - y),
        3 => (h - y, x),
        _ => (x, y),
    }
}

/// A monitor put up: its buffers on the card and the one that puts it together.
fn make_monitor(drm: &DrmDeviceFd, gbm: &Arc<Mutex<gbm::Device<DrmDeviceFd>>>, name: String, conn: connector::Handle, mode: Mode, crtc: crtc::Handle) -> Monitor {
    let (w, h) = mode.size();
    println!("session · monitor {name}: {w}×{h} at {:.2} Hz", refresh_mhz(&mode) as f64 / 1000.0);
    let real = (w as u32, h as u32);
    let flips: Arc<Mutex<Flips>> = Default::default();
    let output = DrmOutput { drm: drm.clone(), gbm: gbm.clone(), connector: conn, crtc, mode, size: real, name: name.clone(), buffers: Vec::new(), failed: false, flips: flips.clone() };
    // Standing on its side: everything else sees it upright, taller than wide.
    let turn = config::get().monitor(&name).map_or(0, |r| r.transform % 4);
    let (size, output): ((u32, u32), Box<dyn screen::Output>) = if turn == 0 {
        (real, Box::new(output))
    } else {
        println!("session · {name}: turned {}°", turn as u32 * 90);
        let t = screen::Turned::new(Box::new(output), turn, real);
        (if turn % 2 == 1 { (real.1, real.0) } else { real }, Box::new(t))
    };
    let screen = screen::screen(name.clone(), size, output);
    let scale = config::get().monitor(&name).and_then(|r| r.scale).unwrap_or(1.0).clamp(0.5, 4.0);
    if scale != 1.0 {
        println!("session · {name}: scale {scale}");
    }
    Monitor { name, crtc: Some(crtc), connector: Some(conn), on: true, size, turn, x: 0, y: 0, scale, screen, flips, mhz: refresh_mhz(&mode) }
}

/// Where the configuration puts them; the ones it does not place, left to
/// right after them, as `PLEAMAR_MONITORS` says («DP-3,HDMI-A-1») and then in
/// the card's order. Numbered left to right, top to bottom.
fn place_monitors(monitors: &mut [Monitor]) {
    // The phone's monitor last, far to the right of the real ones.
    monitors.sort_by_key(|m| m.crtc.is_none());
    let real = monitors.iter().filter(|m| m.crtc.is_some()).count();
    let (monitors, phones) = monitors.split_at_mut(real);
    if let Ok(order) = std::env::var("PLEAMAR_MONITORS") {
        let names: Vec<&str> = order.split(',').map(str::trim).collect();
        monitors.sort_by_key(|m| names.iter().position(|n| *n == m.name).unwrap_or(names.len()));
    }
    let placed: Vec<Option<(i32, i32)>> = monitors.iter().map(|m| config::get().monitor(&m.name).and_then(|r| r.at)).collect();
    let mut x = monitors.iter().zip(&placed).filter_map(|(m, p)| p.map(|(px, _)| px + m.units().0)).max().unwrap_or(0);
    for (m, p) in monitors.iter_mut().zip(&placed) {
        (m.x, m.y) = match p {
            Some(at) => *at,
            None => {
                let at = (x, 0);
                x += m.units().0;
                at
            }
        };
    }
    monitors.sort_by_key(|m| (m.x, m.y));
    let right = monitors.iter().map(|m| m.x + m.units().0).max().unwrap_or(0);
    for m in phones.iter_mut() {
        (m.x, m.y) = crate::phone::place(right);
    }
    println!("session · monitors: {}", monitors.iter().chain(phones.iter()).map(|m| format!("{} at {},{}", m.name, m.x, m.y)).collect::<Vec<_>>().join(", "));
}

/// The scene's surfaces on the monitors, as sheets for the render. Its own:
/// its copies (`screens: each`) one per monitor, in order; without copies, on
/// the first. The named ones —a bar, a corner, a panel—: where they say, and
/// put together over or under the scene's by their level.
fn give_sheets(monitors: &[Monitor], surfaces: &[Surface], to_render: &Sender<ToRender>, cursor_kind: &Arc<Mutex<Cursor>>, next: &mut u32) -> Vec<u32> {
    let mut given = Vec::new();
    let names: Vec<String> = monitors.iter().map(|m| m.name.clone()).collect();
    for (k, s) in surfaces.iter().enumerate() {
        let on: Vec<usize> = match &s.screens {
            Screens::Number(n) => vec![*n],
            Screens::Named(want) => names.iter().enumerate().filter(|(_, n)| want.contains(n)).map(|(i, _)| i).collect(),
            Screens::All if !s.name.is_empty() => (0..monitors.len()).collect(),
            Screens::All => vec![0],
        };
        for which in on {
            let Some(m) = monitors.get(which) else { continue };
            let taken = s.name.is_empty() && m.screen.0.lock().unwrap().layers.iter().any(|l| l.main);
            if taken {
                continue;
            }
            *next += 1;
            let id = *next;
            let layer = screen::layer(id, k, s, m.size, m.scale as f32);
            let size = (layer.rect[2] as u32, layer.rect[3] as u32);
            let units = layer.units;
            if !s.name.is_empty() {
                println!("session · the surface '{}' on {}: {}×{} at {},{}", s.name, m.name, size.0, size.1, layer.rect[0], layer.rect[1]);
            }
            m.screen.0.lock().unwrap().layers.push(layer);
            let _ = to_render.send(ToRender::Sheet(Box::new(NewSheet {
                id,
                target: Target::Frames(Box::new(LayerFrames::new(m.screen.clone(), id, size, to_render.clone()))),
                window: Box::new(LayerWindow { screen: m.screen.clone(), sheet: id, cursor: cursor_kind.clone() }),
                scale: m.scale as f32,
                size: units,
                mhz: m.mhz,
                name: m.name.clone(),
                view: View { surface: k, popup: None, origin: s.origin, size: (units.0 as f32, units.1 as f32) },
            })));
            given.push(id);
        }
    }
    given
}

/// The keyboard layout: the configuration's (or Hyprland's), else
/// `XKB_DEFAULT_LAYOUT`, else what `localectl` says the X11 layout is, else
/// the default one.
pub(crate) fn keymap() -> Result<xkb::Keymap, String> {
    let from_env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let k = &config::get().keyboard;
    let mut layout = k.layout.clone().or_else(|| from_env("XKB_DEFAULT_LAYOUT")).unwrap_or_default();
    let mut variant = k.variant.clone().or_else(|| from_env("XKB_DEFAULT_VARIANT")).unwrap_or_default();
    let options = k.options.clone().or_else(|| from_env("XKB_DEFAULT_OPTIONS"));
    if layout.is_empty() {
        if let Ok(out) = std::process::Command::new("localectl").arg("status").output() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("X11 Layout:") {
                    layout = v.trim().to_owned();
                }
                if let Some(v) = line.trim().strip_prefix("X11 Variant:") {
                    variant = v.trim().to_owned();
                }
            }
        }
    }
    println!("session · keyboard: {}{}", if layout.is_empty() { "the default" } else { &layout }, if variant.is_empty() { String::new() } else { format!(" ({variant})") });
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(&context, "", "", &layout, &variant, options, xkb::COMPILE_NO_FLAGS).ok_or_else(|| format!("the keyboard layout '{layout}' could not be read"))
}

impl State {
    /// The monitors again, after one was plugged in or out: the ones that are
    /// still there go on as they were; a new one is put up, one that left is
    /// let go, and the scene's surfaces are given again for the new row
    /// (which copy is on which monitor may have changed).
    fn rescan(&mut self) {
        let now = connected(&self.drm);
        let names: Vec<&str> = now.iter().map(|c| c.0.as_str()).collect();
        let before: Vec<String> = self.monitors.iter().map(|m| m.name.clone()).collect();
        let (kept, gone): (Vec<Monitor>, Vec<Monitor>) = std::mem::take(&mut self.monitors).into_iter().partition(|m| m.crtc.is_none() || names.contains(&m.name.as_str()));
        self.monitors = kept;
        for m in gone {
            println!("session · monitor {} unplugged", m.name);
            let mut st = m.screen.0.lock().unwrap();
            st.quit = true;
            drop(st);
            m.screen.1.notify_all();
            if let Some(crtc) = m.crtc {
                let _ = self.drm.set_crtc(crtc, None, (0, 0), &[], None);
            }
        }
        for (name, conn, mode, crtcs) in now {
            if self.monitors.iter().any(|m| m.name == name) {
                continue;
            }
            let used: Vec<crtc::Handle> = self.monitors.iter().filter_map(|m| m.crtc).collect();
            let Some(crtc) = crtcs.into_iter().find(|c| !used.contains(c)) else {
                eprintln!("session · {name}: no controller left to drive it");
                continue;
            };
            let m = make_monitor(&self.drm, &self.gbm, name, conn, mode, crtc);
            println!("session · monitor {} plugged in", m.name);
            self.monitors.push(m);
        }
        if self.monitors.iter().map(|m| m.name.clone()).collect::<Vec<_>>() == before {
            return;
        }
        self.monitors_changed();
    }

    /// The row of monitors is new (one plugged in or out, the phone's put up
    /// or taken down): placed again, and the scene's surfaces given again.
    fn monitors_changed(&mut self) {
        place_monitors(&mut self.monitors);
        if self.monitors.is_empty() {
            return;
        }
        // The scene's surfaces, given again for the new row of monitors.
        for id in std::mem::take(&mut self.sheets) {
            let _ = self.to_render.send(ToRender::SheetGone(id));
        }
        for m in &self.monitors {
            let mut st = m.screen.0.lock().unwrap();
            st.layers.clear();
            st.changed_all = true;
            st.dirty = true;
        }
        self.sheets = give_sheets(&self.monitors, &self.surfaces, &self.to_render, &self.cursor_kind, &mut self.next_sheet);
        layers::register(self.monitors.iter().map(|m| (MonitorInfo { name: m.name.clone(), size: m.size, x: m.x, y: m.y, mhz: m.mhz, scale: m.scale }, m.screen.clone())).collect());
        layers::tell(ToLayers::Monitors);
        // Which one is the phone's, for the scene (-1: none).
        let phone = self.monitors.iter().position(|m| m.crtc.is_none()).map_or(-1.0, |k| k as f32);
        let _ = self.to_render.send(ToRender::Fact(pleamar::scene::intern("phone"), phone));
        // The cursor on every monitor, and the pointer within them.
        self.shown = None;
        self.show_cursor();
        self.move_pointer(0.0, 0.0);
    }

    /// Back from the phone to this desk: the phone's monitor goes (its
    /// windows go back to where they were) and the session is locked —it was
    /// out of its owner's hands, and whoever is at the desk now has to say
    /// who they are—.
    fn take_back(&mut self) {
        println!("session · someone at the desk: the session comes back from the phone, locked");
        self.desk_moved = 0.0;
        self.phone(None);
        let lock = config::get().phone_lock.clone().unwrap_or_else(|| "marea lock".to_owned());
        if !lock.is_empty() && lock != "none" {
            layers::tell(ToLayers::Launch(lock));
        }
        let _ = self.to_render.send(ToRender::ExternalSignal(pleamar::scene::intern("phone_taken_back"), None));
    }

    /// The phone's monitor put up (of that size and scale: again if it
    /// changed, a phone turned on its side), or taken down.
    fn phone(&mut self, wish: Option<layers::PhoneWish>) {
        let now = self.monitors.iter().position(|m| m.crtc.is_none());
        if let (Some(k), Some(w)) = (now, wish) {
            let m = &self.monitors[k];
            if m.size == w.size && (m.scale - w.scale).abs() < 0.01 {
                return;
            }
        }
        if wish.is_none() && now.is_none() {
            return;
        }
        if let Some(k) = now {
            let m = self.monitors.remove(k);
            println!("session · the phone's monitor goes");
            let mut st = m.screen.0.lock().unwrap();
            st.quit = true;
            drop(st);
            m.screen.1.notify_all();
        }
        self.desk_moved = 0.0;
        if let Some(w) = wish {
            let screen = crate::phone::make(w.size, &self.to_render);
            let scale = w.scale.clamp(0.5, 4.0);
            println!("session · a monitor for the phone: {}×{} at scale {scale}", w.size.0, w.size.1);
            self.monitors.push(Monitor { name: layers::PHONE_NAME.to_owned(), crtc: None, connector: None, on: true, size: w.size, turn: 0, x: 0, y: 0, scale, screen, flips: Default::default(), mhz: crate::phone::PHONE_MHZ });
        }
        self.monitors_changed();
    }

    /// The pointer moved by that much: across the monitors as they are
    /// placed. Off all of them, it stays at the edge of the nearest.
    fn move_pointer(&mut self, dx: f64, dy: f64) {
        let (mut px, mut py) = (self.pointer.0 + dx, self.pointer.1 + dy);
        // Confined by a program: inside its window.
        if let Some(layers::Hold::Confined(Some(r))) = layers::pointer_hold() {
            px = px.clamp(r[0] as f64, (r[0] + r[2]).max(r[0] + 1) as f64 - 1.0);
            py = py.clamp(r[1] as f64, (r[1] + r[3]).max(r[1] + 1) as f64 - 1.0);
        }
        let clamp = |m: &Monitor| (px.clamp(m.x as f64, (m.x + m.units().0) as f64 - 1.0), py.clamp(m.y as f64, (m.y + m.units().1) as f64 - 1.0));
        let Some((on, (px, py))) = self
            .monitors
            .iter()
            .enumerate()
            .map(|(k, m)| (k, clamp(m)))
            .min_by(|a, b| {
                let d = |(x, y): (f64, f64)| (x - px).powi(2) + (y - py).powi(2);
                d(a.1).total_cmp(&d(b.1))
            })
        else {
            return;
        };
        self.pointer = (px, py);
        layers::move_drag((px, py));
        layers::set_pointer_at((px, py));
        // The monitor is put together in its pixels: from units to them, and
        // on one standing on its side, turned the way it stands.
        for (k, m) in self.monitors.iter().enumerate() {
            let hot = self.shown.and_then(|c| self.cursors.iter().find(|x| x.0 == c && x.1 == m.turn).or_else(|| self.cursors.iter().find(|x| x.0 == c && x.1 == 0))).map_or((0, 0), |x| x.3);
            let (cx, cy) = if k == on {
                let (x, y) = turned_point(m.turn, ((px - m.x as f64) * m.scale, (py - m.y as f64) * m.scale), (m.size.0 as f64, m.size.1 as f64));
                (x as i32 - hot.0, y as i32 - hot.1)
            } else {
                (-256, -256)
            };
            if let Some(crtc) = m.crtc {
                self.mover.to(crtc, (cx, cy));
            }
        }
        let m = &self.monitors[on];
        let (mx, my) = ((px - m.x as f64) * m.scale, (py - m.y as f64) * m.scale);
        let screen = m.screen.clone();
        if self.route.pointer(&screen, (mx, my)) {
            self.show_cursor();
        }
    }

    /// Every monitor's picture, for the route to look at.
    fn screens(&self) -> Vec<Screen> {
        self.monitors.iter().map(|m| m.screen.clone()).collect()
    }

    /// The cursor's shapes, on the card's cursor plane: moving the mouse does
    /// not repaint anything. From the system's cursor theme (XCURSOR_THEME, or
    /// what ~/.icons/default inherits), at XCURSOR_SIZE (24); an arrow of its
    /// own if there is none.
    fn make_cursors(&mut self, gbm: &Arc<Mutex<gbm::Device<DrmDeviceFd>>>) {
        let theme = std::env::var("XCURSOR_THEME").ok().filter(|t| !t.is_empty()).or_else(|| {
            let home = std::env::var("HOME").ok()?;
            let text = std::fs::read_to_string(format!("{home}/.icons/default/index.theme")).ok()?;
            text.lines().find_map(|l| l.trim().strip_prefix("Inherits=")).map(|v| v.split(',').next().unwrap_or("").trim().to_owned())
        });
        // As big as the monitors' scale asks (the plane holds up to 64).
        let most = self.monitors.iter().map(|m| m.scale).fold(1.0, f64::max);
        let size: u32 = (std::env::var("XCURSOR_SIZE").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(24.0) * most).round() as u32;
        let names: [(Cursor, &[&str]); 12] = [
            (Cursor::Normal, &["default", "left_ptr", "arrow"]),
            (Cursor::Hand, &["pointer", "hand2", "pointing_hand", "hand1"]),
            (Cursor::Text, &["text", "xterm", "ibeam"]),
            (Cursor::Grab, &["grab", "openhand", "hand1"]),
            (Cursor::Grabbing, &["grabbing", "closedhand", "fleur"]),
            (Cursor::EwResize, &["ew-resize", "col-resize", "sb_h_double_arrow", "h_double_arrow", "size_hor"]),
            (Cursor::NsResize, &["ns-resize", "row-resize", "sb_v_double_arrow", "v_double_arrow", "size_ver"]),
            (Cursor::NwseResize, &["nwse-resize", "size_fdiag", "bd_double_arrow", "bottom_right_corner"]),
            (Cursor::NeswResize, &["nesw-resize", "size_bdiag", "fd_double_arrow", "bottom_left_corner"]),
            (Cursor::Move, &["move", "all-scroll", "fleur", "size_all"]),
            (Cursor::NotAllowed, &["not-allowed", "crossed_circle", "forbidden"]),
            (Cursor::Crosshair, &["crosshair", "cross", "tcross"]),
        ];
        let loaded = theme.as_deref().map(xcursor::CursorTheme::load);
        for (kind, candidates) in names {
            let image = loaded.as_ref().and_then(|t| {
                candidates.iter().find_map(|n| {
                    let path = t.load_icon(n)?;
                    let data = std::fs::read(path).ok()?;
                    let images = xcursor::parser::parse_xcursor(&data)?;
                    // The size closest to the one asked for, that fits the plane.
                    images.into_iter().filter(|i| i.width <= 64 && i.height <= 64).min_by_key(|i| (i.size as i64 - size as i64).abs())
                })
            });
            let Ok(mut bo) = gbm.lock().unwrap().create_buffer_object::<()>(64, 64, gbm::Format::Argb8888, gbm::BufferObjectFlags::CURSOR | gbm::BufferObjectFlags::WRITE) else {
                eprintln!("session · no cursor on the card: the pointer will not be seen");
                return;
            };
            let mut px = vec![0u8; 64 * 64 * 4];
            let hot = match &image {
                Some(i) => {
                    for y in 0..i.height as usize {
                        let row = &i.pixels_rgba[y * i.width as usize * 4..(y + 1) * i.width as usize * 4];
                        px[y * 64 * 4..y * 64 * 4 + row.len()].copy_from_slice(row);
                    }
                    (i.xhot as i32, i.yhot as i32)
                }
                None => {
                    if kind != Cursor::Normal {
                        continue;
                    }
                    // The usual arrow: white, with a dark edge.
                    let arrow: [&str; 19] = [
                        "X", "XX", "X.X", "X..X", "X...X", "X....X", "X.....X", "X......X", "X.......X", "X........X", "X.........X", "X..........X", "X......XXXXX", "X...X..X", "X..XX..X",
                        "X.X  X..X", "XX   X..X", "X     X..X", "      XXX",
                    ];
                    for (y, row) in arrow.iter().enumerate() {
                        for (x, c) in row.chars().enumerate() {
                            let v: [u8; 4] = match c {
                                'X' => [20, 20, 20, 255],
                                '.' => [255, 255, 255, 255],
                                _ => continue,
                            };
                            let i = (y * 64 + x) * 4;
                            px[i..i + 4].copy_from_slice(&v);
                        }
                    }
                    (0, 0)
                }
            };
            if bo.write(&px).is_err() {
                continue;
            }
            self.cursor_pictures.push((kind, std::sync::Arc::new((px.clone(), hot))));
            self.cursors.push((kind, 0, bo, hot));
            // And turned, for the monitors that stand on their side.
            let mut turns: Vec<u8> = self.monitors.iter().map(|m| m.turn).filter(|t| *t != 0).collect();
            turns.sort();
            turns.dedup();
            for turn in turns {
                let mut out = vec![0u8; 64 * 64 * 4];
                for y in 0..64usize {
                    for x in 0..64usize {
                        let (qx, qy) = turned_point(turn, (x as f64, y as f64), (63.0, 63.0));
                        let (i, o) = ((y * 64 + x) * 4, (qy as usize * 64 + qx as usize) * 4);
                        out[o..o + 4].copy_from_slice(&px[i..i + 4]);
                    }
                }
                let (hx, hy) = turned_point(turn, (hot.0 as f64, hot.1 as f64), (63.0, 63.0));
                let Ok(mut bo) = gbm.lock().unwrap().create_buffer_object::<()>(64, 64, gbm::Format::Argb8888, gbm::BufferObjectFlags::CURSOR | gbm::BufferObjectFlags::WRITE) else { continue };
                if bo.write(&out).is_ok() {
                    self.cursors.push((kind, turn, bo, (hx as i32, hy as i32)));
                }
            }
        }
        println!("session · cursor: {} ({} shapes)", theme.as_deref().unwrap_or("its own arrow"), self.cursors.len());
        self.show_cursor();
    }

    /// The cursor of whoever has the pointer, if it is not the one shown.
    fn show_cursor(&mut self) {
        let want = if matches!(self.route.hit, Hit::Client(..)) { self.program_cursor } else { self.scene_cursor };
        // A shape the theme lacks is shown as the arrow.
        let want = if self.cursors.iter().any(|c| c.0 == want) { want } else { Cursor::Normal };
        if !self.cursors.iter().any(|c| c.0 == want) {
            return;
        }
        if self.shown == Some(want) {
            return;
        }
        use smithay::reexports::drm::buffer::Buffer;
        for m in &self.monitors {
            let Some(crtc) = m.crtc else { continue };
            let Some((_, _, bo, hot)) = self.cursors.iter().find(|c| c.0 == want && c.1 == m.turn).or_else(|| self.cursors.iter().find(|c| c.0 == want && c.1 == 0)) else { continue };
            let image = CursorImage { size: Buffer::size(bo), format: Buffer::format(bo), pitch: Buffer::pitch(bo), handle: Buffer::handle(bo) };
            self.mover.shape(crtc, image, *hot);
        }
        self.shown = Some(want);
        layers::set_pointer_picture(self.cursor_pictures.iter().find(|c| c.0 == want).map(|c| c.1.clone()));
        // Its tip where the pointer is (told again: a new shape may have
        // put the plane back anywhere).
        self.mover.sent.clear();
        self.move_pointer(0.0, 0.0);
    }

    /// Page flips done: the frame on its way is on screen, and the monitor can
    /// be put together again.
    fn flipped(&mut self) {
        let Ok(events) = self.drm.receive_events() else { return };
        for e in events {
            if let DrmEvent::PageFlip(e) = e {
                for m in self.monitors.iter().filter(|m| m.crtc == Some(e.crtc)) {
                    {
                        let mut f = m.flips.lock().unwrap();
                        if let Some(p) = f.pending.take() {
                            f.on_screen = Some(p);
                        }
                    }
                    screen::landed(&m.screen, &self.to_render);
                }
            }
        }
    }

    fn session_event(&mut self, e: SessionEvent) {
        match e {
            SessionEvent::PauseSession => {
                println!("session · another TTY has the screen");
                self.libinput.suspend();
                for m in &self.monitors {
                    m.flips.lock().unwrap().pending = None;
                    let mut st = m.screen.0.lock().unwrap();
                    st.paused = true;
                    st.idle = true;
                    m.screen.1.notify_all();
                }
            }
            SessionEvent::ActivateSession => {
                println!("session · the screen is ours again");
                if self.libinput.resume().is_err() {
                    eprintln!("session · the input devices did not come back");
                }
                for m in &self.monitors {
                    let mut st = m.screen.0.lock().unwrap();
                    st.paused = false;
                    st.anew = true;
                    st.dirty = true;
                    st.changed_all = true;
                    m.screen.1.notify_all();
                }
                // The cursor again: another TTY may have left its own.
                self.shown = None;
                self.show_cursor();
                let _ = self.to_render.send(ToRender::Repaint);
            }
        }
    }

    fn input(&mut self, event: InputEvent<LibinputInputBackend>) {
        // Anything but a device coming or going is someone there.
        if !matches!(event, InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. }) {
            self.touched();
        }
        // The session is on the phone and someone at this desk uses it: it
        // comes back here, locked (docs/phone.md). The phone's own taps come
        // through the remote's devices, and do not count.
        if self.monitors.iter().any(|m| m.crtc.is_none()) {
            use smithay::backend::input::Event;
            let here = |name: String| !name.starts_with("pleamar remote");
            // A key someone types with, not one a headset, a power button or
            // a remote sends by itself (volume, media, power, sleep): the
            // keyboard's own keys, its arrows and pad, and Super.
            let typed = |code: u32| code < 112 || code == 119 || (125..=127).contains(&code);
            let at_desk = match &event {
                InputEvent::Keyboard { event } => {
                    let code = event.key_code().raw().saturating_sub(8);
                    (event.state() == KeyState::Pressed && here(event.device().name().to_owned()) && typed(code)).then(|| format!("key {code} on «{}»", event.device().name()))
                }
                InputEvent::PointerButton { event } => (event.state() == ButtonState::Pressed && here(event.device().name().to_owned())).then(|| format!("button {} on «{}»", event.button_code(), event.device().name())),
                InputEvent::PointerMotion { event } if here(event.device().name().to_owned()) => {
                    // (Still for a second, it starts counting again.)
                    if self.desk_moved_at.elapsed() > Duration::from_secs(1) {
                        self.desk_moved = 0.0;
                    }
                    self.desk_moved_at = std::time::Instant::now();
                    self.desk_moved += event.delta_x().abs() + event.delta_y().abs();
                    (self.desk_moved > 60.0).then(|| format!("«{}» moved", event.device().name()))
                }
                _ => None,
            };
            if let Some(what) = at_desk {
                println!("session · at the desk: {what}");
                self.take_back();
                return;
            }
        }
        match event {
            InputEvent::PointerMotion { event } => {
                // As it moved, for a program that locked the pointer (a game).
                layers::tell(ToLayers::Relative { dx: event.delta_x(), dy: event.delta_y(), ux: event.delta_x_unaccel(), uy: event.delta_y_unaccel(), utime: smithay::backend::input::Event::time(&event) });
                match layers::pointer_hold() {
                    // Locked: the pointer stays where it is; only the motion is told.
                    Some(layers::Hold::Locked) => {}
                    _ => self.move_pointer(event.delta_x(), event.delta_y()),
                }
            }
            InputEvent::PointerMotionAbsolute { event } => {
                // A tablet or a virtual machine's pointer: over all of the desktop.
                let x0 = self.monitors.iter().map(|m| m.x).min().unwrap_or(0);
                let y0 = self.monitors.iter().map(|m| m.y).min().unwrap_or(0);
                let x1 = self.monitors.iter().map(|m| m.x + m.units().0).max().unwrap_or(1);
                let y1 = self.monitors.iter().map(|m| m.y + m.units().1).max().unwrap_or(1);
                let p = event.position_transformed((x1 - x0, y1 - y0).into());
                let (dx, dy) = (p.x + x0 as f64 - self.pointer.0, p.y + y0 as f64 - self.pointer.1);
                self.move_pointer(dx, dy);
            }
            InputEvent::DeviceAdded { mut device } => set_up_device(&mut device),
            InputEvent::PointerButton { event } => {
                let down = event.state() == ButtonState::Pressed;
                let screens = self.screens();
                if self.route.button(&screens, event.button_code(), down) && !down {
                    self.move_pointer(0.0, 0.0);
                }
            }
            InputEvent::PointerAxis { event } => {
                // A wheel in notches; a touchpad, a notch every 15 px of finger.
                let notches = match (event.source(), event.amount_v120(Axis::Vertical), event.amount(Axis::Vertical)) {
                    (AxisSource::Wheel, Some(v), _) => -v / 120.0,
                    (_, _, Some(a)) => {
                        self.scroll += a;
                        let n = (self.scroll / 15.0).trunc();
                        self.scroll -= n * 15.0;
                        -n
                    }
                    _ => 0.0,
                };
                if notches != 0.0 {
                    self.route.wheel(notches as f32);
                }
                let sideways = match (event.source(), event.amount_v120(Axis::Horizontal), event.amount(Axis::Horizontal)) {
                    (AxisSource::Wheel, Some(v), _) => -v / 120.0,
                    (_, _, Some(a)) => {
                        self.scroll_sideways += a;
                        let n = (self.scroll_sideways / 15.0).trunc();
                        self.scroll_sideways -= n * 15.0;
                        -n
                    }
                    _ => 0.0,
                };
                if sideways != 0.0 {
                    self.route.wheel_sideways(sideways as f32);
                }
            }
            InputEvent::Keyboard { event } => self.key(event.key_code(), event.state() == KeyState::Pressed),
            // Swipes and pinches with three or more fingers are the scene's, by
            // name: `swipe3_down`, `swipe4_left`, `pinch3_in`… (two are the
            // programs' scrolling). One it does not declare does nothing.
            InputEvent::GestureSwipeBegin { event } => self.swipe = Some((event.fingers(), 0.0, 0.0)),
            InputEvent::GestureSwipeUpdate { event } => {
                if let Some(s) = &mut self.swipe {
                    s.1 += event.delta_x();
                    s.2 += event.delta_y();
                }
            }
            InputEvent::GestureSwipeEnd { event } => {
                if let Some((fingers, dx, dy)) = self.swipe.take() {
                    if !event.cancelled() && fingers >= 3 && dx.abs().max(dy.abs()) > 60.0 {
                        let way = if dx.abs() > dy.abs() { if dx > 0.0 { "right" } else { "left" } } else if dy > 0.0 { "down" } else { "up" };
                        self.gesture(format!("swipe{fingers}_{way}"));
                    }
                }
            }
            InputEvent::GesturePinchBegin { event } => self.pinch = Some((event.fingers(), 1.0)),
            InputEvent::GesturePinchUpdate { event } => {
                if let Some(p) = &mut self.pinch {
                    p.1 = event.scale();
                }
            }
            InputEvent::GesturePinchEnd { event } => {
                if let Some((fingers, scale)) = self.pinch.take() {
                    if !event.cancelled() && fingers >= 3 && !(0.8..=1.25).contains(&scale) {
                        self.gesture(format!("pinch{fingers}_{}", if scale < 1.0 { "in" } else { "out" }));
                    }
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, code: xkb::Keycode, down: bool) {
        let sym = self.keymap.key_get_one_sym(code);
        let name = xkb::keysym_get_name(sym);
        let typed = if down { Some(self.keymap.key_get_utf8(code)).filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) } else { None };
        let active = |s: &xkb::State, m: &str| s.mod_name_is_active(m, xkb::STATE_MODS_EFFECTIVE);
        let mods = Mods { ctrl: active(&self.keymap, xkb::MOD_NAME_CTRL), alt: active(&self.keymap, xkb::MOD_NAME_ALT), shift: active(&self.keymap, xkb::MOD_NAME_SHIFT), logo: active(&self.keymap, xkb::MOD_NAME_LOGO) };
        self.keymap.update_key(code, if down { xkb::KeyDirection::Down } else { xkb::KeyDirection::Up });
        let evdev = code.raw().saturating_sub(8);
        if down {
            // The way out, whatever the scene is doing.
            if mods.ctrl && mods.alt && name == "BackSpace" {
                self.quit = true;
                return;
            }
            // Ctrl+Alt+F1…F12: the keymap already says it as «switch to VT n».
            if let Some(vt) = name.strip_prefix("XF86Switch_VT_").and_then(|n| n.parse::<i32>().ok()) {
                if let Err(e) = self.session.change_vt(vt) {
                    eprintln!("session · could not go to TTY {vt}: {e:?}");
                }
                return;
            }
        }
        // The key's own symbol, as if nothing were held (for the bindings).
        let km = self.keymap.get_keymap();
        let base = km.key_get_syms_by_level(code, self.keymap.key_get_layout(code), 0).first().map(|s| xkb::keysym_get_name(*s));
        let screens = self.screens();
        self.route.key(&screens, &name, base.as_deref(), typed, mods, evdev, down);
        // Held, it acts again: after the keyboard's delay, at its rate.
        if let Some(held) = self.route.take_new_repeat() {
            let (rate, delay) = config::get().repeat();
            let every = Duration::from_millis((1000 / rate).max(1) as u64);
            let _ = self.handle.insert_source(smithay::reexports::calloop::timer::Timer::from_duration(Duration::from_millis(delay as u64)), move |_, _, state: &mut State| {
                if state.route.repeat(held) {
                    smithay::reexports::calloop::timer::TimeoutAction::ToDuration(every)
                } else {
                    smithay::reexports::calloop::timer::TimeoutAction::Drop
                }
            });
        }
    }

    /// Someone is there: the compositor tells whoever watches for idleness
    /// (not more than a few times a second).
    fn touched(&mut self) {
        self.last_input = std::time::Instant::now();
        // Dark for lack of input: any input lights them again.
        if self.dark_for_idle {
            self.dark_for_idle = false;
            self.power(None, true);
        }
        if self.last_touch.elapsed() > Duration::from_millis(200) {
            self.last_touch = std::time::Instant::now();
            layers::tell(ToLayers::Activity);
        }
    }

    /// Monitors on or off (all, if none is said): off, dark (DPMS) and not
    /// put together; on, lit and put together again from the start.
    fn power(&mut self, which: Option<usize>, on: bool) {
        for (k, m) in self.monitors.iter_mut().enumerate() {
            // (The phone's goes on while the phone is there: someone reading
            // on it touches nothing for a while.)
            if which.is_some_and(|w| w != k) || m.on == on || (m.crtc.is_none() && !on) {
                continue;
            }
            m.on = on;
            if let Some((conn, (h, _))) = m.connector.and_then(|c| Some((c, property(&self.drm, c, "DPMS")?))) {
                // 0 on, 3 off.
                if let Err(e) = self.drm.set_property(conn, h, if on { 0 } else { 3 }) {
                    eprintln!("session · {}: could not turn it {}: {e}", m.name, if on { "on" } else { "off" });
                }
            }
            println!("session · {} {}", m.name, if on { "on" } else { "off" });
            let mut st = m.screen.0.lock().unwrap();
            if on {
                st.paused = false;
                st.anew = true;
                st.dirty = true;
                st.changed_all = true;
            } else {
                st.paused = true;
                st.idle = true;
                m.flips.lock().unwrap().pending = None;
            }
            drop(st);
            m.screen.1.notify_all();
        }
        layers::set_powered(self.monitors.iter().map(|m| m.on).collect());
        layers::tell(ToLayers::Power);
        if on {
            self.shown = None;
            self.show_cursor();
            let _ = self.to_render.send(ToRender::Repaint);
        }
    }

    /// Once a second: dark after `idle off-after` seconds without input,
    /// unless a program keeps the screen awake.
    fn idle_check(&mut self) {
        let Some(secs) = config::get().off_after else { return };
        if !self.dark_for_idle && !layers::inhibited() && self.last_input.elapsed() > Duration::from_secs(secs) && self.monitors.iter().any(|m| m.on) {
            println!("session · {secs} s without input: the monitors go dark");
            self.dark_for_idle = true;
            self.power(None, false);
        }
    }

    /// A touchpad gesture, to the scene as the event of that name.
    fn gesture(&self, name: String) {
        if layers::locked() {
            return;
        }
        println!("session · gesture {name}");
        // Bound in keys.conf, its action; if not, the scene's event of that name.
        match crate::keys::get().gesture(&name) {
            Some(action) => self.route.perform(action),
            None => {
                let _ = self.to_render.send(ToRender::ExternalSignal(pleamar::scene::intern(&name), None));
            }
        }
    }
}

/// A mouse or a touchpad as the configuration says: its acceleration and
/// speed, and on a touchpad, tapping, natural scrolling and ignoring it while
/// typing.
fn set_up_device(device: &mut smithay::reexports::input::Device) {
    use smithay::reexports::input::{AccelProfile, DeviceCapability};
    if !device.has_capability(DeviceCapability::Pointer) {
        return;
    }
    let touchpad = device.config_tap_finger_count() > 0;
    let c = config::get();
    let p = if touchpad { &c.touchpad } else { &c.pointer };
    match p.accel.as_deref() {
        Some("flat") => {
            let _ = device.config_accel_set_profile(AccelProfile::Flat);
        }
        Some("adaptive") => {
            let _ = device.config_accel_set_profile(AccelProfile::Adaptive);
        }
        _ => {}
    }
    if let Some(s) = p.speed {
        let _ = device.config_accel_set_speed(s.clamp(-1.0, 1.0));
    }
    if let Some(n) = p.natural {
        let _ = device.config_scroll_set_natural_scroll_enabled(n);
    }
    if touchpad {
        // Tapping on unless it is said otherwise, as most desktops do.
        let _ = device.config_tap_set_enabled(p.tap.unwrap_or(true));
        if let Some(d) = p.dwt {
            let _ = device.config_dwt_set_enabled(d);
        }
    }
    println!("session · {}: {}", device.name(), if touchpad { "a touchpad" } else { "a pointer" });
}

/// A cursor's picture as the card knows it: the buffer itself stays with the
/// session (`cursors`), which keeps it alive; this goes to the cursor thread.
#[derive(Clone, Copy)]
struct CursorImage {
    size: (u32, u32),
    format: smithay::reexports::drm::buffer::DrmFourcc,
    pitch: u32,
    handle: smithay::reexports::drm::buffer::Handle,
}

impl smithay::reexports::drm::buffer::Buffer for CursorImage {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn format(&self) -> smithay::reexports::drm::buffer::DrmFourcc {
        self.format
    }
    fn pitch(&self) -> u32 {
        self.pitch
    }
    fn handle(&self) -> smithay::reexports::drm::buffer::Handle {
        self.handle
    }
}

/// What a monitor's cursor plane is to become: a new picture (with its tip), a new place.
#[derive(Default)]
struct CursorWant {
    image: Option<(CursorImage, (i32, i32))>,
    at: Option<(i32, i32)>,
}

/// The cursor plane, moved and changed from a thread of its own. Both are
/// calls to the card's driver that may wait for the monitor's next refresh
/// (NVIDIA's do): made from the loop that reads the mouse, a mouse that
/// speaks a thousand times a second —and a browser whose cursor changes on
/// every link— kept it behind (libinput: «lagging behind by 30 ms»), and with
/// it everything the loop does. Here the loop only leaves what it wants; the
/// thread takes the latest and does it, and what came in between is skipped.
struct CursorMover {
    wanted: Arc<(Mutex<std::collections::HashMap<crtc::Handle, CursorWant>>, std::sync::Condvar)>,
    /// Where each was last left, so a monitor the pointer is not on is not
    /// told again and again to hide it.
    sent: std::collections::HashMap<crtc::Handle, (i32, i32)>,
}

impl CursorMover {
    fn new(drm: DrmDeviceFd) -> CursorMover {
        let wanted: Arc<(Mutex<std::collections::HashMap<crtc::Handle, CursorWant>>, std::sync::Condvar)> = Arc::default();
        let shared = wanted.clone();
        let _ = std::thread::Builder::new().name("cursor".into()).spawn(move || {
            let (lock, cv) = &*shared;
            loop {
                let batch: Vec<(crtc::Handle, CursorWant)> = {
                    let mut w = lock.lock().unwrap();
                    while w.is_empty() {
                        w = cv.wait(w).unwrap();
                    }
                    w.drain().collect()
                };
                for (crtc, want) in batch {
                    if let Some((image, hot)) = want.image {
                        #[allow(deprecated)]
                        if let Err(e) = drm.set_cursor2(crtc, Some(&image), hot) {
                            eprintln!("session · no cursor ({e})");
                        }
                    }
                    if let Some(at) = want.at {
                        #[allow(deprecated)]
                        let _ = drm.move_cursor(crtc, at);
                    }
                }
            }
        });
        CursorMover { wanted, sent: Default::default() }
    }

    fn to(&mut self, crtc: crtc::Handle, at: (i32, i32)) {
        if self.sent.get(&crtc) == Some(&at) {
            return;
        }
        self.sent.insert(crtc, at);
        let (lock, cv) = &*self.wanted;
        lock.lock().unwrap().entry(crtc).or_default().at = Some(at);
        cv.notify_one();
    }

    fn shape(&mut self, crtc: crtc::Handle, image: CursorImage, hot: (i32, i32)) {
        let (lock, cv) = &*self.wanted;
        lock.lock().unwrap().entry(crtc).or_default().image = Some((image, hot));
        cv.notify_one();
    }
}

/// The threads a frame goes through —the input loop, the windows, the render,
/// each monitor's composing, the cursor— run ahead of the programs, as
/// Hyprland's do (round robin at priority 1). A browser loading a page keeps
/// every core busy, and a compositor at the same level as its threads waited
/// its turn in the middle of a frame: 15, 18, 25 ms where the frame had 6.
/// Only them, and never what they start (reset on fork): a program launched
/// from here runs as any other. The monitors' threads come when their first
/// frame does, so it looks again every couple of seconds.
fn keep_ahead() {
    let _ = std::thread::Builder::new().name("keep ahead".into()).spawn(|| {
        let ours = |name: &str| matches!(name, "pleamar-wm" | "render" | "windows" | "cursor") || name.starts_with("screen ");
        let mut done: std::collections::HashSet<i32> = Default::default();
        let mut told = false;
        loop {
            if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
                for t in tasks.flatten() {
                    let Some(tid) = t.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
                    if done.contains(&tid) {
                        continue;
                    }
                    let name = std::fs::read_to_string(t.path().join("comm")).unwrap_or_default();
                    if !ours(name.trim()) {
                        continue;
                    }
                    let param = libc::sched_param { sched_priority: 1 };
                    // SAFETY: a thread of this process, by its id; the parameters are valid.
                    let r = unsafe { libc::sched_setscheduler(tid, libc::SCHED_RR | libc::SCHED_RESET_ON_FORK, &param) };
                    if r == 0 {
                        done.insert(tid);
                    } else if !told {
                        told = true;
                        eprintln!("session · the compositor's threads could not run ahead of the programs ({}): it may stutter under load", std::io::Error::last_os_error());
                    }
                }
            }
            if !told && !done.is_empty() {
                // Once is enough to say it.
                told = true;
                println!("session · the compositor's threads run ahead of the programs (round robin, priority 1)");
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::turned_point;

    /// `transform` goes the way wlroots' and Hyprland's does, anticlockwise: with
    /// 1, what is at the top left of the upright monitor is at the bottom left of
    /// the real one, and with 3 at its top right. (A real buffer is `h` wide and
    /// `w` tall when the upright monitor is `w` wide and `h` tall.)
    #[test]
    fn turns_go_anticlockwise() {
        let (w, h) = (1920.0, 1080.0);
        assert_eq!(turned_point(0, (0.0, 0.0), (w, h)), (0.0, 0.0));
        assert_eq!(turned_point(1, (0.0, 0.0), (w, h)), (0.0, w), "1: top left to bottom left");
        assert_eq!(turned_point(1, (w, 0.0), (w, h)), (0.0, 0.0), "1: top right to top left");
        assert_eq!(turned_point(2, (0.0, 0.0), (w, h)), (w, h), "2: top left to bottom right");
        assert_eq!(turned_point(3, (0.0, 0.0), (w, h)), (h, 0.0), "3: top left to top right");
        assert_eq!(turned_point(3, (w, 0.0), (w, h)), (h, w), "3: top right to bottom right");
    }

    /// A quarter one way and a quarter the other bring a point home.
    #[test]
    fn opposite_turns_cancel() {
        let (w, h) = (1920.0, 1080.0);
        let p = (300.0, 200.0);
        let there = turned_point(1, p, (w, h));
        assert_eq!(turned_point(3, there, (h, w)), p);
        let there = turned_point(3, p, (w, h));
        assert_eq!(turned_point(1, there, (h, w)), p);
    }
}
