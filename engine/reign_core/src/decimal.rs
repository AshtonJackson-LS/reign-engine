//! Exact decimal text for [`Fx`], for scene files and editors.
//!
//! Every 24.8 fixed-point value has a finite decimal expansion (at most
//! eight fractional digits, since 1/256 = 0.00390625), so [`Fx`]'s
//! `Display` writes it exactly and [`Fx::parse_decimal`] reads it back to
//! the same bits. Decimals that fall between two representable values round
//! to the nearest, ties away from zero. Parsing uses integer arithmetic only:
//! no `f32` or `f64` is involved, so every platform reads a scene file the
//! same way.

use std::fmt;

use crate::Fx;

/// Why text isn't a fixed-point number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseFxError {
    /// Not of the form `-12.375`: an optional sign, digits, and optionally a
    /// point followed by more digits.
    Invalid(String),
    /// Outside the range a 24.8 number can hold.
    OutOfRange(String),
}

impl fmt::Display for ParseFxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseFxError::Invalid(s) => write!(f, "{s:?} is not a decimal number like -12.375"),
            ParseFxError::OutOfRange(s) => write!(f, "{s:?} is out of range for a 24.8 number"),
        }
    }
}

impl std::error::Error for ParseFxError {}

/// Fractional digits kept while parsing. Extra digits are dropped, which is
/// still exact: rounding boundaries (multiples of 1/512) need at most nine
/// decimal digits, so truncating past twenty never moves a value across one.
/// It keeps `whole * 10^digits * 256` inside an `i128`.
const MAX_DIGITS: usize = 20;

impl Fx {
    /// Parses decimal text such as `"120"`, `"-3.5"` or `"0.00390625"`
    /// exactly, rounding to the nearest 1/256 with ties away from zero.
    pub fn parse_decimal(text: &str) -> Result<Fx, ParseFxError> {
        let invalid = || ParseFxError::Invalid(text.to_owned());
        let (negative, digits) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
        let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if whole.is_empty() && frac.is_empty()
            || !all_digits(whole)
            || !all_digits(frac)
            || digits.ends_with('.')
        {
            return Err(invalid());
        }
        let out_of_range = || ParseFxError::OutOfRange(text.to_owned());
        // value = whole + frac / 10^k; raw = value * 256, rounded.
        let whole = whole.trim_start_matches('0');
        if whole.len() > 12 {
            return Err(out_of_range());
        }
        let whole: i128 =
            if whole.is_empty() { 0 } else { whole.parse().map_err(|_| invalid())? };
        let frac = &frac[..frac.len().min(MAX_DIGITS)];
        let scale = 10_i128.pow(frac.len() as u32);
        let frac: i128 = if frac.is_empty() { 0 } else { frac.parse().map_err(|_| invalid())? };
        let numerator = (whole * scale + frac) << Fx::FRAC_BITS;
        // Round half away from zero (on the magnitude, before the sign).
        let raw = (numerator + scale / 2) / scale;
        let raw = if negative { -raw } else { raw };
        i32::try_from(raw).map(Fx::from_raw).map_err(|_| out_of_range())
    }
}

impl fmt::Display for Fx {
    /// The exact value in decimal, with no trailing zeros: `120`, `-3.5`,
    /// `0.00390625`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let raw = self.raw() as i64;
        let magnitude = raw.unsigned_abs();
        let whole = magnitude >> Fx::FRAC_BITS;
        let frac = magnitude & ((1 << Fx::FRAC_BITS) - 1);
        let sign = if raw < 0 { "-" } else { "" };
        if frac == 0 {
            return write!(f, "{sign}{whole}");
        }
        // frac / 256 = frac * 390625 / 10^8, exactly.
        let digits = format!("{:08}", frac * 390_625);
        write!(f, "{sign}{whole}.{}", digits.trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Fx {
        Fx::parse_decimal(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn whole_and_exact_fractions() {
        assert_eq!(parse("0"), Fx::ZERO);
        assert_eq!(parse("-0"), Fx::ZERO);
        assert_eq!(parse("120"), Fx::from_px(120));
        assert_eq!(parse("+7"), Fx::from_px(7));
        assert_eq!(parse("-3.5"), Fx::from_raw(-896));
        assert_eq!(parse(".5"), Fx::from_raw(128));
        assert_eq!(parse("0.00390625"), Fx::from_raw(1));
        assert_eq!(parse("-0.00390625"), Fx::from_raw(-1));
        assert_eq!(parse("007.250"), Fx::from_raw(7 * 256 + 64));
    }

    #[test]
    fn inexact_decimals_round_to_nearest_ties_away_from_zero() {
        // 0.1 * 256 = 25.6 -> 26.
        assert_eq!(parse("0.1"), Fx::from_raw(26));
        assert_eq!(parse("-0.1"), Fx::from_raw(-26));
        // Exactly half a step: away from zero.
        assert_eq!(parse("0.001953125"), Fx::from_raw(1));
        assert_eq!(parse("-0.001953125"), Fx::from_raw(-1));
        // Just under half a step rounds down.
        assert_eq!(parse("0.001953124999999999999999"), Fx::ZERO);
        // Digits past what's kept only affect rounding.
        assert_eq!(parse(&format!("1.{}", "9".repeat(60))), Fx::from_px(2));
    }

    #[test]
    fn range_limits() {
        assert_eq!(parse("8388607.99609375"), Fx::from_raw(i32::MAX));
        assert_eq!(parse("-8388608"), Fx::from_raw(i32::MIN));
        assert!(matches!(Fx::parse_decimal("8388608"), Err(ParseFxError::OutOfRange(_))));
        assert!(matches!(Fx::parse_decimal("-8388608.01"), Err(ParseFxError::OutOfRange(_))));
        assert!(matches!(
            Fx::parse_decimal("99999999999999999999999"),
            Err(ParseFxError::OutOfRange(_))
        ));
    }

    #[test]
    fn rejects_what_isnt_a_plain_decimal() {
        for bad in ["", "-", ".", "1.", "1e3", "1.5.2", " 1", "1 ", "0x10", "nan", "--1", "1,5"] {
            assert!(
                matches!(Fx::parse_decimal(bad), Err(ParseFxError::Invalid(_))),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn display_is_exact_and_parses_back_to_the_same_bits() {
        assert_eq!(Fx::from_px(120).to_string(), "120");
        assert_eq!(Fx::from_raw(-896).to_string(), "-3.5");
        assert_eq!(Fx::from_raw(1).to_string(), "0.00390625");
        assert_eq!(Fx::from_raw(-1).to_string(), "-0.00390625");
        assert_eq!(Fx::from_raw(i32::MIN).to_string(), "-8388608");
        let mut raw = i32::MIN;
        loop {
            let fx = Fx::from_raw(raw);
            assert_eq!(parse(&fx.to_string()), fx, "{raw}");
            match raw.checked_add(65_537) {
                Some(next) => raw = next,
                None => break,
            }
        }
        for raw in -1024..=1024 {
            assert_eq!(parse(&Fx::from_raw(raw).to_string()), Fx::from_raw(raw));
        }
        assert_eq!(parse(&Fx::from_raw(i32::MAX).to_string()), Fx::from_raw(i32::MAX));
    }
}
