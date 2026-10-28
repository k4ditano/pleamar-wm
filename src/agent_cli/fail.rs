//! How a command fails: what happened, and the exit code that says it.

/// Done.
pub(super) const DONE: i32 = 0;
/// Any other error: the message says it.
pub(super) const OTHER: i32 = 1;
/// A private window, or the screen locked: the user is asked.
pub(super) const PRIVATE: i32 = 2;
/// Not for this window: it does something else.
pub(super) const NOT_HERE: i32 = 3;
/// The element is gone: the window is read again.
pub(super) const GONE: i32 = 4;
pub(super) const TIMEOUT: i32 = 5;
/// Several match: one of them is named.
pub(super) const AMBIGUOUS: i32 = 6;
/// The window is gone.
pub(super) const WINDOW_GONE: i32 = 7;
/// The user stopped the agent.
pub(super) const STOPPED: i32 = 8;
/// Bad arguments: the message gives the right form.
pub(super) const USAGE: i32 = 64;
/// No session to speak to.
pub(super) const NO_SESSION: i32 = 69;

#[derive(Debug, PartialEq)]
pub(crate) struct Fail {
    pub(crate) code: i32,
    pub(crate) msg: String,
}

pub(super) type R<T = ()> = Result<T, Fail>;

impl Fail {
    pub(super) fn new(code: i32, msg: impl Into<String>) -> Fail {
        Fail { code, msg: msg.into() }
    }
}

impl From<String> for Fail {
    fn from(msg: String) -> Fail {
        Fail::new(OTHER, msg)
    }
}

impl From<&str> for Fail {
    fn from(msg: &str) -> Fail {
        Fail::new(OTHER, msg)
    }
}

pub(super) fn usage(msg: impl Into<String>) -> Fail {
    Fail::new(USAGE, msg)
}

pub(super) fn stopped() -> Fail {
    Fail::new(STOPPED, "stopped: the user stopped the agent: stop here and tell them where you left it (`pleamar-wm agent done` lets the hands be used again)")
}

/// What an `err` was about: a window, or one of its elements.
pub(super) enum Of<'a> {
    Window(&'a str),
    Element(&'a str, &'a str),
}

/// An `err REASON` of the window manager's, as its exit code and what it
/// means: every one is told here.
pub(super) fn answered(reason: &str, of: Of) -> Fail {
    let reason = reason.strip_prefix("err ").unwrap_or(reason).trim();
    let (w, e) = match of {
        Of::Window(w) => (w, None),
        Of::Element(w, e) => (w, Some(e)),
    };
    match reason {
        "private" => Fail::new(PRIVATE, format!("private: {w} is private (a `private` rule in session.conf), or the screen is locked: ask the user, never another way around it")),
        "locked" => Fail::new(PRIVATE, "locked: the screen is locked: ask the user"),
        "stopped-by-user" => stopped(),
        "refused-by-rule" => Fail::new(PRIVATE, format!("refused: the person's rules (gate.rules, `hands`) do not let your hands into {w}: ask them, never another way around it")),
        "taken-by-person" => Fail::new(PRIVATE, format!("taken: the person took {w} over from you (they clicked or typed in it): wait until they give it back, or ask")),
        "unknown-root-pid" | "target-not-found" | "unknown-pid" | "unknown-app-id" => Fail::new(WINDOW_GONE, format!("window gone: {w} is not open (`pleamar-wm agent windows` names them)")),
        "gone" | "unknown-id" => match e {
            Some(e) => Fail::new(GONE, format!("gone: {e} is no longer in {w} (or its tree never gave it): read it again with `tree {w}`")),
            None => Fail::new(WINDOW_GONE, format!("window gone: {w} closed")),
        },
        "timeout" => Fail::new(TIMEOUT, "timeout: it did not happen in time"),
        r if r.starts_with("ambiguous") => Fail::new(AMBIGUOUS, format!("ambiguous: {w} names more than one window: PID.N, as `pleamar-wm agent windows` names them")),
        "no-tree" => Fail::new(NOT_HERE, format!("{w} has no tree: its program publishes nothing for screen readers (or started before the session's bus): it does look")),
        "no-frame" => Fail::new(NOT_HERE, format!("{w} has no tree: its program publishes one, but none of its frames is this window: it does look")),
        "no-bus" => Fail::new(OTHER, "no accessibility bus in this session (an older pleamar-wm, or `agent off`)"),
        "no-box" => Fail::new(OTHER, format!("no box: {} is not in the window, and its program did not scroll it into view", e.unwrap_or(w))),
        "no-keyboard-resource" => Fail::new(OTHER, format!("no keyboard: keys cannot reach {w}, its program took no keyboard: use the pointer")),
        "no-pointer-resource" => Fail::new(OTHER, format!("no pointer: the pointer cannot reach {w}, its program took none here")),
        "no-picture" =>Fail::new(OTHER, format!("no picture: {w} has not drawn anything to look at yet")),
        "not-editable" => Fail::new(OTHER, format!("not editable: {} takes no text: focus it and type", e.unwrap_or(w))),
        "unknown-state" => usage("a state is focused, focusable, checked, pressed, selected, expanded, collapsed, editable, disabled, enabled, required, invalid or busy"),
        "unknown-key" | "unknown-hotkey" => usage("no key called that: enter tab escape backspace space up down left right delete home end pageup pagedown f1..f12, a letter after modifiers"),
        "bad-args" => usage(format!("the window manager did not take that: `pleamar-wm agent help`")),
        "unknown-command" => Fail::new(OTHER, "this session's pleamar-wm does not know that: an older one (log out and in again after an update)"),
        r => Fail::new(OTHER, r.to_owned()),
    }
}

/// A scene's answer that starts with `?`: what it could not do.
pub(super) fn scene_said(answer: &str) -> Fail {
    let said = answer.trim().strip_prefix("? ").unwrap_or(answer.trim());
    if said.starts_with("not after ") {
        Fail::new(TIMEOUT, format!("timeout: {said}"))
    } else if said.ends_with(" is not there now") || said.starts_with("there is nothing called ") {
        Fail::new(GONE, format!("gone: {said}: read it again with `tree`"))
    } else {
        Fail::new(OTHER, said.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_err_has_its_code() {
        let w = |r: &str| answered(r, Of::Window("4521")).code;
        let e = |r: &str| answered(r, Of::Element("4521", "@24")).code;
        assert_eq!(w("err private"), PRIVATE);
        assert_eq!(w("err locked"), PRIVATE);
        assert_eq!(w("err stopped-by-user"), STOPPED);
        assert_eq!(w("err unknown-root-pid"), WINDOW_GONE);
        assert_eq!(w("err target-not-found"), WINDOW_GONE);
        assert_eq!(w("err gone"), WINDOW_GONE);
        assert_eq!(e("err gone"), GONE);
        assert_eq!(e("err unknown-id"), GONE);
        assert_eq!(w("err timeout"), TIMEOUT);
        assert_eq!(w("err ambiguous-root-pid"), AMBIGUOUS);
        assert_eq!(w("err no-tree"), NOT_HERE);
        assert_eq!(w("err bad-args"), USAGE);
        assert_eq!(w("err unknown-state"), USAGE);
        assert_eq!(w("err no-picture"), OTHER);
        assert_eq!(w("err something-new"), OTHER);
        // What happened first, with no prefix.
        assert!(answered("err private", Of::Window("4521")).msg.starts_with("private: 4521"));
        assert!(answered("err gone", Of::Element("4521", "@24")).msg.starts_with("gone: @24"));
        assert_eq!(scene_said("? not after 2.0 s: status is \"\"\n").code, TIMEOUT);
        assert_eq!(scene_said("? save is not there now").code, GONE);
        assert_eq!(scene_said("? there is nothing called 'x' on screen: `describe` says what there is").code, GONE);
        assert_eq!(scene_said("? save is inactive").code, OTHER);
        assert_eq!(Fail::from("x").code, OTHER);
        assert_eq!(DONE, 0);
        assert_eq!(NO_SESSION, 69);
    }
}
