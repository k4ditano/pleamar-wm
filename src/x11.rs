//! XWayland: programs that only speak X11 (Steam, older games, xterm) open in
//! the scene's slots like any other window.
//!
//! XWayland is an X server that is itself a Wayland program of ours: each X11
//! window is also one of its surfaces here, so it is drawn, given the pointer
//! and the keyboard the same way. What X adds is what this module does: the
//! window manager's side (`X11Wm`) — which windows there are, their titles,
//! being told their size and where they are on the desktop — and the windows
//! that manage themselves (menus, tooltips: «override-redirect»), which are
//! drawn as menus of the window they belong to. Copying and pasting crosses
//! between X11 and Wayland both ways.

use super::*;
use smithay::wayland::selection::data_device::{clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection, set_data_device_selection};
use smithay::wayland::selection::primary_selection::{clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection, set_primary_selection};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, WmWindowType, X11Window, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};

/// How long XWayland waits for the card to be offered to programs before it
/// starts without it (it is usually there in a tenth of a second).
pub(super) const CARD_PATIENCE: Duration = Duration::from_secs(3);

/// How many lines of what XWayland says (its errors: glamor, EGL, the card)
/// go to the session's log, and how long each may be.
const XWAYLAND_LINES: usize = 200;
const XWAYLAND_LINE: usize = 300;

/// The monitor a rectangle of X11 covers whole (`x, y, w, h`), of those given
/// as `x, y, w, h` in units and `w, h` in pixels: of its size, at its corner,
/// or at 0,0 (an X11 window here keeps its own place on the desktop, and a
/// game going fullscreen asks for 0,0).
pub(super) fn covers(r: (i32, i32, i32, i32), monitors: &[(i32, i32, i32, i32, i32, i32)]) -> Option<usize> {
    let sized = |m: &(i32, i32, i32, i32, i32, i32)| (r.2, r.3) == (m.2, m.3) || (r.2, r.3) == (m.4, m.5);
    monitors.iter().position(|m| sized(m) && (r.0, r.1) == (m.0, m.1)).or_else(|| if (r.0, r.1) == (0, 0) { monitors.iter().position(sized) } else { None })
}

impl State {
    /// XWayland, once the card is offered to programs (see `x_deferred`);
    /// what is launched meanwhile waits for it, as while it starts.
    pub(super) fn want_xwayland(&mut self) {
        if std::env::var_os("PLEAMAR_WM_NO_X11").is_some() {
            return;
        }
        self.x_starting = true;
        self.x_deferred = Some(Instant::now());
    }

    /// XWayland started now, if it was waiting.
    pub(super) fn xwayland_due(&mut self, why: &str) {
        let Some(since) = self.x_deferred.take() else { return };
        println!("windows · XWayland starts after {} ms: {why}", since.elapsed().as_millis());
        self.start_xwayland();
    }

    /// The monitors as `covers` takes them.
    fn monitor_rects(&self) -> Vec<(i32, i32, i32, i32, i32, i32)> {
        (0..self.outputs.len())
            .map(|k| {
                let at = self.outputs[k].current_location();
                let (pw, ph) = self.monitor_pixels(k);
                let s = self.scale_of(k).max(0.1);
                (at.x, at.y, (pw as f64 / s).round() as i32, (ph as f64 / s).round() as i32, pw, ph)
            })
            .collect()
    }

    /// Whether an X11 window may be taken for fullscreen because it covers a
    /// monitor: a normal one (not a menu, dialog or splash, not maximized),
    /// and its program not ruled `fullscreen=never`.
    fn x11_may_cover(&self, x: &X11Surface) -> bool {
        !x.is_maximized()
            && x.is_transient_for().is_none()
            && !x.is_popup()
            && matches!(x.window_type(), None | Some(WmWindowType::Normal))
            && crate::config::get().for_window(&x.class(), &x.title()).fullscreen != Some(false)
    }

    /// Fullscreen as it enters its slot: `Some(false)` it is or asked to be
    /// (`_NET_WM_STATE`), `Some(true)` it covers a monitor without saying so.
    pub(super) fn x11_full_at_map(&mut self, x: &X11Surface) -> Option<bool> {
        let asked = self.x11_fs_asked.contains(x);
        self.x11_fs_asked.retain(|w| w != x);
        if asked || x.is_fullscreen() {
            return Some(false);
        }
        let g = x.geometry();
        (self.x11_may_cover(x) && covers((g.loc.x, g.loc.y, g.size.w, g.size.h), &self.monitor_rects()).is_some()).then_some(true)
    }

    /// A window in a slot asked for a size: the whole monitor (a Proton game
    /// going fullscreen without `_NET_WM_STATE`) makes it fullscreen; smaller,
    /// it is no longer, if it was only for that.
    fn x11_asked_size(&mut self, slot: usize, x: &X11Surface, r: (i32, i32, i32, i32)) {
        let Some(w) = self.slots[slot].as_ref() else { return };
        let (full, auto) = (w.fullscreen, w.auto_full);
        let covering = self.x11_may_cover(x) && covers(r, &self.monitor_rects()).is_some();
        if covering && !full {
            println!("windows · {} «{}» covers a monitor: fullscreen", x.class(), x.title());
            self.set_fullscreen(slot, true);
            if let Some(Some(w)) = self.slots.get_mut(slot) {
                w.auto_full = true;
            }
        } else if !covering && full && auto {
            println!("windows · {} «{}» no longer covers a monitor: not fullscreen", x.class(), x.title());
            self.set_fullscreen(slot, false);
        }
    }

    /// The monitors X11 windows that place themselves (override-redirect)
    /// cover whole: fullscreen there (bars step aside), and so the window
    /// they belong to for the scene (`win.$i.fullscreen`).
    fn x11_recheck_covering(&mut self) {
        let rects = self.monitor_rects();
        let now: Vec<(usize, Option<usize>)> = self
            .unmanaged
            .iter()
            .filter(|x| crate::config::get().for_window(&x.class(), &x.title()).fullscreen != Some(false))
            .filter_map(|x| {
                let g = x.geometry();
                covers((g.loc.x, g.loc.y, g.size.w, g.size.h), &rects).map(|m| (m, self.unmanaged_owner(x)))
            })
            .collect();
        if now == self.x11_or_full {
            return;
        }
        let owners = |v: &[(usize, Option<usize>)]| v.iter().filter_map(|(_, o)| *o).collect::<Vec<_>>();
        let (before, after) = (owners(&self.x11_or_full), owners(&now));
        self.x11_or_full = now;
        for (slot, yes) in before.iter().filter(|o| !after.contains(o)).map(|o| (*o, false)).chain(after.iter().filter(|o| !before.contains(o)).map(|o| (*o, true))) {
            // (One fullscreen of its own already says so.)
            if self.slots.get(slot).and_then(Option::as_ref).is_some_and(|w| !w.fullscreen) {
                self.tell(NestEvent::Fullscreen(slot, yes));
            }
        }
        self.tell_fullscreen();
    }

    /// XWayland started, if it is there and not turned off
    /// (`PLEAMAR_WM_NO_X11`). Programs started meanwhile wait for it (see
    /// `x_starting`), so that an X11 one in the autostart finds its display.
    pub(super) fn start_xwayland(&mut self) {
        if std::env::var_os("PLEAMAR_WM_NO_X11").is_some() {
            return;
        }
        // What it says goes to the log, a few lines: why it could not use the
        // card, if it could not, was nowhere to be read.
        let said: std::process::Stdio = match std::io::pipe() {
            Ok((reader, writer)) => {
                let _ = std::thread::Builder::new().name("xwayland-said".into()).spawn(move || {
                    use std::io::BufRead;
                    for (n, line) in std::io::BufReader::new(reader).lines().map_while(Result::ok).enumerate() {
                        if n == XWAYLAND_LINES {
                            println!("xwayland · (what else it says is not kept)");
                        }
                        if n >= XWAYLAND_LINES {
                            continue;
                        }
                        let cut = line.char_indices().nth(XWAYLAND_LINE).map_or(line.len(), |(i, _)| i);
                        println!("xwayland · {}", &line[..cut]);
                    }
                });
                writer.into()
            }
            Err(_) => std::process::Stdio::null(),
        };
        let spawned = XWayland::spawn(&self.dh, None, std::iter::empty::<(String, String)>(), true, std::process::Stdio::null(), said, |_| ());
        let (xwayland, client) = match spawned {
            Ok(x) => x,
            Err(e) => {
                eprintln!("windows · no XWayland ({e}): X11 programs will not open");
                return;
            }
        };
        self.x_starting = true;
        let started = self.handle.insert_source(xwayland, move |event, _, state: &mut State| {
            state.x_starting = false;
            match event {
                XWaylandEvent::Ready { x11_socket, display_number } => match X11Wm::start_wm(state.handle.clone(), x11_socket, client.clone()) {
                    Ok(wm) => {
                        state.xwm = Some(wm);
                        state.x_display = Some(format!(":{display_number}"));
                        pleamar::set_child_env("DISPLAY", Some(&format!(":{display_number}")));
                        println!("windows · X11 programs open here too (XWayland, DISPLAY=:{display_number})");
                        state.export_environment();
                    }
                    Err(e) => eprintln!("windows · XWayland started, but its windows cannot be managed: {e}"),
                },
                XWaylandEvent::Error => eprintln!("windows · XWayland did not start: X11 programs will not open"),
            }
        });
        if let Err(e) = started {
            self.x_starting = false;
            eprintln!("windows · XWayland could not be listened to: {e}");
        }
    }

    /// The slot of an X11 window, if it has one.
    pub(super) fn x11_slot(&self, x: &X11Surface) -> Option<usize> {
        self.slots.iter().position(|w| w.as_ref().is_some_and(|w| w.toplevel == Toplevel::X11(x.clone())))
    }

    /// An X11 window, mapped and with its surface: to a slot.
    fn manage(&mut self, x: X11Surface) {
        let Some(surface) = x.wl_surface() else {
            if !self.x11_pending.contains(&x) {
                self.x11_pending.push(x);
            }
            return;
        };
        if self.x11_slot(&x).is_some() || self.waiting.iter().any(|(t, _)| *t == Toplevel::X11(x.clone())) {
            return;
        }
        self.place(Toplevel::X11(x), surface);
    }

    /// Which window a menu or tooltip of X11 belongs to: the one it says it
    /// is for, or the X11 one it opened over, or the one with the keyboard.
    pub(super) fn unmanaged_owner(&self, x: &X11Surface) -> Option<usize> {
        let x11 = |k: usize| match self.slots.get(k).and_then(Option::as_ref).map(|w| &w.toplevel) {
            Some(Toplevel::X11(t)) => Some(t.clone()),
            _ => None,
        };
        let n = self.slots.len();
        if let Some(parent) = x.is_transient_for() {
            if let Some(k) = (0..n).find(|k| x11(*k).is_some_and(|t| t.window_id() == parent)) {
                return Some(k);
            }
        }
        let at = x.geometry().loc;
        let over = |k: &usize| x11(*k).is_some_and(|t| t.geometry().contains(at));
        self.focus.filter(over).or_else(|| self.order.iter().copied().find(over)).or(self.focus).or_else(|| (0..n).find(|k| x11(*k).is_some()))
    }

    /// The menus and tooltips of X11 that belong to that window, each with
    /// its surface and where it is from the window's corner.
    pub(super) fn unmanaged_of(&self, slot: usize) -> Vec<(WlSurface, (i32, i32))> {
        let Some(Some(Window { toplevel: Toplevel::X11(main), .. })) = self.slots.get(slot) else { return Vec::new() };
        let origin = main.geometry().loc;
        self.unmanaged
            .iter()
            .filter(|x| x.alive() && self.unmanaged_owner(x) == Some(slot))
            .filter_map(|x| {
                let g = x.geometry().loc;
                x.wl_surface().map(|s| (s, (g.x - origin.x, g.y - origin.y)))
            })
            .collect()
    }

    /// Where an X11 window is seen: X places its menus from there.
    pub(super) fn x11_shown_at(&self, slot: usize, desktop: (i32, i32)) {
        if let Some(Some(Window { toplevel: Toplevel::X11(x), .. })) = self.slots.get(slot) {
            let g = x.geometry();
            if (g.loc.x, g.loc.y) != desktop {
                let _ = x.configure(smithay::utils::Rectangle::new(desktop.into(), g.size));
            }
        }
    }

    fn dirty_owner_of(&mut self, x: &X11Surface) {
        if let Some(s) = self.unmanaged_owner(x).and_then(|k| self.slots[k].as_ref()).map(|w| w.surface.clone()) {
            self.dirty.push(s);
        }
    }

    fn let_go(&mut self, window: &X11Surface) {
        self.x11_pending.retain(|w| w != window);
        self.x11_fs_asked.retain(|w| w != window);
        if window.is_override_redirect() {
            self.dirty_owner_of(window);
            self.unmanaged.retain(|w| w != window);
            self.x11_recheck_covering();
        } else {
            self.forget(&Toplevel::X11(window.clone()));
        }
    }
}

impl XwmHandler for State {
    fn xwm_state(&mut self, _: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("an X11 event without its window manager")
    }

    fn new_window(&mut self, _: XwmId, _: X11Surface) {}

    fn new_override_redirect_window(&mut self, _: XwmId, _: X11Surface) {}

    fn map_window_request(&mut self, _: XwmId, window: X11Surface) {
        if window.set_mapped(true).is_ok() {
            self.manage(window);
        }
    }

    fn mapped_override_redirect_window(&mut self, _: XwmId, window: X11Surface) {
        if !self.unmanaged.contains(&window) {
            self.unmanaged.push(window.clone());
        }
        self.dirty_owner_of(&window);
        self.x11_recheck_covering();
    }

    fn unmapped_window(&mut self, _: XwmId, window: X11Surface) {
        self.let_go(&window);
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _: XwmId, window: X11Surface) {
        self.let_go(&window);
    }

    /// A window in a slot has the size the scene gives it; one not yet in
    /// a slot, the one it asks for.
    fn configure_request(&mut self, _: XwmId, window: X11Surface, x: Option<i32>, y: Option<i32>, w: Option<u32>, h: Option<u32>, _: Option<Reorder>) {
        let mut g = window.geometry();
        if let (Some(slot), Some(rw), Some(rh)) = (self.x11_slot(&window), w, h) {
            self.x11_asked_size(slot, &window, (x.unwrap_or(g.loc.x), y.unwrap_or(g.loc.y), rw as i32, rh as i32));
        }
        if self.x11_slot(&window).is_none() {
            if let Some(x) = x {
                g.loc.x = x;
            }
            if let Some(y) = y {
                g.loc.y = y;
            }
            if let Some(w) = w {
                g.size.w = w as i32;
            }
            if let Some(h) = h {
                g.size.h = h as i32;
            }
        }
        let _ = window.configure(g);
    }

    fn configure_notify(&mut self, _: XwmId, window: X11Surface, _: smithay::utils::Rectangle<i32, Logical>, _: Option<X11Window>) {
        if window.is_override_redirect() {
            self.dirty_owner_of(&window);
            self.x11_recheck_covering();
        }
    }

    fn property_notify(&mut self, _: XwmId, window: X11Surface, property: WmWindowProperty) {
        let Some(slot) = self.x11_slot(&window) else { return };
        match property {
            WmWindowProperty::Title => self.set_title(slot, window.title()),
            WmWindowProperty::Class => self.set_app(slot, window.class()),
            _ => {}
        }
    }

    fn fullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        if let Some(slot) = self.x11_slot(&window) {
            self.set_fullscreen(slot, true);
        } else if !self.x11_fs_asked.contains(&window) {
            // Asked before it has a slot: it gets it when it has one.
            self.x11_fs_asked.push(window);
        }
    }

    fn minimize_request(&mut self, _: XwmId, window: X11Surface) {
        if let Some(slot) = self.x11_slot(&window) {
            self.asked("window_minimize", slot);
        }
    }

    fn maximize_request(&mut self, _: XwmId, window: X11Surface) {
        if let Some(slot) = self.x11_slot(&window) {
            self.asked("window_maximize", slot);
        }
    }

    fn unmaximize_request(&mut self, xwm: XwmId, window: X11Surface) {
        XwmHandler::maximize_request(self, xwm, window);
    }

    fn unminimize_request(&mut self, _: XwmId, window: X11Surface) {
        if let Some(slot) = self.x11_slot(&window) {
            self.set_minimized(slot, false);
        }
    }

    fn unfullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        if let Some(slot) = self.x11_slot(&window) {
            self.set_fullscreen(slot, false);
        } else {
            self.x11_fs_asked.retain(|w| w != &window);
        }
    }

    // Where a window goes and how big is the scene's: moving or resizing it
    // from its own frame is asked of the scene (`win.$i.held`), which may do it.
    fn resize_request(&mut self, _: XwmId, window: X11Surface, _: u32, edges: ResizeEdge) {
        if let Some(slot) = self.x11_slot(&window) {
            // As xdg-shell counts them: 1 top, 2 bottom, 4 left, 8 right.
            let bits = match edges {
                ResizeEdge::Top => 1,
                ResizeEdge::Bottom => 2,
                ResizeEdge::Left => 4,
                ResizeEdge::TopLeft => 5,
                ResizeEdge::BottomLeft => 6,
                ResizeEdge::Right => 8,
                ResizeEdge::TopRight => 9,
                ResizeEdge::BottomRight => 10,
            };
            self.hold(slot, 2, bits);
        }
    }

    fn move_request(&mut self, _: XwmId, window: X11Surface, _: u32) {
        if let Some(slot) = self.x11_slot(&window) {
            self.hold(slot, 1, 0);
        }
    }

    fn allow_selection_access(&mut self, _: XwmId, _: SelectionTarget) -> bool {
        true
    }

    /// An X11 program pastes what a Wayland one copied.
    fn send_selection(&mut self, _: XwmId, selection: SelectionTarget, mime_type: String, fd: std::os::fd::OwnedFd) {
        let seat = self.seat.clone();
        // Something kept here (given back after the agent pasted): from here.
        if matches!(selection, SelectionTarget::Clipboard) {
            let kept = current_data_device_selection_userdata(&seat).and_then(|c| match &*c {
                super::Copied::Kept(k) => Some(k.clone()),
                super::Copied::X11 => None,
            });
            if let Some(kept) = kept {
                super::serve_kept(&kept, &mime_type, fd);
                return;
            }
        }
        let sent = match selection {
            SelectionTarget::Clipboard => request_data_device_client_selection(&seat, mime_type, fd).map_err(|e| e.to_string()),
            SelectionTarget::Primary => request_primary_client_selection(&seat, mime_type, fd).map_err(|e| e.to_string()),
        };
        if let Err(e) = sent {
            eprintln!("windows · what was copied could not reach the X11 program: {e}");
        }
    }

    /// An X11 program copied something: the Wayland ones can paste it.
    fn new_selection(&mut self, _: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        let seat = self.seat.clone();
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.dh, &seat, mime_types, super::Copied::X11);
                self.clipboard = super::Clipboard::X11;
            }
            SelectionTarget::Primary => set_primary_selection(&self.dh, &seat, mime_types, super::Copied::X11),
        }
    }

    fn cleared_selection(&mut self, _: XwmId, selection: SelectionTarget) {
        let seat = self.seat.clone();
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&seat).is_some_and(|c| matches!(*c, super::Copied::X11)) {
                    clear_data_device_selection(&self.dh, &seat);
                    self.clipboard = super::Clipboard::Nothing;
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&seat).is_some() {
                    clear_primary_selection(&self.dh, &seat);
                }
            }
        }
    }

    fn disconnected(&mut self, _: XwmId) {
        println!("windows · XWayland is gone");
        self.xwm = None;
        self.x_display = None;
        pleamar::set_child_env("DISPLAY", None);
    }
}

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell
    }

    /// An X11 window has its surface now: if it was waiting for it, to its slot.
    fn surface_associated(&mut self, _: XwmId, _: WlSurface, window: X11Surface) {
        if let Some(k) = self.x11_pending.iter().position(|w| w == &window) {
            let w = self.x11_pending.remove(k);
            self.manage(w);
        } else if window.is_override_redirect() {
            self.dirty_owner_of(&window);
        }
    }
}

/// What a Wayland program copies reaches the X11 ones.
pub(super) fn wayland_copied(state: &mut State, ty: SelectionTarget, mime_types: Option<Vec<String>>) {
    if let Some(wm) = state.xwm.as_mut() {
        if let Err(e) = wm.new_selection(ty, mime_types) {
            eprintln!("windows · what was copied could not be offered to X11: {e}");
        }
    }
}

/// A Wayland program pastes what an X11 one copied.
pub(super) fn wayland_pastes(state: &mut State, ty: SelectionTarget, mime_type: String, fd: std::os::fd::OwnedFd) {
    let handle = state.handle.clone();
    if let Some(wm) = state.xwm.as_mut() {
        if let Err(e) = wm.send_selection(ty, mime_type, fd, handle) {
            eprintln!("windows · what X11 copied could not be pasted: {e}");
        }
    }
}

smithay::delegate_xwayland_shell!(State);

#[cfg(test)]
mod tests {
    use super::covers;

    // One 1920×1080 monitor at 0,0 and a 2560×1440 one at scale 2 to its right.
    const MONITORS: [(i32, i32, i32, i32, i32, i32); 2] = [(0, 0, 1920, 1080, 1920, 1080), (1920, 0, 1280, 720, 2560, 1440)];

    #[test]
    fn a_window_covering_a_monitor_is_told_from_a_big_one() {
        assert_eq!(covers((0, 0, 1920, 1080), &MONITORS), Some(0), "a game at 0,0 the size of the monitor");
        assert_eq!(covers((1920, 0, 2560, 1440), &MONITORS), Some(1), "in pixels, at its corner");
        assert_eq!(covers((1920, 0, 1280, 720), &MONITORS), Some(1), "in units");
        assert_eq!(covers((0, 0, 1280, 720), &MONITORS), Some(1), "at 0,0, the size of another monitor");
        assert_eq!(covers((0, 0, 1904, 929), &MONITORS), None, "a window, however big");
        assert_eq!(covers((0, 32, 1920, 1048), &MONITORS), None, "maximized under a bar");
        assert_eq!(covers((100, 0, 1920, 1080), &MONITORS), None, "the size, somewhere else");
        assert_eq!(covers((0, 0, 1920, 1080), &[]), None);
    }
}
