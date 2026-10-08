//! Window rule syntax and matching shared by the native sessions.
/// A rule for the windows that match it: by their program (`app=`, the
/// app_id or the X11 class) and/or their title (`title=`), with no case, and
/// `*` for anything. On Windows, the app is the executable filename.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowRule {
    pub app: Option<String>,
    pub title: Option<String>,
    pub float: bool,
    pub size: Option<(u32, u32)>,
    pub monitor: Option<String>,
    pub workspace: Option<usize>,
    pub private: bool,
}

#[derive(Default, Debug, PartialEq)]
pub struct ForWindow {
    pub float: bool,
    pub size: Option<(u32, u32)>,
    pub monitor: Option<String>,
    pub workspace: Option<usize>,
    pub private: bool,
}

pub fn matches(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.to_lowercase(), text.to_lowercase());
    let Some((prefix, tail)) = p.split_once('*') else { return p == t; };
    let Some(rest) = t.strip_prefix(prefix) else { return false; };
    let (middle, suffix) = tail.rsplit_once('*').unwrap_or(("", tail));
    let Some(mut rest) = rest.strip_suffix(suffix) else { return false; };
    for part in middle.split('*').filter(|p| !p.is_empty()) {
        let Some(at) = rest.find(part) else { return false; };
        rest = &rest[at + part.len()..];
    }
    true
}

pub fn for_window(rules: &[WindowRule], app: &str, title: &str) -> ForWindow {
    let mut out = ForWindow::default();
    for r in rules {
        if r.app.as_deref().is_some_and(|p| !matches(p, app)) || r.title.as_deref().is_some_and(|p| !matches(p, title)) {
            continue;
        }
        out.float |= r.float;
        out.private |= r.private;
        out.size = r.size.or(out.size);
        out.monitor = r.monitor.clone().or(out.monitor);
        out.workspace = r.workspace.or(out.workspace);
    }
    out
}

pub fn parse(rest: &[String]) -> Option<WindowRule> {
    let mut r = WindowRule::default();
    let mut k = 0;
    let mut ok = true;
    while k < rest.len() {
        let w = rest[k].as_str();
        if let Some(v) = w.strip_prefix("app=") {
            r.app = Some(v.to_owned());
        } else if let Some(v) = w.strip_prefix("title=") {
            r.title = Some(v.to_owned());
        } else if w == "float" {
            r.float = true;
        } else if w == "private" {
            r.private = true;
        } else if w == "size" {
            r.size = rest.get(k + 1).and_then(|s| s.split_once('x')).and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)));
            ok &= r.size.is_some();
            k += 1;
        } else if w == "monitor" {
            r.monitor = rest.get(k + 1).cloned();
            ok &= r.monitor.is_some();
            k += 1;
        } else if w == "workspace" {
            r.workspace = rest.get(k + 1).and_then(|s| s.parse().ok()).filter(|n: &usize| *n >= 1);
            ok &= r.workspace.is_some();
            k += 1;
        } else {
            ok = false;
        }
        k += 1;
    }
    (ok && (r.app.is_some() || r.title.is_some())).then_some(r)
}

/// Words, with "quoted ones" kept whole (an empty variant is `""`).
pub fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
        } else if ch == '"' {
            chars.next();
            out.push(chars.by_ref().take_while(|c| *c != '"').collect());
        } else {
            // `title="Some title"`: quotes inside a word keep its spaces, and go.
            let mut w = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                chars.next();
                if c == '"' {
                    w.extend(chars.by_ref().take_while(|c| *c != '"'));
                } else {
                    w.push(c);
                }
            }
            out.push(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_and_later_overrides_are_shared_by_both_platforms() {
        let rules: Vec<_> = ["app=PLAYER.exe title=\"Vídeo *\" float size 800x600 monitor 0",
            "app=player* size 900x700", "title=\"Vídeo ñ\" private workspace 3"].into_iter()
            .map(|line|parse(&words(line)).unwrap()).collect();
        let r = for_window(&rules,"player.EXE","Vídeo ñ");
        assert!(r.float && r.private);
        assert_eq!(r.size,Some((900,700)));
        assert_eq!(r.monitor.as_deref(),Some("0"));
        assert_eq!(r.workspace,Some(3));
        assert!(!for_window(&rules,"other","Vídeo ñ").float);
        assert!(!for_window(&rules,"player.exe","Other").float);
    }
    #[test]
    fn wildcards_anchor_the_last_occurrence_and_preserve_unicode_boundaries() {
        for (pattern,text) in [("*a","aba"),("a*a","aba"),("*é*ñ","Él niño ñ"),("*",""),("a**b*","aabab")] {
            assert!(matches(pattern,text),"{pattern}: {text}");
        }
        for (pattern,text) in [("a*a","a"),("a*b","ba"),("*a*b","ba"),("fire","firefox"),("ñ*é","ñ")] {
            assert!(!matches(pattern,text),"{pattern}: {text}");
        }
    }
}

