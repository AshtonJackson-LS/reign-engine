//! Reading what `cargo build` prints, including how far along it is.
//!
//! Cargo tracks its progress (`Building [=====>   ] 254/257: reign_setup`)
//! but only prints it when it thinks it is writing to a terminal, and
//! redraws it in place with a carriage return rather than a new line. The
//! hub and the editor run cargo with its output piped, so they ask for the
//! progress line ([`ENV`], which `project::build_command` sets) and read the
//! output with [`read_output`], which splits at either kind of line end and
//! tells the progress apart from the lines that belong in a log.
//!
//! The count is cargo's own: build steps done out of all of them, including
//! every dependency's build script and library. A project's first build has
//! hundreds, so the bar moves steadily; the last step (the project itself)
//! takes as long as it takes.

use std::io::Read;

/// The environment variables that make cargo print its progress when its
/// output isn't a terminal. The width keeps the bar and names short; only
/// the numbers and names after it are read.
pub const ENV: [(&str, &str); 2] =
    [("CARGO_TERM_PROGRESS_WHEN", "always"), ("CARGO_TERM_PROGRESS_WIDTH", "80")];

/// How far along a build is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildProgress {
    /// Build steps finished.
    pub done: u32,
    /// All of them, as far as cargo knows (it can grow as it finds more).
    pub total: u32,
    /// What is being built now, like `syn, serde_derive`; cargo cuts a long
    /// list short with `…`.
    pub building: String,
}

impl BuildProgress {
    /// Done over total, 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.done as f32 / self.total as f32).min(1.0) }
    }

    /// `254 of 257`.
    pub fn count(&self) -> String {
        format!("{} of {}", self.done, self.total)
    }
}

/// One thing cargo printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// A line for the log: `Compiling x v1.0`, a warning, an error.
    Line(String),
    /// A progress update.
    Progress(BuildProgress),
}

/// Reads cargo's progress line, `Building [====>    ] 254/257: names`
/// (leading space and trailing padding allowed); `None` for anything else.
pub fn parse_progress(segment: &str) -> Option<BuildProgress> {
    let rest = segment.trim().strip_prefix("Building")?.trim_start().strip_prefix('[')?;
    let (_, rest) = rest.split_once(']')?;
    let (counts, building) = rest.trim_start().split_once(':').unwrap_or((rest.trim(), ""));
    let (done, total) = counts.trim().split_once('/')?;
    Some(BuildProgress {
        done: done.trim().parse().ok()?,
        total: total.trim().parse().ok()?,
        building: building.trim().to_owned(),
    })
}

/// Reads `reader` (cargo's stderr) to its end, calling `f` for each line
/// and each progress update, in order. Lines end at `\n`, `\r` or `\r\n`;
/// empty ones are skipped. Bytes that aren't UTF-8 are replaced, never an
/// error.
pub fn read_output(mut reader: impl Read, mut f: impl FnMut(Output)) {
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let emit = |bytes: &[u8], f: &mut dyn FnMut(Output)| {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end();
        if text.trim().is_empty() {
            return;
        }
        f(match parse_progress(text) {
            Some(progress) => Output::Progress(progress),
            None => Output::Line(text.to_owned()),
        });
    };
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &byte in &chunk[..n] {
            if byte == b'\n' || byte == b'\r' {
                emit(&pending, &mut f);
                pending.clear();
            } else {
                pending.push(byte);
            }
        }
    }
    emit(&pending, &mut f);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(done: u32, total: u32, building: &str) -> BuildProgress {
        BuildProgress { done, total, building: building.into() }
    }

    #[test]
    fn reads_the_progress_line_as_cargo_prints_it() {
        assert_eq!(
            parse_progress(
                "    Building [=======================> ] 254/257: reign_setup                 "
            ),
            Some(progress(254, 257, "reign_setup"))
        );
        // Several crates, cut short with an ellipsis.
        assert_eq!(
            parse_progress(
                "    Building [=======================> ] 255/257: reign-hub(bin), reign-relea…"
            ),
            Some(progress(255, 257, "reign-hub(bin), reign-relea…"))
        );
        // The start: nothing done, no bar yet, no names.
        assert_eq!(
            parse_progress("    Building [>                        ] 0/257"),
            Some(progress(0, 257, ""))
        );
        assert_eq!(parse_progress("Building [===] 1/2: a"), Some(progress(1, 2, "a")));
    }

    #[test]
    fn other_lines_are_not_progress() {
        for line in [
            "   Compiling reign_setup v0.2.0 (C:\\Users\\x)",
            "    Finished `dev` profile [optimized + debuginfo] target(s) in 4.64s",
            "warning: unused variable: `x`",
            "error[E0425]: cannot find value `y` in this scope",
            "Building the thing",
            "Building [=>] soon",
            "Building [=>] a/b: c",
            "",
        ] {
            assert_eq!(parse_progress(line), None, "{line:?}");
        }
    }

    #[test]
    fn the_fraction_and_count() {
        assert_eq!(progress(1, 4, "").fraction(), 0.25);
        assert_eq!(progress(0, 0, "").fraction(), 0.0, "no total yet");
        assert_eq!(progress(9, 4, "").fraction(), 1.0, "never past full");
        assert_eq!(progress(254, 257, "").count(), "254 of 257");
    }

    /// What cargo really wrote building the hub, byte for byte.
    const REAL: &[u8] = b"   Compiling reign_setup v0.2.0 (C:\\Users\\cbfox\\reign-engine\\installer)\n    Building [=======================> ] 254/257: reign_setup                 \r   Compiling reign_hub v0.2.0 (C:\\Users\\cbfox\\reign-engine\\hub)\n    Building [=======================> ] 255/257: reign-hub(bin), reign-relea\xe2\x80\xa6\r    Building [=======================> ] 256/257: reign-hub(bin)              \r    Finished `dev` profile [optimized + debuginfo] target(s) in 4.64s\n";

    fn collect(bytes: &[u8]) -> Vec<Output> {
        let mut out = Vec::new();
        read_output(bytes, |o| out.push(o));
        out
    }

    #[test]
    fn splits_real_cargo_output_into_lines_and_progress() {
        let out = collect(REAL);
        assert_eq!(out.len(), 6, "{out:#?}");
        assert!(
            matches!(&out[0], Output::Line(l) if l.starts_with("   Compiling reign_setup")),
            "{:?}",
            out[0]
        );
        assert_eq!(out[1], Output::Progress(progress(254, 257, "reign_setup")));
        assert!(matches!(&out[2], Output::Line(l) if l.contains("Compiling reign_hub")));
        assert_eq!(out[3], Output::Progress(progress(255, 257, "reign-hub(bin), reign-relea…")));
        assert_eq!(out[4], Output::Progress(progress(256, 257, "reign-hub(bin)")));
        assert!(matches!(&out[5], Output::Line(l) if l.contains("Finished")));
    }

    #[test]
    fn line_ends_of_every_kind_and_no_final_one() {
        let out = collect(b"one\r\ntwo\n\n\rthree\r   Building [=>] 1/2: x");
        assert_eq!(
            out,
            [
                Output::Line("one".into()),
                Output::Line("two".into()),
                Output::Line("three".into()),
                Output::Progress(progress(1, 2, "x")),
            ]
        );
        assert!(collect(b"").is_empty());
        assert!(collect(b"\r\n\r\n  \n").is_empty());
    }

    /// The same bytes in tiny pieces (a slow pipe, a read boundary in the
    /// middle of a line or of a multi-byte character) give the same result.
    #[test]
    fn reads_in_any_sized_pieces() {
        struct Trickle<'a>(&'a [u8], usize);
        impl Read for Trickle<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.1.min(self.0.len()).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let whole = collect(REAL);
        for size in [1, 2, 3, 7, 50] {
            let mut out = Vec::new();
            read_output(Trickle(REAL, size), |o| out.push(o));
            assert_eq!(out, whole, "pieces of {size}");
        }
    }

    #[test]
    fn bytes_that_are_not_utf8_never_stop_the_reader() {
        let out = collect(b"bad \xff\xfe bytes\nnext\n");
        assert_eq!(out.len(), 2);
        assert!(
            matches!(&out[0], Output::Line(l) if l.starts_with("bad ") && l.ends_with(" bytes"))
        );
        assert_eq!(out[1], Output::Line("next".into()));
    }
}
