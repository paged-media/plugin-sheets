/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! The `General` format and number->text coercion (registry ruling
//! `sheet.format.general`, spec §9).
//!
//! ## The T0 `General` ruling (precise, adopted)
//!
//! Excel's `General` is "shortest representation that round-trips, within
//! 15-significant-digit display precision, switching to scientific only for
//! extreme magnitudes." We adopt this **exact** T0 spelling:
//!
//! 1. Non-numeric values map directly: `Bool` -> `TRUE`/`FALSE`, `Error` ->
//!    its [`CellError::as_str`], `Empty` -> `""`, `Text` passes through.
//! 2. For a finite number `x`:
//!    - `0` (and `-0`) -> `"0"`.
//!    - Round to **15 significant decimal digits first** (Excel's display
//!      precision), *then* produce the shortest decimal string that parses
//!      back to that rounded value (via `ryu`'s shortest round-trip, then
//!      trimmed).
//!    - Let `e = floor(log10(|x|))`. **Plain decimal** notation when
//!      `-7 <= e < 21`; otherwise **scientific** (`1.5E-08` style): a
//!      mantissa of up to 15 significant digits, `E+`/`E-`, and at least two
//!      exponent digits.
//!    - Integers print with no decimal point.
//! 3. Non-finite (`NaN`, `±inf`) -> `#NUM!` (Excel surfaces overflow there).
//!
//! This is the single source of truth for "what a bare number looks like"
//! and is reused by [`format_general`] (General cells) and by the numeric
//! `@`/text-coercion path in [`crate::number`].

use sheet_core::CellValue;

/// Format any [`CellValue`] under `General` (spec §9; ruling
/// `sheet.format.general`). The number path is [`general_number`].
pub fn format_general(v: &CellValue) -> String {
    match v {
        CellValue::Empty => String::new(),
        CellValue::Text(t) => t.to_string(),
        CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        CellValue::Error(e) => e.as_str().to_string(),
        CellValue::Number(n) => general_number(*n),
    }
}

/// `General` rendering of a finite f64 per the module ruling. Used directly
/// for number->text coercion (e.g. TEXT(x,"General"), `&` concatenation).
pub fn general_number(x: f64) -> String {
    if !x.is_finite() {
        return sheet_core::CellError::Num.as_str().to_string();
    }
    if x == 0.0 {
        return "0".to_string();
    }

    // Round to 15 significant digits (Excel display precision).
    let r = round_sig(x, 15);
    if r == 0.0 {
        return "0".to_string();
    }

    let neg = r < 0.0;
    let mag = r.abs();
    let e = mag.log10().floor() as i32;

    let body = if (-7..21).contains(&e) {
        plain_decimal(mag)
    } else {
        scientific(mag, e)
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// Shortest plain-decimal string for a positive, 15-sig-rounded magnitude.
/// Integers drop the point; fractions keep only the shortest round-trip
/// digits (trailing zeros trimmed).
fn plain_decimal(mag: f64) -> String {
    // ryu gives the shortest round-trip decimal; normalize its shape.
    let mut s = ryu::Buffer::new().format(mag).to_string();
    // ryu may emit exponential form for some magnitudes in our plain range
    // (it never does for [-7, 21) magnitudes we reach here, but be safe).
    if s.contains('e') || s.contains('E') {
        s = expand_ryu_exp(&s);
    }
    // Trim a trailing ".0" so integers print bare.
    if let Some(dot) = s.find('.') {
        let frac = &s[dot + 1..];
        if frac.chars().all(|c| c == '0') {
            s.truncate(dot);
        }
    }
    s
}

/// Scientific form `m.mmmE±NN` for a positive magnitude with exponent `e`.
fn scientific(mag: f64, e: i32) -> String {
    // Dividing by 10^e reintroduces float error (e.g. 1.5e-8 -> 1.4999…998);
    // round the mantissa back to 15 sig digits to recover the clean value.
    let mantissa = round_sig(mag / 10f64.powi(e), 15);
    // Shortest round-trip mantissa, capped at 15 sig digits.
    let mut m = ryu::Buffer::new().format(mantissa).to_string();
    if m.contains('e') || m.contains('E') {
        m = expand_ryu_exp(&m);
    }
    if let Some(dot) = m.find('.') {
        let frac = &m[dot + 1..];
        if frac.chars().all(|c| c == '0') {
            m.truncate(dot);
        }
    }
    let sign = if e < 0 { '-' } else { '+' };
    format!("{m}E{sign}{:02}", e.unsigned_abs())
}

/// Expand a ryu exponential string (e.g. `1.5e-8`) into plain decimal.
/// Only reached on the rare ryu-chooses-exp path inside the plain range.
fn expand_ryu_exp(s: &str) -> String {
    let val: f64 = s.parse().unwrap_or(0.0);
    // Reconstruct without exponent via a wide fixed format, then trim.
    let mut out = format!("{val:.*}", 17);
    if let Some(dot) = out.find('.') {
        // Trim trailing zeros but keep at least one digit before the dot.
        while out.ends_with('0') {
            out.pop();
        }
        if out.ends_with('.') {
            out.truncate(dot);
        }
    }
    out
}

/// Round `x` to `sig` significant decimal digits, half away from zero.
fn round_sig(x: f64, sig: i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let d = (sig - 1) - x.abs().log10().floor() as i32; // decimal places
    let factor = 10f64.powi(d);
    let scaled = x * factor;
    // half-away-from-zero
    let rounded = if scaled >= 0.0 {
        (scaled + 0.5).floor()
    } else {
        (scaled - 0.5).ceil()
    };
    rounded / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(x: f64) -> String {
        general_number(x)
    }

    #[test]
    fn integers_have_no_point() {
        assert_eq!(g(0.0), "0");
        assert_eq!(g(1.0), "1");
        assert_eq!(g(-42.0), "-42");
        assert_eq!(g(1000.0), "1000");
    }

    #[test]
    fn shortest_fractions() {
        assert_eq!(g(1.5), "1.5");
        assert_eq!(g(0.1), "0.1");
        assert_eq!(g(-0.25), "-0.25");
    }

    #[test]
    fn fifteen_sig_cap() {
        // 1e15 is the largest exactly-representable display integer.
        assert_eq!(g(1e15), "1000000000000000");
        // Beyond 15 sig digits Excel still shows the integer up to e<21.
        assert_eq!(g(123456789012345.0), "123456789012345");
    }

    #[test]
    fn scientific_extremes() {
        assert_eq!(g(1e21), "1E+21");
        assert_eq!(g(1.5e-8), "1.5E-08");
        assert_eq!(g(-2e-10), "-2E-10");
        assert_eq!(g(1e-7), "0.0000001"); // e == -7 stays plain
    }

    #[test]
    fn non_numeric() {
        assert_eq!(format_general(&CellValue::Empty), "");
        assert_eq!(format_general(&CellValue::Bool(true)), "TRUE");
        assert_eq!(format_general(&CellValue::Bool(false)), "FALSE");
        assert_eq!(format_general(&CellValue::from("hi")), "hi");
        assert_eq!(
            format_general(&CellValue::Error(sheet_core::CellError::Div0)),
            "#DIV/0!"
        );
    }

    #[test]
    fn non_finite_is_num_error() {
        assert_eq!(g(f64::NAN), "#NUM!");
        assert_eq!(g(f64::INFINITY), "#NUM!");
    }
}
