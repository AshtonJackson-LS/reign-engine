//! Editors the hub starts, watched so one that quits with an error says why.
//!
//! An editor started without a console has nowhere to print, so before
//! this a project whose editor exited at once (a bad argument, a panic, a
//! missing file) looked like a click that did nothing. The hub now keeps
//! the end of each editor's stderr and, when it exits unsuccessfully, puts
//! the most telling line in the status bar.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How many of an editor's last stderr lines are kept.
const TAIL: usize = 40;

/// An editor the hub started.
pub struct Launched {
    child: Child,
    started: Instant,
    tail: Arc<Mutex<VecDeque<String>>>,
}

/// How an editor ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub success: bool,
    /// The exit code, when the platform gives one.
    pub code: Option<i32>,
    /// How long it ran.
    pub after: Duration,
    /// Its last stderr lines, oldest first.
    pub tail: Vec<String>,
}

impl Launched {
    /// Starts `command` with stderr captured and passed on to the hub's own
    /// stderr; stdout is left as it was. A thread drains stderr as it comes,
    /// so a chatty editor never blocks on a full pipe.
    pub fn spawn(mut command: Command) -> std::io::Result<Launched> {
        command.stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(TAIL)));
        if let Some(stderr) = child.stderr.take() {
            let tail = Arc::clone(&tail);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    // Still printed where it went before (the hub's own
                    // terminal, when there is one).
                    eprintln!("{line}");
                    let Ok(mut tail) = tail.lock() else { return };
                    if tail.len() == TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }
        Ok(Launched { child, started: Instant::now(), tail })
    }

    /// `None` while it runs; how it ended once it has.
    pub fn poll(&mut self) -> Option<Exit> {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status,
            Ok(None) => return None,
            // Can't ask: treat it as gone, with nothing to say.
            Err(_) => return Some(self.exit(true, None)),
        };
        Some(self.exit(status.success(), status.code()))
    }

    fn exit(&self, success: bool, code: Option<i32>) -> Exit {
        // The drain thread may still be reading the last lines: give it a
        // moment once the process is gone.
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            let done = Arc::strong_count(&self.tail) == 1;
            if done || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let tail = self.tail.lock().map(|t| t.iter().cloned().collect()).unwrap_or_default();
        Exit { success, code, after: self.started.elapsed(), tail }
    }
}

impl Exit {
    /// The line that best says what went wrong: the last one that reads
    /// like an error or a panic, else the last line printed.
    pub fn reason(&self) -> Option<&str> {
        let lines = || self.tail.iter().map(|l| l.trim()).filter(|l| !l.is_empty());
        let looks_wrong = |l: &&str| {
            let lower = l.to_ascii_lowercase();
            ["error", "panicked", "unknown argument", "failed", "can't", "cannot"]
                .iter()
                .any(|w| lower.contains(w))
        };
        lines().rfind(looks_wrong).or_else(|| lines().next_back())
    }

    /// What the status bar says when `name`'s editor exits; `None` for a
    /// normal close.
    pub fn message(&self, name: &str) -> Option<String> {
        if self.success {
            return None;
        }
        let code = self.code.map_or_else(String::new, |c| format!(" (exit code {c})"));
        let when = if self.after < Duration::from_secs(10) {
            "couldn't start"
        } else {
            "quit with an error"
        };
        Some(match self.reason() {
            Some(reason) => format!("{name}'s editor {when}{code}: {reason}"),
            None => format!("{name}'s editor {when}{code}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(success: bool, code: Option<i32>, secs: u64, tail: &[&str]) -> Exit {
        Exit {
            success,
            code,
            after: Duration::from_secs(secs),
            tail: tail.iter().map(|l| l.to_string()).collect(),
        }
    }

    #[test]
    fn a_normal_close_says_nothing() {
        assert_eq!(exit(true, Some(0), 300, &["bye"]).message("Game"), None);
    }

    #[test]
    fn an_early_failure_names_the_error_line() {
        let e = exit(
            false,
            Some(2),
            1,
            &[
                "[INFO reign_render] GPU: something",
                "unknown argument C:\\Games\\Exreign",
                "usage: exreign-editor [--play]",
            ],
        );
        assert_eq!(
            e.message("Exreign").unwrap(),
            "Exreign's editor couldn't start (exit code 2): unknown argument C:\\Games\\Exreign"
        );
    }

    #[test]
    fn a_late_crash_says_it_quit() {
        let e = exit(false, Some(101), 120, &["thread 'main' panicked at src/lib.rs:4:5:", ""]);
        assert_eq!(
            e.message("Game").unwrap(),
            "Game's editor quit with an error (exit code 101): thread 'main' panicked at src/lib.rs:4:5:"
        );
    }

    #[test]
    fn with_no_telling_line_the_last_one_is_used() {
        assert_eq!(exit(false, None, 1, &["first", "last", "  "]).reason(), Some("last"));
        assert_eq!(
            exit(false, Some(1), 1, &[]).message("G").unwrap(),
            "G's editor couldn't start (exit code 1)"
        );
    }

    /// A real process: its stderr is captured and its exit code read.
    #[test]
    fn captures_a_real_process() {
        #[cfg(windows)]
        let command = {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo warming up 1>&2 & echo error: no such project 1>&2 & exit 3"]);
            c
        };
        #[cfg(not(windows))]
        let command = {
            let mut c = Command::new("sh");
            c.args(["-c", "echo warming up >&2; echo 'error: no such project' >&2; exit 3"]);
            c
        };
        let mut launched = Launched::spawn(command).expect("spawn");
        let started = Instant::now();
        let exit = loop {
            if let Some(exit) = launched.poll() {
                break exit;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "never exited");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!exit.success);
        assert_eq!(exit.code, Some(3));
        assert_eq!(exit.reason(), Some("error: no such project"));
    }
}
