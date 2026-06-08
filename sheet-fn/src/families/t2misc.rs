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

//! Misc T2 functions (spec §7/§11 T2 "remaining publishing-relevant
//! functions"): [`aggregate`], [`subtotal`], [`roman`], [`arabic`],
//! [`convert`], [`hyperlink`]. Pure kernels of the frozen signature
//! `fn(&[Arg], &EvalCtx) -> CellValue`; the arity guard runs in the generated
//! [`crate::dispatch`] before these are called. All coercion routes through
//! [`crate::coerce`]; arithmetic accumulation routes through the
//! [`crate::num`] seam (D-6).
//!
//! # Documented T2 subsets (these are RULINGS, not accidents — spec §3)
//!
//! ## `AGGREGATE(function_num, options, ref1, …)`
//!
//! Microsoft's `AGGREGATE` has 19 inner aggregates and 8 option modes. The T2
//! scope is the **publishing-relevant core** and is enforced, not faked:
//!
//! - **`function_num` 1–11** only — `AVERAGE`(1), `COUNT`(2), `COUNTA`(3),
//!   `MAX`(4), `MIN`(5), `PRODUCT`(6), `STDEV.S`(7), `STDEV.P`(8), `SUM`(9),
//!   `VAR.S`(10), `VAR.P`(11). `function_num` 12–19 (`MEDIAN`, `MODE.SNGL`,
//!   `LARGE`, `SMALL`, `PERCENTILE.INC`, `QUARTILE.INC`, `PERCENTILE.EXC`,
//!   `QUARTILE.EXC`) take a trailing `k`/quantile argument and are **deferred
//!   in T2** → `#VALUE!`. Any other `function_num` → `#VALUE!` (Excel).
//! - **`options` 0, 4, or 6** only. `0`/`4` = "ignore nothing" (in the pure
//!   kernel these are identical: there is no nested-`SUBTOTAL`/`AGGREGATE`
//!   metadata to ignore). `6` = "ignore error values" (an error cell inside a
//!   range is skipped rather than propagated). The hidden-row options
//!   (`1`,`2`,`3`,`5`,`7`) need row-visibility metadata that never reaches a
//!   pure kernel → `#VALUE!`. (Spec §2: no SDK/model access from a kernel.)
//!
//! Under option `0`/`4`, an error cell anywhere in a `ref` **is** the result
//! (first error wins, row-major) — exactly the aggregation family's ruling.
//! Under option `6`, error cells are skipped.
//!
//! ## `SUBTOTAL(function_num, ref1, …)`
//!
//! `function_num` 1–11 (include manually-hidden rows) and 101–111 (ignore
//! manually-hidden rows) — same inner-aggregate mapping as `AGGREGATE` above.
//! **T2 ruling:** a pure kernel has no row-visibility metadata, so 1–11 and
//! 101–111 compute **identically** (all cells participate). `SUBTOTAL` also
//! excludes the results of *nested* `SUBTOTAL` calls inside a `ref`; a pure
//! kernel sees only plain values, so this exclusion is documented but not
//! re-derived. An error cell in a `ref` propagates (first error wins).
//!
//! ## `ROMAN(number, [form])` / `ARABIC(text)`
//!
//! `ROMAN` renders an integer 1–3999 as classic roman-numeral **text**;
//! `ROMAN(0)` → `""`; a negative number or >3999 → `#VALUE!` (Excel). The
//! optional `form` selects a more-concise style in Excel (0 = classic, 1–4
//! progressively shorter); **T2 implements form 0 (classic)** and accepts any
//! `form` 0–4 by rendering classic (documented simplification — the publishing
//! use is the classic numeral). `ARABIC` is the inverse: roman text → number,
//! a leading `-` negates, `""` → 0, non-roman text → `#VALUE!`.
//!
//! ## `CONVERT(number, from_unit, to_unit)`
//!
//! A documented **basic** unit set (the publishing-relevant subset of
//! ECMA-376 §18.17.7): length `m`/`ft`/`in`/`mi`/`km`, mass `g`/`kg`/`lbm`,
//! time `s`/`min`/`hr`/`day`, temperature `C`/`F`/`K`. Units are
//! case-sensitive (Excel: `"m"` ≠ `"M"`). An unknown unit or a cross-dimension
//! conversion (`"m"` → `"kg"`) → `#N/A` (Excel). Temperature converts through
//! the affine Celsius pivot; the others through a multiplicative SI base.
//!
//! ## `HYPERLINK(link_location, [friendly_name])`
//!
//! **T2 ruling: display-only.** A formula cell cannot host a navigable link on
//! the lowered page surface (spec §8), so `HYPERLINK` returns its
//! `friendly_name` text — or the `link_location` text when `friendly_name` is
//! absent or blank. The link target is preserved in the formula (round-trip)
//! but is **not navigable on the page**. Documented, not faked.

use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;
use crate::num::{Numeric, F64};

// ============================================================================
// AGGREGATE / SUBTOTAL — function_num-selected aggregation
// ============================================================================

/// The inner aggregate an `AGGREGATE`/`SUBTOTAL` `function_num` selects
/// (1-based, the T2-supported 1–11 set). `SUBTOTAL`'s 101–111 map to the same
/// kernel here (T2 has no hidden-row metadata, see module docs).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum InnerFn {
    Average,
    Count,
    CountA,
    Max,
    Min,
    Product,
    StdevS,
    StdevP,
    Sum,
    VarS,
    VarP,
}

impl InnerFn {
    /// Map a 1–11 `function_num` to its inner aggregate (the shared
    /// `AGGREGATE`/`SUBTOTAL` selector). `None` for anything outside 1–11.
    fn from_num(n: u32) -> Option<InnerFn> {
        Some(match n {
            1 => InnerFn::Average,
            2 => InnerFn::Count,
            3 => InnerFn::CountA,
            4 => InnerFn::Max,
            5 => InnerFn::Min,
            6 => InnerFn::Product,
            7 => InnerFn::StdevS,
            8 => InnerFn::StdevP,
            9 => InnerFn::Sum,
            10 => InnerFn::VarS,
            11 => InnerFn::VarP,
            _ => return None,
        })
    }
}

/// What an inner aggregate counts vs. sums: most operate on the numeric values
/// of the participating cells; `COUNT`/`COUNTA` are pure counters.
fn apply_inner(inner: InnerFn, nums: &[f64], count_numbers: u64, count_nonblank: u64) -> CellValue {
    match inner {
        InnerFn::Count => CellValue::Number(count_numbers as f64),
        InnerFn::CountA => CellValue::Number(count_nonblank as f64),
        InnerFn::Average => {
            if nums.is_empty() {
                return CellValue::Error(CellError::Div0);
            }
            CellValue::Number(mean(nums))
        }
        InnerFn::Max => {
            let m = nums
                .iter()
                .copied()
                .fold(None, |a: Option<f64>, n| Some(a.map_or(n, |a| a.max(n))));
            CellValue::Number(m.unwrap_or(0.0))
        }
        InnerFn::Min => {
            let m = nums
                .iter()
                .copied()
                .fold(None, |a: Option<f64>, n| Some(a.map_or(n, |a| a.min(n))));
            CellValue::Number(m.unwrap_or(0.0))
        }
        InnerFn::Product => {
            // PRODUCT of no numbers is 0 (Excel).
            if nums.is_empty() {
                return CellValue::Number(0.0);
            }
            let mut p = F64::from_f64(1.0);
            for n in nums {
                p = p.mul(F64::from_f64(*n));
            }
            CellValue::Number(p.to_f64())
        }
        InnerFn::Sum => {
            let mut s = F64::from_f64(0.0);
            for n in nums {
                s = s.add(F64::from_f64(*n));
            }
            CellValue::Number(s.to_f64())
        }
        // Sample statistics need at least 2 points (n-1 divisor); population
        // needs at least 1. Too-few points -> #DIV/0! (Excel).
        InnerFn::VarS => {
            variance(nums, true).map_or(CellValue::Error(CellError::Div0), CellValue::Number)
        }
        InnerFn::VarP => {
            variance(nums, false).map_or(CellValue::Error(CellError::Div0), CellValue::Number)
        }
        InnerFn::StdevS => variance(nums, true).map_or(CellValue::Error(CellError::Div0), |v| {
            CellValue::Number(v.sqrt())
        }),
        InnerFn::StdevP => variance(nums, false).map_or(CellValue::Error(CellError::Div0), |v| {
            CellValue::Number(v.sqrt())
        }),
    }
}

/// Arithmetic mean (routes the running sum through the [`crate::num`] seam).
fn mean(nums: &[f64]) -> f64 {
    let mut s = F64::from_f64(0.0);
    for n in nums {
        s = s.add(F64::from_f64(*n));
    }
    s.div(F64::from_f64(nums.len() as f64)).to_f64()
}

/// Variance: `sample` selects the `n-1` divisor (`VAR.S`/`STDEV.S`) vs the `n`
/// divisor (`VAR.P`/`STDEV.P`). `None` when there are too few points for the
/// chosen divisor (sample needs ≥2, population needs ≥1).
fn variance(nums: &[f64], sample: bool) -> Option<f64> {
    let n = nums.len();
    let divisor = if sample { n.checked_sub(1)? } else { n };
    if divisor == 0 {
        return None;
    }
    let m = mean(nums);
    let mut acc = F64::from_f64(0.0);
    for x in nums {
        let d = F64::from_f64(*x).sub(F64::from_f64(m));
        acc = acc.add(d.mul(d));
    }
    Some(acc.div(F64::from_f64(divisor as f64)).to_f64())
}

/// Outcome of scanning the `ref` arguments of an `AGGREGATE`/`SUBTOTAL` call.
struct Scan {
    /// Numeric values participating in value-based aggregates.
    nums: Vec<f64>,
    /// Count of numeric cells (the `COUNT` answer).
    count_numbers: u64,
    /// Count of non-blank cells (the `COUNTA` answer).
    count_nonblank: u64,
    /// First error encountered (only set when errors are NOT skipped).
    first_error: Option<CellError>,
}

/// Scan the `ref` arguments past the leading selector args. `skip_errors`
/// controls error handling: when `true` (AGGREGATE option 6) an error cell is
/// skipped; when `false` the first error is captured for propagation.
///
/// Scalar args go through [`coerce::to_number`] (bool / numeric-text
/// participate), matching the aggregation family's scalar/range asymmetry; a
/// non-numeric scalar that fails coercion is treated as a value error (skipped
/// or propagated per `skip_errors`). Range cells: numbers participate, errors
/// per `skip_errors`, and text/bool/blank are skipped for value aggregates but
/// still feed `COUNTA` when non-blank.
fn scan_refs(refs: &[Arg], skip_errors: bool) -> Scan {
    let mut s = Scan {
        nums: Vec::new(),
        count_numbers: 0,
        count_nonblank: 0,
        first_error: None,
    };
    for a in refs {
        match a {
            Arg::Scalar(v) => {
                // A scalar argument is always "present" for COUNTA.
                if !v.is_blank() {
                    s.count_nonblank += 1;
                }
                match coerce::to_number(v) {
                    Ok(n) => {
                        s.nums.push(n);
                        s.count_numbers += 1;
                    }
                    Err(e) => {
                        // Numeric-text/bool already succeeded above; this is a
                        // genuine error or non-numeric text. Propagate the
                        // error variant unless errors are being skipped.
                        if matches!(v, CellValue::Error(_)) && !skip_errors {
                            s.first_error.get_or_insert(e);
                        }
                    }
                }
            }
            Arg::Range(view) => {
                for cell in view.iter() {
                    if !cell.is_blank() {
                        s.count_nonblank += 1;
                    }
                    match cell {
                        CellValue::Number(n) => {
                            s.nums.push(n);
                            s.count_numbers += 1;
                        }
                        CellValue::Error(e) => {
                            if !skip_errors {
                                s.first_error.get_or_insert(e);
                            }
                        }
                        // Text / Bool / Empty: skipped for value aggregates
                        // (already counted for COUNTA above when non-blank).
                        _ => {}
                    }
                }
            }
        }
    }
    s
}

/// `AGGREGATE(function_num, options, ref1, …)` (registry
/// `sheet.fn.math.aggregate`). T2 subset: `function_num` 1–11, `options`
/// 0/4/6. See the module docs for the full ruling. The leading two args are
/// the selector + option (read as numbers); the rest are the refs.
pub fn aggregate(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    // function_num (arg 0). An error here propagates.
    let fnum = match scalar_u32(args.first()) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let options = match scalar_u32(args.get(1)) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };

    let Some(inner) = InnerFn::from_num(fnum) else {
        // function_num 12-19 are valid in Excel but deferred in T2; any other
        // value is invalid. Both surface as #VALUE! (documented subset).
        return CellValue::Error(CellError::Value);
    };

    // T2 options: 0/4 = ignore nothing, 6 = ignore error values. The
    // hidden-row options (1/2/3/5/7) need row-visibility metadata a pure
    // kernel cannot see -> #VALUE!.
    let skip_errors = match options {
        0 | 4 => false,
        6 => true,
        _ => return CellValue::Error(CellError::Value),
    };

    let refs = &args[2..];
    let scan = scan_refs(refs, skip_errors);
    if let Some(e) = scan.first_error {
        return CellValue::Error(e);
    }
    apply_inner(inner, &scan.nums, scan.count_numbers, scan.count_nonblank)
}

/// `SUBTOTAL(function_num, ref1, …)` (registry `sheet.fn.math.subtotal`).
/// `function_num` 1–11 (include hidden) and 101–111 (ignore hidden); T2 has no
/// row-visibility metadata so the two ranges compute identically (module
/// docs). Errors propagate (first error wins). The leading arg is the
/// selector; the rest are the refs.
pub fn subtotal(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let raw = match scalar_u32(args.first()) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    // 101-111 normalize to 1-11 (T2: no hidden-row distinction).
    let fnum = if (101..=111).contains(&raw) {
        raw - 100
    } else {
        raw
    };
    let Some(inner) = InnerFn::from_num(fnum) else {
        return CellValue::Error(CellError::Value);
    };

    let refs = &args[1..];
    // SUBTOTAL always propagates errors (there is no ignore-error option).
    let scan = scan_refs(refs, false);
    if let Some(e) = scan.first_error {
        return CellValue::Error(e);
    }
    apply_inner(inner, &scan.nums, scan.count_numbers, scan.count_nonblank)
}

/// Read a leading selector/option arg as a non-negative integer. The arg is
/// coerced to a number (bool/numeric-text accepted), truncated toward zero;
/// an error coercion propagates; a negative value is `#VALUE!` (no negative
/// function_num/option exists). A range arg uses its top-left cell.
fn scalar_u32(arg: Option<&Arg>) -> Result<u32, CellError> {
    let v = match arg {
        Some(Arg::Scalar(v)) => v.clone(),
        Some(Arg::Range(view)) => view.get(0, 0),
        None => return Err(CellError::Value),
    };
    let n = coerce::to_number(&v)?;
    if !n.is_finite() || n < 0.0 {
        return Err(CellError::Value);
    }
    Ok(n.trunc() as u32)
}

// ============================================================================
// ROMAN / ARABIC — roman-numeral <-> integer text conversions
// ============================================================================

/// The classic additive/subtractive roman-numeral table, descending.
const ROMAN_TABLE: &[(u32, &str)] = &[
    (1000, "M"),
    (900, "CM"),
    (500, "D"),
    (400, "CD"),
    (100, "C"),
    (90, "XC"),
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
];

/// `ROMAN(number, [form])` (registry `sheet.fn.math.roman`). Classic
/// roman-numeral **text** for an integer 1–3999 (T2 renders classic for any
/// `form` 0–4, see module docs). `0` → `""`; negative or >3999 → `#VALUE!`.
pub fn roman(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let number = match scalar_number(args.first()) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    // [form] is accepted (0-4 classic..simplified); out-of-range -> #VALUE!.
    if let Some(form_arg) = args.get(1) {
        match scalar_number(Some(form_arg)) {
            Ok(f) => {
                let f = f.trunc();
                if !(0.0..=4.0).contains(&f) {
                    return CellValue::Error(CellError::Value);
                }
            }
            Err(e) => return CellValue::Error(e),
        }
    }

    let n = number.trunc();
    if !(0.0..=3999.0).contains(&n) {
        return CellValue::Error(CellError::Value);
    }
    let mut v = n as u32;
    let mut out = String::new();
    for (val, sym) in ROMAN_TABLE {
        while v >= *val {
            out.push_str(sym);
            v -= *val;
        }
    }
    CellValue::Text(out.into())
}

/// `ARABIC(text)` (registry `sheet.fn.math.arabic`). The inverse of classic
/// `ROMAN`: parse roman-numeral text to a number. A leading `-` negates;
/// `""`/whitespace → `0`; any non-roman character → `#VALUE!`. Lower-case is
/// accepted (Excel folds case). An error scalar propagates.
pub fn arabic(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let v = match args.first() {
        Some(Arg::Scalar(v)) => v.clone(),
        Some(Arg::Range(view)) => view.get(0, 0),
        None => return CellValue::Error(CellError::Value),
    };
    if let CellValue::Error(e) = v {
        return CellValue::Error(e);
    }
    let text = coerce::to_text(&v);
    let trimmed = text.trim();
    let (negative, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed),
    };
    if body.is_empty() {
        return CellValue::Number(0.0);
    }
    match parse_roman(body) {
        Some(val) => CellValue::Number(if negative { -(val as f64) } else { val as f64 }),
        None => CellValue::Error(CellError::Value),
    }
}

/// Parse an upper/lower-case roman string to its integer value via the
/// standard subtractive scan (a smaller symbol before a larger one
/// subtracts). Returns `None` on any non-roman character. Permissive about
/// canonical form (Excel's `ARABIC` accepts non-canonical strings like
/// `"MCMXC"`); we accept any string of roman letters and sum subtractively.
fn parse_roman(s: &str) -> Option<u32> {
    fn digit(c: char) -> Option<u32> {
        Some(match c.to_ascii_uppercase() {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            'C' => 100,
            'D' => 500,
            'M' => 1000,
            _ => return None,
        })
    }
    let vals: Vec<u32> = s.chars().map(digit).collect::<Option<Vec<u32>>>()?;
    let mut total: i64 = 0;
    for i in 0..vals.len() {
        let cur = vals[i] as i64;
        // A smaller value immediately before a larger one is subtractive.
        if i + 1 < vals.len() && cur < vals[i + 1] as i64 {
            total -= cur;
        } else {
            total += cur;
        }
    }
    if total < 0 {
        None
    } else {
        Some(total as u32)
    }
}

// ============================================================================
// CONVERT — basic unit conversion
// ============================================================================

/// The dimension a unit belongs to. Conversions across dimensions are `#N/A`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Dimension {
    Length,
    Mass,
    Time,
    Temperature,
}

/// `CONVERT(number, from_unit, to_unit)` (registry
/// `sheet.fn.engineering.convert`). The documented T2 basic unit set (module
/// docs). Unknown unit or cross-dimension conversion → `#N/A`. Units are
/// case-sensitive. An error in any arg propagates.
pub fn convert(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let number = match scalar_number(args.first()) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let from = scalar_text(args.get(1));
    let to = scalar_text(args.get(2));

    let (Some((from_dim, _)), Some((to_dim, _))) = (unit_info(&from), unit_info(&to)) else {
        // Unknown unit string -> #N/A (Excel).
        return CellValue::Error(CellError::Na);
    };
    if from_dim != to_dim {
        // Cross-dimension conversion -> #N/A (Excel).
        return CellValue::Error(CellError::Na);
    }

    let out = if from_dim == Dimension::Temperature {
        // Temperature is affine: pivot through Celsius.
        let celsius = match to_celsius(&from, number) {
            Some(c) => c,
            None => return CellValue::Error(CellError::Na),
        };
        match from_celsius(&to, celsius) {
            Some(v) => v,
            None => return CellValue::Error(CellError::Na),
        }
    } else {
        // Multiplicative: value -> SI base -> target.
        let (_, from_factor) = unit_info(&from).unwrap();
        let (_, to_factor) = unit_info(&to).unwrap();
        // base = number * from_factor; result = base / to_factor.
        F64::from_f64(number)
            .mul(F64::from_f64(from_factor))
            .div(F64::from_f64(to_factor))
            .to_f64()
    };
    CellValue::Number(out)
}

/// Look up a unit string: its [`Dimension`] and the factor to the SI base of
/// that dimension (metre / gram / second). Temperature carries a `1.0`
/// placeholder factor — it converts through the affine helpers, not the
/// multiplicative path. `None` for an unknown unit.
fn unit_info(u: &str) -> Option<(Dimension, f64)> {
    Some(match u {
        // Length -> base metre.
        "m" => (Dimension::Length, 1.0),
        "km" => (Dimension::Length, 1000.0),
        "in" => (Dimension::Length, 0.0254),
        "ft" => (Dimension::Length, 0.3048),
        "mi" => (Dimension::Length, 1609.344),
        // Mass -> base gram.
        "g" => (Dimension::Mass, 1.0),
        "kg" => (Dimension::Mass, 1000.0),
        "lbm" => (Dimension::Mass, 453.59237),
        // Time -> base second.
        "s" => (Dimension::Time, 1.0),
        "min" => (Dimension::Time, 60.0),
        "hr" => (Dimension::Time, 3600.0),
        "day" => (Dimension::Time, 86400.0),
        // Temperature (affine; factor is a placeholder).
        "C" => (Dimension::Temperature, 1.0),
        "F" => (Dimension::Temperature, 1.0),
        "K" => (Dimension::Temperature, 1.0),
        _ => return None,
    })
}

/// Convert a temperature `value` in unit `u` to Celsius. `None` for a
/// non-temperature unit (caller has already dimension-checked, so this is
/// defensive).
fn to_celsius(u: &str, value: f64) -> Option<f64> {
    Some(match u {
        "C" => value,
        "F" => (value - 32.0) * 5.0 / 9.0,
        "K" => value - 273.15,
        _ => return None,
    })
}

/// Convert a Celsius `value` to the temperature unit `u`. `None` for a
/// non-temperature unit (defensive — caller dimension-checks first).
fn from_celsius(u: &str, value: f64) -> Option<f64> {
    Some(match u {
        "C" => value,
        "F" => value * 9.0 / 5.0 + 32.0,
        "K" => value + 273.15,
        _ => return None,
    })
}

// ============================================================================
// HYPERLINK — display-only (T2 ruling)
// ============================================================================

/// `HYPERLINK(link_location, [friendly_name])` (registry
/// `sheet.fn.lookup.hyperlink`). T2 ruling: display-only — returns the
/// `friendly_name` text, or the `link_location` text when `friendly_name` is
/// absent or blank. The link is not navigable on the page surface (module
/// docs). An error in either argument propagates.
pub fn hyperlink(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let link = match args.first() {
        Some(Arg::Scalar(v)) => v.clone(),
        Some(Arg::Range(view)) => view.get(0, 0),
        None => return CellValue::Error(CellError::Value),
    };
    match args.get(1) {
        Some(Arg::Scalar(v)) => {
            let t = coerce::to_text(v);
            if t.is_empty() {
                CellValue::Text(coerce::to_text(&link))
            } else {
                CellValue::Text(t)
            }
        }
        Some(Arg::Range(view)) => {
            let v = view.get(0, 0);
            let t = coerce::to_text(&v);
            if t.is_empty() {
                CellValue::Text(coerce::to_text(&link))
            } else {
                CellValue::Text(t)
            }
        }
        None => CellValue::Text(coerce::to_text(&link)),
    }
}

// ============================================================================
// shared scalar readers
// ============================================================================

/// Read an argument as a number through [`coerce::to_number`] (a range uses
/// its top-left cell). Propagates a coercion error.
fn scalar_number(arg: Option<&Arg>) -> Result<f64, CellError> {
    let v = match arg {
        Some(Arg::Scalar(v)) => v.clone(),
        Some(Arg::Range(view)) => view.get(0, 0),
        None => return Err(CellError::Value),
    };
    coerce::to_number(&v)
}

/// Read an argument as its General text through [`coerce::to_text`] (a range
/// uses its top-left cell). A missing arg yields `""`.
fn scalar_text(arg: Option<&Arg>) -> compact_str::CompactString {
    match arg {
        Some(Arg::Scalar(v)) => coerce::to_text(v),
        Some(Arg::Range(view)) => coerce::to_text(&view.get(0, 0)),
        None => compact_str::CompactString::default(),
    }
}
