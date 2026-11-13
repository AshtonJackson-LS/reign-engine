//! The Console: a `log` logger that writes to stderr exactly as
//! `env_logger` does (honouring `RUST_LOG`) and keeps the most recent lines
//! for the Console panel.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use log::{Level, Log, Metadata, Record};

/// Lines kept for the panel; older ones are dropped.
pub const CONSOLE_LINES: usize = 4000;

/// One logged line.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Seconds since the logger was installed.
    pub time: f32,
    pub level: Level,
    pub target: String,
    pub message: String,
}

/// The lines the Console shows. Cheap to clone; all clones share the lines.
#[derive(Clone, Debug, Default)]
pub struct ConsoleBuffer {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
}

impl ConsoleBuffer {
    pub fn push(&self, line: LogLine) {
        let mut lines = self.lock();
        if lines.len() == CONSOLE_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The kept lines, oldest first, that pass `keep`.
    pub fn lines(&self, mut keep: impl FnMut(&LogLine) -> bool) -> Vec<LogLine> {
        self.lock().iter().filter(|l| keep(l)).cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<LogLine>> {
        // A panic while logging must not take the Console down with it.
        self.lines.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Tee {
    stderr: env_logger::Logger,
    buffer: ConsoleBuffer,
    start: Instant,
}

impl Log for Tee {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.stderr.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.stderr.matches(record) {
            return;
        }
        self.stderr.log(record);
        self.buffer.push(LogLine {
            time: self.start.elapsed().as_secs_f32(),
            level: record.level(),
            target: record.target().to_owned(),
            message: record.args().to_string(),
        });
    }

    fn flush(&self) {
        self.stderr.flush();
    }
}

static CONSOLE: OnceLock<ConsoleBuffer> = OnceLock::new();

/// Installs the editor's logger, at `info` unless `RUST_LOG` says
/// otherwise, and returns the Console's lines. Safe to call more than once;
/// if another logger was installed first, logs keep going there and the
/// Console stays empty.
pub fn init_logger() -> ConsoleBuffer {
    CONSOLE
        .get_or_init(|| {
            let buffer = ConsoleBuffer::default();
            let stderr =
                env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                    .build();
            let max = stderr.filter();
            let tee = Tee { stderr, buffer: buffer.clone(), start: Instant::now() };
            if log::set_boxed_logger(Box::new(tee)).is_ok() {
                log::set_max_level(max);
            } else {
                eprintln!("reign_editor: a logger is already installed; the Console stays empty");
            }
            buffer
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(message: &str, level: Level) -> LogLine {
        LogLine { time: 0.0, level, target: "test".into(), message: message.into() }
    }

    #[test]
    fn buffer_keeps_the_newest_lines_and_filters() {
        let buffer = ConsoleBuffer::default();
        for i in 0..CONSOLE_LINES + 5 {
            buffer.push(line(&i.to_string(), if i % 2 == 0 { Level::Info } else { Level::Warn }));
        }
        assert_eq!(buffer.len(), CONSOLE_LINES);
        assert_eq!(buffer.lines(|_| true)[0].message, "5");
        let warnings = buffer.lines(|l| l.level == Level::Warn);
        assert!(warnings.iter().all(|l| l.level == Level::Warn));
        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn logger_tees_into_the_console() {
        let console = init_logger();
        log::warn!(target: "console_test", "hello console");
        let found = console.lines(|l| l.target == "console_test");
        assert_eq!(found.last().map(|l| l.message.as_str()), Some("hello console"));
        assert_eq!(found.last().map(|l| l.level), Some(Level::Warn));
    }
}
