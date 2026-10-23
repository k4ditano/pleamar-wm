//! Terminals, read, followed and typed into through their layer.

use super::*;

/// A window whose process is a terminal with pleamar-wm's layer (term.rs):
/// its text, history included, is asked of the layer, not seen in pictures.
pub(super) struct Term {
    pub(super) name: String,
    pub(super) pid: String,
    pub(super) socket: String,
}

/// What a terminal's wait is for.
pub(super) enum Until {
    /// A text in what came after the mark; and the screen's lines that
    /// said it when the wait began (a screen written over is not news).
    Has(String, Vec<String>),
    /// No line of the screen says it.
    Gone(String),
    Changed,
    Settled(u64),
    Prompt,
}

/// How a wait ended.
pub(super) enum Ended {
    Held,
    Late,
    Stopped,
}

impl Term {
    /// That window's terminal, once the window manager has let the agent at
    /// it (`i`): it says no for a private window or a locked session, and
    /// for typing while stopped. A process with several shells (tabs, a
    /// terminal that serves many windows) does not say which is the
    /// window's: refused, with the shells, unless one is named (`PID:SHELL`).
    pub(super) fn of(k: &Kinds, w: &Window, verb: &str, types: bool) -> R<Term> {
        let Some((id, _)) = k.shells.first() else { return Err(k.refuse(verb)) };
        Term::allowed(&w.pid, types)?;
        if k.shells.len() > 1 {
            let list: Vec<String> = k
                .shells
                .iter()
                .map(|(id, i)| {
                    let g = |k: &str| i.get(k).map_or("", String::as_str);
                    format!("  {}:{id}  {}  {}  {:?}", w.pid, g("cwd"), g("command"), g("title"))
                })
                .collect();
            return Err(Fail::new(fail::AMBIGUOUS, format!("ambiguous: {}'s terminal has {} shells (tabs, or windows of one terminal) and which is this window's is not known: name one\n{}", w.pid, k.shells.len(), list.join("\n"))));
        }
        Ok(Term { name: k.name.clone(), pid: w.pid.clone(), socket: format!("{}/{id}.sock", crate::term::socket_dir()) })
    }

    /// While the agent is stopped: how long ago the user stopped it, in ms.
    pub(super) fn allowed(pid: &str, types: bool) -> R<Option<u64>> {
        let line = format!("i {pid}{}", if types { " w" } else { "" });
        match Hands::open()?.say(&line)?.as_str() {
            "ok" => Ok(None),
            r if r.starts_with("ok stopped ") => Ok(r["ok stopped ".len()..].parse().ok()),
            r => Err(answered(r, Of::Window(pid))),
        }
    }

    pub(super) fn ask(&self, question: &str) -> R<String> {
        match crate::term::ask(&self.socket, question)? {
            r if r.starts_with("error: ") => Err(r["error: ".len()..].trim_end().into()),
            r => Ok(r),
        }
    }

    /// The number of its newest line (`mark` in a read), and what it says.
    pub(super) fn mark(&self) -> R<(u64, String)> {
        let reply = self.ask("lines 1")?;
        let mut lines = reply.lines();
        let mark = lines.next().and_then(|l| l.strip_prefix("mark ")).and_then(|m| m.parse().ok()).ok_or(reply.clone())?;
        Ok((mark, lines.next().unwrap_or_default().to_owned()))
    }

    /// `tree PID`: `mark M` and its screen's lines; `--history` all it
    /// keeps; `--since M` what came after line M. Its mark.
    pub(super) fn tree(&self, history: bool, since: Option<u64>) -> R<u64> {
        let n = if history || since.is_some() {
            crate::term::HISTORY
        } else {
            let info = crate::term::info(&self.socket)?;
            info.get("size").and_then(|s| s.split_once('x')).and_then(|(_, r)| r.parse().ok()).unwrap_or(50)
        };
        let reply = self.ask(&format!("lines {n}{}", since.map_or(String::new(), |m| format!(" {m}"))))?;
        print!("{reply}");
        reply.lines().next().and_then(|l| l.strip_prefix("mark ")).and_then(|m| m.parse().ok()).ok_or_else(|| Fail::from(reply.clone()))
    }

    /// `find PID --text T`: the lines it keeps that say T, by number.
    pub(super) fn find(&self, text: &str) -> R {
        let reply = self.ask(&format!("lines {}", crate::term::HISTORY))?;
        let mut lines = reply.lines();
        let head = lines.next().unwrap_or_default();
        let mark: u64 = head.strip_prefix("mark ").and_then(|m| m.parse().ok()).ok_or(reply.clone())?;
        let body: Vec<&str> = lines.collect();
        println!("{head}");
        let t = text.to_lowercase();
        let first = (mark + 1).saturating_sub(body.len() as u64);
        let mut any = false;
        for (k, l) in body.iter().enumerate().filter(|(_, l)| l.to_lowercase().contains(&t)) {
            println!("{}: {l}", first + k as u64);
            any = true;
        }
        if !any {
            println!("(no line says that)");
        }
        Ok(())
    }

    /// Typed as a paste; a newline is Enter.
    pub(super) fn type_text(&self, text: &str) -> R {
        for (k, piece) in text.split('\n').enumerate() {
            if k > 0 {
                self.ask("keys enter")?;
            }
            if !piece.is_empty() {
                self.ask(&format!("paste {}", hex(piece)))?;
            }
        }
        Ok(())
    }

    pub(super) fn key(&self, name: &str) -> R {
        self.ask(&format!("keys {name}"))?;
        Ok(())
    }

    /// `run`: typed, Enter, and what it printed once the shell waits again
    /// (as run.rs answers it). A Stop meanwhile ends it with what came out.
    pub(super) fn run(&self, command: &str, secs: f64, full: bool) -> R {
        let (start, from) = (std::time::Instant::now(), self.mark()?);
        self.ask(&format!("paste {}", hex(command)))?;
        self.ask("keys enter")?;
        let ended = self.until(&Until::Prompt, secs, start, &from, true)?;
        let reply = self.ask(&format!("lines {} {}", crate::term::HISTORY, from.0))?;
        // The shell waiting, its cursor is on its prompt's line.
        let cursor = match ended {
            Ended::Held => crate::term::info(&self.socket)?.get("cursor").and_then(|c| c.parse().ok()),
            _ => None,
        };
        let out = run::Output::of(&reply, command, &from.1, matches!(ended, Ended::Held), cursor);
        print!("{}", out.answer(&self.name, from.0, full));
        match ended {
            Ended::Held => Ok(()),
            Ended::Stopped => Err(Fail::new(fail::STOPPED, format!("{} (what it printed so far is above)", fail::stopped().msg))),
            Ended::Late => Err(Fail::new(fail::TIMEOUT, run::late(&self.name, secs, out.mark))),
        }
    }

    /// `wait`: from now, or (`from`) after an earlier read's mark.
    pub(super) fn wait(&self, until: &Until, secs: f64, from: Option<u64>) -> R {
        let start = std::time::Instant::now();
        let from = match from {
            Some(m) => (m, self.line(m)?),
            None => self.mark()?,
        };
        self.follow(until, secs, start, &from)
    }

    /// What line M says now, unless it is no longer kept.
    fn line(&self, m: u64) -> R<String> {
        let reply = self.ask(&format!("lines {} 0 peek", crate::term::HISTORY))?;
        let mut lines = reply.lines();
        let mark = lines.next().and_then(|l| l.strip_prefix("mark ")).and_then(|m| m.parse::<u64>().ok()).ok_or(reply.clone())?;
        if m > mark {
            return Err(usage(format!("--from {m}: its newest line is {mark}")));
        }
        let body: Vec<&str> = lines.collect();
        Ok(body.len().checked_sub(1 + (mark - m) as usize).map_or(String::new(), |k| body[k].to_owned()))
    }

    /// The screen's lines.
    pub(super) fn screen(&self) -> R<Vec<String>> {
        Ok(self.ask("screen")?.lines().map(str::to_owned).collect())
    }

    /// Whether what came after the mark `from` (whose line said `last`) has
    /// that text: the new lines, and of the mark's line only what was written
    /// on it since; of the screen when that was written over (`screen`), the
    /// lines that say it beyond those that did (`before`).
    pub(super) fn has(&self, text: &str, before: &[String], from: u64, last: &str) -> R<bool> {
        let reply = self.ask(&format!("lines {} {from} peek", crate::term::HISTORY))?;
        let mut lines = reply.lines();
        let mark = lines.next().and_then(|l| l.strip_prefix("mark ")).and_then(|m| m.parse::<u64>().ok()).unwrap_or(from);
        let body: Vec<&str> = lines.collect();
        Ok(match body.split_first() {
            Some((&"screen", screen)) => {
                let mut before = before.to_vec();
                screen.iter().filter(|l| says(l, text)).any(|l| match before.iter().position(|b| b == l) {
                    Some(k) => {
                        before.swap_remove(k);
                        false
                    }
                    None => true,
                })
            }
            _ => body.iter().enumerate().any(|(k, l)| {
                let again = k == 0 && body.len() as u64 == mark.saturating_sub(from) + 1;
                says(if again { l.strip_prefix(last).unwrap_or(l) } else { l }, text)
            }),
        })
    }

    /// Until it holds, or for so many seconds: then `ok` and what came out
    /// after the mark `from` (as a read's answer).
    pub(super) fn follow(&self, until: &Until, secs: f64, start: std::time::Instant, from: &(u64, String)) -> R {
        let ended = self.until(until, secs, start, from, false)?;
        print!("{}{}", if matches!(ended, Ended::Held) { "ok\n" } else { "" }, self.ask(&format!("lines {} {}", crate::term::HISTORY, from.0))?);
        match ended {
            Ended::Held => Ok(()),
            Ended::Stopped => {
                let f = fail::stopped();
                Err(Fail::new(f.code, format!("{} (what came out meanwhile is above)", f.msg)))
            }
            Ended::Late => Err(Fail::new(fail::TIMEOUT, format!("timeout: it did not happen in {secs} s (what came out meanwhile is above; `tree {} --since {}` goes on from there)", self.name, from.0))),
        }
    }

    /// Looks until it holds, it is too late or the agent was stopped. `run`:
    /// the prompt counts only once something came out (the command was taken).
    pub(super) fn until(&self, until: &Until, secs: f64, start: std::time::Instant, (from, last): &(u64, String), run: bool) -> R<Ended> {
        let from = *from;
        let mut asked = std::time::Instant::now();
        // The shell in the foreground for so many looks in a row: between the
        // commands of `a; b` it is, for a moment.
        let mut at_shell = 0;
        loop {
            let info = crate::term::info(&self.socket)?;
            let number = |k: &str| info.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let quiet = number("quiet");
            let came = quiet < start.elapsed().as_millis() as u64;
            // Or another that reads commands under it (ssh, a shell started
            // in it, a REPL), its prompt the cursor waits after.
            let other = info.get("fullscreen").is_some_and(|f| f == "0") && info.get("before").is_some_and(|b| prompted(b));
            at_shell = if number("foreground") == number("shell") || other { at_shell + 1 } else { 0 };
            let holds = match until {
                Until::Has(text, before) => self.has(text, before, from, last)?,
                Until::Gone(text) => !self.screen()?.iter().any(|l| says(l, text)),
                Until::Changed => came,
                Until::Settled(ms) => quiet >= *ms,
                Until::Prompt => at_shell >= 2 && quiet >= 300 && (came || !run),
            };
            if holds {
                return Ok(Ended::Held);
            }
            // A window made private, a session locked or a Stop meanwhile ends it.
            let stopped = asked.elapsed() >= std::time::Duration::from_secs(1) && {
                asked = std::time::Instant::now();
                Term::allowed(&self.pid, false)?.is_some_and(|ago| ago < start.elapsed().as_millis() as u64)
            };
            if stopped {
                return Ok(Ended::Stopped);
            }
            if start.elapsed().as_secs_f64() >= secs {
                return Ok(Ended::Late);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Whether what a line says before the cursor ends as a prompt does: a
/// shell's or a REPL's mark, and a space.
fn prompted(before: &str) -> bool {
    matches!(before.strip_suffix(' ').and_then(|b| b.chars().next_back()), Some('$' | '#' | '%' | '>' | '\u{276f}' | '\u{279c}' | '\u{bb}' | '\u{3bb}'))
}

/// Whether a line says that, case folded.
pub(super) fn says(line: &str, text: &str) -> bool {
    line.to_lowercase().contains(&text.to_lowercase())
}

/// The text `type` or `run` takes: its words, or (`-`) the input, so that
/// a password is not seen in the list of processes and nothing is quoted twice.
pub(super) fn typed(words: &[String]) -> R<String> {
    if words.len() == 1 && words[0] == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
        return Ok(s.strip_suffix('\n').map(str::to_owned).unwrap_or(s));
    }
    Ok(words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_is_told_by_its_mark_before_the_cursor() {
        for p in ["bash-5.3$ ", "root@box:~# ", "user@host ~ % ", ">>> ", "\u{276f} ", "~/src \u{279c} "] {
            assert!(prompted(p), "{p:?}");
        }
        // Output that stopped, a mark not followed by a space, or nothing.
        for p in ["Progress: 50%", "Downloading... ", "a$b", "", "$"] {
            assert!(!prompted(p), "{p:?}");
        }
    }
}
