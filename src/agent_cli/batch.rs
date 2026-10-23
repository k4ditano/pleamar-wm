//! `agent do PID <<'EOF'`: several steps on one window in one call. A step
//! a line, as the verb would be said without `pleamar-wm agent` and its
//! window (`press @12`, `type @12 -- "ana"`, `wait has --text OK 5s`), with
//! a shell's quoting and `#` comments; in order, until one fails.

use super::*;

/// The verbs a step may be: those of a window.
const VERBS: [&str; 18] = ["tree", "find", "at", "listen", "watch", "press", "focus", "type", "key", "hotkey", "look", "wait", "say", "move", "click", "drag", "scroll", "can"];

/// A script's commands, each its words and the line it starts on. Quoting
/// as a shell's: '...' as it is, "..." with \" \\ \$ \` escaped, \ before
/// any other character outside them; a newline inside quotes is text, and
/// \ before one goes on to the next line. `#` at a word's start is a
/// comment to the end of the line; blank lines are none.
pub(super) fn commands(script: &str) -> R<Vec<(usize, Vec<String>)>> {
    let mut out = Vec::new();
    let (mut words, mut word, mut started) = (Vec::new(), String::new(), false);
    let (mut line, mut first) = (1, 1);
    let mut c = script.chars().peekable();
    let end = |words: &mut Vec<String>, word: &mut String, started: &mut bool| {
        if *started {
            words.push(std::mem::take(word));
            *started = false;
        }
    };
    while let Some(ch) = c.next() {
        match ch {
            '\n' => {
                end(&mut words, &mut word, &mut started);
                if !words.is_empty() {
                    out.push((first, std::mem::take(&mut words)));
                }
                line += 1;
            }
            ' ' | '\t' | '\r' => end(&mut words, &mut word, &mut started),
            '#' if !started => {
                while c.peek().is_some_and(|n| *n != '\n') {
                    c.next();
                }
            }
            '\\' => match c.next() {
                Some('\n') => line += 1,
                Some(n) => {
                    if words.is_empty() && !started {
                        first = line;
                    }
                    word.push(n);
                    started = true;
                }
                None => return Err(usage(format!("line {line}: a \\ at the end, before nothing"))),
            },
            '\'' | '"' => {
                if words.is_empty() && !started {
                    first = line;
                }
                let from = line;
                started = true;
                loop {
                    match c.next() {
                        None => return Err(usage(format!("line {from}: its {ch} is not closed"))),
                        Some(q) if q == ch => break,
                        Some('\\') if ch == '"' => match c.next() {
                            Some(e @ ('"' | '\\' | '$' | '`')) => word.push(e),
                            Some('\n') => line += 1,
                            Some(e) => {
                                word.push('\\');
                                word.push(e);
                            }
                            None => return Err(usage(format!("line {from}: its \" is not closed"))),
                        },
                        Some(q) => {
                            if q == '\n' {
                                line += 1;
                            }
                            word.push(q);
                        }
                    }
                }
            }
            ch => {
                if words.is_empty() && !started {
                    first = line;
                }
                word.push(ch);
                started = true;
            }
        }
    }
    end(&mut words, &mut word, &mut started);
    if !words.is_empty() {
        out.push((first, words));
    }
    Ok(out)
}

/// A word as a step says it: as it is when nothing in it needs quoting.
pub(crate) fn quote(w: &str) -> String {
    let plain = !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "@%+,-./:=_".contains(c));
    if plain {
        w.to_owned()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// A duration in seconds, or what is wrong with it.
pub(crate) fn seconds(s: &str) -> Result<f64, String> {
    duration(s).map_err(|f| f.msg)
}

/// Whether a `type` would read its text from the input, which here is the
/// script.
fn reads_input(words: &[String]) -> bool {
    let mut rest: Vec<&str> = words.iter().map(String::as_str).filter(|w| *w != "--").collect();
    if rest.first().is_some_and(|w| w.starts_with('@')) {
        rest.remove(0);
    }
    rest == ["-"]
}

/// Each step's words for `go`, its window put in after the verb.
fn steps(pid: &str, script: &str) -> R<Vec<(String, Vec<String>)>> {
    Window::parse(pid).ok_or_else(|| usage(format!("{pid}: which window: PID or PID.N, as `pleamar-wm agent windows` names it")))?;
    let cmds = commands(script)?;
    if cmds.is_empty() {
        return Err(usage("do what: one command a line on the input, as `press @12`, `type @12 -- \"ana\"`, `key tab`"));
    }
    cmds.into_iter()
        .map(|(line, words)| {
            let verb = words[0].as_str();
            if !VERBS.contains(&verb) {
                return Err(usage(format!("line {line}: {verb} is not a step: a step is a window's verb ({}), without the window", VERBS.join(", "))));
            }
            if verb == "type" && reads_input(&words[1..]) {
                return Err(usage(format!("line {line}: {verb} - reads the input, which is the script here: say the text itself (quoted)")));
            }
            let said = words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ");
            let mut args = vec![words[0].clone(), pid.to_owned()];
            args.extend_from_slice(&words[1..]);
            Ok((said, args))
        })
        .collect()
}

/// The steps in order, each under `step N: COMMAND`; the first that fails
/// ends it, with its number and its exit code.
pub(super) fn run_steps(pid: &str, script: &str, out: &mut impl Write, mut step: impl FnMut(&[String]) -> R) -> R {
    let all = steps(pid, script)?;
    let n = all.len();
    for (k, (said, args)) in all.iter().enumerate() {
        let _ = writeln!(out, "step {}: {said}", k + 1);
        let _ = out.flush();
        let done = step(args);
        let _ = std::io::stdout().flush();
        if let Err(f) = done {
            let rest = if k + 1 < n { format!(", {} not run", n - k - 1) } else { String::new() };
            return Err(Fail::new(f.code, format!("step {} of {n} failed{rest}: {}", k + 1, f.msg)));
        }
    }
    Ok(())
}

/// `do PID`, the script on the input.
pub(super) fn run(words: &[String]) -> R {
    let pid = match words {
        [p] => p,
        _ => return Err(usage("do PID <<'EOF' ... EOF: the window, and one command a line on the input")),
    };
    let mut script = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut script).map_err(|e| e.to_string())?;
    run_steps(pid, &script, &mut std::io::stdout(), super::go)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_script_read_as_a_shell_would() {
        let c = commands("press @12\n\n  # a comment\ntype @12 -- \"ana  maria\"   # its name\nkey tab\n").unwrap();
        assert_eq!(c, vec![(1, words(&["press", "@12"])), (4, words(&["type", "@12", "--", "ana  maria"])), (5, words(&["key", "tab"]))]);
        // Single quotes as they are; double with their four escapes.
        let c = commands(r#"type -- 'it''s \n' "a \"b\" \\ \$x \n" c\ d e\#f #g"#).unwrap();
        assert_eq!(c[0].1, words(&["type", "--", r"its \n", r#"a "b" \ $x \n"#, "c d", "e#f"]));
        // A word joined of parts, an empty one, and # inside a word.
        assert_eq!(commands("say a'b'\"c\" '' x#y").unwrap()[0].1, words(&["say", "abc", "", "x#y"]));
        // A newline in quotes is text; \ before one goes on.
        let c = commands("type -- \"one\ntwo\"\nkey \\\n  enter\nkey tab").unwrap();
        assert_eq!(c, vec![(1, words(&["type", "--", "one\ntwo"])), (3, words(&["key", "enter"])), (5, words(&["key", "tab"]))]);
        assert_eq!(commands("").unwrap(), vec![]);
        assert_eq!(commands("# only\n\n").unwrap(), vec![]);
        let f = commands("key tab\ntype -- \"open").unwrap_err();
        assert_eq!((f.code, f.msg.as_str()), (fail::USAGE, "line 2: its \" is not closed"));
        assert_eq!(commands("type 'x").unwrap_err().code, fail::USAGE);
        assert_eq!(commands("key \\").unwrap_err().code, fail::USAGE);
    }

    #[test]
    fn quoted_back_as_it_was() {
        for w in ["@12", "ana maria", "it's", "", "--", "a\"b", "#x", "one\ntwo", "$HOME", "500ms"] {
            assert_eq!(commands(&format!("say {}", quote(w))).unwrap()[0].1, words(&["say", w]), "{w}");
        }
        assert_eq!(quote("@12"), "@12");
        assert_eq!(quote("ana maria"), "'ana maria'");
    }

    #[test]
    fn each_step_gets_its_window() {
        let s = steps("4521", "press @12\ntype @12 -- ana\nwait has --text OK 5s").unwrap();
        assert_eq!(s[0], ("press @12".into(), words(&["press", "4521", "@12"])));
        assert_eq!(s[1].1, words(&["type", "4521", "@12", "--", "ana"]));
        assert_eq!(s[2].1, words(&["wait", "4521", "has", "--text", "OK", "5s"]));
        let s = steps("7002.1", "look -o /tmp/x.png").unwrap();
        assert_eq!(s[0].1, words(&["look", "7002.1", "-o", "/tmp/x.png"]));
        for (script, says) in [("type @12 -", "line 1: type -"), ("press @1\ntype -- -", "line 2: type -"), ("run ls", "line 1: run is not a step"), ("windows", "line 1: windows is not a step"), ("do 4521", "is not a step"), ("", "do what")] {
            let f = steps("4521", script).unwrap_err();
            assert_eq!(f.code, fail::USAGE, "{script}");
            assert!(f.msg.contains(says), "{script}: {}", f.msg);
        }
        // `-` as text among others is typed.
        assert!(steps("4521", "type a - b").is_ok());
        assert_eq!(steps("firefox", "key tab").unwrap_err().code, fail::USAGE);
        assert_eq!(steps("7002:7044", "key tab").unwrap_err().code, fail::USAGE);
    }

    #[test]
    fn the_first_that_fails_ends_it_with_its_code() {
        let mut ran = Vec::new();
        let mut out = Vec::new();
        let f = run_steps("4521", "press @12\nkey tab  # next\npress @99\nkey enter", &mut out, |a| {
            ran.push(a.join(" "));
            match a[2].as_str() {
                "@99" => Err(Fail::new(fail::GONE, "gone: @99 is no longer in 4521")),
                _ => Ok(()),
            }
        })
        .unwrap_err();
        assert_eq!(ran, ["press 4521 @12", "key 4521 tab", "press 4521 @99"]);
        assert_eq!(f, Fail::new(fail::GONE, "step 3 of 4 failed, 1 not run: gone: @99 is no longer in 4521"));
        assert_eq!(String::from_utf8(out).unwrap(), "step 1: press @12\nstep 2: key tab\nstep 3: press @99\n");
        // Stopped: the next step is refused and nothing after it runs.
        let mut n = 0;
        let f = run_steps("4521", "key tab\nkey tab\nkey tab", &mut Vec::new(), |_| {
            n += 1;
            if n == 2 { Err(fail::stopped()) } else { Ok(()) }
        })
        .unwrap_err();
        assert_eq!((n, f.code), (2, fail::STOPPED));
        assert!(f.msg.starts_with("step 2 of 3 failed, 1 not run: stopped:"));
        // The last one: nothing left to say.
        let f = run_steps("4521", "key tab", &mut Vec::new(), |_| Err(Fail::new(fail::TIMEOUT, "timeout"))).unwrap_err();
        assert_eq!(f, Fail::new(fail::TIMEOUT, "step 1 of 1 failed: timeout"));
        // A script that does not read runs nothing.
        let f = run_steps("4521", "key tab\nkey 'enter", &mut Vec::new(), |_| unreachable!()).unwrap_err();
        assert_eq!(f.code, fail::USAGE);
        assert!(run_steps("4521", "key tab\nkey enter", &mut Vec::new(), |_| Ok(())).is_ok());
    }
}
