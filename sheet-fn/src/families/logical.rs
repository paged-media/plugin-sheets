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

//! The logical function family (spec §7, §11 T0): `IF`, `AND`, `OR`, `NOT`,
//! `TRUE`, `FALSE`, `IFERROR` (ECMA-376 §18.17.7). Each kernel is the frozen
//! pure signature `fn(&[Arg], &EvalCtx) -> CellValue`; arity is enforced by
//! the generated [`crate::dispatch`] before the kernel runs, so a kernel may
//! assume its argument count is in `[min, max]`. All type conversion and
//! error propagation flow through [`crate::coerce`] so the rulings are stated
//! once (repo constitution, §"Excel-compat is a ruling").
//!
//! # KNOWN T0 DEVIATION — eager evaluation (no short-circuit)
//!
//! Excel's `IF`/`AND`/`OR`/`IFERROR` are **lazy**: only the taken branch (and,
//! for `AND`/`OR`, only arguments up to the decisive one) is evaluated, and
//! `IFERROR(x, y)` evaluates `y` only when `x` errored. At T0 the calling
//! convention hands a kernel its arguments **already evaluated** — both `IF`
//! branches are computed, every `AND`/`OR` argument is computed, and
//! `IFERROR`'s fallback is computed even when the primary is fine. This is
//! semantically correct for **value** results (these functions just select
//! among already-computed values), but it changes two observable things vs
//! Excel:
//!   1. side-effect-free cost — a discarded branch still pays its evaluation
//!      (irrelevant to results; a performance note only); and
//!   2. **error masking does not extend to the discarded branch** — e.g.
//!      `IF(TRUE, 1, 1/0)` is `1` in Excel because the `1/0` branch is never
//!      evaluated; under T0 eager evaluation the `1/0` is computed up the
//!      stack and would surface as `#DIV/0!` *before* this kernel runs.
//!
//! Short-circuit requires these functions to be **special forms** the
//! evaluator handles before argument evaluation (the evaluator must own the
//! "don't evaluate the other branch" decision — a pure `fn(&[Arg], …)` kernel
//! physically cannot, since it never sees the unevaluated AST). This is queued
//! for `sheet-calc` Phase 2: `IF`/`AND`/`OR`/`IFERROR`/`IFS`/`CHOOSE` become
//! lazy special forms dispatched ahead of the eager kernel path; these kernels
//! then remain the value-selection fallback for the fully-evaluated case.
//! **sheet-calc agent: see this note before wiring lazy dispatch.**

use sheet_core::{CellError, CellValue};

use crate::arg::{Arg, RangeView};
use crate::coerce;
use crate::ctx::EvalCtx;

/// `IF(condition, value_if_true, [value_if_false])` — select a value by a
/// boolean test (ECMA-376 §18.17.7). The condition is coerced via
/// [`coerce::to_bool`] (so `0`/non-zero numbers and the `"TRUE"`/`"FALSE"`
/// text literals work; other text → `#VALUE!`); an error condition
/// propagates. With the third argument omitted, a FALSE condition yields the
/// boolean `FALSE` (Excel's documented default), not `Empty`.
///
/// T0 eager note: both branches arrive already evaluated — see the module
/// docs. A branch is returned verbatim (it is whatever value the convention
/// handed us), so an error *in the selected branch* surfaces unchanged.
pub fn if_fn(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    // Arity (2..=3) is guaranteed by the generated dispatch.
    let cond = match scalar_to_bool(&args[0]) {
        Ok(b) => b,
        Err(e) => return CellValue::Error(e),
    };
    if cond {
        arg_value(&args[1])
    } else if args.len() >= 3 {
        arg_value(&args[2])
    } else {
        CellValue::Bool(false)
    }
}

/// `AND(logical1, [logical2], …)` — TRUE iff every logical value is TRUE
/// (ECMA-376 §18.17.7). Range-aware: ranges are scanned cell-by-cell, **text
/// and blank cells inside a range are ignored**, numbers/bools count, and an
/// **error cell propagates**. Scalar arguments are coerced strictly via
/// [`coerce::to_bool`] (non-boolean text → `#VALUE!`). If no logical value is
/// found among all arguments (e.g. a single all-text range), the result is
/// `#VALUE!` (Excel's ruling — there is nothing to AND).
pub fn and_fn(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    fold_bools(args, true, |acc, b| acc && b)
}

/// `OR(logical1, [logical2], …)` — TRUE iff at least one logical value is TRUE
/// (ECMA-376 §18.17.7). Same range-aware scan and rulings as [`and_fn`]: text
/// and blanks in ranges ignored, error cells propagate, no-logical-value →
/// `#VALUE!`.
pub fn or_fn(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    fold_bools(args, false, |acc, b| acc || b)
}

/// `NOT(logical)` — boolean negation (ECMA-376 §18.17.7). The single argument
/// is coerced via [`coerce::to_bool`]; an error propagates, non-boolean text
/// is `#VALUE!`.
pub fn not_fn(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match scalar_to_bool(&args[0]) {
        Ok(b) => CellValue::Bool(!b),
        Err(e) => CellValue::Error(e),
    }
}

/// `TRUE()` — the boolean constant TRUE (ECMA-376 §18.17.7). No arguments.
pub fn true_fn(_args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(true)
}

/// `FALSE()` — the boolean constant FALSE (ECMA-376 §18.17.7). No arguments.
pub fn false_fn(_args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(false)
}

/// `IFERROR(value, value_if_error)` — return `value` unless it **is** an
/// error, in which case return `value_if_error` (ECMA-376 §18.17.7). Only the
/// first argument's *error-ness* is tested; any non-error value (including a
/// blank cell, text, or `#N/A`… wait: `#N/A` IS an error, so it IS caught) is
/// returned verbatim. Catches all eight OOXML error codes.
///
/// T0 eager note (module docs): the fallback is already evaluated even when
/// the primary is fine; an error *in the fallback* would have surfaced before
/// this kernel, so a clean primary still returns cleanly.
pub fn iferror(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let primary = arg_value(&args[0]);
    if matches!(primary, CellValue::Error(_)) {
        arg_value(&args[1])
    } else {
        primary
    }
}

// ---- shared helpers (private to the family) ----

/// Materialize one argument to a single [`CellValue`] for the value-selecting
/// kernels (`IF`, `IFERROR`). A scalar is returned as-is; a range collapses to
/// its top-left cell (the degenerate implicit-intersection a value context
/// applies when handed a range — the convention has not pre-intersected for
/// these non-`range_aware` rows, so we take the anchor cell).
fn arg_value(arg: &Arg) -> CellValue {
    match arg {
        Arg::Scalar(v) => v.clone(),
        Arg::Range(rv) => rv.get(0, 0),
    }
}

/// Coerce a scalar-context argument to a boolean for `IF`/`NOT`. A range in a
/// scalar boolean context collapses to its top-left cell, matching the value
/// helper above (a full implicit-intersection over the formula's own row/col
/// is a `sheet-calc` concern above this layer).
fn scalar_to_bool(arg: &Arg) -> Result<bool, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_bool(v),
        Arg::Range(rv) => coerce::to_bool(&rv.get(0, 0)),
    }
}

/// Fold the logical values across all `AND`/`OR` arguments, honoring the
/// range-aware rulings: scalars coerce strictly (text → `#VALUE!`), range
/// cells skip text/blank and propagate errors, and an empty logical population
/// is `#VALUE!`. `init`/`op` encode the family (AND: `true`/`&&`, OR:
/// `false`/`||`).
fn fold_bools(args: &[Arg], init: bool, op: fn(bool, bool) -> bool) -> CellValue {
    let mut acc = init;
    let mut saw_logical = false;
    for arg in args {
        match arg {
            Arg::Scalar(v) => match coerce::to_bool(v) {
                Ok(b) => {
                    acc = op(acc, b);
                    saw_logical = true;
                }
                Err(e) => return CellValue::Error(e),
            },
            Arg::Range(rv) => match fold_range(rv, &mut acc, op) {
                Ok(found) => saw_logical |= found,
                Err(e) => return CellValue::Error(e),
            },
        }
    }
    if saw_logical {
        CellValue::Bool(acc)
    } else {
        // Nothing to AND/OR over — Excel rules this #VALUE!.
        CellValue::Error(CellError::Value)
    }
}

/// Scan one range for `AND`/`OR`: numbers and bools contribute (via the
/// number-zero / bool ruling), **text and blank cells are skipped**, and an
/// error cell short-circuits the whole call. Returns whether any logical value
/// was seen so the caller can detect the empty-population `#VALUE!`.
fn fold_range(
    rv: &RangeView,
    acc: &mut bool,
    op: fn(bool, bool) -> bool,
) -> Result<bool, CellError> {
    let mut found = false;
    for cell in rv.iter() {
        match cell {
            // Errors inside a range propagate (Excel: an error cell poisons
            // the aggregation).
            CellValue::Error(e) => return Err(e),
            // Text and blanks are not logical values in a range — skip them.
            CellValue::Text(_) | CellValue::Empty => {}
            // Numbers and bools coerce to a boolean (number != 0).
            CellValue::Number(_) | CellValue::Bool(_) => {
                // `to_bool` cannot fail for Number/Bool, but stay total.
                if let Ok(b) = coerce::to_bool(&cell) {
                    *acc = op(*acc, b);
                    found = true;
                }
            }
        }
    }
    Ok(found)
}
