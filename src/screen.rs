//! A monitor of pleamar-wm's own session, and what is shown on it: the
//! surfaces of the scene —the scene's own, and the named ones: a bar, a
//! corner, a panel— each painted by the render in textures of its own, and
//! put together here, in their places and in the order of their levels, into
//! the buffer that goes to the screen. What layer-shell and the compositor
//! underneath did, done by hand.
//!
//! Each surface paints on its own (and only where something changed); the
//! monitor is put together at most once per refresh, on a thread of its own,
//! when something new has been painted. Where it ends up —the card's buffers
//! with page flips, or textures for a picture with no screen— is its `Output`.

use crate::layers::{self, ClientLayer, ToLayers};
use pleamar::scene::{Cursor, Keyboard, Level, PieceContent, Surface, SurfaceAnchor, ToRender};
use pleamar::wgpu;
use pleamar::{Frames, PlatformWindow};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Where a monitor's frames end up.
pub trait Output: Send {
    /// A buffer to put the next frame together in, and which one; none if all
    /// are still on screen or on their way.
    fn buffer(&mut self, device: &wgpu::Device, modifiers: &[u64]) -> Option<(usize, wgpu::Texture)>;
    /// Show that one once `done` has been done. Whether a flip is now on its
    /// way —its landing will be told— or it is already shown.
    fn show(&mut self, which: usize, done: pleamar::Sent, device: &wgpu::Device, queue: &wgpu::Queue, anew: bool) -> bool;
}

/// A monitor standing on its side (or upside down): everything is put
/// together upright, in a buffer of the size the monitor has once turned,
/// and turned into the real one at the end. One more pass, and only on that
/// monitor; nothing before it knows the monitor is turned. `turn` in quarter
/// turns, the way wlroots' and Hyprland's `transform` goes: 1 puts what is
/// upright's top left corner in the real buffer's top right.
pub struct Turned {
    inner: Box<dyn Output>,
    turn: u8,
    /// Its size upright, as everything else sees it.
    size: (u32, u32),
    /// One upright buffer for each of the real ones: each keeps what it had,
    /// so only what changed is put together again, as on any monitor.
    upright: Vec<Option<(wgpu::Texture, wgpu::Texture)>>,
    pass: Option<(wgpu::RenderPipeline, wgpu::Sampler, wgpu::Buffer)>,
}

impl Turned {
    /// `size`: the monitor's own, as its mode says; what is put together is
    /// that turned, if the turn is a quarter or three.
    pub fn new(inner: Box<dyn Output>, turn: u8, size: (u32, u32)) -> Turned {
        let size = if turn % 2 == 1 { (size.1, size.0) } else { size };
        Turned { inner, turn: turn % 4, size, upright: Vec::new(), pass: None }
    }
}

/// From the real buffer's point to the upright one's, in 0..1.
const TURN: &str = "
struct Turn { turn: u32, _a: u32, _b: u32, _c: u32 };
@group(0) @binding(0) var upright: texture_2d<f32>;
@group(0) @binding(1) var nearest: sampler;
@group(0) @binding(2) var<uniform> t: Turn;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) v: u32) -> Out {
    let p = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u));
    var o: Out;
    o.pos = vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    o.uv = p;
    return o;
}
@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    let u = o.uv.x;
    let v = o.uv.y;
    var q = vec2<f32>(u, v);
    if (t.turn == 1u) { q = vec2<f32>(v, 1.0 - u); }
    if (t.turn == 2u) { q = vec2<f32>(1.0 - u, 1.0 - v); }
    if (t.turn == 3u) { q = vec2<f32>(1.0 - v, u); }
    return textureSampleLevel(upright, nearest, q, 0.0);
}
";

impl Output for Turned {
    fn buffer(&mut self, device: &wgpu::Device, modifiers: &[u64]) -> Option<(usize, wgpu::Texture)> {
        let (which, real) = self.inner.buffer(device, modifiers)?;
        if self.upright.len() <= which {
            self.upright.resize_with(which + 1, || None);
        }
        // A new real buffer (the monitor was set up again): a new upright one too.
        let stale = self.upright[which].as_ref().is_none_or(|(_, r)| *r != real);
        if stale {
            let up = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("upright monitor"),
                size: wgpu::Extent3d { width: self.size.0, height: self.size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.upright[which] = Some((up, real.clone()));
        }
        let (up, _) = self.upright[which].as_ref()?;
        Some((which, up.clone()))
    }

    fn show(&mut self, which: usize, done: pleamar::Sent, device: &wgpu::Device, queue: &wgpu::Queue, anew: bool) -> bool {
        let Some((up, real)) = self.upright.get(which).and_then(|u| u.clone()) else { return self.inner.show(which, done, device, queue, anew) };
        let (pipeline, sampler, uniform) = self.pass.get_or_insert_with(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("turn"), source: wgpu::ShaderSource::Wgsl(TURN.into()) });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("turn"),
                layout: None,
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Bgra8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, ..Default::default() });
            let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            (pipeline, sampler, uniform)
        });
        queue.write_buffer(uniform, 0, &[self.turn as u32, 0, 0, 0].iter().flat_map(|v: &u32| v.to_ne_bytes()).collect::<Vec<u8>>());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&up.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });
        let view = real.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("turn"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, &group, &[]);
            rp.draw(0..3, 0..1);
        }
        drop(done);
        queue.submit(Some(encoder.finish()));
        self.inner.show(which, pleamar::Sent::after(queue), device, queue, anew)
    }
}

/// One surface of the scene on a monitor.
pub struct Layer {
    pub sheet: u32,
    pub surface: usize,
    /// Where it looks from in the scene's plane.
    pub origin: (f32, f32),
    /// Where it is on the monitor: x, y, width, height, in its pixels.
    pub rect: [i32; 4],
    /// Its size in the scene's units, and how many pixels each is (the
    /// monitor's scale): the render paints `units` × `scale` pixels.
    pub units: (u32, u32),
    pub scale: f32,
    pub level: Level,
    pub main: bool,
    pub anchor: SurfaceAnchor,
    pub margin: [i32; 4],
    /// The last frame it painted.
    pub latest: Option<wgpu::Texture>,
    /// Where it takes the pointer, from its corner (its zones).
    pub region: Vec<[i32; 4]>,
    /// `captures: hidden`: on the monitor, not in the pictures taken of it.
    pub unshared: bool,
}

pub struct ScreenState {
    pub name: String,
    pub size: (u32, u32),
    pub layers: Vec<Layer>,
    /// Something new has been painted since the last time it was put together.
    pub dirty: bool,
    /// No flip on its way.
    pub idle: bool,
    pub paused: bool,
    /// Back from another TTY: the monitor has to be given its mode again.
    pub anew: bool,
    /// The sheets whose new frame goes with the next flip, and the ones
    /// waiting for the flip on its way to land.
    pub fresh: Vec<u32>,
    pub on_flip: Vec<u32>,
    pub modifiers: Vec<u64>,
    /// The programs' surfaces on it (layer-shell): Marea, a bar, a wallpaper.
    pub clients: Vec<ClientLayer>,
    /// The programs' buffers it read the last time it was put together: lent
    /// until it no longer shows them.
    pub held: Vec<u64>,
    /// Buffers the programs destroyed, to drop what was kept of them.
    pub forget: Vec<u64>,
    /// What has changed on it since it was last put together, on the monitor
    /// (x, y, w, h); `changed_all` when all of it may have.
    /// And whose each change is (0, the scene's).
    pub changed: Vec<([i32; 4], u64)>,
    pub changed_all: bool,
    /// Pictures programs asked for: which, and of what piece.
    /// With `true`, not before something in that piece changes.
    /// And the program that asked: its own changes do not count for it.
    pub captures: Vec<(u64, [i32; 4], bool, u64)>,
    output: Option<Box<dyn Output>>,
    pub quit: bool,
    /// A window is fullscreen on this monitor: other programs' bars (their
    /// `top` layer) are not shown over it, as on any desktop.
    pub fullscreen: bool,
}

impl ScreenState {
    /// Something changed there. Only putting the monitor together reads these,
    /// and an off monitor (DPMS, another TTY) is not put together: past a few
    /// hundred, «all of it» says the same, and the list stops growing all night.
    pub fn note_change(&mut self, rect: [i32; 4], owner: u64) {
        if self.changed_all {
            return;
        }
        if self.changed.len() >= 512 {
            self.changed.clear();
            self.changed_all = true;
            return;
        }
        self.changed.push((rect, owner));
    }
}

pub type Screen = Arc<(Mutex<ScreenState>, Condvar)>;

pub fn screen(name: String, size: (u32, u32), output: Box<dyn Output>) -> Screen {
    Arc::new((
        Mutex::new(ScreenState { name, size, layers: Vec::new(), dirty: false, idle: true, paused: false, anew: true, fresh: Vec::new(), on_flip: Vec::new(), modifiers: Vec::new(), clients: Vec::new(), held: Vec::new(), forget: Vec::new(), changed: Vec::new(), changed_all: true, captures: Vec::new(), output: Some(output), quit: false, fullscreen: false }),
        Condvar::new(),
    ))
}

/// Where a surface goes on a monitor of that size: its size (0 is all of
/// it, minus the margins) and its anchor, with the margins from the edges
/// it is attached to. Top, right, bottom, left.
pub fn place(s_size: (u32, u32), anchor: SurfaceAnchor, margin: [i32; 4], (mw, mh): (u32, u32)) -> [i32; 4] {
    let [top, right, bottom, left] = margin;
    let w = if s_size.0 == 0 { mw as i32 - left - right } else { s_size.0 as i32 };
    let h = if s_size.1 == 0 { mh as i32 - top - bottom } else { s_size.1 as i32 };
    let [l, t, r, b] = anchor.attached_edges();
    let x = if l { left } else if r { mw as i32 - w - right } else { (mw as i32 - w) / 2 };
    let y = if t { top } else if b { mh as i32 - h - bottom } else { (mh as i32 - h) / 2 };
    [x, y, w.max(1), h.max(1)]
}

pub fn level_rank(l: Level) -> u8 {
    match l {
        Level::Background => 0,
        Level::Below => 1,
        Level::Above => 2,
        Level::Overlay => 3,
    }
}

/// A layer for a surface of the scene on a monitor of that many pixels, at
/// that scale: laid out in units, placed in pixels.
pub fn layer(sheet: u32, k: usize, s: &Surface, monitor: (u32, u32), scale: f32) -> Layer {
    let main = s.name.is_empty();
    let (units, rect) = placed((s.width, s.height), s.anchor, s.margin, monitor, scale);
    Layer { sheet, surface: k, origin: s.origin, rect, units, scale, level: s.level, main, anchor: s.anchor, margin: s.margin, latest: None, region: Vec::new(), unshared: s.hidden_from_captures }
}

/// Where a surface goes, laid out in units on a monitor of that many pixels:
/// its size in units, and its box in pixels.
pub fn placed(s_size: (u32, u32), anchor: SurfaceAnchor, margin: [i32; 4], monitor: (u32, u32), scale: f32) -> ((u32, u32), [i32; 4]) {
    let logical = ((monitor.0 as f32 / scale).round() as u32, (monitor.1 as f32 / scale).round() as u32);
    let r = place(s_size, anchor, margin, logical);
    let px = |v: i32| (v as f32 * scale).round() as i32;
    ((r[2] as u32, r[3] as u32), [px(r[0]), px(r[1]), px(r[2]).max(1), px(r[3]).max(1)])
}

/// Who takes the pointer at a point of a monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// The scene, at that point of its plane (none: nowhere).
    Scene(Option<(f32, f32)>),
    /// A program's surface, at that point of it.
    Client(u64, (f64, f64)),
}

/// What is put together on a monitor, bottom to top: by level; within a
/// level the scene's own surface, then its named ones in the order they were
/// declared, then the programs' in the order they came (at the overlay
/// level the scene's named ones go last, over the programs' menus).
enum Item {
    Scene(usize),
    Client(usize),
}

fn stacked(st: &ScreenState) -> Vec<Item> {
    let mut all: Vec<((u8, u8, usize), Item)> = Vec::new();
    // Locked, only the lock screen: nothing else is seen or touched.
    if layers::locked() {
        return st.clients.iter().enumerate().filter(|(_, c)| c.level >= 4).map(|(k, _)| Item::Client(k)).collect();
    }
    for (k, l) in st.layers.iter().enumerate() {
        // A named surface of the scene at the overlay level goes over the
        // programs' of that level too (their menus): the agent's cursors.
        let over = !l.main && level_rank(l.level) == 3;
        all.push(((level_rank(l.level), if l.main { 0 } else if over { 3 } else { 1 }, l.surface), Item::Scene(k)));
    }
    for (k, c) in st.clients.iter().enumerate() {
        if st.fullscreen && c.level == 2 {
            continue;
        }
        all.push(((c.level, 2, k), Item::Client(k)));
    }
    all.sort_by_key(|(key, _)| *key);
    all.into_iter().map(|(_, i)| i).collect()
}

/// The pointer at that point of a monitor: the highest surface that takes
/// it there —a named one only where it has zones, a program's where its
/// input region says—; the scene's own takes whatever is left above it.
pub fn pointer_at(st: &ScreenState, (x, y): (f64, f64)) -> Hit {
    let inside = |r: &[i32; 4], px: f64, py: f64| px >= r[0] as f64 && py >= r[1] as f64 && px < (r[0] + r[2]) as f64 && py < (r[1] + r[3]) as f64;
    let scene = |l: &Layer| Hit::Scene(Some((l.origin.0 + (x - l.rect[0] as f64) as f32 / l.scale, l.origin.1 + (y - l.rect[1] as f64) as f32 / l.scale)));
    for item in stacked(st).iter().rev() {
        match item {
            Item::Scene(k) => {
                let l = &st.layers[*k];
                if l.main {
                    return scene(l);
                }
                let (lx, ly) = ((x - l.rect[0] as f64) / l.scale as f64, (y - l.rect[1] as f64) / l.scale as f64);
                if l.latest.is_some() && inside(&l.rect, x, y) && l.region.iter().any(|b| lx >= b[0] as f64 && ly >= b[1] as f64 && lx < b[2] as f64 && ly < b[3] as f64) {
                    return scene(l);
                }
            }
            Item::Client(k) => {
                let c = &st.clients[*k];
                if c.takes(x, y) {
                    return Hit::Client(c.id, c.local(x, y));
                }
            }
        }
    }
    Hit::Scene(None)
}

/// The program's surface that takes all the keyboard on this monitor, if one does.
pub fn keyboard_taker(st: &ScreenState) -> Option<u64> {
    // The highest that asks for it: the lock screen over everything.
    let locked = layers::locked();
    st.clients.iter().rev().filter(|c| c.keyboard == 1 && c.level >= 2 && !c.pieces.is_empty() && (!locked || c.level >= 4)).max_by_key(|c| c.level).map(|c| c.id)
}

/// Whether that program's surface takes the keyboard when clicked.
pub fn takes_keyboard_on_click(st: &ScreenState, id: u64) -> bool {
    st.clients.iter().any(|c| c.id == id && c.keyboard != 0)
}

/// A surface's frames: two textures of its own, lent in turn to the render;
/// the last one painted is what the monitor puts together.
pub struct LayerFrames {
    pub screen: Screen,
    pub sheet: u32,
    pub size: (u32, u32),
    pub to_render: Sender<ToRender>,
    textures: Vec<wgpu::Texture>,
    latest: Option<usize>,
}

impl LayerFrames {
    pub fn new(screen: Screen, sheet: u32, size: (u32, u32), to_render: Sender<ToRender>) -> Self {
        LayerFrames { screen, sheet, size, to_render, textures: Vec::new(), latest: None }
    }
}

impl Frames for LayerFrames {
    fn acquire(&mut self, device: &wgpu::Device, modifiers: &[u64]) -> Option<(usize, wgpu::Texture)> {
        {
            let mut st = self.screen.0.lock().unwrap();
            if st.paused {
                return None;
            }
            if st.modifiers.is_empty() {
                st.modifiers = modifiers.to_vec();
            }
        }
        if self.textures.is_empty() {
            for _ in 0..2 {
                self.textures.push(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("a surface's frame"),
                    size: wgpu::Extent3d { width: self.size.0.max(1), height: self.size.1.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                }));
            }
        }
        let k = self.latest.map_or(0, |l| 1 - l);
        Some((k, self.textures[k].clone()))
    }

    fn present(&mut self, which: usize, _done: wgpu::SubmissionIndex, device: &wgpu::Device, queue: &wgpu::Queue, changed: Option<[u32; 4]>) {
        self.latest = Some(which);
        let (lock, cv) = &*self.screen;
        let mut st = lock.lock().unwrap();
        let mut piece = None;
        if let Some(l) = st.layers.iter_mut().find(|l| l.sheet == self.sheet) {
            // Its first frame changes all it covers.
            let first = l.latest.is_none();
            l.latest = Some(self.textures[which].clone());
            piece = match changed {
                Some(c) if !first => Some([l.rect[0] + c[0] as i32, l.rect[1] + c[1] as i32, c[2] as i32, c[3] as i32]),
                _ => Some(l.rect),
            };
        }
        match piece {
            Some(p) if p[2] > 0 && p[3] > 0 => st.note_change(p, 0),
            Some(_) => {}
            None => st.changed_all = true,
        }
        // Once: a monitor that is off does not take them, and the scene goes on painting.
        if !st.fresh.contains(&self.sheet) {
            st.fresh.push(self.sheet);
        }
        st.dirty = true;
        // The first frame starts the one that puts the monitor together.
        if let Some(output) = st.output.take() {
            let (screen, device, queue, to_render) = (self.screen.clone(), device.clone(), queue.clone(), self.to_render.clone());
            let name = st.name.clone();
            let _ = std::thread::Builder::new().name(format!("screen {name}")).spawn(move || compose_loop(screen, output, device, queue, to_render));
        }
        cv.notify_all();
    }
}

/// A surface's sheet as the render sees it: its zones are where it takes the
/// pointer; the rest goes to what is below.
pub struct LayerWindow {
    pub screen: Screen,
    pub sheet: u32,
    pub cursor: Arc<Mutex<Cursor>>,
}

impl PlatformWindow for LayerWindow {
    fn update_input_region(&self, boxes: &[[i32; 4]]) {
        let mut st = self.screen.0.lock().unwrap();
        if let Some(l) = st.layers.iter_mut().find(|l| l.sheet == self.sheet) {
            l.region = boxes.to_vec();
        }
    }
    fn cursor(&self, c: Cursor) {
        *self.cursor.lock().unwrap() = c;
        layers::cursor(false, c);
    }
    fn keyboard(&self, _: Keyboard) {}
}

const SHADER: &str = r#"
struct Rect { r: vec4<f32>, f: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(0) @binding(2) var<uniform> q: Rect;
struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> V {
    let c = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
    let p = mix(q.r.xy, q.r.zw, c);
    var v: V;
    v.pos = vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    v.uv = c;
    return v;
}
@fragment fn fs(v: V) -> @location(0) vec4<f32> {
    let c = textureSample(t, s, v.uv);
    // A buffer without alpha (XRGB) covers, whatever its fourth byte holds.
    return select(c, vec4<f32>(c.rgb, 1.0), q.f.x > 0.5);
}
"#;

/// Dual-Kawase blur: four taps on the diagonals, a little further each pass.
const BLUR: &str = r#"
struct K { texel: vec2<f32>, offset: f32, pad: f32 };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(0) @binding(2) var<uniform> k: K;
struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> V {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var v: V;
    v.pos = vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    v.uv = p;
    return v;
}
@fragment fn fs(v: V) -> @location(0) vec4<f32> {
    let o = (k.offset + 0.5) * k.texel;
    let c = textureSample(t, s, v.uv + vec2<f32>(o.x, o.y)) + textureSample(t, s, v.uv + vec2<f32>(-o.x, o.y))
          + textureSample(t, s, v.uv + vec2<f32>(o.x, -o.y)) + textureSample(t, s, v.uv + vec2<f32>(-o.x, -o.y));
    return vec4<f32>((c * 0.25).rgb, 1.0);
}
"#;

/// How much smaller what is behind is painted before blurring it, and how many passes.
const BLUR_DOWN: u32 = 4;
const BLUR_PASSES: u32 = 3;

/// Where a quad takes its pixels from.
enum Source {
    Texture(wgpu::Texture),
    /// A program's buffer on the card, by number.
    Buffer(u64),
    /// A program's pixels, copied into a texture of the surface's.
    Pixels(u64),
}

/// A bind group already made for a texture at a place: remade only when
/// either changes, not every time the monitor is put together.
struct Bound {
    texture: wgpu::Texture,
    uniform: [f32; 8],
    group: wgpu::BindGroup,
    /// The last time it was put together with it.
    used: u64,
}

/// Puts the monitor together whenever something new has been painted and
/// the last flip has landed: at most once per refresh.
fn compose_loop(screen: Screen, mut output: Box<dyn Output>, device: wgpu::Device, queue: wgpu::Queue, to_render: Sender<ToRender>) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("screen"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("screen"),
        layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            // What the render paints is premultiplied, and so is what the programs hand over.
            targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Bgra8Unorm, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, ..Default::default() });
    let smooth = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let layout = pipeline.get_bind_group_layout(0);
    let blur_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("blur"), source: wgpu::ShaderSource::Wgsl(BLUR.into()) });
    let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("blur"),
        layout: None,
        vertex: wgpu::VertexState { module: &blur_module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &blur_module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Bgra8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let blur_layout = blur_pipeline.get_bind_group_layout(0);
    // The two small textures the blur goes back and forth between, kept while their size holds.
    let mut blur_room: Option<[wgpu::Texture; 2]> = None;
    let mut bound: Vec<Bound> = Vec::new();
    let mut readback = Readback::default();
    let mut round = 0u64;
    // What changed in each of the last times it was put together, and when
    // each of the output's buffers was last put together: each is only put
    // together again where something changed since then.
    let mut changes: std::collections::VecDeque<(u64, Option<[i32; 4]>)> = Default::default();
    let mut last_in: Vec<Option<u64>> = Vec::new();
    let mut time_no = 0u64;
    // Black, to empty the piece put together again.
    let black = client_texture(&device, (1, 1));
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &black, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &[0, 0, 0, 255],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let black_group = {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&buffer, 0, as_bytes(&[0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&black.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() },
            ],
        })
    };
    // `PLEAMAR_TIMING=1`: how many times a second it is put together, how long
    // it waits for the card, and the CPU each time costs.
    let timing = std::env::var_os("PLEAMAR_TIMING").is_some();
    let mut tally = (std::time::Instant::now(), 0u32, 0f64, 0f64);
    // The CPU of each part: gathering, reading what programs brought, recording,
    // sending, showing (the wait and the flip), and the rest.
    let mut parts = [0f64; 6];
    let mut mark = if timing { thread_cpu_ms() } else { 0.0 };
    let mut part = |k: usize, parts: &mut [f64; 6]| {
        if timing {
            let now = thread_cpu_ms();
            parts[k] += now - mark;
            mark = now;
        }
    };
    // The programs' buffers read so far, and each surface's copied pixels.
    let mut buffers: std::collections::HashMap<u64, wgpu::Texture> = Default::default();
    let mut pixels: std::collections::HashMap<u64, wgpu::Texture> = Default::default();
    // Programs' video buffers (NV12) as they are; `buffers` has them as RGB.
    let mut frames: std::collections::HashMap<u64, wgpu::Texture> = Default::default();
    let mut yuv: Option<pleamar::dmabuf::YuvToRgb> = None;
    let debug = std::env::var_os("PLEAMAR_DEBUG_SCREEN").is_some();
    let (lock, cv) = &*screen;
    loop {
        let (quads, blurs, size, modifiers, fresh, anew, arrived, forget, shows, drew_clients, surfaces, changed, captures, unshared) = {
            let mut st = lock.lock().unwrap();
            while !st.quit && !(st.dirty && st.idle && !st.paused) {
                st = cv.wait_timeout(st, Duration::from_millis(500)).unwrap().0;
            }
            if st.quit {
                return;
            }
            if capture_debug() {
                eprintln!("capture · {:.1} {} composing ({} pictures asked)", wall_ms(), st.name, st.captures.len());
            }
            st.dirty = false;
            let mut quads: Vec<(Source, [i32; 4], bool)> = Vec::new();
            // The quads left out of the pictures taken of the monitor.
            let mut unshared: Vec<usize> = Vec::new();
            // Before which quad what is behind gets blurred, and where (a program's glass).
            let mut blurs: Vec<(usize, Vec<[i32; 4]>)> = Vec::new();
            let mut arrived: Vec<(u64, Option<u64>, (u32, u32), PieceContent)> = Vec::new();
            let mut shows: Vec<u64> = Vec::new();
            let order = stacked(&st);
            for item in &order {
                match item {
                    Item::Scene(k) => {
                        let l = &st.layers[*k];
                        if let Some(t) = &l.latest {
                            if l.unshared {
                                unshared.push(quads.len());
                            }
                            quads.push((Source::Texture(t.clone()), l.rect, false));
                        }
                    }
                    Item::Client(k) => {
                        let c = &st.clients[*k];
                        if !c.blur.is_empty() && !c.pieces.is_empty() {
                            let z = c.zoom;
                            blurs.push((quads.len(), c.blur.iter().map(|b| [c.rect[0] + (b[0] as f64 * z).round() as i32, c.rect[1] + (b[1] as f64 * z).round() as i32, (b[2] as f64 * z).ceil() as i32, (b[3] as f64 * z).ceil() as i32]).collect()));
                        }
                        for p in &c.pieces {
                            let r = c.piece_rect(p);
                            let source = match p.buffer {
                                Some(b) => Source::Buffer(b),
                                None => Source::Pixels(p.key),
                            };
                            quads.push((source, r, p.opaque));
                        }
                    }
                }
            }
            // The copied pixels of every surface still there are kept, drawn
            // now or not: a wallpaper does not send them again after a lock.
            let mut surfaces: Vec<u64> = Vec::new();
            for c in &mut st.clients {
                for p in &mut c.pieces {
                    if let Some(content) = p.content.take() {
                        arrived.push((p.key, p.buffer, p.px, content));
                    }
                    shows.extend(p.buffer);
                    surfaces.push(p.key);
                }
            }
            if debug {
                eprintln!("screen · {}: {} surfaces of the scene, {} of programs, {} new", st.name, st.layers.len(), st.clients.len(), arrived.len());
            }
            let anew = std::mem::take(&mut st.anew);
            let drew_clients = !st.clients.is_empty();
            // What changed, as one box (none: all of it).
            // `PLEAMAR_FULL_COMPOSE=1`: all of it every time, to compare.
            // `PLEAMAR_FULL_COMPOSE=1`: all of it every time, to compare.
            let mut changed = if std::mem::take(&mut st.changed_all) || anew || std::env::var_os("PLEAMAR_FULL_COMPOSE").is_some() {
                st.changed.clear();
                None
            } else {
                let b = st.changed.iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |a, (r, _)| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[0] + r[2]), a[3].max(r[1] + r[3])]);
                Some(b)
            };
            let changes = std::mem::take(&mut st.changed);
            // The pictures due now: the plain ones, and the ones waiting for a
            // change once something changed where they look.
            // Waiting for a change: someone else's, where it looks. A program's
            // own frame is not «what is behind changed»: counted, its glass
            // watched itself and painted again, sixty times a second.
            let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut st.captures).into_iter().partition(|(_, p, on_change, owner)| {
                !on_change || changed.is_none() || changes.iter().any(|(r, who)| who != owner && p[0] < r[0] + r[2] && p[0] + p[2] > r[0] && p[1] < r[1] + r[3] && p[1] + p[3] > r[1])
            });
            st.captures = waiting;
            // One picture for each program each time: a recorder that asks for
            // the next one ahead (so as not to miss a time) gets it the next
            // time something changes, not this same picture twice.
            let mut each = std::collections::HashSet::new();
            let (due, later): (Vec<_>, Vec<_>) = due.into_iter().partition(|c| each.insert(c.3));
            // (A plain one —not waiting for a change— is due at once: again.)
            if later.iter().any(|c| !c.2) {
                st.dirty = true;
            }
            st.captures.extend(later);
            // A picture is taken of all of it: put together whole.
            if !due.is_empty() {
                changed = None;
            }
            (quads, blurs, st.size, st.modifiers.clone(), std::mem::take(&mut st.fresh), anew, arrived, std::mem::take(&mut st.forget), shows, drew_clients, surfaces, changed, due, unshared)
        };
        part(0, &mut parts);
        for b in &forget {
            buffers.remove(b);
            frames.remove(b);
        }
        // What the programs brought since the last time: their buffers on the
        // card are read where they are; their pixels, copied.
        for (key, buffer, (w, h), content) in arrived {
            match content {
                PieceContent::Dmabuf(d) => {
                    let Some(b) = buffer else { continue };
                    let video = d.fourcc == pleamar::dmabuf::NV12;
                    // RGB is read where it is, once; video is painted as RGB
                    // into a texture of its own every time it brings a frame.
                    if !video && buffers.contains_key(&b) {
                        continue;
                    }
                    if video {
                        if !frames.contains_key(&b) {
                            match pleamar::dmabuf::import(&device, d, (w, h), wgpu::TextureUses::RESOURCE, wgpu::TextureUsages::TEXTURE_BINDING, wgpu::TextureUses::RESOURCE) {
                                Ok(t) => {
                                    frames.insert(b, t);
                                    buffers.insert(b, rgb_texture(&device, (w, h)));
                                }
                                Err(e) => {
                                    eprintln!("screen · a program's video could not be read: {e}");
                                    continue;
                                }
                            }
                        }
                        let (Some(frame), Some(rgb)) = (frames.get(&b), buffers.get(&b)) else { continue };
                        let yuv = yuv.get_or_insert_with(|| pleamar::dmabuf::YuvToRgb::new(&device, wgpu::TextureFormat::Bgra8Unorm));
                        let mut encoder = device.create_command_encoder(&Default::default());
                        yuv.convert(&device, &mut encoder, frame, &rgb.create_view(&Default::default()), (w, h));
                        queue.submit(Some(encoder.finish()));
                        continue;
                    }
                    match pleamar::dmabuf::import(&device, d, (w, h), wgpu::TextureUses::RESOURCE, wgpu::TextureUsages::TEXTURE_BINDING, wgpu::TextureUses::RESOURCE) {
                        Ok(t) => {
                            buffers.insert(b, t);
                        }
                        Err(e) => eprintln!("screen · a program's buffer could not be read: {e}"),
                    }
                }
                PieceContent::Pixels(data) => {
                    if data.len() < (w * h * 4) as usize {
                        continue;
                    }
                    let t = pixels.entry(key).or_insert_with(|| client_texture(&device, (w, h)));
                    if t.size().width != w || t.size().height != h {
                        *t = client_texture(&device, (w, h));
                    }
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                        &data,
                        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: None },
                        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    );
                }
                PieceContent::Kept => {}
            }
        }
        // A buffer destroyed in the same round it arrived: forgotten before it
        // was read, it was read after, and its texture was kept for good
        // (buffer numbers are never used again).
        for b in &forget {
            buffers.remove(b);
            frames.remove(b);
        }
        part(1, &mut parts);
        let Some((which, target)) = output.buffer(&device, &modifiers) else {
            // Nothing to put it together in yet: again as soon as there is, with
            // all it had to do (what changed and the pictures asked for).
            let mut st = lock.lock().unwrap();
            st.dirty = true;
            st.fresh.extend(fresh);
            match changed {
                Some(b) if b[2] > b[0] && b[3] > b[1] => st.note_change([b[0], b[1], b[2] - b[0], b[3] - b[1]], 0),
                Some(_) => {}
                None => st.changed_all = true,
            }
            st.captures.extend(captures);
            drop(st);
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        let view = target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        // The piece of this buffer to put together again: what changed since it
        // was last put together, if that is known; all of it with glass (the
        // blur reads around it) or the first time.
        time_no += 1;
        changes.push_back((time_no, changed));
        while changes.len() > 8 {
            changes.pop_front();
        }
        if last_in.len() <= which {
            last_in.resize(which + 1, None);
        }
        let since = last_in[which].replace(time_no);
        let piece: Option<[i32; 4]> = if !blurs.is_empty() {
            None
        } else {
            since.and_then(|p| {
                let complete = changes.front().is_some_and(|(n, _)| *n <= p + 1);
                complete.then(|| changes.iter().filter(|(n, _)| *n > p).try_fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |a, (_, b)| b.map(|b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])))?
            })
        };
        // In pixels of the monitor, within it; empty if nothing changed.
        let scissor: Option<[u32; 4]> = piece.map(|b| {
            let x0 = b[0].clamp(0, size.0 as i32);
            let y0 = b[1].clamp(0, size.1 as i32);
            let x1 = b[2].clamp(x0, size.0 as i32);
            let y1 = b[3].clamp(y0, size.1 as i32);
            [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]
        });
        let touches = |r: &[i32; 4]| scissor.is_none_or(|s| r[0] < (s[0] + s[2]) as i32 && r[0] + r[2] > s[0] as i32 && r[1] < (s[1] + s[3]) as i32 && r[1] + r[3] > s[1] as i32);
        round += 1;
        // Each quad's bind group, if what it shows is there.
        let mut groups: Vec<Option<usize>> = Vec::new();
        for (source, r, opaque) in &quads {
            let texture = match source {
                Source::Texture(t) => Some(t),
                Source::Buffer(b) => buffers.get(b),
                Source::Pixels(k) => pixels.get(k),
            };
            // Nothing of it falls where it is put together again: not drawn.
            let Some(texture) = texture.filter(|_| touches(r)) else {
                groups.push(None);
                continue;
            };
            let (w, h) = (size.0 as f32, size.1 as f32);
            let uniform = [r[0] as f32 / w, r[1] as f32 / h, (r[0] + r[2]) as f32 / w, (r[1] + r[3]) as f32 / h, if *opaque { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            let k = match bound.iter().position(|b| b.used != round && &b.texture == texture && b.uniform == uniform) {
                Some(k) => k,
                None => {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                    queue.write_buffer(&buffer, 0, as_bytes(&uniform));
                    let tv = texture.create_view(&Default::default());
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tv) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                            wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() },
                        ],
                    });
                    bound.push(Bound { texture: texture.clone(), uniform, group, used: round });
                    bound.len() - 1
                }
            };
            bound[k].used = round;
            groups.push(Some(k));
        }
        // In stretches: up to each glass, what is below it is put together; then
        // that is blurred where the glass asks, and the rest goes on top.
        let (w, h) = (size.0 as f32, size.1 as f32);
        let mut from = 0;
        // Put together again only in a piece: what is there stays, and the
        // piece is emptied to black first (a clear would empty all of it).
        let mut load = if scissor.is_some() { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) };
        let mut empty_first = scissor.is_some();
        let blank = scissor.is_some_and(|s| s[2] == 0 || s[3] == 0);
        let black_group = empty_first.then_some(&black_group);
        for (upto, glass) in blurs.iter().map(|(at, r)| (*at, Some(r))).chain(std::iter::once((quads.len(), None))) {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("screen"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                if blank {
                    from = upto;
                    continue;
                }
                if let Some(sc) = scissor {
                    pass.set_scissor_rect(sc[0], sc[1], sc[2], sc[3]);
                    if std::mem::take(&mut empty_first) {
                        if let Some(g) = &black_group {
                            pass.set_bind_group(0, *g, &[]);
                            pass.draw(0..4, 0..1);
                        }
                    }
                }
                for k in groups[from..upto].iter().flatten() {
                    pass.set_bind_group(0, &bound[*k].group, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
            load = wgpu::LoadOp::Load;
            from = upto;
            let Some(glass) = glass else { continue };
            // The box around the glass, on the monitor.
            let b = glass.iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |a, r| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[0] + r[2]), a[3].max(r[1] + r[3])]);
            let b = [b[0].max(0), b[1].max(0), b[2].min(size.0 as i32), b[3].min(size.1 as i32)];
            let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
            if bw <= 0 || bh <= 0 {
                continue;
            }
            let small = ((bw as u32 / BLUR_DOWN).max(1), (bh as u32 / BLUR_DOWN).max(1));
            if blur_room.as_ref().is_none_or(|r| r[0].size().width != small.0 || r[0].size().height != small.1) {
                let make = || {
                    device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("blur"),
                        size: wgpu::Extent3d { width: small.0, height: small.1, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Bgra8Unorm,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                };
                blur_room = Some([make(), make()]);
            }
            let room = blur_room.as_ref().expect("just made");
            let views = [room[0].create_view(&Default::default()), room[1].create_view(&Default::default())];
            let group_for = |layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView, sampler: &wgpu::Sampler, uniform: &[f32; 8]| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                queue.write_buffer(&buffer, 0, as_bytes(uniform));
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() },
                    ],
                })
            };
            // What is below, again, smaller: only the box.
            let below: Vec<wgpu::BindGroup> = quads[..upto]
                .iter()
                .zip(&groups)
                .filter_map(|((_, r, opaque), g)| {
                    let t = &bound[(*g)?].texture;
                    let (fx, fy) = (bw as f32, bh as f32);
                    let u = [(r[0] - b[0]) as f32 / fx, (r[1] - b[1]) as f32 / fy, (r[0] + r[2] - b[0]) as f32 / fx, (r[1] + r[3] - b[1]) as f32 / fy, if *opaque { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
                    Some(group_for(&layout, &t.create_view(&Default::default()), &smooth, &u))
                })
                .collect();
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("behind the glass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &views[0], depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                for g in &below {
                    pass.set_bind_group(0, g, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
            // Back and forth, a little further each time.
            let texel = [1.0 / small.0 as f32, 1.0 / small.1 as f32];
            for n in 0..BLUR_PASSES as usize {
                let g = group_for(&blur_layout, &views[n % 2], &smooth, &[texel[0], texel[1], n as f32, 0.0, 0.0, 0.0, 0.0, 0.0]);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("blur"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &views[(n + 1) % 2], depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&blur_pipeline);
                pass.set_bind_group(0, &g, &[]);
                pass.draw(0..3, 0..1);
            }
            // And onto the monitor, only where the glass is.
            let blurred = &views[BLUR_PASSES as usize % 2];
            let back = group_for(&layout, blurred, &smooth, &[b[0] as f32 / w, b[1] as f32 / h, b[2] as f32 / w, b[3] as f32 / h, 1.0, 0.0, 0.0, 0.0]);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &back, &[]);
            for r in glass {
                let x0 = r[0].clamp(0, size.0 as i32);
                let y0 = r[1].clamp(0, size.1 as i32);
                let x1 = (r[0] + r[2]).clamp(0, size.0 as i32);
                let y1 = (r[1] + r[3]).clamp(0, size.1 as i32);
                if x1 > x0 && y1 > y0 {
                    pass.set_scissor_rect(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
                    pass.draw(0..4, 0..1);
                }
            }
        }
        part(2, &mut parts);
        queue.submit(Some(encoder.finish()));
        let done = pleamar::Sent::after(&queue);
        part(3, &mut parts);
        // The pictures asked for: what was just put together, again, whole,
        // into a texture of its own, and read back.
        if capture_debug() && !captures.is_empty() {
            let t = std::time::Instant::now();
            done.wait(&device, Duration::from_secs(1));
            eprintln!("capture ·   the putting together took the card {:.1} ms more", t.elapsed().as_secs_f64() * 1000.0);
        }
        for (id, piece, _, _) in &captures {
            let t = std::time::Instant::now();
            capture(&device, &queue, &pipeline, &groups, &unshared, &bound, size, *piece, Some(&target), &mut readback, *id);
            if capture_debug() {
                eprintln!("capture · {:.1} picture asked of the card in {:.1} ms", wall_ms(), t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        // What has not been shown for a while is not kept (a surface's frames
        // take turns, so one that was not used this time may be the next).
        // Only now: `groups` points into `bound` by place, and the pictures
        // above still draw with it.
        bound.retain(|b| round - b.used < 8);
        let shown_at = std::time::Instant::now();
        // A flip on its way from before it is asked for: its landing may be told
        // before `show` returns (at once, headless; a fast event from the card),
        // and marking it after that left the monitor waiting for a flip that had
        // already landed —half a second, until the wait ran out—.
        lock.lock().unwrap().idle = false;
        let flying = output.show(which, done, &device, &queue, anew);
        if !flying {
            lock.lock().unwrap().idle = true;
        }
        part(4, &mut parts);
        if timing {
            tally.1 += 1;
            tally.2 += shown_at.elapsed().as_secs_f64() * 1000.0;
            tally.3 = thread_cpu_ms();
            if tally.1 >= 300 {
                let secs = tally.0.elapsed().as_secs_f64();
                let name = lock.lock().unwrap().name.clone();
                println!("screen · {name}: put together {:.0} times a second, {:.2} ms waiting for the card each, {:.2} ms of CPU each", tally.1 as f64 / secs, tally.2 / tally.1 as f64, (tally.3 - CPU_AT.with(|c| c.get())) / tally.1 as f64);
                let n = tally.1 as f64;
                println!("screen · {name}: CPU each time: gathering {:.2} · reading programs' frames {:.2} · recording {:.2} · sending {:.2} · showing {:.2} · the rest {:.2} ms", parts[0] / n, parts[1] / n, parts[2] / n, parts[3] / n, parts[4] / n, parts[5] / n);
                // What it keeps: over a long session, none of these should only grow.
                println!("screen · {name}: kept: {} programs' buffers, {} video frames, {} copied surfaces", buffers.len(), frames.len(), pixels.len());
                parts = [0.0; 6];
                CPU_AT.with(|c| c.set(tally.3));
                tally = (std::time::Instant::now(), 0, 0.0, tally.3);
            }
        }
        // The buffers read before and no longer shown go back to their programs
        // (`show` waited for the card to finish with them).
        let released: Vec<u64> = {
            let mut st = lock.lock().unwrap();
            let gone: Vec<u64> = st.held.iter().copied().filter(|b| !shows.contains(b)).collect();
            st.held = shows;
            // The surfaces whose frame went into this may paint the next one now,
            // not when the flip lands: the card has read their texture already
            // (`show` waited for it), and the next goes into their other one.
            // Waiting for the flip too put a whole refresh into each frame's
            // way, and a terminal that asked for 50 got 32.
            for id in fresh {
                let _ = to_render.send(ToRender::Frame(id));
            }
            gone
        };
        // What was read of a buffer given back is kept: the program draws into
        // it again in a moment (it goes round two or three), and reading it
        // anew each time was most of what a monitor cost. It goes when the
        // program destroys it (`forget`).
        pixels.retain(|k, _| surfaces.contains(k));
        if !released.is_empty() {
            layers::tell(ToLayers::Released(released));
        }
        if drew_clients {
            let name = lock.lock().unwrap().name.clone();
            layers::tell(ToLayers::FrameDone(name));
        }
        part(5, &mut parts);
    }
}

/// `PLEAMAR_DEBUG_CAPTURE=1`: when a monitor is put together and its pictures
/// read back, on the wall clock (ms), to follow one change to a recorder.
pub fn capture_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PLEAMAR_DEBUG_CAPTURE").is_some())
}

pub fn wall_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Where the pictures of a monitor are read back: kept while its size holds
/// (made anew each time, a picture of a big monitor spent most of its time
/// making them).
#[derive(Default)]
struct Readback {
    texture: Option<wgpu::Texture>,
    /// Each with whether it is still being read (a picture on its way).
    buffers: Vec<(u64, wgpu::Buffer, Arc<std::sync::atomic::AtomicBool>)>,
}

/// A piece of the monitor as it has just been put together (what the
/// programs and the scene show; not the cursor, which is on its own plane),
/// read back: BGRA, rows with no padding. Copied straight from what was put
/// together when all of it is for pictures too (`direct`); drawn again
/// without what is only for the monitor if not. Read on a thread of its own
/// and handed to whoever asked from there (`ToLayers::Captured`): the
/// monitor goes on meanwhile, instead of waiting for the card.
#[allow(clippy::too_many_arguments)]
fn capture(device: &wgpu::Device, queue: &wgpu::Queue, pipeline: &wgpu::RenderPipeline, groups: &[Option<usize>], unshared: &[usize], bound: &[Bound], size: (u32, u32), piece: [i32; 4], target: Option<&wgpu::Texture>, room: &mut Readback, id: u64) {
    let x0 = piece[0].clamp(0, size.0 as i32) as u32;
    let y0 = piece[1].clamp(0, size.1 as i32) as u32;
    let x1 = (piece[0] + piece[2]).clamp(x0 as i32, size.0 as i32) as u32;
    let y1 = (piece[1] + piece[3]).clamp(y0 as i32, size.1 as i32) as u32;
    let (w, h) = (x1 - x0, y1 - y0);
    if w == 0 || h == 0 {
        layers::tell(ToLayers::Captured { id, pixels: None });
        return;
    }
    let t_made = std::time::Instant::now();
    let mut encoder = device.create_command_encoder(&Default::default());
    let direct = target.filter(|t| unshared.is_empty() && t.usage().contains(wgpu::TextureUsages::COPY_SRC) && t.size().width == size.0 && t.size().height == size.1);
    let source = match direct {
        Some(t) => t.clone(),
        None => {
            if room.texture.as_ref().is_none_or(|t| t.size().width != size.0 || t.size().height != size.1) {
                room.texture = Some(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("a picture"),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                }));
            }
            let texture = room.texture.clone().unwrap();
            let view = texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("a picture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_scissor_rect(x0, y0, w, h);
            // All that was put together but what is only for the monitor.
            for (_, k) in groups.iter().enumerate().filter(|(i, _)| !unshared.contains(i)).filter_map(|(i, k)| k.map(|k| (i, k))) {
                pass.set_bind_group(0, &bound[k].group, &[]);
                pass.draw(0..4, 0..1);
            }
            drop(pass);
            texture
        }
    };
    let row = (w * 4).div_ceil(256) * 256;
    let bytes = (row * h) as u64;
    use std::sync::atomic::Ordering::{Acquire, Release};
    room.buffers.retain(|b| b.0 == bytes);
    let (out, busy) = match room.buffers.iter().find(|b| !b.2.load(Acquire)) {
        Some(b) => (b.1.clone(), b.2.clone()),
        None => {
            let b = device.create_buffer(&wgpu::BufferDescriptor { label: Some("a picture, read back"), size: bytes, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
            // A few kept; more at once (a slow card) are made and let go.
            if room.buffers.len() < 3 {
                room.buffers.push((bytes, b.clone(), busy.clone()));
            }
            (b, busy)
        }
    };
    busy.store(true, Release);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &source, mip_level: 0, origin: wgpu::Origin3d { x: x0, y: y0, z: 0 }, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));
    let mapped = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let flag = mapped.clone();
    out.slice(..).map_async(wgpu::MapMode::Read, move |r| flag.store(if r.is_ok() { 1 } else { 2 }, Release));
    let t_sub = std::time::Instant::now();
    let direct = direct.is_some();
    let device = device.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        while mapped.load(Acquire) == 0 && start.elapsed() < Duration::from_secs(2) {
            let _ = device.poll(wgpu::PollType::Poll);
            std::thread::sleep(Duration::from_micros(100));
        }
        if mapped.load(Acquire) != 1 {
            // (Still mapping, or it failed: that buffer is not used again.)
            layers::tell(ToLayers::Captured { id, pixels: None });
            return;
        }
        let t_map = std::time::Instant::now();
        let pixels = out.slice(..).get_mapped_range().ok().map(|data| {
            let mut pixels = Vec::with_capacity((w * h * 4) as usize);
            if row == w * 4 {
                pixels.extend_from_slice(&data[..(w * h * 4) as usize]);
            } else {
                for y in 0..h {
                    pixels.extend_from_slice(&data[(y * row) as usize..(y * row + w * 4) as usize]);
                }
            }
            pixels
        });
        out.unmap();
        busy.store(false, Release);
        if capture_debug() {
            let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1000.0;
            eprintln!("capture ·   {} recorded {:.1} · card {:.1} · copied {:.1} ms", if direct { "direct," } else { "drawn again," }, ms(t_made, t_sub), ms(t_sub, t_map), t_map.elapsed().as_secs_f64() * 1000.0);
        }
        layers::tell(ToLayers::Captured { id, pixels });
    });
}

/// Where a program's video frame is painted as RGB, to be put together from.
fn rgb_texture(device: &wgpu::Device, (w, h): (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("a program's video, as RGB"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn client_texture(device: &wgpu::Device, (w, h): (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("a program's pixels"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// A flip has landed on this monitor: the sheets whose frame it carried may
/// paint the next one, and the monitor can be put together again.
pub fn landed(screen: &Screen, to_render: &Sender<ToRender>) {
    let (lock, cv) = &**screen;
    let mut st = lock.lock().unwrap();
    st.idle = true;
    for id in st.on_flip.drain(..) {
        let _ = to_render.send(ToRender::Frame(id));
    }
    cv.notify_all();
}

fn as_bytes(v: &[f32; 8]) -> &[u8] {
    // SAFETY: eight f32 are thirty-two bytes, laid out as the uniform wants them.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, 32) }
}

thread_local! {
    /// The thread's CPU when the last tally was written.
    static CPU_AT: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// The CPU this thread has used, in milliseconds.
fn thread_cpu_ms() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid clock and a timespec of our own to fill.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) } == 0 {
        return t.tv_sec as f64 * 1000.0 + t.tv_nsec as f64 / 1e6;
    }
    0.0
}
