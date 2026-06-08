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

use crate::locale::LocaleData;
use crate::sections::{FractionSpec, Section, Token};
use std::fmt::Write as _;

/// Render the magnitude-or-signed value `x` through a numeric `section`. The
/// caller has already selected the section and decided `force_minus` (true
/// only for the 1-section-applied-to-negative case). `loc` supplies the
/// locale's decimal-point and group (thousands) separators — en-US (`.`/`,`)
/// is the default and keeps existing output byte-identical. An empty section
/// (no tokens) means `General` — handled by the caller, never reached here
/// with real General intent, but we fall back to a plain decimal to be safe.
pub fn render_number(x: f64, section: &Section, force_minus: bool, loc: &LocaleData) -> String {
    // Fraction path if the section has a fraction token (`# ?/?`).
    if let Some(Token::Fraction(spec)) = section
        .tokens
        .iter()
        .find(|t| matches!(t, Token::Fraction(_)))
    {
        return render_fraction(x, section, *spec, force_minus, loc);
    }
    // Scientific path if the section has an exponent token.
    if section
        .tokens
        .iter()
        .any(|t| matches!(t, Token::Exponent { .. }))
    {
        return render_scientific(x, section, force_minus, loc);
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

    let int_rendered = render_int_part(&int_digits, &int_specs, grouped, loc);
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
            // Repeat-fill char (`*x`): T0 emits it ONCE (no column width;
            // ruling `sheet.format.padding`).
            Token::Fill(c) => out.push(*c),
            Token::Percent => out.push('%'),
            Token::DecimalPoint => {
                seen_decimal = true;
                // Emit the integer block right before the point if not yet.
                if !int_written {
                    out.push_str(&int_rendered);
                    int_written = true;
                }
                if !frac_rendered.is_empty() || frac_has_forced(&frac_specs) {
                    out.push_str(loc.decimal);
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
/// grouping separators if requested. `digits` has no leading zeros ("" for
/// 0). `loc` supplies the locale group separator (en-US `,`).
fn render_int_part(digits: &str, specs: &[DigitKind], grouped: bool, loc: &LocaleData) -> String {
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
        group_thousands(&body, loc.group)
    } else {
        body
    }
}

/// Insert the locale group separator every three digits from the right,
/// ignoring leading spaces (en-US `,`, de-DE `.`).
fn group_thousands(s: &str, sep: &str) -> String {
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
            out.push_str(sep);
        }
        out.push(*ch);
    }
    format!("{spaces}{out}")
}

/// Replace the ASCII `.` produced by Rust's float formatter with the locale's
/// decimal separator. For en-US (`.`) this is a no-op, keeping output
/// byte-identical; only the scientific mantissa path uses Rust's formatter
/// directly (the main numeric path emits the separator token by token).
fn localize_point(s: &str, loc: &LocaleData) -> String {
    if loc.decimal == "." {
        s.to_string()
    } else {
        s.replacen('.', loc.decimal, 1)
    }
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
/// exponent-sign style; placeholders after `E` set the exponent width. The
/// mantissa's decimal separator is localized (en-US `.`); the exponent
/// `E`/sign/digits stay locale-neutral.
fn render_scientific(x: f64, section: &Section, force_minus: bool, loc: &LocaleData) -> String {
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
        out.push_str(&localize_point(&format!("{:.*}", mant_decimals, 0.0), loc));
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

    out.push_str(&localize_point(&format!("{mant:.mant_decimals$}"), loc));
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

/// Render a fraction format (`# ?/?`, `# ??/??`, `# ?/16`; spec §9, ruling
/// `sheet.format.fractions`).
///
/// The section's tokens look like `[integer placeholders] Fraction(spec)
/// [trailing literals]`. We split `|x|` into the integer part (rendered
/// through the leading digit placeholders) and a remaining fractional part,
/// fit the best `n/d` for that fraction within the spec's denominator-digit
/// budget (or to the fixed denominator), and weave it back into the literal
/// order. When there is NO integer placeholder the value is shown as an
/// improper fraction (`n/d` with `n >= d` allowed).
///
/// ## Excel rulings adopted here
/// - Best fit minimizes `|frac - n/d|` over `1 <= d <= 10^den_digits - 1`,
///   preferring the SMALLER denominator on ties (Excel's behaviour).
/// - A fraction that fits exactly to `0/1` collapses: with an integer part the
///   fraction slot blanks to spaces (a `?` slot is space-padded); a pure
///   fraction shows `0` (via the `0/1` -> the renderer keeps `n=0,d=1`).
/// - The integer part uses Excel display rounding only through the carry from
///   the fraction (e.g. `2 24/25` of 2.96 with `?/?` rounds the fraction so it
///   does not exceed 1, carrying into the integer).
fn render_fraction(
    x: f64,
    section: &Section,
    spec: FractionSpec,
    force_minus: bool,
    loc: &LocaleData,
) -> String {
    let neg = x < 0.0;
    let mag = x.abs();

    let has_int_part = section.tokens.iter().any(|t| {
        // A digit placeholder that comes BEFORE the fraction token is the
        // whole-number part.
        matches!(t, Token::DigitZero | Token::DigitHash | Token::DigitSpace)
    });

    let (mut whole, frac) = if has_int_part {
        (mag.floor(), mag - mag.floor())
    } else {
        (0.0, mag)
    };

    let max_den: u32 = if let Some(d) = spec.fixed {
        d
    } else {
        (10u32.pow(spec.den_digits as u32)).saturating_sub(1).max(1)
    };

    let (mut num, den) = match spec.fixed {
        Some(d) => {
            // Fixed denominator: round numerator to nearest.
            let n = (frac * d as f64).round() as u32;
            (n, d)
        }
        None => best_fraction(frac, max_den),
    };

    // A numerator that rounds up to the denominator carries into the whole part
    // (e.g. 2.99 with `?/?` -> 3, fraction 0/1).
    if den > 0 && num >= den {
        whole += (num / den) as f64;
        num %= den;
    }

    // Render the integer placeholders (left of the fraction).
    let int_specs = collect_int_specs(&section.tokens);
    let int_digits = if has_int_part {
        let s = format!("{whole:.0}");
        strip_leading_zeros(&s)
    } else {
        String::new()
    };
    let grouped = has_grouping(&section.tokens);
    let int_rendered = render_int_part(&int_digits, &int_specs, grouped, loc);

    // Build the numerator/denominator strings, honouring the placeholder widths
    // (`?` space-pads; `0` zero-pads; `#` no pad).
    let num_specs = fraction_num_specs(&section.tokens);
    let den_specs = fraction_den_specs(&section.tokens, spec.fixed);

    let mut out = String::new();
    if force_minus || neg {
        out.push('-');
    }

    let mut int_written = false;
    let mut frac_written = false;
    for t in &section.tokens {
        match t {
            Token::Literal(s) => out.push_str(s),
            Token::Fill(c) => out.push(*c),
            Token::DigitZero | Token::DigitHash | Token::DigitSpace => {
                if !int_written {
                    out.push_str(&int_rendered);
                    int_written = true;
                }
            }
            Token::Fraction(_) => {
                if !int_written && has_int_part {
                    out.push_str(&int_rendered);
                    int_written = true;
                }
                if !frac_written {
                    // With an integer part and a zero fraction, the slot is
                    // blanked (space-padded) so "2    " not "2 0/1".
                    if has_int_part && num == 0 {
                        out.push_str(&pad_slot(&num_specs));
                        out.push(' '); // the `/` slot blanks to a space
                        out.push_str(&pad_slot(&den_specs));
                    } else {
                        out.push_str(&render_frac_digits(num, &num_specs));
                        out.push('/');
                        out.push_str(&render_frac_digits(den, &den_specs));
                    }
                    frac_written = true;
                }
            }
            _ => {}
        }
    }
    out
}

/// Best `n/d` approximation of `0 <= frac < 1` with `1 <= d <= max_den`,
/// minimizing `|frac - n/d|`, preferring the smaller denominator on ties.
fn best_fraction(frac: f64, max_den: u32) -> (u32, u32) {
    if frac == 0.0 {
        return (0, 1);
    }
    let mut best = (0u32, 1u32);
    let mut best_err = f64::INFINITY;
    for d in 1..=max_den {
        let n = (frac * d as f64).round() as u32;
        let err = (frac - n as f64 / d as f64).abs();
        // Strictly-less keeps the FIRST (smallest) d on ties.
        if err < best_err - 1e-12 {
            best_err = err;
            best = (n, d);
        }
    }
    best
}

/// Render a numerator/denominator integer through its placeholder specs:
/// `0` zero-pads to width, `?` space-pads, `#` no pad (left-aligned digits).
fn render_frac_digits(value: u32, specs: &[DigitKind]) -> String {
    let digits = value.to_string();
    let zero_min = specs.iter().filter(|d| **d == DigitKind::Zero).count();
    let space_min = specs.iter().filter(|d| **d == DigitKind::Space).count();
    let mut body = digits;
    if body.len() < zero_min {
        let pad = zero_min - body.len();
        let mut p = "0".repeat(pad);
        p.push_str(&body);
        body = p;
    }
    let want = zero_min + space_min;
    if body.len() < want {
        let pad = want - body.len();
        let mut p = " ".repeat(pad);
        p.push_str(&body);
        body = p;
    }
    body
}

/// All-space slot of the placeholder width (used to blank a zero fraction).
fn pad_slot(specs: &[DigitKind]) -> String {
    " ".repeat(specs.len().max(1))
}

/// Numerator placeholder specs: digit placeholders that come AFTER the integer
/// part but BEFORE the fraction token. Since [`crate::parse::resolve_fraction`]
/// folds those into the Fraction token, we reconstruct widths from the spec.
fn fraction_num_specs(tokens: &[Token]) -> Vec<DigitKind> {
    // The numerator digit count was captured in the FractionSpec; reconstruct a
    // space-padded (`?`) spec of that width — the most common authoring form.
    // (Exact per-placeholder kinds are not retained post-fold; `?` matches
    // Excel's typical `# ?/?`.)
    if let Some(Token::Fraction(spec)) = tokens.iter().find(|t| matches!(t, Token::Fraction(_))) {
        vec![DigitKind::Space; spec.num_digits.max(1)]
    } else {
        vec![DigitKind::Space]
    }
}

/// Denominator placeholder specs from the fraction token (fixed-denominator
/// codes render the literal denominator, so no padding slots).
fn fraction_den_specs(tokens: &[Token], fixed: Option<u32>) -> Vec<DigitKind> {
    if fixed.is_some() {
        return vec![];
    }
    if let Some(Token::Fraction(spec)) = tokens.iter().find(|t| matches!(t, Token::Fraction(_))) {
        vec![DigitKind::Space; spec.den_digits.max(1)]
    } else {
        vec![DigitKind::Space]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::compile;

    fn fmt(code: &str, x: f64) -> String {
        let f = compile(code).unwrap();
        let (sec, force) = f.select_numeric(x);
        render_number(
            x,
            sec,
            force,
            crate::locale::locale_data(crate::Locale::EnUs),
        )
    }

    fn fmt_de(code: &str, x: f64) -> String {
        let f = compile(code).unwrap();
        let (sec, force) = f.select_numeric(x);
        render_number(
            x,
            sec,
            force,
            crate::locale::locale_data(crate::Locale::DeDe),
        )
    }

    #[test]
    fn sheet_format_locale_de_separators() {
        // de-DE: "," decimal, "." group — the same code, localized output.
        assert_eq!(fmt_de("#,##0.00", 1234.5), "1.234,50");
        assert_eq!(fmt_de("0.00", 1.5), "1,50");
        assert_eq!(fmt_de("#,##0", 1234567.0), "1.234.567");
    }

    #[test]
    fn sheet_format_locale_de_scientific() {
        // The mantissa separator localizes; the exponent stays neutral.
        assert_eq!(fmt_de("0.00E+00", 12345.0), "1,23E+04");
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
