//! `pleamar-wm-stream OUTPUT FPS KBPS`: a monitor as H.264, for `pleamar-wm
//! remote`, with as little time as possible between a change on it and its
//! frame out.
//!
//! - The picture: wlr-screencopy, into two buffers of shared memory taken in
//!   turns (one is copied into while the other is encoded). A frame is asked
//!   for as soon as the last one is out, and comes when the monitor changes;
//!   a monitor that does not change costs nothing.
//! - The encoder: ffmpeg's, in this process: the card's (NVENC), which takes
//!   the pixels as they are (BGRX) and converts them itself, or x264 on the
//!   processor. Asked for no frame held back (no B frames, no look-ahead).
//! - What it is told on its input, a line at a time, while it runs: `rate
//!   KBPS FPS` (a new bitrate, without starting again), `key` (a whole frame
//!   next: the page lost something).
//! - What it writes: each frame as it comes out, as `[length: u32 BE]
//!   [flags: u8, 1 = whole]` and the frame (H.264, Annex B).
//!
//! When a burst of changes ends, the last picture is encoded again a couple
//! of times: in a burst the bitrate runs short and the picture gets soft, and
//! a still screen should be sharp.

use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ffmpeg_sys_next as ff;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: pleamar-wm-stream OUTPUT FPS KBPS");
        std::process::exit(2);
    }
    let fps: u32 = args[2].parse().unwrap_or(60).clamp(1, 240);
    let kbps: u32 = args[3].parse().unwrap_or(8000).clamp(100, 200_000);
    if let Err(e) = run(&args[1], fps, kbps) {
        eprintln!("pleamar-wm-stream: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- Wayland

#[derive(Default)]
struct State {
    outputs: Vec<(wl_output::WlOutput, String)>,
    /// Each copy asked for, by its buffer's number: what it says its buffer
    /// must be, and whether it is in.
    copies: [Copy; BUFFERS],
    /// The monitor's global, and whether it went (unplugged; a phone's turned
    /// is put up again in its new shape): nothing more is asked of it.
    target_global: u32,
    gone: bool,
}

#[derive(Default, Clone, Copy)]
struct Copy {
    shape: Option<(wl_shm::Format, u32, u32, u32)>,
    shape_done: bool,
    ready: bool,
    failed: bool,
}

/// Two copies asked for at a time (so that the monitor's next time is never
/// missed while one is on its way), and the last picture kept for sharpening.
const BUFFERS: usize = 3;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, _: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_registry::Event::GlobalRemove { name } = event {
            if name == state.target_global {
                state.gone = true;
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, k: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event {
            if let Some(o) = state.outputs.get_mut(*k) {
                o.1 = name;
            }
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(_: &mut Self, _: &wl_shm::WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for State {
    fn event(_: &mut Self, _: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _: zwlr_screencopy_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, usize> for State {
    fn event(state: &mut Self, _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, event: zwlr_screencopy_frame_v1::Event, k: &usize, _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_screencopy_frame_v1::Event;
        let c = &mut state.copies[*k];
        match event {
            Event::Buffer { format: WEnum::Value(f), width, height, stride } if matches!(f, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888) => {
                c.shape = Some((f, width, height, stride));
            }
            Event::BufferDone => c.shape_done = true,
            Event::Ready { .. } => c.ready = true,
            Event::Failed => c.failed = true,
            _ => {}
        }
    }
}

/// Events dispatched until `done` holds, or the time is up (false). The
/// monitor gone ends it all: whoever started this starts it again for the
/// monitor there is now.
fn wait(queue: &mut EventQueue<State>, state: &mut State, most: Duration, done: impl Fn(&State) -> bool) -> Result<bool, String> {
    let end = Instant::now() + most;
    loop {
        queue.dispatch_pending(state).map_err(|e| e.to_string())?;
        if state.gone {
            return Err("the monitor went".into());
        }
        if done(state) {
            return Ok(true);
        }
        queue.flush().map_err(|e| e.to_string())?;
        let Some(guard) = queue.prepare_read() else { continue };
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        let mut poll = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let n = unsafe { libc::poll(&mut poll, 1, left.as_millis().max(1) as i32) };
        if n > 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// A buffer of shared memory the compositor copies the monitor into.
struct Shm {
    buffer: wl_buffer::WlBuffer,
    data: *mut u8,
    len: usize,
    _fd: OwnedFd,
}

impl Shm {
    fn new(shm: &wl_shm::WlShm, qh: &QueueHandle<State>, (format, w, h, stride): (wl_shm::Format, u32, u32, u32)) -> Result<Shm, String> {
        let len = (stride * h) as usize;
        let fd = unsafe { libc::memfd_create(c"pleamar-wm-stream".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(format!("memfd: {}", std::io::Error::last_os_error()));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as i64) } < 0 {
            return Err(format!("memfd: {}", std::io::Error::last_os_error()));
        }
        let data = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if data == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, ());
        pool.destroy();
        Ok(Shm { buffer, data: data as *mut u8, len, _fd: fd })
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        self.buffer.destroy();
        unsafe { libc::munmap(self.data as *mut libc::c_void, self.len) };
    }
}

// ---------------------------------------------------------------- the encoder

struct Encoder {
    ctx: *mut ff::AVCodecContext,
    frame: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    /// x264 takes YUV: the pixels converted into `yuv` first.
    sws: *mut ff::SwsContext,
    yuv: *mut ff::AVFrame,
    name: &'static str,
    started: Instant,
    last_pts: i64,
}

/// The encoders tried, in order, and what makes each answer at once.
const ENCODERS: &[(&str, &[(&str, &str)])] = &[
    ("h264_nvenc", &[("preset", "p1"), ("tune", "ull"), ("zerolatency", "1"), ("delay", "0"), ("rc", "vbr"), ("rc-lookahead", "0"), ("forced-idr", "1"), ("b_ref_mode", "disabled")]),
    ("libx264", &[("preset", "ultrafast"), ("tune", "zerolatency"), ("forced-idr", "1")]),
];

impl Encoder {
    fn open(w: u32, h: u32, fps: u32, kbps: u32) -> Result<Encoder, String> {
        let mut why = String::new();
        for (name, options) in ENCODERS {
            match unsafe { Encoder::try_open(name, options, w, h, fps, kbps) } {
                Ok(e) => return Ok(e),
                Err(e) => why.push_str(&format!("{name}: {e}; ")),
            }
        }
        Err(format!("no encoder worked: {why}"))
    }

    unsafe fn try_open(name: &'static str, options: &[(&str, &str)], w: u32, h: u32, fps: u32, kbps: u32) -> Result<Encoder, String> {
        unsafe {
            let cname = std::ffi::CString::new(name).unwrap();
            let codec = ff::avcodec_find_encoder_by_name(cname.as_ptr());
            if codec.is_null() {
                return Err("not in this ffmpeg".into());
            }
            let ctx = ff::avcodec_alloc_context3(codec);
            let rgb = name != "libx264";
            (*ctx).width = w as i32;
            (*ctx).height = h as i32;
            (*ctx).time_base = ff::AVRational { num: 1, den: 1000 };
            (*ctx).framerate = ff::AVRational { num: fps as i32, den: 1 };
            (*ctx).pix_fmt = if rgb { ff::AVPixelFormat::AV_PIX_FMT_BGR0 } else { ff::AVPixelFormat::AV_PIX_FMT_YUV420P };
            (*ctx).max_b_frames = 0;
            // A whole frame every ten seconds anyway; the page asks for one when it loses something.
            (*ctx).gop_size = (fps * 10) as i32;
            set_rate(ctx, kbps);
            for (k, v) in options {
                let (k, v) = (std::ffi::CString::new(*k).unwrap(), std::ffi::CString::new(*v).unwrap());
                ff::av_opt_set((*ctx).priv_data, k.as_ptr(), v.as_ptr(), 0);
            }
            let r = ff::avcodec_open2(ctx, codec, std::ptr::null_mut());
            if r < 0 {
                let mut ctx = ctx;
                ff::avcodec_free_context(&mut ctx);
                return Err(error(r));
            }
            let frame = ff::av_frame_alloc();
            (*frame).width = w as i32;
            (*frame).height = h as i32;
            (*frame).format = ff::AVPixelFormat::AV_PIX_FMT_BGR0 as i32;
            let (mut sws, mut yuv) = (std::ptr::null_mut(), std::ptr::null_mut());
            if !rgb {
                sws = ff::sws_getContext(w as i32, h as i32, ff::AVPixelFormat::AV_PIX_FMT_BGR0, w as i32, h as i32, ff::AVPixelFormat::AV_PIX_FMT_YUV420P, ff::SwsFlags::SWS_POINT as i32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
                yuv = ff::av_frame_alloc();
                (*yuv).width = w as i32;
                (*yuv).height = h as i32;
                (*yuv).format = ff::AVPixelFormat::AV_PIX_FMT_YUV420P as i32;
                ff::av_frame_get_buffer(yuv, 0);
            }
            Ok(Encoder { ctx, frame, packet: ff::av_packet_alloc(), sws, yuv, name, started: Instant::now(), last_pts: -1 })
        }
    }

    fn rate(&mut self, kbps: u32, fps: u32) {
        unsafe {
            set_rate(self.ctx, kbps);
            (*self.ctx).framerate = ff::AVRational { num: fps as i32, den: 1 };
        }
    }

    /// One picture (BGRX rows of `stride` bytes) in; what comes out, written.
    fn encode(&mut self, pixels: *mut u8, stride: u32, key: bool, out: &mut impl Write) -> Result<(), String> {
        unsafe {
            // In milliseconds, and always forward (the same picture again is a new frame).
            let pts = (self.started.elapsed().as_millis() as i64).max(self.last_pts + 1);
            self.last_pts = pts;
            let input = if self.sws.is_null() {
                // The shared memory itself, as a buffer the frame holds (one
                // it does not own: nothing is freed): handed as it is, not
                // copied first (15 MB a frame on a tablet). The encoder has
                // uploaded it before it answers.
                ff::av_buffer_unref(&mut (*self.frame).buf[0]);
                let size = stride as usize * (*self.ctx).height as usize;
                (*self.frame).buf[0] = ff::av_buffer_create(pixels, size, Some(keep), std::ptr::null_mut(), 0);
                (*self.frame).data[0] = pixels;
                (*self.frame).linesize[0] = stride as i32;
                self.frame
            } else {
                ff::av_frame_make_writable(self.yuv);
                let src = [pixels as *const u8, std::ptr::null(), std::ptr::null(), std::ptr::null()];
                let src_stride = [stride as i32, 0, 0, 0];
                ff::sws_scale(self.sws, src.as_ptr(), src_stride.as_ptr(), 0, (*self.ctx).height, (*self.yuv).data.as_ptr(), (*self.yuv).linesize.as_ptr());
                self.yuv
            };
            (*input).pts = pts;
            (*input).pict_type = if key { ff::AVPictureType::AV_PICTURE_TYPE_I } else { ff::AVPictureType::AV_PICTURE_TYPE_NONE };
            let r = ff::avcodec_send_frame(self.ctx, input);
            if r < 0 {
                return Err(format!("encoding: {}", error(r)));
            }
            loop {
                let r = ff::avcodec_receive_packet(self.ctx, self.packet);
                if r == ff::AVERROR(libc::EAGAIN) || r == ff::AVERROR_EOF {
                    break;
                }
                if r < 0 {
                    return Err(format!("encoding: {}", error(r)));
                }
                let p = &*self.packet;
                let data = std::slice::from_raw_parts(p.data, p.size as usize);
                let whole = p.flags & ff::AV_PKT_FLAG_KEY != 0;
                let mut head = [0u8; 5];
                head[..4].copy_from_slice(&(data.len() as u32).to_be_bytes());
                head[4] = whole as u8;
                let written = out.write_all(&head).and_then(|_| out.write_all(data)).and_then(|_| out.flush());
                ff::av_packet_unref(self.packet);
                written.map_err(|e| format!("out: {e}"))?;
            }
            Ok(())
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_frame_free(&mut self.yuv);
            ff::av_packet_free(&mut self.packet);
            ff::avcodec_free_context(&mut self.ctx);
            if !self.sws.is_null() {
                ff::sws_freeContext(self.sws);
            }
        }
    }
}

/// A buffer's memory is the shared memory's: nothing to free.
unsafe extern "C" fn keep(_: *mut libc::c_void, _: *mut u8) {}

/// The bitrate, with room for a burst (a page scrolled) and a buffer of half
/// a second: what keeps a frame from waiting behind a big one.
unsafe fn set_rate(ctx: *mut ff::AVCodecContext, kbps: u32) {
    unsafe {
        (*ctx).bit_rate = kbps as i64 * 1000;
        (*ctx).rc_max_rate = kbps as i64 * 1500;
        (*ctx).rc_buffer_size = (kbps * 500) as i32;
    }
}

fn error(code: i32) -> String {
    let mut buf = [0 as libc::c_char; 256];
    unsafe { ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- the loop

enum Order {
    Rate(u32, u32),
    Key,
}

fn run(output: &str, mut fps: u32, kbps: u32) -> Result<(), String> {
    let trace = std::env::var_os("PLEAMAR_STREAM_TRACE").is_some();
    let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("wayland: {e}"))?;
    let qh = queue.handle();
    let mut state = State::default();
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wl_shm")?;
    let manager: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 = globals.bind(&qh, 1..=3, ()).map_err(|_| "this compositor has no wlr-screencopy")?;
    let registry = globals.registry();
    let mut names = Vec::new();
    for g in globals.contents().clone_list().into_iter().filter(|g| g.interface == "wl_output") {
        let k = state.outputs.len();
        let o = registry.bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(4), &qh, k);
        state.outputs.push((o, String::new()));
        names.push(g.name);
    }
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    let k = state.outputs.iter().position(|o| o.1 == output).ok_or_else(|| format!("no monitor {output}"))?;
    let target = state.outputs[k].0.clone();
    state.target_global = names[k];

    // What the remote says, as it comes.
    let (tx, orders) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let stdin = std::io::stdin();
        loop {
            line.clear();
            if stdin.read_line(&mut line).unwrap_or(0) == 0 {
                std::process::exit(0);
            }
            let w: Vec<&str> = line.split_whitespace().collect();
            let order = match w.as_slice() {
                ["key"] => Order::Key,
                ["rate", k, f] => Order::Rate(k.parse().unwrap_or(8000), f.parse().unwrap_or(60)),
                _ => continue,
            };
            if tx.send(order).is_err() {
                return;
            }
        }
    });

    let mut out = std::io::BufWriter::with_capacity(1 << 20, std::io::stdout().lock());
    let mut buffers: Vec<Shm> = Vec::new();
    let mut shape: Option<(wl_shm::Format, u32, u32, u32)> = None;
    let mut encoder: Option<Encoder> = None;
    let mut kbps = kbps;
    // The copies on their way, oldest first: (buffer, frame); and the buffer
    // with the last picture (kept: it is encoded again to sharpen it, or as a
    // whole frame when one is asked for).
    let mut flight: std::collections::VecDeque<(usize, zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1)> = Default::default();
    let mut last: Option<usize> = None;
    let mut want_key = true;
    // After a burst: the same picture again, sharper, at these times.
    let mut refresh: Vec<Instant> = Vec::new();
    let mut asked_at = Instant::now() - Duration::from_secs(1);
    // Asks for a copy into buffer `k`; None if the monitor's size is not the
    // one the buffers have (all is made again).
    let ask = |state: &mut State, queue: &mut EventQueue<State>, k: usize, buffers: &[Shm], shape: Option<(wl_shm::Format, u32, u32, u32)>, plain: bool| -> Result<Option<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1>, String> {
        state.copies[k] = Copy::default();
        let frame = manager.capture_output(0, &target, &qh, k);
        if !wait(queue, state, Duration::from_secs(5), |s| s.copies[k].failed || (s.copies[k].shape.is_some() && s.copies[k].shape_done))? || state.copies[k].failed {
            return Err("the compositor gave no picture of the monitor".into());
        }
        if shape.is_none() || state.copies[k].shape != shape {
            frame.destroy();
            return Ok(None);
        }
        if plain {
            frame.copy(&buffers[k].buffer);
        } else {
            frame.copy_with_damage(&buffers[k].buffer);
        }
        Ok(Some(frame))
    };
    loop {
        // (Made again: at the start, and when the monitor changes size.)
        if shape.is_none() {
            for (_, f) in flight.drain(..) {
                f.destroy();
            }
            state.copies[0] = Copy::default();
            let probe = manager.capture_output(0, &target, &qh, 0);
            if !wait(&mut queue, &mut state, Duration::from_secs(5), |s| s.copies[0].failed || (s.copies[0].shape.is_some() && s.copies[0].shape_done))? || state.copies[0].failed {
                return Err("the compositor gave no picture of the monitor".into());
            }
            probe.destroy();
            let wanted = state.copies[0].shape.ok_or("no buffer shape")?;
            buffers.clear();
            for _ in 0..BUFFERS {
                buffers.push(Shm::new(&shm, &qh, wanted)?);
            }
            shape = Some(wanted);
            encoder = Some(Encoder::open(wanted.1, wanted.2, fps, kbps)?);
            eprintln!("pleamar-wm-stream · {output} {}×{} with {}, {kbps} kb/s, {fps} a second", wanted.1, wanted.2, encoder.as_ref().unwrap().name);
            last = None;
            want_key = true;
            // The first at once; the next when it changes.
            match ask(&mut state, &mut queue, 0, &buffers, shape, true)? {
                Some(f) => flight.push_back((0, f)),
                None => {
                    shape = None;
                    continue;
                }
            }
            asked_at = Instant::now();
        }
        // Two on their way, not more often than asked.
        let period = Duration::from_micros(1_000_000 / fps as u64);
        while flight.len() < 2 && asked_at.elapsed() >= period {
            let Some(k) = (0..BUFFERS).find(|k| Some(*k) != last && !flight.iter().any(|(b, _)| b == k)) else { break };
            match ask(&mut state, &mut queue, k, &buffers, shape, false)? {
                Some(f) => flight.push_back((k, f)),
                None => {
                    shape = None;
                    break;
                }
            }
            asked_at = Instant::now();
        }
        if shape.is_none() {
            continue;
        }
        let Some(enc) = encoder.as_mut() else { continue };
        let pitch = shape.unwrap().3;
        let Some(&(k, _)) = flight.front() else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        // Waiting for the oldest copy, the remote's orders and the sharpening
        // are seen to (and the second copy asked for once it is time).
        let mut came = false;
        loop {
            for order in orders.try_iter() {
                match order {
                    Order::Key => want_key = true,
                    Order::Rate(kb, f) => {
                        kbps = kb.clamp(100, 200_000);
                        fps = f.clamp(1, 240);
                        enc.rate(kbps, fps);
                    }
                }
            }
            let due_refresh = refresh.first().is_some_and(|t| Instant::now() >= *t);
            if let Some(l) = last.filter(|_| want_key || due_refresh) {
                if due_refresh {
                    refresh.remove(0);
                }
                enc.encode(buffers[l].data, pitch, std::mem::take(&mut want_key), &mut out)?;
            }
            let c = state.copies[k];
            if c.ready || c.failed {
                came = true;
                break;
            }
            if flight.len() < 2 && asked_at.elapsed() >= Duration::from_micros(1_000_000 / fps as u64) {
                break;
            }
            let mut until = refresh.first().map_or(Duration::from_millis(20), |t| t.saturating_duration_since(Instant::now()).min(Duration::from_millis(20)));
            if flight.len() < 2 {
                until = until.min((asked_at + Duration::from_micros(1_000_000 / fps as u64)).saturating_duration_since(Instant::now()));
            }
            wait(&mut queue, &mut state, until.max(Duration::from_millis(1)), |s| s.copies[k].ready || s.copies[k].failed)?;
        }
        if !came {
            continue;
        }
        let (k, frame) = flight.pop_front().unwrap();
        frame.destroy();
        if state.copies[k].failed {
            // Its size changed (turned): all of it again, with the new shape.
            shape = None;
            continue;
        }
        let t = Instant::now();
        last = Some(k);
        // The next one on its way before this one is encoded, if it is time.
        while flight.len() < 2 && asked_at.elapsed() >= Duration::from_micros(1_000_000 / fps as u64) {
            let Some(n) = (0..BUFFERS).find(|n| Some(*n) != last && !flight.iter().any(|(b, _)| b == n)) else { break };
            match ask(&mut state, &mut queue, n, &buffers, shape, false)? {
                Some(f) => flight.push_back((n, f)),
                None => {
                    shape = None;
                    break;
                }
            }
            asked_at = Instant::now();
        }
        let enc = encoder.as_mut().unwrap();
        enc.encode(buffers[k].data, pitch, std::mem::take(&mut want_key), &mut out)?;
        if trace {
            eprintln!("stream · encoded in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        }
        // The burst may be over: the same picture again in a moment, twice.
        refresh = vec![t + Duration::from_millis(120), t + Duration::from_millis(400)];
    }
}
