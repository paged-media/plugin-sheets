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

//! Numeric-section rendering (spec §9; ECMA-376 §18.8.31): digit
//! placeholders (`0`/`#`/`?`), the decimal point, `,` thousands grouping,
//! trailing-`,` thousand-scaling, `%` scaling, scientific (`E+00`), and
//! Excel display rounding (half away from zero at the section's decimal
//! count). Literals (quoted, escaped, and the always-literal punctuation)
//! interleave verbatim.

use crate::sections::{Section, Token};
use std::fmt::Write as _;

/// Render the magnitude-or-signed value `x` through a numeric `section`. The
/// caller has already selected the section and decided `force_minus` (true
/// only for the 1-section-applied-to-negative case). An empty section (no
/// tokens) means `General` — handled by the caller, never reached here with
/// real General intent, but we fall back to a plain decimal to be safe.
pub fn render_number(x: f64, section: &Section, force_minus: bool) -> String {
    // Scientific path if the section has an exponent token.
    if section
        .tokens
        .iter()
        .any(|t| matches!(t, Token::Exponent { .. }))
    {
        return render_scientific(x, section, force_minus);
    }

    // ---- Apply scaling: percent (×100 per %) and trailing-comma (/1000). ----
    let percent_count = section
        .tokens
        .iter()
        .filter(|t| matches!(t, Token::Percent))
        .count();
    let scale_commas = trailing_comma_scale(&section.tokens);

    let mut value = x.abs();
    for _ in 0..percent_count {
        value *= 100.0;
    }
    for _ in 0..scale_commas {
        value /= 1000.0;
    }

    let decimals = section.decimals();
    let grouped = has_grouping(&section.tokens);

    // Round to the section's decimal count, half away from zero.
    let rounded = round_half_away(value, decimals);

    // Split into integer and fractional digit strings.
    let (int_digits, frac_digits) = split_digits(rounded, decimals);

    // Count integer-side placeholders to know min width and where to put the
    // first digit.
    let int_specs = collect_int_specs(&section.tokens);
    let frac_specs = collect_frac_specs(&section.tokens);

    let int_rendered = render_int_part(&int_digits, &int_specs, grouped);
    let frac_rendered = render_frac_part(&frac_digits, &frac_specs);

    // Now weave literals + the assembled number into the token order.
    let mut out = String::new();
    if force_minus {
        out.push('-');
    }

    let mut int_written = false;
    let mut frac_written = false;
    let mut seen_decimal = false;
    for t in &section.tokens {
        match t {
            Token::Literal(s) => out.push_str(s),
            Token::Percent => out.push('%'),
            Token::DecimalPoint => {
                seen_decimal = true;
                // Emit the integer block right before the point if not yet.
                if !int_written {
                    out.push_str(&int_rendered);
                    int_written = true;
                }
                if !frac_rendered.is_empty() || frac_has_forced(&frac_specs) {
                    out.push('.');
                }
            }
            Token::DigitZero | Token::DigitHash | Token::DigitSpace => {
                if seen_decimal {
                    if !frac_written {
                        out.push_str(&frac_rendered);
                        frac_written = true;
                    }
                } else if !int_written {
                    out.push_str(&int_rendered);
                    int_written = true;
                }
            }
            Token::ThousandsSep => {
                // Grouping/scaling already handled; emit nothing here.
            }
            _ => {}
        }
    }
    // If the section had no placeholder at all (pure literal), nothing more.
    out
}

/// Trailing-comma scale count: count `,` tokens that appear immediately
/// after the last digit placeholder with no further digit placeholder. Each
/// such comma divides by 1000.
fn trailing_comma_scale(tokens: &[Token]) -> usize {
    // Find index of the last digit placeholder.
    let last_digit = tokens
        .iter()
        .rposition(|t| matches!(t, Token::DigitZero | Token::DigitHash | Token::DigitSpace));
    let Some(last) = last_digit else {
        return 0;
    };
    // Count consecutive ThousandsSep immediately following `last`, allowing
    // only commas (no decimal point) after it.
    let mut count = 0;
    for t in &tokens[last + 1..] {
        match t {
            Token::ThousandsSep => count += 1,
            // A decimal point after digits cancels trailing-scale (the comma
            // would be inside the fraction context). Stop on anything else
            // that is a digit/decimal.
            Token::DecimalPoint => return 0,
            _ => {}
        }
    }
    count
}

/// True when a `,` appears strictly between two digit placeholders (i.e. it
/// is a grouping comma, not a scale comma).
fn has_grouping(tokens: &[Token]) -> bool {
    let mut seen_digit_before = false;
    for (i, t) in tokens.iter().enumerate() {
        match t {
            Token::DigitZero | Token::DigitHash | Token::DigitSpace => {
                seen_digit_before = true;
            }
            Token::ThousandsSep if seen_digit_before => {
                // Grouping only if a digit placeholder also follows.
                if tokens[i + 1..]
                    .iter()
                    .any(|u| matches!(u, Token::DigitZero | Token::DigitHash | Token::DigitSpace))
                {
                    return true;
                }
            }
            Token::DecimalPoint => break,
            _ => {}
        }
    }
    false
}

/// Round `v >= 0` to `decimals` places, half away from zero.
fn round_half_away(v: f64, decimals: usize) -> f64 {
    let factor = 10f64.powi(decimals as i32);
    let scaled = v * factor;
    (scaled + 0.5).floor() / factor
}

/// Split a non-negative rounded value into (integer-digit string,
/// fraction-digit string of exactly `decimals` chars).
fn split_digits(v: f64, decimals: usize) -> (String, String) {
    // Format with fixed decimals; Rust's formatter rounds half-to-even, but
    // we already rounded half-away, so re-formatting the rounded value at the
    // same precision is stable.
    let s = format!("{v:.decimals$}");
    match s.split_once('.') {
        Some((i, f)) => (strip_leading_zeros(i), f.to_string()),
        None => (strip_leading_zeros(&s), String::new()),
    }
}

fn strip_leading_zeros(s: &str) -> String {
    let t = s.trim_start_matches('0');
    if t.is_empty() {
        String::new()
    } else {
        t.to_string()
    }
}

/// Integer-side placeholder specs in left-to-right order (the kinds before
/// the decimal point).
fn collect_int_specs(tokens: &[Token]) -> Vec<DigitKind> {
    let mut out = Vec::new();
    for t in tokens {
        match t {
            Token::DigitZero => out.push(DigitKind::Zero),
            Token::DigitHash => out.push(DigitKind::Hash),
            Token::DigitSpace => out.push(DigitKind::Space),
            Token::DecimalPoint => break,
            _ => {}
        }
    }
    out
}

/// Fraction-side placeholder specs after the decimal point.
fn collect_frac_specs(tokens: &[Token]) -> Vec<DigitKind> {
    let mut out = Vec::new();
    let mut after = false;
    for t in tokens {
        match t {
            Token::DecimalPoint => after = true,
            Token::DigitZero if after => out.push(DigitKind::Zero),
            Token::DigitHash if after => out.push(DigitKind::Hash),
            Token::DigitSpace if after => out.push(DigitKind::Space),
            _ => {}
        }
    }
    out
}

#[derive(Copy, Clone, PartialEq)]
enum DigitKind {
    Zero,
    Hash,
    Space,
}

fn frac_has_forced(frac: &[DigitKind]) -> bool {
    frac.iter()
        .any(|d| matches!(d, DigitKind::Zero | DigitKind::Space))
}

/// Render the integer part: pad to the count of forced placeholders, add
/// grouping commas if requested. `digits` has no leading zeros ("" for 0).
fn render_int_part(digits: &str, specs: &[DigitKind], grouped: bool) -> String {
    // Minimum digits = count of Zero placeholders (and Space pads with space,
    // but Excel treats `?` like a forced-but-blank slot — we render a space
    // only when there is no digit for that slot).
    let zero_min = specs.iter().filter(|d| **d == DigitKind::Zero).count();
    let space_min = specs.iter().filter(|d| **d == DigitKind::Space).count();

    let mut body = digits.to_string();
    // Pad leading zeros up to zero_min.
    if body.len() < zero_min {
        let pad = zero_min - body.len();
        let mut p = "0".repeat(pad);
        p.push_str(&body);
        body = p;
    }
    // Pad with spaces up to (zero_min + space_min) for `?` slots.
    let want = zero_min + space_min;
    if body.len() < want {
        let pad = want - body.len();
        let mut p = " ".repeat(pad);
        p.push_str(&body);
        body = p;
    }
    if grouped {
        group_thousands(&body)
    } else {
        body
    }
}

/// Insert `,` every three digits from the right, ignoring leading spaces.
fn group_thousands(s: &str) -> String {
    // Separate any leading spaces.
    let lead_spaces = s.len() - s.trim_start_matches(' ').len();
    let (spaces, digits) = s.split_at(lead_spaces);
    if digits.is_empty() {
        return s.to_string();
    }
    let bytes: Vec<char> = digits.chars().collect();
    let mut out = String::new();
    let n = bytes.len();
    for (i, ch) in bytes.iter().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*ch);
    }
    format!("{spaces}{out}")
}

/// Render the fraction part. `digits` is exactly `frac.len()`-wide already
/// (from `split_digits` with the right `decimals`). `#` trailing slots drop
/// trailing zeros; `0` keeps them; `?` keeps a space for a dropped digit.
fn render_frac_part(digits: &str, specs: &[DigitKind]) -> String {
    if specs.is_empty() {
        return String::new();
    }
    let dch: Vec<char> = digits.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        let d = dch.get(i).copied().unwrap_or('0');
        out.push(d);
        let _ = spec;
    }
    // Trim trailing positions whose placeholder is Hash and whose digit is 0.
    // Walk from the right.
    let mut end = out.len();
    while end > 0 {
        let idx = end - 1;
        match specs[idx] {
            DigitKind::Hash if out[idx] == '0' => {
                end -= 1;
            }
            DigitKind::Space if out[idx] == '0' => {
                out[idx] = ' ';
                end -= 1; // keep scanning but the slot becomes blank
            }
            _ => break,
        }
    }
    // For Space placeholders, the trimmed trailing ones already became ' ';
    // include them up to the last non-trimmed OR last space slot.
    let last_space = specs
        .iter()
        .rposition(|d| matches!(d, DigitKind::Space))
        .map(|p| p + 1)
        .unwrap_or(0);
    let keep = end.max(last_space);
    out[..keep].iter().collect()
}

/// Scientific rendering `m...E±NN` (spec §9). The section's pre-`E`
/// placeholders set the mantissa decimal count; the `E+`/`E-` token sets the
/// exponent-sign style; placeholders after `E` set the exponent width.
fn render_scientific(x: f64, section: &Section, force_minus: bool) -> String {
    let exp_idx = section
        .tokens
        .iter()
        .position(|t| matches!(t, Token::Exponent { .. }))
        .unwrap();
    let plus = matches!(section.tokens[exp_idx], Token::Exponent { plus: true });

    // Mantissa decimals = digit placeholders after the decimal point but
    // before the exponent token.
    let mant_decimals = section.tokens[..exp_idx]
        .iter()
        .scan(false, |after, t| {
            if t == &Token::DecimalPoint {
                *after = true;
            }
            Some((*after, t))
        })
        .filter(|(after, t)| {
            *after && matches!(t, Token::DigitZero | Token::DigitHash | Token::DigitSpace)
        })
        .count();

    // Exponent width = digit placeholders after the exponent token.
    let exp_width = section.tokens[exp_idx + 1..]
        .iter()
        .filter(|t| matches!(t, Token::DigitZero | Token::DigitHash | Token::DigitSpace))
        .count()
        .max(1);

    let neg = x < 0.0;
    let mut out = String::new();
    if force_minus || neg {
        out.push('-');
    }

    let mag = x.abs();
    if mag == 0.0 {
        let _ = write!(out, "{:.*}", mant_decimals, 0.0);
        out.push('E');
        out.push(if plus { '+' } else { '-' });
        let _ = write!(out, "{:0width$}", 0, width = exp_width);
        return out;
    }

    let mut e = mag.log10().floor() as i32;
    let mut mant = mag / 10f64.powi(e);
    // Rounding the mantissa can push it to 10.x; renormalize.
    let rm = round_half_away(mant, mant_decimals);
    if rm >= 10.0 {
        e += 1;
        mant = mag / 10f64.powi(e);
    } else {
        mant = rm;
    }
    let mant = round_half_away(mant, mant_decimals);

    let _ = write!(out, "{mant:.mant_decimals$}");
    out.push('E');
    let esign = if e < 0 {
        '-'
    } else if plus {
        '+'
    } else {
        // E- style with a non-negative exponent shows no sign.
        '\0'
    };
    if esign != '\0' {
        out.push(esign);
    }
    let _ = write!(out, "{:0width$}", e.unsigned_abs(), width = exp_width);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::compile;

    fn fmt(code: &str, x: f64) -> String {
        let f = compile(code).unwrap();
        let (sec, force) = f.select_numeric(x);
        render_number(x, sec, force)
    }

    #[test]
    fn fixed_decimals() {
        assert_eq!(fmt("0.00", 1.5), "1.50");
        assert_eq!(fmt("0.00", 1.0), "1.00");
        assert_eq!(fmt("0", 1.4), "1");
        assert_eq!(fmt("0", 1.5), "2"); // half away from zero
    }

    #[test]
    fn optional_digits() {
        assert_eq!(fmt("#.##", 1.5), "1.5");
        assert_eq!(fmt("#.##", 1.0), "1");
        assert_eq!(fmt("0.##", 0.5), "0.5");
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(fmt("#,##0", 1234.0), "1,234");
        assert_eq!(fmt("#,##0", 1234567.0), "1,234,567");
        assert_eq!(fmt("#,##0.00", 1234.5), "1,234.50");
    }

    #[test]
    fn thousand_scaling() {
        assert_eq!(fmt("0,", 12000.0), "12");
        assert_eq!(fmt("0.0,", 12500.0), "12.5");
        assert_eq!(fmt("0,,", 12000000.0), "12");
    }

    #[test]
    fn percent_scaling() {
        assert_eq!(fmt("0%", 0.5), "50%");
        assert_eq!(fmt("0.00%", 0.1234), "12.34%");
    }

    #[test]
    fn negative_single_section() {
        assert_eq!(fmt("0.00", -1.5), "-1.50");
    }

    #[test]
    fn scientific() {
        assert_eq!(fmt("0.00E+00", 12345.0), "1.23E+04");
        assert_eq!(fmt("0.00E+00", 0.0001234), "1.23E-04");
        assert_eq!(fmt("0E+0", 12345.0), "1E+4");
    }

    #[test]
    fn leading_zero_pad() {
        assert_eq!(fmt("000", 7.0), "007");
        assert_eq!(fmt("00.0", 7.5), "07.5");
    }
}
