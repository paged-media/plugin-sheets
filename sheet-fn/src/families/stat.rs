/*
 * This file is part of paged (https://paged.media).
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 * paged is distributed in the hope that it will be useful, but WITHOUT ANY
 * WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
 * FOR A PARTICULAR PURPOSE. See the licenses for details.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

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

//! The statistics family (spec §7, §11/§13 M1): the 30 T1 stat rows. Pure
//! kernels of the frozen signature `fn(&[Arg], &EvalCtx) -> CellValue`; arity
//! is range-checked by the generated [`crate::dispatch`] before these run.
//!
//! ## Shared range/scalar rulings (mirrors the `agg`/`math` families)
//!
//! Every accumulating kernel treats its variadic value arguments the same way
//! (ECMA-376 §18.17.7 aggregation rule, replicated in `families::agg`):
//!
//! - a **scalar** argument is coerced via [`crate::coerce::to_number`] — the
//!   bool `TRUE`→`1`, numeric text `"3"`→`3`, un-parseable text →`#VALUE!`;
//! - a **range** cell participates only when it is a stored
//!   [`CellValue::Number`]; text, bools, and blanks inside a range are
//!   **skipped**; an [`CellValue::Error`] cell anywhere propagates (first
//!   error wins, row-major scan).
//!
//! ## Excel rulings honored here (each is a registry-tested feature)
//!
//! - **MEDIAN/MODE/PERCENTILE.INC/QUARTILE.INC** sort a copied number vector.
//!   `PERCENTILE.INC` does **linear interpolation** between the two bracketing
//!   ranks (`k` in `0..=1`, out of range → `#NUM!`). `QUARTILE.INC(data, q)`
//!   is `PERCENTILE.INC(data, q/4)` for integer `q` in `0..=4` (`q` is
//!   truncated; out of range → `#NUM!`). `MEDIAN` of no numbers → `#NUM!`;
//!   `MODE` with no repeated value → `#N/A` (the most frequent value, ties
//!   broken by first appearance).
//! - **STDEV.S/VAR.S** are the **sample** estimators (divide by `n-1`);
//!   **STDEV.P/VAR.P** are the **population** estimators (divide by `n`). The
//!   legacy aliases follow Excel: `STDEV`/`VAR` == sample, `STDEVP`/`VARP` ==
//!   population. Sample estimators need at least 2 numbers (else `#DIV/0!`);
//!   population estimators need at least 1 (else `#DIV/0!`).
//! - **LARGE/SMALL** return the `k`-th largest / smallest of a sorted copy;
//!   `k` is truncated to an integer and `k < 1` or `k >` count → `#NUM!`.
//! - **RANK.EQ/RANK** rank a number within a reference; default order `0`
//!   (descending), any nonzero `order` is ascending; ties share the top rank;
//!   a number absent from the reference → `#N/A`.
//! - **COUNTIFS/SUMIFS/AVERAGEIFS/MAXIFS/MINIFS** take variadic
//!   `criteria_range, criteria` pairs (SUM/AVERAGE/MAX/MINIFS lead with the
//!   value range), AND across every pair, offset-aligned; the criteria ranges
//!   must share the leading range's shape (else `#VALUE!`). `AVERAGEIFS` over
//!   an empty match set → `#DIV/0!`; `MAXIFS`/`MINIFS` over an empty set →
//!   `0` (Excel).
//! - **SUMPRODUCT** multiplies the offset-aligned cells of equal-shaped ranges
//!   and sums the products; a non-numeric cell contributes `0` (it does **not**
//!   error); mismatched shapes → `#VALUE!`; an error cell propagates.
//! - **GEOMEAN/HARMEAN** need strictly positive numbers (`≤ 0` → `#NUM!`);
//!   **AVEDEV** is the mean absolute deviation; **DEVSQ** the sum of squared
//!   deviations from the mean.
//! - **CORREL/SLOPE/INTERCEPT/RSQ** take paired `y`/`x` ranges of equal shape
//!   (mismatched shape → `#N/A`); a degenerate spread (`SLOPE`/`INTERCEPT`/
//!   `RSQ` with zero variance in `x`, `CORREL` with zero variance in either)
//!   → `#DIV/0!`. Cells are paired position-wise; a pair is dropped when
//!   either side is non-numeric.

use sheet_core::{CellError, CellValue};

use crate::arg::{Arg, RangeView};
use crate::coerce;
use crate::criteria;
use crate::ctx::EvalCtx;
use crate::num::{Numeric, F64};

// ---- shared numeric collection ---------------------------------------------

/// Gather every participating number across all value args (the shared
/// range/scalar ruling above). Returns the first [`CellError`] met (a scalar
/// coercion failure or a range error cell) so the caller can propagate it;
/// otherwise the numbers are pushed into `acc` in scan order.
fn collect_numbers(args: &[Arg], acc: &mut Vec<f64>) -> Option<CellError> {
    for a in args {
        match a {
            Arg::Scalar(v) => match coerce::to_number(v) {
                Ok(n) => acc.push(n),
                Err(e) => return Some(e),
            },
            Arg::Range(view) => {
                for cell in view.iter() {
                    match cell {
                        CellValue::Number(n) => acc.push(n),
                        CellValue::Error(e) => return Some(e),
                        _ => {}
                    }
                }
            }
        }
    }
    None
}

/// Collect into a fresh vector, returning either the numbers or the propagated
/// error. The spine of the order-statistic + summary kernels.
fn numbers_or_error(args: &[Arg]) -> Result<Vec<f64>, CellError> {
    let mut v = Vec::new();
    match collect_numbers(args, &mut v) {
        Some(e) => Err(e),
        None => Ok(v),
    }
}

/// Sum of a slice through the [`Numeric`] seam (D-6).
fn sum(values: &[f64]) -> f64 {
    let mut acc = F64::from_f64(0.0);
    for &v in values {
        acc = acc.add(F64::from_f64(v));
    }
    acc.to_f64()
}

/// Arithmetic mean of a non-empty slice (caller guarantees non-empty).
fn mean(values: &[f64]) -> f64 {
    sum(values) / values.len() as f64
}

/// A sorted ascending copy of `values` (total order; values never NaN since
/// they came from stored [`CellValue::Number`]s / coercion).
fn sorted(values: &[f64]) -> Vec<f64> {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Map a non-finite outcome to `#NUM!` (overflow / domain), keeping kernels
/// total. Real in-domain results are finite.
#[inline]
fn finite(n: f64) -> CellValue {
    if n.is_finite() {
        CellValue::Number(n)
    } else {
        CellValue::Error(CellError::Num)
    }
}

// ---- MEDIAN / MODE ---------------------------------------------------------

/// `MEDIAN(number1, …)` (registry `sheet.fn.stat.median`). Middle of the
/// sorted numbers; the mean of the two middles for an even count. No numbers
/// → `#NUM!`.
pub fn median(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let nums = match numbers_or_error(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if nums.is_empty() {
        return CellValue::Error(CellError::Num);
    }
    let s = sorted(&nums);
    let n = s.len();
    let m = if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    };
    CellValue::Number(m)
}

/// `MODE(number1, …)` (registry `sheet.fn.stat.mode`). The most frequently
/// occurring value; ties resolve to the value that appears first. No value
/// repeats → `#N/A` (Excel). No numbers at all → `#N/A` as well.
pub fn mode(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let nums = match numbers_or_error(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    // Count occurrences preserving first-seen order for the tie-break.
    let mut order: Vec<f64> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for &x in &nums {
        if let Some(i) = order.iter().position(|&v| v == x) {
            counts[i] += 1;
        } else {
            order.push(x);
            counts.push(1);
        }
    }
    let mut best: Option<usize> = None;
    for (i, &c) in counts.iter().enumerate() {
        if c >= 2 && best.is_none_or(|b| c > counts[b]) {
            best = Some(i);
        }
    }
    match best {
        Some(i) => CellValue::Number(order[i]),
        None => CellValue::Error(CellError::Na),
    }
}

// ---- PERCENTILE.INC / QUARTILE.INC -----------------------------------------

/// The inclusive linear-interpolation percentile of a sorted slice for
/// `k` in `0..=1` (Excel `PERCENTILE.INC`): rank `= k * (n - 1)`, interpolate
/// between the floor and ceil ranks. `s` MUST be non-empty and sorted.
fn percentile_of_sorted(s: &[f64], k: f64) -> f64 {
    let n = s.len();
    if n == 1 {
        return s[0];
    }
    let rank = k * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let frac = rank - lo as f64;
    if lo + 1 >= n {
        s[n - 1]
    } else {
        s[lo] + frac * (s[lo + 1] - s[lo])
    }
}

/// `PERCENTILE.INC(array, k)` (registry `sheet.fn.stat.percentile-inc`).
/// Inclusive percentile with linear interpolation; `k` outside `0..=1` →
/// `#NUM!`; no numbers → `#NUM!`.
pub fn percentile_inc(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let k = match scalar_number(args.get(1)) {
        Ok(k) => k,
        Err(e) => return CellValue::Error(e),
    };
    let nums = match numbers_or_error(&args[..1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if nums.is_empty() {
        return CellValue::Error(CellError::Num);
    }
    if !(0.0..=1.0).contains(&k) {
        return CellValue::Error(CellError::Num);
    }
    let s = sorted(&nums);
    finite(percentile_of_sorted(&s, k))
}

/// `QUARTILE.INC(array, quart)` (registry `sheet.fn.stat.quartile-inc`).
/// `quart` truncated to an integer in `0..=4`; equals
/// `PERCENTILE.INC(array, quart/4)`. Out of range → `#NUM!`; no numbers →
/// `#NUM!`.
pub fn quartile_inc(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let q_raw = match scalar_number(args.get(1)) {
        Ok(q) => q,
        Err(e) => return CellValue::Error(e),
    };
    let nums = match numbers_or_error(&args[..1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if nums.is_empty() {
        return CellValue::Error(CellError::Num);
    }
    let q = q_raw.trunc();
    if !(0.0..=4.0).contains(&q) {
        return CellValue::Error(CellError::Num);
    }
    let s = sorted(&nums);
    finite(percentile_of_sorted(&s, q / 4.0))
}

// ---- STDEV / VAR (sample + population, legacy aliases) ----------------------

/// Sum of squared deviations from the mean (`Σ(xᵢ − x̄)²`). `values`
/// non-empty.
fn sum_sq_dev(values: &[f64]) -> f64 {
    let m = mean(values);
    let mut acc = F64::from_f64(0.0);
    for &v in values {
        let d = F64::from_f64(v - m);
        acc = acc.add(d.mul(d));
    }
    acc.to_f64()
}

/// Variance with `divisor` degrees-of-freedom adjustment. Returns `#DIV/0!`
/// when there are not enough numbers (`n <= ddof_offset`).
fn variance(args: &[Arg], population: bool) -> CellValue {
    let nums = match numbers_or_error(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let n = nums.len();
    let denom = if population { n } else { n.saturating_sub(1) };
    if denom == 0 {
        return CellValue::Error(CellError::Div0);
    }
    finite(sum_sq_dev(&nums) / denom as f64)
}

fn std_dev(args: &[Arg], population: bool) -> CellValue {
    match variance(args, population) {
        CellValue::Number(v) => finite(v.sqrt()),
        other => other,
    }
}

/// `STDEV.S(number1, …)` — sample standard deviation (`n−1`).
pub fn stdev_s(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    std_dev(args, false)
}
/// `STDEV.P(number1, …)` — population standard deviation (`n`).
pub fn stdev_p(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    std_dev(args, true)
}
/// `VAR.S(number1, …)` — sample variance (`n−1`).
pub fn var_s(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    variance(args, false)
}
/// `VAR.P(number1, …)` — population variance (`n`).
pub fn var_p(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    variance(args, true)
}
/// `STDEV(number1, …)` — legacy alias of [`stdev_s`] (sample).
pub fn stdev(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    std_dev(args, false)
}
/// `STDEVP(number1, …)` — legacy alias of [`stdev_p`] (population).
pub fn stdevp(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    std_dev(args, true)
}
/// `VAR(number1, …)` — legacy alias of [`var_s`] (sample).
pub fn var(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    variance(args, false)
}
/// `VARP(number1, …)` — legacy alias of [`var_p`] (population).
pub fn varp(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    variance(args, true)
}

// ---- LARGE / SMALL ---------------------------------------------------------

/// Shared k-th order statistic. `largest` selects from the top when true.
fn kth(args: &[Arg], largest: bool) -> CellValue {
    let k_raw = match scalar_number(args.get(1)) {
        Ok(k) => k,
        Err(e) => return CellValue::Error(e),
    };
    let nums = match numbers_or_error(&args[..1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let n = nums.len();
    let k = k_raw.trunc();
    // k must be an integer in 1..=n (Excel truncates k, then bounds-checks).
    if n == 0 || k < 1.0 || k > n as f64 {
        return CellValue::Error(CellError::Num);
    }
    let s = sorted(&nums);
    let idx = if largest {
        n - k as usize
    } else {
        k as usize - 1
    };
    CellValue::Number(s[idx])
}

/// `LARGE(array, k)` — the `k`-th largest value (`k=1` is the maximum).
pub fn large(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    kth(args, true)
}
/// `SMALL(array, k)` — the `k`-th smallest value (`k=1` is the minimum).
pub fn small(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    kth(args, false)
}

// ---- RANK.EQ / RANK --------------------------------------------------------

/// `RANK.EQ(number, ref, [order])` (and the legacy `RANK` alias). The rank of
/// `number` within the numeric cells of `ref`; default `order` `0` ranks
/// descending (largest = rank 1), any nonzero `order` ranks ascending. Ties
/// share the top rank. `number` absent from `ref` → `#N/A`.
fn rank_impl(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let needle = match scalar_number(args.first()) {
        Ok(x) => x,
        Err(e) => return CellValue::Error(e),
    };
    // The reference range is value arg #2; collect its numbers (errors
    // propagate).
    let pool = match numbers_or_error(&args[1..2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let ascending = match args.get(2) {
        None => false,
        Some(_) => match scalar_number(args.get(2)) {
            Ok(o) => o != 0.0,
            Err(e) => return CellValue::Error(e),
        },
    };
    if !pool.contains(&needle) {
        return CellValue::Error(CellError::Na);
    }
    // Rank = 1 + number of values strictly better (larger for descending,
    // smaller for ascending). Ties therefore share the top rank.
    let better = pool
        .iter()
        .filter(|&&v| if ascending { v < needle } else { v > needle })
        .count();
    CellValue::Number((better + 1) as f64)
}

/// `RANK.EQ(number, ref, [order])` (registry `sheet.fn.stat.rank-eq`).
pub fn rank_eq(args: &[Arg], ctx: &EvalCtx) -> CellValue {
    rank_impl(args, ctx)
}
/// `RANK(number, ref, [order])` — legacy alias of [`rank_eq`].
pub fn rank(args: &[Arg], ctx: &EvalCtx) -> CellValue {
    rank_impl(args, ctx)
}

// ---- COUNTIFS / SUMIFS / AVERAGEIFS / MAXIFS / MINIFS -----------------------

/// Borrow an arg as a [`RangeView`] (scalars / missing → `None`).
fn as_view<'a, 'b>(arg: Option<&'a Arg<'b>>) -> Option<&'a RangeView<'b>> {
    match arg {
        Some(Arg::Range(v)) => Some(v),
        _ => None,
    }
}

/// A criteria range paired with its compiled criterion.
struct Pair<'a, 'b> {
    range: &'a RangeView<'b>,
    crit: criteria::Criteria,
}

/// Parse the variadic `(criteria_range, criteria)` pairs starting at `start`.
/// On success returns the pairs and the shared `(rows, cols)` shape; a missing
/// criteria value, a non-range criteria_range, or a shape mismatch is
/// `#VALUE!`.
fn parse_pairs<'a, 'b>(
    args: &'a [Arg<'b>],
    start: usize,
) -> Result<(Vec<Pair<'a, 'b>>, u32, u32), CellError> {
    // Pairs come in twos; an odd tail is malformed.
    if !(args.len() - start).is_multiple_of(2) || args.len() == start {
        return Err(CellError::Value);
    }
    let mut pairs = Vec::new();
    let mut shape: Option<(u32, u32)> = None;
    let mut i = start;
    while i < args.len() {
        let range = as_view(args.get(i)).ok_or(CellError::Value)?;
        let crit = match args.get(i + 1) {
            Some(Arg::Scalar(v)) => criteria::parse_criteria(v),
            Some(Arg::Range(v)) => criteria::parse_criteria(&v.get(0, 0)),
            None => return Err(CellError::Value),
        };
        let this = (range.rows(), range.cols());
        match shape {
            None => shape = Some(this),
            Some(s) if s != this => return Err(CellError::Value),
            _ => {}
        }
        pairs.push(Pair { range, crit });
        i += 2;
    }
    let (rows, cols) = shape.unwrap_or((0, 0));
    Ok((pairs, rows, cols))
}

/// True when every pair's offset-aligned cell satisfies its criterion (AND).
fn all_match(pairs: &[Pair], r: u32, c: u32) -> bool {
    pairs
        .iter()
        .all(|p| criteria::matches(&p.crit, &p.range.get(r, c)))
}

/// `COUNTIFS(criteria_range1, criteria1, …)` (registry
/// `sheet.fn.stat.countifs`). Counts the offset positions where ALL pairs
/// match. Total; never errors except a malformed/mismatched-shape call
/// (`#VALUE!`).
pub fn countifs(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let (pairs, rows, cols) = match parse_pairs(args, 0) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    let mut n = 0u64;
    for r in 0..rows {
        for c in 0..cols {
            if all_match(&pairs, r, c) {
                n += 1;
            }
        }
    }
    CellValue::Number(n as f64)
}

/// The matched numeric values of `value_range` where every pair matches. The
/// value range is offset-aligned with the criteria ranges. Returns the
/// propagated error if a contributing value cell is an error, else the matched
/// numbers (non-numeric matched cells contribute nothing).
fn ifs_values(
    value: Option<&RangeView>,
    pairs: &[Pair],
    rows: u32,
    cols: u32,
) -> Result<Vec<f64>, CellError> {
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            if !all_match(pairs, r, c) {
                continue;
            }
            let cell = value.map_or(CellValue::Empty, |v| v.get(r, c));
            match cell {
                CellValue::Number(n) => out.push(n),
                CellValue::Error(e) => return Err(e),
                _ => {}
            }
        }
    }
    Ok(out)
}

/// Shared front-end for `SUMIFS`/`AVERAGEIFS`/`MAXIFS`/`MINIFS`: the leading
/// value range, then variadic pairs. `reduce` turns the matched numbers into a
/// result (so each kernel only states its aggregation + empty-set ruling). A
/// value range whose shape differs from the criteria ranges is `#VALUE!`.
fn ifs(args: &[Arg], reduce: impl FnOnce(Vec<f64>) -> CellValue) -> CellValue {
    let value = match args.first() {
        Some(Arg::Range(v)) => Some(v),
        // A scalar value range is degenerate but treated as a 1×1 below via
        // the shape check; in practice sheet-calc always hands a range here.
        _ => None,
    };
    let (pairs, rows, cols) = match parse_pairs(args, 1) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    // The value range must match the criteria-range shape (Excel).
    if let Some(v) = value {
        if (v.rows(), v.cols()) != (rows, cols) {
            return CellValue::Error(CellError::Value);
        }
    }
    match ifs_values(value, &pairs, rows, cols) {
        Ok(vals) => reduce(vals),
        Err(e) => CellValue::Error(e),
    }
}

/// `SUMIFS(sum_range, criteria_range1, criteria1, …)` (registry
/// `sheet.fn.stat.sumifs`). Sum of the matched numbers; empty match → `0`.
pub fn sumifs(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    ifs(args, |vals| CellValue::Number(sum(&vals)))
}

/// `AVERAGEIFS(average_range, criteria_range1, criteria1, …)` (registry
/// `sheet.fn.stat.averageifs`). Mean of the matched numbers; **empty match →
/// `#DIV/0!`** (Excel).
pub fn averageifs(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    ifs(args, |vals| {
        if vals.is_empty() {
            CellValue::Error(CellError::Div0)
        } else {
            CellValue::Number(mean(&vals))
        }
    })
}

/// `MAXIFS(max_range, criteria_range1, criteria1, …)` (registry
/// `sheet.fn.stat.maxifs`). Largest matched number; empty match → `0` (Excel).
pub fn maxifs(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    ifs(args, |vals| {
        let m = vals
            .iter()
            .copied()
            .fold(None, |a: Option<f64>, n| Some(a.map_or(n, |x| x.max(n))));
        CellValue::Number(m.unwrap_or(0.0))
    })
}

/// `MINIFS(min_range, criteria_range1, criteria1, …)` (registry
/// `sheet.fn.stat.minifs`). Smallest matched number; empty match → `0`
/// (Excel).
pub fn minifs(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    ifs(args, |vals| {
        let m = vals
            .iter()
            .copied()
            .fold(None, |a: Option<f64>, n| Some(a.map_or(n, |x| x.min(n))));
        CellValue::Number(m.unwrap_or(0.0))
    })
}

// ---- SUMPRODUCT ------------------------------------------------------------

/// `SUMPRODUCT(array1, [array2], …)` (registry `sheet.fn.stat.sumproduct`).
/// Element-wise product of the offset-aligned cells of equal-shaped ranges,
/// summed. A non-numeric cell contributes `0` to its product position (so a
/// whole product term goes to 0); an error cell propagates. Mismatched shapes
/// → `#VALUE!`. A scalar argument acts as a 1×1 array. With a single array,
/// `SUMPRODUCT` is just the sum of its numeric cells.
pub fn sumproduct(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    // Establish the common shape from the first range arg; a scalar is 1×1.
    let mut shape: Option<(u32, u32)> = None;
    for a in args {
        let s = match a {
            Arg::Range(v) => (v.rows(), v.cols()),
            Arg::Scalar(_) => (1, 1),
        };
        match shape {
            None => shape = Some(s),
            Some(prev) if prev != s => return CellValue::Error(CellError::Value),
            _ => {}
        }
    }
    let (rows, cols) = shape.unwrap_or((0, 0));

    let mut total = F64::from_f64(0.0);
    for r in 0..rows {
        for c in 0..cols {
            let mut prod = F64::from_f64(1.0);
            for a in args {
                let cell = match a {
                    Arg::Range(v) => v.get(r, c),
                    Arg::Scalar(v) => v.clone(),
                };
                match cell {
                    CellValue::Number(n) => prod = prod.mul(F64::from_f64(n)),
                    CellValue::Error(e) => return CellValue::Error(e),
                    // Non-numeric cell → this product term is 0 (Excel).
                    _ => prod = F64::from_f64(0.0),
                }
            }
            total = total.add(prod);
        }
    }
    finite(total.to_f64())
}

// ---- GEOMEAN / HARMEAN / AVEDEV / DEVSQ ------------------------------------

/// `GEOMEAN(number1, …)` (registry `sheet.fn.stat.geomean`). The geometric
/// mean `(Πxᵢ)^(1/n)`; every value must be strictly positive (`≤ 0` →
/// `#NUM!`). No numbers → `#NUM!`. Computed in log-space for range safety.
pub fn geomean(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let nums = match numbers_or_error(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if nums.is_empty() || nums.iter().any(|&x| x <= 0.0) {
        return CellValue::Error(CellError::Num);
    }
    let log_sum: f64 = nums.iter().map(|&x| x.ln()).sum();
    finite((log_sum / nums.len() as f64).exp())
}

/// `HARMEAN(number1, …)` (registry `sheet.fn.stat.harmean`). The harmonic
/// mean `n / Σ(1/xᵢ)`; every value must be strictly positive (`≤ 0` →
/// `#NUM!`). No numbers → `#NUM!`.
pub fn harmean(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let nums = match numbers_or_error(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if nums.is_empty() || nums.iter().any(|&x| x <= 0.0) {
        return CellValue::Error(CellError::Num);
    }
    let recip_sum: f64 = nums.iter().map(|&x| 1.0 / x).sum();
    finite(nums.len() as f64 / recip_sum)
}

/// `AVEDEV(number1, …)` (registry `sheet.fn.stat.avedev`). The mean of the
/// absolute deviations from the mean. No numbers → `#NUM!`.
pub fn avedev(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let nums = match numbers_or_error(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if nums.is_empty() {
        return CellValue::Error(CellError::Num);
    }
    let m = mean(&nums);
    let total: f64 = nums.iter().map(|&x| (x - m).abs()).sum();
    finite(total / nums.len() as f64)
}

/// `DEVSQ(number1, …)` (registry `sheet.fn.stat.devsq`). The sum of squared
/// deviations from the mean. No numbers → `#NUM!`.
pub fn devsq(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let nums = match numbers_or_error(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if nums.is_empty() {
        return CellValue::Error(CellError::Num);
    }
    finite(sum_sq_dev(&nums))
}

// ---- CORREL / SLOPE / INTERCEPT / RSQ (paired regression) ------------------

/// The aligned numeric pairs `(y, x)` of two equal-shaped ranges. A pair is
/// dropped when EITHER side is non-numeric; an error cell on either side
/// propagates. Mismatched shape → `#N/A` (Excel's array-mismatch ruling for
/// these functions).
fn paired(args: &[Arg]) -> Result<Vec<(f64, f64)>, CellError> {
    let ys = as_view(args.first());
    let xs = as_view(args.get(1));
    let (yr, yc) = ys.map_or((0, 0), |v| (v.rows(), v.cols()));
    let (xr, xc) = xs.map_or((0, 0), |v| (v.rows(), v.cols()));
    // Different cardinalities are #N/A; same total count is enough (Excel
    // pairs in scan order even across differing rectangle shapes).
    if (yr as u64 * yc as u64) != (xr as u64 * xc as u64) {
        return Err(CellError::Na);
    }
    let yv: Vec<CellValue> = ys.map(|v| v.iter().collect()).unwrap_or_default();
    let xv: Vec<CellValue> = xs.map(|v| v.iter().collect()).unwrap_or_default();
    let mut out = Vec::new();
    for (yc, xc) in yv.iter().zip(xv.iter()) {
        if let CellValue::Error(e) = yc {
            return Err(*e);
        }
        if let CellValue::Error(e) = xc {
            return Err(*e);
        }
        if let (CellValue::Number(y), CellValue::Number(x)) = (yc, xc) {
            out.push((*y, *x));
        }
    }
    Ok(out)
}

/// The regression sums over the aligned pairs: `(n, Σx, Σy, Σxy, Σx², Σy²)`.
fn reg_sums(pairs: &[(f64, f64)]) -> (f64, f64, f64, f64, f64, f64) {
    let n = pairs.len() as f64;
    let (mut sx, mut sy, mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(y, x) in pairs {
        sx += x;
        sy += y;
        sxy += x * y;
        sxx += x * x;
        syy += y * y;
    }
    (n, sx, sy, sxy, sxx, syy)
}

/// `CORREL(array1, array2)` (registry `sheet.fn.stat.correl`). The Pearson
/// correlation coefficient. Zero variance on either side → `#DIV/0!`; shape
/// mismatch → `#N/A`.
pub fn correl(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let pairs = match paired(args) {
        Ok(p) => p,
        Err(e) => return CellValue::Error(e),
    };
    if pairs.is_empty() {
        return CellValue::Error(CellError::Div0);
    }
    let (n, sx, sy, sxy, sxx, syy) = reg_sums(&pairs);
    let cov = n * sxy - sx * sy;
    let var_x = n * sxx - sx * sx;
    let var_y = n * syy - sy * sy;
    let denom = (var_x * var_y).sqrt();
    if denom == 0.0 {
        return CellValue::Error(CellError::Div0);
    }
    finite(cov / denom)
}

/// `SLOPE(known_ys, known_xs)` (registry `sheet.fn.stat.slope`). The slope of
/// the least-squares line. Zero variance in `x` → `#DIV/0!`; shape mismatch →
/// `#N/A`.
pub fn slope(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let pairs = match paired(args) {
        Ok(p) => p,
        Err(e) => return CellValue::Error(e),
    };
    if pairs.is_empty() {
        return CellValue::Error(CellError::Div0);
    }
    let (n, sx, sy, sxy, sxx, _) = reg_sums(&pairs);
    let var_x = n * sxx - sx * sx;
    if var_x == 0.0 {
        return CellValue::Error(CellError::Div0);
    }
    finite((n * sxy - sx * sy) / var_x)
}

/// `INTERCEPT(known_ys, known_xs)` (registry `sheet.fn.stat.intercept`). The
/// y-intercept of the least-squares line (`ȳ − slope·x̄`). Zero variance in
/// `x` → `#DIV/0!`; shape mismatch → `#N/A`.
pub fn intercept(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let pairs = match paired(args) {
        Ok(p) => p,
        Err(e) => return CellValue::Error(e),
    };
    if pairs.is_empty() {
        return CellValue::Error(CellError::Div0);
    }
    let (n, sx, sy, sxy, sxx, _) = reg_sums(&pairs);
    let var_x = n * sxx - sx * sx;
    if var_x == 0.0 {
        return CellValue::Error(CellError::Div0);
    }
    let slope = (n * sxy - sx * sy) / var_x;
    finite((sy - slope * sx) / n)
}

/// `RSQ(known_ys, known_xs)` (registry `sheet.fn.stat.rsq`). The square of the
/// Pearson correlation coefficient. Zero variance on either side → `#DIV/0!`;
/// shape mismatch → `#N/A`.
pub fn rsq(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match correl(args, _ctx) {
        CellValue::Number(r) => finite(r * r),
        other => other,
    }
}

// ---- small helper ----------------------------------------------------------

/// Coerce one trailing scalar arg to a number, propagating errors. A range in
/// the slot reads its top-left cell (the implicit-intersection fallback the
/// math family uses), so `k`/`order`/`quart`/`needle` arrive uniformly.
fn scalar_number(arg: Option<&Arg>) -> Result<f64, CellError> {
    match arg {
        Some(Arg::Scalar(v)) => coerce::to_number(v),
        Some(Arg::Range(r)) => coerce::to_number(&r.get(0, 0)),
        None => Err(CellError::Value),
    }
}
