//! The phone's monitor: a monitor with no screen, put up while someone uses
//! the session from a phone (`pleamar-wm remote`, docs/phone.md). It is put
//! together like any other —each surface painted on its own, then laid
//! together— into textures of its own, and it "flips" at its refresh. What
//! the phone sees is taken from it as from any monitor (wlr-screencopy).

use crate::screen::{self, Output, Screen};
use pleamar::scene::ToRender;
use pleamar::wgpu;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// How often it refreshes: the stream to the phone is at most this.
pub const PHONE_MHZ: i32 = 60_000;

pub struct Virtual {
    size: (u32, u32),
    textures: Vec<wgpu::Texture>,
    shown: Option<usize>,
    /// Its next refresh: a steady clock, as a screen's.
    next: Instant,
    /// Its monitor, once made: what it tells when a flip "lands".
    screen: Arc<OnceLock<Screen>>,
    to_render: Sender<ToRender>,
}

impl Virtual {
    /// A monitor of that many pixels, and the place where its own screen will
    /// be once it is made (it is made from this output).
    pub fn new(size: (u32, u32), to_render: Sender<ToRender>) -> (Virtual, Arc<OnceLock<Screen>>) {
        let own = Arc::new(OnceLock::new());
        (Virtual { size, textures: Vec::new(), shown: None, next: Instant::now(), screen: own.clone(), to_render }, own)
    }
}

impl Output for Virtual {
    fn buffer(&mut self, device: &wgpu::Device, _: &[u64]) -> Option<(usize, wgpu::Texture)> {
        if self.textures.is_empty() {
            for _ in 0..3 {
                self.textures.push(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("phone monitor"),
                    size: wgpu::Extent3d { width: self.size.0, height: self.size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                }));
            }
        }
        let k = self.shown.map_or(0, |s| (s + 1) % 3);
        Some((k, self.textures[k].clone()))
    }

    fn show(&mut self, which: usize, done: pleamar::Sent, device: &wgpu::Device, _: &wgpu::Queue, _: bool) -> bool {
        self.shown = Some(which);
        let Some(sc) = self.screen.get().cloned() else {
            done.wait(device, Duration::from_millis(200));
            return false;
        };
        // The flip "lands" on its next refresh of a steady clock, once the card
        // has finished it: putting it together takes part of a refresh, not
        // one more. (Waiting for the card here and then a whole refresh more
        // made it some 40 a second, and every picture of it later.)
        let period = Duration::from_micros(1_000_000_000 / PHONE_MHZ as u64);
        let now = Instant::now();
        self.next = (self.next + period).max(now);
        let at = self.next;
        let (tx, device) = (self.to_render.clone(), device.clone());
        std::thread::spawn(move || {
            done.wait(&device, Duration::from_millis(200));
            if let Some(wait) = at.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            screen::landed(&sc, &tx);
        });
        true
    }
}

/// A phone's monitor, made: its screen, ready to be shown on.
pub fn make(size: (u32, u32), to_render: &Sender<ToRender>) -> Screen {
    let (output, own) = Virtual::new(size, to_render.clone());
    let sc = screen::screen(crate::layers::PHONE_NAME.to_owned(), size, Box::new(output));
    let _ = own.set(sc.clone());
    sc
}

/// Where the phone's monitor goes on the desktop: far to the right of the
/// real ones, with a gap no mouse crosses (the pointer stays on the nearest
/// monitor), so only a tap from the phone gets there.
pub fn place(right_edge: i32) -> (i32, i32) {
    (right_edge + 4096, 0)
}
