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

//! Argument coercion — the cross-engine hot zone (spec §7). This is where
//! most Excel/LibreOffice incompatibilities live, so each rule carries its
//! own registry row in `registry/features/coerce.yaml` and a golden corpus
//! under `corpus/fn-corpus/coerce/`. Family kernels call these instead of
//! re-deriving conversion behavior, so the rulings are stated once.
//!
//! The conversions are deliberately *total* in their failure mode: a number
//! coercion that cannot succeed returns the Excel error code
//! ([`CellError::Value`] for un-parseable text), and any [`CellValue::Error`]
//! input passes straight through (first-error-wins, §"error propagation").

use compact_str::CompactString;
use sheet_core::{CellError, CellValue};

use crate::arg::Arg;

/// Coerce a value to a number (registry `sheet.fn.coerce.to-number`).
///
/// Rulings (ECMA-376 implicit conversion; OpenFormula §6.3):
/// - [`CellValue::Empty`] → `0.0` (a blank cell is numeric zero in math).
/// - [`CellValue::Bool`] → `1.0` / `0.0`.
/// - [`CellValue::Number`] → itself.
/// - [`CellValue::Text`] → trim ASCII whitespace, then parse as `f64`.
///   Accepts a leading `+`, an `e`/`E` exponent (`"1e5"`), and plain
///   decimals. **T0 rejects thousands separators** (`"1,000"` → `#VALUE!`)
///   and currency/percent decoration — those are number-format concerns, not
///   coercion. The empty/whitespace-only string is `#VALUE!`, not `0`.
/// - [`CellValue::Error`] → that same error (propagates).
pub fn to_number(v: &CellValue) -> Result<f64, CellError> {
    match v {
        CellValue::Empty => Ok(0.0),
        CellValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        CellValue::Number(n) => Ok(*n),
        CellValue::Error(e) => Err(*e),
        CellValue::Text(t) => parse_number_text(t).ok_or(CellError::Value),
    }
}

/// Parse the textual form of a number per the T0 [`to_number`] ruling. The
/// gate is Rust's own `f64::from_str` over the trimmed string, which already
/// accepts `+1`, `1e5`, `.5`, `1.`, and `inf`/`nan` spellings — so we
/// additionally **reject** the non-Excel spellings (`inf`, `nan`,
/// hex/underscore literals) that Rust would otherwise swallow. Thousands
/// separators are absent by construction (a `,` fails the parse).
fn parse_number_text(t: &str) -> Option<f64> {
    let s = t.trim();
    if s.is_empty() {
        return None;
    }
    // Reject Rust-only spellings that Excel does not treat as numbers.
    // (Excel surfaces `inf`/`nan`/underscored/hex text as #VALUE!.)
    let probe = s.strip_prefix(['+', '-']).unwrap_or(s);
    let head = probe.as_bytes().first().copied();
    let numeric_head = matches!(head, Some(b'0'..=b'9') | Some(b'.'));
    if !numeric_head {
        return None;
    }
    if s.bytes().any(|b| b == b'_') {
        return None;
    }
    s.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Coerce a value to its General text form (registry `sheet.fn.coerce.to-text`).
///
/// Rulings (ECMA-376 implicit conversion):
/// - [`CellValue::Empty`] → `""`.
/// - [`CellValue::Bool`] → `"TRUE"` / `"FALSE"` (the literal tokens).
/// - [`CellValue::Number`] → the **General** representation (see
///   [`general_number_text`]). This is the same contract the FMT track's
///   `sheet_format::format_general` realizes for display; coercion keeps a
///   self-contained copy so `sheet-fn` is independently testable (D-6,
///   15-significant-digit display).
/// - [`CellValue::Text`] → itself.
/// - [`CellValue::Error`] → its display token (`"#VALUE!"`, …). A function
///   that text-coerces an error has usually already short-circuited via
///   [`first_error`]; this branch keeps the conversion total.
pub fn to_text(v: &CellValue) -> CompactString {
    match v {
        CellValue::Empty => CompactString::default(),
        CellValue::Bool(b) => CompactString::new(if *b { "TRUE" } else { "FALSE" }),
        CellValue::Number(n) => general_number_text(*n),
        CellValue::Text(t) => t.clone(),
        CellValue::Error(e) => CompactString::new(e.as_str()),
    }
}

/// Coerce a value to a boolean (registry `sheet.fn.coerce.to-bool`).
///
/// Rulings (ECMA-376 implicit conversion):
/// - [`CellValue::Bool`] → itself.
/// - [`CellValue::Number`] → `n != 0.0` (so `0`/`-0` are FALSE; everything
///   else TRUE).
/// - [`CellValue::Text`] → `"TRUE"`/`"FALSE"` case-insensitively, after
///   trimming; any other text → `#VALUE!`. (Excel does **not** read numeric
///   text like `"1"` as a boolean — only the two literal words.)
/// - [`CellValue::Empty`] → `false`.
/// - [`CellValue::Error`] → that error (propagates).
pub fn to_bool(v: &CellValue) -> Result<bool, CellError> {
    match v {
        CellValue::Bool(b) => Ok(*b),
        CellValue::Number(n) => Ok(*n != 0.0),
        CellValue::Empty => Ok(false),
        CellValue::Error(e) => Err(*e),
        CellValue::Text(t) => {
            let s = t.trim();
            if s.eq_ignore_ascii_case("TRUE") {
                Ok(true)
            } else if s.eq_ignore_ascii_case("FALSE") {
                Ok(false)
            } else {
                Err(CellError::Value)
            }
        }
    }
}

/// First-error-wins propagation over a kernel's argument list (registry
/// `sheet.fn.coerce.error-propagation`).
///
/// Scans **scalar** arguments left-to-right and returns the first
/// [`CellValue::Error`] it finds. **Ranges are deliberately NOT scanned**:
/// aggregation functions (`COUNT`, `SUM`, `AVERAGE`, …) accept ranges that
/// may contain error cells without the whole call failing — error handling
/// inside a range is per-function (e.g. `SUM` *does* propagate, `COUNT` does
/// not), so it cannot live in this shared pre-pass. Kernels that want the
/// "any error anywhere fails" behavior iterate their ranges and call
/// [`to_number`] (which propagates) themselves. This keeps COUNT-style
/// semantics correct by construction (spec §7).
pub fn first_error(args: &[Arg]) -> Option<CellError> {
    for a in args {
        if let Arg::Scalar(CellValue::Error(e)) = a {
            return Some(*e);
        }
    }
    None
}

/// The cross-type total order RULING (registry `sheet.fn.coerce.comparison`).
///
/// Excel orders values *across* types: **Number < Text < Bool**. Within a
/// type:
/// - Numbers compare numerically (NaN folds to `Equal` against itself — it
///   should never reach storage, but the order stays total).
/// - Text compares **case-insensitively** (ASCII-fold), then by raw bytes as
///   a tie-break so the order is total (`"a"` vs `"A"` are equal under the
///   fold but ordered by byte to stay antisymmetric).
/// - Bools order `false < true`.
///
/// [`CellValue::Empty`] is not its own rank: it **coerces to the peer's
/// type** — `0` against a Number, `""` against Text, `false` against a Bool,
/// and (Empty vs Empty) compares equal. [`CellValue::Error`] has no place in
/// an ordering (it short-circuits upstream); it is parked at the very end so
/// the function stays total.
pub fn compare(a: &CellValue, b: &CellValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    use CellValue::*;

    // Resolve Empty against the other operand's type, so the type-rank and
    // within-type comparisons below see a concrete peer.
    fn rank(v: &CellValue) -> u8 {
        match v {
            CellValue::Number(_) => 0,
            CellValue::Text(_) => 1,
            CellValue::Bool(_) => 2,
            CellValue::Error(_) => 3,
            // Empty has no intrinsic rank; callers resolve it first.
            CellValue::Empty => u8::MAX,
        }
    }

    match (a, b) {
        // Empty vs Empty: equal.
        (Empty, Empty) => Ordering::Equal,
        // Empty coerces to the peer's type.
        (Empty, Number(_)) => Number(0.0).partial_cmp_total(b),
        (Number(_), Empty) => a.partial_cmp_total(&Number(0.0)),
        (Empty, Text(_)) => Text(CompactString::default()).cmp_text(b),
        (Text(_), Empty) => a.cmp_text(&Text(CompactString::default())),
        (Empty, Bool(_)) => Bool(false).cmp_bool(b),
        (Bool(_), Empty) => a.cmp_bool(&Bool(false)),
        // Empty vs Error: park errors last.
        (Empty, Error(_)) => Ordering::Less,
        (Error(_), Empty) => Ordering::Greater,

        // Same concrete type: within-type order.
        (Number(_), Number(_)) => a.partial_cmp_total(b),
        (Text(_), Text(_)) => a.cmp_text(b),
        (Bool(x), Bool(y)) => x.cmp(y),
        (Error(x), Error(y)) => (*x as u8).cmp(&(*y as u8)),

        // Cross type: by the Number<Text<Bool<Error rank.
        _ => rank(a).cmp(&rank(b)),
    }
}

// Small within-type comparison helpers kept private to `compare`'s ruling.
trait CompareExt {
    fn partial_cmp_total(&self, other: &CellValue) -> std::cmp::Ordering;
    fn cmp_text(&self, other: &CellValue) -> std::cmp::Ordering;
    fn cmp_bool(&self, other: &CellValue) -> std::cmp::Ordering;
}

impl CompareExt for CellValue {
    fn partial_cmp_total(&self, other: &CellValue) -> std::cmp::Ordering {
        let x = match self {
            CellValue::Number(n) => *n,
            _ => 0.0,
        };
        let y = match other {
            CellValue::Number(n) => *n,
            _ => 0.0,
        };
        x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
    }
    fn cmp_text(&self, other: &CellValue) -> std::cmp::Ordering {
        let x = match self {
            CellValue::Text(t) => t.as_str(),
            _ => "",
        };
        let y = match other {
            CellValue::Text(t) => t.as_str(),
            _ => "",
        };
        // Case-insensitive primary key, raw bytes as a total tie-break.
        let folded = x
            .bytes()
            .map(|b| b.to_ascii_lowercase())
            .cmp(y.bytes().map(|b| b.to_ascii_lowercase()));
        folded.then_with(|| x.cmp(y))
    }
    fn cmp_bool(&self, other: &CellValue) -> std::cmp::Ordering {
        let x = matches!(self, CellValue::Bool(true));
        let y = matches!(other, CellValue::Bool(true));
        x.cmp(&y)
    }
}

/// Render an `f64` in Excel's **General** number format (D-6: 15 significant
/// digits, shortest round-trippable). This is `sheet-fn`'s self-contained
/// copy of the `sheet_format::format_general` contract so coercion is
/// testable without a `sheet-format` dependency cycle on its codegen; the
/// two MUST agree, and the `coerce/to_text.golden.tsv` corpus pins the
/// behavior cross-engine.
///
/// Rules:
/// - Non-finite (should not reach storage) maps to Excel's `#NUM!` token so
///   the conversion is total; real values never hit this.
/// - Integers within the safe-integer range print with no decimal point.
/// - Magnitudes in `[1e-4, 1e15)` print in fixed notation, trimmed of
///   trailing zeros, at most 15 significant digits.
/// - Anything else prints in scientific notation `dE±dd` (Excel's General
///   exponent form), again ≤15 significant digits.
fn general_number_text(n: f64) -> CompactString {
    if !n.is_finite() {
        return CompactString::new(CellError::Num.as_str());
    }
    if n == 0.0 {
        // Collapses -0.0 to "0" as Excel General does.
        return CompactString::new("0");
    }

    let abs = n.abs();
    // Excel General switches to scientific outside roughly [1e-4, 1e15).
    if !(1e-4..1e15).contains(&abs) {
        return scientific_15(n);
    }

    // Fixed notation: 15 significant digits, then trim trailing zeros.
    // Significant digits left of the point:
    let int_digits = if abs >= 1.0 {
        abs.log10().floor() as i32 + 1
    } else {
        0
    };
    let decimals = (15 - int_digits).clamp(0, 15) as usize;
    let mut s = format!("{n:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    CompactString::new(s)
}

/// Scientific-notation branch of [`general_number_text`]: a normalized
/// mantissa (≤15 significant digits, trailing zeros trimmed) and a signed,
/// at-least-two-digit exponent — Excel's General scientific form (`1.5E+20`,
/// `1E-05`).
fn scientific_15(n: f64) -> CompactString {
    // {:e} gives "d.ddddde±d"; render with 14 fractional mantissa digits
    // (15 significant), then trim.
    let raw = format!("{n:.14e}");
    let (mantissa, exp) = raw.split_once('e').unwrap_or((raw.as_str(), "0"));
    let mut m = mantissa.to_string();
    if m.contains('.') {
        while m.ends_with('0') {
            m.pop();
        }
        if m.ends_with('.') {
            m.pop();
        }
    }
    let exp_i: i32 = exp.parse().unwrap_or(0);
    let sign = if exp_i < 0 { '-' } else { '+' };
    CompactString::new(format!("{m}E{sign}{:02}", exp_i.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(n: f64) -> CellValue {
        CellValue::Number(n)
    }
    fn txt(s: &str) -> CellValue {
        CellValue::from(s)
    }

    #[test]
    fn to_number_basic() {
        assert_eq!(to_number(&CellValue::Empty), Ok(0.0));
        assert_eq!(to_number(&CellValue::Bool(true)), Ok(1.0));
        assert_eq!(to_number(&CellValue::Bool(false)), Ok(0.0));
        assert_eq!(to_number(&num(3.5)), Ok(3.5));
        assert_eq!(to_number(&txt("  42  ")), Ok(42.0));
        assert_eq!(to_number(&txt("1e5")), Ok(100000.0));
        assert_eq!(to_number(&txt("+7")), Ok(7.0));
        assert_eq!(to_number(&txt(".5")), Ok(0.5));
    }

    #[test]
    fn to_number_rejections() {
        assert_eq!(to_number(&txt("1,000")), Err(CellError::Value));
        assert_eq!(to_number(&txt("abc")), Err(CellError::Value));
        assert_eq!(to_number(&txt("")), Err(CellError::Value));
        assert_eq!(to_number(&txt("   ")), Err(CellError::Value));
        assert_eq!(to_number(&txt("inf")), Err(CellError::Value));
        assert_eq!(to_number(&txt("NaN")), Err(CellError::Value));
        assert_eq!(to_number(&txt("1_000")), Err(CellError::Value));
        assert_eq!(
            to_number(&CellValue::Error(CellError::Div0)),
            Err(CellError::Div0)
        );
    }

    #[test]
    fn to_text_basic() {
        assert_eq!(to_text(&CellValue::Empty).as_str(), "");
        assert_eq!(to_text(&CellValue::Bool(true)).as_str(), "TRUE");
        assert_eq!(to_text(&CellValue::Bool(false)).as_str(), "FALSE");
        assert_eq!(to_text(&txt("hi")).as_str(), "hi");
        assert_eq!(
            to_text(&CellValue::Error(CellError::Value)).as_str(),
            "#VALUE!"
        );
    }

    #[test]
    fn to_text_general_numbers() {
        assert_eq!(to_text(&num(0.0)).as_str(), "0");
        assert_eq!(to_text(&num(-0.0)).as_str(), "0");
        assert_eq!(to_text(&num(42.0)).as_str(), "42");
        assert_eq!(to_text(&num(-3.0)).as_str(), "-3");
        assert_eq!(to_text(&num(1.5)).as_str(), "1.5");
        assert_eq!(to_text(&num(0.1)).as_str(), "0.1");
        assert_eq!(to_text(&num(100000.0)).as_str(), "100000");
        // 15-sig-digit fixed.
        assert_eq!(to_text(&num(1.0 / 3.0)).as_str(), "0.333333333333333");
    }

    #[test]
    fn to_text_scientific() {
        assert_eq!(to_text(&num(1e20)).as_str(), "1E+20");
        assert_eq!(to_text(&num(1.5e20)).as_str(), "1.5E+20");
        assert_eq!(to_text(&num(1e-5)).as_str(), "1E-05");
        assert_eq!(to_text(&num(1.25e-7)).as_str(), "1.25E-07");
    }

    #[test]
    fn to_bool_basic() {
        assert_eq!(to_bool(&CellValue::Bool(true)), Ok(true));
        assert_eq!(to_bool(&num(0.0)), Ok(false));
        assert_eq!(to_bool(&num(-2.0)), Ok(true));
        assert_eq!(to_bool(&txt("true")), Ok(true));
        assert_eq!(to_bool(&txt("FALSE")), Ok(false));
        assert_eq!(to_bool(&txt("  True  ")), Ok(true));
        assert_eq!(to_bool(&CellValue::Empty), Ok(false));
        assert_eq!(to_bool(&txt("1")), Err(CellError::Value));
        assert_eq!(to_bool(&txt("yes")), Err(CellError::Value));
        assert_eq!(
            to_bool(&CellValue::Error(CellError::Na)),
            Err(CellError::Na)
        );
    }

    #[test]
    fn first_error_scalars_only() {
        let args = [
            Arg::Scalar(num(1.0)),
            Arg::Scalar(CellValue::Error(CellError::Div0)),
            Arg::Scalar(CellValue::Error(CellError::Na)),
        ];
        assert_eq!(first_error(&args), Some(CellError::Div0));

        let clean = [Arg::Scalar(num(1.0)), Arg::Scalar(txt("x"))];
        assert_eq!(first_error(&clean), None);
    }

    #[test]
    fn first_error_ignores_ranges() {
        use crate::arg::RangeView;
        use sheet_core::CellRef;
        let origin = CellRef {
            sheet: 0,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        };
        let cells = [num(1.0), CellValue::Error(CellError::Div0)];
        let v = RangeView::from_slice(origin, 1, 2, &cells);
        let args = [Arg::Range(v)];
        // The error is INSIDE a range -> first_error does NOT see it.
        assert_eq!(first_error(&args), None);
    }

    #[test]
    fn compare_within_type() {
        use std::cmp::Ordering::*;
        assert_eq!(compare(&num(1.0), &num(2.0)), Less);
        assert_eq!(compare(&num(2.0), &num(2.0)), Equal);
        assert_eq!(compare(&txt("apple"), &txt("banana")), Less);
        // Case-insensitive primary, byte tie-break.
        assert_eq!(compare(&txt("A"), &txt("a")), Less); // 'A' < 'a' by byte
        assert_eq!(compare(&txt("abc"), &txt("ABC")), Greater);
        assert_eq!(
            compare(&CellValue::Bool(false), &CellValue::Bool(true)),
            Less
        );
    }

    #[test]
    fn compare_cross_type_ranks() {
        use std::cmp::Ordering::*;
        // Number < Text < Bool.
        assert_eq!(compare(&num(999.0), &txt("a")), Less);
        assert_eq!(compare(&txt("z"), &CellValue::Bool(false)), Less);
        assert_eq!(compare(&num(0.0), &CellValue::Bool(false)), Less);
        assert_eq!(compare(&CellValue::Bool(true), &num(0.0)), Greater);
    }

    #[test]
    fn compare_empty_coerces() {
        use std::cmp::Ordering::*;
        assert_eq!(compare(&CellValue::Empty, &CellValue::Empty), Equal);
        // Empty vs Number coerces to 0.
        assert_eq!(compare(&CellValue::Empty, &num(0.0)), Equal);
        assert_eq!(compare(&CellValue::Empty, &num(1.0)), Less);
        // Empty vs Text coerces to "".
        assert_eq!(compare(&CellValue::Empty, &txt("")), Equal);
        assert_eq!(compare(&CellValue::Empty, &txt("a")), Less);
        // Empty vs Bool coerces to false.
        assert_eq!(compare(&CellValue::Empty, &CellValue::Bool(false)), Equal);
        assert_eq!(compare(&CellValue::Empty, &CellValue::Bool(true)), Less);
    }
}
