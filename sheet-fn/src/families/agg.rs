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

//! The aggregation family (spec §7, §11 T0): `AVERAGE`, `MIN`, `MAX`,
//! `COUNT`, `COUNTA`, `COUNTBLANK`, `SUMIF`, `COUNTIF`. Pure kernels of the
//! frozen signature `fn(&[Arg], &EvalCtx) -> CellValue`; arity is range-
//! checked by the generated [`crate::dispatch`] before these are called.
//!
//! ## The scalar/range asymmetry (ECMA-376 §18.17.7)
//!
//! Excel treats a value differently depending on whether it arrives as a
//! direct scalar argument or as a cell inside a range — the family's most
//! load-bearing ruling:
//!
//! - **`COUNT`** counts *numbers*. A **scalar** argument counts if it is a
//!   number, a boolean, or text that parses as a number (`COUNT(TRUE)` is
//!   `1`, `COUNT("5")` is `1`). A **range** cell counts only when it is
//!   actually stored as a number — a boolean or numeric-text cell *inside*
//!   a range does **not** count (`COUNT(A1:A2)` over `{TRUE, "5"}` is `0`).
//!   This asymmetry is intentional and tested.
//! - **`AVERAGE`/`MIN`/`MAX`** skip non-numeric cells *in ranges* (text,
//!   blanks, bools are ignored), but a **scalar** argument is coerced via
//!   [`crate::coerce::to_number`] (so a bool/numeric-text scalar
//!   participates and a non-numeric-text scalar is `#VALUE!`).
//!
//! Errors propagate: an [`CellValue::Error`] cell anywhere in an aggregated
//! range *is* the result (first error wins, scan order row-major). Scalar
//! errors propagate via [`crate::coerce::first_error`] where Excel does.
//! `MIN`/`MAX` of no numbers is `0`; `AVERAGE` of no numbers is `#DIV/0!`.

use sheet_core::{CellError, CellValue};

use crate::arg::{Arg, RangeView};
use crate::coerce;
use crate::criteria;
use crate::num::{Numeric, F64};

// ---- shared numeric scan ----------------------------------------------------

/// Accumulate the numeric values an `AVERAGE`/`MIN`/`MAX`/`SUM`-style
/// aggregation sees, honoring the range/scalar rules above. Returns the
/// first [`CellError`] encountered (anywhere — scalar arg or range cell) so
/// the caller can propagate it; otherwise pushes each participating number
/// into `acc`.
///
/// - Scalar args go through [`coerce::to_number`] (bool/numeric-text count;
///   un-parseable text → `#VALUE!`; errors propagate).
/// - Range cells: errors propagate; numbers participate; **everything else
///   (text, bool, blank) is skipped** — the range-skip ruling.
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
                        // Text / Bool / Empty inside a range are skipped.
                        _ => {}
                    }
                }
            }
        }
    }
    None
}

// ---- AVERAGE / MIN / MAX ----------------------------------------------------

/// `AVERAGE(value1, [value2], …)` (registry `sheet.fn.agg.average`). Mean of
/// the participating numbers (range-skip rules above). Averaging no numbers
/// is `#DIV/0!` (Excel) — an empty range or all-text args.
pub fn average(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let mut nums = Vec::new();
    if let Some(e) = collect_numbers(args, &mut nums) {
        return CellValue::Error(e);
    }
    if nums.is_empty() {
        return CellValue::Error(CellError::Div0);
    }
    let mut sum = F64::from_f64(0.0);
    for n in &nums {
        sum = sum.add(F64::from_f64(*n));
    }
    let mean = sum.div(F64::from_f64(nums.len() as f64));
    CellValue::Number(mean.to_f64())
}

/// `MIN(value1, [value2], …)` (registry `sheet.fn.agg.min`). Smallest
/// participating number; **`MIN` of nothing is `0`** (Excel) — not an error.
pub fn min(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let mut nums = Vec::new();
    if let Some(e) = collect_numbers(args, &mut nums) {
        return CellValue::Error(e);
    }
    let m = nums.iter().copied().fold(None, |acc: Option<f64>, n| {
        Some(acc.map_or(n, |a| if n < a { n } else { a }))
    });
    CellValue::Number(m.unwrap_or(0.0))
}

/// `MAX(value1, [value2], …)` (registry `sheet.fn.agg.max`). Largest
/// participating number; **`MAX` of nothing is `0`** (Excel) — not an error.
pub fn max(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let mut nums = Vec::new();
    if let Some(e) = collect_numbers(args, &mut nums) {
        return CellValue::Error(e);
    }
    let m = nums.iter().copied().fold(None, |acc: Option<f64>, n| {
        Some(acc.map_or(n, |a| if n > a { n } else { a }))
    });
    CellValue::Number(m.unwrap_or(0.0))
}

// ---- COUNT / COUNTA / COUNTBLANK -------------------------------------------

/// `COUNT(value1, [value2], …)` (registry `sheet.fn.agg.count`). Counts
/// numbers — but with the scalar/range asymmetry of the module doc:
///
/// - **scalar** arg counts iff it coerces to a number (number, bool, or
///   numeric text); an un-parseable-text or error scalar does NOT count and
///   does NOT propagate (COUNT never errors on its inputs);
/// - **range** cell counts iff it is stored as a [`CellValue::Number`] —
///   bools and numeric-text cells inside a range are excluded.
///
/// Returns the count as a number; `COUNT` is total and never errors.
pub fn count(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let mut n = 0u64;
    for a in args {
        match a {
            // Scalar: counts if coercible-as-number (bool/numeric-text yes).
            Arg::Scalar(v) => {
                if coerce::to_number(v).is_ok() {
                    n += 1;
                }
            }
            // Range: numeric cells only (bool/text inside a range excluded).
            Arg::Range(view) => {
                for cell in view.iter() {
                    if matches!(cell, CellValue::Number(_)) {
                        n += 1;
                    }
                }
            }
        }
    }
    CellValue::Number(n as f64)
}

/// `COUNTA(value1, [value2], …)` (registry `sheet.fn.agg.counta`). Counts
/// non-empty values: every scalar arg counts (a scalar is, by definition,
/// present), and every range cell that is not [`CellValue::Empty`] counts —
/// including text, bools, and even error cells (`COUNTA` counts the error
/// cell, it does not propagate). Total; never errors.
pub fn counta(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let mut n = 0u64;
    for a in args {
        match a {
            // A scalar argument is a present value -> counts. (Excel counts
            // even an empty-string scalar; only a truly blank cell ref, which
            // arrives as an Empty range cell, is skipped.)
            Arg::Scalar(_) => n += 1,
            Arg::Range(view) => {
                for cell in view.iter() {
                    if !cell.is_blank() {
                        n += 1;
                    }
                }
            }
        }
    }
    CellValue::Number(n as f64)
}

/// `COUNTBLANK(range)` (registry `sheet.fn.agg.countblank`). Counts the
/// **blank** ([`CellValue::Empty`]) cells of a single range argument
/// (arity locked to exactly one in the registry). A scalar argument has no
/// blank cells to count, so a scalar yields `0`. Total; never errors.
pub fn countblank(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let mut n = 0u64;
    if let Some(Arg::Range(view)) = args.first() {
        for cell in view.iter() {
            if cell.is_blank() {
                n += 1;
            }
        }
    }
    CellValue::Number(n as f64)
}

// ---- SUMIF / COUNTIF --------------------------------------------------------

/// `COUNTIF(range, criteria)` (registry `sheet.fn.agg.countif`). Counts the
/// cells of `range` that satisfy `criteria` (the shared [`criteria`] ruling:
/// operator prefixes, number↔text equality, wildcards, case-insensitive).
/// A non-range `range` argument is treated as a one-cell window. Total;
/// never errors (an error cell simply fails to match a non-error criterion).
pub fn countif(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let crit = match args.get(1) {
        Some(Arg::Scalar(v)) => criteria::parse_criteria(v),
        // A range as the criterion arg uses its top-left cell (implicit
        // intersection is the caller's job; we read the corner defensively).
        Some(Arg::Range(view)) => criteria::parse_criteria(&view.get(0, 0)),
        None => return CellValue::Error(CellError::Value),
    };
    let mut n = 0u64;
    for_each_range_cell(args.first(), |cell| {
        if criteria::matches(&crit, &cell) {
            n += 1;
        }
    });
    CellValue::Number(n as f64)
}

/// `SUMIF(range, criteria, [sum_range])` (registry `sheet.fn.agg.sumif`).
/// Sums the cells of `sum_range` whose **positionally aligned** cell in
/// `range` satisfies `criteria`. When `sum_range` is omitted it defaults to
/// `range` itself (Excel). Alignment is **by relative offset** from each
/// range's top-left — Excel sizes `sum_range` to match `range` regardless of
/// the literal `sum_range` extent, so we index `sum_range` by the same
/// `(r, c)` we test in `range`.
///
/// Only numeric `sum_range` cells contribute (non-numeric matches add `0`);
/// an [`CellValue::Error`] in a *contributing* `sum_range` cell propagates.
pub fn sumif(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let crit = match args.get(1) {
        Some(Arg::Scalar(v)) => criteria::parse_criteria(v),
        Some(Arg::Range(view)) => criteria::parse_criteria(&view.get(0, 0)),
        None => return CellValue::Error(CellError::Value),
    };

    // The criteria range and (optional) sum range, as views. A scalar in
    // either slot becomes a 1x1 window so the offset alignment is uniform.
    let crit_arg = args.first();
    let sum_arg = args.get(2).or(crit_arg); // default sum_range = range.

    let crit_view = as_view(crit_arg);
    let sum_view = as_view(sum_arg);

    let (rows, cols) = crit_view.as_ref().map_or((0, 0), |v| (v.rows(), v.cols()));

    let mut sum = F64::from_f64(0.0);
    for r in 0..rows {
        for c in 0..cols {
            let cand = crit_view.as_ref().map_or(CellValue::Empty, |v| v.get(r, c));
            if !criteria::matches(&crit, &cand) {
                continue;
            }
            // Aligned by offset into sum_range.
            let target = sum_view.as_ref().map_or(CellValue::Empty, |v| v.get(r, c));
            match target {
                CellValue::Number(n) => sum = sum.add(F64::from_f64(n)),
                CellValue::Error(e) => return CellValue::Error(e),
                // Text / Bool / Empty contribute 0.
                _ => {}
            }
        }
    }
    CellValue::Number(sum.to_f64())
}

// ---- small helpers ----------------------------------------------------------

/// Borrow an [`Arg`] as a [`RangeView`] when it is a range; scalars (and a
/// missing arg) yield `None` so a 1x1 scalar window can be synthesized by
/// the caller through [`for_each_range_cell`] / explicit reads. Used by the
/// `*IF` kernels, which iterate by relative offset.
fn as_view<'a, 'b>(arg: Option<&'a Arg<'b>>) -> Option<&'a RangeView<'b>> {
    match arg {
        Some(Arg::Range(v)) => Some(v),
        _ => None,
    }
}

/// Run `f` over each cell of a range argument; a scalar argument is a single
/// cell, a missing argument is none. Keeps the `COUNTIF` scan uniform across
/// the range/scalar cases without materializing.
fn for_each_range_cell(arg: Option<&Arg>, mut f: impl FnMut(CellValue)) {
    match arg {
        Some(Arg::Range(view)) => {
            for cell in view.iter() {
                f(cell);
            }
        }
        Some(Arg::Scalar(v)) => f(v.clone()),
        None => {}
    }
}
