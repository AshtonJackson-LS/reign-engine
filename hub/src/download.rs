//! Downloads with progress: how much has arrived, how big the whole file is,
//! and how fast it's coming.
//!
//! The hub downloads with the system's `curl` (see `update.rs`). While curl
//! runs, this watches the file it is writing: its size is the bytes received,
//! and curl is asked to save the response headers, whose `Content-Length`
//! (the last one, after any redirects) is the total. Speed is measured over
//! the last few seconds, so it follows the connection instead of averaging
//! the whole download. [`Progress`] is shared with the window that shows it,
//! and can cancel the download.

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use reign_setup::system_tool;

/// The error a cancelled download returns.
pub const CANCELLED: &str = "cancelled";

/// What an update or engine download is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the first bytes.
    Starting,
    Downloading,
    /// Checking the release key's signature.
    Verifying,
    Unpacking,
    /// Putting the files in place.
    Installing,
}

/// A download's progress, shared between the thread doing it and the window
/// showing it.
pub struct Progress {
    state: Mutex<State>,
    cancel: AtomicBool,
}

struct State {
    label: String,
    phase: Phase,
    received: u64,
    total: Option<u64>,
    meter: SpeedMeter,
}

/// A copy of a [`Progress`] at one moment, for drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// What is being downloaded, like `Reign 0.2.0`.
    pub label: String,
    pub phase: Phase,
    pub received: u64,
    /// The whole file's size, when the server said.
    pub total: Option<u64>,
    /// Bytes a second over the last few seconds.
    pub speed: Option<f64>,
}

impl Progress {
    pub fn new(label: impl Into<String>) -> Self {
        Progress {
            state: Mutex::new(State {
                label: label.into(),
                phase: Phase::Starting,
                received: 0,
                total: None,
                meter: SpeedMeter::default(),
            }),
            cancel: AtomicBool::new(false),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            label: state.label.clone(),
            phase: state.phase,
            received: state.received,
            total: state.total,
            speed: state.meter.speed(Instant::now()),
        }
    }

    pub fn set_phase(&self, phase: Phase) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).phase = phase;
    }

    /// Records that `received` bytes of `total` (if known) have arrived.
    pub fn report(&self, received: u64, total: Option<u64>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.phase == Phase::Starting && received > 0 {
            state.phase = Phase::Downloading;
        }
        state.received = received;
        if total.is_some() {
            state.total = total;
        }
        state.meter.push(Instant::now(), received);
    }

    /// Asks the download to stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl Snapshot {
    /// How much of the download has arrived, 0 to 1; `None` when the total
    /// isn't known.
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(total) if total > 0 => Some((self.received as f64 / total as f64).min(1.0) as f32),
            _ => None,
        }
    }

    /// How long the rest should take at the current speed.
    pub fn eta(&self) -> Option<Duration> {
        let (total, speed) = (self.total?, self.speed?);
        if speed < 1.0 || self.received >= total {
            return None;
        }
        Some(Duration::from_secs_f64((total - self.received) as f64 / speed))
    }

    /// The line under the bar while downloading: `12.3 MB of 48.1 MB ·
    /// 3.2 MB/s · about 11 s left`. Without a known total, just what has
    /// arrived and the speed.
    pub fn detail(&self) -> String {
        let mut parts = vec![match self.total {
            Some(total) => format!("{} of {}", format_size(self.received), format_size(total)),
            None => format!("{} downloaded", format_size(self.received)),
        }];
        if let Some(speed) = self.speed {
            parts.push(format_speed(speed));
        }
        if let Some(eta) = self.eta() {
            parts.push(format!("about {} left", format_duration(eta)));
        }
        parts.join(" · ")
    }

    /// What the window says in each phase.
    pub fn phase_text(&self) -> &'static str {
        match self.phase {
            Phase::Starting => "Connecting…",
            Phase::Downloading => "Downloading…",
            Phase::Verifying => "Checking the signature…",
            Phase::Unpacking => "Unpacking…",
            Phase::Installing => "Installing…",
        }
    }
}

/// Bytes a second, measured between the oldest and newest samples of the
/// last few seconds.
#[derive(Default)]
struct SpeedMeter {
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    /// How far back the speed looks.
    const WINDOW: Duration = Duration::from_secs(3);
    /// The least time two samples must span to give a speed.
    const MIN_SPAN: Duration = Duration::from_millis(400);

    fn push(&mut self, now: Instant, bytes: u64) {
        self.samples.push_back((now, bytes));
        while self.samples.len() > 2
            && self.samples.front().is_some_and(|(t, _)| now.duration_since(*t) > Self::WINDOW)
        {
            self.samples.pop_front();
        }
    }

    fn speed(&self, _now: Instant) -> Option<f64> {
        let (first_at, first) = *self.samples.front()?;
        let (last_at, last) = *self.samples.back()?;
        let span = last_at.duration_since(first_at);
        if span < Self::MIN_SPAN {
            return None;
        }
        Some(last.saturating_sub(first) as f64 / span.as_secs_f64())
    }
}

/// `512 B`, `123 KB`, `12.3 MB`, `1.25 GB`.
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

/// `3.2 MB/s`.
pub fn format_speed(bytes_per_second: f64) -> String {
    format!("{}/s", format_size(bytes_per_second.max(0.0) as u64))
}

/// `8 s`, `1 min 5 s`, `12 min`.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64().ceil() as u64;
    match secs {
        0..60 => format!("{} s", secs.max(1)),
        60..600 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} min", secs.div_ceil(60)),
    }
}

/// The `Content-Length` of the last successful response in the headers curl
/// saved with `-D`: with `-L` there is one block per redirect, each ending
/// in a blank line, and only the final one describes the file.
pub fn final_content_length(headers: &str) -> Option<u64> {
    let headers = headers.replace("\r\n", "\n");
    let mut length = None;
    for block in headers.split("\n\n") {
        let status = block.trim_start().lines().next().unwrap_or("");
        let succeeded = status.starts_with("HTTP/")
            && status.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2'));
        if !succeeded {
            continue;
        }
        let found = block.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok())?
        });
        // A later success replaces an earlier one; a block without a length
        // (chunked) leaves the answer unknown rather than keeping a stale one.
        length = found;
    }
    length
}

/// Downloads `url` to `to` with curl, reporting to `progress` (if any) about
/// ten times a second. Returns [`CANCELLED`] if `progress` is cancelled, and
/// leaves no partial file behind on failure.
pub fn download_file(url: &str, to: &Path, progress: Option<&Progress>) -> Result<(), String> {
    let headers = to.with_extension("headers");
    let result = run_curl(url, to, &headers, progress);
    let _ = std::fs::remove_file(&headers);
    if result.is_err() {
        let _ = std::fs::remove_file(to);
    }
    result
}

fn run_curl(
    url: &str,
    to: &Path,
    headers: &Path,
    progress: Option<&Progress>,
) -> Result<(), String> {
    let mut child = system_tool("curl")
        .args(["-fsSL", "--max-time", "600", "-D"])
        .arg(headers)
        .arg("-o")
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run curl: {e}"))?;
    let size = || std::fs::metadata(to).map_or(0, |m| m.len());
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(format!("can't run curl: {e}")),
        }
        if let Some(progress) = progress {
            if progress.cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CANCELLED.to_owned());
            }
            let total =
                std::fs::read_to_string(headers).ok().and_then(|h| final_content_length(&h));
            progress.report(size(), total);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if status.success() {
        if let Some(progress) = progress {
            let done = size();
            progress.report(done, Some(done));
        }
        return Ok(());
    }
    let mut why = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut why);
    }
    let why = why.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    Err(if why.is_empty() {
        format!("couldn't download {url}")
    } else {
        format!("couldn't download {url} ({why})")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(300 * 1024), "300 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(12_900_000), "12.3 MB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024 / 4), "1.25 GB");
        assert_eq!(format_speed(3.2 * 1024.0 * 1024.0), "3.2 MB/s");
        assert_eq!(format_speed(-5.0), "0 B/s");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(Duration::from_millis(200)), "1 s");
        assert_eq!(format_duration(Duration::from_secs(8)), "8 s");
        assert_eq!(format_duration(Duration::from_secs_f64(8.2)), "9 s");
        assert_eq!(format_duration(Duration::from_secs(65)), "1 min 5 s");
        assert_eq!(format_duration(Duration::from_secs(599)), "9 min 59 s");
        assert_eq!(format_duration(Duration::from_secs(601)), "11 min");
    }

    #[test]
    fn the_speed_follows_the_last_few_seconds() {
        let t0 = Instant::now();
        let mut meter = SpeedMeter::default();
        assert_eq!(meter.speed(t0), None, "no samples");
        meter.push(t0, 0);
        assert_eq!(meter.speed(t0), None, "one sample");
        meter.push(t0 + Duration::from_millis(100), 1000);
        assert_eq!(meter.speed(t0), None, "too short a span to trust");
        // Fast for 2 seconds: 1 MB a second.
        for i in 1..=20 {
            meter.push(t0 + Duration::from_millis(100 * i), 100_000 * i);
        }
        let fast = meter.speed(t0).unwrap();
        assert!((fast - 1_000_000.0).abs() < 1.0, "{fast}");
        // Then stalled for 5 seconds: the window forgets the fast part.
        for i in 21..=70 {
            meter.push(t0 + Duration::from_millis(100 * i), 2_000_000);
        }
        assert_eq!(meter.speed(t0), Some(0.0));
        // Samples older than the window are dropped, not kept forever.
        assert!(meter.samples.len() <= 32, "{}", meter.samples.len());
    }

    #[test]
    fn the_last_successful_response_has_the_size() {
        let redirected = "HTTP/2 302 \r\nlocation: https://cdn.example/x\r\ncontent-length: 0\r\n\r\n\
             HTTP/2 200 \r\nContent-Type: application/octet-stream\r\nContent-Length: 48123456\r\n\r\n";
        assert_eq!(final_content_length(redirected), Some(48_123_456));
        // HTTP/1.1, LF endings, header names in any case.
        assert_eq!(final_content_length("HTTP/1.1 200 OK\ncontent-LENGTH: 7\n\n"), Some(7));
        // The redirect's zero is never the answer, and neither is an error's.
        assert_eq!(final_content_length("HTTP/2 302 \r\ncontent-length: 0\r\n\r\n"), None);
        assert_eq!(final_content_length("HTTP/2 404 \r\ncontent-length: 9\r\n\r\n"), None);
        // No length (chunked), partial text, nothing at all.
        assert_eq!(final_content_length("HTTP/2 200 \r\ntransfer-encoding: chunked\r\n\r\n"), None);
        assert_eq!(final_content_length("HTTP/2 200 \r\ncontent-len"), None);
        assert_eq!(final_content_length(""), None);
    }

    #[test]
    fn snapshots_work_out_fraction_and_time_left() {
        let mut s = Snapshot {
            label: "Reign 0.2.0".into(),
            phase: Phase::Downloading,
            received: 12 * 1024 * 1024,
            total: Some(48 * 1024 * 1024),
            speed: Some(3.0 * 1024.0 * 1024.0),
        };
        assert_eq!(s.fraction(), Some(0.25));
        assert_eq!(s.eta(), Some(Duration::from_secs(12)));
        assert_eq!(s.detail(), "12.0 MB of 48.0 MB · 3.0 MB/s · about 12 s left");
        // Unknown total: no bar, no time left, but what has arrived.
        s.total = None;
        assert_eq!((s.fraction(), s.eta()), (None, None));
        assert_eq!(s.detail(), "12.0 MB downloaded · 3.0 MB/s");
        // No speed yet, or finished: no time left.
        s.total = Some(48 * 1024 * 1024);
        s.speed = None;
        assert_eq!(s.eta(), None);
        s.speed = Some(1e6);
        s.received = s.total.unwrap();
        assert_eq!((s.fraction(), s.eta()), (Some(1.0), None));
        // More bytes than the total never overfills the bar.
        s.received += 99;
        assert_eq!(s.fraction(), Some(1.0));
    }

    #[test]
    fn progress_moves_from_starting_to_downloading() {
        let progress = Progress::new("Reign 0.2.0");
        assert_eq!(progress.snapshot().phase, Phase::Starting);
        progress.report(0, Some(100));
        assert_eq!(progress.snapshot().phase, Phase::Starting, "no bytes yet");
        progress.report(10, None);
        let s = progress.snapshot();
        assert_eq!((s.phase, s.received, s.total), (Phase::Downloading, 10, Some(100)));
        progress.set_phase(Phase::Verifying);
        assert_eq!(progress.snapshot().phase, Phase::Verifying);
        assert!(!progress.cancelled());
        progress.cancel();
        assert!(progress.cancelled());
    }

    /// A one-file HTTP server on a free port: `redirect` sends the client to
    /// `/real` first; `chunked` leaves out the length; the body goes out in
    /// `pieces` pieces, `pause` apart. Serves `requests` requests.
    fn serve(
        body: Vec<u8>,
        pieces: usize,
        pause: Duration,
        redirect: bool,
        chunked: bool,
        requests: usize,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(requests) {
                let Ok(mut stream) = stream else { return };
                let mut request = [0u8; 2048];
                let n = std::io::Read::read(&mut stream, &mut request).unwrap_or(0);
                let first =
                    String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("").to_owned();
                if redirect && first.contains("GET /file") {
                    let _ = stream.write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: /real\r\nContent-Length: 0\r\n\r\n",
                    );
                    continue;
                }
                if first.contains("/missing") {
                    let _ =
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                send(&mut stream, &body, pieces, pause, chunked);
            }
        });
        format!("http://{address}")
    }

    fn send(stream: &mut TcpStream, body: &[u8], pieces: usize, pause: Duration, chunked: bool) {
        let head = if chunked {
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_owned()
        } else {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
        };
        if stream.write_all(head.as_bytes()).is_err() {
            return;
        }
        for piece in body.chunks(body.len().div_ceil(pieces)) {
            if stream.write_all(piece).is_err() || stream.flush().is_err() {
                return;
            }
            std::thread::sleep(pause);
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("reign-download-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A real curl against a local server that sends 400 KB in 20 slow
    /// pieces, behind a redirect: the progress grows, the total is known
    /// from the first moments, a speed shows up, and the file is whole.
    #[test]
    fn a_download_reports_growing_progress_and_the_total() {
        let body: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
        let base = serve(body.clone(), 20, Duration::from_millis(60), true, false, 2);
        let dir = scratch("progress");
        let file = dir.join("archive.zip");
        let progress = std::sync::Arc::new(Progress::new("test"));
        let watcher = {
            let progress = progress.clone();
            std::thread::spawn(move || {
                let mut seen = Vec::new();
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(20) {
                    let s = progress.snapshot();
                    seen.push(s.clone());
                    if s.total.is_some() && s.received == s.total.unwrap() && s.received > 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
                seen
            })
        };
        download_file(&format!("{base}/file"), &file, Some(&progress)).unwrap();
        let seen = watcher.join().unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), body, "the whole file");
        assert!(!file.with_extension("headers").exists(), "header file cleaned up");
        let last = progress.snapshot();
        assert_eq!((last.received, last.total), (400_000, Some(400_000)));
        let received: Vec<u64> = seen.iter().map(|s| s.received).collect();
        assert!(received.windows(2).all(|w| w[0] <= w[1]), "never goes backwards: {received:?}");
        assert!(
            seen.iter().any(|s| s.received > 0 && s.received < 400_000),
            "saw it part way: {received:?}"
        );
        assert!(
            seen.iter().any(|s| s.total == Some(400_000) && s.received < 400_000),
            "total known early"
        );
        let fastest = seen.iter().filter_map(|s| s.speed).fold(0.0, f64::max);
        assert!(fastest > 50_000.0, "a speed was measured: {fastest}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_without_a_length_still_reports_bytes() {
        let body = vec![7u8; 90_000];
        let base = serve(body.clone(), 6, Duration::from_millis(50), false, true, 1);
        let dir = scratch("chunked");
        let file = dir.join("a.bin");
        let progress = Progress::new("test");
        download_file(&format!("{base}/file"), &file, Some(&progress)).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), body);
        let last = progress.snapshot();
        // curl reports the length it got to at the end: the bar finishes.
        assert_eq!((last.received, last.total), (90_000, Some(90_000)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_download_says_why_and_leaves_nothing() {
        let base = serve(Vec::new(), 1, Duration::ZERO, false, false, 1);
        let dir = scratch("missing");
        let file = dir.join("a.bin");
        let error = download_file(&format!("{base}/missing"), &file, Some(&Progress::new("t")))
            .unwrap_err();
        assert!(error.contains("couldn't download") && error.contains("404"), "{error}");
        assert!(!file.exists() && !file.with_extension("headers").exists());
        // Nothing listening at all.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", closed.local_addr().unwrap());
        drop(closed);
        let error = download_file(&url, &file, None).unwrap_err();
        assert!(error.starts_with("couldn't download"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_can_be_cancelled_and_leaves_nothing() {
        // 2 MB over about 6 seconds, cancelled after the first bytes arrive.
        let body = vec![1u8; 2_000_000];
        let base = serve(body, 40, Duration::from_millis(150), false, false, 1);
        let dir = scratch("cancel");
        let file = dir.join("a.bin");
        let progress = std::sync::Arc::new(Progress::new("test"));
        let canceller = {
            let progress = progress.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                while progress.snapshot().received == 0
                    && started.elapsed() < Duration::from_secs(10)
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                progress.cancel();
            })
        };
        let started = Instant::now();
        let error = download_file(&format!("{base}/file"), &file, Some(&progress)).unwrap_err();
        canceller.join().unwrap();
        assert_eq!(error, CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stopped early: {:?}",
            started.elapsed()
        );
        assert!(!file.exists() && !file.with_extension("headers").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
