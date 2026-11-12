//! `wlr-output-management`: the monitors for a program that configures them
//! (`wlr-randr`, `kanshi`, `wdisplays`, `nwg-displays`) —each with its modes,
//! the one it is at, where it is, its scale, how it is turned and its
//! variable refresh— and what it asks done as `pleamar-wm output` does it
//! (output.rs): every monitor of the configuration checked before any is
//! changed, all of it or none. A new mode, scale or turn puts the monitor up
//! again with them; where it is and its variable refresh change in place.
//!
//! What is not done, answered as failed: turning a monitor off from here (a
//! `monitor NAME off` line in session.conf does it), a mode the monitor does
//! not offer (`set_custom_mode`), and a mirrored turn (`flipped`).

use super::State;
use crate::config::ModeWish;
use crate::layers::{self, MonitorInfo};
use crate::output::Wish;
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::{
    zwlr_output_configuration_head_v1::{self as config_head, ZwlrOutputConfigurationHeadV1},
    zwlr_output_configuration_v1::{self as configuration, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self as head, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self as manager, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self as mode, ZwlrOutputModeV1},
};
use smithay::reexports::wayland_server::protocol::wl_output::Transform;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum};
use std::sync::Mutex;

/// The version offered: 4, with variable refresh.
const VERSION: u32 = 4;

pub fn global(dh: &DisplayHandle) {
    dh.create_global::<State, ZwlrOutputManagerV1, _>(VERSION, ());
}

/// A mode as told: width and height in pixels, refresh in mHz.
type ModeOf = (u32, u32, i32);

/// One monitor as told to one program: its head, its modes, and what it was told last.
struct Told {
    name: String,
    head: ZwlrOutputHeadV1,
    modes: Vec<(ZwlrOutputModeV1, ModeOf)>,
    last: MonitorInfo,
}

/// Every program that watches the monitors, and the serial of what they were told last.
#[derive(Default)]
pub struct Managers {
    serial: u32,
    all: Vec<(ZwlrOutputManagerV1, Vec<Told>)>,
}

/// The monitors to tell: all but the phone's, which `pleamar-wm remote` places.
fn shown() -> Vec<MonitorInfo> {
    layers::monitors().into_iter().filter(|m| m.name != layers::PHONE_NAME).collect()
}

/// Its modes, and the one it is at. A monitor that says none (a headless
/// session's) has the one it is at, as it stands.
fn modes_of(m: &MonitorInfo) -> (Vec<(u32, u32, i32, bool)>, Option<ModeOf>) {
    if m.modes.is_empty() {
        let unturned = if m.turn % 2 == 1 { (m.size.1, m.size.0) } else { m.size };
        return (vec![(unturned.0, unturned.1, m.mhz, true)], Some((unturned.0, unturned.1, m.mhz)));
    }
    (m.modes.clone(), m.mode)
}

fn transform_of(turn: u8) -> Transform {
    match turn % 4 {
        1 => Transform::_90,
        2 => Transform::_180,
        3 => Transform::_270,
        _ => Transform::Normal,
    }
}

impl Managers {
    /// The monitors changed (or not, but something of one did): each program
    /// told what is new, then `done` with a new serial. A configuration made
    /// before this is cancelled when it is applied.
    pub fn update(&mut self, dh: &DisplayHandle) {
        let now = shown();
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        self.all.retain(|(m, _)| m.is_alive());
        for (manager, told) in &mut self.all {
            let Some(client) = manager.client() else { continue };
            told.retain(|t| {
                let stays = now.iter().any(|m| m.name == t.name);
                if !stays {
                    for (mode, _) in &t.modes {
                        mode.finished();
                    }
                    t.head.finished();
                }
                stays
            });
            for m in &now {
                match told.iter_mut().find(|t| t.name == m.name) {
                    Some(t) => tell_changes(dh, &client, t, m),
                    None => {
                        if let Some(t) = announce(dh, &client, manager, m) {
                            told.push(t);
                        }
                    }
                }
            }
            manager.done(serial);
        }
    }

    fn bound(&mut self, dh: &DisplayHandle, manager: ZwlrOutputManagerV1) {
        let Some(client) = manager.client() else { return };
        let told: Vec<Told> = shown().iter().filter_map(|m| announce(dh, &client, &manager, m)).collect();
        manager.done(self.serial);
        self.all.push((manager, told));
    }
}

/// A new head for a monitor, with everything about it.
fn announce(dh: &DisplayHandle, client: &Client, manager: &ZwlrOutputManagerV1, m: &MonitorInfo) -> Option<Told> {
    let head = client.create_resource::<ZwlrOutputHeadV1, String, State>(dh, manager.version(), m.name.clone()).ok()?;
    manager.head(&head);
    head.name(m.name.clone());
    head.description(m.name.clone());
    if m.mm.0 > 0 && m.mm.1 > 0 {
        head.physical_size(m.mm.0 as i32, m.mm.1 as i32);
    }
    let mut t = Told { name: m.name.clone(), head, modes: Vec::new(), last: m.clone() };
    tell_modes(dh, client, &mut t, m);
    t.head.enabled(1);
    tell_place(&t, m, None);
    Some(t)
}

/// Its modes as they are now, each a new object.
fn tell_modes(dh: &DisplayHandle, client: &Client, t: &mut Told, m: &MonitorInfo) {
    let (modes, _) = modes_of(m);
    for (w, h, mhz, preferred) in modes {
        let Ok(mode) = client.create_resource::<ZwlrOutputModeV1, ModeOf, State>(dh, t.head.version().min(3), (w, h, mhz)) else { continue };
        t.head.mode(&mode);
        mode.size(w as i32, h as i32);
        if mhz > 0 {
            mode.refresh(mhz);
        }
        if preferred {
            mode.preferred();
        }
        t.modes.push((mode, (w, h, mhz)));
    }
}

/// The mode it is at, where it is, its scale, turn and variable refresh:
/// all of it the first time (`before` none), then only what changed.
fn tell_place(t: &Told, m: &MonitorInfo, before: Option<&MonitorInfo>) {
    let changed = |f: &dyn Fn(&MonitorInfo) -> bool| before.is_none_or(|b| f(b) != f(m));
    let (_, current) = modes_of(m);
    let mode_moved = before.is_none_or(|b| modes_of(b).1 != current);
    if mode_moved && let Some(c) = current && let Some((mode, _)) = t.modes.iter().find(|(_, of)| *of == c) {
        t.head.current_mode(mode);
    }
    if before.is_none_or(|b| (b.x, b.y) != (m.x, m.y)) {
        t.head.position(m.x, m.y);
    }
    if before.is_none_or(|b| b.turn != m.turn) {
        t.head.transform(transform_of(m.turn));
    }
    if before.is_none_or(|b| (b.scale - m.scale).abs() > 1e-9) {
        t.head.scale(m.scale);
    }
    if t.head.version() >= 4 && changed(&|i: &MonitorInfo| i.vrr) {
        t.head.adaptive_sync(if m.vrr { head::AdaptiveSyncState::Enabled } else { head::AdaptiveSyncState::Disabled });
    }
}

fn tell_changes(dh: &DisplayHandle, client: &Client, t: &mut Told, m: &MonitorInfo) {
    let before = std::mem::replace(&mut t.last, m.clone());
    if modes_of(&before).0 != modes_of(m).0 {
        for (mode, _) in t.modes.drain(..) {
            mode.finished();
        }
        tell_modes(dh, client, t, m);
        // New mode objects: the one it is at, and the rest, said again.
        tell_place(t, m, None);
        return;
    }
    tell_place(t, m, Some(&before));
}

/// A configuration being made: the serial it was made against, whether it
/// was applied or tested already, and each monitor's part, by name (`None`:
/// to be turned off).
struct Pending {
    serial: u32,
    used: bool,
    heads: Vec<(String, Option<ZwlrOutputConfigurationHeadV1>)>,
}

/// What a configuration asks of one monitor.
#[derive(Default)]
struct HeadWish {
    mode: Option<ModeOf>,
    custom: Option<ModeOf>,
    at: Option<(i32, i32)>,
    transform: Option<WEnum<Transform>>,
    scale: Option<f64>,
    vrr: Option<bool>,
}

impl GlobalDispatch<ZwlrOutputManagerV1, ()> for State {
    fn bind(state: &mut Self, dh: &DisplayHandle, _: &Client, resource: New<ZwlrOutputManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        let manager = init.init(resource, ());
        state.output_managers.bound(dh, manager);
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn request(state: &mut Self, _: &Client, manager: &ZwlrOutputManagerV1, request: manager::Request, _: &(), _: &DisplayHandle, init: &mut DataInit<'_, Self>) {
        match request {
            manager::Request::CreateConfiguration { id, serial } => {
                init.init(id, Mutex::new(Pending { serial, used: false, heads: Vec::new() }));
            }
            manager::Request::Stop => {
                manager.finished();
                state.output_managers.all.retain(|(m, _)| m != manager);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputHeadV1, String> for State {
    fn request(_: &mut Self, _: &Client, _: &ZwlrOutputHeadV1, _: head::Request, _: &String, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        // Only `release`: nothing kept is the program's.
    }
}

impl Dispatch<ZwlrOutputModeV1, ModeOf> for State {
    fn request(_: &mut Self, _: &Client, _: &ZwlrOutputModeV1, _: mode::Request, _: &ModeOf, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
}

impl Dispatch<ZwlrOutputConfigurationV1, Mutex<Pending>> for State {
    fn request(state: &mut Self, _: &Client, conf: &ZwlrOutputConfigurationV1, request: configuration::Request, pending: &Mutex<Pending>, dh: &DisplayHandle, init: &mut DataInit<'_, Self>) {
        let mut p = pending.lock().unwrap();
        match request {
            configuration::Request::EnableHead { id, head } => {
                let name = head.data::<String>().cloned().unwrap_or_default();
                let made = init.init(id, Mutex::new(HeadWish::default()));
                if p.heads.iter().any(|(n, _)| *n == name) {
                    conf.post_error(configuration::Error::AlreadyConfiguredHead, format!("{name} is in this configuration already"));
                    return;
                }
                p.heads.push((name, Some(made)));
            }
            configuration::Request::DisableHead { head } => {
                let name = head.data::<String>().cloned().unwrap_or_default();
                if p.heads.iter().any(|(n, _)| *n == name) {
                    conf.post_error(configuration::Error::AlreadyConfiguredHead, format!("{name} is in this configuration already"));
                    return;
                }
                p.heads.push((name, None));
            }
            configuration::Request::Apply => finish(state, dh, conf, &mut p, false),
            configuration::Request::Test => finish(state, dh, conf, &mut p, true),
            _ => {}
        }
    }
}

/// A configuration applied or tested: cancelled if the monitors changed since
/// it was made; failed if any part cannot be done; else done, or seen to be doable.
fn finish(state: &mut State, dh: &DisplayHandle, conf: &ZwlrOutputConfigurationV1, p: &mut Pending, only_check: bool) {
    if std::mem::replace(&mut p.used, true) {
        conf.post_error(configuration::Error::AlreadyUsed, "a configuration is applied or tested once");
        return;
    }
    if p.serial != state.output_managers.serial {
        conf.cancelled();
        return;
    }
    let now = shown();
    if let Some(m) = now.iter().find(|m| !p.heads.iter().any(|(n, _)| *n == m.name)) {
        conf.post_error(configuration::Error::UnconfiguredHead, format!("{} is not in the configuration", m.name));
        return;
    }
    match wishes(&p.heads) {
        Ok(w) => match crate::output::apply_many(w, only_check) {
            Ok(done) => {
                if !only_check && !done.is_empty() {
                    println!("windows · a program configured the monitors: {done}");
                }
                conf.succeeded();
                // What was done, said now with a new serial, as the protocol asks
                // after `succeeded`: a configuration made before it is cancelled.
                // Not left to the monitors' own notice, which comes later or (only
                // variable refresh, or a place already the same) not at all: a
                // program that started meanwhile still saw the old serial.
                if !only_check {
                    state.output_managers.update(dh);
                }
            }
            Err(why) => {
                println!("windows · a program's monitor configuration was refused: {why}");
                conf.failed();
            }
        },
        Err(why) => {
            println!("windows · a program's monitor configuration was refused: {why}");
            conf.failed();
        }
    }
}

/// What each monitor of the configuration is asked, as `pleamar-wm output` would ask it.
fn wishes(heads: &[(String, Option<ZwlrOutputConfigurationHeadV1>)]) -> Result<Vec<(String, Wish)>, String> {
    let mut out = Vec::new();
    for (name, part) in heads {
        let Some(part) = part else {
            return Err(format!("{name}: a monitor is not turned off from here (a `monitor {name} off` line in session.conf)"));
        };
        let Some(w) = part.data::<Mutex<HeadWish>>() else { continue };
        let w = w.lock().unwrap();
        if let Some((cw, ch, mhz)) = w.custom {
            return Err(format!("{name}: only the modes it offers, not {cw}x{ch}@{:.3}", mhz as f64 / 1000.0));
        }
        let transform = match w.transform.map(|t| t.into_result()) {
            None => None,
            Some(Ok(Transform::Normal)) => Some(0),
            Some(Ok(Transform::_90)) => Some(1),
            Some(Ok(Transform::_180)) => Some(2),
            Some(Ok(Transform::_270)) => Some(3),
            Some(_) => return Err(format!("{name}: a mirrored turn is not done")),
        };
        let mode = w.mode.map(|(mw, mh, mhz)| ModeWish::Exact(mw, mh, mhz as f64 / 1000.0));
        out.push((name.clone(), Wish { mode, scale: w.scale, transform, vrr: w.vrr, at: w.at }));
    }
    Ok(out)
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, Mutex<HeadWish>> for State {
    fn request(_: &mut Self, _: &Client, part: &ZwlrOutputConfigurationHeadV1, request: config_head::Request, wish: &Mutex<HeadWish>, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        let mut w = wish.lock().unwrap();
        let twice = |part: &ZwlrOutputConfigurationHeadV1, what: &str| part.post_error(config_head::Error::AlreadySet, format!("its {what} is set already"));
        match request {
            config_head::Request::SetMode { mode } => {
                if w.mode.is_some() || w.custom.is_some() {
                    return twice(part, "mode");
                }
                match mode.data::<ModeOf>() {
                    Some(of) => w.mode = Some(*of),
                    None => part.post_error(config_head::Error::InvalidMode, "not one of its modes"),
                }
            }
            config_head::Request::SetCustomMode { width, height, refresh } => {
                if w.mode.is_some() || w.custom.is_some() {
                    return twice(part, "mode");
                }
                if width <= 0 || height <= 0 || refresh < 0 {
                    return part.post_error(config_head::Error::InvalidCustomMode, "a mode of no size");
                }
                w.custom = Some((width as u32, height as u32, refresh));
            }
            config_head::Request::SetPosition { x, y } => {
                if w.at.replace((x, y)).is_some() {
                    twice(part, "position");
                }
            }
            config_head::Request::SetTransform { transform } => {
                if w.transform.replace(transform).is_some() {
                    twice(part, "transform");
                }
            }
            config_head::Request::SetScale { scale } => {
                if !(scale > 0.0) {
                    return part.post_error(config_head::Error::InvalidScale, "a scale has to be more than 0");
                }
                if w.scale.replace(scale).is_some() {
                    twice(part, "scale");
                }
            }
            config_head::Request::SetAdaptiveSync { state } => {
                let on = match state.into_result() {
                    Ok(head::AdaptiveSyncState::Enabled) => true,
                    Ok(head::AdaptiveSyncState::Disabled) => false,
                    _ => return part.post_error(config_head::Error::InvalidAdaptiveSyncState, "on or off"),
                };
                if w.vrr.replace(on).is_some() {
                    twice(part, "variable refresh");
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(turn: u8) -> MonitorInfo {
        MonitorInfo { name: "HEADLESS-1".into(), size: (1080, 1920), x: 0, y: 0, mhz: 60_000, scale: 1.0, turn, ..Default::default() }
    }

    /// A monitor that says no modes (a headless one) has the one it is at,
    /// unturned: a monitor on its side is wider than tall in its mode.
    #[test]
    fn a_monitor_without_modes_has_the_one_it_is_at() {
        let (modes, current) = modes_of(&info(1));
        assert_eq!(modes, vec![(1920, 1080, 60_000, true)]);
        assert_eq!(current, Some((1920, 1080, 60_000)));
    }

    #[test]
    fn quarter_turns_are_wl_output_transforms() {
        assert_eq!([0, 1, 2, 3].map(transform_of), [Transform::Normal, Transform::_90, Transform::_180, Transform::_270]);
    }
}
