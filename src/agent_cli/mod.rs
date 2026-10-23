//! `pleamar-wm agent …`: the agent's hands from a shell, for an AI agent (or
//! anyone) that wants to use the desktop without writing the protocol. It
//! speaks `cua-inject v1` (see `agent.rs`) to the session's socket.
//!
//! A window is named by its process (`pleamar-wm agent windows` lists them).
//! Its coordinates are the pixels of `pleamar-wm agent look PID`: what is seen
//! in that picture at (x, y) is where `click PID x y` lands.
//!
//! One grammar for every kind of window (target.rs), what each does
//! (kind.rs), one record format (record.rs), and an exit code for each way
//! a command fails (fail.rs).

use crate::config::Pace;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

mod act;
mod batch;
mod conn;
mod fail;
mod help;
mod kind;
mod look;
mod pointer;
mod record;
mod run;
mod scene;
mod target;
mod terminal;
mod tree;
mod versions;
mod wait;
mod windows;

use conn::*;
use fail::{answered, scene_said, usage, Fail, Of, R};
use help::*;
use kind::Kinds;
use record::*;
use scene::*;
use target::*;
use terminal::*;
use versions::*;
pub(crate) use conn::{tell, Line};
pub(crate) use batch::{quote, seconds};
pub(crate) use windows::monitors;

pub fn run(args: &[String]) -> i32 {
    // Read by `head` or `sed -n 1,9p`: it ends quietly when they stop reading.
    // SAFETY: before any thread, the default action back.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    match go(args) {
        Ok(()) => fail::DONE,
        Err(f) => {
            eprintln!("{}", f.msg);
            f.code
        }
    }
}

fn go(args: &[String]) -> R {
    let Some(what) = args.first() else {
        println!("{HELP}");
        return Ok(());
    };
    let rest = &args[1..];
    // `VERB --help` too: never a verb's first word (a command to open).
    let flag = |w: &str| matches!(w, "--help" | "-h");
    if what == "help" || flag(what) || rest.first().is_some_and(|w| flag(w)) {
        println!("{HELP}");
        return Ok(());
    }
    match what.as_str() {
        "windows" => windows::windows()?,
        "can" => kind::can(&parse(rest, &SHELL)?)?,
        "layers" => windows::layers()?,
        "hide" | "show" => windows::hide(what, rest)?,
        "monitors" => windows::print_monitors()?,
        "tree" => tree::tree(rest)?,
        "find" => tree::find(rest)?,
        "at" => tree::at(rest)?,
        "listen" | "watch" => tree::listen(rest)?,
        "press" => act::press(rest)?,
        "focus" => act::focus(rest)?,
        "type" => act::type_(rest)?,
        "key" => act::key(rest)?,
        "hotkey" => act::hotkey(rest)?,
        "look" => look::look(rest)?,
        "run" => run::run(rest)?,
        "wait" => wait::wait(rest)?,
        "do" => batch::run(rest)?,
        "mcp" => crate::agent_mcp::run(),
        "setup" => crate::agent_mcp::setup(rest)?,
        "say" => say(&parse(rest, &Spec { raw: true, ..PLAIN })?)?,
        "move" | "click" | "drag" | "scroll" => pointer::go(what, rest)?,
        "open" => {
            let mut rest: Vec<String> = rest.to_vec();
            let mut monitor = -1i64;
            if rest.first().map(String::as_str) == Some("--monitor") {
                monitor = rest.get(1).and_then(|m| m.parse().ok()).ok_or_else(|| usage("which monitor: its number (pleamar-wm agent monitors)"))?;
                rest.drain(..2);
            }
            if rest.first().map(String::as_str) == Some("--") {
                rest.remove(0);
            }
            let command = rest.join(" ");
            if command.trim().is_empty() {
                return Err(usage("what to open: pleamar-wm agent open firefox"));
            }
            let reply = Hands::open()?.say(&format!("L {monitor} {}", hex(&command)))?;
            match reply.split_whitespace().collect::<Vec<&str>>()[..] {
                ["opened", pid, screen] => println!("opened on monitor {screen} (process {pid}): its window opens there, without the user's keyboard. Find it with `pleamar-wm agent windows`."),
                _ => return Err(answered(&reply, Of::Window(&command))),
            }
        }
        "send" => {
            let a = parse(rest, &PLAIN)?;
            let monitor = a.word(0).filter(|m| m.parse::<usize>().is_ok()).ok_or_else(|| usage("which monitor: its number (pleamar-wm agent monitors)"))?;
            Hands::open()?.act(&format!("v {} {monitor}", a.win()?.pid))?;
        }
        "done" => Hands::open()?.act("x")?,
        "stop" => Hands::open()?.act("s")?,
        "pace" => match rest.first() {
            None => println!("{}", Hands::open()?.pace.name()),
            Some(p) => {
                let pace = Pace::parse(p).ok_or_else(|| usage(format!("a pace is default, fast or instant: {p} is not one")))?;
                Hands::open()?.act(&format!("T {}", pace.name()))?;
            }
        },
        "raw" => {
            let mut h = Hands::open()?;
            for line in rest {
                println!("{}", h.say(line)?);
            }
        }
        // Words of this branch that the grammar took over.
        "read" => return Err(usage("read is gone: tree PID (a terminal's text too), find PID --text T, at PID X Y, tree PID --focused")),
        other => return Err(usage(format!("I don't know {other:?}: pleamar-wm agent help"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_pace_moves_as_its_table_says() {
        // As a hand: from 260 to 620 ms, 8 to 24 steps, and a moment still.
        assert_eq!(glide(Pace::Default, 10.0), Some((260.0, 8, 70)));
        assert_eq!(glide(Pace::Default, 2000.0), Some((620.0, 24, 70)));
        // Quicker, and no stop.
        assert_eq!(glide(Pace::Fast, 10.0), Some((80.0, 4, 0)));
        assert_eq!(glide(Pace::Fast, 2000.0), Some((150.0, 6, 0)));
        for far in [5.0, 300.0, 900.0] {
            let (ms, steps, _) = glide(Pace::Fast, far).unwrap();
            assert!((80.0..=150.0).contains(&ms) && (4..=6).contains(&steps));
        }
        assert_eq!(glide(Pace::Instant, 500.0), None);
        assert_eq!(strokes(Pace::Default), (24, 60));
        assert_eq!(strokes(Pace::Fast), (8, 20));
    }

    #[test]
    fn the_old_words_of_this_branch_are_refused_with_the_new() {
        let go = |w: &[&str]| go(&w.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let f = go(&["read", "4521", "focus"]).unwrap_err();
        assert_eq!(f.code, fail::USAGE);
        assert!(f.msg.contains("tree PID --focused"));
        // Before any socket: the grammar.
        assert_eq!(go(&["tree"]).unwrap_err().code, fail::USAGE);
        assert_eq!(go(&["tree", "4521", "all"]).unwrap_err().code, fail::USAGE);
        assert_eq!(go(&["press", "4521", "--shell", "7"]).unwrap_err().code, fail::USAGE);
        assert_eq!(go(&["find", "4521", "Save"]).unwrap_err().code, fail::USAGE);
        assert_eq!(go(&["run", "7044:x", "ls"]).unwrap_err().code, fail::USAGE);
        assert_eq!(go(&["nonsense"]).unwrap_err().code, fail::USAGE);
    }
}
