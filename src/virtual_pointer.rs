//! `wlr-virtual-pointer`: a program that moves the pointer and presses its
//! buttons as a mouse would (`wlrctl pointer`, `ydotool` through its wayland
//! backend, `wayvnc`, a remote-desktop server, a test harness). Open to every
//! program, as sway's is and as the virtual keyboard already is here: whoever
//! can talk to the compositor can type for the person, and so may click.
//!
//! What the protocol says is put together here and handed on as
//! `layers::VirtualPointer`; the session (session.rs) or the screenless one
//! (headless.rs) takes it as if libinput had said it, so it goes the road a
//! mouse's does (`route.rs`): to the scene, to a program's surface, to a
//! window. Motion and buttons go at once; the wheel's events are kept until
//! the `frame` that closes them, as the protocol asks.
//!
//! It costs nothing until a program binds the global and sends something:
//! the global is advertised and nothing runs.

use super::State;
use crate::layers::{self, VirtualAxis, VirtualAxisSource, VirtualPointer};
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::server::{
    zwlr_virtual_pointer_manager_v1::{self as manager, ZwlrVirtualPointerManagerV1},
    zwlr_virtual_pointer_v1::{self as pointer, ZwlrVirtualPointerV1},
};
use smithay::reexports::wayland_server::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum};
use std::sync::Mutex;

/// The version offered: 2, with the pointer tied to an output.
const VERSION: u32 = 2;

pub fn global(dh: &DisplayHandle) {
    dh.create_global::<State, ZwlrVirtualPointerManagerV1, _>(VERSION, ());
}

/// One virtual pointer: the monitor its absolute motion is over (none: the
/// whole desktop), and the wheel's events since the last `frame`.
#[derive(Default)]
pub struct Virtual {
    output: Option<String>,
    source: Option<VirtualAxisSource>,
    vertical: VirtualAxis,
    horizontal: VirtualAxis,
    axis: bool,
}

/// Which of the two axes, or none: a program naming another is wrong.
fn which(axis: WEnum<Axis>) -> Option<Axis> {
    match axis.into_result() {
        Ok(a @ (Axis::VerticalScroll | Axis::HorizontalScroll)) => Some(a),
        _ => None,
    }
}

impl Virtual {
    fn axis_mut(&mut self, axis: Axis) -> &mut VirtualAxis {
        self.axis = true;
        if axis == Axis::VerticalScroll {
            &mut self.vertical
        } else {
            &mut self.horizontal
        }
    }

    /// `frame`: what the wheel did, as one event; nothing if it did nothing.
    fn frame(&mut self) -> Option<VirtualPointer> {
        if !std::mem::take(&mut self.axis) {
            return None;
        }
        let (vertical, horizontal) = (std::mem::take(&mut self.vertical), std::mem::take(&mut self.horizontal));
        // With no source said, a wheel: what a program sending notches means.
        let source = self.source.take().unwrap_or(if vertical.v120.is_some() || horizontal.v120.is_some() { VirtualAxisSource::Wheel } else { VirtualAxisSource::Continuous });
        Some(VirtualPointer::Axis { source, vertical, horizontal })
    }
}

impl GlobalDispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrVirtualPointerManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn request(state: &mut Self, _: &Client, _: &ZwlrVirtualPointerManagerV1, request: manager::Request, _: &(), _: &DisplayHandle, init: &mut DataInit<'_, Self>) {
        match request {
            // The seat is not looked at: there is the person's pointer, and this drives it.
            manager::Request::CreateVirtualPointer { id, .. } => {
                init.init(id, Mutex::new(Virtual::default()));
            }
            manager::Request::CreateVirtualPointerWithOutput { output, id, .. } => {
                let output = output.and_then(|o| state.outputs.iter().find(|x| x.owns(&o)).map(|x| x.name().to_owned()));
                init.init(id, Mutex::new(Virtual { output, ..Default::default() }));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrVirtualPointerV1, Mutex<Virtual>> for State {
    fn request(_: &mut Self, _: &Client, resource: &ZwlrVirtualPointerV1, request: pointer::Request, data: &Mutex<Virtual>, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        let mut v = data.lock().unwrap();
        let told = match request {
            pointer::Request::Motion { time, dx, dy } => Some(VirtualPointer::Motion { dx, dy, time }),
            pointer::Request::MotionAbsolute { x, y, x_extent, y_extent, .. } => {
                // An extent of nothing says nowhere: the position is not worked out.
                (x_extent > 0 && y_extent > 0).then(|| VirtualPointer::Absolute { x: (x as f64 / x_extent as f64).clamp(0.0, 1.0), y: (y as f64 / y_extent as f64).clamp(0.0, 1.0), output: v.output.clone() })
            }
            pointer::Request::Button { button, state, .. } => match state.into_result() {
                Ok(s) => Some(VirtualPointer::Button { code: button, down: s == ButtonState::Pressed }),
                Err(_) => None,
            },
            pointer::Request::Axis { axis, value, .. } => {
                match which(axis) {
                    Some(a) => v.axis_mut(a).amount = Some(value),
                    None => resource.post_error(pointer::Error::InvalidAxis, "an axis is vertical or horizontal"),
                }
                None
            }
            pointer::Request::AxisDiscrete { axis, value, discrete, .. } => {
                match which(axis) {
                    // A step is 120 of the wheel's, as libinput says them.
                    Some(a) => {
                        let a = v.axis_mut(a);
                        a.amount = Some(value);
                        a.v120 = Some(discrete as f64 * 120.0);
                    }
                    None => resource.post_error(pointer::Error::InvalidAxis, "an axis is vertical or horizontal"),
                }
                None
            }
            pointer::Request::AxisStop { axis, .. } => {
                match which(axis) {
                    Some(a) => v.axis_mut(a).stop = true,
                    None => resource.post_error(pointer::Error::InvalidAxis, "an axis is vertical or horizontal"),
                }
                None
            }
            pointer::Request::AxisSource { axis_source } => {
                match axis_source.into_result() {
                    Ok(AxisSource::Wheel) => v.source = Some(VirtualAxisSource::Wheel),
                    Ok(AxisSource::Finger) => v.source = Some(VirtualAxisSource::Finger),
                    Ok(AxisSource::Continuous) => v.source = Some(VirtualAxisSource::Continuous),
                    Ok(AxisSource::WheelTilt) => v.source = Some(VirtualAxisSource::WheelTilt),
                    _ => resource.post_error(pointer::Error::InvalidAxisSource, "that is not an axis source"),
                }
                None
            }
            pointer::Request::Frame => v.frame(),
            _ => None,
        };
        drop(v);
        if let Some(event) = told {
            layers::virtual_pointer(event);
        }
    }
}
