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

//! M1 logical (M1 additions) family kernels (spec §7, §11 T1): `IFS`,
//! `SWITCH`, `XOR`, `IFNA` (Microsoft public function docs; ECMA-376
//! §18.17.7 for `XOR`). Each kernel is the frozen pure signature
//! `fn(&[Arg], &EvalCtx) -> CellValue`; arity is enforced by the generated
//! [`crate::dispatch`] (`IFS` min 2, `SWITCH` min 3, `XOR` min 1, `IFNA`
//! min/max 2) before the kernel runs, so a kernel may assume its argument
//! count is in `[min, max]`. All type conversion and error propagation flow
//! through [`crate::coerce`] so the rulings are stated once (repo
//! constitution, §"Excel-compat is a ruling").
//!
//! # T1 eager note (mirrors `crate::families::logical`)
//!
//! Like `IF`/`AND`/`OR`/`IFERROR`, these selectors are EAGER at T1: the
//! calling convention hands the kernel its arguments **already evaluated**.
//! `IFS`/`SWITCH`/`IFNA` therefore compute every candidate value/result even
//! though Excel only evaluates the taken one. This is semantically correct
//! for **value** results (the kernel just selects among already-computed
//! values), but an error in a *discarded* branch surfaces up the stack
//! before this kernel runs (Excel's lazy short-circuit would have masked it).
//! The lazy special-form path is a `sheet-calc` Phase-2 concern; these
//! kernels remain the value-selection fallback for the fully-evaluated case.
//! `IFS`/`SWITCH` are listed alongside `IF`/`CHOOSE` in that logical module's
//! lazy-dispatch note — **sheet-calc agent: see it before wiring laziness.**

use sheet_core::{CellError, CellValue};

use crate::arg::{Arg, RangeView};
use crate::coerce;
use crate::ctx::EvalCtx;

/// `IFS(condition1, value1, [condition2, value2], …)` — return the value
/// paired with the first TRUE condition (Microsoft IFS, public docs). The
/// conditions/values arrive as **alternating** arguments: even index = a
/// condition, the following odd index = its value. The dispatch guarantees
/// `args.len() >= 2`; this kernel additionally rules:
///
/// - **odd argument count → `#N/A`** (a dangling condition with no value).
///   Excel surfaces this "You've entered too few arguments" shape as `#N/A`
///   at evaluation; we adopt the `#N/A` ruling (registry row).
/// - the first condition that coerces to TRUE returns its paired value
///   verbatim; later pairs are not consulted (value-selection only — see the
///   T1 eager note for the cost/error caveat).
/// - **no condition is TRUE → `#N/A`** (Excel's documented "no match" result;
///   unlike `IF`, `IFS` has no default slot).
/// - an **error in a condition** propagates as that error (a condition we
///   cannot evaluate poisons the whole `IFS`, first-condition-error-wins;
///   later pairs are not reached). Non-boolean text in a condition is
///   `#VALUE!` (via [`coerce::to_bool`]).
pub fn ifs(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    // A dangling final condition (odd arity) has no value to return → #N/A.
    if !args.len().is_multiple_of(2) {
        return CellValue::Error(CellError::Na);
    }
    let mut i = 0;
    while i + 1 < args.len() {
        match scalar_to_bool(&args[i]) {
            Ok(true) => return arg_value(&args[i + 1]),
            Ok(false) => {}
            Err(e) => return CellValue::Error(e),
        }
        i += 2;
    }
    // No condition matched — Excel's no-match result for IFS.
    CellValue::Error(CellError::Na)
}

/// `SWITCH(expression, value1, result1, [value2, result2], …, [default])` —
/// compare `expression` against each `valueN`, returning the matching
/// `resultN`; an unmatched lone trailing argument is the default (Microsoft
/// SWITCH, public docs). The dispatch guarantees `args.len() >= 3`.
///
/// Rulings:
/// - **matching is Excel `=` equality** (the cross-type total order
///   [`coerce::compare`], with the case-insensitive text-equality refinement
///   below): `5` matches the number `5`, never the text `"5"`; text matches
///   **case-insensitively** (`"HELLO"` matches `"hello"`), mirroring the `=`
///   operator's `apply_text_op` fold in [`crate::criteria`]. The first matching
///   `valueN` wins; its `resultN` is returned verbatim.
/// - an **error in the expression** propagates (we cannot compare against an
///   error subject; first-error-wins). An error in a *candidate value* that
///   the comparison reaches is treated as a non-match value (it simply will
///   not equal a non-error expression) — it is data, not a short-circuit.
/// - after the `(value, result)` pairs, a **lone trailing argument is the
///   default**, returned when nothing matched. With an **even** trailing
///   shape (no default) and no match, the result is `#N/A` (Excel's ruling).
pub fn switch(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let expr = arg_value(&args[0]);
    // An error expression cannot be compared — propagate it.
    if let CellValue::Error(e) = expr {
        return CellValue::Error(e);
    }

    // After arg[0], arguments come in (value, result) pairs; a final unpaired
    // argument is the default.
    let rest = &args[1..];
    let mut i = 0;
    while i + 1 < rest.len() {
        let candidate = arg_value(&rest[i]);
        if values_equal(&expr, &candidate) {
            return arg_value(&rest[i + 1]);
        }
        i += 2;
    }
    // One argument left over after the pairs → it is the default.
    if i < rest.len() {
        return arg_value(&rest[i]);
    }
    // No match and no default supplied.
    CellValue::Error(CellError::Na)
}

/// `XOR(logical1, [logical2], …)` — exclusive OR: TRUE iff an **odd** number
/// of the logical values are TRUE (ECMA-376 §18.17.7). Range-aware, mirroring
/// `AND`/`OR`: scalars coerce strictly via [`coerce::to_bool`] (non-boolean
/// text → `#VALUE!`); inside a range **text and blank cells are ignored**,
/// numbers/bools count (number ≠ 0 is TRUE), and an **error cell
/// propagates**. With no logical value found anywhere the result is `#VALUE!`
/// (Excel's ruling — there is nothing to XOR), matching the `AND`/`OR`
/// empty-population rule.
pub fn xor(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let mut true_count: u64 = 0;
    let mut saw_logical = false;
    for arg in args {
        match arg {
            Arg::Scalar(v) => match coerce::to_bool(v) {
                Ok(b) => {
                    if b {
                        true_count += 1;
                    }
                    saw_logical = true;
                }
                Err(e) => return CellValue::Error(e),
            },
            Arg::Range(rv) => match count_true_in_range(rv) {
                Ok((trues, found)) => {
                    true_count += trues;
                    saw_logical |= found;
                }
                Err(e) => return CellValue::Error(e),
            },
        }
    }
    if saw_logical {
        // Parity: odd number of TRUEs → TRUE.
        CellValue::Bool(true_count % 2 == 1)
    } else {
        CellValue::Error(CellError::Value)
    }
}

/// `IFNA(value, value_if_na)` — return `value` unless it is the `#N/A` error,
/// in which case return `value_if_na` (Microsoft IFNA, public docs). The
/// narrow sibling of `IFERROR`: it catches **only** `#N/A`, so every other
/// error (`#DIV/0!`, `#VALUE!`, …) passes straight through unchanged. Any
/// non-error value is returned verbatim.
///
/// T1 eager note (module docs): the fallback arrives already evaluated even
/// when `value` is fine; a clean `value` still returns cleanly.
pub fn ifna(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let primary = arg_value(&args[0]);
    if matches!(primary, CellValue::Error(CellError::Na)) {
        arg_value(&args[1])
    } else {
        primary
    }
}

// ---- shared helpers (private to the family) ----

/// `SWITCH`'s match test — Excel `=` equality, NOT the raw [`coerce::compare`]
/// total order. [`coerce::compare`] is an *ordering* primitive: to stay total
/// it tie-breaks case-folded-equal text by raw bytes, so `compare("HELLO",
/// "hello")` is `Less`, not `Equal`. Excel's `=` (and therefore `SWITCH`'s
/// match) is **case-insensitive** for text — the codebase's authoritative
/// `=`-equality lives in `criteria::apply_text_op` (`Op::Eq` is the fold with
/// no tie-break). This helper reconciles the two: two texts are equal iff their
/// ASCII-lowercased byte streams are equal; every other pair (cross-type,
/// number, bool, blank) defers to `compare == Equal`, which is already the
/// equality those types want. (`compare`/`criteria` are frozen, so the
/// reconciliation lives here in the kernel.)
fn values_equal(a: &CellValue, b: &CellValue) -> bool {
    if let (CellValue::Text(x), CellValue::Text(y)) = (a, b) {
        return x
            .bytes()
            .map(|c| c.to_ascii_lowercase())
            .eq(y.bytes().map(|c| c.to_ascii_lowercase()));
    }
    coerce::compare(a, b) == std::cmp::Ordering::Equal
}

/// Materialize one argument to a single [`CellValue`] for the value-selecting
/// kernels. A scalar is returned as-is; a range collapses to its top-left
/// cell (the degenerate implicit-intersection a value context applies — the
/// convention has not pre-intersected for these non-`range_aware` rows, so we
/// take the anchor cell). Mirrors `logical::arg_value`.
fn arg_value(arg: &Arg) -> CellValue {
    match arg {
        Arg::Scalar(v) => v.clone(),
        Arg::Range(rv) => rv.get(0, 0),
    }
}

/// Coerce a scalar-context argument to a boolean for the `IFS` conditions. A
/// range collapses to its top-left cell, matching the value helper above
/// (full implicit-intersection is a `sheet-calc` concern). Mirrors
/// `logical::scalar_to_bool`.
fn scalar_to_bool(arg: &Arg) -> Result<bool, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_bool(v),
        Arg::Range(rv) => coerce::to_bool(&rv.get(0, 0)),
    }
}

/// Scan one range for `XOR`, returning `(count_of_TRUE, saw_any_logical)`.
/// Numbers and bools contribute (number ≠ 0 → TRUE), **text and blank cells
/// are skipped**, and an error cell short-circuits the whole call (Excel: an
/// error cell poisons the aggregation). Mirrors `logical::fold_range`.
fn count_true_in_range(rv: &RangeView) -> Result<(u64, bool), CellError> {
    let mut trues: u64 = 0;
    let mut found = false;
    for cell in rv.iter() {
        match cell {
            CellValue::Error(e) => return Err(e),
            CellValue::Text(_) | CellValue::Empty => {}
            CellValue::Number(_) | CellValue::Bool(_) => {
                // `to_bool` cannot fail for Number/Bool, but stay total.
                if let Ok(b) = coerce::to_bool(&cell) {
                    if b {
                        trues += 1;
                    }
                    found = true;
                }
            }
        }
    }
    Ok((trues, found))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::{CellRef, DateSystem};

    fn cr() -> CellRef {
        CellRef {
            sheet: 0,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        }
    }

    fn ctx() -> EvalCtx {
        EvalCtx::new(DateSystem::Date1900, cr(), 45000.5, 42)
    }

    fn num(n: f64) -> CellValue {
        CellValue::Number(n)
    }
    fn txt(s: &str) -> CellValue {
        CellValue::from(s)
    }
    fn b(v: bool) -> CellValue {
        CellValue::Bool(v)
    }
    fn s(v: CellValue) -> Arg<'static> {
        Arg::Scalar(v)
    }

    #[test]
    fn ifs_first_true_wins() {
        // Second condition is the first TRUE → its value.
        assert_eq!(
            ifs(&[s(b(false)), s(txt("a")), s(b(true)), s(txt("b"))], &ctx()),
            txt("b")
        );
    }

    #[test]
    fn ifs_no_match_is_na() {
        assert_eq!(
            ifs(
                &[s(b(false)), s(num(1.0)), s(num(0.0)), s(num(2.0))],
                &ctx()
            ),
            CellValue::Error(CellError::Na)
        );
    }

    #[test]
    fn ifs_odd_arity_is_na() {
        // Dangling final condition with no value → #N/A.
        assert_eq!(
            ifs(&[s(b(false)), s(num(1.0)), s(b(true))], &ctx()),
            CellValue::Error(CellError::Na)
        );
    }

    #[test]
    fn ifs_error_condition_propagates() {
        assert_eq!(
            ifs(&[s(CellValue::Error(CellError::Div0)), s(num(1.0))], &ctx()),
            CellValue::Error(CellError::Div0)
        );
        // Non-boolean text condition → #VALUE!.
        assert_eq!(
            ifs(&[s(txt("maybe")), s(num(1.0))], &ctx()),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn switch_matches_and_defaults() {
        // 2 matches the second value → "two".
        assert_eq!(
            switch(
                &[
                    s(num(2.0)),
                    s(num(1.0)),
                    s(txt("one")),
                    s(num(2.0)),
                    s(txt("two")),
                ],
                &ctx()
            ),
            txt("two")
        );
        // No match, lone trailing default returned.
        assert_eq!(
            switch(
                &[s(num(9.0)), s(num(1.0)), s(txt("one")), s(txt("default"))],
                &ctx()
            ),
            txt("default")
        );
    }

    #[test]
    fn switch_no_match_no_default_is_na() {
        assert_eq!(
            switch(&[s(num(9.0)), s(num(1.0)), s(txt("one"))], &ctx()),
            CellValue::Error(CellError::Na)
        );
    }

    #[test]
    fn switch_cross_type_and_case_fold() {
        // Number expression never matches text "5".
        assert_eq!(
            switch(
                &[s(num(5.0)), s(txt("5")), s(txt("text")), s(txt("def"))],
                &ctx()
            ),
            txt("def")
        );
        // Text matches case-insensitively (the compare fold).
        assert_eq!(
            switch(&[s(txt("HELLO")), s(txt("hello")), s(txt("hit"))], &ctx()),
            txt("hit")
        );
    }

    #[test]
    fn switch_error_expression_propagates() {
        assert_eq!(
            switch(
                &[
                    s(CellValue::Error(CellError::Ref)),
                    s(num(1.0)),
                    s(txt("one")),
                ],
                &ctx()
            ),
            CellValue::Error(CellError::Ref)
        );
    }

    #[test]
    fn xor_parity() {
        // One TRUE → TRUE; two TRUEs → FALSE; three → TRUE.
        assert_eq!(xor(&[s(b(true)), s(b(false))], &ctx()), b(true));
        assert_eq!(xor(&[s(b(true)), s(b(true))], &ctx()), b(false));
        assert_eq!(xor(&[s(b(true)), s(b(true)), s(b(true))], &ctx()), b(true));
    }

    #[test]
    fn xor_coercion() {
        // Numbers coerce (non-zero TRUE, zero FALSE).
        assert_eq!(xor(&[s(num(1.0)), s(num(0.0))], &ctx()), b(true));
        assert_eq!(xor(&[s(num(2.0)), s(num(3.0))], &ctx()), b(false));
        // Non-boolean scalar text → #VALUE!.
        assert_eq!(
            xor(&[s(b(true)), s(txt("nope"))], &ctx()),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn xor_range_and_error() {
        // Range with two TRUEs (mixed bool/number); text skipped → FALSE.
        let cells = [b(true), num(1.0), txt("skip"), CellValue::Empty];
        let rv = RangeView::from_slice(cr(), 1, 4, &cells);
        assert_eq!(xor(&[Arg::Range(rv)], &ctx()), b(false));

        // An error inside the range propagates.
        let cells2 = [b(true), CellValue::Error(CellError::Na)];
        let rv2 = RangeView::from_slice(cr(), 1, 2, &cells2);
        assert_eq!(
            xor(&[Arg::Range(rv2)], &ctx()),
            CellValue::Error(CellError::Na)
        );
    }

    #[test]
    fn xor_no_logical_value_is_value_error() {
        let cells = [txt("a"), CellValue::Empty];
        let rv = RangeView::from_slice(cr(), 1, 2, &cells);
        assert_eq!(
            xor(&[Arg::Range(rv)], &ctx()),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn ifna_catches_only_na() {
        // #N/A primary → the fallback.
        assert_eq!(
            ifna(&[s(CellValue::Error(CellError::Na)), s(txt("ok"))], &ctx()),
            txt("ok")
        );
        // A different error passes straight through.
        assert_eq!(
            ifna(
                &[s(CellValue::Error(CellError::Div0)), s(txt("ok"))],
                &ctx()
            ),
            CellValue::Error(CellError::Div0)
        );
        // A clean value passes through.
        assert_eq!(ifna(&[s(num(42.0)), s(txt("ok"))], &ctx()), num(42.0));
    }
}
