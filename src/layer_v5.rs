//! `zwlr_layer_shell_v1`, version 5.
//!
//! smithay 0.7.0 (the latest published) creates the global with version 4 fixed
//! (`wlr_layer/mod.rs`, `create_global(4, …)`) and ignores the one request version 5
//! adds, `set_exclusive_edge`. hyprtoolkit 0.6 (hyprpolkitagent, hyprlauncher) asks for
//! version 5, and wayland-server answered «Invalid binding of zwlr_layer_shell_v1
//! version 5»: the program was thrown out before it drew anything.
//!
//! Rather than carry a copy of smithay (`[patch.crates-io]`), this module does the two
//! things on this side of it, and leaves the rest to smithay's own layer shell:
//!  * the global: smithay's version-4 one is removed and one of version 5 takes its
//!    place (`advertise`). Its bind is smithay's (`init(resource, ())`), so everything
//!    after the bind, the surfaces included, is smithay's.
//!  * the layer surface's requests: `set_exclusive_edge` is kept here, double-buffered
//!    like the anchor and the zone (`ExclusiveEdge`, applied on `wl_surface.commit`);
//!    every other request goes on to smithay's layer shell as it was.
//!
//! The edge matters to `reserved_side`: a surface anchored to a corner keeps a strip
//! of the screen for its exclusive zone, and which edge's it is cannot be told from
//! the anchors, so until now it kept none.

use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use smithay::wayland::compositor::{with_states, Cacheable};
use smithay::wayland::shell::wlr_layer::{Anchor, LayerSurface, LayerSurfaceCachedState, WlrLayerShellHandler, WlrLayerShellState, WlrLayerSurfaceUserData};

/// What the global announces: the version of the protocol the wlr-protocols file has.
pub const VERSION: u32 = 5;

/// The data of the global that replaces smithay's.
pub struct GlobalV5;

/// Where the requests that smithay does not know are told (and the rest passed on).
pub struct LayerShellV5;

/// `set_exclusive_edge`: the edge (one `Anchor` bit) the exclusive zone applies to, or
/// 0 when the program said none. Double-buffered: asked now, in force at the commit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExclusiveEdge(pub u32);

impl Cacheable for ExclusiveEdge {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        *self
    }
    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

/// The version-5 global in place of smithay's version-4 one (`smithay` is the id its
/// `WlrLayerShellState::new` gave back). Before the first program connects.
pub fn advertise<D>(display: &DisplayHandle, smithay: GlobalId) -> GlobalId
where
    D: GlobalDispatch<ZwlrLayerShellV1, GlobalV5> + 'static,
{
    display.remove_global::<D>(smithay);
    display.create_global::<D, ZwlrLayerShellV1, GlobalV5>(VERSION, GlobalV5)
}

impl<D> GlobalDispatch<ZwlrLayerShellV1, GlobalV5, D> for LayerShellV5
where
    D: GlobalDispatch<ZwlrLayerShellV1, GlobalV5>,
    D: Dispatch<ZwlrLayerShellV1, ()>,
    D: 'static,
{
    fn bind(_: &mut D, _: &DisplayHandle, _: &Client, resource: New<ZwlrLayerShellV1>, _: &GlobalV5, init: &mut DataInit<'_, D>) {
        init.init(resource, ());
    }
}

impl<D> Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData, D> for LayerShellV5
where
    D: Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData>,
    D: WlrLayerShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        client: &Client,
        layer_surface: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        data: &WlrLayerSurfaceUserData,
        display: &DisplayHandle,
        init: &mut DataInit<'_, D>,
    ) {
        if let zwlr_layer_surface_v1::Request::SetExclusiveEdge { edge } = request {
            let bits: u32 = edge.into();
            let surface = state.shell_state().layer_surfaces().find(|l| l.shell_surface() == layer_surface).map(|l| l.wl_surface().clone());
            if let Some(surface) = surface {
                with_states(&surface, |s| s.cached_state.get::<ExclusiveEdge>().pending().0 = bits);
            }
            return;
        }
        <WlrLayerShellState as Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData, D>>::request(state, client, layer_surface, request, data, display, init);
    }

    fn destroyed(state: &mut D, client: ClientId, layer_surface: &ZwlrLayerSurfaceV1, data: &WlrLayerSurfaceUserData) {
        // smithay forgets the surface and resets its own state; the edge goes with it.
        let surface = state.shell_state().layer_surfaces().find(|l| l.shell_surface() == layer_surface).map(|l| l.wl_surface().clone());
        <WlrLayerShellState as Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData, D>>::destroyed(state, client, layer_surface, data);
        if let Some(surface) = surface {
            with_states(&surface, |s| {
                let mut edge = s.cached_state.get::<ExclusiveEdge>();
                *edge.pending() = ExclusiveEdge::default();
                *edge.current() = ExclusiveEdge::default();
            });
        }
    }
}

/// The edge the program asked for, as of the last commit (0: none).
pub fn exclusive_edge(layer: &LayerSurface) -> u32 {
    with_states(layer.wl_surface(), |s| s.cached_state.get::<ExclusiveEdge>().current().0)
}

/// Is the edge asked for wrong, given the anchors? It has to be one edge, and one the
/// surface is anchored to (0, no edge, is always right).
pub fn invalid_edge(anchor: Anchor, edge: u32) -> bool {
    edge != 0 && (edge.count_ones() != 1 || !Anchor::from_bits(edge).is_some_and(|e| anchor.contains(e)))
}

/// At each commit: a wrong edge is the program's protocol error
/// (`invalid_exclusive_edge`), not something to guess around.
pub fn check(layer: &LayerSurface) {
    let edge = exclusive_edge(layer);
    let anchor = with_states(layer.wl_surface(), |s| s.cached_state.get::<LayerSurfaceCachedState>().current().anchor);
    if invalid_edge(anchor, edge) {
        layer.shell_surface().post_error(zwlr_layer_surface_v1::Error::InvalidExclusiveEdge, format!("exclusive edge {edge} is not one of the surface's anchors ({})", anchor.bits()));
    }
}

/// The side of the screen whose strip an exclusive zone is kept on, and the margin on
/// that side: 0 top, 1 right, 2 bottom, 3 left. With an edge asked for, that one; without
/// it, the one the anchors tell (one edge, or an edge and both of its sides), and none
/// when they do not (a corner, or every side).
pub fn reserved_side(anchor: Anchor, edge: u32, margin: (i32, i32, i32, i32)) -> Option<(usize, i32)> {
    let (top, right, bottom, left) = margin;
    if !invalid_edge(anchor, edge) {
        match Anchor::from_bits(edge) {
            Some(e) if e == Anchor::TOP => return Some((0, top)),
            Some(e) if e == Anchor::RIGHT => return Some((1, right)),
            Some(e) if e == Anchor::BOTTOM => return Some((2, bottom)),
            Some(e) if e == Anchor::LEFT => return Some((3, left)),
            _ => {}
        }
    }
    let (t, r, b, l) = (anchor.contains(Anchor::TOP), anchor.contains(Anchor::RIGHT), anchor.contains(Anchor::BOTTOM), anchor.contains(Anchor::LEFT));
    match (t, r, b, l) {
        (true, _, false, _) if r == l || (r && l) => Some((0, top)),
        (false, _, true, _) if r == l || (r && l) => Some((2, bottom)),
        (_, true, _, false) if t == b || (t && b) => Some((1, right)),
        (_, false, _, true) if t == b || (t && b) => Some((3, left)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: (i32, i32, i32, i32) = (1, 2, 3, 4);

    #[test]
    fn the_edge_must_be_one_of_the_anchors() {
        assert!(!invalid_edge(Anchor::TOP | Anchor::LEFT, 0));
        assert!(!invalid_edge(Anchor::TOP | Anchor::LEFT, Anchor::TOP.bits()));
        assert!(!invalid_edge(Anchor::TOP | Anchor::LEFT, Anchor::LEFT.bits()));
        assert!(invalid_edge(Anchor::TOP | Anchor::LEFT, Anchor::BOTTOM.bits()));
        assert!(invalid_edge(Anchor::TOP | Anchor::LEFT, (Anchor::TOP | Anchor::LEFT).bits()), "two edges are not an edge");
        assert!(invalid_edge(Anchor::TOP | Anchor::LEFT, 16), "not an anchor bit");
        assert!(invalid_edge(Anchor::empty(), Anchor::TOP.bits()));
    }

    #[test]
    fn a_corner_keeps_the_side_its_program_names() {
        let corner = Anchor::TOP | Anchor::LEFT;
        // Without the edge a corner is ambiguous, as it was before version 5.
        assert_eq!(reserved_side(corner, 0, M), None);
        assert_eq!(reserved_side(corner, Anchor::TOP.bits(), M), Some((0, 1)));
        assert_eq!(reserved_side(corner, Anchor::LEFT.bits(), M), Some((3, 4)));
        assert_eq!(reserved_side(Anchor::BOTTOM | Anchor::RIGHT, Anchor::RIGHT.bits(), M), Some((1, 2)));
        assert_eq!(reserved_side(Anchor::BOTTOM | Anchor::RIGHT, Anchor::BOTTOM.bits(), M), Some((2, 3)));
        // Every side: the edge says which one the zone is on.
        let all = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
        assert_eq!(reserved_side(all, 0, M), None);
        assert_eq!(reserved_side(all, Anchor::BOTTOM.bits(), M), Some((2, 3)));
    }

    #[test]
    fn what_the_anchors_tell_stays_as_it_was() {
        assert_eq!(reserved_side(Anchor::TOP, 0, M), Some((0, 1)));
        assert_eq!(reserved_side(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, 0, M), Some((0, 1)));
        assert_eq!(reserved_side(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT, 0, M), Some((2, 3)));
        assert_eq!(reserved_side(Anchor::LEFT | Anchor::TOP | Anchor::BOTTOM, 0, M), Some((3, 4)));
        assert_eq!(reserved_side(Anchor::RIGHT, 0, M), Some((1, 2)));
        // The edge asked for and the one the anchors tell can differ: the program's word wins.
        assert_eq!(reserved_side(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, Anchor::LEFT.bits(), M), Some((3, 4)));
        // A wrong edge is a protocol error at the commit (`check`); until then it does not move the zone.
        assert_eq!(reserved_side(Anchor::TOP, Anchor::BOTTOM.bits(), M), Some((0, 1)));
    }
}
