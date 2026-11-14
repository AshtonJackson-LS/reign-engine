//! Editors the hub starts, watched so one that quits with an error says why.
//!
//! An editor started without a console has nowhere to print, so before
//! this a project whose editor exited at once (a bad argument, a panic, a
//! missing file) looked like a click that did nothing. The hub now keeps
//! the end of each editor's stderr and, when it exits unsuccessfully, puts
//! the most telling line in the status bar.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
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
                    // terminal, when there is one). Not eprintln!: it panics
                    // when that terminal is gone, which would lose the line.
                    let _ = writeln!(std::io::stderr(), "{line}");
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
    /// What best says what went wrong, from the editor's last lines, in this
    /// order: a panic's message (Rust prints it on the line after
    /// `thread '..' panicked at FILE:`), the last ERROR-level log line
    /// (without its timestamp), the last line starting `error`, the last
    /// that mentions a failure, else the last line. Cut to fit the status
    /// bar.
    pub fn reason(&self) -> Option<String> {
        let lines: Vec<&str> =
            self.tail.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        let panic = lines.iter().rposition(|l| l.contains("panicked at")).map(|at| {
            let place = lines[at].split("panicked at").nth(1).unwrap_or("").trim();
            let place = place.trim_end_matches(':');
            let message = lines[at + 1..]
                .iter()
                .find(|l| !l.starts_with("note:") && !l.starts_with("stack backtrace"));
            match message {
                Some(message) => format!("panicked: {message} (at {place})"),
                None => format!("panicked at {place}"),
            }
        });
        let logged_error = || {
            lines.iter().rev().find_map(|l| {
                // env_logger: `[2026-11-14T17:44:38Z ERROR target] message`.
                let (head, message) = l.strip_prefix('[')?.split_once("] ")?;
                head.split_whitespace().any(|w| w == "ERROR").then(|| message.to_owned())
            })
        };
        let starts_error =
            || lines.iter().rev().find(|l| l.to_ascii_lowercase().starts_with("error"));
        let mentions_failure = || {
            lines.iter().rev().find(|l| {
                let lower = l.to_ascii_lowercase();
                [
                    "panicked",
                    "unknown argument",
                    "unknown flag",
                    "failed",
                    "can't",
                    "cannot",
                    "not found",
                ]
                .iter()
                .any(|w| lower.contains(w))
            })
        };
        let reason = panic
            .or_else(logged_error)
            .or_else(|| starts_error().map(|l| l.to_string()))
            .or_else(|| mentions_failure().map(|l| l.to_string()))
            .or_else(|| lines.last().map(|l| l.to_string()))?;
        Some(shorten(&reason, 160))
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

/// `text` cut to at most `max` characters, with `…` when cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
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
        let e = exit(
            false,
            Some(101),
            120,
            &[
                "[2026-11-14T17:44:38Z INFO  reign_editor::shell] play",
                "",
                "thread 'main' (4312) panicked at src/lib.rs:4:5:",
                "index out of bounds: the len is 3 but the index is 7",
                "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
            ],
        );
        assert_eq!(
            e.message("Game").unwrap(),
            "Game's editor quit with an error (exit code 101): panicked: index out of bounds: the len is 3 but the index is 7 (at src/lib.rs:4:5)"
        );
        // A panic whose message didn't make it out still says where.
        let bare = exit(false, Some(101), 1, &["thread 'main' panicked at src/a.rs:1:1:"]);
        assert_eq!(bare.reason().unwrap(), "panicked at src/a.rs:1:1");
    }

    #[test]
    fn an_error_log_line_beats_info_that_mentions_errors() {
        let e = exit(
            false,
            Some(1),
            2,
            &[
                "[2026-11-14T17:44:38Z ERROR reign_assets::db] can't read scenes/main.scene.ron",
                "[2026-11-14T17:44:39Z INFO  reign_editor] checked 40 assets, 0 errors",
            ],
        );
        assert_eq!(e.reason().unwrap(), "can't read scenes/main.scene.ron");
        // `Error: ..` from a main that returned Err.
        let e = exit(false, Some(1), 2, &["[INFO x] 0 errors so far", "Error: Os { code: 5 }"]);
        assert_eq!(e.reason().unwrap(), "Error: Os { code: 5 }");
    }

    #[test]
    fn long_reasons_are_cut_to_fit() {
        let long = "error: ".to_owned() + &"x".repeat(400);
        let e = exit(false, Some(1), 1, &[&long]);
        let reason = e.reason().unwrap();
        assert_eq!(reason.chars().count(), 160);
        assert!(reason.ends_with('…') && reason.starts_with("error: xxx"));
    }

    #[test]
    fn with_no_telling_line_the_last_one_is_used() {
        assert_eq!(
            exit(false, None, 1, &["first", "last", "  "]).reason().as_deref(),
            Some("last")
        );
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
        assert_eq!(exit.reason().as_deref(), Some("error: no such project"));
    }
}
