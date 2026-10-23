//! The session's socket: the hands, their pace, and its lines.

use super::*;

/// Where the session's socket is.
pub(super) fn socket() -> Option<String> {
    if let Ok(p) = std::env::var("CUA_INJECT_SOCKET") {
        if !p.is_empty() {
            return Some(p);
        }
    }
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    let path = crate::agent_socket_path(&display);
    std::path::Path::new(&path).exists().then_some(path)
}

/// The session's socket, or why there is none (exit 69).
fn connect() -> R<UnixStream> {
    let none = |why: String| Fail::new(fail::NO_SESSION, format!("no session: {why}: is this a pleamar-wm session with `agent on` in session.conf?"));
    let path = socket().ok_or_else(|| none("no agent socket here".into()))?;
    let stream = UnixStream::connect(&path).map_err(|e| none(format!("{path}: {e}")))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    Ok(stream)
}

/// One question to the window manager, as no agent in particular (what is
/// asked, not done: `C`).
pub(super) fn wm(line: &str) -> R<String> {
    let stream = connect()?;
    let writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut asked = Line { writer, reader: BufReader::new(stream) };
    asked.say("cua-inject v1")?;
    Ok(asked.say(line)?)
}

/// One line to the session's socket, as no agent in particular (its cursor's
/// label is left as it is): the reply.
pub(crate) fn tell(line: &str) -> Result<String, String> {
    Line::open()?.say(line)
}

/// The session's socket kept open, for what is said many times a second (a
/// finger moving on a phone's glass): each line, and its reply.
pub(crate) struct Line {
    pub(super) writer: UnixStream,
    pub(super) reader: BufReader<UnixStream>,
}

impl Line {
    pub(crate) fn open() -> Result<Line, String> {
        let path = socket().ok_or("no agent socket here")?;
        let stream = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut line = Line { writer, reader: BufReader::new(stream) };
        line.say("cua-inject v1")?;
        Ok(line)
    }

    pub(crate) fn say(&mut self, line: &str) -> Result<String, String> {
        writeln!(self.writer, "{line}").map_err(|e| e.to_string())?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply).map_err(|e| e.to_string())? == 0 {
            return Err("the session hung up".into());
        }
        Ok(reply.trim_end().to_owned())
    }
}



pub(super) struct Hands {
    pub(super) reader: BufReader<UnixStream>,
    pub(super) writer: UnixStream,
    /// The session's pace, asked once.
    pub(super) pace: Pace,
}

/// A glide of FAR units at a pace: its time in ms, its steps, and the moment
/// still before pressing; none, one motion.
pub(super) fn glide(pace: Pace, far: f64) -> Option<(f64, u32, u64)> {
    match pace {
        Pace::Default => Some(((220.0 + far * 0.45).clamp(260.0, 620.0), ((far / 22.0).round() as u32).clamp(8, 24), 70)),
        Pace::Fast => Some(((60.0 + far * 0.12).clamp(80.0, 150.0), ((far / 80.0).round() as u32).clamp(4, 6), 0)),
        Pace::Instant => None,
    }
}

/// A drag's steps and the ms between scrolls at a pace.
pub(super) fn strokes(pace: Pace) -> (u32, u64) {
    match pace {
        Pace::Default => (24, 60),
        Pace::Fast | Pace::Instant => (8, 20),
    }
}

impl Hands {
    pub(super) fn open() -> R<Self> {
        let stream = connect()?;
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut hands = Hands { reader: BufReader::new(stream), writer, pace: Pace::Default };
        let hello = hands.say("cua-inject v1")?;
        if hello != "cua-inject v1" {
            return Err(Fail::new(fail::NO_SESSION, format!("no session: the socket answered {hello:?}")));
        }
        // Who it is, for the cursor's label: `PLEAMAR_AGENT_NAME` (Marea
        // says hers), «agent» otherwise. An older session does not know
        // it: nothing is lost.
        let _ = hands.say(&format!("n {}", hex(&agent_name())));
        // How fast it goes is the session's (`T`); an older one: as ever.
        if let Some(p) = hands.say("T").unwrap_or_default().strip_prefix("pace ").and_then(Pace::parse) {
            hands.pace = p;
        }
        Ok(hands)
    }

    pub(super) fn say(&mut self, line: &str) -> R<String> {
        writeln!(self.writer, "{line}").map_err(|e| e.to_string())?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply).map_err(|e| e.to_string())? == 0 {
            return Err("the session hung up".into());
        }
        Ok(reply.trim_end().to_owned())
    }

    /// Where the scene shows the window on the desktop, `[X, Y, W, H]`, and
    /// how much it scales it (`r`; an older session: 1): its own units,
    /// `m`'s, are W / ZOOM wide.
    pub(super) fn shown(&mut self, pid: &str) -> R<Option<([f64; 4], f64)>> {
        let reply = self.say(&format!("r {pid}"))?;
        let v: Vec<f64> = reply.strip_prefix("rect ").map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
        Ok((v.len() >= 4).then(|| ([v[0], v[1], v[2], v[3]], v.get(5).copied().filter(|z| *z > 0.0).unwrap_or(1.0))))
    }

    /// The cursor to a point of that window, the way a hand takes it there:
    /// along the way, eased, with the motions a pointer makes. It is seen
    /// travelling (not jumping), and programs that only light what is under
    /// the pointer when it moves over it (a browser's menu: it took no press
    /// on an item it had not seen the pointer move over) see it move.
    pub(super) fn arrive(&mut self, pid: &str, x: f64, y: f64) -> R {
        let from = self.say("p 0")?;
        let f = from.strip_prefix("at ").map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect::<Vec<f64>>()).filter(|f| f.len() >= 2);
        let shown = self.shown(pid)?;
        // From where it is, if that is on this window (in its units); else
        // from a little before the point, so there is a way to go.
        let start = match (&f, &shown) {
            (Some(f), Some((r, z))) => {
                let (sx, sy) = ((f[0] - r[0]) / z, (f[1] - r[1]) / z);
                (sx >= 0.0 && sy >= 0.0 && sx < r[2] / z && sy < r[3] / z).then_some((sx, sy))
            }
            _ => None,
        };
        let (w, h) = shown.map_or((f64::MAX, f64::MAX), |(r, z)| ((r[2] / z).max(1.0), (r[3] / z).max(1.0)));
        let (sx, sy) = start.unwrap_or(((x - 70.0).clamp(0.0, w - 1.0), (y - 45.0).clamp(0.0, h - 1.0)));
        let far = ((x - sx).powi(2) + (y - sy).powi(2)).sqrt();
        if far < 2.0 {
            return self.act(&format!("m root:{pid} 0 {x} {y}"));
        }
        let Some((total, steps, still)) = glide(self.pace, far) else {
            // One motion, from a unit short of the point: a program sees the
            // pointer move over what is there, not only land on it.
            let _ = self.say(&format!("m root:{pid} 0 {:.1} {:.1}", x - (x - sx) / far, y - (y - sy) / far));
            return self.act(&format!("m root:{pid} 0 {x} {y}"));
        };
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let e = t * t * (3.0 - 2.0 * t);
            let (px, py) = (sx + (x - sx) * e, sy + (y - sy) * e);
            if k < steps {
                // On the way it may cross where nothing of the window is
                // (its shadow): those steps are only skipped.
                let _ = self.say(&format!("m root:{pid} 0 {px:.1} {py:.1}"));
                std::thread::sleep(std::time::Duration::from_millis((total / steps as f64) as u64));
            } else {
                self.act(&format!("m root:{pid} 0 {x} {y}"))?;
            }
        }
        // A moment on the spot, as a hand stops before it presses.
        std::thread::sleep(std::time::Duration::from_millis(still));
        Ok(())
    }

    /// A command that has to be answered `ok`: an `err` is about the
    /// window its target names.
    pub(super) fn act(&mut self, line: &str) -> R {
        match self.say(line)? {
            r if r == "ok" => Ok(()),
            r => {
                let target = line.split_whitespace().nth(1).unwrap_or_default();
                Err(answered(&r, Of::Window(target.strip_prefix("root:").unwrap_or(target))))
            }
        }
    }
}

pub(super) fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn unhex(s: &str) -> String {
    if s == "-" {
        return String::new();
    }
    let bytes: Vec<u8> = (0..s.len() / 2).filter_map(|k| u8::from_str_radix(&s[2 * k..2 * k + 2], 16).ok()).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}


/// Who the agent is: `PLEAMAR_AGENT_NAME`, `agent` otherwise.
pub(super) fn agent_name() -> String {
    std::env::var("PLEAMAR_AGENT_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "agent".to_owned())
}


