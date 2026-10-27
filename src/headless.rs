//! `pleamar-wm headless scene.plm [options]`: the session's way of showing a
//! scene —each surface painted in frames of its own, the monitor put together
//! from them— with no screen at all. After `PLEAMAR_HEADLESS_AT` seconds (8 by
//! default) what each monitor shows goes to a PNG (`PLEAMAR_HEADLESS_PNG`, by
//! default /tmp/pleamar-headless.png; `-1`, `-2`… for the others).
//! `PLEAMAR_HEADLESS_SCREENS=2` makes two monitors. It is how the session is
//! checked without a TTY. `PLEAMAR_HEADLESS_INPUT` is a mouse and a keyboard
//! that go where the session's real ones would (`route.rs`): to the scene, or
//! to a program's surface —Marea's finder, her catcher—.
//! What starts with it is only what `PLEAMAR_WM_AUTOSTART` names: never the
//! user's own autostart, which belongs to their real session.

use crate::cursor_theme::{self, Playing, Theme};
use crate::screen::{self, Hit, LayerFrames, LayerWindow, Output, Screen};
use pleamar::scene::{Cursor, Screens, Surface, ToRender};
use pleamar::wgpu;
use pleamar::{NewSheet, Target, View};
use smithay::reexports::calloop::{channel, timer::{TimeoutAction, Timer}, EventLoop, LoopHandle, RegistrationToken};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where `pleamar-wm output … at` put each monitor, by number; the others
/// side by side, as they start.
static ASKED_AT: Mutex<Vec<(usize, (i32, i32))>> = Mutex::new(Vec::new());

fn placed(m: usize, unit_w: i32) -> (i32, i32) {
    ASKED_AT.lock().unwrap().iter().find(|(k, _)| *k == m).map_or((m as i32 * unit_w, 0), |(_, at)| *at)
}

/// A monitor changed as `pleamar-wm output` asks, as a session would: one
/// that is not a card's cannot vary its refresh, and moving one moves it
/// in the desktop the programs and `hyprctl monitors` see. The pointer of
/// `PLEAMAR_HEADLESS_INPUT` still takes them side by side.
fn apply_output(name: &str, wish: &crate::output::Wish) -> Result<String, String> {
    if let Some(why) = wish.not_yet() {
        return Err(why);
    }
    let all = crate::layers::monitors();
    let real: Vec<_> = all.iter().filter(|m| m.name != crate::layers::PHONE_NAME).collect();
    if !real.iter().any(|m| m.name == name) {
        let names: Vec<&str> = real.iter().map(|m| m.name.as_str()).collect();
        return Err(format!("there is no monitor {name}; there are {}", names.join(", ")));
    }
    if let Some((x, y)) = wish.at {
        let boxes: Vec<(&str, [i32; 4])> = real
            .iter()
            .map(|m| {
                let (w, h) = ((m.size.0 as f64 / m.scale).round() as i32, (m.size.1 as f64 / m.scale).round() as i32);
                (m.name.as_str(), if m.name == name { [x, y, w, h] } else { [m.x, m.y, w, h] })
            })
            .collect();
        if let Some((a, b)) = crate::output::overlapping(&boxes) {
            return Err(format!("{a} and {b} would cover the same part of the desktop"));
        }
    }
    let mut done = Vec::new();
    if let Some(on) = wish.vrr {
        // No card, no connector to say it can.
        crate::output::vrr_change(name, None, None, on)?;
        done.push(format!("{name}: variable refresh off"));
    }
    if let Some(at) = wish.at {
        // Gone between the check above and here: nothing is kept for it.
        if !crate::layers::place(name, at) {
            return Err(format!("{name} is gone"));
        }
        let m = name.strip_prefix("HEADLESS-").and_then(|n| n.parse::<usize>().ok()).map(|n| n - 1);
        if let Some(m) = m {
            let mut asked = ASKED_AT.lock().unwrap();
            asked.retain(|(k, _)| *k != m);
            asked.push((m, at));
        }
        crate::layers::tell(crate::layers::ToLayers::Monitors);
        done.push(format!("{name} at {},{}", at.0, at.1));
    }
    Ok(done.join("; "))
}

pub struct Headless;

/// A monitor with no screen: three textures, "flipped" at its refresh, and a picture.
struct Offscreen {
    size: (u32, u32),
    textures: Vec<wgpu::Texture>,
    shown: Option<usize>,
    started: Instant,
    at: f32,
    written: bool,
    taken: usize,
    path: String,
    /// Its monitor, once made: what it tells when a flip "lands".
    screen: Arc<std::sync::OnceLock<Screen>>,
    to_render: Sender<ToRender>,
    /// How long a refresh lasts on it.
    period: Duration,
}

impl Output for Offscreen {
    fn buffer(&mut self, device: &wgpu::Device, _: &[u64]) -> Option<(usize, wgpu::Texture)> {
        if self.textures.is_empty() {
            for _ in 0..3 {
                self.textures.push(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("headless monitor"),
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

    fn show(&mut self, which: usize, done: pleamar::Sent, device: &wgpu::Device, queue: &wgpu::Queue, _: bool) -> bool {
        done.wait(device, Duration::from_millis(200));
        self.shown = Some(which);
        if std::env::var_os("PLEAMAR_DEBUG_SCREEN").is_some() {
            eprintln!("headless · {} shown {which} at {:.3} s", self.path, self.started.elapsed().as_secs_f32());
        }
        // `PLEAMAR_HEADLESS_FRAMES=n`: n frames in a row from then on, -f0, -f1…
        let frames = std::env::var("PLEAMAR_HEADLESS_FRAMES").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
        if self.taken < frames && self.started.elapsed().as_secs_f32() >= self.at {
            let path = if frames > 1 { self.path.replace(".png", &format!("-f{}.png", self.taken)) } else { self.path.clone() };
            self.taken += 1;
            self.written = true;
            match write_png(device, queue, &self.textures[which], self.size, &path) {
                Ok(()) => println!("headless · the frame shown went to {path}"),
                Err(e) => eprintln!("headless · {e}"),
            }
        }
        // The flip "lands" on the monitor's next refresh: on its beat, as a
        // real one does, not a refresh after it was asked for.
        if let Some(sc) = self.screen.get().cloned() {
            let (tx, period) = (self.to_render.clone(), self.period);
            let since = self.started.elapsed().as_nanos();
            let wait = Duration::from_nanos((period.as_nanos() - since % period.as_nanos().max(1)) as u64);
            std::thread::spawn(move || {
                std::thread::sleep(wait);
                screen::landed(&sc, &tx);
            });
            return true;
        }
        false
    }
}

fn write_png(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, (w, h): (u32, u32), path: &str) -> Result<(), String> {
    let row = (w * 4).div_ceil(256) * 256;
    let out = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));
    let sent = pleamar::Sent::after(queue);
    let mapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = mapped.clone();
    out.slice(..).map_async(wgpu::MapMode::Read, move |_| flag.store(true, std::sync::atomic::Ordering::Release));
    sent.wait(device, Duration::from_secs(2));
    let start = std::time::Instant::now();
    while !mapped.load(std::sync::atomic::Ordering::Acquire) && start.elapsed() < Duration::from_secs(2) {
        let _ = device.poll(wgpu::PollType::Poll);
        std::thread::sleep(Duration::from_micros(200));
    }
    let data = out.slice(..).get_mapped_range().map_err(|e| format!("{e:?}"))?;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for px in data[(y * row) as usize..(y * row + w * 4) as usize].chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    image::save_buffer(path, &rgba, w, h, image::ColorType::Rgba8).map_err(|e| e.to_string())
}

impl pleamar::Platform for Headless {
    fn run(self: Box<Self>, surfaces: Vec<Surface>, _: u32, _: wgpu::Instance, to_render: Sender<ToRender>) {
        let real = (1920u32, 1080u32);
        // `PLEAMAR_HEADLESS_TRANSFORM=1`: monitors standing on their side, as
        // `transform` makes them in a session; the picture is the real
        // buffer, turned, as the monitor would get it.
        let turn = std::env::var("PLEAMAR_HEADLESS_TRANSFORM").ok().and_then(|v| v.parse::<u8>().ok()).unwrap_or(0) % 4;
        let size = if turn % 2 == 1 { (real.1, real.0) } else { real };
        // `PLEAMAR_HEADLESS_SCALE=1.5`: monitors of that scale, to check HiDPI.
        let scale = std::env::var("PLEAMAR_HEADLESS_SCALE").ok().and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0).unwrap_or(1.0);
        let unit_w = (size.0 as f64 / scale).round() as i32;
        let count = std::env::var("PLEAMAR_HEADLESS_SCREENS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
        let png = std::env::var("PLEAMAR_HEADLESS_PNG").unwrap_or_else(|_| "/tmp/pleamar-headless.png".into());
        let at = std::env::var("PLEAMAR_HEADLESS_AT").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(8.0);
        // `PLEAMAR_HEADLESS_HZ=165,60`: each monitor's refresh (60 by default),
        // to see a fast one beside a slow one, as on a real desk.
        let hz: std::sync::Arc<Vec<f64>> = std::env::var("PLEAMAR_HEADLESS_HZ").unwrap_or_default().split(',').filter_map(|v| v.trim().parse::<f64>().ok()).filter(|v| *v > 1.0).collect::<Vec<f64>>().into();
        let mhz_of = move |m: usize| (hz.get(m).copied().unwrap_or(60.0) * 1000.0).round() as i32;
        let screens: Vec<Screen> = (0..count)
            .map(|m| {
                let path = if m == 0 { png.clone() } else { png.replace(".png", &format!("-{m}.png")) };
                let own = Arc::new(std::sync::OnceLock::new());
                let output = Offscreen { size: real, textures: Vec::new(), shown: None, started: Instant::now(), at, written: false, taken: 0, path, screen: own.clone(), to_render: to_render.clone(), period: Duration::from_secs_f64(1000.0 / mhz_of(m) as f64) };
                let output: Box<dyn Output> = if turn == 0 { Box::new(output) } else { Box::new(screen::Turned::new(Box::new(output), turn, real)) };
                let sc = screen::screen(format!("HEADLESS-{}", m + 1), size, output);
                let _ = own.set(sc.clone());
                sc
            })
            .collect();
        crate::layers::register(screens.iter().enumerate().map(|(m, sc)| (crate::layers::MonitorInfo { name: format!("HEADLESS-{}", m + 1), size, x: placed(m, unit_w).0, y: placed(m, unit_w).1, mhz: mhz_of(m), scale }, sc.clone())).collect());
        crate::output::set_sink(Box::new(|(name, wish, answer)| answer.send(apply_output(&name, &wish)).is_ok()));
        let cursor = Arc::new(Mutex::new(Cursor::Normal));
        let mut id = 7000;
        // Which sheets are on each monitor: to take one away, as if unplugged.
        let mut sheets_on: Vec<Vec<u32>> = vec![Vec::new(); count];
        for (k, s) in surfaces.iter().enumerate() {
            let on: Vec<usize> = match &s.screens {
                Screens::Number(n) => vec![*n],
                Screens::All if !s.name.is_empty() => (0..count).collect(),
                _ => vec![0],
            };
            for which in on {
                let Some(sc) = screens.get(which) else { continue };
                if s.name.is_empty() && sc.0.lock().unwrap().layers.iter().any(|l| l.main) {
                    continue;
                }
                id += 1;
                let layer = screen::layer(id, k, s, size, scale as f32);
                let lsize = (layer.rect[2] as u32, layer.rect[3] as u32);
                let units = layer.units;
                println!("headless · monitor {which}: surface {k} '{}' {}×{} at {},{}", s.name, lsize.0, lsize.1, layer.rect[0], layer.rect[1]);
                sc.0.lock().unwrap().layers.push(layer);
                sheets_on[which].push(id);
                let _ = to_render.send(ToRender::Sheet(Box::new(NewSheet {
                    id,
                    target: Target::Frames(Box::new(LayerFrames::new(sc.clone(), id, lsize, to_render.clone()))),
                    window: Box::new(LayerWindow { screen: sc.clone(), sheet: id, cursor: cursor.clone() }),
                    scale: scale as f32,
                    size: units,
                    mhz: mhz_of(which),
                    name: format!("HEADLESS-{}", which + 1),
                    view: View { surface: k, popup: None, origin: s.origin, size: (units.0 as f32, units.1 as f32) },
                })));
            }
        }
        // The phone's monitor, put up and taken down as `pleamar-wm remote`
        // asks (`P W H SCALE` on the agent socket), as in a session; or by
        // itself: `PLEAMAR_HEADLESS_PHONE="1080x2400@2.5 3 20"`, that size and
        // scale 3 s in, and down again 20 s in. Its picture is taken as any
        // monitor's (`grim -o PHONE-1`).
        let phone: Arc<Mutex<Option<(Screen, Vec<u32>, i32, f64)>>> = Arc::new(Mutex::new(None));
        {
            let (ptx, prx) = std::sync::mpsc::channel::<Option<crate::layers::PhoneWish>>();
            let auto = ptx.clone();
            crate::layers::set_phone_sink(Box::new(move |w| ptx.send(w).is_ok()));
            if let Ok(spec) = std::env::var("PLEAMAR_HEADLESS_PHONE") {
                let mut words = spec.split_whitespace();
                let wish = words.next().and_then(|w| {
                    let (size, scale) = w.split_once('@').unwrap_or((w, "2"));
                    let (pw, ph) = size.split_once('x')?;
                    Some(crate::layers::PhoneWish { size: (pw.parse().ok()?, ph.parse().ok()?), scale: scale.parse().ok()? })
                });
                let on = words.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(3.0);
                let off = words.next().and_then(|v| v.parse::<f32>().ok());
                if let Some(wish) = wish {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs_f32(on));
                        let _ = auto.send(Some(wish));
                        if let Some(off) = off {
                            std::thread::sleep(Duration::from_secs_f32((off - on).max(0.0)));
                            let _ = auto.send(None);
                        }
                    });
                }
            }
            let (tx, screens, phone, surfaces) = (to_render.clone(), screens.clone(), phone.clone(), surfaces.clone());
            let (cursor, mhz_of) = (cursor.clone(), mhz_of.clone());
            std::thread::spawn(move || {
                let mut id = 9000;
                let real = |screens: &[Screen]| screens.iter().enumerate().map(|(m, sc)| (crate::layers::MonitorInfo { name: format!("HEADLESS-{}", m + 1), size, x: placed(m, unit_w).0, y: placed(m, unit_w).1, mhz: mhz_of(m), scale }, sc.clone())).collect::<Vec<_>>();
                for wish in prx {
                    if let Some((sc, sheets, ..)) = phone.lock().unwrap().take() {
                        println!("headless · the phone's monitor goes");
                        for id in sheets {
                            let _ = tx.send(ToRender::SheetGone(id));
                        }
                        sc.0.lock().unwrap().quit = true;
                        sc.1.notify_all();
                    }
                    let mut all = real(&screens);
                    if let Some(w) = wish {
                        let sc = crate::phone::make(w.size, &tx);
                        let k = screens.len();
                        println!("headless · a monitor for the phone: {}×{} at scale {}", w.size.0, w.size.1, w.scale);
                        let mut given = Vec::new();
                        for (n, s) in surfaces.iter().enumerate() {
                            let mine = match &s.screens {
                                Screens::All => !s.name.is_empty(),
                                // `screens: each`: its copy for this monitor.
                                Screens::Number(m) => *m == k,
                                _ => false,
                            };
                            if !mine {
                                continue;
                            }
                            id += 1;
                            let layer = screen::layer(id, n, s, w.size, w.scale as f32);
                            let lsize = (layer.rect[2] as u32, layer.rect[3] as u32);
                            let units = layer.units;
                            sc.0.lock().unwrap().layers.push(layer);
                            given.push(id);
                            let _ = tx.send(ToRender::Sheet(Box::new(NewSheet {
                                id,
                                target: Target::Frames(Box::new(LayerFrames::new(sc.clone(), id, lsize, tx.clone()))),
                                window: Box::new(LayerWindow { screen: sc.clone(), sheet: id, cursor: cursor.clone() }),
                                scale: w.scale as f32,
                                size: units,
                                mhz: crate::phone::PHONE_MHZ,
                                name: crate::layers::PHONE_NAME.to_owned(),
                                view: View { surface: n, popup: None, origin: s.origin, size: (units.0 as f32, units.1 as f32) },
                            })));
                        }
                        let right = screens.len() as i32 * unit_w;
                        let (px, py) = crate::phone::place(right);
                        all.push((crate::layers::MonitorInfo { name: crate::layers::PHONE_NAME.to_owned(), size: w.size, x: px, y: py, mhz: crate::phone::PHONE_MHZ, scale: w.scale }, sc.clone()));
                        *phone.lock().unwrap() = Some((sc, given, px, w.scale));
                    }
                    let k = if wish.is_some() { screens.len() as f32 } else { -1.0 };
                    crate::layers::register(all);
                    crate::layers::tell(crate::layers::ToLayers::Monitors);
                    let _ = tx.send(ToRender::Fact(pleamar::scene::intern("phone"), k));
                    let _ = tx.send(ToRender::Repaint);
                }
            });
        }
        // `PLEAMAR_HEADLESS_SLEEP=s`: every monitor goes after that long, as a
        // session sees them when they go dark and their connectors drop, and
        // all come back `PLEAMAR_HEADLESS_REPLUG` seconds later (3 if not
        // said). They go on drawing: it is the compositor inside, and its
        // programs, that see them leave and come back.
        if let Some(after) = std::env::var("PLEAMAR_HEADLESS_SLEEP").ok().and_then(|v| v.parse::<f32>().ok()) {
            let screens = screens.clone();
            let mhz: Vec<i32> = (0..screens.len()).map(&mhz_of).collect();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs_f32(after));
                // One after another, as a real session's connectors drop.
                for left in (0..screens.len()).rev() {
                    crate::layers::register(screens[screens.len() - left..].iter().enumerate().map(|(k, sc)| {
                        let m = screens.len() - left + k;
                        (crate::layers::MonitorInfo { name: format!("HEADLESS-{}", m + 1), size, x: placed(m, unit_w).0, y: placed(m, unit_w).1, mhz: mhz[m], scale }, sc.clone())
                    }).collect());
                    crate::layers::tell(crate::layers::ToLayers::Monitors);
                    std::thread::sleep(Duration::from_millis(300));
                }
                println!("headless · every monitor gone");
                let back = std::env::var("PLEAMAR_HEADLESS_REPLUG").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(3.0);
                std::thread::sleep(Duration::from_secs_f32(back));
                println!("headless · every monitor back");
                crate::layers::register(screens.iter().enumerate().map(|(m, sc)| (crate::layers::MonitorInfo { name: format!("HEADLESS-{}", m + 1), size, x: placed(m, unit_w).0, y: placed(m, unit_w).1, mhz: mhz[m], scale }, sc.clone())).collect());
                crate::layers::tell(crate::layers::ToLayers::Monitors);
            });
        }
        // `PLEAMAR_HEADLESS_UNPLUG=s`: the last monitor goes away after that
        // long, as a session sees one unplugged: its surfaces are taken from the
        // render, and the compositor inside is told.
        if let Some(after) = std::env::var("PLEAMAR_HEADLESS_UNPLUG").ok().and_then(|v| v.parse::<f32>().ok()).filter(|_| count > 1) {
            let (tx, screens, sheets_on) = (to_render.clone(), screens.clone(), sheets_on.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs_f32(after));
                let last = screens.len() - 1;
                println!("headless · monitor HEADLESS-{} unplugged", last + 1);
                for id in &sheets_on[last] {
                    let _ = tx.send(ToRender::SheetGone(*id));
                }
                {
                    let (lock, cv) = &*screens[last];
                    lock.lock().unwrap().quit = true;
                    cv.notify_all();
                }
                crate::layers::register(screens[..last].iter().enumerate().map(|(m, sc)| (crate::layers::MonitorInfo { name: format!("HEADLESS-{}", m + 1), size, x: placed(m, unit_w).0, y: placed(m, unit_w).1, mhz: mhz_of(m), scale }, sc.clone())).collect());
                crate::layers::tell(crate::layers::ToLayers::Monitors);
                // `PLEAMAR_HEADLESS_REPLUG=s`: and back that long after, for the
                // compositor inside (its picture is not drawn again: this is to
                // see where the windows go).
                if let Some(back) = std::env::var("PLEAMAR_HEADLESS_REPLUG").ok().and_then(|v| v.parse::<f32>().ok()) {
                    std::thread::sleep(Duration::from_secs_f32(back));
                    println!("headless · monitor HEADLESS-{} plugged back", last + 1);
                    crate::layers::register(screens.iter().enumerate().map(|(m, sc)| (crate::layers::MonitorInfo { name: format!("HEADLESS-{}", m + 1), size, x: placed(m, unit_w).0, y: placed(m, unit_w).1, mhz: mhz_of(m), scale }, sc.clone())).collect());
                    crate::layers::tell(crate::layers::ToLayers::Monitors);
                }
            });
        }
        // `PLEAMAR_HEADLESS_INPUT="900,40@3000 down@3500 up@3600 key:Escape:1@4000"`:
        // a mouse and a keyboard, through the same road as the session's
        // (`route.rs`): to the scene or to a program's surface —a bar, Marea—
        // as the real ones would go. Points in units of the whole desktop; a
        // key by its keysym's name and evdev code (it types itself if the name
        // is one character). `PLEAMAR_HEADLESS_INPUT_FIFO=path`: the same
        // steps, without the `@`, one a line, as they come down that pipe
        // (what `pleamar-wm remote` sends with `PLEAMAR_REMOTE_HANDS_TO`, to
        // try it without real devices).
        let script = std::env::var("PLEAMAR_HEADLESS_INPUT").ok();
        let fifo = std::env::var("PLEAMAR_HEADLESS_INPUT_FIFO").ok();
        if script.is_some() || fifo.is_some() {
            let route = Arc::new(Mutex::new(crate::route::Route::new(to_render.clone())));
            // A pointer to be seen where one is drawn into a picture (sharing
            // the screen), as the cursor theme draws it.
            let look = Look::start(route.clone(), scale);
            // A phone's fingers, as the session's own would take them.
            let theirs = route.clone();
            crate::layers::set_fingers_sink(Box::new(move |f| {
                theirs.lock().unwrap().fingers(f);
                true
            }));
            let hand = Hand { route, screens: screens.clone(), phone: phone.clone(), unit_w, scale, look };
            if let Some(script) = script {
                let hand = hand.clone();
                std::thread::spawn(move || {
                    let start = Instant::now();
                    for step in script.split_whitespace() {
                        let Some((what, ms)) = step.rsplit_once('@') else { continue };
                        let Ok(ms) = ms.parse::<u64>() else { continue };
                        if let Some(wait) = Duration::from_millis(ms).checked_sub(start.elapsed()) {
                            std::thread::sleep(wait);
                        }
                        hand.step(what);
                    }
                });
            }
            if let Some(path) = fifo {
                let _ = std::fs::remove_file(&path);
                let made = std::ffi::CString::new(path.clone()).map(|c| unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0).unwrap_or(false);
                if !made {
                    eprintln!("headless · no pipe at {path}");
                }
                std::thread::spawn(move || loop {
                    let Ok(f) = std::fs::File::open(&path) else { return };
                    for line in std::io::BufRead::lines(std::io::BufReader::new(f)).map_while(Result::ok) {
                        for what in line.split_whitespace() {
                            hand.step(what);
                        }
                    }
                });
            }
        }
        // At the hour of the picture everything is painted, so that a monitor
        // where nothing moves shows its frame all the same.
        let tx = to_render.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f32(at + 0.05));
            let _ = tx.send(ToRender::Repaint);
        });
        let _ = to_render.send(ToRender::KeyboardFocus(true));
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}


/// A headless mouse and keyboard (`PLEAMAR_HEADLESS_INPUT`, `_FIFO`): each
/// step goes where the session's real ones would, the phone's monitor too.
#[derive(Clone)]
struct Hand {
    route: Arc<Mutex<crate::route::Route>>,
    screens: Vec<Screen>,
    phone: Arc<Mutex<Option<(Screen, Vec<u32>, i32, f64)>>>,
    unit_w: i32,
    scale: f64,
    /// Told when what is under the pointer changes, for its shape.
    look: channel::Sender<()>,
}

impl Hand {
    fn step(&self, what: &str) {
        let phone = self.phone.lock().unwrap().clone();
        let mut screens = self.screens.clone();
        if let Some((sc, ..)) = &phone {
            screens.push(sc.clone());
        }
        let mut route = self.route.lock().unwrap();
        match what {
            "down" | "up" => {
                route.button(&screens, 0x110, what == "down");
            }
            // The right button.
            "rdown" | "rup" => {
                route.button(&screens, 0x111, what == "rdown");
            }
            // The mouse's side buttons: back and forward.
            "back" | "forward" => {
                let code = if what == "back" { 0x113 } else { 0x114 };
                route.button(&screens, code, true);
                route.button(&screens, code, false);
            }
            // `wheelx:-1` a notch to the right (the scene's only, as at the session).
            _ if what.starts_with("wheelx:") => {
                if let Ok(n) = what[7..].parse::<f32>() {
                    route.wheel_sideways(n);
                }
            }
            // `scroll:0,-40` fingers moving what is under the pointer 40 units
            // up (as a phone's does); `scrollend`, lifted.
            "scrollend" => route.fingers(crate::layers::Fingers::Up),
            _ if what.starts_with("scroll:") => {
                if let Some((Ok(x), Ok(y))) = what[7..].split_once(',').map(|(x, y)| (x.parse::<f64>(), y.parse::<f64>())) {
                    route.fingers(crate::layers::Fingers::Moved(x, y));
                }
            }
            // `wheel:-1` a notch down (as the session tells it: up is positive).
            _ if what.starts_with("wheel:") => {
                if let Ok(n) = what[6..].parse::<f32>() {
                    route.wheel(n);
                }
            }
            // `keydown:` and `keyup:` apart: a key let go after
            // whoever took it has gone (Marea's search closing on Escape).
            _ if what.starts_with("key:") || what.starts_with("keydown:") || what.starts_with("keyup:") => {
                let (step, rest) = what.split_once(':').unwrap_or(("key", ""));
                let mut parts = rest.splitn(2, ':');
                let name = parts.next().unwrap_or("");
                let Some(code) = parts.next().and_then(|c| c.parse::<u32>().ok()) else { return };
                // `key:Super+Shift+q:16`: modifiers before the name.
                let (mods_part, name) = name.rsplit_once('+').unwrap_or(("", name));
                let mut mods = pleamar::scene::Mods::default();
                for m in mods_part.split('+') {
                    match m.to_lowercase().as_str() {
                        "super" => mods.logo = true,
                        "ctrl" => mods.ctrl = true,
                        "alt" => mods.alt = true,
                        "shift" => mods.shift = true,
                        _ => {}
                    }
                }
                let typed = Some(name.to_owned()).filter(|n| n.chars().count() == 1 && !mods.logo && !mods.ctrl && !mods.alt);
                println!("headless · key {name} → {:?}", route.key_owner(&screens));
                if std::env::var_os("PLEAMAR_DEBUG_WINDOWS").is_some() {
                    for (m, s) in screens.iter().enumerate() {
                        for c in &s.0.lock().unwrap().clients {
                            println!("headless ·   monitor {m}: surface {} level {} keyboard {} pieces {}", c.id, c.level, c.keyboard, c.pieces.len());
                        }
                    }
                }
                if step != "keyup" {
                    route.key(&screens, name, None, typed, mods, code, true);
                }
                if step != "keydown" {
                    route.key(&screens, name, None, None, mods, code, false);
                }
            }
            _ => {
                let Some((x, y)) = what.split_once(',').and_then(|(x, y)| Some((x.parse::<f64>().ok()?, y.parse::<f64>().ok()?))) else { return };
                // On the phone's monitor, far to the right of the others.
                let crossed = if let Some((sc, _, px, pscale)) = phone.as_ref().filter(|p| x >= p.2 as f64) {
                    route.pointer(sc, ((x - *px as f64) * pscale, y * pscale))
                } else {
                    let m = ((x / self.unit_w as f64).floor().max(0.0) as usize).min(self.screens.len() - 1);
                    let (mx, my) = ((x - (m as i32 * self.unit_w) as f64) * self.scale, y * self.scale);
                    route.pointer(&self.screens[m], (mx, my))
                };
                if crossed {
                    let _ = self.look.send(());
                }
                crate::layers::set_pointer_at((x, y));
                println!("headless · pointer {x},{y} → {:?}", route.hit);
            }
        }
    }
}

/// One frame of a picture of the theme, as pictures of the screen take it,
/// and how long it shows (ms).
type LookFrame = (Arc<(Vec<u8>, (i32, i32))>, u32);

/// The pointer as pictures of the screen show it, the only place a screenless
/// session has one: the theme's shape for what the scene or the program under
/// it asks (as the route says), frame by frame and through the theme's
/// transitions, as the session's cursor plane plays it; none while a program
/// hides it, and a program's own while it draws one. On a thread of its own,
/// with a loop for the frames' timers.
struct Look {
    theme: Theme,
    size: u32,
    /// Each picture read so far, by name: its frames.
    pictures: Vec<(String, Vec<LookFrame>)>,
    route: Arc<Mutex<crate::route::Route>>,
    scene: Cursor,
    program: Cursor,
    hidden: bool,
    custom: Option<Arc<(Vec<u8>, (i32, i32))>>,
    /// The shape asked for, what plays on the way to it, and what was asked
    /// for while a transition played.
    shown: Option<Cursor>,
    playing: Option<Playing>,
    queued: Option<Cursor>,
    frame: usize,
    timer: Option<RegistrationToken>,
    handle: LoopHandle<'static, Look>,
}

impl Look {
    /// Starts it, listening for the cursors asked for: what is given back is
    /// to be told when what is under the pointer changes.
    fn start(route: Arc<Mutex<crate::route::Route>>, scale: f64) -> channel::Sender<()> {
        let (crossed_tx, crossed_rx) = channel::channel::<()>();
        let (cursor_tx, cursor_rx) = channel::channel::<(bool, Cursor)>();
        let (hidden_tx, hidden_rx) = channel::channel::<bool>();
        let (custom_tx, custom_rx) = channel::channel::<Option<crate::layers::CursorPicture>>();
        crate::layers::set_cursor_sink(cursor_tx);
        crate::layers::set_cursor_hidden_sink(hidden_tx);
        crate::layers::set_custom_cursor_sink(custom_tx);
        let spawned = std::thread::Builder::new().name("headless·cursor".into()).spawn(move || {
            let Ok(mut lp) = EventLoop::<Look>::try_new() else { return };
            let handle = lp.handle();
            let mut look = Look { theme: Theme::load(), size: cursor_theme::size_at(scale), pictures: Vec::new(), route, scene: Cursor::Normal, program: Cursor::Normal, hidden: false, custom: None, shown: None, playing: None, queued: None, frame: 0, timer: None, handle: handle.clone() };
            let _ = handle.insert_source(cursor_rx, |e, _, look: &mut Look| {
                if let channel::Event::Msg((from_program, c)) = e {
                    if from_program { look.program = c } else { look.scene = c }
                    look.show();
                }
            });
            let _ = handle.insert_source(hidden_rx, |e, _, look: &mut Look| {
                if let channel::Event::Msg(hidden) = e {
                    look.hidden = hidden;
                    look.shown = None;
                    look.show();
                }
            });
            let _ = handle.insert_source(custom_rx, |e, _, look: &mut Look| {
                if let channel::Event::Msg(picture) = e {
                    look.custom = picture.and_then(|p| cursor_theme::program_picture(&p)).map(Arc::new);
                    look.shown = None;
                    look.show();
                }
            });
            let _ = handle.insert_source(crossed_rx, |e, _, look: &mut Look| {
                if let channel::Event::Msg(()) = e {
                    look.show();
                }
            });
            for c in cursor_theme::KINDS {
                look.picture(c.name());
            }
            if !look.picture("default") {
                look.theme.has("default");
                look.pictures.push(("default".into(), vec![(Arc::new((cursor_theme::arrow(), (0, 0))), 0)]));
            }
            println!("headless · cursor: {} at {} px", look.theme.name.as_deref().unwrap_or("its own arrow"), look.size);
            look.show();
            let _ = lp.run(None, &mut look, |_| {});
        });
        if let Err(e) = spawned {
            eprintln!("headless · no cursor: {e}");
        }
        crossed_tx
    }

    /// A picture of the theme by its name, read once: whether it is there.
    fn picture(&mut self, name: &str) -> bool {
        if self.pictures.iter().any(|p| p.0 == name) {
            return true;
        }
        let Some((_, frames)) = self.theme.frames(name, &[self.size]).and_then(|mut each| each.pop()).filter(|(_, f)| !f.is_empty()) else { return false };
        self.pictures.push((name.to_owned(), frames.into_iter().map(|(px, hot, delay)| (Arc::new((px, hot)), delay)).collect()));
        true
    }

    fn frames(&self) -> Option<&[LookFrame]> {
        let p = self.playing.as_ref()?;
        self.pictures.iter().find(|c| c.0 == p.asset).map(|c| c.1.as_slice())
    }

    /// The cursor of whoever has the pointer, if it is not the one shown.
    fn show(&mut self) {
        if let Some(picture) = &self.custom {
            crate::layers::set_pointer_picture((!self.hidden).then(|| picture.clone()));
            self.stop();
            self.playing = None;
            self.queued = None;
            return;
        }
        let want = if matches!(self.route.lock().unwrap().hit, Hit::Client(..)) { self.program } else { self.scene };
        // A shape the theme lacks is shown as the arrow.
        let want = if self.picture(want.name()) { want } else { Cursor::Normal };
        if !self.picture(want.name()) || self.shown == Some(want) {
            self.queued = None;
            return;
        }
        // A transition is let finish; the last shape asked for meanwhile comes after.
        if self.timer.is_some() && self.playing.as_ref().is_some_and(|p| p.transition) {
            self.queued = Some(want);
            return;
        }
        self.change_to(want);
    }

    /// From the shape shown to another: through the theme's transition
    /// between the two if it has one, then the shape itself.
    fn change_to(&mut self, want: Cursor) {
        let from = self.shown.replace(want);
        self.queued = None;
        self.playing = Some(Playing { asset: want.name().to_owned(), reverse: false, transition: false });
        if let Some(from) = from.filter(|f| *f != want) {
            if let Some((asset, reverse)) = cursor_theme::transition_between(from.name(), want.name(), |n| self.picture(n)) {
                self.playing = Some(Playing { asset, reverse, transition: true });
            }
        }
        self.frame = 0;
        self.stop();
        self.show_frame();
        self.next_frame_later();
    }

    fn show_frame(&self) {
        let (Some(p), Some(frames)) = (&self.playing, self.frames()) else { return };
        let Some(f) = frames.get(p.index(self.frame, frames.len())) else { return };
        crate::layers::set_pointer_picture((!self.hidden).then(|| f.0.clone()));
    }

    fn stop(&mut self) {
        if let Some(t) = self.timer.take() {
            self.handle.remove(t);
        }
    }

    /// The next frame when this one's time is up: round and round for a shape
    /// of several, once through for a transition. None while it is hidden.
    fn next_frame_later(&mut self) {
        if self.hidden {
            return;
        }
        let (Some(p), Some(frames)) = (&self.playing, self.frames()) else { return };
        let len = frames.len();
        if len < 2 && !p.transition {
            return;
        }
        let delay = frames.get(p.index(self.frame, len)).map_or(1, |f| f.1.max(1));
        self.timer = self
            .handle
            .insert_source(Timer::from_duration(Duration::from_millis(delay as u64)), move |_, _, look: &mut Look| {
                look.timer = None;
                look.frame += 1;
                if look.playing.as_ref().is_some_and(|p| p.transition) && look.frame >= len {
                    // Played through: the shape it led to, or the one asked for since.
                    let next = look.queued.take().or(look.shown);
                    match (next, look.shown) {
                        (Some(q), Some(s)) if q != s => look.change_to(q),
                        (Some(n), _) => {
                            look.shown = None;
                            look.playing = None;
                            look.change_to(n);
                        }
                        (None, _) => {
                            look.playing = None;
                            look.show();
                        }
                    }
                    return TimeoutAction::Drop;
                }
                look.show_frame();
                look.next_frame_later();
                TimeoutAction::Drop
            })
            .ok();
    }
}
