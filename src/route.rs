//! Where input goes: to the scene, or to a program's surface (a bar, Marea,
//! a lock screen). The session feeds it what libinput says; headless, a
//! script (`PLEAMAR_HEADLESS_INPUT`) — the same road, so what is tried
//! without a screen is what happens with one.

use crate::layers::{self, ToLayers};
use crate::screen::{self, Hit, Screen};
use pleamar::scene::{Mods, ToRender};
use std::sync::mpsc::Sender;

pub struct Route {
    /// Who has the pointer: the scene, or a program's surface (layer-shell).
    pub hit: Hit,
    /// The program's surface a button was pressed on: it keeps the pointer until it is let go.
    grab: Option<u64>,
    /// The buttons pressed on the scene: the scene keeps the pointer, and gets
    /// their release, wherever they are let go —over a bar, over Marea—. Given
    /// to the program under it instead, a window carried by its title bar never
    /// heard it had been let go, and stayed stuck to the mouse. Each button on
    /// its own: with one flag, letting go of the right one first gave the
    /// left one's release to whatever was under the pointer.
    scene_held: Vec<u32>,
    /// And the ones pressed on a program's surface (`grab`): it keeps the
    /// pointer until the last is let go, and gets each release.
    client_held: Vec<u32>,
    /// The program's surface that took the keyboard when clicked, and how many
    /// times a window had taken it by then (`layers::window_keyboard`): if a
    /// window has since (the scene's focus on hover, a shortcut), the surface
    /// no longer has it.
    key_client: Option<u64>,
    key_client_at: u64,
    /// Keys held down that went to a binding: their release is the binding's
    /// too, and does what its `release` says, if it has one.
    bound: Vec<(u32, Option<crate::keys::Action>)>,
    /// The binding held down that repeats, by its key; and whether its
    /// repetition still has to be started (the loop that holds the timers does it).
    repeating: Option<(u32, crate::keys::Action)>,
    repeat_new: bool,
    /// Keys held down that went to a program's surface, and to which: their
    /// release goes there too, even if it no longer has the keyboard. Marea's
    /// search closing on a key press never heard it let go, and the keyboard
    /// kept that key held for every window after it (a Backspace that erased
    /// everything, letters that were shortcuts).
    layer_keys: Vec<(u32, u64)>,
    /// Super held: the scene is told (`keys.super`), and with it a window is
    /// carried from anywhere on it.
    super_held: bool,
    to_render: Sender<ToRender>,
}

impl Route {
    pub fn new(to_render: Sender<ToRender>) -> Route {
        Route { hit: Hit::Scene(None), grab: None, scene_held: Vec::new(), client_held: Vec::new(), key_client: None, key_client_at: 0, bound: Vec::new(), repeating: None, repeat_new: false, layer_keys: Vec::new(), super_held: false, to_render }
    }

    /// The pointer at that point of a monitor, in its pixels. Says whether it
    /// went from the scene to a program or back (the cursor changes hands).
    pub fn pointer(&mut self, on: &Screen, (mx, my): (f64, f64)) -> bool {
        // The one holding it is no longer drawn (a menu whose window went to
        // another workspace): it is let go, as if its buttons were.
        if let Some(id) = self.grab {
            if on.0.lock().unwrap().clients.iter().any(|c| c.id == id && !c.drawn) {
                for code in std::mem::take(&mut self.client_held) {
                    layers::tell(ToLayers::Button { code, down: false });
                }
                self.grab = None;
            }
        }
        let hit = {
            let st = on.0.lock().unwrap();
            // (Dragging something out of it, the pointer goes where it is:
            // the windows and surfaces it crosses are where it may be let go.)
            match self.grab.filter(|_| !layers::dragging()) {
                // Held: the one it was pressed on keeps it, wherever it goes.
                Some(id) => match st.clients.iter().find(|c| c.id == id) {
                    Some(c) => Hit::Client(id, c.local(mx, my)),
                    None => self.hit,
                },
                None if !self.scene_held.is_empty() && !layers::dragging() => match st.layers.iter().find(|l| l.main) {
                    Some(l) => Hit::Scene(Some((l.origin.0 + (mx - l.rect[0] as f64) as f32 / l.scale, l.origin.1 + (my - l.rect[1] as f64) as f32 / l.scale))),
                    None => screen::pointer_at(&st, (mx, my)),
                },
                None => screen::pointer_at(&st, (mx, my)),
            }
        };
        self.point(hit)
    }

    /// The pointer goes to whoever takes it now, and leaves whoever had it.
    fn point(&mut self, hit: Hit) -> bool {
        match hit {
            Hit::Scene(at) => {
                if matches!(self.hit, Hit::Client(..)) {
                    layers::tell(ToLayers::PointerOut);
                }
                let _ = self.to_render.send(ToRender::Pointer(at));
            }
            Hit::Client(id, (x, y)) => {
                if matches!(self.hit, Hit::Scene(Some(_))) {
                    let _ = self.to_render.send(ToRender::Pointer(None));
                }
                layers::tell(ToLayers::Pointer { id, x, y });
            }
        }
        let was_program = matches!(self.hit, Hit::Client(..));
        self.hit = hit;
        let on_program = matches!(hit, Hit::Client(..));
        // The scene is told, so that the window under a program's surface is
        // pointed at again when the pointer comes back to it from there: a
        // window «pointed» all the time, wherever the program's surface is
        // over it, never took the keyboard back from Marea's box on hover.
        if was_program != on_program {
            let _ = self.to_render.send(ToRender::Fact("pointer.panel", if on_program { 1.0 } else { 0.0 }));
        }
        was_program != on_program
    }

    /// A button (evdev code). Says whether a program's surface had it: then,
    /// let go, the pointer is to be placed again (the grab is over).
    pub fn button(&mut self, screens: &[Screen], code: u32, down: bool) -> bool {
        // Let go while a program drags something: straight to the compositor,
        // which drops it wherever the pointer is (a window, a surface, the scene).
        if layers::dragging() && !down {
            self.grab = None;
            self.client_held.retain(|c| *c != code);
            // Pressed on the scene (on a window of it): the scene is told it is
            // let go too, or it went on holding it, and the pointer with it.
            if self.scene_held.contains(&code) {
                self.scene_held.retain(|c| *c != code);
                if let 0x110..=0x116 = code {
                    let _ = self.to_render.send(ToRender::Button((code - 0x110) as u8, false));
                }
            }
            layers::tell(ToLayers::Button { code, down });
            return true;
        }
        // Let go of what was pressed on the scene: the scene's, wherever it is.
        if !down && self.scene_held.contains(&code) {
            self.scene_held.retain(|c| *c != code);
            if let 0x110..=0x116 = code {
                let _ = self.to_render.send(ToRender::Button((code - 0x110) as u8, false));
            }
            return false;
        }
        if !down && self.client_held.contains(&code) {
            self.client_held.retain(|c| *c != code);
            if self.client_held.is_empty() {
                self.grab = None;
            }
            layers::tell(ToLayers::Button { code, down });
            return true;
        }
        // Pressed while it was still held: its release was lost somewhere (a
        // window gone, a workspace changed under it). Whoever had it is let
        // go first, or a program went on taking every move for a drag —text
        // selected, a window stuck to the mouse— until the session ended.
        if down && self.scene_held.contains(&code) {
            self.scene_held.retain(|c| *c != code);
            if let 0x110..=0x116 = code {
                let _ = self.to_render.send(ToRender::Button((code - 0x110) as u8, false));
            }
        }
        if down && self.client_held.contains(&code) {
            self.client_held.retain(|c| *c != code);
            layers::tell(ToLayers::Button { code, down: false });
            if self.client_held.is_empty() {
                self.grab = None;
            }
        }
        if let Hit::Client(id, _) = self.hit {
            if down {
                self.grab = Some(id);
                if !self.client_held.contains(&code) {
                    self.client_held.push(code);
                }
                let takes = screens.iter().any(|s| screen::takes_keyboard_on_click(&s.0.lock().unwrap(), id));
                self.set_key_client(if takes { Some(id) } else { None });
            }
            layers::tell(ToLayers::Button { code, down });
            return true;
        }
        if down {
            self.set_key_client(None);
            layers::tell(ToLayers::ScenePress);
            if !self.scene_held.contains(&code) {
                self.scene_held.push(code);
            }
        }
        // Left, right, middle; and the side ones (back, forward), which only
        // the windows use.
        let b = match code {
            0x110..=0x116 => (code - 0x110) as u8,
            _ => return false,
        };
        let _ = self.to_render.send(ToRender::Button(b, down));
        false
    }

    /// Sideways, in notches (to the right, negative, as down is): only the
    /// scene's (a deck of cards, the line between two apps on a tablet); the
    /// programs are given the wheel up and down only.
    pub fn wheel_sideways(&self, notches: f32) {
        if !matches!(self.hit, Hit::Client(..)) {
            let _ = self.to_render.send(ToRender::Wheel(notches));
        }
    }

    /// The wheel, in notches: to whoever has the pointer.
    pub fn wheel(&self, notches: f32) {
        if matches!(self.hit, Hit::Client(..)) {
            layers::tell(ToLayers::Wheel(notches));
        } else {
            let _ = self.to_render.send(ToRender::Wheel(notches));
        }
    }

    /// Fingers moving what is under the pointer (a touchpad's two, one on a
    /// phone's glass): to a program as far as they went, in units (down and to
    /// the right, positive), and to the scene as the notches that is worth.
    pub fn scroll(&self, x: f64, y: f64, notches: f32) {
        if matches!(self.hit, Hit::Client(..)) {
            layers::tell(ToLayers::Scroll(x, y));
        } else {
            let _ = self.to_render.send(ToRender::Scroll { x: x as f32, y: y as f32, notches });
        }
    }

    /// The fingers lifted: what was moving may go on by itself.
    pub fn scroll_end(&self) {
        if matches!(self.hit, Hit::Client(..)) {
            layers::tell(ToLayers::ScrollEnd);
        } else {
            let _ = self.to_render.send(ToRender::ScrollEnd);
        }
    }

    /// Fingers from elsewhere (a phone's glass, through `pleamar-wm remote`).
    /// The scene's own zones hear a notch every 50 units, as they did when
    /// the phone's page counted them itself.
    pub fn fingers(&self, f: layers::Fingers) {
        match f {
            layers::Fingers::Moved(x, y) => self.scroll(x, y, (-(x + y) / 50.0) as f32),
            layers::Fingers::Up => self.scroll_end(),
        }
    }

    /// Where the keys go: a program's surface that takes all of it; else the
    /// one clicked that takes it on demand; else the scene.
    pub fn key_owner(&self, screens: &[Screen]) -> Option<u64> {
        let all = screens.iter().find_map(|s| screen::keyboard_taker(&s.0.lock().unwrap()));
        // Only while it still asks for it: a card that closes gives it back.
        let alive = |id: u64| screens.iter().any(|s| s.0.lock().unwrap().clients.iter().any(|c| c.id == id && c.keyboard != 0 && !c.pieces.is_empty()));
        all.or(self.key_client.filter(|id| self.key_client_at == layers::window_keyboard() && alive(*id)))
    }

    /// A key, by its keysym's name, what it types (if anything) and its evdev code.
    /// `base`: the key's own name, without what Shift makes of it (`1` where
    /// it types `!`): a binding says `Super+Shift+1`, whatever the layout.
    pub fn key(&mut self, screens: &[Screen], name: &str, base: Option<&str>, typed: Option<String>, mods: Mods, evdev: u32, down: bool) {
        if name.starts_with("Super_") && self.super_held != down {
            self.super_held = down;
            let _ = self.to_render.send(ToRender::Fact("keys.super", if down { 1.0 } else { 0.0 }));
        }
        // A binding (keys.conf) takes the key before anyone —while locked,
        // only the ones marked `locked`—.
        if down {
            let keys = crate::keys::get();
            let open = |b: &&crate::keys::Bind| b.locked || !layers::locked();
            let bind = keys.bind(name, mods).or_else(|| base.and_then(|b| keys.bind(b, mods))).filter(open);
            //  What letting it go does is decided now: by then the modifiers may be up.
            let later = keys.on_release(name, mods).or_else(|| base.and_then(|b| keys.on_release(b, mods))).filter(open);
            if bind.is_some() || later.is_some() {
                if let Some(b) = bind {
                    println!("session · key {}{}{}{}{name} → {:?}", if mods.ctrl { "Ctrl+" } else { "" }, if mods.alt { "Alt+" } else { "" }, if mods.shift { "Shift+" } else { "" }, if mods.logo { "Super+" } else { "" }, b.action);
                    self.perform(&b.action);
                    if b.repeat {
                        self.repeating = Some((evdev, b.action.clone()));
                        self.repeat_new = true;
                    }
                }
                self.bound.push((evdev, later.map(|b| b.action.clone())));
                return;
            }
            // A program's global shortcut (the portal): down now, up when let go.
            if let Some((session, id)) = crate::portal::shortcut_at(name, base, mods) {
                crate::portal::shortcut_signal(&session, &id, true);
                self.bound.push((evdev, Some(crate::keys::Action::ShortcutUp(session, id))));
                return;
            }
        }
        if !down {
            if self.repeating.as_ref().is_some_and(|(c, _)| *c == evdev) {
                self.repeating = None;
            }
            if let Some(k) = self.bound.iter().position(|(c, _)| *c == evdev) {
                if let (_, Some(action)) = self.bound.remove(k) {
                    self.perform(&action);
                }
                return;
            }
        }
        if down {
            let owner = self.key_owner(screens);
            // Locked, a key goes to the lock screen or nowhere: never to the
            // scene or its windows behind it.
            if owner.is_none() && layers::locked() {
                return;
            }
            // Shortcuts (never what is typed): where each went, to find out why one does nothing.
            if mods.ctrl || mods.alt || mods.logo || name == "Escape" {
                println!("session · key {}{}{}{name} → {}", if mods.ctrl { "Ctrl+" } else { "" }, if mods.alt { "Alt+" } else { "" }, if mods.logo { "Super+" } else { "" }, owner.map_or("the scene".to_owned(), |id| format!("the program's surface {id}")));
            }
            if let Some(id) = owner {
                self.layer_keys.retain(|(c, _)| *c != evdev);
                self.layer_keys.push((evdev, id));
                layers::tell(ToLayers::Key { id, code: evdev, down: true });
                return;
            }
            let _ = self.to_render.send(ToRender::Key(name.to_owned(), typed, mods, evdev));
        } else {
            // A key let go goes where it went down; to both, if that is not known.
            let went_to = self.layer_keys.iter().position(|(c, _)| *c == evdev).map(|k| self.layer_keys.remove(k).1);
            if let Some(id) = went_to.or_else(|| self.key_owner(screens)) {
                layers::tell(ToLayers::Key { id, code: evdev, down: false });
            }
            if layers::locked() {
                return;
            }
            let _ = self.to_render.send(ToRender::KeyReleased(name.to_owned(), evdev));
        }
    }

    /// A repeating binding just pressed: its key, once, for the loop to start its timer.
    pub fn take_new_repeat(&mut self) -> Option<u32> {
        if !std::mem::take(&mut self.repeat_new) {
            return None;
        }
        self.repeating.as_ref().map(|(c, _)| *c)
    }

    /// The timer of a repeating binding: acts again if that key is still held.
    pub fn repeat(&self, evdev: u32) -> bool {
        match &self.repeating {
            Some((c, action)) if *c == evdev => {
                self.perform(action);
                true
            }
            _ => false,
        }
    }

    /// A binding's action: a program, or an event of the window manager's scene.
    pub fn perform(&self, action: &crate::keys::Action) {
        match action {
            crate::keys::Action::Launch(command) => layers::tell(ToLayers::Launch(command.clone())),
            crate::keys::Action::Emit(event, n) => {
                let _ = self.to_render.send(ToRender::ExternalSignal(pleamar::scene::intern(event), *n));
            }
            crate::keys::Action::ShortcutUp(session, id) => crate::portal::shortcut_signal(session, id, false),
        }
    }

    fn set_key_client(&mut self, id: Option<u64>) {
        if self.key_client.is_some() && id.is_none() {
            layers::tell(ToLayers::KeyboardBack);
        }
        self.key_client = id;
        self.key_client_at = layers::window_keyboard();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::ClientLayer;

    struct Nowhere;
    impl crate::screen::Output for Nowhere {
        fn buffer(&mut self, _: &pleamar::wgpu::Device, _: &[u64]) -> Option<(usize, pleamar::wgpu::Texture)> {
            None
        }
        fn show(&mut self, _: usize, _: pleamar::Sent, _: &pleamar::wgpu::Device, _: &pleamar::wgpu::Queue, _: bool) -> bool {
            false
        }
    }

    fn menu(drawn: bool) -> ClientLayer {
        let piece = layers::ClientPiece { key: 1, at: (0, 0), size: (100, 100), px: (100, 100), content: None, buffer: None, opaque: false };
        ClientLayer { id: 9, level: 3, rect: [10, 10, 100, 100], pieces: vec![piece], region: None, keyboard: 0, blur: Vec::new(), blur_cut: 0.0, owner: 0, zoom: 1.0, drawn }
    }

    /// A menu pressed on keeps the pointer; once it is no longer drawn (its
    /// window went to another workspace) it is let go of it.
    #[test]
    fn a_menu_no_longer_drawn_loses_its_grab() {
        let _drag = layers::tests::DRAGGING.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut route = Route::new(tx);
        let on = crate::screen::screen("A".into(), (1920, 1080), Box::new(Nowhere));
        on.0.lock().unwrap().clients.push(menu(true));
        route.pointer(&on, (50.0, 50.0));
        assert!(matches!(route.hit, Hit::Client(9, _)));
        route.button(&[], 0x110, true);
        route.pointer(&on, (500.0, 500.0));
        assert!(matches!(route.hit, Hit::Client(9, _)), "held, it keeps the pointer");
        on.0.lock().unwrap().clients[0].drawn = false;
        route.pointer(&on, (50.0, 50.0));
        assert!(!matches!(route.hit, Hit::Client(9, _)));
        assert!(route.grab.is_none() && route.client_held.is_empty());
    }

    /// Marea's box asks for the keyboard on demand: a click gives it to her.
    /// A window taking it after that (the scene focusing the one under the
    /// pointer, a shortcut) ends it: the keys are the window's, not hers, until
    /// the box is clicked again.
    #[test]
    fn a_window_taking_the_keyboard_ends_a_clicked_surfaces_claim() {
        let _drag = layers::tests::DRAGGING.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut route = Route::new(tx);
        let on = crate::screen::screen("A".into(), (1920, 1080), Box::new(Nowhere));
        let mut box_ = menu(true);
        box_.keyboard = 2;
        on.0.lock().unwrap().clients.push(box_);
        let screens = [on.clone()];
        route.pointer(&on, (50.0, 50.0));
        route.button(&screens, 0x110, true);
        route.button(&screens, 0x110, false);
        assert_eq!(route.key_owner(&screens), Some(9), "clicked, the box has the keys");
        layers::keyboard_to_window();
        assert_eq!(route.key_owner(&screens), None, "a window took the keyboard from it");
        route.button(&screens, 0x110, true);
        route.button(&screens, 0x110, false);
        assert_eq!(route.key_owner(&screens), Some(9), "clicked again, it has them again");
    }
}
