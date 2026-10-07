//! `pleamar-wm remote`, the direct way: WebRTC between the page and here.
//!
//! The page and home meet over the page's socket (which goes through
//! whatever is in front, Tailscale Funnel), and then the picture and the
//! hands go straight between the two computers over UDP, when the networks
//! on both sides let them: no detour through anyone's servers, and a late
//! packet does not hold up the ones behind it.
//!
//! WebRTC itself is str0m's: here, the socket it speaks through, where this
//! computer is seen from outside (one STUN question), the H.264 frames the
//! monitor gives, and the page's keys and pointer coming back on a data
//! channel.
//!
//! The frames go on a data channel of their own (`video`) when the page opens
//! one —it decodes them itself (WebCodecs), each as soon as it is whole—, and
//! as a video track only if not: a track goes through the browser's jitter
//! buffer, which waits to smooth the picture out, and here every millisecond
//! between a touch and its picture counts more than smoothness.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use str0m::change::SdpOffer;
use str0m::format::Codec;
use str0m::media::{Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::channel::ChannelId;
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};

/// A frame on the `video` channel goes in pieces of at most this (the
/// browsers' messages are only sure up to 64 KB; a whole frame of a tablet is
/// several hundred).
const PIECE: usize = 16 * 1024;
/// More than this waiting to go on the channel: the way is full, the frames
/// are dropped until it empties, and then a whole one.
const CHANNEL_FULL: usize = 1 << 20;

/// What the connection says to the viewer.
pub enum PeerEvent {
    /// The way is open: the picture can go this way now.
    Connected,
    /// A line from the page (a key, the pointer…), as the socket's.
    Line(String),
    /// The page lost too much: it needs a whole frame.
    WholeFrame,
    /// How much the way takes now, in kb/s.
    Estimate(u32),
    Gone,
}

/// A page connected directly.
pub struct Peer {
    frames: mpsc::SyncSender<(bool, Vec<u8>)>,
    pub events: mpsc::Receiver<PeerEvent>,
    alive: Arc<AtomicBool>,
}

impl Peer {
    /// A frame for the page; false if the way is full (it was dropped).
    pub fn send(&self, key: bool, data: Vec<u8>) -> bool {
        self.frames.try_send((key, data)).is_ok()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

/// The page's offer answered: the answer to give it, and the connection.
pub fn answer(offer: &str, kbps: u32) -> Result<(String, Peer), String> {
    static CRYPTO: Once = Once::new();
    CRYPTO.call_once(|| str0m::crypto::from_feature_flags().install_process_default());

    let local = local_ip().ok_or("no network here")?;
    let socket = UdpSocket::bind((local, 0)).map_err(|e| e.to_string())?;
    let here = socket.local_addr().map_err(|e| e.to_string())?;
    // Room on the channel for a whole frame of a tablet (some hundreds of KB:
    // str0m's own 128 KB turned them away, and every one asked for again).
    let mut rtc = RtcConfig::new().enable_bwe(Some(str0m::bwe::Bitrate::kbps(kbps as u64))).set_sctp_max_buffered_amount(4 << 20).build(Instant::now());
    // What it may find out the way takes, up to what is worth sending.
    rtc.bwe().set_desired_bitrate(str0m::bwe::Bitrate::kbps(20_000));
    rtc.add_local_candidate(Candidate::host(here, "udp").map_err(|e| e.to_string())?);
    // How the other side reaches us from outside our router.
    match outside(&socket) {
        Some(public) if public != here => {
            if let Ok(c) = Candidate::server_reflexive(public, here, "udp") {
                rtc.add_local_candidate(c);
            }
            println!("remote · direct way offered at {here} and {public}");
        }
        _ => println!("remote · direct way offered at {here} (no answer from STUN)"),
    }
    let offer = SdpOffer::from_sdp_string(offer).map_err(|e| format!("offer: {e}"))?;
    let answer = rtc.sdp_api().accept_offer(offer).map_err(|e| format!("offer: {e}"))?;

    let (frames_tx, frames_rx) = mpsc::sync_channel(30);
    let (events_tx, events_rx) = mpsc::channel();
    let alive = Arc::new(AtomicBool::new(true));
    let still = alive.clone();
    std::thread::spawn(move || {
        let _ = run(rtc, socket, frames_rx, events_tx.clone(), still);
        let _ = events_tx.send(PeerEvent::Gone);
    });
    Ok((answer.to_sdp_string(), Peer { frames: frames_tx, events: events_rx, alive }))
}

fn run(mut rtc: Rtc, socket: UdpSocket, frames: mpsc::Receiver<(bool, Vec<u8>)>, events: mpsc::Sender<PeerEvent>, alive: Arc<AtomicBool>) -> Result<(), String> {
    let here = socket.local_addr().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut video: Option<(Mid, Pt)> = None;
    let mut buf = vec![0u8; 2000];
    let mut last_keyframe_ask = Instant::now() - Duration::from_secs(10);
    // The page's own channel for the frames, once open; and a frame dropped on
    // it (only a whole one can follow).
    let mut channel: Option<ChannelId> = None;
    let mut seq = 0u32;
    let mut broken = false;
    while alive.load(Ordering::Relaxed) {
        // The frames that came, onto the way.
        while let Ok((key, data)) = frames.try_recv() {
            if let Some(id) = channel {
                seq = seq.wrapping_add(1);
                if broken && !key {
                    continue;
                }
                let Some(mut ch) = rtc.channel(id) else { continue };
                let parts = data.len().div_ceil(PIECE).max(1);
                let mut whole = ch.buffered_amount() < CHANNEL_FULL;
                for (k, piece) in data.chunks(PIECE).enumerate() {
                    if !whole {
                        break;
                    }
                    // [2, whole?, seq: u32, piece: u16, pieces: u16] and the piece.
                    let mut m = Vec::with_capacity(piece.len() + 10);
                    m.extend_from_slice(&[2, key as u8]);
                    m.extend_from_slice(&seq.to_be_bytes());
                    m.extend_from_slice(&(k as u16).to_be_bytes());
                    m.extend_from_slice(&(parts as u16).to_be_bytes());
                    m.extend_from_slice(piece);
                    whole = ch.write(true, &m).unwrap_or(false);
                }
                broken = !whole;
                if broken && last_keyframe_ask.elapsed() > Duration::from_millis(300) {
                    last_keyframe_ask = Instant::now();
                    let _ = events.send(PeerEvent::WholeFrame);
                }
                continue;
            }
            let Some((mid, pt)) = video else { continue };
            let now = Instant::now();
            let rtp = MediaTime::new((now - started).as_micros() as u64 * 9 / 100, Frequency::NINETY_KHZ);
            if let Some(writer) = rtc.writer(mid) {
                if let Err(e) = writer.write(pt, now, rtp, data) {
                    return Err(format!("video: {e}"));
                }
            }
        }
        let timeout = match rtc.poll_output().map_err(|e| e.to_string())? {
            Output::Timeout(t) => t,
            Output::Transmit(t) => {
                let _ = socket.send_to(&t.contents, t.destination);
                continue;
            }
            Output::Event(e) => {
                match e {
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => return Ok(()),
                    Event::Connected => {
                        let _ = events.send(PeerEvent::Connected);
                    }
                    Event::MediaAdded(m) if m.kind == MediaKind::Video => {
                        let pt = rtc.writer(m.mid).and_then(|w| h264(w.payload_params().map(|p| (p.pt(), p.spec())).collect()));
                        if let Some(pt) = pt {
                            video = Some((m.mid, pt));
                        }
                    }
                    Event::ChannelOpen(id, label) if label == "video" => {
                        channel = Some(id);
                        broken = true;
                        let _ = events.send(PeerEvent::WholeFrame);
                    }
                    Event::ChannelData(d) if !d.binary => {
                        if let Ok(line) = String::from_utf8(d.data) {
                            let _ = events.send(PeerEvent::Line(line));
                        }
                    }
                    Event::KeyframeRequest(_) if last_keyframe_ask.elapsed() > Duration::from_millis(800) => {
                        last_keyframe_ask = Instant::now();
                        let _ = events.send(PeerEvent::WholeFrame);
                    }
                    Event::EgressBitrateEstimate(str0m::bwe::BweKind::Twcc { estimate, .. }) => {
                        let _ = events.send(PeerEvent::Estimate((estimate.as_u64() / 1000) as u32));
                    }
                    _ => {}
                }
                continue;
            }
        };
        // Wait for the network, but come back soon for the frames.
        let wait = timeout.saturating_duration_since(Instant::now()).min(Duration::from_millis(2));
        if wait.is_zero() {
            rtc.handle_input(Input::Timeout(Instant::now())).map_err(|e| e.to_string())?;
            continue;
        }
        socket.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
        let input = match socket.recv_from(&mut buf) {
            Ok((n, source)) => match buf[..n].try_into() {
                Ok(contents) => Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: here, contents }),
                Err(_) => continue,
            },
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Input::Timeout(Instant::now()),
            Err(e) => return Err(e.to_string()),
        };
        rtc.handle_input(input).map_err(|e| e.to_string())?;
    }
    rtc.disconnect();
    Ok(())
}

/// The H.264 the page takes, one frame per packet group (mode 1): High
/// profile if it says so (what the card makes), any other if not.
fn h264(params: Vec<(Pt, str0m::format::CodecSpec)>) -> Option<Pt> {
    let h264: Vec<_> = params.into_iter().filter(|(_, s)| s.codec == Codec::H264 && s.format.packetization_mode == Some(1)).collect();
    h264.iter()
        .find(|(_, s)| s.format.profile_level_id.is_some_and(|p| p >> 16 == 0x64))
        .or(h264.first())
        .map(|(pt, _)| *pt)
}

/// The address this computer has on its network (the one its traffic leaves by).
fn local_ip() -> Option<std::net::IpAddr> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("8.8.8.8:80").ok()?;
    probe.local_addr().ok().map(|a| a.ip())
}

/// Where this socket is seen from outside (STUN, RFC 5389): asked of two
/// public servers, the first answer.
fn outside(socket: &UdpSocket) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(700)));
    for server in ["stun.l.google.com:19302", "stun.cloudflare.com:3478"] {
        let Some(to) = server.to_socket_addrs().ok().and_then(|mut a| a.find(|a| a.is_ipv4())) else { continue };
        let mut id = [0u8; 12];
        if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut id)).is_err() {
            continue;
        }
        let mut ask = vec![0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42];
        ask.extend_from_slice(&id);
        for _ in 0..2 {
            if socket.send_to(&ask, to).is_err() {
                break;
            }
            let mut buf = [0u8; 512];
            let deadline = Instant::now() + Duration::from_millis(700);
            while Instant::now() < deadline {
                let Ok((n, from)) = socket.recv_from(&mut buf) else { break };
                if from == to && n >= 20 && buf[8..20] == id {
                    if let Some(a) = mapped(&buf[..n]) {
                        return Some(a);
                    }
                }
            }
        }
    }
    None
}

/// The XOR-MAPPED-ADDRESS of a STUN answer (IPv4).
fn mapped(m: &[u8]) -> Option<SocketAddr> {
    let mut at = 20;
    while at + 4 <= m.len() {
        let kind = u16::from_be_bytes([m[at], m[at + 1]]);
        let len = u16::from_be_bytes([m[at + 2], m[at + 3]]) as usize;
        let v = m.get(at + 4..at + 4 + len)?;
        if kind == 0x0020 && len >= 8 && v[1] == 0x01 {
            let port = u16::from_be_bytes([v[2], v[3]]) ^ 0x2112;
            let ip = [v[4] ^ 0x21, v[5] ^ 0x12, v[6] ^ 0xa4, v[7] ^ 0x42];
            return Some(SocketAddr::from((ip, port)));
        }
        at += 4 + len.div_ceil(4) * 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stun_answer() {
        // A binding success with XOR-MAPPED-ADDRESS 203.0.113.7:51000.
        let port = 51000u16 ^ 0x2112;
        let ip = [203 ^ 0x21, 0 ^ 0x12, 113 ^ 0xa4, 7 ^ 0x42];
        let mut m = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xa4, 0x42];
        m.extend_from_slice(&[0; 12]);
        m.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
        m.extend_from_slice(&port.to_be_bytes());
        m.extend_from_slice(&ip);
        assert_eq!(mapped(&m), Some("203.0.113.7:51000".parse().unwrap()));
    }
}
