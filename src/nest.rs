//! A Wayland compositor inside the scene: `windows win max 6`.
//!
//! Other programs —a terminal, a browser— connect to it as they would to
//! Hyprland, and what they draw reaches the render as images, one per window.
//! Where each one goes, how big, how it arrives and how it leaves is the
//! scene's: here there is no layout, no animation and no decoration, only the
//! protocol. That is the point: the window manager is a `.plm` file, with
//! springs, rules and zones, and it reloads on save while the programs in it
//! keep running.
//!
//! It runs on its own thread with its own event loop (calloop), so a client
//! that floods it or stalls does not reach the render; the render does not
//! wait for it either, it paints the last image it has.
//!
//! In a session of its own it is also the desktop's compositor: other
//! programs' layer-shell surfaces (the wallpaper, Marea) go to the monitors
//! (see `layers`), and the windows are listed with wlr-foreign-toplevel.
//!
//! What it does not do yet: XWayland, and more than one scale.

#[path = "x11.rs"]
mod x11;
#[path = "capture.rs"]
mod capture;

use crate::layers::{self, ClientLayer, ClientPiece, ToLayers};
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::xwayland::{X11Surface, X11Wm, XWaylandClientData};
use pleamar::scene::{DmabufPiece, NestEvent, PieceContent, ToNest, ToRender, WindowPicture, WindowPiece};
use smithay::delegate_layer_shell;
use smithay::input::pointer::RelativeMotionEvent;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::compositor::send_surface_state;
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleHandler, FractionalScaleManagerState};
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::viewporter::{ViewportCachedState, ViewporterState};
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState};
use smithay::wayland::idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState};
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};
use smithay::wayland::input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface as ImePopup};
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint, PointerConstraintsHandler, PointerConstraintsState};
use smithay::wayland::presentation::{PresentationFeedbackCachedState, PresentationFeedbackCallback, PresentationState, Refresh};
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::ext_data_control::{DataControlHandler as ExtDataControlHandler, DataControlState as ExtDataControlState};
use smithay::wayland::selection::primary_selection::{set_primary_focus, PrimarySelectionHandler, PrimarySelectionState};
use smithay::wayland::selection::wlr_data_control::{DataControlHandler as WlrDataControlHandler, DataControlState as WlrDataControlState};
use smithay::wayland::shell::xdg::dialog::{XdgDialogHandler, XdgDialogState};
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};
use smithay::wayland::xdg_foreign::{XdgForeignHandler, XdgForeignState};
use smithay::delegate_session_lock;
use smithay::delegate_drm_syncobj;
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncPoint, DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState};
use smithay::wayland::session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker};
use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1::{self as toplevel_handle, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self as toplevel_manager, ZwlrForeignToplevelManagerV1},
};
use smithay::reexports::wayland_server::{DataInit, Dispatch, GlobalDispatch, New};
use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::{self as power_manager, ZwlrOutputPowerManagerV1},
    zwlr_output_power_v1::{self as output_power, ZwlrOutputPowerV1},
};
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self as copy_frame, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self as copy_manager, ZwlrScreencopyManagerV1},
};
use smithay::wayland::shm::with_buffer_contents_mut;
use smithay::reexports::wayland_protocols::ext::background_effect::v1::server::{
    ext_background_effect_manager_v1::{self as effect_manager, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::{self as effect_surface, ExtBackgroundEffectSurfaceV1},
};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::compositor::RectangleKind;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::shell::wlr_layer::{Anchor, ExclusiveZone, KeyboardInteractivity, Layer as ShellLayer, LayerSurface, LayerSurfaceCachedState, WlrLayerShellHandler, WlrLayerShellState};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer, Format, Fourcc, Modifier};
use smithay::delegate_dmabuf;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::wayland::compositor::{add_blocker, add_pre_commit_hook};
use smithay::wayland::dmabuf::{get_dmabuf, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use std::collections::HashMap;
use smithay::delegate_compositor;
use smithay::delegate_cursor_shape;
use smithay::delegate_data_device;
use smithay::delegate_output;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_xdg_decoration;
use smithay::delegate_xdg_shell;
use smithay::desktop::{PopupKind, PopupManager};
use smithay::input::keyboard::{FilterResult, KeyboardHandle, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent, PointerHandle};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::channel::{self, Channel, Event as ChannelEvent};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_callback::WlCallback;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::utils::{IsAlive, Logical, Point, Serial, Transform, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::wayland::compositor::{
    get_parent, with_states, with_surface_tree_downward, with_surface_tree_upward, BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SubsurfaceCachedState, SurfaceAttributes,
    TraversalAction,
};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::{with_buffer_contents, ShmHandler, ShmState};
use smithay::wayland::socket::ListeningSocketSource;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a surface last showed: its size, and either its pixels (BGRA,
/// premultiplied) or its buffer on the card. `changed`: the render does not
/// have it yet. `key` names the surface for the render, frame after frame.
#[derive(Default)]
struct Content {
    key: u64,
    /// Its size in the window's units, and its pixels: more than that if it
    /// draws at a monitor's scale, or other if it is scaled (wp-viewporter);
    /// `src`, which part of them it shows.
    size: (usize, usize),
    px: (usize, usize),
    src: [f32; 4],
    data: Vec<u8>,
    dmabuf: Option<(u64, Dmabuf)>,
    changed: bool,
}

/// A program's window: a Wayland one (xdg-shell), or an X11 one (XWayland).
/// Both are a surface of the compositor; they differ in how they are asked
/// for a size, told they have the keyboard, or closed.
#[derive(Clone, PartialEq)]
enum Toplevel {
    Xdg(ToplevelSurface),
    X11(X11Surface),
}

impl Toplevel {
    fn send_close(&self) {
        match self {
            Toplevel::Xdg(t) => t.send_close(),
            Toplevel::X11(x) => {
                let _ = x.close();
            }
        }
    }

    /// The size the scene gives it (0 × 0: the one it chooses); an X11 one
    /// keeps where it is on the desktop.
    fn resize(&self, w: i32, h: i32) {
        match self {
            Toplevel::Xdg(t) => {
                t.with_pending_state(|s| s.size = (w > 0 && h > 0).then(|| (w, h).into()));
                if t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
            Toplevel::X11(x) => {
                if w > 0 && h > 0 {
                    let loc = x.geometry().loc;
                    let _ = x.configure(smithay::utils::Rectangle::new(loc, (w, h).into()));
                }
            }
        }
    }

    /// Whether it is a dialog: it belongs to another window, or it has a size
    /// of its own it cannot leave (a message, a file chooser, a splash).
    fn is_dialog(&self, surface: &WlSurface) -> bool {
        match self {
            Toplevel::Xdg(t) => {
                let (min, max) = with_states(surface, |s| {
                    let c = *s.cached_state.get::<SurfaceCachedState>().current();
                    (c.min_size, c.max_size)
                });
                t.parent().is_some() || (min.w > 0 && min.h > 0 && min == max)
            }
            Toplevel::X11(x) => {
                use smithay::xwayland::xwm::WmWindowType as T;
                let fixed = matches!((x.min_size(), x.max_size()), (Some(a), Some(b)) if a == b && a.w > 0);
                x.is_transient_for().is_some() || fixed || matches!(x.window_type(), Some(T::Dialog | T::Utility | T::Splash))
            }
        }
    }

    fn set_activated(&self, yes: bool) {
        match self {
            Toplevel::Xdg(t) => {
                let changed = t.with_pending_state(|s| {
                    let has = s.states.contains(xdg_toplevel::State::Activated);
                    if yes && !has {
                        s.states.set(xdg_toplevel::State::Activated);
                    } else if !yes && has {
                        s.states.unset(xdg_toplevel::State::Activated);
                    }
                    has != yes
                });
                if changed && t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
            Toplevel::X11(x) => {
                if x.is_activated() != yes {
                    let _ = x.set_activated(yes);
                }
            }
        }
    }

    fn set_fullscreen(&self, yes: bool) {
        match self {
            Toplevel::Xdg(t) => {
                t.with_pending_state(|s| {
                    if yes {
                        s.states.set(xdg_toplevel::State::Fullscreen);
                    } else {
                        s.states.unset(xdg_toplevel::State::Fullscreen);
                    }
                });
                if t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
            Toplevel::X11(x) => {
                let _ = x.set_fullscreen(yes);
            }
        }
    }
}

struct Window {
    toplevel: Toplevel,
    /// Its surface in the compositor (for an X11 one, the one XWayland made for it).
    surface: WlSurface,
    title: String,
    app: String,
    /// Where the window itself is inside its buffer: a program that draws
    /// its own shadow says where the shadow ends.
    geometry: [i32; 4],
    /// The surfaces the render was last told about: one that appears again
    /// is sent whole, even if it has not drawn anything new.
    sent: Vec<u64>,
    /// The monitor it is on: which copy of the scene lays it out.
    screen: usize,
    /// Where the scene shows it: on which monitor, and its box there.
    shown: Option<(String, [i32; 4])>,
    /// Its entry in ext-foreign-toplevel-list (what a screen share lists).
    listed: ForeignToplevelHandle,
    fullscreen: bool,
    /// A dialog floats: it is not in the layout's order.
    dialog: bool,
    /// A rule says it floats (`window app=… float`): as a dialog does.
    floating: bool,
    /// What the rules said of it, once one matched (its size, if they gave one).
    ruled: Option<Option<(i32, i32)>>,
    /// Put away (minimized): out of the order too, and where it was in it,
    /// to go back there.
    minimized: bool,
    was_at: usize,
    /// The scene floats it over the layout (`float`): unlike `floating`, it
    /// keeps its turn in the order (the keyboard, `promote`); only the
    /// layout leaves it out. Kept here, it outlives a reload of the scene.
    floated: bool,
    /// The monitor it had to leave because it went away (unplugged): when it
    /// comes back, the window goes back to it, with its workspace.
    home: Option<String>,
}

/// A number for a program, the same for all its surfaces and buffers.
fn owner_hash(c: &ClientId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    c.hash(&mut h);
    h.finish() | 1
}

/// Where a program's surface goes on its monitor, in units, by its anchors
/// and margins.
fn panel_place(c: &LayerSurfaceCachedState, (mw, mh): (i32, i32), (w, h): (i32, i32)) -> (i32, i32) {
    let m = c.margin;
    let (l, r, t, b) = (c.anchor.contains(Anchor::LEFT), c.anchor.contains(Anchor::RIGHT), c.anchor.contains(Anchor::TOP), c.anchor.contains(Anchor::BOTTOM));
    let x = if l && !r { m.left } else if r && !l { mw - w - m.right } else if l && r { m.left + (mw - m.left - m.right - w) / 2 } else { (mw - w) / 2 };
    let y = if t && !b { m.top } else if b && !t { mh - h - m.bottom } else if t && b { m.top + (mh - m.top - m.bottom - h) / 2 } else { (mh - h) / 2 };
    (x, y)
}

/// How much smaller a panel is shown on the phone's monitor: one wider than
/// the phone is shrunk as if the phone were `PHONE_ROOM` points across —room
/// for the middle of a desktop's panel: Marea's card—; anywhere else, and
/// any panel that fits, as it is.
const PHONE_ROOM: f64 = 580.0;

fn phone_zoom(monitor: Option<&str>, monitor_w: i32, w: i32) -> f64 {
    if monitor != Some(layers::PHONE_NAME) || w <= monitor_w || monitor_w <= 0 {
        return 1.0;
    }
    (monitor_w as f64 / PHONE_ROOM).clamp(0.4, 1.0)
}

fn owner_of(surface: &WlSurface) -> u64 {
    surface.client().map_or(0, |c| owner_hash(&c.id()))
}

/// The name the window manager's scene listens by (`session` for
/// `session.plm`): its programs reach it as `wm` (`pleamar --say wm …`).
static SCENE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_scene_name(path: &str) {
    let stem = std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("session").to_owned();
    let _ = SCENE.set(stem);
}

/// Where a session says its desktop (`pleamar-wm hyprctl` reads it): in the
/// folder of its own programs, the one `PLEAMAR_SOCKETS` names for them. One
/// file for every session was the last one's word for all: a check run
/// without a screen told Marea, on Hyprland, of monitors that do not exist.
pub fn desktop_file_of(socket: &str) -> String {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    format!("{dir}/pleamar-{socket}/desktop")
}

/// The desktop of the session this program runs in, if it runs in one.
pub fn desktop_file() -> Option<String> {
    std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()).map(|d| format!("{d}/desktop"))
}

/// A picture of a monitor a program asked for: of which piece (x, y, w, h,
/// on it), and into which of its buffers once it says.
struct Picture {
    frame: ZwlrScreencopyFrameV1,
    monitor: usize,
    piece: [i32; 4],
    buffer: Option<WlBuffer>,
    damage: bool,
    /// The pointer drawn into it: its program asked for it (`overlay_cursor`).
    pointer: bool,
}

/// A program's buffer on loan: given back with `release`, and, with
/// explicit sync, its release point signalled.
struct Lent {
    buffer: WlBuffer,
    release: Option<DrmSyncPoint>,
    /// Since when, and whether it was said that it is taking long.
    since: std::time::Instant,
    told: bool,
    /// Whose it is: a window (by its program), a program's surface, a menu;
    /// and its surface, to know whether it is still the picture shown.
    what: String,
    surface: WlSurface,
}

impl Lent {
    fn give_back(self) {
        self.buffer.release();
        if let Some(p) = self.release {
            if let Err(e) = p.signal() {
                eprintln!("windows · a release point could not be signalled: {e}");
            }
        }
    }
}

/// A program's layer-shell surface (Marea, a bar, a wallpaper): not the
/// scene's to lay out, but put together by the monitor it asked for.
struct Panel {
    shell: Shell,
    id: u64,
    monitor: usize,
    /// The size it was last told.
    configured: Option<(i32, i32)>,
    sent: Vec<u64>,
}

/// A window's menu in a session of its own: not a piece of the window, which
/// the scene draws inside its box, but a surface of the monitor over
/// everything, where the window is seen —so it can go past the window's edge—.
struct Menu {
    id: u64,
    surface: WlSurface,
    /// The window's surface it hangs from.
    root: WlSurface,
    monitor: usize,
    sent: Vec<u64>,
}

/// A program dragging something: its icon (one surface per monitor, at the
/// pointer), and, read as soon as it starts, what it drags as text —paths,
/// words—, for the scene if it is dropped there (by the time of the drop the
/// program has been told it went nowhere).
struct Drag {
    icon: Option<WlSurface>,
    ids: Vec<(usize, u64, Vec<u64>)>,
    text: Arc<Mutex<Option<(String, String)>>>,
}

/// What a program's surface of its own is: a layer (layer-shell), or the
/// lock screen of one monitor (ext-session-lock).
enum Shell {
    Layer(LayerSurface),
    Lock(LockSurface),
}

impl Shell {
    fn wl_surface(&self) -> &WlSurface {
        match self {
            Shell::Layer(l) => l.wl_surface(),
            Shell::Lock(l) => l.wl_surface(),
        }
    }
}

/// Where the programs connect, and whether they can.
#[derive(Default)]
struct ClientState {
    compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

/// A computer-use agent's hands (`agent on`): see `agent.rs`.
#[path = "agent.rs"]
mod agent;

struct State {
    dh: DisplayHandle,
    compositor: CompositorState,
    xdg: XdgShellState,
    /// Kept alive: without them their globals go away.
    _decorations: XdgDecorationState,
    kde_decorations: KdeDecorationState,
    _cursor_shapes: CursorShapeManagerState,
    shm: ShmState,
    seats: SeatState<State>,
    data_device: DataDeviceState,
    /// What is copied now, as far as giving it back after the agent pastes needs.
    pub(super) clipboard: Clipboard,
    popups: PopupManager,
    seat: Seat<State>,
    keyboard: KeyboardHandle<State>,
    pointer: PointerHandle<State>,
    /// One per monitor, with its name (`DP-3`); nested, a single one.
    outputs: Vec<Output>,
    output_globals: Vec<smithay::reexports::wayland_server::backend::GlobalId>,
    _output_manager: OutputManagerState,
    layer_shell: WlrLayerShellState,
    panels: Vec<Panel>,
    menus: Vec<Menu>,
    drag: Option<Drag>,
    /// What the programs' bars keep on each monitor, last told to the scene.
    reserved: Vec<[f32; 4]>,
    /// Pictures of a monitor a program asked for (wlr-screencopy: grim, a
    /// recorder, a lens), until their monitor has taken them.
    pictures: HashMap<u64, Picture>,
    /// Pictures of single windows, for the programs that want them
    /// (ext-image-copy-capture): see `capture.rs`.
    thumbs: capture::Thumbs,
    /// The program's surface the pointer is on, or the one with the keyboard.
    panel_pointer: Option<u64>,
    panel_keyboard: Option<u64>,
    /// Whether the program's surface with the keyboard has it because it
    /// asked for all of it (then it goes back when it stops asking).
    exclusive_keyboard: bool,
    _session_lock: SessionLockManagerState,
    /// Who listens to what windows there are (wlr-foreign-toplevel: Marea's
    /// «where the focus is», a taskbar), and each window's handle for each.
    toplevel_managers: Vec<ZwlrForeignToplevelManagerV1>,
    toplevel_handles: Vec<(usize, ZwlrForeignToplevelHandleV1)>,
    /// What the session starts, once programs can hand over their frames on
    /// the card: started before, they would be told to draw in software.
    autostart: Vec<String>,
    /// One per slot of the scene. A window that finds them all taken waits in `waiting`.
    slots: Vec<Option<Window>>,
    waiting: Vec<(Toplevel, WlSurface)>,
    /// The order the scene lays them out in: the first is the one that leads.
    order: Vec<usize>,
    /// Where the next one goes: turning round, so that a slot just freed —whose
    /// image the scene may still be fading— is the last to be taken again.
    next_slot: usize,
    /// The size the scene wants for each slot, to answer a new window with it.
    asked: Vec<Option<(i32, i32)>>,
    /// The windows that took the frame the scene draws (xdg-decoration,
    /// server side), by their surface: the others draw their own.
    decorated: std::collections::HashSet<WlSurface>,
    /// A window that asked, from its own title bar or edge, to be carried or
    /// stretched: until the button is let go.
    held: Option<usize>,
    focus: Option<usize>,
    /// The last new window given your keyboard, the one that had it before,
    /// and when: a dialog that says only afterwards whose it is (a portal's)
    /// gives it back if it is of the program the agent works with.
    focus_given: Option<(usize, Option<usize>, Instant)>,
    /// The monitor the pointer is on: where a new window opens.
    on_screen: usize,
    /// Whether pleamar's own window has the keyboard: without it, no program does.
    host_focus: bool,
    pointer_on: Option<usize>,
    /// The windows whose image has to be made again after this round.
    dirty: Vec<WlSurface>,
    /// The programs' frame callbacks, and the monitor of the surface when it is
    /// a program's own (a bar, Marea): that one is told only when its monitor
    /// has shown it. Told by any monitor, a surface on a 60 Hz monitor was
    /// asked for another frame at the pace of a 165 Hz one, and drawing it
    /// waited for a buffer its own monitor had not let go of yet: the program
    /// went at 60 on both. Windows (`None`) are drawn by the scene, as before.
    callbacks: Vec<(WlCallback, Option<usize>)>,
    to_render: Sender<ToRender>,
    handle: LoopHandle<'static, State>,
    /// Frames on the card (linux-dmabuf), once the render has said what it can read.
    dmabuf: DmabufState,
    dmabuf_global: Option<DmabufGlobal>,
    /// Each program buffer by number, and the ones lent to the render until it has copied them.
    buffers: HashMap<ObjectId, u64>,
    lent: HashMap<u64, Lent>,
    /// Explicit sync (linux-drm-syncobj), if the card can: programs say when
    /// their frame is ready and are told when it is no longer read.
    syncobj: Option<DrmSyncobjState>,
    next_number: u64,
    /// The programs pinned to the dock, as `dock …` says; pinning or unpinning
    /// from the dock changes them, and that line of session.conf.
    pins: Vec<String>,
    // The usual protocols: activation (a program that asks for its window to
    // come forward), the middle-click selection and clipboard managers,
    // idleness, virtual keyboards and input methods, pointer lock (games),
    // dialogs, the window list, and when each frame was shown.
    activation: XdgActivationState,
    /// The windows asking for attention (see `request_activation`), until they get the keyboard.
    urgent: std::collections::HashSet<usize>,
    primary: PrimarySelectionState,
    _wlr_data_control: WlrDataControlState,
    _ext_data_control: ExtDataControlState,
    idle: IdleNotifierState<State>,
    last_activity: Instant,
    /// Surfaces that keep the screen awake (a video playing).
    inhibitors: Vec<WlSurface>,
    _idle_inhibit: IdleInhibitManagerState,
    _virtual_keyboard: VirtualKeyboardManagerState,
    _text_input: TextInputManagerState,
    _input_method: InputMethodManagerState,
    _constraints: PointerConstraintsState,
    _relative: RelativePointerManagerState,
    foreign: XdgForeignState,
    _dialog: XdgDialogState,
    toplevel_list: ForeignToplevelListState,
    _presentation: PresentationState,
    _content_type: ContentTypeState,
    _viewporter: ViewporterState,
    _fractional_scale: FractionalScaleManagerState,
    _single_pixel: SinglePixelBufferState,
    /// Frames waiting to be said shown, and on which monitor.
    presented: Vec<(PresentationFeedbackCallback, usize)>,
    presented_seq: u64,
    /// The surface under the pointer and where it is, as last told: where
    /// relative motion goes.
    last_under: Option<(WlSurface, Point<f64, Logical>)>,
    /// The surface that holds the pointer (locked or confined), if any.
    constrained: Option<WlSurface>,
    /// Who watches a monitor's power (wlr-output-power-management), and which.
    powers: Vec<(ZwlrOutputPowerV1, usize)>,
    socket: String,
    /// XWayland's display (`:1`), once it is ready; while it starts, what is
    /// launched waits for it.
    x_display: Option<String>,
    x_starting: bool,
    xwm: Option<X11Wm>,
    xwayland_shell: XWaylandShellState,
    /// X11 windows mapped whose surface has not arrived yet.
    x11_pending: Vec<X11Surface>,
    /// X11's menus and tooltips (override-redirect): drawn with their window.
    unmanaged: Vec<X11Surface>,
    start: Instant,
    quit: bool,
    /// A computer-use agent's seats and socket, if session.conf lets one in.
    agent: Option<agent::Agent>,
}

/// Starts the compositor on its own thread. What it is told goes through the
/// returned sender; what it has to say arrives at the render as `ToRender::Nest`.
pub fn start(max: usize, to_render: Sender<ToRender>) -> Option<channel::Sender<ToNest>> {
    let (tx, rx) = channel::channel::<ToNest>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("windows".into())
        .spawn(move || {
            if let Err(e) = run(max, to_render, rx, ready_tx) {
                eprintln!("windows · the compositor stopped: {e}");
            }
        })
        .ok()?;
    ready_rx.recv_timeout(Duration::from_secs(6)).ok().flatten().map(|()| tx)
}

fn run(max: usize, to_render: Sender<ToRender>, rx: Channel<ToNest>, ready: std::sync::mpsc::Sender<Option<()>>) -> Result<(), String> {
    let mut event_loop: EventLoop<State> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let display: Display<State> = Display::new().map_err(|e| e.to_string())?;
    let dh = display.handle();

    // A name of our own, not `wayland-1`: that one may be another compositor's,
    // and a program started by hand should not end up here by mistake.
    let (source, socket) = (1..64)
        .find_map(|k| {
            let name = format!("pleamar-{k}");
            ListeningSocketSource::with_name(&name).ok().map(|s| (s, name))
        })
        .ok_or("no free socket name")?;
    event_loop
        .handle()
        .insert_source(source, |stream, _, state: &mut State| {
            remember_client(&stream);
            if let Err(e) = state.dh.insert_client(stream, Arc::new(ClientState::default())) {
                eprintln!("windows · a program could not connect: {e}");
            }
        })
        .map_err(|e| e.to_string())?;
    event_loop
        .handle()
        .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, state: &mut State| {
            // SAFETY: the display is not dropped while the loop runs: it lives in this source.
            unsafe {
                display.get_mut().dispatch_clients(state).map_err(std::io::Error::other)?;
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| e.to_string())?;
    event_loop
        .handle()
        .insert_source(rx, |event, _, state: &mut State| match event {
            ChannelEvent::Msg(m) => state.handle(m),
            ChannelEvent::Closed => state.quit = true,
        })
        .map_err(|e| e.to_string())?;

    dh.create_global::<State, ZwlrForeignToplevelManagerV1, _>(3, ());
    // What is behind a program's surface, blurred where it asks (Marea's glass).
    dh.create_global::<State, ExtBackgroundEffectManagerV1, _>(1, ());
    let mut seats = SeatState::new();
    let mut seat = seats.new_wl_seat(&dh, "pleamar");
    // The keyboard as the user has it: the layout pleamar's own window was
    // given. If there is none yet, the system's default until it arrives.
    // Repeating as the session's configuration says (25 a second after 400 ms, if nothing).
    let (rate, delay) = crate::config::get().repeat();
    let keyboard = seat.add_keyboard(XkbConfig::default(), delay as i32, rate as i32).map_err(|e| e.to_string())?;
    let pointer = seat.add_pointer();
    // The monitors, as the session has them; nested, one that is the scene.
    let monitors = layers::wait_monitors(Duration::from_millis(2500));
    let outputs: Vec<Output> = if monitors.is_empty() {
        let output = Output::new("pleamar".into(), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "pleamar".into(), model: "windows".into() });
        let mode = OutputMode { size: (1280, 800).into(), refresh: 60_000 };
        output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Integer(1)), Some((0, 0).into()));
        output.set_preferred(mode);
        vec![output]
    } else {
        monitors
            .iter()
            .map(|m| {
                let output = Output::new(m.name.clone(), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "pleamar".into(), model: m.name.clone() });
                let mode = OutputMode { size: (m.size.0 as i32, m.size.1 as i32).into(), refresh: m.mhz };
                output.change_current_state(Some(mode), Some(Transform::Normal), Some(output_scale(m.scale)), Some((m.x, m.y).into()));
                output.set_preferred(mode);
                output
            })
            .collect()
    };
    let output_globals: Vec<smithay::reexports::wayland_server::backend::GlobalId> = outputs.iter().map(|o| o.create_global::<State>(&dh)).collect();
    // Pictures of the monitors, for the programs that take them (grim, a
    // recorder): only in a session of its own, which has monitors.
    if !monitors.is_empty() {
        dh.create_global::<State, ZwlrScreencopyManagerV1, _>(3, ());
        // And their power, for what turns them off when idle (hypridle, wlopm).
        dh.create_global::<State, ZwlrOutputPowerManagerV1, _>(1, ());
    }
    // Pictures of one window (an overview's thumbnails, a recorder of one
    // window): on any session, the windows are on the card in all of them.
    capture::globals(&dh);
    let (thumbs_tx, thumbs_rx) = channel::channel::<(String, WindowPicture)>();
    event_loop
        .handle()
        .insert_source(thumbs_rx, |event, _, state: &mut State| {
            if let ChannelEvent::Msg((window, picture)) = event {
                state.thumb_arrived(window, picture);
            }
        })
        .map_err(|e| e.to_string())?;
    // What the session and the monitors tell about the programs' surfaces.
    let (layers_tx, layers_rx) = channel::channel::<ToLayers>();
    layers::set_nest(layers_tx);
    crate::portal::start(to_render.clone());
    event_loop
        .handle()
        .insert_source(layers_rx, |event, _, state: &mut State| {
            if let ChannelEvent::Msg(m) = event {
                state.layer_input(m);
            }
        })
        .map_err(|e| e.to_string())?;

    let primary = PrimarySelectionState::new::<State>(&dh);
    let mut state = State {
        compositor: CompositorState::new::<State>(&dh),
        xdg: XdgShellState::new::<State>(&dh),
        _decorations: XdgDecorationState::new::<State>(&dh),
        kde_decorations: KdeDecorationState::new::<State>(&dh, smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode::Server),
        _cursor_shapes: CursorShapeManagerState::new::<State>(&dh),
        shm: ShmState::new::<State>(&dh, vec![]),
        data_device: DataDeviceState::new::<State>(&dh),
        clipboard: Clipboard::Nothing,
        popups: PopupManager::default(),
        activation: XdgActivationState::new::<State>(&dh),
        urgent: std::collections::HashSet::new(),
        _wlr_data_control: WlrDataControlState::new::<State, _>(&dh, Some(&primary), |_| true),
        _ext_data_control: ExtDataControlState::new::<State, _>(&dh, Some(&primary), |_| true),
        primary,
        idle: IdleNotifierState::new(&dh, event_loop.handle()),
        last_activity: Instant::now(),
        inhibitors: Vec::new(),
        _idle_inhibit: IdleInhibitManagerState::new::<State>(&dh),
        _virtual_keyboard: VirtualKeyboardManagerState::new::<State, _>(&dh, |_| true),
        _text_input: TextInputManagerState::new::<State>(&dh),
        _input_method: InputMethodManagerState::new::<State, _>(&dh, |_| true),
        _constraints: PointerConstraintsState::new::<State>(&dh),
        _relative: RelativePointerManagerState::new::<State>(&dh),
        foreign: XdgForeignState::new::<State>(&dh),
        _dialog: XdgDialogState::new::<State>(&dh),
        toplevel_list: ForeignToplevelListState::new::<State>(&dh),
        _presentation: PresentationState::new::<State>(&dh, libc::CLOCK_MONOTONIC as u32),
        _content_type: ContentTypeState::new::<State>(&dh),
        _viewporter: ViewporterState::new::<State>(&dh),
        _fractional_scale: FractionalScaleManagerState::new::<State>(&dh),
        _single_pixel: SinglePixelBufferState::new::<State>(&dh),
        presented: Vec::new(),
        presented_seq: 0,
        last_under: None,
        constrained: None,
        powers: Vec::new(),
        seats,
        seat,
        keyboard,
        pointer,
        _output_manager: OutputManagerState::new_with_xdg_output::<State>(&dh),
        layer_shell: WlrLayerShellState::new::<State>(&dh),
        outputs,
        output_globals,
        panels: Vec::new(),
        menus: Vec::new(),
        drag: None,
        reserved: Vec::new(),
        pictures: HashMap::new(),
        thumbs: capture::Thumbs::new(thumbs_tx),
        panel_pointer: None,
        panel_keyboard: None,
        exclusive_keyboard: false,
        _session_lock: SessionLockManagerState::new::<State, _>(&dh, |_| true),
        toplevel_managers: Vec::new(),
        toplevel_handles: Vec::new(),
        autostart: Vec::new(),
        slots: (0..max).map(|_| None).collect(),
        waiting: Vec::new(),
        order: Vec::new(),
        next_slot: 0,
        asked: vec![None; max],
        decorated: std::collections::HashSet::new(),
        held: None,
        focus: None,
        focus_given: None,
        on_screen: 0,
        host_focus: true,
        pointer_on: None,
        dirty: Vec::new(),
        callbacks: Vec::new(),
        to_render,
        handle: event_loop.handle(),
        dmabuf: DmabufState::new(),
        dmabuf_global: None,
        buffers: HashMap::new(),
        lent: HashMap::new(),
        syncobj: None,
        next_number: 0,
        pins: crate::config::get().dock.clone(),
        socket: socket.clone(),
        x_display: None,
        x_starting: false,
        xwm: None,
        xwayland_shell: XWaylandShellState::new::<State>(&dh),
        x11_pending: Vec::new(),
        unmanaged: Vec::new(),
        start: Instant::now(),
        quit: false,
        agent: None,
        dh,
    };
    if let Some(keymap) = pleamar::host_keymap() {
        let k = state.keyboard.clone();
        if let Err(e) = k.set_keymap_from_string(&mut state, keymap) {
            eprintln!("windows · the keyboard layout could not be copied: {e:?}");
        }
    }
    state.write_desktop();
    agent::start(&mut state);
    state.start_xwayland();
    state.export_environment();
    println!("windows · programs connect at WAYLAND_DISPLAY={socket}");
    // And so do the ones the scene's logic starts (a screenshot, an app from
    // its launcher): this process may not have it in its own environment.
    pleamar::set_child_env("WAYLAND_DISPLAY", Some(&socket));
    pleamar::set_child_env("DISPLAY", None);
    let _ = state.to_render.send(ToRender::Nest(NestEvent::Socket(socket)));
    // What the dock has pinned (`dock …` in session.conf).
    let pins = crate::config::get().dock.iter().map(|w| crate::desktop::pin(w)).collect();
    let _ = state.to_render.send(ToRender::Nest(NestEvent::Dock(pins)));
    let _ = ready.send(Some(()));
    // A session of its own starts what the desktop has (the wallpaper, Marea):
    // `PLEAMAR_WM_AUTOSTART`, or ~/.config/pleamar/autostart, one command a
    // line. The same file `pleamar --autostart` runs on another compositor;
    // what only makes sense here says so with `wm:` in front.
    if !monitors.is_empty() {
        let file = std::env::var("PLEAMAR_WM_AUTOSTART").ok().or_else(|| crate::config::user_file("autostart", "autostart"));
        // With none of the user's, the one that comes with it (inside it).
        let text = match file.as_deref() {
            Some(f) => std::fs::read_to_string(f).ok(),
            None => Some(include_str!("../autostart").to_owned()),
        };
        if let Some(text) = text {
            state.autostart = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.strip_prefix("wm:").map_or(l, str::trim_start).to_owned())
                .collect();
        }
    }

    // A frame callback nobody answers leaves a program stopped: if the render
    // is not painting —a window that is not on the scene—, they are answered
    // anyway, slowly.
    let mut last_done = Instant::now();
    let mut last_census = Instant::now();
    let mut last_trim = Instant::now();
    while !state.quit {
        event_loop.dispatch(Some(Duration::from_millis(100)), &mut state).map_err(|e| e.to_string())?;
        state.tell_held();
        state.popups.cleanup();
        state.prune_menus();
        state.prune_inhibitors();
        state.compose_dirty();
        let x_ready = !state.x_starting || state.start.elapsed() > Duration::from_secs(5);
        if !state.autostart.is_empty() && x_ready && (state.dmabuf_global.is_some() || state.start.elapsed() > Duration::from_secs(3)) {
            for line in std::mem::take(&mut state.autostart) {
                println!("windows · starting: {line}");
                state.launch(&line);
            }
        }
        // Now and then, what the allocator keeps without using goes back to the
        // system: a window's pixels are megabytes, freed in one thread and
        // asked for again in another, and glibc kept the room for good —the
        // session looked hundreds of MB bigger than what it held—.
        #[cfg(target_env = "gnu")]
        if last_trim.elapsed() > Duration::from_secs(30) {
            last_trim = Instant::now();
            // SAFETY: plain glibc call, no pointers.
            unsafe { libc::malloc_trim(0) };
        }
        // `PLEAMAR_TIMING=1`: what the windows' side keeps, now and then.
        if pleamar::gpu::timing_enabled() && last_census.elapsed() > Duration::from_secs(10) {
            last_census = Instant::now();
            println!(
                "windows · kept: {} windows, {} waiting, {} programs' buffers ({} lent), {} window listers ({} handles), {} pictures asked, {} keeping awake",
                state.slots.iter().flatten().count(),
                state.waiting.len(),
                state.buffers.len(),
                state.lent.len(),
                state.toplevel_managers.len(),
                state.toplevel_handles.len(),
                state.pictures.len(),
                state.inhibitors.len()
            );
        }
        if last_done.elapsed() > Duration::from_millis(250) && !state.callbacks.is_empty() {
            state.frame_done_all();
        }
        if !state.callbacks.is_empty() {
            last_done = last_done.min(Instant::now());
        } else {
            last_done = Instant::now();
        }
        let _ = state.dh.flush_clients();
    }
    Ok(())
}

impl State {
    fn time(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn tell(&self, e: NestEvent) {
        let _ = self.to_render.send(ToRender::Nest(e));
    }

    fn window_of(&self, surface: &WlSurface) -> Option<usize> {
        self.slots.iter().position(|w| w.as_ref().is_some_and(|w| &w.surface == surface))
    }

    /// The window a window belongs to (a dialog's): the one it says it is
    /// for, of its own program or of another (a portal's file chooser, for
    /// the browser that asked, through xdg-foreign).
    pub(crate) fn parent_slot(&self, slot: usize) -> Option<usize> {
        match &self.slots.get(slot)?.as_ref()?.toplevel {
            Toplevel::Xdg(t) => t.parent().and_then(|p| self.window_of(&p)),
            Toplevel::X11(x) => {
                let parent = x.is_transient_for()?;
                self.slots.iter().position(|w| matches!(w.as_ref().map(|w| &w.toplevel), Some(Toplevel::X11(t)) if t.window_id() == parent))
            }
        }
        .filter(|p| *p != slot)
    }

    /// A dialog opens where the window it belongs to is, not where the
    /// mouse happens to be: a «Save as» asked on one monitor came up on the
    /// other, and was not found.
    fn beside_parent(&mut self, slot: usize) {
        let Some(parent) = self.parent_slot(slot) else { return };
        let Some(screen) = self.slots[parent].as_ref().map(|w| w.screen) else { return };
        if self.slots[slot].as_ref().is_some_and(|w| w.screen != screen) {
            self.handle(ToNest::Send(slot, screen));
        }
    }

    fn handle(&mut self, m: ToNest) {
        if std::env::var_os("PLEAMAR_DEBUG_WINDOWS").is_some() && !matches!(m, ToNest::FrameDone) {
            eprintln!("windows · {m:?}");
        }
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.time();
        match m {
            ToNest::Size(w, h) => {
                // Nested, the output is the scene; with monitors of our own, they are.
                if layers::monitors().is_empty() {
                    let mode = OutputMode { size: (w.max(1), h.max(1)).into(), refresh: 60_000 };
                    self.outputs[0].change_current_state(Some(mode), None, None, None);
                    self.outputs[0].set_preferred(mode);
                }
            }
            ToNest::Configure { slot, w, h } => {
                // Never a size a program cannot live in: a scene that asks for
                // the box of an animation (60 × 4, 1 × 1038) froze browsers.
                // 0 is still «the size you choose».
                const LEAST: i32 = 32;
                if (w > 0 && w < LEAST) || (h > 0 && h < LEAST) {
                    return;
                }
                if self.asked.get(slot) == Some(&Some((w, h))) {
                    return;
                }
                if let Some(a) = self.asked.get_mut(slot) {
                    *a = Some((w, h));
                }
                if let Some(Some(win)) = self.slots.get(slot) {
                    // Floating at its own size (`ask: 0, 0`) with a rule that gives it one: that one.
                    let (w, h) = match win.ruled {
                        Some(Some(size)) if (w, h) == (0, 0) => size,
                        _ => (w, h),
                    };
                    win.toplevel.resize(w, h);
                }
            }
            ToNest::Pointer { slot, x, y } => {
                let Some(Some(win)) = self.slots.get(slot) else { return };
                let g = win.geometry;
                let at: Point<f64, Logical> = (g[0] as f64 + x, g[1] as f64 + y).into();
                let root = win.surface.clone();
                let under = self.surface_under(&root, g, at).map(|(s, o)| (s, o.to_f64()));
                self.pointer_on = Some(slot);
                self.panel_pointer = None;
                self.last_under = under.clone();
                let p = self.pointer.clone();
                p.motion(self, under, &MotionEvent { location: at, serial, time });
                p.frame(self);
                self.update_constraint();
                self.activity();
            }
            ToNest::PointerOut => {
                if self.pointer_on.take().is_some() {
                    self.last_under = None;
                    let p = self.pointer.clone();
                    p.motion(self, None, &MotionEvent { location: (0.0, 0.0).into(), serial, time });
                    p.frame(self);
                    self.update_constraint();
                }
            }
            ToNest::Button { code, down } => {
                // Clicking outside a menu closes it, as it would on any desktop:
                // the program is told its popup is done.
                if down {
                    if let Some(slot) = self.pointer_on {
                        if let Some(Some(w)) = self.slots.get(slot) {
                            let root = w.surface.clone();
                            self.dismiss_popups_not_under(&root);
                        }
                        if self.focus != Some(slot) {
                            self.set_focus(Some(slot));
                        }
                    }
                }
                // A window carried from its own title bar is let go with the button.
                if !down {
                    if let Some(slot) = self.held.take() {
                        self.tell(NestEvent::Held { slot, how: 0, edges: 0 });
                    }
                }
                let p = self.pointer.clone();
                let state = if down { smithay::backend::input::ButtonState::Pressed } else { smithay::backend::input::ButtonState::Released };
                p.button(self, &ButtonEvent { serial, time, button: code, state });
                p.frame(self);
                self.activity();
            }
            ToNest::Wheel(dy) => {
                let p = self.pointer.clone();
                // Up is positive in pleamar; in Wayland, down.
                let frame = AxisFrame::new(time)
                    .source(smithay::backend::input::AxisSource::Wheel)
                    .value(smithay::backend::input::Axis::Vertical, -dy * 15.0)
                    .v120(smithay::backend::input::Axis::Vertical, (-dy * 120.0) as i32);
                p.axis(self, frame);
                p.frame(self);
            }
            ToNest::Key { code, down } => {
                // A key let go is only that: the keyboard hears it whoever has
                // it now, and nobody's focus changes. Lost —no window had the
                // focus any more, or a program's surface had it— it stayed held
                // for every window that got the keyboard after.
                if !down {
                    let k = self.keyboard.clone();
                    k.input::<(), _>(self, (code + 8).into(), smithay::backend::input::KeyState::Released, serial, time, |_, _, _| FilterResult::Forward);
                    return;
                }
                if self.focus.is_none() {
                    return;
                }
                // The keyboard was on a program's surface: back to the windows.
                if self.panel_keyboard.take().is_some() {
                    self.set_focus(self.focus);
                }
                // A key that arrives is pleamar's to give, whatever was said about its focus.
                if !self.host_focus {
                    self.handle(ToNest::HostFocus(true));
                }
                let k = self.keyboard.clone();
                let state = if down { smithay::backend::input::KeyState::Pressed } else { smithay::backend::input::KeyState::Released };
                k.input::<(), _>(self, (code + 8).into(), state, serial, time, |_, _, _| FilterResult::Forward);
                self.activity();
            }
            ToNest::HostFocus(yes) => {
                self.host_focus = yes;
                let target = if yes { self.focus.and_then(|s| self.slots[s].as_ref()).map(|w| w.surface.clone()) } else { None };
                let k = self.keyboard.clone();
                k.set_focus(self, target, serial);
            }
            // The scene gives the keyboard to the window the pointer is over. A
            // window of the program the agent is working with this moment, while
            // your keyboard is in another, does not take it that way: it opened
            // under your pointer, or the agent's work moved it there; it was not
            // gone to. A click of yours gives it (`Button`, above).
            ToNest::Focus(slot) => {
                if !self.agent_keeps_your_keyboard(self.pid_of(slot)) {
                    self.set_focus(Some(slot));
                }
            }
            ToNest::Close(slot) => {
                if let Some(Some(w)) = self.slots.get(slot) {
                    w.toplevel.send_close();
                }
            }
            ToNest::Promote(slot) => {
                if self.order.contains(&slot) {
                    self.order.retain(|s| *s != slot);
                    self.order.insert(0, slot);
                    self.tell(NestEvent::Order(self.order.clone()));
                }
            }
            ToNest::Launch(command) => {
                self.launch(&command);
            }
            ToNest::Pin(program, yes) => self.pin(&program, yes),
            // Its workspace is no longer shown: nobody has the keyboard.
            ToNest::Blur => self.set_focus(None),
            ToNest::Minimize(slot, yes) => self.set_minimized(slot, yes),
            ToNest::Float(slot, yes) => self.set_floated(slot, yes),
            ToNest::Fullscreen(slot) => {
                let now = self.slots.get(slot).and_then(Option::as_ref).is_some_and(|w| w.fullscreen);
                self.set_fullscreen(slot, !now);
            }
            ToNest::OnScreen(screen) => {
                if self.on_screen != screen {
                    self.on_screen = screen;
                    self.write_desktop();
                }
            }
            ToNest::Shown { slot, monitor, rect } => {
                layers::set_shown(slot, monitor.clone(), rect);
                // No longer drawn: where it was is still where its menus and
                // its X11 place are; only the ones who hide it need to know.
                if monitor.is_empty() {
                    return;
                }
                if let Some(m) = layers::monitors().iter().find(|m| m.name == monitor) {
                    self.x11_shown_at(slot, (m.x + rect[0], m.y + rect[1]));
                }
                if let Some(Some(w)) = self.slots.get_mut(slot) {
                    w.shown = Some((monitor, rect));
                    // Its menus go where it is now.
                    let root = w.surface.clone();
                    if self.menus.iter().any(|m| m.root == root) {
                        self.dirty.push(root);
                    }
                }
                if self.focus == Some(slot) {
                    self.write_desktop();
                }
            }
            ToNest::Send(slot, screen) => {
                if let Some(Some(w)) = self.slots.get_mut(slot) {
                    // Carried somewhere on purpose: where it came from no longer calls it back.
                    w.home = None;
                    if w.screen != screen {
                        let (from, to) = (self.outputs.get(w.screen).cloned(), self.outputs.get(screen).cloned());
                        w.screen = screen;
                        let surface = w.surface.clone();
                        if let (Some(from), Some(to)) = (from, to) {
                            enter_tree(&to, Some(&from), &surface);
                            for (_, h) in self.toplevel_handles.iter().filter(|(s, _)| *s == slot) {
                                let Some(client) = h.client() else { continue };
                                for o in from.client_outputs(&client) {
                                    h.output_leave(&o);
                                }
                                for o in to.client_outputs(&client) {
                                    h.output_enter(&o);
                                }
                                h.done();
                            }
                        }
                        self.tell(NestEvent::Screen(slot, screen));
                        self.tell_fullscreen();
                    }
                }
            }
            ToNest::Swap(a, b) => {
                let (Some(ia), Some(ib)) = (self.order.iter().position(|s| *s == a), self.order.iter().position(|s| *s == b)) else { return };
                self.order.swap(ia, ib);
                self.tell(NestEvent::Order(self.order.clone()));
                // Each takes the other's monitor too, if they were on different ones.
                let screens = (self.slots.get(a).and_then(Option::as_ref).map(|w| w.screen), self.slots.get(b).and_then(Option::as_ref).map(|w| w.screen));
                if let (Some(sa), Some(sb)) = screens {
                    if sa != sb {
                        self.handle(ToNest::Send(a, sb));
                        self.handle(ToNest::Send(b, sa));
                    }
                }
            }
            ToNest::FrameDone => self.frame_done(None),
            ToNest::Gpu { device, formats } => {
                if self.dmabuf_global.is_some() {
                    return;
                }
                let formats: Vec<Format> = formats.iter().filter_map(|(c, m)| Some(Format { code: Fourcc::try_from(*c).ok()?, modifier: Modifier::from(*m) })).collect();
                match DmabufFeedbackBuilder::new(device as libc::dev_t, formats).build() {
                    Ok(feedback) => {
                        self.dmabuf_global = Some(self.dmabuf.create_global_with_default_feedback::<State>(&self.dh, &feedback));
                        println!("windows · programs hand over their frames on the card");
                        // Explicit sync on the render node the programs draw with (the
                        // same as headless): with the session's card —the one that
                        // drives the screens— Chromium (Discord) stopped painting after
                        // its first frames, waiting for points that never arrived.
                        let card = render_node(device).or_else(layers::card);
                        match card {
                            Some(card) if supports_syncobj_eventfd(&card) => {
                                self.syncobj = Some(DrmSyncobjState::new::<State>(&self.dh, card));
                                println!("windows · programs sync with the card explicitly (linux-drm-syncobj)");
                            }
                            Some(_) => println!("windows · the card cannot wait on sync points: implicit sync only"),
                            None => println!("windows · no card to import sync points with: implicit sync only"),
                        }
                    }
                    Err(e) => eprintln!("windows · no frames on the card: {e}"),
                }
            }
            ToNest::Released(numbers) => self.release(numbers),
            ToNest::Picked(what) => crate::portal::picked(what),
            ToNest::Quit => self.quit = true,
        }
    }

    /// In the session the login screen starts (`PLEAMAR_WM_EXPORT`), where
    /// the programs started by dbus and systemd —the portals, notifications—
    /// are told where the desktop is. Not from a TTY beside another desktop:
    /// its portals would be pointed here.
    fn export_environment(&self) {
        if std::env::var_os("PLEAMAR_WM_EXPORT").is_none() || layers::monitors().is_empty() {
            return;
        }
        // Hyprland open on another TTY: the portals are its (they are one per
        // user), and pointing them here would leave it without them.
        if std::process::Command::new("pgrep").args(["-x", "Hyprland"]).stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success()) {
            println!("windows · Hyprland is open too: the portals stay its (sharing the screen from here will not work)");
            return;
        }
        let mut vars = vec![format!("WAYLAND_DISPLAY={}", self.socket), "XDG_SESSION_TYPE=wayland".to_owned()];
        vars.push(format!("XDG_CURRENT_DESKTOP={}", std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "pleamar".into())));
        if let Some(d) = &self.x_display {
            vars.push(format!("DISPLAY={d}"));
        }
        if let Some(a) = &self.agent {
            vars.push(format!("CUA_INJECT_SOCKET={}", a.path));
            vars.push("CUA_DRIVER_RS_ENABLE_WAYLAND=1".to_owned());
        }
        match std::process::Command::new("dbus-update-activation-environment").arg("--systemd").args(&vars).status() {
            Ok(s) if s.success() => println!("windows · dbus and systemd know where the desktop is: {}", vars.join(" ")),
            _ => eprintln!("windows · dbus-update-activation-environment failed: portals may not find the desktop"),
        }
    }

    /// A window's states as wlr-foreign-toplevel lists them: with the
    /// keyboard, and put away.
    fn states(&self, slot: usize) -> Vec<u8> {
        let mut out = Vec::new();
        if self.focus == Some(slot) {
            out.extend((toplevel_handle::State::Activated as u32).to_ne_bytes());
        }
        if self.slots.get(slot).and_then(Option::as_ref).is_some_and(|w| w.minimized) {
            out.extend((toplevel_handle::State::Minimized as u32).to_ne_bytes());
        }
        out
    }

    /// A window put away, or brought back. Away, it leaves the layout's order
    /// (the others close up) and the keyboard; back, it goes to where it was
    /// in the order and takes the keyboard. Its program and whoever lists the
    /// windows (Marea) are told.
    fn set_minimized(&mut self, slot: usize, yes: bool) {
        let Some(Some(w)) = self.slots.get_mut(slot) else { return };
        if w.minimized == yes {
            if !yes {
                self.set_focus(Some(slot));
            }
            return;
        }
        w.minimized = yes;
        let dialog = w.dialog;
        if yes {
            if let Some(at) = self.order.iter().position(|s| *s == slot) {
                if let Some(Some(w)) = self.slots.get_mut(slot) {
                    w.was_at = at;
                }
            }
            self.order.retain(|s| *s != slot);
        } else if !dialog && !self.order.contains(&slot) {
            let at = self.slots[slot].as_ref().map_or(0, |w| w.was_at).min(self.order.len());
            self.order.insert(at, slot);
        }
        self.tell(NestEvent::Minimized(slot, yes));
        self.tell(NestEvent::Order(self.order.clone()));
        if yes && self.focus == Some(slot) {
            // The keyboard to one that is seen on the same monitor —not to the
            // first in the order, which may be on another workspace: it had
            // the keys, unseen, and the next Super+M put it away too—.
            let monitor = layers::shown(slot).map(|(m, _)| m);
            let next = self.order.iter().copied().find(|s| *s != slot && layers::shown(*s).is_some_and(|(m, _)| Some(&m) == monitor.as_ref()));
            self.set_focus(next);
        } else if !yes {
            self.set_focus(Some(slot));
        }
        for (s, h) in &self.toplevel_handles {
            if *s == slot {
                h.state(self.states(slot));
                h.done();
            }
        }
    }

    /// One window over the layout, or back into it: the scene is told, and
    /// lays the others out without it.
    fn set_floated(&mut self, slot: usize, yes: bool) {
        let Some(Some(w)) = self.slots.get_mut(slot) else { return };
        if w.floated == yes {
            return;
        }
        w.floated = yes;
        self.tell(NestEvent::Floating(slot, yes));
    }

    /// A window asks to be carried (1) or stretched (2) by its edges, from
    /// its own frame: the scene does it while the button is down.
    pub(crate) fn hold(&mut self, slot: usize, how: u32, edges: u32) {
        if let Some(before) = self.held.replace(slot).filter(|s| *s != slot) {
            self.tell(NestEvent::Held { slot: before, how: 0, edges: 0 });
        }
        self.tell(NestEvent::Held { slot, how, edges });
    }

    /// A dialog is left out of the layout's order (the scene floats it);
    /// back in it at the end if it stops being one.
    fn set_dialog(&mut self, slot: usize, yes: bool) {
        let Some(Some(w)) = self.slots.get_mut(slot) else { return };
        if w.dialog == yes {
            return;
        }
        w.dialog = yes;
        self.order.retain(|s| *s != slot);
        if !yes {
            self.order.push(slot);
        }
        self.tell(NestEvent::Dialog(slot, yes));
        self.tell(NestEvent::Order(self.order.clone()));
    }

    /// A window's title, to the scene and to whoever lists the windows.
    fn set_title(&mut self, slot: usize, title: String) {
        if let Some(Some(w)) = self.slots.get_mut(slot) {
            w.title = title.clone();
            w.listed.send_title(&title);
            w.listed.send_done();
        }
        for (_, h) in self.toplevel_handles.iter().filter(|(s, _)| *s == slot) {
            h.title(title.clone());
            h.done();
        }
        self.tell(NestEvent::Title(slot, title));
        self.apply_rules(slot);
    }

    /// What `window` lines in session.conf say of it: floating, a size, a
    /// monitor, a workspace. A program says who it is only after its window
    /// exists, so this is looked at again as its app and title arrive, until
    /// one rule matches; then once.
    fn apply_rules(&mut self, slot: usize) {
        let Some(Some(w)) = self.slots.get(slot) else { return };
        if w.ruled.is_some() || (w.app.is_empty() && w.title.is_empty()) {
            return;
        }
        let rules = crate::config::get().for_window(&w.app, &w.title);
        if rules == crate::config::ForWindow::default() {
            return;
        }
        println!("windows · {} «{}»: {rules:?}", w.app, w.title);
        layers::set_private(slot, rules.private);
        let size = rules.size.map(|(a, b)| (a as i32, b as i32));
        let screen = w.screen;
        if let Some(Some(w)) = self.slots.get_mut(slot) {
            w.ruled = Some(size);
            w.floating |= rules.float;
            if let Some((a, b)) = size {
                w.toplevel.resize(a, b);
            }
        }
        if rules.float {
            self.set_dialog(slot, true);
        }
        let to = rules.monitor.as_deref().and_then(|m| m.parse::<usize>().ok().or_else(|| layers::monitors().iter().position(|x| x.name == m))).filter(|k| *k < self.outputs.len());
        if let Some(to) = to.filter(|k| *k != screen) {
            self.handle(ToNest::Send(slot, to));
        }
        if let Some(ws) = rules.workspace {
            self.tell(NestEvent::Workspace(slot, ws));
        }
    }

    /// Which program it is (its app id; an X11 one's class).
    fn set_app(&mut self, slot: usize, app: String) {
        if let Some(Some(w)) = self.slots.get_mut(slot) {
            w.app = app.clone();
            w.listed.send_app_id(&app);
            w.listed.send_done();
        }
        for (_, h) in self.toplevel_handles.iter().filter(|(s, _)| *s == slot) {
            h.app_id(app.clone());
            h.done();
        }
        self.tell(NestEvent::App(slot, app.clone()));
        let (icon, name, exec) = crate::desktop::program(&app);
        self.tell(NestEvent::Program { slot, icon, name, exec });
        self.apply_rules(slot);
    }

    /// A window to fullscreen or back: the program is told (it hides its own
    /// bars), and the scene, which decides where it goes. The monitors are
    /// told which have one.
    fn set_fullscreen(&mut self, slot: usize, yes: bool) {
        let Some(Some(w)) = self.slots.get_mut(slot) else { return };
        if w.fullscreen == yes {
            return;
        }
        w.fullscreen = yes;
        w.toplevel.set_fullscreen(yes);
        self.tell(NestEvent::Fullscreen(slot, yes));
        self.tell_fullscreen();
    }

    fn tell_fullscreen(&self) {
        let mut on = vec![false; self.outputs.len()];
        for w in self.slots.iter().flatten().filter(|w| w.fullscreen) {
            if let Some(o) = on.get_mut(w.screen) {
                *o = true;
            }
        }
        layers::set_fullscreen(&on);
    }

    /// A program, started so that it opens here and not on the desktop.
    /// A program pinned to the dock or unpinned: the dock is told again, and
    /// session.conf keeps it (its `dock` line, the rest of the file as it was).
    fn pin(&mut self, program: &str, yes: bool) {
        let p = program.to_lowercase();
        let known = |w: &String| crate::desktop::pin(w).keys.iter().any(|k| *k == p);
        if yes {
            if !self.pins.iter().any(known) {
                self.pins.push(program.to_owned());
            }
        } else {
            self.pins.retain(|w| !known(w));
        }
        println!("windows · the dock pins: {}", self.pins.join(" "));
        let pins = self.pins.iter().map(|w| crate::desktop::pin(w)).collect();
        self.tell(NestEvent::Dock(pins));
        crate::config::write_dock(&self.pins);
    }

    /// A command, as the session starts its programs; its process.
    fn launch(&self, command: &str) -> Option<u32> {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(command);
        c.env("WAYLAND_DISPLAY", &self.socket);
        // X11 through our XWayland; without it, no DISPLAY at all: a program
        // that prefers X11 would open on the real desktop instead of here.
        match &self.x_display {
            Some(d) => {
                c.env("DISPLAY", d);
            }
            None => {
                c.env_remove("DISPLAY");
            }
        }
        // Nor Hyprland's: a program that asks it things would get another desktop's answers.
        c.env_remove("HYPRLAND_INSTANCE_SIGNATURE");
        // In a session of its own, the scenes it starts (Marea) listen apart
        // from the ones of another desktop of the same user that may be open.
        // And the window manager's scene is there as `wm`: Marea switches it
        // between tiled and free with `pleamar --say wm "emit toggle_free"`.
        if !layers::monitors().is_empty() {
            let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
            let own = format!("{dir}/pleamar-{}", self.socket);
            let _ = std::fs::create_dir_all(&own);
            if let Some(scene) = SCENE.get() {
                let link = format!("{own}/wm.sock");
                let _ = std::fs::remove_file(&link);
                // Where the scene listens: its own place, if it has one (headless).
                let scenes = std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| format!("{dir}/pleamar"));
                let _ = std::os::unix::fs::symlink(format!("{scenes}/{scene}.sock"), &link);
            }
            // And where the mouse is on the whole desktop, for whoever asks.
            crate::cursor::serve(&own);
            c.env("PLEAMAR_SOCKETS", own);
        }
        // pleamar-wm itself by its name (`pleamar-wm hyprctl`, what Marea asks instead).
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
            let path = std::env::var("PATH").unwrap_or_default();
            c.env("PATH", format!("{}:{path}", dir.display()));
        }
        // A computer-use agent (Cua Driver) started from here finds its hands.
        if let Some(a) = &self.agent {
            c.env("CUA_INJECT_SOCKET", &a.path).env("CUA_DRIVER_RS_ENABLE_WAYLAND", "1");
        }
        c.env("GDK_BACKEND", "wayland").env("QT_QPA_PLATFORM", "wayland").env("MOZ_ENABLE_WAYLAND", "1").env("SDL_VIDEODRIVER", "wayland");
        // Programs that draw with the GPU hand over their frames on the card
        // (dmabuf). If the card the scene is painted on cannot read them, with
        // Mesa's software GL they hand over shared memory instead.
        if self.dmabuf_global.is_none() {
            c.env("LIBGL_ALWAYS_SOFTWARE", "1");
            if std::path::Path::new("/usr/share/glvnd/egl_vendor.d/50_mesa.json").exists() {
                c.env("__EGL_VENDOR_LIBRARY_FILENAMES", "/usr/share/glvnd/egl_vendor.d/50_mesa.json");
            }
        }
        c.stdin(std::process::Stdio::null());
        // A group of its own (with whatever it starts: a browser's content
        // processes), to be closed with the session.
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        match c.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                LAUNCHED.lock().unwrap().push(pid as i32);
                // Reaped when it ends, so it does not linger as a zombie.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Some(pid)
            }
            Err(e) => {
                eprintln!("windows · '{command}' could not start: {e}");
                None
            }
        }
    }

    fn set_focus(&mut self, slot: Option<usize>) {
        let before = self.focus;
        self.focus = slot.filter(|s| self.slots.get(*s).is_some_and(Option::is_some));
        // Active: the one with your keyboard, and the one the agent types in
        // (agent.rs, `kb_slot`).
        let agent_typing = self.agent.as_ref().and_then(|a| a.kb_slot);
        for (k, w) in self.slots.iter().enumerate() {
            let Some(w) = w else { continue };
            w.toplevel.set_activated(Some(k) == self.focus || Some(k) == agent_typing);
        }
        // An X11 one with the keyboard goes over the other X11 ones: its menus
        // and dialogs are found above it.
        if let Some(Toplevel::X11(x)) = self.focus.and_then(|s| self.slots[s].as_ref()).map(|w| w.toplevel.clone()) {
            if let Some(wm) = self.xwm.as_mut() {
                let _ = wm.raise_window(&x);
            }
        }
        let target = if self.host_focus { self.focus.and_then(|s| self.slots[s].as_ref()).map(|w| w.surface.clone()) } else { None };
        let k = self.keyboard.clone();
        k.set_focus(self, target, SERIAL_COUNTER.next_serial());
        // Given the keyboard, it no longer asks for attention.
        if let Some(s) = self.focus {
            if self.urgent.remove(&s) {
                self.tell(NestEvent::Urgent(s, false));
            }
        }
        if before != self.focus {
            // Where the keys go, said when it changes: «I could not type» is found here.
            let app = self.focus.and_then(|s| self.slots[s].as_ref()).map_or("nobody".to_owned(), |w| w.app.clone());
            println!("windows · the keyboard to {app}{}", if self.host_focus { "" } else { " (a program's surface has it for now)" });
            self.write_desktop();
            self.tell(NestEvent::Focused(self.focus));
            for (slot, h) in &self.toplevel_handles {
                if Some(*slot) == before || Some(*slot) == self.focus {
                    h.state(self.states(*slot));
                    h.done();
                }
            }
        }
    }

    /// The surface under a point of the window, in the window's surface
    /// coordinates: a menu first, then the window and its subsurfaces.
    fn surface_under(&self, root: &WlSurface, g: [i32; 4], at: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
        // An X11 window's menus, over it.
        if let Some(slot) = self.window_of(root) {
            for (surface, o) in self.unmanaged_of(slot).iter().rev() {
                if let Some(found) = hit_tree(surface, at, *o) {
                    return Some(found);
                }
            }
        }
        let popups: Vec<(PopupKind, Point<i32, Logical>)> = PopupManager::popups_for_surface(root).collect();
        for (popup, offset) in popups.iter().rev() {
            let origin = Point::<i32, Logical>::from((g[0], g[1])) + *offset - popup.geometry().loc;
            if let Some(found) = hit_tree(popup.wl_surface(), at, (origin.x, origin.y)) {
                return Some(found);
            }
        }
        hit_tree(root, at, (0, 0))
    }

    /// A window, to one who listens: a handle of its own, and all it is.
    fn announce(&mut self, manager: &ZwlrForeignToplevelManagerV1, slot: usize) {
        let Some(Some(w)) = self.slots.get(slot) else { return };
        let Some(client) = manager.client() else { return };
        let Ok(h) = client.create_resource::<ZwlrForeignToplevelHandleV1, _, State>(&self.dh, manager.version(), slot) else { return };
        manager.toplevel(&h);
        h.title(w.title.clone());
        h.app_id(w.app.clone());
        if let Some(o) = self.outputs.get(w.screen) {
            for o in o.client_outputs(&client) {
                h.output_enter(&o);
            }
        }
        h.state(self.states(slot));
        h.done();
        self.toplevel_handles.push((slot, h));
    }

    /// The desktop as it is, for whoever asks without a compositor's own
    /// socket to ask (`pleamar-wm hyprctl`, what Marea's screenshots use):
    /// the monitors, which has the focus, and where the window with the
    /// keyboard is. Only in a session of its own.
    fn write_desktop(&self) {
        let monitors = layers::monitors();
        if monitors.is_empty() {
            return;
        }
        let mut text = String::new();
        for (k, m) in monitors.iter().enumerate() {
            text.push_str(&format!("monitor {} {} {} {} {} {} {} {}\n", m.name, m.size.0, m.size.1, m.x, m.y, m.mhz, (k == self.on_screen) as u8, m.scale));
        }
        if let Some(w) = self.focus.and_then(|s| self.slots.get(s)).and_then(Option::as_ref) {
            if let Some((name, r)) = &w.shown {
                let (x, y) = monitors.iter().find(|m| &m.name == name).map_or((0, 0), |m| (m.x, m.y));
                text.push_str(&format!("window {} {} {} {} {}\t{}\n", r[0] + x, r[1] + y, r[2], r[3], w.app, w.title));
            }
        }
        // Asked on every frame a window with the keyboard moves: written only
        // when it says something new.
        static WRITTEN: Mutex<String> = Mutex::new(String::new());
        let mut written = WRITTEN.lock().unwrap();
        if *written == text {
            return;
        }
        let file = desktop_file_of(&self.socket);
        if let Some(dir) = std::path::Path::new(&file).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, &text);
        *written = text;
    }

    /// A monitor was plugged in or out: the outputs follow, by name. What was
    /// on one that left goes to the first: its windows, and its programs'
    /// surfaces are told they are closed (as layer-shell asks).
    fn monitors_changed(&mut self) {
        let monitors = layers::monitors();
        if monitors.is_empty() {
            return;
        }
        let old_names: Vec<String> = self.outputs.iter().map(|o| o.name()).collect();
        let mut outputs = Vec::new();
        let mut globals = Vec::new();
        for m in &monitors {
            let mode = OutputMode { size: (m.size.0 as i32, m.size.1 as i32).into(), refresh: m.mhz };
            let (o, g) = match old_names.iter().position(|n| *n == m.name) {
                Some(i) => (self.outputs[i].clone(), self.output_globals[i].clone()),
                None => {
                    println!("windows · a monitor for the programs: {}", m.name);
                    let o = Output::new(m.name.clone(), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "pleamar".into(), model: m.name.clone() });
                    let g = o.create_global::<State>(&self.dh);
                    (o, g)
                }
            };
            o.change_current_state(Some(mode), Some(Transform::Normal), Some(output_scale(m.scale)), Some((m.x, m.y).into()));
            o.set_preferred(mode);
            outputs.push(o);
            globals.push(g);
        }
        for (i, name) in old_names.iter().enumerate() {
            if !monitors.iter().any(|m| &m.name == name) {
                println!("windows · the monitor {name} is gone");
                self.dh.remove_global::<State>(self.output_globals[i].clone());
            }
        }
        let new_index = |old: usize| old_names.get(old).and_then(|n| monitors.iter().position(|m| &m.name == n));
        // The windows, by the name of their monitor; the ones on one that left,
        // to the first, remembering it; the ones whose monitor came back, to it.
        // Each with its workspace, whole (`Moved`), not mixed into the one shown.
        // The phone's monitor put up (docs/phone.md): every window goes to it,
        // remembering where it was, as one whose monitor was unplugged does;
        // while it is up they stay there; taken down, they go back.
        let phone = monitors.iter().position(|m| m.name == layers::PHONE_NAME);
        let phone_came = phone.is_some() && !old_names.iter().any(|n| n == layers::PHONE_NAME);
        let mut moved = Vec::new();
        for (slot, w) in self.slots.iter_mut().enumerate() {
            let Some(w) = w else { continue };
            let back = w.home.as_ref().and_then(|h| monitors.iter().position(|m| &m.name == h));
            // One that comes from the phone with no place of its own joins
            // the pool shown where it lands; the rest keep theirs.
            let join = w.home.is_none() && new_index(w.screen).is_none() && old_names.get(w.screen).is_some_and(|n| n == layers::PHONE_NAME);
            let to = match (back, new_index(w.screen)) {
                _ if phone_came => {
                    if w.home.is_none() {
                        w.home = old_names.get(w.screen).cloned();
                    }
                    phone.unwrap_or(0)
                }
                (_, Some(i)) if Some(i) == phone => i,
                (Some(b), _) => {
                    println!("windows · {} back to {}", w.app, monitors[b].name);
                    w.home = None;
                    b
                }
                (None, Some(i)) => i,
                (None, None) => {
                    // Opened on the phone, it has no other place: it stays here.
                    let from_phone = old_names.get(w.screen).is_some_and(|n| n == layers::PHONE_NAME);
                    if w.home.is_none() && !from_phone {
                        w.home = old_names.get(w.screen).cloned();
                    }
                    0
                }
            };
            if to != w.screen || new_index(w.screen).is_none() {
                w.screen = to;
                moved.push((slot, to, w.surface.clone(), join));
            }
        }
        for (slot, to, surface, join) in moved {
            if let Some(o) = outputs.get(to) {
                o.enter(&surface);
            }
            self.tell(if join { NestEvent::Screen(slot, to) } else { NestEvent::Moved(slot, to) });
        }
        // The programs' surfaces: on their monitor by its name; the ones on one that left, closed.
        let mut closed = Vec::new();
        for p in &mut self.panels {
            match new_index(p.monitor) {
                Some(i) => p.monitor = i,
                None => closed.push(p.id),
            }
        }
        for id in &closed {
            if let Some(p) = self.panels.iter().find(|p| p.id == *id) {
                if let Shell::Layer(l) = &p.shell {
                    l.send_close();
                }
            }
            layers::hide(*id);
        }
        self.panels.retain(|p| !closed.contains(&p.id));
        self.outputs = outputs;
        self.output_globals = globals;
        self.on_screen = self.on_screen.min(self.outputs.len() - 1);
        // What opens now opens on the phone, where whoever uses it is.
        if let (true, Some(p)) = (phone_came, phone) {
            self.on_screen = p;
        }
        // Everything shown again where it now is.
        for k in 0..self.panels.len() {
            self.panels[k].configured = None;
            self.configure_panel(k);
            self.show_panel(k);
        }
        self.reserved.clear();
        self.tell_reserved();
        self.write_desktop();
    }

    /// A picture taken: into the program's memory, and told it is ready.
    fn hand_picture(&mut self, id: u64, pixels: Option<Vec<u8>>) {
        // One the portal asked for (sharing the screen): its, not a program's.
        if crate::portal::deliver(id, pixels.clone()) {
            return;
        }
        let Some(p) = self.pictures.remove(&id) else { return };
        let (w, h) = (p.piece[2] as usize, p.piece[3] as usize);
        if p.pointer {
            layers::unwait_pointer(id);
        }
        let written = match (&p.buffer, pixels) {
            (Some(buffer), Some(mut px)) if px.len() >= w * h * 4 => with_buffer_contents_mut(buffer, |ptr, len, d| {
                if p.pointer {
                    layers::pointer_onto_piece(p.monitor, p.piece, &mut px);
                }
                let (stride, offset) = (d.stride.max(0) as usize, d.offset.max(0) as usize);
                if offset + stride * h > len || stride < w * 4 {
                    return false;
                }
                // SAFETY: the program's pool, `len` bytes from `ptr`, and the range was checked.
                let dst = unsafe { std::slice::from_raw_parts_mut(ptr.add(offset), stride * h) };
                for y in 0..h {
                    dst[y * stride..y * stride + w * 4].copy_from_slice(&px[y * w * 4..(y + 1) * w * 4]);
                }
                true
            })
            .unwrap_or(false),
            _ => false,
        };
        if !written {
            p.frame.failed();
            return;
        }
        p.frame.flags(copy_frame::Flags::empty());
        if p.damage && p.frame.version() >= 2 {
            p.frame.damage(0, 0, w as u32, h as u32);
        }
        // On the presentation clock, CLOCK_MONOTONIC, as the protocol asks: a
        // recorder (wf-recorder) times its frames by it, and with the wall
        // clock a ten-second video said it lasted fifty-six years.
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a valid timespec for the call to fill.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        let secs = ts.tv_sec as u64;
        p.frame.ready((secs >> 32) as u32, secs as u32, ts.tv_nsec as u32);
        if crate::screen::capture_debug() {
            eprintln!("capture · {:.1} handed to the program", crate::screen::wall_ms());
        }
    }

    /// A program's buffer held for long is a program that cannot draw again
    /// (with three buffers, a browser freezes): said once, with whose it is.
    fn tell_held(&mut self) {
        for (n, l) in self.lent.iter_mut().filter(|(_, l)| !l.told && l.since.elapsed() > Duration::from_secs(2)) {
            // The picture a program's surface is showing is read by its monitor
            // for as long as it is shown: held, and rightly.
            let shown = l.surface.alive() && with_states(&l.surface, |s| s.data_map.get::<Mutex<Content>>().and_then(|c| c.lock().unwrap().dmabuf.as_ref().map(|(k, _)| k == n))).unwrap_or(false);
            if shown && !l.what.starts_with("window") {
                // Its time counts from when it stops being shown.
                l.since = std::time::Instant::now();
                continue;
            }
            l.told = true;
            let who = l.buffer.client().and_then(|c| c.get_credentials(&self.dh).ok()).map_or(0, |c| c.pid);
            eprintln!("windows · buffer {n} of {} (pid {who}) held for {:.1} s{}: not handed back", l.what, l.since.elapsed().as_secs_f32(), if l.release.is_some() { " with a release point" } else { "" });
        }
    }

    fn release(&mut self, numbers: Vec<u64>) {
        for n in numbers {
            if let Some(b) = self.lent.remove(&n) {
                if b.told {
                    eprintln!("windows · buffer {n} of {} handed back after {:.1} s", b.what, b.since.elapsed().as_secs_f32());
                }
                b.give_back();
            }
        }
    }

    /// What the session and the monitors say about the programs' surfaces.
    fn layer_input(&mut self, m: ToLayers) {
        if std::env::var_os("PLEAMAR_DEBUG_WINDOWS").is_some() && !matches!(m, ToLayers::FrameDone(_) | ToLayers::Released(_)) {
            eprintln!("windows · {m:?}");
        }
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.time();
        match m {
            ToLayers::Pointer { id, x, y } => {
                let panel = self.panels.iter().find(|p| p.id == id).map(|p| (p.shell.wl_surface().clone(), p.monitor));
                let Some((root, monitor)) = panel.or_else(|| self.menus.iter().find(|m| m.id == id).map(|m| (m.surface.clone(), m.monitor))) else { return };
                let s = self.scale_of(monitor);
                let at: Point<f64, Logical> = (x / s, y / s).into();
                let under = self.surface_under(&root, [0, 0, 0, 0], at).map(|(s, o)| (s, o.to_f64()));
                self.pointer_on = None;
                self.panel_pointer = Some(id);
                self.last_under = under.clone();
                let p = self.pointer.clone();
                p.motion(self, under, &MotionEvent { location: at, serial, time });
                p.frame(self);
                self.update_constraint();
            }
            ToLayers::PointerOut => {
                if self.panel_pointer.take().is_some() {
                    self.last_under = None;
                    let p = self.pointer.clone();
                    p.motion(self, None, &MotionEvent { location: (0.0, 0.0).into(), serial, time });
                    p.frame(self);
                    self.update_constraint();
                }
            }
            ToLayers::Button { code, down } => {
                if down {
                    if let Some(root) = self.panel_pointer.and_then(|id| self.panels.iter().find(|p| p.id == id)).map(|p| p.shell.wl_surface().clone()) {
                        self.dismiss_popups_not_under(&root);
                    }
                    // On another program's surface, not on a menu: the windows' menus close.
                    if !self.panel_pointer.is_some_and(|id| self.menus.iter().any(|m| m.id == id)) {
                        self.dismiss_all_popups();
                    }
                }
                let p = self.pointer.clone();
                let state = if down { smithay::backend::input::ButtonState::Pressed } else { smithay::backend::input::ButtonState::Released };
                p.button(self, &ButtonEvent { serial, time, button: code, state });
                p.frame(self);
            }
            ToLayers::Wheel(dy) => self.handle(ToNest::Wheel(dy as f64)),
            ToLayers::Key { id, code, down } => {
                // Let go, as with a window: heard, whoever has the keyboard, and
                // without taking it for a surface that may be gone.
                if self.panel_keyboard != Some(id) && down {
                    let Some(surface) = self.panels.iter().find(|p| p.id == id).map(|p| p.shell.wl_surface().clone()) else { return };
                    self.panel_keyboard = Some(id);
                    self.exclusive_keyboard = false;
                    let k = self.keyboard.clone();
                    k.set_focus(self, Some(surface), serial);
                }
                let k = self.keyboard.clone();
                let state = if down { smithay::backend::input::KeyState::Pressed } else { smithay::backend::input::KeyState::Released };
                k.input::<(), _>(self, (code + 8).into(), state, serial, time, |_, _, _| FilterResult::Forward);
            }
            ToLayers::KeyboardBack => {
                if self.panel_keyboard.take().is_some() {
                    self.set_focus(self.focus);
                }
            }
            ToLayers::FrameDone(name) => {
                let monitor = layers::monitors().iter().position(|m| m.name == name);
                self.frame_done(monitor);
            }
            ToLayers::Launch(command) => {
                self.launch(&command);
            }
            ToLayers::Released(numbers) => self.release(numbers),
            ToLayers::Monitors => self.monitors_changed(),
            ToLayers::Captured { id, pixels } => self.hand_picture(id, pixels),
            ToLayers::Relative { dx, dy, ux, uy, utime } => {
                // A hold its program let go of (it unlocked): the pointer is free again.
                if let Some(s) = self.constrained.clone() {
                    if !s.alive() || !with_pointer_constraint(&s, &self.pointer, |c| c.is_some_and(|c| c.is_active())) {
                        self.constrained = None;
                        layers::set_pointer_hold(None);
                    }
                }
                let focus = self.last_under.clone().filter(|(s, _)| s.alive() && self.pointer.current_focus().as_ref() == Some(s));
                if focus.is_some() {
                    let p = self.pointer.clone();
                    p.relative_motion(self, focus, &RelativeMotionEvent { delta: (dx, dy).into(), delta_unaccel: (ux, uy).into(), utime });
                    p.frame(self);
                }
            }
            ToLayers::Activity => self.activity(),
            ToLayers::ScenePress => self.dismiss_all_popups(),
            ToLayers::Power => {
                self.powers.retain(|(p, _)| p.is_alive());
                for (p, k) in &self.powers {
                    p.mode(if layers::powered(*k) { output_power::Mode::On } else { output_power::Mode::Off });
                }
            }
        }
    }

    /// Someone touched something: whoever watches for idleness is told (not
    /// more than a few times a second).
    fn activity(&mut self) {
        if self.last_activity.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_activity = Instant::now();
        let seat = self.seat.clone();
        self.idle.notify_activity(&seat);
    }

    /// A program that asked to hold the pointer gets it while the pointer is
    /// on its surface, and lets go of it when it leaves; the session is told
    /// to keep the pointer still or inside that window.
    fn update_constraint(&mut self) {
        let focus = self.pointer.current_focus();
        if let Some(s) = self.constrained.clone() {
            if focus.as_ref() != Some(&s) {
                if s.alive() {
                    with_pointer_constraint(&s, &self.pointer, |c| {
                        if let Some(c) = c {
                            c.deactivate();
                        }
                    });
                }
                self.constrained = None;
                layers::set_pointer_hold(None);
            }
        }
        let Some(f) = focus else { return };
        if self.constrained.as_ref() == Some(&f) {
            return;
        }
        let locked = with_pointer_constraint(&f, &self.pointer, |c| {
            let c = c?;
            if !c.is_active() {
                c.activate();
            }
            Some(matches!(&*c, PointerConstraint::Locked(_)))
        });
        let Some(locked) = locked else { return };
        self.constrained = Some(f);
        let monitors = layers::monitors();
        let rect = self.pointer_on.and_then(|s| self.slots.get(s)).and_then(Option::as_ref).and_then(|w| w.shown.clone()).and_then(|(name, r)| {
            monitors.iter().find(|m| m.name == name).map(|m| [r[0] + m.x, r[1] + m.y, r[2], r[3]])
        });
        layers::set_pointer_hold(Some(if locked { layers::Hold::Locked } else { layers::Hold::Confined(rect) }));
    }

    /// How many pixels a unit is on that monitor.
    fn scale_of(&self, k: usize) -> f64 {
        self.outputs.get(k).map_or(1.0, |o| o.current_scale().fractional_scale())
    }

    /// A monitor's size in pixels (its mode).
    fn monitor_pixels(&self, k: usize) -> (i32, i32) {
        self.outputs.get(k).and_then(|o| o.current_mode()).map_or((1280, 800), |m| (m.size.w, m.size.h))
    }

    /// A monitor's name (`DP-3`, `PHONE-1`).
    fn monitor_name(&self, k: usize) -> Option<String> {
        self.outputs.get(k).map(|o| o.name())
    }

    /// A monitor's size in units: what programs lay themselves out in.
    fn monitor_size(&self, k: usize) -> (i32, i32) {
        let (w, h) = self.monitor_pixels(k);
        let s = self.scale_of(k);
        ((w as f64 / s).round() as i32, (h as f64 / s).round() as i32)
    }

    /// A program's surface is told its size whenever what it asks for changes:
    /// what it asks, or, where it asks for 0, all the monitor between its edges.
    fn configure_panel(&mut self, k: usize) {
        let (mw, mh) = self.monitor_size(self.panels[k].monitor);
        let p = &mut self.panels[k];
        let Shell::Layer(layer) = &p.shell else { return };
        let c = with_states(layer.wl_surface(), |s| *s.cached_state.get::<LayerSurfaceCachedState>().current());
        let m = c.margin;
        let w = if c.size.w == 0 { mw - m.left - m.right } else { c.size.w };
        let h = if c.size.h == 0 { mh - m.top - m.bottom } else { c.size.h };
        let want = (w.max(1), h.max(1));
        if p.configured != Some(want) {
            p.configured = Some(want);
            layer.with_pending_state(|s| s.size = Some(want.into()));
            layer.send_configure();
        }
    }

    /// A program's surface of that size (a program may have several: Marea's
    /// card, and what catches a click outside it), and where it is: its
    /// monitor and its corner there, in units. What an agent's hand reaches
    /// when the program has no window.
    pub(super) fn panel_of(&self, pid: u32, size: (i32, i32)) -> Option<(usize, WlSurface, usize, (i32, i32))> {
        self.panels.iter().enumerate().find_map(|(k, p)| {
            let root = p.shell.wl_surface().clone();
            let own = root.client().and_then(|c| c.get_credentials(&self.dh).ok()).is_some_and(|c| c.pid as u32 == pid);
            let s = content_of(&root, |c| c.size).unwrap_or((0, 0));
            if !own || (s.0 as i32, s.1 as i32) != size || matches!(p.shell, Shell::Lock(_)) {
                return None;
            }
            let c = with_states(&root, |s| *s.cached_state.get::<LayerSurfaceCachedState>().current());
            Some((k, root, p.monitor, panel_place(&c, self.monitor_size(p.monitor), size)))
        })
    }

    /// What a program's surface shows, to its monitor: where, at what level,
    /// and its pieces (it, its subsurfaces and its menus).
    fn show_panel(&mut self, k: usize) {
        let root = self.panels[k].shell.wl_surface().clone();
        let (mw, mh) = self.monitor_size(self.panels[k].monitor);
        let c = with_states(&root, |s| *s.cached_state.get::<LayerSurfaceCachedState>().current());
        let size = content_of(&root, |c| c.size).unwrap_or((0, 0));
        let sent = self.panels[k].sent.clone();
        // What the monitor is given is in its pixels: the program's units, scaled.
        let scale = self.scale_of(self.panels[k].monitor);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let mut pieces = Vec::new();
        if size.0 > 0 && size.1 > 0 {
            panel_pieces(&root, (0, 0), &sent, scale, &mut pieces);
            for (popup, offset) in PopupManager::popups_for_surface(&root) {
                let origin = offset - popup.geometry().loc;
                panel_pieces(popup.wl_surface(), (origin.x, origin.y), &sent, scale, &mut pieces);
            }
        }
        self.panels[k].sent = pieces.iter().map(|p| p.key).collect();
        let (w, h) = (size.0 as i32, size.1 as i32);
        let id = self.panels[k].id;
        // A lock screen: all of its monitor, over everything, with the keyboard.
        if matches!(self.panels[k].shell, Shell::Lock(_)) {
            layers::show(self.panels[k].monitor, ClientLayer { id, level: 4, rect: [0, 0, px(w), px(h)], pieces, region: None, keyboard: 1, blur: Vec::new(), owner: owner_of(&root), zoom: 1.0 });
            return;
        }
        // On the phone, a panel wider than it is shown smaller (Marea: her
        // card fits); placed by the size it is shown at.
        let zoom = phone_zoom(self.monitor_name(self.panels[k].monitor).as_deref(), mw, w);
        let (zw, zh) = ((w as f64 * zoom).round() as i32, (h as f64 * zoom).round() as i32);
        let (x, y) = panel_place(&c, (mw, mh), (zw, zh));
        let region = with_states(&root, |s| {
            s.cached_state.get::<SurfaceAttributes>().current().input_region.as_ref().map(|r| {
                r.rects.iter().map(|(kind, rect)| (matches!(kind, RectangleKind::Add), [px(rect.loc.x), px(rect.loc.y), px(rect.size.w), px(rect.size.h)])).collect()
            })
        });
        let level = match c.layer {
            ShellLayer::Background => 0,
            ShellLayer::Bottom => 1,
            ShellLayer::Top => 2,
            ShellLayer::Overlay => 3,
        };
        let keyboard = match c.keyboard_interactivity {
            KeyboardInteractivity::None => 0,
            KeyboardInteractivity::Exclusive => 1,
            KeyboardInteractivity::OnDemand => 2,
        };
        // Asking for all of the keyboard (Marea's finder as it opens), it is
        // given at once, not with the first key: the program is told it has
        // it, and puts its cursor in its field. Letting go, it goes back to
        // the window that had it.
        let visible = w > 0 && h > 0;
        if keyboard == 1 && visible && self.panel_keyboard != Some(id) {
            self.panel_keyboard = Some(id);
            self.exclusive_keyboard = true;
            let k = self.keyboard.clone();
            k.set_focus(self, Some(root.clone()), SERIAL_COUNTER.next_serial());
        } else if (keyboard != 1 || !visible) && self.panel_keyboard == Some(id) && self.exclusive_keyboard {
            self.panel_keyboard = None;
            self.exclusive_keyboard = false;
            self.set_focus(self.focus);
        }
        let blur: Vec<[i32; 4]> = with_states(&root, |s| s.data_map.get::<Blur>().map(|b| b.0.lock().unwrap().clone())).unwrap_or_default().iter().map(|b| [px(b[0]), px(b[1]), px(b[2]), px(b[3])]).collect();
        if std::env::var_os("PLEAMAR_DEBUG_WINDOWS").is_some() {
            eprintln!("windows · surface {id}: level {level}, keyboard {keyboard}, {}×{} at {x},{y}", w, h);
        }
        layers::show(self.panels[k].monitor, ClientLayer { id, level, rect: [px(x), px(y), px(zw), px(zh)], pieces, region, keyboard, blur, owner: owner_of(&root), zoom });
    }

    /// What the programs' bars keep at each edge of each monitor (their
    /// exclusive zones, and their margin on that edge): the scene lays the
    /// windows out around it. Told when it changes.
    fn tell_reserved(&mut self) {
        let mut now = vec![[0f32; 4]; self.outputs.len()];
        for p in &self.panels {
            let Shell::Layer(layer) = &p.shell else { continue };
            if content_of(layer.wl_surface(), |c| c.size == (0, 0)).unwrap_or(true) {
                continue;
            }
            let c = with_states(layer.wl_surface(), |s| *s.cached_state.get::<LayerSurfaceCachedState>().current());
            let ExclusiveZone::Exclusive(zone) = c.exclusive_zone else { continue };
            let (t, r, b, l) = (c.anchor.contains(Anchor::TOP), c.anchor.contains(Anchor::RIGHT), c.anchor.contains(Anchor::BOTTOM), c.anchor.contains(Anchor::LEFT));
            // One edge, or an edge and both of its sides: which one it keeps.
            let edge = match (t, r, b, l) {
                (true, _, false, _) if r == l || (r && l) => Some((0, c.margin.top)),
                (false, _, true, _) if r == l || (r && l) => Some((2, c.margin.bottom)),
                (_, true, _, false) if t == b || (t && b) => Some((1, c.margin.right)),
                (_, false, _, true) if t == b || (t && b) => Some((3, c.margin.left)),
                _ => None,
            };
            if let (Some((e, margin)), Some(m)) = (edge, now.get_mut(p.monitor)) {
                m[e] += zone as f32 + margin.max(0) as f32;
            }
        }
        for (k, edges) in now.iter().enumerate() {
            if self.reserved.get(k) != Some(edges) {
                self.tell(NestEvent::Reserved(k, *edges));
            }
        }
        self.reserved = now;
    }

    fn dismiss_popups_not_under(&mut self, root: &WlSurface) {
        let root = root.clone();
        let focused = self.pointer.current_focus();
        for (popup, _) in PopupManager::popups_for_surface(&root) {
            let inside = focused.as_ref().is_some_and(|f| {
                let mut s = f.clone();
                loop {
                    if &s == popup.wl_surface() {
                        break true;
                    }
                    match get_parent(&s) {
                        Some(p) => s = p,
                        None => break false,
                    }
                }
            });
            if !inside {
                if let PopupKind::Xdg(p) = &popup {
                    p.send_popup_done();
                }
            }
        }
    }

    /// A monitor was shown (`Some`), or the scene painted (`None`): the
    /// windows may draw again, and the programs' surfaces on that monitor.
    fn frame_done(&mut self, monitor: Option<usize>) {
        let t = self.time();
        self.callbacks.retain(|(cb, on)| {
            let now = on.is_none() || *on == monitor;
            if now {
                cb.done(t);
            }
            !now
        });
        self.presentation_done();
    }

    /// All of them: nothing has been shown for a while (see the loop).
    fn frame_done_all(&mut self) {
        let t = self.time();
        for (cb, _) in self.callbacks.drain(..) {
            cb.done(t);
        }
        self.presentation_done();
    }

    fn presentation_done(&mut self) {
        // When they were shown (presentation-time): now, on their monitor.
        if !self.presented.is_empty() {
            let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            // SAFETY: a valid timespec for the call to fill.
            unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
            let now = Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32);
            self.presented_seq += 1;
            for (cb, monitor) in std::mem::take(&mut self.presented) {
                let Some(output) = self.outputs.get(monitor).or(self.outputs.first()) else { continue };
                let mhz = output.current_mode().map_or(60_000, |m| m.refresh.max(1));
                let refresh = Refresh::fixed(Duration::from_nanos(1_000_000_000_000 / mhz as u64));
                cb.presented(output, now, refresh, self.presented_seq, wp_presentation_feedback::Kind::Vsync);
            }
        }
    }

    /// A slot for a new window, if there is one free: turning round from the last one given.
    fn place(&mut self, toplevel: Toplevel, surface: WlSurface) {
        let n = self.slots.len();
        let Some(slot) = (0..n).map(|k| (self.next_slot + k) % n).find(|k| self.slots[*k].is_none()) else {
            self.waiting.push((toplevel, surface));
            return;
        };
        self.next_slot = (slot + 1) % n.max(1);
        let (title, app) = match &toplevel {
            Toplevel::Xdg(_) => with_states(&surface, |s| {
                let d = s.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap());
                d.map_or((String::new(), String::new()), |d| (d.title.clone().unwrap_or_default(), d.app_id.clone().unwrap_or_default()))
            }),
            Toplevel::X11(x) => (x.title(), x.class()),
        };
        // Where you are; or, if it is a program the agent is working with
        // that opened it, where the agent is.
        let pid = match &toplevel {
            Toplevel::X11(x) => x.pid().unwrap_or(0),
            Toplevel::Xdg(_) => surface.client().and_then(|c| c.get_credentials(&self.dh).ok()).map_or(0, |c| c.pid as u32),
        };
        // One the agent opened (`agent open`): on its monitor, and yours
        // stays where it was.
        let opened = self.agent_opened(pid, &app).filter(|s| *s < self.outputs.len());
        let screen = opened.or_else(|| self.agent_screen_for(pid)).filter(|s| *s < self.outputs.len()).unwrap_or(self.on_screen);
        // Its size, the one the scene already has for that slot; tiled on all
        // sides, so it does not draw a shadow or round corners of its own: the
        // scene decides how it looks.
        let size = self.asked[slot];
        match &toplevel {
            Toplevel::Xdg(t) => t.with_pending_state(|s| {
                // `0 × 0` is «the one it chooses», not 1 × 1 (which froze browsers).
                s.size = size.filter(|(w, h)| *w >= 32 && *h >= 32).map(|(w, h)| (w, h).into());
                for t in [xdg_toplevel::State::TiledLeft, xdg_toplevel::State::TiledRight, xdg_toplevel::State::TiledTop, xdg_toplevel::State::TiledBottom] {
                    s.states.set(t);
                }
            }),
            Toplevel::X11(_) => {
                if let Some((w, h)) = size {
                    toplevel.resize(w, h);
                }
            }
        }
        self.outputs[screen.min(self.outputs.len() - 1)].enter(&surface);
        let listed = self.toplevel_list.new_toplevel::<State>(title.clone(), app.clone());
        self.slots[slot] = Some(Window { toplevel, surface, title: title.clone(), app: app.clone(), geometry: [0, 0, 0, 0], sent: Vec::new(), screen, shown: None, listed, fullscreen: false, dialog: false, floating: false, ruled: None, minimized: false, was_at: 0, floated: false, home: None });
        self.order.push(slot);
        layers::set_private(slot, false);
        let program = (!app.is_empty()).then(|| crate::desktop::program(&app));
        self.tell(NestEvent::Opened { slot, title, app, screen });
        // Whether it lets the scene draw its frame: X11 programs do (the scene
        // is their window manager); a Wayland one, if it took xdg-decoration.
        let framed = self.slots[slot].as_ref().is_some_and(|w| matches!(w.toplevel, Toplevel::X11(_)) || self.decorated.contains(&w.surface));
        self.tell(NestEvent::Framed(slot, framed));
        if let Some((icon, name, exec)) = program {
            self.tell(NestEvent::Program { slot, icon, name, exec });
        }
        self.tell(NestEvent::Order(self.order.clone()));
        self.apply_rules(slot);
        for m in self.toplevel_managers.clone() {
            self.announce(&m, slot);
        }
        // X11 says whose it is before it is shown (Wayland, later: `parent_changed`).
        self.beside_parent(slot);
        // Not one the agent opened, nor one of the program it is working with
        // while your keyboard is in another (a dialog its click opened).
        let parent_pid = self.parent_slot(slot).map_or(0, |p| self.pid_of(p));
        if opened.is_none() && !self.agent_keeps_your_keyboard(pid) && !self.agent_keeps_your_keyboard(parent_pid) {
            let before = self.focus;
            self.set_focus(Some(slot));
            self.focus_given = Some((slot, before, Instant::now()));
        }
    }

    fn forget(&mut self, toplevel: &Toplevel) {
        self.waiting.retain(|(t, _)| t != toplevel);
        let Some(slot) = self.slots.iter().position(|w| w.as_ref().is_some_and(|w| &w.toplevel == toplevel)) else { return };
        // Where the keyboard goes back to, if it had it: the window that opened
        // it (a «Save image» asked by a viewer), or one fullscreen on its
        // monitor. The one that leads was a window the user was not looking at.
        let parent = match toplevel {
            Toplevel::Xdg(t) => t.parent(),
            Toplevel::X11(_) => None,
        };
        // Else one that is seen on the same monitor, as when one is put away:
        // the one that leads may be on another workspace or monitor, and it
        // had the keys unseen.
        let monitor = layers::shown(slot).map(|(m, _)| m);
        let back_to = parent.and_then(|p| self.window_of(&p)).filter(|s| *s != slot).or_else(|| {
            let screen = self.slots[slot].as_ref().map(|w| w.screen);
            self.order.iter().copied().find(|s| *s != slot && self.slots[*s].as_ref().is_some_and(|w| w.fullscreen && Some(w.screen) == screen))
        }).or_else(|| self.order.iter().copied().find(|s| *s != slot && monitor.is_some() && layers::shown(*s).is_some_and(|(m, _)| Some(&m) == monitor.as_ref())));
        if let Some(w) = self.slots[slot].take() {
            self.thumb_closed(&w.listed.identifier());
            self.toplevel_list.remove_toplevel(&w.listed);
            if w.fullscreen {
                self.tell_fullscreen();
            }
        }
        self.order.retain(|s| *s != slot);
        for (_, h) in self.toplevel_handles.iter().filter(|(s, _)| *s == slot) {
            h.closed();
        }
        self.toplevel_handles.retain(|(s, _)| *s != slot);
        if self.pointer_on == Some(slot) {
            self.pointer_on = None;
        }
        if self.urgent.remove(&slot) {
            self.tell(NestEvent::Urgent(slot, false));
        }
        self.tell(NestEvent::Closed(slot));
        self.tell(NestEvent::Order(self.order.clone()));
        if self.focus == Some(slot) {
            // Nothing seen there: the one that leads (a window closed with no
            // monitor known, headless or nested).
            let next = back_to.or_else(|| if monitor.is_none() { self.order.first().copied() } else { None });
            self.set_focus(next);
        }
        // One that was waiting takes its place.
        if !self.waiting.is_empty() {
            let (t, s) = self.waiting.remove(0);
            self.place(t, s);
        }
    }

    /// Whether windows' menus are surfaces of the monitor (a session of its
    /// own, with monitors) or pieces of their window (nested).
    fn menus_apart(&self) -> bool {
        !layers::monitors().is_empty()
    }

    /// A window's menus, over everything on the monitor it is seen on, where
    /// the scene shows it; the ones that closed, taken away.
    fn show_menus(&mut self, slot: usize, root: &WlSurface, g: [i32; 4]) {
        let monitors = layers::monitors();
        let place = self.slots.get(slot).and_then(Option::as_ref).and_then(|w| w.shown.clone()).and_then(|(name, r)| monitors.iter().position(|m| m.name == name).map(|k| (k, r)));
        let mut now = Vec::new();
        if let Some((k, r)) = place {
            let scale = self.scale_of(k);
            let px = |v: f64| (v * scale).round() as i32;
            // As much as the scene scales the window (at a glance, while it glides).
            let sx = if g[2] > 0 { r[2] as f64 / g[2] as f64 } else { 1.0 };
            let sy = if g[3] > 0 { r[3] as f64 / g[3] as f64 } else { 1.0 };
            // Its menus: Wayland ones (xdg popups) and X11 ones (override-redirect),
            // each from the window's corner.
            let mut all: Vec<(WlSurface, Point<i32, Logical>)> = PopupManager::popups_for_surface(root).map(|(p, offset)| (p.wl_surface().clone(), offset - p.geometry().loc)).collect();
            all.extend(self.unmanaged_of(slot).into_iter().map(|(s, at)| (s, Point::from(at))));
            for (surface, at) in all {
                let Some((w, h)) = content_of(&surface, |c| c.size).filter(|s| s.0 > 0 && s.1 > 0) else { continue };
                let (x, y) = (r[0] as f64 + at.x as f64 * sx, r[1] as f64 + at.y as f64 * sy);
                let i = match self.menus.iter().position(|m| m.surface == surface) {
                    Some(i) => i,
                    None => {
                        self.next_number += 1;
                        self.menus.push(Menu { id: self.next_number, surface: surface.clone(), root: root.clone(), monitor: k, sent: Vec::new() });
                        self.menus.len() - 1
                    }
                };
                // Moved to another monitor: gone from the one it was on.
                if self.menus[i].monitor != k {
                    layers::hide(self.menus[i].id);
                    self.menus[i].monitor = k;
                    self.menus[i].sent.clear();
                }
                let mut pieces = Vec::new();
                panel_pieces(&surface, (0, 0), &self.menus[i].sent, scale, &mut pieces);
                self.menus[i].sent = pieces.iter().map(|p| p.key).collect();
                let blur: Vec<[i32; 4]> = with_states(&surface, |s| s.data_map.get::<Blur>().map(|b| b.0.lock().unwrap().clone())).unwrap_or_default().iter().map(|b| [px(b[0] as f64), px(b[1] as f64), px(b[2] as f64), px(b[3] as f64)]).collect();
                let id = self.menus[i].id;
                layers::show(k, ClientLayer { id, level: 3, rect: [px(x), px(y), px(w as f64), px(h as f64)], pieces, region: None, keyboard: 0, blur, owner: owner_of(&surface), zoom: 1.0 });
                now.push(id);
            }
        }
        self.menus.retain(|m| {
            let keep = &m.root != root || now.contains(&m.id);
            if !keep {
                layers::hide(m.id);
            }
            keep
        });
    }

    /// Menus whose surface or window went: off their monitor.
    fn prune_menus(&mut self) {
        self.menus.retain(|m| {
            let keep = m.surface.alive()
                && m.root.alive()
                && (PopupManager::popups_for_surface(&m.root).any(|(p, _)| p.wl_surface() == &m.surface) || self.unmanaged.iter().any(|x| x.wl_surface().as_ref() == Some(&m.surface)));
            if !keep {
                layers::hide(m.id);
            }
            keep
        });
    }

    /// The drag icon, on each monitor (only the one with the pointer shows it
    /// where it can be seen): it takes no pointer, so what is under it does.
    fn show_drag_icon(&mut self) {
        let Some(icon) = self.drag.as_ref().and_then(|d| d.icon.clone()) else { return };
        let size = content_of(&icon, |c| c.size);
        if std::env::var_os("PLEAMAR_DEBUG_WINDOWS").is_some() {
            eprintln!("windows · the drag icon: {size:?}");
        }
        let Some((w, h)) = size.filter(|s| s.0 > 0 && s.1 > 0) else { return };
        let scales: Vec<f64> = (0..self.outputs.len()).map(|k| self.scale_of(k)).collect();
        let owner = owner_of(&icon);
        let Some(d) = self.drag.as_mut() else { return };
        for (k, id, sent) in d.ids.iter_mut() {
            let s = scales.get(*k).copied().unwrap_or(1.0);
            let mut pieces = Vec::new();
            panel_pieces(&icon, (0, 0), sent, s, &mut pieces);
            *sent = pieces.iter().map(|p| p.key).collect();
            let rect = layers::drag_rect(*k, (w as f64 * s) as i32, (h as f64 * s) as i32);
            layers::show(*k, ClientLayer { id: *id, level: 3, rect, pieces, region: Some(Vec::new()), keyboard: 0, blur: Vec::new(), owner, zoom: 1.0 });
        }
    }

    /// Every window's menus close unless the pointer is in them: a press
    /// somewhere else (the scene, another program's surface).
    fn dismiss_all_popups(&mut self) {
        let roots: Vec<WlSurface> = self.slots.iter().flatten().map(|w| w.surface.clone()).collect();
        for root in roots {
            self.dismiss_popups_not_under(&root);
        }
    }

    /// What every window touched in this round shows: its surface, its
    /// subsurfaces and its menus, each one a piece of its own, in order.
    fn compose_dirty(&mut self) {
        let dirty = std::mem::take(&mut self.dirty);
        let mut done: Vec<WlSurface> = Vec::new();
        for root in dirty {
            if done.contains(&root) || !root.alive() {
                continue;
            }
            done.push(root.clone());
            if let Some(k) = self.panels.iter().position(|p| p.shell.wl_surface() == &root) {
                self.show_panel(k);
                self.tell_reserved();
                continue;
            }
            let Some(slot) = self.window_of(&root) else { continue };
            // It may say it is a dialog only now (a parent, a fixed size).
            if let Some(dialog) = self.slots[slot].as_ref().map(|w| w.floating || w.toplevel.is_dialog(&root)) {
                self.set_dialog(slot, dialog);
            }
            let Some((w, h)) = content_of(&root, |c| c.size) else { continue };
            if w == 0 || h == 0 {
                continue;
            }
            let g = with_states(&root, |s| s.cached_state.get::<SurfaceCachedState>().current().geometry);
            let g = g.map_or([0, 0, w as i32, h as i32], |r| [r.loc.x, r.loc.y, r.size.w, r.size.h]);
            let sent = self.slots[slot].as_ref().map(|w| w.sent.clone()).unwrap_or_default();
            let mut pieces = Vec::new();
            pieces_of(&root, (0, 0), &sent, &mut pieces);
            if self.menus_apart() {
                self.show_menus(slot, &root, g);
            } else {
                for (popup, offset) in PopupManager::popups_for_surface(&root) {
                    let origin = Point::<i32, Logical>::from((g[0], g[1])) + offset - popup.geometry().loc;
                    pieces_of(popup.wl_surface(), (origin.x, origin.y), &sent, &mut pieces);
                }
            }
            if !self.menus_apart() {
                for (surface, at) in self.unmanaged_of(slot) {
                    pieces_of(&surface, at, &sent, &mut pieces);
                }
            }
            if let Some(Some(win)) = self.slots.get_mut(slot) {
                win.geometry = g;
                win.sent = pieces.iter().map(|p| p.id).collect();
            }
            if std::env::var_os("PLEAMAR_DEBUG_PIECES").is_some() {
                let what: Vec<String> = pieces
                    .iter()
                    .map(|p| {
                        let kind = match &p.content {
                            PieceContent::Kept => "kept".to_owned(),
                            PieceContent::Pixels(_) => "pixels".to_owned(),
                            PieceContent::Dmabuf(d) => format!("{}·{}planes", String::from_utf8_lossy(&d.fourcc.to_le_bytes()), d.planes.len()),
                        };
                        format!("{} at {:?} {:?} px {:?} src {:?} {kind}", p.id, p.at, p.size, p.px, p.src)
                    })
                    .collect();
                eprintln!("windows · slot {slot} {g:?}: {}", what.join(" | "));
            }
            self.tell(NestEvent::Frame { slot, geometry: g, pieces });
        }
    }
}

/// Where a menu goes so that it fits where it can be seen: flipped, slid or
/// shrunk, as the program allows. In a session of its own, the monitor its
/// window is on (menus are drawn over everything there); nested, the window
/// itself (they are drawn inside it).
fn unconstrained(state: &State, popup: &PopupSurface, positioner: PositionerState) -> smithay::utils::Rectangle<i32, Logical> {
    let kind = PopupKind::Xdg(popup.clone());
    let Ok(root) = smithay::desktop::find_popup_root_surface(&kind) else { return positioner.get_geometry() };
    let window = with_states(&root, |s| s.cached_state.get::<SurfaceCachedState>().current().geometry);
    // In the coordinates of the popup's parent.
    let parent = smithay::desktop::get_popup_toplevel_coords(&kind);
    if state.menus_apart() {
        let monitors = layers::monitors();
        let shown = state.window_of(&root).and_then(|s| state.slots[s].as_ref()).and_then(|w| w.shown.clone());
        if let Some((k, r)) = shown.and_then(|(name, r)| monitors.iter().position(|m| m.name == name).map(|k| (k, r))) {
            let (mw, mh) = state.monitor_size(k);
            let target = smithay::utils::Rectangle::new((-r[0] - parent.x, -r[1] - parent.y).into(), (mw, mh).into());
            return positioner.get_unconstrained_geometry(target);
        }
    }
    let Some(window) = window else { return positioner.get_geometry() };
    let target = smithay::utils::Rectangle::new((-parent.x, -parent.y).into(), window.size);
    positioner.get_unconstrained_geometry(target)
}

/// What the session started (each a process group): the programs go with it.
static LAUNCHED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Closes what the session started, as it leaves: without their compositor
/// they have nowhere to show themselves, and some do not notice —a browser
/// went on animating a page for nobody, at a quarter of a core, long after—.
/// Asked first; whatever is still there a moment later, made to.
/// Every program that ever connected here, by its process: the session asks
/// them all to close when it ends, not only the ones it launched itself —what
/// Marea opens is detached, and a browser that lost its screen stayed alive,
/// unseen, holding its profile («Zen is already running»)—.
static CLIENTS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn remember_client(stream: &std::os::unix::net::UnixStream) {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED fills a ucred of that size on a connected Unix socket.
    let ok = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) } == 0;
    if ok && cred.pid > 0 && cred.pid != std::process::id() as i32 {
        let mut all = CLIENTS.lock().unwrap();
        // The ones gone are forgotten, so the list does not grow for ever.
        all.retain(|p| unsafe { libc::kill(*p, 0) } == 0);
        if !all.contains(&cred.pid) {
            all.push(cred.pid);
        }
    }
}

/// The programs connected here, asked to close (SIGTERM: they save what they
/// have). The session is ending: their screen goes with it.
pub fn stop_clients() {
    let pids = std::mem::take(&mut *CLIENTS.lock().unwrap());
    for p in &pids {
        // SAFETY: a signal to a process that connected to this compositor.
        unsafe { libc::kill(*p, libc::SIGTERM) };
    }
    let until = std::time::Instant::now() + Duration::from_millis(1500);
    while pids.iter().any(|p| unsafe { libc::kill(*p, 0) } == 0) && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn stop_launched() {
    let groups = std::mem::take(&mut *LAUNCHED.lock().unwrap());
    // SAFETY: kill with a negative pid signals that process group; nothing else is touched.
    let alive = |g: i32| unsafe { libc::kill(-g, 0) == 0 };
    for &g in &groups {
        unsafe { libc::kill(-g, libc::SIGTERM) };
    }
    let until = std::time::Instant::now() + Duration::from_millis(1500);
    while groups.iter().any(|g| alive(*g)) && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    for &g in groups.iter().filter(|g| alive(**g)) {
        unsafe { libc::kill(-g, libc::SIGKILL) };
    }
}

/// A surface and every subsurface in it enter a monitor (leaving another):
/// a browser draws its page in a subsurface and paces it by the monitor that
/// one is on. Told nothing, with two monitors Zen fell to a few frames a second.
fn enter_tree(to: &Output, from: Option<&Output>, root: &WlSurface) {
    with_surface_tree_downward(
        root,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |s, _, _| {
            if let Some(f) = from {
                f.leave(s);
            }
            to.enter(s);
        },
        |_, _, _| true,
    );
}

/// A monitor's scale as the programs are told it: the whole number above it
/// for the ones that only know those (they draw bigger and are scaled down),
/// and the exact one for the ones that ask (fractional-scale).
fn output_scale(s: f64) -> Scale {
    if (s - s.round()).abs() < 0.001 { Scale::Integer(s.round().max(1.0) as i32) } else { Scale::Custom { advertised_integer: s.ceil() as i32, fractional: s } }
}

/// What a surface keeps of its last buffer.
fn content_of<T>(s: &WlSurface, f: impl FnOnce(&Content) -> T) -> Option<T> {
    with_states(s, |st| st.data_map.get::<Mutex<Content>>().map(|p| f(&p.lock().unwrap())))
}

/// A surface and its subsurfaces as pieces, each at its place, from the one
/// furthest back to the nearest (the order they are drawn in): with what it
/// shows if the render does not have it yet, or `Kept`.
fn pieces_of(root: &WlSurface, at: (i32, i32), sent: &[u64], out: &mut Vec<WindowPiece>) {
    with_surface_tree_upward(
        root,
        at,
        |s, states, &(x, y)| {
            let (dx, dy) = if s != root {
                let l = states.cached_state.get::<SubsurfaceCachedState>().current().location;
                (l.x, l.y)
            } else {
                (0, 0)
            };
            TraversalAction::DoChildren((x + dx, y + dy))
        },
        |s, states, &(x, y)| {
            let (dx, dy) = if s != root {
                let l = states.cached_state.get::<SubsurfaceCachedState>().current().location;
                (l.x, l.y)
            } else {
                (0, 0)
            };
            let Some(c) = states.data_map.get::<Mutex<Content>>() else { return };
            let mut c = c.lock().unwrap();
            if c.size.0 == 0 || c.size.1 == 0 {
                return;
            }
            let content = if c.changed || !sent.contains(&c.key) {
                c.changed = false;
                match &c.dmabuf {
                    Some((number, d)) => match dmabuf_piece(*number, d) {
                        Some(p) => PieceContent::Dmabuf(p),
                        None => PieceContent::Kept,
                    },
                    None => PieceContent::Pixels(c.data.clone()),
                }
            } else {
                PieceContent::Kept
            };
            out.push(WindowPiece { id: c.key, at: (x + dx, y + dy), size: (c.size.0 as u32, c.size.1 as u32), px: (c.px.0 as u32, c.px.1 as u32), src: c.src, content });
        },
        |_, _, _| true,
    );
}

/// A program's surface and its subsurfaces as pieces for its monitor, each at
/// its place: what it shows if the monitor does not have it yet, and the
/// buffer on the card it is, if it is one.
fn panel_pieces(root: &WlSurface, at: (i32, i32), sent: &[u64], scale: f64, out: &mut Vec<ClientPiece>) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    with_surface_tree_upward(
        root,
        at,
        |s, states, &(x, y)| {
            let l = if s != root { states.cached_state.get::<SubsurfaceCachedState>().current().location } else { (0, 0).into() };
            TraversalAction::DoChildren((x + l.x, y + l.y))
        },
        |s, states, &(x, y)| {
            let l = if s != root { states.cached_state.get::<SubsurfaceCachedState>().current().location } else { (0, 0).into() };
            let Some(c) = states.data_map.get::<Mutex<Content>>() else { return };
            let mut c = c.lock().unwrap();
            if c.size.0 == 0 || c.size.1 == 0 {
                return;
            }
            let fresh = c.changed || !sent.contains(&c.key);
            c.changed = false;
            let content = fresh.then(|| match &c.dmabuf {
                Some((number, d)) => dmabuf_piece(*number, d).map(PieceContent::Dmabuf),
                None => Some(PieceContent::Pixels(c.data.clone())),
            });
            let opaque = c.dmabuf.as_ref().is_some_and(|(_, d)| pleamar::dmabuf::opaque(d.format().code as u32));
            let size = (px(c.size.0 as i32).max(1) as u32, px(c.size.1 as i32).max(1) as u32);
            out.push(ClientPiece { key: c.key, at: (px(x + l.x), px(y + l.y)), size, px: (c.px.0 as u32, c.px.1 as u32), content: content.flatten(), buffer: c.dmabuf.as_ref().map(|(n, _)| *n), opaque });
        },
        |_, _, _| true,
    );
}

/// What the render needs to read a program's buffer on the card: its own copy
/// of the handle, and the layout.
fn dmabuf_piece(number: u64, d: &Dmabuf) -> Option<DmabufPiece> {
    let planes = d
        .handles()
        .zip(d.strides())
        .zip(d.offsets())
        .map(|((fd, stride), offset)| Some(pleamar::scene::DmabufPlane { fd: fd.try_clone_to_owned().ok()?, stride, offset }))
        .collect::<Option<Vec<_>>>()?;
    Some(DmabufPiece { buffer: number, planes, fourcc: d.format().code as u32, modifier: d.format().modifier.into() })
}

/// The topmost surface of a tree under a point, and where that surface is:
/// measured against what each one last drew, which is what is seen.
fn hit_tree(root: &WlSurface, at: Point<f64, Logical>, origin: (i32, i32)) -> Option<(WlSurface, Point<i32, Logical>)> {
    let mut found = None;
    with_surface_tree_downward(
        root,
        origin,
        |s, states, &(x, y)| {
            let (dx, dy) = if s != root {
                let l = states.cached_state.get::<SubsurfaceCachedState>().current().location;
                (l.x, l.y)
            } else {
                (0, 0)
            };
            TraversalAction::DoChildren((x + dx, y + dy))
        },
        |s, states, &(x, y)| {
            let (dx, dy) = if s != root {
                let l = states.cached_state.get::<SubsurfaceCachedState>().current().location;
                (l.x, l.y)
            } else {
                (0, 0)
            };
            let (x, y) = (x + dx, y + dy);
            let Some(p) = states.data_map.get::<Mutex<Content>>() else { return };
            let (w, h) = p.lock().unwrap().size;
            // Nearest first: the first one under the point that takes the
            // pointer there. A surface that only shows —Firefox paints its
            // page in one with an empty input region— lets it through to the
            // one below, which is the one its program listens to.
            let inside = at.x >= x as f64 && at.y >= y as f64 && at.x < (x + w as i32) as f64 && at.y < (y + h as i32) as f64;
            let takes = || {
                let local = ((at.x - x as f64).floor() as i32, (at.y - y as f64).floor() as i32);
                states.cached_state.get::<SurfaceAttributes>().current().input_region.as_ref().is_none_or(|r| r.contains(local))
            };
            if found.is_none() && inside && takes() {
                found = Some((s.clone(), Point::from((x, y))));
            }
        },
        |_, _, _| true,
    );
    found
}

/// A buffer of one pixel (wp-single-pixel-buffer: a background, black bars
/// under a video), scaled to its size by its viewport.
fn single_pixel(buffer: &WlBuffer) -> Option<((usize, usize), Vec<u8>)> {
    let p = smithay::wayland::single_pixel_buffer::get_single_pixel_buffer(buffer).ok()?;
    let c = |v: u32| (v >> 24) as u8;
    Some(((1, 1), vec![c(p.b), c(p.g), c(p.r), c(p.a)]))
}

/// A buffer's pixels, copied out of the program's memory so it can draw the next one.
fn read_buffer(buffer: &WlBuffer) -> Option<((usize, usize), Vec<u8>)> {
    with_buffer_contents(buffer, |ptr, len, d| {
        let (w, h, stride) = (d.width.max(0) as usize, d.height.max(0) as usize, d.stride.max(0) as usize);
        let opaque = match d.format {
            wl_shm::Format::Argb8888 => false,
            wl_shm::Format::Xrgb8888 => true,
            _ => return None,
        };
        let offset = d.offset.max(0) as usize;
        if offset + stride * h > len || stride < w * 4 {
            return None;
        }
        // SAFETY: smithay hands over the mapped pool, `len` bytes from `ptr`, and the range was checked.
        let src = unsafe { std::slice::from_raw_parts(ptr.add(offset), stride * h) };
        let mut data = Vec::with_capacity(w * h * 4);
        for row in src.chunks_exact(stride).take(h) {
            data.extend_from_slice(&row[..w * 4]);
        }
        if opaque {
            for px in data.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        Some(((w, h), data))
    })
    .ok()
    .flatten()
}

// ── the protocols ────────────────────────────────────────────────

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    /// Its program destroyed it: what was copied of it goes now. Whoever still
    /// holds a handle to it keeps its data alive —XWayland's windows did, all
    /// of them, a whole window of pixels each, for as long as the session ran—.
    fn destroyed(&mut self, surface: &WlSurface) {
        with_states(surface, |s| {
            if let Some(c) = s.data_map.get::<Mutex<Content>>() {
                let mut c = c.lock().unwrap();
                c.data = Vec::new();
                c.dmabuf = None;
            }
        });
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(x) = client.get_data::<XWaylandClientData>() {
            return &x.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor
    }

    /// A frame on the card is not taken until the program has finished
    /// drawing it: the commit waits for the buffer to be ready to read.
    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, |state, _, surface| {
            // With explicit sync the program says when its frame is ready: its
            // acquire point. Waited for without spinning, and nothing else is needed.
            let acquire = with_states(surface, |s| s.cached_state.get::<DrmSyncobjCachedState>().pending().acquire_point.clone());
            if let Some(point) = acquire {
                if let Ok((blocker, source)) = point.generate_blocker() {
                    if let Some(client) = surface.client() {
                        let waiting = state.handle.insert_source(source, move |_, _, state: &mut State| {
                            let dh = state.dh.clone();
                            state.client_compositor_state(&client).blocker_cleared(state, &dh);
                            Ok(())
                        });
                        if waiting.is_ok() {
                            add_blocker(surface, blocker);
                        }
                    }
                }
                return;
            }
            let pending = with_states(surface, |s| {
                s.cached_state.get::<SurfaceAttributes>().pending().buffer.as_ref().and_then(|a| match a {
                    BufferAssignment::NewBuffer(b) => get_dmabuf(b).cloned().ok(),
                    _ => None,
                })
            });
            let Some(d) = pending else { return };
            let Ok((blocker, source)) = d.generate_blocker(Interest::READ) else { return };
            let Some(client) = surface.client() else { return };
            let waiting = state.handle.insert_source(source, move |_, _, state: &mut State| {
                let dh = state.dh.clone();
                state.client_compositor_state(&client).blocker_cleared(state, &dh);
                Ok(())
            });
            if waiting.is_ok() {
                add_blocker(surface, blocker);
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        // A window's subsurface is on the monitor its window is on.
        if get_parent(surface).is_some() {
            let mut root = surface.clone();
            while let Some(p) = get_parent(&root) {
                root = p;
            }
            if let Some(w) = self.slots.iter().flatten().find(|w| w.surface == root) {
                if let Some(o) = self.outputs.get(w.screen) {
                    o.enter(surface);
                }
            }
        }
        let (buffer, callbacks) = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let attrs = guard.current();
            (attrs.buffer.take(), std::mem::take(&mut attrs.frame_callbacks))
        });
        // A program's own surface (or one hanging from it): the monitor it is on.
        let mut root = surface.clone();
        while let Some(p) = get_parent(&root) {
            root = p;
        }
        let on = self.panels.iter().find(|p| p.shell.wl_surface() == &root).map(|p| p.monitor);
        self.callbacks.extend(callbacks.into_iter().map(|cb| (cb, on)));
        let feedback = with_states(surface, |s| std::mem::take(&mut s.cached_state.get::<PresentationFeedbackCachedState>().current().callbacks));
        let buffer_was_removed = matches!(buffer, Some(BufferAssignment::Removed));
        // Each surface has its number for the render the first time it shows something.
        if !with_states(surface, |s| s.data_map.get::<Mutex<Content>>().is_some()) {
            self.next_number += 1;
            let key = self.next_number;
            with_states(surface, |s| {
                s.data_map.insert_if_missing_threadsafe(|| Mutex::new(Content { key, ..Content::default() }));
            });
        }
        match buffer {
            Some(BufferAssignment::NewBuffer(b)) => {
                let on_card = get_dmabuf(&b).ok().cloned();
                let fresh = match on_card {
                    // On the card: lent to the render, and handed back once copied.
                    Some(d) => {
                        let number = match self.buffers.get(&b.id()) {
                            Some(n) => *n,
                            None => {
                                self.next_number += 1;
                                self.buffers.insert(b.id(), self.next_number);
                                self.next_number
                            }
                        };
                        let size = (d.width() as usize, d.height() as usize);
                        let what = {
                            let mut root = surface.clone();
                            while let Some(p) = get_parent(&root) {
                                root = p;
                            }
                            match self.window_of(&root) {
                                Some(k) => format!("window {k} ({})", self.slots[k].as_ref().map_or("", |w| w.app.as_str())),
                                None if self.panels.iter().any(|p| p.shell.wl_surface() == &root) => "a program's surface".to_owned(),
                                None => "a menu or popup".to_owned(),
                            }
                        };
                        // With explicit sync, where to say it is no longer read.
                        let release = with_states(surface, |s| s.cached_state.get::<DrmSyncobjCachedState>().current().release_point.take());
                        // The same buffer again while still lent is the program's
                        // mistake; its earlier point is signalled so nobody waits for ever.
                        if let Some(old) = self.lent.insert(number, Lent { buffer: b, release, since: std::time::Instant::now(), told: false, what, surface: surface.clone() }) {
                            if let Some(p) = old.release {
                                let _ = p.signal();
                            }
                        }
                        Some((size, Vec::new(), Some((number, d))))
                    }
                    None => {
                        let p = read_buffer(&b).or_else(|| single_pixel(&b));
                        b.release();
                        p.map(|(size, data)| (size, data, None))
                    }
                };
                let before = with_states(surface, |s| {
                    let mut c = s.data_map.get::<Mutex<Content>>().unwrap().lock().unwrap();
                    // One that never reached the render goes back now: a newer one takes its place.
                    let unsent = c.changed.then(|| c.dmabuf.as_ref().map(|(n, _)| *n)).flatten();
                    match fresh {
                        Some((size, data, dmabuf)) => {
                            c.px = size;
                            c.data = data;
                            c.dmabuf = dmabuf;
                        }
                        None => {
                            c.px = (0, 0);
                            c.data = Vec::new();
                            c.dmabuf = None;
                        }
                    }
                    c.changed = true;
                    unsent.filter(|n| c.dmabuf.as_ref().is_none_or(|(now, _)| now != n))
                });
                if let Some(b) = before.and_then(|n| self.lent.remove(&n)) {
                    b.give_back();
                }
            }
            Some(BufferAssignment::Removed) => with_states(surface, |s| {
                let mut c = s.data_map.get::<Mutex<Content>>().unwrap().lock().unwrap();
                c.px = (0, 0);
                c.data = Vec::new();
                c.dmabuf = None;
                c.changed = true;
            }),
            None => {}
        }
        // Its size in its own units, from its pixels: at the scale it drew at,
        // or as its viewport says (which also may crop them). Every commit:
        // the viewport can change without a new buffer.
        with_states(surface, |s| {
            let scale = s.cached_state.get::<SurfaceAttributes>().current().buffer_scale.max(1) as f64;
            let vp = *s.cached_state.get::<ViewportCachedState>().current();
            let mut c = s.data_map.get::<Mutex<Content>>().unwrap().lock().unwrap();
            let (pw, ph) = (c.px.0 as f64, c.px.1 as f64);
            let src = vp.src.map_or([0.0, 0.0, pw as f32, ph as f32], |r| [(r.loc.x * scale) as f32, (r.loc.y * scale) as f32, (r.size.w * scale) as f32, (r.size.h * scale) as f32]);
            let size = if c.px == (0, 0) {
                (0, 0)
            } else if let Some(d) = vp.dst {
                (d.w.max(1) as usize, d.h.max(1) as usize)
            } else if let Some(r) = vp.src {
                (r.size.w.round().max(1.0) as usize, r.size.h.round().max(1.0) as usize)
            } else {
                ((pw / scale).round().max(1.0) as usize, (ph / scale).round().max(1.0) as usize)
            };
            if c.size != size || c.src != src {
                c.size = size;
                c.src = src;
                c.changed = true;
            }
        });
        self.popups.commit(surface);
        // Its root: the window it belongs to, or the menu.
        let mut root = surface.clone();
        while let Some(p) = get_parent(&root) {
            root = p;
        }
        let monitor = self.panels.iter().find(|p| p.shell.wl_surface() == &root).map(|p| p.monitor).or_else(|| self.window_of(&root).and_then(|s| self.slots[s].as_ref()).map(|w| w.screen)).unwrap_or(self.on_screen);
        if !feedback.is_empty() {
            self.presented.extend(feedback.into_iter().map(|f| (f, monitor)));
        }
        // The scale to draw at: the one of the monitor it is on.
        let scale = self.scale_of(monitor);
        with_states(surface, |s| {
            with_fractional_scale(s, |f| f.set_preferred_scale(scale));
            send_surface_state(surface, s, scale.ceil() as i32, Transform::Normal);
        });
        // A program's surface: told its size (again, if it asks for another or
        // it was hidden), and shown.
        if let Some(k) = self.panels.iter().position(|p| p.shell.wl_surface() == &root) {
            if surface == &root && buffer_was_removed {
                self.panels[k].configured = None;
            }
            self.configure_panel(k);
            self.dirty.push(root);
        // A window says nothing until it is answered: the first configure goes on its first commit.
        } else if let Some(slot) = self.window_of(&root) {
            if let Some(Toplevel::Xdg(t)) = self.slots[slot].as_ref().map(|w| w.toplevel.clone()) {
                if !t.is_initial_configure_sent() {
                    t.send_configure();
                }
            }
            self.dirty.push(root);
        } else if let Some(popup) = self.popups.find_popup(&root) {
            if let PopupKind::Xdg(p) = &popup {
                if !p.is_initial_configure_sent() {
                    let _ = p.send_configure();
                }
            }
            // A menu (or an input method's candidates) is drawn inside its
            // window: that window's image changes.
            if let Ok(owner) = smithay::desktop::find_popup_root_surface(&popup) {
                self.dirty.push(owner);
            }
        } else if self.drag.as_ref().is_some_and(|d| d.icon.as_ref() == Some(&root)) {
            self.show_drag_icon();
        } else if let Some(x) = self.unmanaged.iter().find(|x| x.wl_surface().as_ref() == Some(&root)).cloned() {
            // An X11 menu or tooltip: drawn with its window.
            if let Some(s) = self.unmanaged_owner(&x).and_then(|k| self.slots[k].as_ref()).map(|w| w.surface.clone()) {
                self.dirty.push(s);
            }
        } else if let Some((Toplevel::Xdg(t), _)) = self.waiting.iter().find(|(_, s)| s == &root).cloned() {
            if !t.is_initial_configure_sent() {
                t.send_configure();
            }
        }
    }
}

impl BufferHandler for State {
    /// A program's buffer is gone: what the render kept of it goes too.
    fn buffer_destroyed(&mut self, buffer: &WlBuffer) {
        if let Some(n) = self.buffers.remove(&buffer.id()) {
            // Gone, it cannot be released; its sync point is signalled all the same.
            if let Some(p) = self.lent.remove(&n).and_then(|l| l.release) {
                let _ = p.signal();
            }
            self.tell(NestEvent::Forget(vec![n]));
            layers::forget(&[n]);
        }
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }

    /// Accepted as it is, up to four planes: the render reads it when it is shown.
    fn dmabuf_imported(&mut self, _: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if (1..=4).contains(&dmabuf.num_planes()) {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let s = surface.wl_surface().clone();
        self.place(Toplevel::Xdg(surface), s);
    }

    /// Pressed on its own title bar (a GTK 4 header bar, a browser's tabs):
    /// it asks to be carried with the mouse. Where it goes is the scene's,
    /// which is told (`win.$i.held`) until the button is let go.
    fn move_request(&mut self, surface: ToplevelSurface, _: WlSeat, _: Serial) {
        if let Some(slot) = self.window_of(surface.wl_surface()) {
            self.hold(slot, 1, 0);
        }
    }

    /// Pressed on its own edge: it asks to be stretched by it.
    fn resize_request(&mut self, surface: ToplevelSurface, _: WlSeat, _: Serial, edges: xdg_toplevel::ResizeEdge) {
        if let Some(slot) = self.window_of(surface.wl_surface()) {
            self.hold(slot, 2, u32::from(edges));
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.decorated.remove(surface.wl_surface());
        if self.held.is_some_and(|slot| self.window_of(surface.wl_surface()) == Some(slot)) {
            self.held = None;
        }
        self.forget(&Toplevel::Xdg(surface));
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let Some(slot) = self.window_of(surface.wl_surface()) else { return };
        let title = with_states(surface.wl_surface(), |s| s.data_map.get::<XdgToplevelSurfaceData>().and_then(|d| d.lock().unwrap().title.clone())).unwrap_or_default();
        self.set_title(slot, title);
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        if let Some(slot) = self.window_of(surface.wl_surface()) {
            self.beside_parent(slot);
            // A dialog given your keyboard as it opened turns out to be of the
            // program the agent is working with (a portal's «Open files» that
            // its keys asked for): your keyboard goes back.
            let parent_pid = self.parent_slot(slot).map_or(0, |p| self.pid_of(p));
            if let Some((given, before, at)) = self.focus_given {
                if given == slot && self.focus == Some(slot) && at.elapsed() < Duration::from_secs(3) && self.agent_keeps_your_keyboard_for(parent_pid, before) {
                    self.focus_given = None;
                    self.set_focus(before.filter(|b| self.slots.get(*b).is_some_and(Option::is_some)));
                }
            }
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let Some(slot) = self.window_of(surface.wl_surface()) else { return };
        let app = with_states(surface.wl_surface(), |s| s.data_map.get::<XdgToplevelSurfaceData>().and_then(|d| d.lock().unwrap().app_id.clone())).unwrap_or_default();
        self.set_app(slot, app);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let geometry = unconstrained(self, &surface, positioner);
        surface.with_pending_state(|s| s.geometry = geometry);
        if let Err(e) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            eprintln!("windows · a menu could not be tracked: {e:?}");
        }
    }

    /// F11, a video, a game: fullscreen, on the monitor it names if it names one.
    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        let Some(slot) = self.window_of(surface.wl_surface()) else {
            // Not in a slot yet (waiting): it is answered all the same.
            if surface.is_initial_configure_sent() {
                surface.send_configure();
            }
            return;
        };
        if let Some(k) = output.and_then(|o| self.outputs.iter().position(|x| x.owns(&o))) {
            self.handle(ToNest::Send(slot, k));
        }
        self.set_fullscreen(slot, true);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        match self.window_of(surface.wl_surface()) {
            Some(slot) => self.set_fullscreen(slot, false),
            None if surface.is_initial_configure_sent() => {
                surface.send_configure();
            }
            None => {}
        }
    }

    /// Its own minimize button: away.
    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(slot) = self.window_of(surface.wl_surface()) {
            self.set_minimized(slot, true);
        }
    }

    /// The layout is the scene's: a window that asks to be maximized is
    /// answered, and stays where the scene has it.
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn grab(&mut self, _: PopupSurface, _: WlSeat, _: Serial) {
        // No grab: a click outside the menu closes it (see `dismiss_popups_not_under`).
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        let geometry = unconstrained(self, &surface, positioner);
        surface.with_pending_state(|s| {
            s.geometry = geometry;
            s.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}

impl XdgDecorationHandler for State {
    // The scene draws the frame: the programs are asked not to.
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecorationMode::ServerSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        // It lets the scene draw its frame (the scene is told when it has a slot).
        let surface = toplevel.wl_surface().clone();
        if self.decorated.insert(surface.clone()) {
            if let Some(slot) = self.window_of(&surface) {
                self.tell(NestEvent::Framed(slot, true));
            }
        }
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _: DecorationMode) {
        XdgDecorationHandler::new_decoration(self, toplevel);
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        XdgDecorationHandler::new_decoration(self, toplevel);
    }
}

// GTK 3 programs (Firefox and the browsers made from it among them) do not
// know xdg-decoration: they ask through KDE's older protocol, and draw their
// own frame unless the compositor says it draws one.
impl KdeDecorationHandler for State {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.kde_decorations
    }
    fn new_decoration(&mut self, _: &WlSurface, decoration: &smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration) {
        decoration.mode(smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode::Server);
    }
    // The program says which it takes: the scene is told whether to frame it.
    fn request_mode(&mut self, surface: &WlSurface, decoration: &smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration, mode: smithay::reexports::wayland_server::WEnum<smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode>) {
        use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode;
        let smithay::reexports::wayland_server::WEnum::Value(mode) = mode else { return };
        decoration.mode(mode);
        let framed = mode == Mode::Server;
        let changed = if framed { self.decorated.insert(surface.clone()) } else { self.decorated.remove(surface) };
        if changed {
            if let Some(slot) = self.window_of(surface) {
                self.tell(NestEvent::Framed(slot, framed));
            }
        }
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seats
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        // What the agent's keyboard is on is not where your clipboard goes.
        if agent::is_agent_seat(seat) {
            return;
        }
        // What is copied goes with the keyboard: the focused program is offered it.
        let client = focused.and_then(|s| self.dh.get_client(s.id()).ok());
        smithay::wayland::selection::data_device::set_data_device_focus(&self.dh, seat, client.clone());
        set_primary_focus(&self.dh, seat, client);
    }

    /// The cursor the program asks for, by its name: pleamar draws its own of
    /// the same kind. One drawn by the program itself is taken as the arrow.
    fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
        // The agent's cursor is the scene's to draw, not yours to change.
        if agent::is_agent_seat(seat) {
            return;
        }
        use smithay::input::pointer::CursorIcon as I;
        let kind = match image {
            CursorImageStatus::Named(I::Text | I::VerticalText) => pleamar::scene::Cursor::Text,
            CursorImageStatus::Named(I::Pointer) => pleamar::scene::Cursor::Hand,
            CursorImageStatus::Named(I::Grab) => pleamar::scene::Cursor::Grab,
            CursorImageStatus::Named(I::Grabbing) => pleamar::scene::Cursor::Grabbing,
            CursorImageStatus::Named(I::Move | I::AllScroll) => pleamar::scene::Cursor::Move,
            CursorImageStatus::Named(I::EwResize | I::ColResize | I::EResize | I::WResize) => pleamar::scene::Cursor::EwResize,
            CursorImageStatus::Named(I::NsResize | I::RowResize | I::NResize | I::SResize) => pleamar::scene::Cursor::NsResize,
            CursorImageStatus::Named(I::NwseResize | I::NwResize | I::SeResize) => pleamar::scene::Cursor::NwseResize,
            CursorImageStatus::Named(I::NeswResize | I::NeResize | I::SwResize) => pleamar::scene::Cursor::NeswResize,
            CursorImageStatus::Named(I::NotAllowed | I::NoDrop) => pleamar::scene::Cursor::NotAllowed,
            CursorImageStatus::Named(I::Crosshair) => pleamar::scene::Cursor::Crosshair,
            _ => pleamar::scene::Cursor::Normal,
        };
        // Over a program's surface of its own, the session shows it; over a
        // window, the scene does (it may draw on top).
        if self.panel_pointer.is_some() {
            layers::cursor(true, kind);
        } else {
            self.tell(NestEvent::Cursor(kind));
        }
    }
}

/// What the compositor itself offers as copied: what an X11 program copied
/// (XWayland serves it), or something kept here —what the agent pastes, and
/// what was copied before it, given back after—.
#[derive(Clone)]
pub enum Copied {
    X11,
    Kept(Arc<Kept>),
}

/// Something copied that is kept here, in each of its kinds, and whether
/// anyone has read it yet.
pub struct Kept {
    pub data: Vec<(String, Vec<u8>)>,
    pub read: std::sync::atomic::AtomicBool,
}

impl Kept {
    pub fn new(data: Vec<(String, Vec<u8>)>) -> Arc<Kept> {
        Arc::new(Kept { data, read: std::sync::atomic::AtomicBool::new(false) })
    }

    pub fn kinds(&self) -> Vec<String> {
        self.data.iter().map(|(k, _)| k.clone()).collect()
    }
}

/// What is copied now: nothing, a Wayland program's (in these kinds), an
/// X11 program's, or something kept here.
pub enum Clipboard {
    Nothing,
    Program(Vec<String>),
    X11,
    Kept(Arc<Kept>),
}

impl SelectionHandler for State {
    type SelectionUserData = Copied;

    fn new_selection(&mut self, ty: smithay::wayland::selection::SelectionTarget, source: Option<smithay::wayland::selection::SelectionSource>, _: Seat<Self>) {
        if matches!(ty, smithay::wayland::selection::SelectionTarget::Clipboard) {
            self.clipboard = source.as_ref().map_or(Clipboard::Nothing, |s| Clipboard::Program(s.mime_types()));
        }
        x11::wayland_copied(self, ty, source.map(|s| s.mime_types()));
    }

    fn send_selection(&mut self, ty: smithay::wayland::selection::SelectionTarget, mime_type: String, fd: std::os::fd::OwnedFd, _: Seat<Self>, copied: &Copied) {
        match copied {
            Copied::X11 => x11::wayland_pastes(self, ty, mime_type, fd),
            Copied::Kept(kept) => serve_kept(kept, &mime_type, fd),
        }
    }
}

/// Something kept here, to whoever pastes it: written on a thread of its
/// own, a program reads when it can.
fn serve_kept(kept: &Arc<Kept>, mime_type: &str, fd: std::os::fd::OwnedFd) {
    use std::io::Write;
    let Some(k) = kept.data.iter().position(|(t, _)| t == mime_type) else { return };
    kept.read.store(true, std::sync::atomic::Ordering::Relaxed);
    let kept = kept.clone();
    std::thread::spawn(move || {
        let _ = std::fs::File::from(fd).write_all(&kept.data[k].1);
    });
}

/// What a Wayland program copied, read here and now in each of these kinds
/// (one picture kind only: a program makes every picture kind it offers on
/// the spot, and a big one in all of them takes long). Waits for it up to a
/// second, with the clients told first: the program writes it on its own,
/// without the compositor. None if it did not arrive whole in time.
fn read_copied(state: &mut State, kinds: &[String]) -> Option<Vec<(String, Vec<u8>)>> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let picture = ["image/png", "image/jpeg"].iter().find(|p| kinds.iter().any(|k| k == *p)).map(|p| p.to_string()).or_else(|| kinds.iter().find(|k| k.starts_with("image/")).cloned());
    let wanted: Vec<&String> = kinds.iter().filter(|k| !k.starts_with("image/") || Some(*k) == picture.as_ref()).collect();
    let seat = state.seat.clone();
    let mut reads: Vec<(String, std::fs::File, Vec<u8>, bool)> = Vec::new();
    for kind in wanted {
        let mut fds = [0i32; 2];
        // SAFETY: a pipe into two fds that are ours from here on.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return None;
        }
        // SAFETY: both were just made by pipe2 and are owned by nothing else.
        let (reading, writing) = unsafe { (std::fs::File::from(std::os::fd::OwnedFd::from_raw_fd(fds[0])), std::os::fd::OwnedFd::from_raw_fd(fds[1])) };
        smithay::wayland::selection::data_device::request_data_device_client_selection(&seat, kind.clone(), writing).ok()?;
        reads.push((kind.clone(), reading, Vec::new(), false));
    }
    state.dh.flush_clients().ok()?;
    let until = Instant::now() + Duration::from_secs(1);
    let mut chunk = vec![0u8; 64 * 1024];
    let mut total = 0usize;
    while reads.iter().any(|r| !r.3) {
        let mut moved = false;
        for r in reads.iter_mut().filter(|r| !r.3) {
            match r.1.read(&mut chunk) {
                Ok(0) => r.3 = true,
                Ok(n) => {
                    r.2.extend_from_slice(&chunk[..n]);
                    total += n;
                    moved = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return None,
            }
        }
        if total > 64 << 20 || Instant::now() > until {
            return None;
        }
        if !moved {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Some(reads.into_iter().map(|r| (r.0, r.2)).collect())
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}

/// A program starts dragging (a file, some text): the pointer goes to
/// whatever window it is over, its icon follows it, and what it drags is
/// read as text in case it is dropped on the scene.
impl ClientDndGrabHandler for State {
    fn started(&mut self, source: Option<smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource>, icon: Option<WlSurface>, _: Seat<Self>) {
        self.tell(NestEvent::Dragging(true));
        println!("windows · a program drags something");
        let text = Arc::new(Mutex::new(None));
        if let Some(source) = source {
            read_dragged(&source, text.clone());
        }
        let mut ids = Vec::new();
        for k in 0..layers::monitors().len() {
            self.next_number += 1;
            ids.push((k, self.next_number, Vec::new()));
        }
        layers::set_drag_icon(ids.iter().map(|(k, id, _)| (*k, *id)).collect());
        self.drag = Some(Drag { icon, ids, text });
        self.show_drag_icon();
    }

    fn dropped(&mut self, target: Option<WlSurface>, validated: bool, _: Seat<Self>) {
        self.tell(NestEvent::Dragging(false));
        println!("windows · dropped {} ({})", if target.is_some() { "on a program" } else { "on nothing of a program's" }, if validated { "taken" } else { "not taken" });
        let Some(d) = self.drag.take() else { return };
        for (_, id, _) in &d.ids {
            layers::hide(*id);
        }
        layers::set_drag_icon(Vec::new());
        // On the scene —no window, no program's surface under it—: to its `drop` zones.
        if target.is_none() && self.pointer_on.is_none() && self.panel_pointer.is_none() {
            if let Some((kind, text)) = d.text.lock().unwrap().take() {
                let _ = self.to_render.send(ToRender::Dropped(kind, text));
            }
        }
    }
}

/// What is dragged, asked for as text (the first of the kinds it offers that
/// is), read on a thread of its own: a program answers when it can.
fn read_dragged(source: &smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource, into: Arc<Mutex<Option<(String, String)>>>) {
    use std::io::Read;
    use std::os::fd::{AsFd, FromRawFd};
    let Ok(kinds) = smithay::wayland::selection::data_device::with_source_metadata(source, |m| m.mime_types.clone()) else { return };
    let Some(kind) = ["text/uri-list", "text/plain;charset=utf-8", "UTF8_STRING", "text/plain"].iter().find(|k| kinds.iter().any(|m| m == *k)) else { return };
    let mut fds = [0i32; 2];
    // SAFETY: a pipe into two fds that are ours from here on.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return;
    }
    // SAFETY: both were just made by pipe2 and are owned by nothing else.
    let (reading, writing) = unsafe { (std::fs::File::from(std::os::fd::OwnedFd::from_raw_fd(fds[0])), std::os::fd::OwnedFd::from_raw_fd(fds[1])) };
    source.send(kind.to_string(), writing.as_fd());
    drop(writing);
    let kind = kind.to_string();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = reading.take(1 << 20).read_to_string(&mut text);
        *into.lock().unwrap() = Some((kind, text));
    });
}
impl ServerDndGrabHandler for State {}

impl DrmSyncobjHandler for State {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.syncobj.as_mut()
    }
}

/// The render node of that card (by its device number), opened: what sync
/// points are imported with when there is no session's card.
fn render_node(device: u64) -> Option<smithay::backend::drm::DrmDeviceFd> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::read_dir("/dev/dri").ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD")) && std::fs::metadata(p).is_ok_and(|m| m.rdev() == device)
    })?;
    let file = std::fs::OpenOptions::new().read(true).write(true).open(path).ok()?;
    Some(smithay::backend::drm::DrmDeviceFd::new(smithay::utils::DeviceFd::from(std::os::fd::OwnedFd::from(file))))
}

impl OutputHandler for State {
    /// A program that lists the windows may bind a monitor after it was told
    /// of them: the windows on that monitor are said to be there now.
    fn output_bound(&mut self, output: Output, wl_output: WlOutput) {
        let Some(k) = self.outputs.iter().position(|o| o == &output) else { return };
        let Some(client) = wl_output.client() else { return };
        for (slot, h) in &self.toplevel_handles {
            let on = self.slots.get(*slot).and_then(Option::as_ref).is_some_and(|w| w.screen == k);
            if on && h.client().as_ref() == Some(&client) {
                h.output_enter(&wl_output);
                h.done();
            }
        }
    }
}

/// The lock screen (Marea's): while it holds, the monitors show only its
/// surfaces, and the keyboard and the pointer go only to them.
impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self._session_lock
    }

    /// Nothing else is shown from now on, so it is locked at once.
    fn lock(&mut self, confirmation: SessionLocker) {
        println!("windows · the session is locked");
        layers::set_locked(true);
        // On the phone, it comes back to the desk, where its lock screen is:
        // the phone unlocks it from there (docs/phone.md).
        if layers::monitors().iter().any(|m| m.name == layers::PHONE_NAME) {
            layers::request_phone(None);
        }
        confirmation.lock();
    }

    fn unlock(&mut self) {
        println!("windows · the session is unlocked");
        layers::set_locked(false);
        let (locks, rest): (Vec<Panel>, Vec<Panel>) = std::mem::take(&mut self.panels).into_iter().partition(|p| matches!(p.shell, Shell::Lock(_)));
        self.panels = rest;
        for p in locks {
            layers::hide(p.id);
            if self.panel_keyboard == Some(p.id) {
                self.panel_keyboard = None;
            }
            if self.panel_pointer == Some(p.id) {
                self.panel_pointer = None;
            }
        }
        self.set_focus(self.focus);
    }

    /// One per monitor, the size of it.
    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let monitor = self.outputs.iter().position(|x| x.owns(&output)).unwrap_or(0);
        let (w, h) = self.monitor_size(monitor);
        surface.with_pending_state(|s| s.size = Some((w.max(1) as u32, h.max(1) as u32).into()));
        surface.send_configure();
        self.next_number += 1;
        self.outputs[monitor].enter(surface.wl_surface());
        self.panels.push(Panel { shell: Shell::Lock(surface), id: self.next_number, monitor, configured: Some((w, h)), sent: Vec::new() });
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrScreencopyManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, ());
    }
}

/// A picture of a monitor, or of a piece of it: said what memory it wants
/// (XRGB, its size), taken the next time that monitor is put together.
impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn request(state: &mut Self, _: &Client, _: &ZwlrScreencopyManagerV1, request: copy_manager::Request, _: &(), _: &DisplayHandle, init: &mut DataInit<'_, Self>) {
        // With the pointer in it, if asked: a recorder asks (wf-recorder), a
        // screenshot does not unless told to (grim -c). It is on the card's
        // own plane, not in what the monitor puts together, so it is drawn
        // onto the picture when it is handed over.
        let (frame, output, piece, pointer) = match request {
            copy_manager::Request::CaptureOutput { frame, overlay_cursor, output } => (frame, output, None, overlay_cursor != 0),
            copy_manager::Request::CaptureOutputRegion { frame, overlay_cursor, output, x, y, width, height } => (frame, output, Some([x, y, width, height]), overlay_cursor != 0),
            _ => return,
        };
        state.next_number += 1;
        let id = state.next_number;
        let frame = init.init(frame, id);
        let Some(monitor) = state.outputs.iter().position(|o| o.owns(&output)) else {
            frame.failed();
            return;
        };
        // In the monitor's pixels; a piece is asked for in units.
        let (mw, mh) = state.monitor_pixels(monitor);
        let s = state.scale_of(monitor);
        let p = piece.map_or([0, 0, mw, mh], |p| [(p[0] as f64 * s) as i32, (p[1] as f64 * s) as i32, (p[2] as f64 * s).round() as i32, (p[3] as f64 * s).round() as i32]);
        let (x0, y0) = (p[0].clamp(0, mw), p[1].clamp(0, mh));
        let (x1, y1) = ((p[0] + p[2]).clamp(x0, mw), (p[1] + p[3]).clamp(y0, mh));
        let piece = [x0, y0, x1 - x0, y1 - y0];
        if piece[2] == 0 || piece[3] == 0 {
            frame.failed();
            return;
        }
        frame.buffer(wl_shm::Format::Xrgb8888, piece[2] as u32, piece[3] as u32, piece[2] as u32 * 4);
        if frame.version() >= 3 {
            frame.buffer_done();
        }
        state.pictures.insert(id, Picture { frame, monitor, piece, buffer: None, damage: false, pointer });
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, u64> for State {
    fn request(state: &mut Self, _: &Client, _: &ZwlrScreencopyFrameV1, request: copy_frame::Request, id: &u64, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        let (buffer, damage) = match request {
            copy_frame::Request::Copy { buffer } => (buffer, false),
            copy_frame::Request::CopyWithDamage { buffer } => (buffer, true),
            copy_frame::Request::Destroy => {
                if state.pictures.remove(id).is_some() {
                    layers::uncapture(*id);
                }
                return;
            }
            _ => return,
        };
        let owner = buffer.client().map_or(0, |c| owner_hash(&c.id()));
        let Some(p) = state.pictures.get_mut(id) else { return };
        p.buffer = Some(buffer);
        p.damage = damage;
        if crate::screen::capture_debug() {
            eprintln!("capture · {:.1} asked (damage {damage})", crate::screen::wall_ms());
        }
        layers::capture(p.monitor, *id, p.piece, damage, owner);
        // Waiting for a change with the pointer in it: its moving is one.
        if damage && p.pointer {
            layers::wait_pointer(*id);
        }
    }

    /// Its program went away with a picture still asked for (a glass waiting
    /// for what is behind it to change): the monitor stopped waiting for it.
    fn destroyed(state: &mut Self, _: ClientId, _: &ZwlrScreencopyFrameV1, id: &u64) {
        if state.pictures.remove(id).is_some() {
            layers::uncapture(*id);
        }
    }
}

/// Where a surface asked for what is behind it to be blurred, from its corner.
#[derive(Default)]
struct Blur(Mutex<Vec<[i32; 4]>>);

impl GlobalDispatch<ExtBackgroundEffectManagerV1, ()> for State {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ExtBackgroundEffectManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        let manager = init.init(resource, ());
        manager.capabilities(effect_manager::Capability::Blur);
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
    fn request(_: &mut Self, _: &Client, _: &ExtBackgroundEffectManagerV1, request: effect_manager::Request, _: &(), _: &DisplayHandle, init: &mut DataInit<'_, Self>) {
        if let effect_manager::Request::GetBackgroundEffect { id, surface } = request {
            with_states(&surface, |s| {
                s.data_map.insert_if_missing_threadsafe(Blur::default);
            });
            init.init(id, surface);
        }
    }
}

/// Its blur region: taken as it is said, and shown with the surface's next frame.
impl Dispatch<ExtBackgroundEffectSurfaceV1, WlSurface> for State {
    fn request(_: &mut Self, _: &Client, _: &ExtBackgroundEffectSurfaceV1, request: effect_surface::Request, surface: &WlSurface, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        let rects: Vec<[i32; 4]> = match request {
            effect_surface::Request::SetBlurRegion { region } => region
                .map(|r| smithay::wayland::compositor::get_region_attributes(&r).rects.iter().filter(|(k, _)| matches!(k, RectangleKind::Add)).map(|(_, r)| [r.loc.x, r.loc.y, r.size.w, r.size.h]).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if surface.alive() {
            with_states(surface, |s| {
                if let Some(b) = s.data_map.get::<Blur>() {
                    *b.0.lock().unwrap() = rects;
                }
            });
        }
    }
}


impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn bind(state: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrForeignToplevelManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        let manager = init.init(resource, ());
        let open: Vec<usize> = state.order.clone();
        for slot in open {
            state.announce(&manager, slot);
        }
        state.toplevel_managers.push(manager);
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn request(state: &mut Self, _: &Client, manager: &ZwlrForeignToplevelManagerV1, request: toplevel_manager::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        if let toplevel_manager::Request::Stop = request {
            state.toplevel_managers.retain(|m| m != manager);
            manager.finished();
        }
    }

    /// Its program went away without saying `stop` (it closed, it crashed):
    /// kept, every window that opened was announced to nobody, for good.
    fn destroyed(state: &mut Self, _: ClientId, manager: &ZwlrForeignToplevelManagerV1, _: &()) {
        state.toplevel_managers.retain(|m| m != manager);
    }
}

/// What one who lists the windows may ask of one: the keyboard, or to close it.
impl Dispatch<ZwlrForeignToplevelHandleV1, usize> for State {
    fn request(state: &mut Self, _: &Client, handle: &ZwlrForeignToplevelHandleV1, request: toplevel_handle::Request, slot: &usize, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        match request {
            // Given the keyboard, a window put away comes back.
            toplevel_handle::Request::Activate { .. } => {
                // Asked for, it is seen: its monitor shows its workspace.
                state.tell(NestEvent::Reveal(*slot));
                if state.slots.get(*slot).and_then(Option::as_ref).is_some_and(|w| w.minimized) {
                    state.set_minimized(*slot, false);
                } else {
                    state.set_focus(Some(*slot));
                }
            }
            toplevel_handle::Request::SetMinimized => state.set_minimized(*slot, true),
            toplevel_handle::Request::UnsetMinimized => state.set_minimized(*slot, false),
            toplevel_handle::Request::Close => {
                if let Some(Some(w)) = state.slots.get(*slot) {
                    w.toplevel.send_close();
                }
            }
            toplevel_handle::Request::Destroy => state.toplevel_handles.retain(|(_, h)| h != handle),
            _ => {}
        }
    }

    /// Its program went away: every change of that window was still told to it.
    fn destroyed(state: &mut Self, _: ClientId, handle: &ZwlrForeignToplevelHandleV1, _: &usize) {
        state.toplevel_handles.retain(|(_, h)| h != handle);
    }
}

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell
    }

    /// On the monitor it asks for; if it asks for none, the one the pointer is on.
    fn new_layer_surface(&mut self, surface: LayerSurface, output: Option<WlOutput>, _: ShellLayer, namespace: String) {
        let monitor = output.as_ref().and_then(|o| self.outputs.iter().position(|x| x.owns(o))).unwrap_or(self.on_screen).min(self.outputs.len() - 1);
        self.next_number += 1;
        self.outputs[monitor].enter(surface.wl_surface());
        println!("windows · '{namespace}' on {}", self.outputs[monitor].name());
        self.panels.push(Panel { shell: Shell::Layer(surface), id: self.next_number, monitor, configured: None, sent: Vec::new() });
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let Some(k) = self.panels.iter().position(|p| p.shell.wl_surface() == surface.wl_surface()) else { return };
        let p = self.panels.remove(k);
        layers::hide(p.id);
        self.tell_reserved();
        if self.panel_pointer == Some(p.id) {
            self.panel_pointer = None;
        }
        if self.panel_keyboard == Some(p.id) {
            self.panel_keyboard = None;
            self.set_focus(self.focus);
        }
    }
}
impl TabletSeatHandler for State {}

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary
    }
}

impl WlrDataControlHandler for State {
    fn data_control_state(&self) -> &WlrDataControlState {
        &self._wlr_data_control
    }
}

impl ExtDataControlHandler for State {
    fn data_control_state(&self) -> &ExtDataControlState {
        &self._ext_data_control
    }
}

/// A program asks for a window to come forward (a link opened in the browser
/// that is already running): it does, with a token from the last few seconds.
impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation
    }

    fn request_activation(&mut self, token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        // A program the agent just opened asking to come forward (Chrome's
        // new window; Discord, already running, shown again): to the
        // agent's monitor, and your keyboard stays where it is.
        if let Some(slot) = self.window_of(&surface) {
            // The program the agent is working with, asking because of what
            // the agent did (a click of its): your keyboard stays where it is.
            let parent_pid = self.parent_slot(slot).map_or(0, |p| self.pid_of(p));
            if self.agent_keeps_your_keyboard(self.pid_of(slot)) || self.agent_keeps_your_keyboard(parent_pid) {
                self.activation.remove_token(&token);
                return;
            }
            let app = self.slots[slot].as_ref().map(|w| w.app.clone()).unwrap_or_default();
            if let Some(screen) = self.agent_opened(self.pid_of(slot), &app) {
                if self.slots[slot].as_ref().is_some_and(|w| w.screen != screen) {
                    self.handle(ToNest::Send(slot, screen));
                }
                self.activation.remove_token(&token);
                return;
            }
        }
        // Only what follows something of yours brings a window forward and
        // takes the keyboard: asked by the program that has your keyboard (a
        // link clicked in it opens the browser), or while the scene has it
        // (Spotlight, Marea's finder). Anything else —a message arrived in a
        // program you are not in— asks for attention instead. (A serial is
        // no proof: GTK sends that of the pointer merely passing over it.)
        let focused_client = self.focus.and_then(|s| self.slots[s].as_ref()).and_then(|w| w.surface.client()).map(|c| c.id());
        let yours = !self.host_focus || (data.client_id.is_some() && data.client_id == focused_client);
        if data.timestamp.elapsed() < Duration::from_secs(10) && yours {
            if let Some(slot) = self.window_of(&surface) {
                self.tell(NestEvent::Reveal(slot));
                self.set_focus(Some(slot));
            }
        } else if let Some(slot) = self.window_of(&surface).filter(|s| Some(*s) != self.focus) {
            // Asking with nothing of yours behind it —a message arrived while
            // you were elsewhere—: it does not come forward, it asks for
            // attention (a dock makes its icon hop), until it gets the keyboard.
            if self.urgent.insert(slot) {
                self.tell(NestEvent::Urgent(slot, true));
            }
        }
        self.activation.remove_token(&token);
    }
}

impl IdleNotifierHandler for State {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle
    }
}

impl State {
    /// A program that kept the screen awake (a video) and died without saying
    /// so: smithay only hears an inhibitor that is destroyed on purpose, and
    /// the monitors never went dark again.
    fn prune_inhibitors(&mut self) {
        if self.inhibitors.iter().any(|s| !s.alive()) {
            self.inhibitors.retain(|s| s.alive());
            let any = !self.inhibitors.is_empty();
            self.idle.set_is_inhibited(any);
            layers::set_inhibited(any);
        }
    }
}

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.inhibitors.retain(|s| s.alive() && s != &surface);
        self.inhibitors.push(surface);
        self.idle.set_is_inhibited(true);
        layers::set_inhibited(true);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.inhibitors.retain(|s| s.alive() && s != &surface);
        let any = !self.inhibitors.is_empty();
        self.idle.set_is_inhibited(any);
        layers::set_inhibited(any);
    }
}

/// An input method's candidates (fcitx5): a menu of the window being typed in.
impl InputMethodHandler for State {
    fn new_popup(&mut self, surface: ImePopup) {
        if let Err(e) = self.popups.track_popup(PopupKind::from(surface)) {
            eprintln!("windows · an input method's popup could not be tracked: {e:?}");
        }
    }

    fn dismiss_popup(&mut self, surface: ImePopup) {
        if let Some(parent) = surface.get_parent().map(|p| p.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    fn popup_repositioned(&mut self, _: ImePopup) {}

    fn parent_geometry(&self, parent: &WlSurface) -> smithay::utils::Rectangle<i32, Logical> {
        with_states(parent, |s| s.cached_state.get::<SurfaceCachedState>().current().geometry).unwrap_or_default()
    }
}

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, _: &WlSurface, _: &PointerHandle<Self>) {
        self.update_constraint();
    }

    fn cursor_position_hint(&mut self, _: &WlSurface, _: &PointerHandle<Self>, _: Point<f64, Logical>) {}
}

impl XdgForeignHandler for State {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.foreign
    }
}

impl XdgDialogHandler for State {}

/// A surface that asks for its exact scale is told it as soon as it commits
/// (see `commit`).
impl FractionalScaleHandler for State {}

impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.toplevel_list
    }
}

delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_xdg_decoration!(State);
smithay::delegate_kde_decoration!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_output!(State);
delegate_cursor_shape!(State);
delegate_dmabuf!(State);
delegate_layer_shell!(State);
delegate_session_lock!(State);
delegate_drm_syncobj!(State);
smithay::delegate_primary_selection!(State);
smithay::delegate_data_control!(State);
smithay::delegate_ext_data_control!(State);
smithay::delegate_xdg_activation!(State);
smithay::delegate_idle_notify!(State);
smithay::delegate_idle_inhibit!(State);
smithay::delegate_virtual_keyboard_manager!(State);
smithay::delegate_text_input_manager!(State);
smithay::delegate_input_method_manager!(State);
smithay::delegate_pointer_constraints!(State);
smithay::delegate_relative_pointer!(State);
smithay::delegate_xdg_foreign!(State);
smithay::delegate_xdg_dialog!(State);
smithay::delegate_foreign_toplevel_list!(State);
smithay::delegate_presentation!(State);
smithay::delegate_content_type!(State);

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrOutputPowerManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, ());
    }
}

/// A monitor's power, for a program to watch and set.
impl Dispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn request(state: &mut Self, _: &Client, _: &ZwlrOutputPowerManagerV1, request: power_manager::Request, _: &(), _: &DisplayHandle, init: &mut DataInit<'_, Self>) {
        if let power_manager::Request::GetOutputPower { id, output } = request {
            let monitor = state.outputs.iter().position(|o| o.owns(&output));
            let power = init.init(id, monitor.unwrap_or(usize::MAX));
            match monitor {
                Some(k) => {
                    power.mode(if layers::powered(k) { output_power::Mode::On } else { output_power::Mode::Off });
                    state.powers.push((power, k));
                }
                None => power.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, usize> for State {
    fn request(state: &mut Self, _: &Client, power: &ZwlrOutputPowerV1, request: output_power::Request, monitor: &usize, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        match request {
            output_power::Request::SetMode { mode } => {
                let on = matches!(mode.into_result(), Ok(output_power::Mode::On));
                layers::request_power(Some(*monitor), on);
            }
            output_power::Request::Destroy => state.powers.retain(|(p, _)| p != power),
            _ => {}
        }
    }
}
smithay::delegate_viewporter!(State);
smithay::delegate_fractional_scale!(State);
smithay::delegate_single_pixel_buffer!(State);
