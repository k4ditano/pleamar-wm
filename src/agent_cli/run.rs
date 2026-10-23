//! `run PID[:SHELL] "CMD"|- [DURATION] [--full]`: a command in a terminal's
//! shell, and what it printed once the shell waits again.

use super::*;

/// The longest a command is waited for.
const LONGEST: f64 = 600.0;

/// The lines a run answers, the last ones; `--full` all.
const TAIL: usize = 200;

const RUN: Spec = Spec { flags: &["--full"], ..SHELL };

pub(super) fn run(words: &[String]) -> R {
    let a = parse(words, &RUN)?;
    let w = a.win()?;
    let (command, secs) = match &a.words[..] {
        [c] => (c.clone(), 30.0),
        [c, d] => (c.clone(), Some(duration(d)?).filter(|s| *s > 0.0 && *s <= LONGEST).ok_or_else(|| usage(format!("{d}: a duration up to 10 minutes")))?),
        _ => return Err(usage("run PID[:SHELL] \"COMMAND\" [DURATION] [--full], or run PID - [DURATION] with the command on the input (one quoted word)")),
    };
    let command = typed(&[command])?;
    if command.trim().is_empty() {
        return Err(usage("run what: run PID \"ls -l\""));
    }
    let k = Kinds::of(w)?;
    k.open()?;
    Term::of(&k, w, "run", true)?.run(&command, secs, a.has("--full"))
}

/// What a command printed: the lines after the mark it was typed at, but
/// for the command as the shell showed it and the prompt that came back.
#[derive(Debug, PartialEq)]
pub(super) struct Output {
    /// The newest line's number when it was read.
    pub(super) mark: u64,
    pub(super) lines: Vec<String>,
}

impl Output {
    /// From a `lines N M` answer read after typing `command` at the line M,
    /// which said `prompt` then. `back`: the shell waits again, its cursor on
    /// the line `cursor` (a layer that says it).
    pub(super) fn of(reply: &str, command: &str, prompt: &str, back: bool, cursor: Option<u64>) -> Output {
        let mut body = reply.lines();
        let mark = body.next().and_then(|l| l.strip_prefix("mark ")).and_then(|m| m.parse().ok()).unwrap_or(0);
        let mut lines: Vec<String> = body.map(str::to_owned).collect();
        // Written over (a redraw, `clear`): the screen, where it was typed last.
        let screen = lines.first().is_some_and(|l| l == "screen");
        if screen {
            lines.remove(0);
        }
        if let Some((_, end)) = echoed(&lines, command, screen) {
            lines.drain(..=end);
        }
        if back {
            if let Some(last) = lines.pop() {
                let prompt = prompt.trim_end();
                let head = last.trim_end().strip_suffix(prompt).filter(|h| !prompt.is_empty() && !h.trim().is_empty());
                match (cursor == Some(mark), head) {
                    // Printed without a newline at its end: the prompt after it.
                    (true, Some(h)) => lines.push(h.to_owned()),
                    (true, None) => {}
                    (false, _) if last.trim_end() == prompt => {}
                    (false, _) => lines.push(last),
                }
            }
        }
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        Output { mark, lines }
    }

    /// `mark M`, how many lines came before the last 200 (if more), and
    /// those; `full`: all of them. `name` and `from` say where they are.
    pub(super) fn answer(&self, name: &str, from: u64, full: bool) -> String {
        let mut s = format!("mark {}\n", self.mark);
        let skip = if full { 0 } else { self.lines.len().saturating_sub(TAIL) };
        if skip > 0 {
            s.push_str(&format!("... {skip} lines before these: `tree {name} --since {from}` gives them, `find {name} --text T` searches them\n"));
        }
        for l in &self.lines[skip..] {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}

/// A run's timeout: it still runs, and how to go on from the mark it answered.
pub(super) fn late(name: &str, secs: f64, mark: u64) -> String {
    format!("timeout: still running after {secs} s (what it printed so far is above): `wait {name} prompt [DURATION]` waits for the rest, `tree {name} --since {mark}` reads what came after")
}

/// Where the command shows in these lines, as the first and last line it
/// takes: each of its lines in order, spaces aside (a long one wraps, a
/// shell puts `> ` before the next), within the lines it could take. From
/// the first two lines; on a `screen`, the last place it is.
fn echoed(lines: &[String], command: &str, screen: bool) -> Option<(usize, usize)> {
    let squeeze = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let parts: Vec<String> = command.lines().map(squeeze).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let (mut joined, mut ends) = (String::new(), Vec::new());
    for l in lines {
        joined.push_str(&squeeze(l));
        ends.push(joined.len());
    }
    let line_of = |at: usize| ends.iter().position(|&e| at < e);
    let room = command.lines().count() + command.len() / 10 + 2;
    let at_line = |s: usize| {
        let mut at = if s == 0 { 0 } else { ends[s - 1] };
        for (k, p) in parts.iter().enumerate() {
            let found = at + joined[at..].find(p.as_str())?;
            if k == 0 && line_of(found) != Some(s) {
                return None;
            }
            at = found + p.len();
        }
        line_of(at - 1).filter(|e| *e < s + room).map(|e| (s, e))
    };
    if screen {
        (0..lines.len()).rev().find_map(at_line)
    } else {
        (0..lines.len().min(2)).find_map(at_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(reply: &str, command: &str, back: bool, cursor: Option<u64>) -> Vec<String> {
        Output::of(reply, command, "$", back, cursor).lines
    }

    #[test]
    fn the_command_and_the_prompt_are_not_its_output() {
        assert_eq!(out("mark 12\n$ echo hi\nhi\n$\n", "echo hi", true, Some(12)), ["hi"]);
        // A layer that does not say where its cursor is: the same prompt.
        assert_eq!(out("mark 12\n$ echo hi\nhi\n$\n", "echo hi", true, None), ["hi"]);
        // Nothing printed.
        assert!(out("mark 11\n$ true\n$\n", "true", true, Some(11)).is_empty());
        // A blank line before the prompt (a prompt that starts with one) goes too.
        assert_eq!(out("mark 13\n$ ls\na\n\n$\n", "ls", true, Some(13)), ["a"]);
    }

    #[test]
    fn a_prompt_that_changed_is_told_by_the_cursor() {
        let reply = "mark 12\n[0] $ false\n[1] $\n";
        let of = |cursor| Output::of(reply, "false", "[0] $", true, cursor).lines;
        assert!(of(Some(12)).is_empty());
        // Without it, it cannot be told from output: kept.
        assert_eq!(of(None), ["[1] $"]);
        // A line that ends without a newline: the prompt after it on its line.
        assert_eq!(out("mark 11\n$ printf foo\nfoo$\n", "printf foo", true, Some(11)), ["foo"]);
        // The cursor below the last line (an empty prompt): the last is output.
        assert_eq!(out("mark 11\n$ echo hi\nhi\n", "echo hi", true, Some(12)), ["hi"]);
    }

    #[test]
    fn a_command_wrapped_or_of_several_lines_is_told_apart() {
        // Wrapped at the terminal's width, at a space.
        assert_eq!(out("mark 13\n$ echo aaaa\nbbbb cc\naaaa bbbb cc\n$\n", "echo aaaa bbbb cc", true, Some(13)), ["aaaa bbbb cc"]);
        // From the input, quotes inside, with the shell's `> ` between.
        let command = "echo \"it's 'quoted'\"\necho second";
        assert_eq!(out("mark 14\n$ echo \"it's 'quoted'\"\n> echo second\nit's 'quoted'\nsecond\n$\n", command, true, Some(14)), ["it's 'quoted'", "second"]);
        // A line before it again, the prompt's line not written on.
        assert_eq!(out("mark 13\nold\n$ ls\na\n$\n", "ls", true, Some(13)), ["a"]);
        // Not where it was typed: nothing is taken for it.
        assert_eq!(out("mark 13\nx\ny\nls\n$\n", "ls", true, Some(13)), ["x", "y", "ls"]);
    }

    #[test]
    fn on_a_screen_written_over_it_starts_where_it_was_typed_last() {
        let reply = "mark 30\nscreen\nold\n$ ls\nold out\n$ ls\nnew out\n$\n";
        assert_eq!(out(reply, "ls", true, Some(30)), ["new out"]);
        // Its output says what it was: the last line it starts on.
        assert_eq!(out("mark 30\nscreen\n$ echo ls\nls\n$\n", "echo ls", true, Some(30)), ["ls"]);
    }

    #[test]
    fn its_last_lines_after_how_many_came_before() {
        let lines: String = (1..=1000).map(|k| format!("{k}\n")).collect();
        let o = Output::of(&format!("mark 1041\n$ seq 1000\n{lines}$\n"), "seq 1000", "$", true, Some(1041));
        assert_eq!(o.lines.len(), 1000);
        let a = o.answer("7002:7044", 40, false);
        let a: Vec<&str> = a.lines().collect();
        assert_eq!(a.len(), 2 + 200);
        assert_eq!(a[0], "mark 1041");
        assert_eq!(a[1], "... 800 lines before these: `tree 7002:7044 --since 40` gives them, `find 7002:7044 --text T` searches them");
        assert_eq!((a[2], a[201]), ("801", "1000"));
        let full = o.answer("7002", 40, true);
        assert_eq!(full.lines().count(), 1001);
        assert!(!full.contains("before these"));
        // 200 or fewer: no count.
        let o = Output { mark: 5, lines: vec!["a".into(); 200] };
        assert_eq!(o.answer("7002", 4, false).lines().count(), 201);
        assert_eq!(Output { mark: 5, lines: vec![] }.answer("7002", 4, false), "mark 5\n");
    }

    #[test]
    fn a_timeout_answers_what_came_so_far_and_how_to_go_on() {
        // Still running: its last line is output, not a prompt.
        let o = Output::of("mark 12\n$ sleep 9; seq 2\n1\n2\n", "sleep 9; seq 2", "$", false, None);
        assert_eq!(o.lines, ["1", "2"]);
        assert_eq!(o.answer("7002", 10, false), "mark 12\n1\n2\n");
        let m = late("7002:7044", 30.0, 12);
        assert!(m.starts_with("timeout: still running after 30 s"), "{m}");
        assert!(m.contains("`wait 7002:7044 prompt [DURATION]`") && m.contains("`tree 7002:7044 --since 12`"), "{m}");
    }

    #[test]
    fn its_words() {
        let w = |w: &[&str]| run(&w.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap_err().code;
        assert_eq!(w(&["7002"]), fail::USAGE);
        assert_eq!(w(&["7002", "ls", "11m"]), fail::USAGE);
        assert_eq!(w(&["7002", "ls", "5s", "more"]), fail::USAGE);
        assert_eq!(w(&["7002", "  "]), fail::USAGE);
        assert_eq!(w(&["7002", "ls", "--fast"]), fail::USAGE);
    }
}
