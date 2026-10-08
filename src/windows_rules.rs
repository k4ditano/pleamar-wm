//! Windows rule policy; the syntax and selector matching are shared with Linux.
use super::*;
use crate::window_rules::{self, ForWindow, WindowRule};
use std::{collections::BTreeMap, io::Read, path::PathBuf};

pub(super) struct Rules {
    pub path: PathBuf,
    pub entries: Vec<WindowRule>,
    pub applied: BTreeMap<String, bool>,
    pub errors: BTreeMap<String, String>,
}

impl Rules {
    pub fn read(path: PathBuf, explicit: bool) -> Result<Self> {
        let mut text = String::new();
        match std::fs::File::open(&path) {
            Ok(file) => { file.take(65537).read_to_string(&mut text)?; }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {}
            Err(e) => return Err(format!("{}: {e}", path.display()).into()),
        }
        let entries = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { path, entries, applied:BTreeMap::new(), errors:BTreeMap::new() })
    }
    pub fn for_window(&self, window: &Window) -> ForWindow {
        window_rules::for_window(&self.entries, &window.app, &window.title)
    }
    pub fn floats(&self, id: &str) -> bool { self.applied.get(id).copied().unwrap_or(false) }
    pub fn forget_closed(&mut self) {
        self.applied.retain(|id, _| identity_exists(id));
        self.errors.retain(|id, _| identity_exists(id));
    }
}

fn identity_exists(id: &str) -> bool {
    id.split(':').nth(2).and_then(|s|usize::from_str_radix(s,16).ok())
        .and_then(|h|Identity::read(HWND(h as _))).is_some_and(|i|i.token()==id)
}

fn parse(text: &str) -> Result<Vec<WindowRule>> {
    if text.len() > 65536 { return Err("window rules exceed 64 KiB".into()); }
    let mut rules = Vec::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        // A '#' in a quoted window title is not a comment. Backslashes in
        // native monitor names are literal, just as in the Linux tokenizer.
        let mut quoted = false;
        let end = line.char_indices().find_map(|(at, ch)| {
            if ch == '"' { quoted = !quoted; }
            (ch == '#' && !quoted).then_some(at)
        }).unwrap_or(line.len());
        let words = window_rules::words(&line[..end]);
        if words.first().map(String::as_str) != Some("window") { continue; }
        let error = |message: &str| format!("line {}: {message}", index + 1);
        if quoted { return Err(error("unclosed quote in window rule").into()); }
        let rule = window_rules::parse(&words[1..]).ok_or_else(||error("invalid window rule"))?;
        if rule.private || rule.workspace.is_some() {
            return Err(error("private and workspace window rules are not available on Windows yet").into());
        }
        if rule.size.is_some_and(|(w,h)| w==0 || h==0 || w>32768 || h>32768) {
            return Err(error("window dimensions must be 1..32768 logical pixels").into());
        }
        if rule.monitor.as_deref() == Some("") { return Err(error("monitor name is empty").into()); }
        rules.push(rule);
        if rules.len() > 256 { return Err(error("at most 256 window rules are supported").into()); }
    }
    Ok(rules)
}

pub(super) fn destination<'a>(rule: &ForWindow, current: &Window, screens: &'a [Monitor]) -> Result<&'a Monitor> {
    let name = rule.monitor.as_deref().unwrap_or(&current.monitor);
    name.parse::<usize>().ok().and_then(|i|screens.get(i))
        .or_else(||screens.iter().find(|m|m.name==name))
        .ok_or_else(||format!("rule monitor {name} is not connected").into())
}

pub(super) fn bounds(rule: &ForWindow, window: &Window, screen: &Monitor) -> Result<Bounds> {
    let (width,height) = rule.size.map(|(w,h)| ((w as f64*screen.scale).round() as i32,
        (h as f64*screen.scale).round() as i32)).unwrap_or((window.bounds.width,window.bounds.height));
    if width<=0 || height<=0 || width>screen.work.width || height>screen.work.height {
        return Err("requested rule size does not fit the monitor work area".into());
    }
    let (x,y) = if window.monitor == screen.name { (window.bounds.x,window.bounds.y) }
        else { (screen.work.x+(screen.work.width-width)/2,screen.work.y+(screen.work.height-height)/2) };
    Ok(Bounds { x:x.clamp(screen.work.x, screen.work.x+screen.work.width-width),
        y:y.clamp(screen.work.y, screen.work.y+screen.work.height-height), width,height })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_native_rules_without_silently_accepting_missing_capabilities() {
        let rules = parse("\u{feff}# config\nwindow app=Player.exe title=\"Capítulo #1\" float size 800x500 monitor \\\\.\\DISPLAY2 # comment").unwrap();
        assert_eq!(rules.len(),1);
        assert_eq!(rules[0].title.as_deref(),Some("Capítulo #1"));
        assert_eq!(rules[0].monitor.as_deref(),Some(r"\\.\DISPLAY2"));
        for text in ["window float", "window app=x private", "window app=x workspace 1", "window app=x size 0x10",
            "window title=\"broken float", "window app=x size 4294967295x1", "window app=x monitor \"\""] {
            assert!(parse(text).is_err(), "accepted {text}");
        }
        assert!(parse(&"window app=x float\n".repeat(257)).is_err());
    }
    #[test]
    fn rule_geometry_uses_destination_dpi_and_rejects_oversize() {
        let screen = Monitor { name:"second".into(), bounds:Bounds{x:-1920,y:200,width:1920,height:1080},
            work:Bounds{x:-1920,y:200,width:1920,height:1020},scale:1.5,primary:false,refresh_hz:60 };
        let window = Window { id:"test".into(),title:"".into(),app:"test.exe".into(),class:"".into(),process:1,
            monitor:"first".into(),bounds:Bounds{x:100,y:100,width:400,height:300},minimized:false,maximized:false,resizable:true };
        let mut rule = ForWindow { size:Some((800,600)), ..Default::default() };
        let target = bounds(&rule,&window,&screen).unwrap();
        assert_eq!((target.width,target.height),(1200,900));
        assert!(screen.work.contains(&target));
        rule.size = Some((1600,900));
        assert!(bounds(&rule,&window,&screen).is_err());
        rule.monitor = Some("outside".into());
        assert!(destination(&rule,&window,std::slice::from_ref(&screen)).is_err());
        rule.monitor = Some("0".into());
        assert_eq!(destination(&rule,&window,std::slice::from_ref(&screen)).unwrap().name,"second");
    }
}
