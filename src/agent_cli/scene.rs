//! Pleamar windows, asked and used by name over their own sockets.

use super::*;

// ── pleamar windows: asked and used by name ──────────────────────

/// Where the session's pleamar programs listen: what the session told them
/// (`PLEAMAR_SOCKETS`), or the folder it gives them.
pub(super) fn scene_sockets() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var("PLEAMAR_SOCKETS").ok().filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    let (run, display) = (std::env::var("XDG_RUNTIME_DIR").ok()?, std::env::var("WAYLAND_DISPLAY").ok()?);
    Some(format!("{run}/pleamar-{display}").into())
}

/// One order to a scene's socket, and its answer as it comes; `each` gets
/// every line (a `watch` goes on talking).
pub(super) fn order(sock: &std::path::Path, line: &str, each: &mut dyn FnMut(&str)) -> Result<(), String> {
    order_within(sock, line, if line.starts_with("watch") { None } else { Some(std::time::Duration::from_secs(65)) }, each)
}

pub(super) fn order_within(sock: &std::path::Path, line: &str, patience: Option<std::time::Duration>, each: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("{}: {e}", sock.display()))?;
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let _ = s.shutdown(std::net::Shutdown::Write);
    let _ = s.set_read_timeout(patience);
    for l in BufReader::new(s).lines().map_while(Result::ok) {
        each(&l);
    }
    Ok(())
}

pub(super) fn ask(sock: &std::path::Path, line: &str) -> Result<String, String> {
    let mut out = String::new();
    order(sock, line, &mut |l| {
        out.push_str(l);
        out.push('\n');
    })?;
    Ok(out)
}

/// Every pleamar program of the session that answers, by its process: its
/// scene's name and its socket. They are asked who they are (`hello`).
pub(super) fn scenes() -> std::collections::HashMap<u32, (String, std::path::PathBuf)> {
    let mut found = std::collections::HashMap::new();
    let Some(dir) = scene_sockets() else { return found };
    let Ok(entries) = std::fs::read_dir(&dir) else { return found };
    for e in entries.filter_map(Result::ok) {
        let path = e.path();
        // The cursor's and the hands' own sockets live there too, and are no scene's.
        if path.extension().is_none_or(|x| x != "sock") || path.file_stem().is_some_and(|n| n == "cursor" || n == "cua-inject") {
            continue;
        }
        // `pleamar 0.2.25 · scene notes · pid 4521 · language 0.2`. Briefly: some
        // sockets there are not a scene's (the cursor's) and never answer.
        let mut hello = String::new();
        if order_within(&path, "hello", Some(std::time::Duration::from_millis(300)), &mut |l| hello.push_str(l)).is_err() {
            continue;
        }
        let field = |k: &str| hello.split(" · ").find_map(|f| f.strip_prefix(k)).map(|v| v.trim().to_owned());
        if let (Some(scene), Some(pid)) = (field("scene "), field("pid ").and_then(|p| p.parse::<u32>().ok())) {
            found.insert(pid, (scene, path));
        }
    }
    found
}

/// An order to a scene and its answer; one that starts with `?` is what
/// it could not do.
pub(super) fn told(sock: &std::path::Path, line: &str) -> R<String> {
    let answer = ask(sock, line)?;
    if answer.starts_with("? ") {
        return Err(scene_said(&answer));
    }
    Ok(answer)
}

/// A scene window's items as records, in its look coordinates.
pub(super) fn described(k: &Kinds) -> R<Read> {
    let (name, sock) = k.scene.as_ref().ok_or_else(|| k.refuse("tree"))?;
    let json = told(sock, "describe json")?;
    // Of a scene with several windows, which part is this one: by its title.
    let title = if json.matches("\"kind\":\"window").count() > 1 { title_of(k) } else { None };
    scene_read(name, &json, &Place { title, popups: k.popups.clone() })
}

/// That window's title, as `l` gives it.
fn title_of(k: &Kinds) -> Option<String> {
    let reply = wm("l").ok()?;
    let list = reply.strip_prefix("windows")?;
    let process = k.name.split(['.', ':']).next()?;
    list.split('|').map(|e| e.split_whitespace().collect::<Vec<&str>>()).filter(|f| f.len() >= 12 && f[0] == process).find(|f| k.slot.is_none_or(|s| f[11] == s.to_string())).map(|f| unhex(f[8]))
}

/// The item of a scene that an element or a `--text` names: its name.
pub(super) fn item(k: &Kinds, a: &Args) -> R<String> {
    match (&a.element, a.text()) {
        (Some(Element::Name(n)), _) => Ok(n.clone()),
        (Some(Element::Id(n)), _) => Err(Fail::new(fail::NOT_HERE, format!("{} is a pleamar scene: its items are named, @name (a name of digits: @={n})", k.name))),
        (None, Some(t)) => {
            let read = described(k)?;
            let found: Vec<&Rec> = read.recs.iter().filter(|r| r.id != "@-" && r.named(t)).collect();
            match Element::parse(&one(&found, t, a.first(), &k.name)?.id) {
                Some(Element::Name(n)) => Ok(n),
                _ => Err(format!("{t:?} names no item of {} that can be acted on", k.name).into()),
            }
        }
        (None, None) => Err(usage("which item: @name, or --text T (its whole name)")),
    }
}

/// `say PID ORDER...`: any order to the scene, as it is.
pub(super) fn say(a: &Args) -> R {
    let k = Kinds::of(a.win()?)?;
    k.open()?;
    let (_, sock) = k.scene.as_ref().ok_or_else(|| k.refuse("say"))?;
    let line = a.words.join(" ");
    if line.trim().is_empty() {
        return Err(usage("say what: say PID type query words, say PID drag knob 0 -40"));
    }
    print!("{}", told(sock, line.trim())?);
    Ok(())
}


