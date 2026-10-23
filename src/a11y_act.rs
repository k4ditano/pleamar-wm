//! Acting on an element of a window's tree (`e`, `S` on the agent's socket),
//! by the number `A` gave it (`a11y_tree.rs`): its box read again, scrolled
//! into view, its actions done, its focus taken, its text set. Each answers
//! `err gone` once the element is no longer there.

use crate::a11y;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const ACTION: &str = "org.a11y.atspi.Action";
const COMPONENT: &str = "org.a11y.atspi.Component";
const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";
/// `Component.GetExtents` in the window's coordinates.
const WINDOW: u32 = 1;
/// `Component.ScrollTo`: wherever it is seen whole.
const ANYWHERE: u32 = 6;
/// AT-SPI's states, by their bit: what lives no longer, what is shown.
const DEFUNCT: u32 = 6;
const SHOWING: u32 = 25;
/// How long one call to a program is waited for.
const CALL: Duration = Duration::from_secs(1);

/// The user's Stops, counted: what was begun before one is not done.
static STOPS: AtomicU64 = AtomicU64::new(0);

/// The user stopped the agent.
pub fn stop() {
    STOPS.fetch_add(1, Ordering::Relaxed);
}

/// The Stops so far, for `stopped_since`.
pub fn stops() -> u64 {
    STOPS.load(Ordering::Relaxed)
}

/// Whether the user stopped the agent after `stops` said that.
pub fn stopped_since(stops: u64) -> bool {
    STOPS.load(Ordering::Relaxed) != stops
}

/// What is done to an element.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Its box now, and whether it is shown.
    Box,
    /// Its default action (the first).
    Press,
    Focus,
    /// Into view.
    Scroll,
    Action(i32),
    /// Its whole text.
    Set(String),
}

impl Act {
    /// `box`, `press`, `focus`, `scroll`, `action N`.
    pub fn parse(words: &[&str]) -> Option<Act> {
        match words {
            ["box"] => Some(Act::Box),
            ["press"] => Some(Act::Press),
            ["focus"] => Some(Act::Focus),
            ["scroll"] => Some(Act::Scroll),
            ["action", n] => n.parse().ok().filter(|n| *n >= 0).map(Act::Action),
            _ => None,
        }
    }
}

/// A call to the element, its answer read as `T`. A program that no longer
/// has the object, or no longer is, has it `gone`; one that does not do
/// that to it, `missing`.
fn ask<T, B>(k: &(String, String), interface: &str, member: &str, body: &B, missing: &'static str) -> Result<T, &'static str>
where
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
{
    match a11y::call_blocking(&k.0, &k.1, interface, member, body, CALL) {
        Ok(reply) => reply.body().deserialize::<T>().map_err(|_| missing),
        Err(a11y::Error::Off) => Err("no-bus"),
        Err(a11y::Error::Gone) => Err("gone"),
        Err(a11y::Error::Timeout) => Err("timeout"),
        Err(a11y::Error::Failed(name, _)) if name.ends_with(".UnknownObject") => Err("gone"),
        Err(a11y::Error::Failed(..)) => Err(missing),
    }
}

/// What a yes-or-no answer means: done, or not done (`missing`).
fn done(said: Result<(bool,), &'static str>, missing: &'static str) -> Result<String, &'static str> {
    match said? {
        (true,) => Ok("ok".to_owned()),
        (false,) => Err(missing),
    }
}

/// That element of the window, as `A` numbered it: `ok`, or for `Box`
/// `box X Y W H shown|hidden` (`- - - -` without one). `stops`: the user's
/// Stops when it was asked; one since refuses it right before it is done.
pub fn act(pid: u32, slot: usize, id: u32, act: Act, stops: u64) -> Result<String, &'static str> {
    let k = crate::a11y_tree::key_of(pid, slot, id).ok_or("gone")?;
    let (states,) = ask::<(Vec<u32>,), _>(&k, ACCESSIBLE, "GetState", &(), "gone")?;
    let has = |bit: u32| states.get(bit as usize / 32).is_some_and(|w| w & (1 << (bit % 32)) != 0);
    if has(DEFUNCT) {
        return Err("gone");
    }
    if act != Act::Box && stopped_since(stops) {
        return Err("stopped-by-user");
    }
    match act {
        Act::Box => {
            let shown = if has(SHOWING) { "shown" } else { "hidden" };
            Ok(match ask::<((i32, i32, i32, i32),), _>(&k, COMPONENT, "GetExtents", &(WINDOW,), "no-box") {
                Ok(((x, y, w, h),)) => crate::a11y_tree::sane([x, y, w, h]).map_or(format!("box - - - - {shown}"), |[x, y, w, h]| format!("box {x} {y} {w} {h} {shown}")),
                Err("no-box") => format!("box - - - - {shown}"),
                Err(e) => return Err(e),
            })
        }
        Act::Press => done(ask(&k, ACTION, "DoAction", &(0i32,), "no-action"), "no-action"),
        Act::Action(n) => done(ask(&k, ACTION, "DoAction", &(n,), "no-action"), "no-action"),
        Act::Focus => done(ask(&k, COMPONENT, "GrabFocus", &(), "no-focus"), "no-focus"),
        Act::Scroll => done(ask(&k, COMPONENT, "ScrollTo", &(ANYWHERE,), "no-scroll"), "no-scroll"),
        Act::Set(text) => done(ask(&k, EDITABLE_TEXT, "SetTextContents", &(text.as_str(),), "not-editable"), "not-editable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_e_takes() {
        assert_eq!(Act::parse(&["press"]), Some(Act::Press));
        assert_eq!(Act::parse(&["action", "2"]), Some(Act::Action(2)));
        assert_eq!(Act::parse(&["action", "-1"]), None);
        assert_eq!(Act::parse(&["action"]), None);
        assert_eq!(Act::parse(&["box", "x"]), None);
    }

    #[test]
    fn an_element_never_given_is_gone() {
        assert_eq!(act(1, 0, 7, Act::Press, stops()), Err("gone"));
    }

    #[test]
    fn a_stop_after_the_ask_is_seen() {
        let before = stops();
        assert!(!stopped_since(before));
        stop();
        assert!(stopped_since(before));
        assert!(!stopped_since(stops()));
    }
}
