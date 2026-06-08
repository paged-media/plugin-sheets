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

//! M1 information (M1 additions) family kernels (spec §7, §11 T1): `N`,
//! `TYPE`, `ERROR.TYPE`, `SHEET`, `SHEETS`, `ISEVEN`, `ISODD` (ECMA-376
//! §18.17.7 + Microsoft public function docs). Each kernel is the frozen pure
//! signature `fn(&[Arg], &EvalCtx) -> CellValue`; the generated
//! [`crate::dispatch`] arity-checks before calling (`N`/`TYPE`/`ERROR.TYPE`/
//! `ISEVEN`/`ISODD` exactly 1 arg; `SHEET`/`SHEETS` 0–1), so a kernel only
//! ever sees a well-formed argument list.
//!
//! # The split with `crate::families::info`
//!
//! The classic `IS*` predicates *inspect* their argument and never
//! propagate an argument error (`ISERROR(#DIV/0!)` is `TRUE`). The M1
//! additions are **not** all predicates and do **not** share that rule:
//!   - `N`/`ISEVEN`/`ISODD` are *coercions* — they route through
//!     [`crate::coerce::to_number`] and therefore **propagate** an error
//!     argument (`N(#DIV/0!)` is `#DIV/0!`), like a math kernel.
//!   - `TYPE`/`ERROR.TYPE` *classify* — they **inspect** the variant (an
//!     error argument is the subject, never returned): `TYPE(#N/A)` is `16`,
//!     `ERROR.TYPE(#NULL!)` is `1`.
//!   - `SHEET`/`SHEETS` are reference functions resolved from the argument's
//!     [`crate::arg::RangeView::origin`] (sheet id) or the current cell.
//!
//! # ISFORMULA — left PLANNED (T1 limitation; see the registry row)
//!
//! `ISFORMULA(reference)` must know whether the *referenced cell* stores a
//! formula. A pure `fn(&[Arg], &EvalCtx)` kernel receives only the cell's
//! **evaluated value** (an [`Arg::Range`]/[`Arg::Scalar`]) — neither [`Arg`]
//! nor [`EvalCtx`] carries a "this cell is a formula" flag, and there is no
//! cell→formula hook on the context. Implementing it from the value alone
//! would be a fabrication (a `2` is the same `Number(2.0)` whether typed or
//! computed). The row stays `planned` until the calling convention grows a
//! reference-introspection seam (an additive `EvalCtx` hook or an `Arg`
//! origin-formula flag) — flagged in the track report, not faked.

use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;

/// Resolve a single info argument to the [`CellValue`] under inspection. A
/// scalar is taken as-is; a range collapses to its relative-`(0,0)` cell (the
/// implicit-intersection fallback — out-of-bounds yields `Empty`). Mirrors
/// `info::subject`.
fn subject(arg: &Arg) -> CellValue {
    match arg {
        Arg::Scalar(v) => v.clone(),
        Arg::Range(view) => view.get(0, 0),
    }
}

/// `N(value)` — coerce a value to a number (ECMA-376 §18.17.7). The narrow
/// coercion `N` performs:
/// - a number → itself;
/// - `TRUE` → `1`, `FALSE` → `0`;
/// - a **date** is already a serial number, so it returns that serial
///   unchanged (no special date handling — dates are numbers, §5);
/// - **text → `0`** (Excel's ruling: `N` does NOT parse numeric text;
///   `N("7")` is `0`, not `7` — this is the defining difference from the
///   general number coercion, which *would* parse it). A blank cell is `0`.
/// - an **error propagates** (`N(#DIV/0!)` is `#DIV/0!`).
pub fn n(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match subject(&args[0]) {
        CellValue::Number(x) => CellValue::Number(x),
        CellValue::Bool(b) => CellValue::Number(if b { 1.0 } else { 0.0 }),
        CellValue::Empty => CellValue::Number(0.0),
        // Excel ruling: N does not parse numeric text — any text is 0.
        CellValue::Text(_) => CellValue::Number(0.0),
        CellValue::Error(e) => CellValue::Error(e),
    }
}

/// `TYPE(value)` — classify a value by type code (ECMA-376 §18.17.7):
/// `1` number, `2` text, `4` logical, `16` error, `64` array. **Inspects**
/// the variant — an error argument is the subject, classified as `16`, never
/// returned. A blank cell is treated as a number (`1`), matching Excel
/// (`TYPE(A1)` over an empty `A1` is `1`). The `64` (array) code cannot arise
/// here: the scalar door collapses a range to a single cell, so an inline
/// array argument is a T1-spill concern above this kernel; the code is
/// documented for completeness but unreachable through this path.
pub fn type_fn(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let code = match subject(&args[0]) {
        // A blank cell classifies as a number in Excel's TYPE.
        CellValue::Number(_) | CellValue::Empty => 1.0,
        CellValue::Text(_) => 2.0,
        CellValue::Bool(_) => 4.0,
        CellValue::Error(_) => 16.0,
    };
    CellValue::Number(code)
}

/// `ERROR.TYPE(error_val)` — map an error value to its 1-based index
/// (ECMA-376 §18.17.7): `1` `#NULL!`, `2` `#DIV/0!`, `3` `#VALUE!`,
/// `4` `#REF!`, `5` `#NAME?`, `6` `#NUM!`, `7` `#N/A`. A **non-error**
/// argument yields `#N/A` (Excel's ruling — there is no error to index).
/// Like `TYPE`, this **inspects**: the error is the subject, never returned.
///
/// `#SPILL!` (an Excel-365 dynamic-array error) has no classic 1..7 index in
/// the ECMA-376 table. We map it to `#N/A` rather than invent a code — the
/// conservative, documented ruling (registry row); revisit if the spill track
/// settles on Excel's modern `ERROR.TYPE(#SPILL!) = 9`.
pub fn error_type(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match subject(&args[0]) {
        CellValue::Error(e) => match e {
            CellError::Null => CellValue::Number(1.0),
            CellError::Div0 => CellValue::Number(2.0),
            CellError::Value => CellValue::Number(3.0),
            CellError::Ref => CellValue::Number(4.0),
            CellError::Name => CellValue::Number(5.0),
            CellError::Num => CellValue::Number(6.0),
            CellError::Na => CellValue::Number(7.0),
            // No classic index for the modern spill error — ruled #N/A.
            CellError::Spill => CellValue::Error(CellError::Na),
        },
        // A non-error subject has no error code.
        _ => CellValue::Error(CellError::Na),
    }
}

/// `SHEET([value])` — the 1-based sheet index of a reference (Microsoft
/// SHEET, public docs). With no argument it returns the sheet of the cell
/// being evaluated ([`EvalCtx::current`] `+ 1`). With a **range** argument it
/// returns the sheet of the reference's top-left
/// ([`crate::arg::RangeView::origin`] `+ 1`).
///
/// # T1 limitation
///
/// A **scalar** argument (a value, not a recoverable reference) cannot name a
/// sheet — the calling convention has erased the reference's address by the
/// time it reaches this kernel. Excel's `SHEET("Name")` (a sheet *name* text
/// argument) is therefore **not** supported at T1: a scalar argument falls
/// back to the current cell's sheet (the conservative total answer). Name
/// resolution needs a workbook→sheet-id lookup the pure kernel lacks (an
/// additive `EvalCtx` hook); flagged in the track report.
pub fn sheet(args: &[Arg], ctx: &EvalCtx) -> CellValue {
    let sheet_id = match args.first() {
        Some(Arg::Range(rv)) => rv.origin().sheet,
        // No arg, or a scalar (no recoverable reference): the current cell.
        _ => ctx.current.sheet,
    };
    CellValue::Number((sheet_id as u32 + 1) as f64)
}

/// `SHEETS([reference])` — the number of sheets a reference spans (Microsoft
/// SHEETS, public docs).
///
/// # T1 limitation — always `1`
///
/// A 3-D reference (`Sheet1:Sheet3!A1`) spans multiple sheets, but the
/// calling convention has no 3-D-range [`Arg`] shape: a range always lands as
/// a single contiguous [`crate::arg::RangeView`] on one sheet. With **no
/// argument**, `SHEETS()` is the workbook's total sheet count, which the pure
/// [`EvalCtx`] does **not** carry. Both cases therefore return `1` at T1: a
/// single contiguous reference, and (lacking a workbook sheet-count field) a
/// no-argument call. The honest fix is an additive `EvalCtx::sheet_count`
/// field plus a 3-D `Arg` variant — flagged in the track report, not faked.
pub fn sheets(_args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    // T1: one contiguous reference spans one sheet; no workbook sheet count.
    CellValue::Number(1.0)
}

/// `ISEVEN(number)` — `TRUE` iff the **truncated** number is even (ECMA-376
/// §18.17.7). The argument is coerced through [`coerce::to_number`] (so
/// numeric text like `"4"` works, `TRUE`→`1`, a blank→`0`), then truncated
/// toward zero; parity is tested on the integer part. Non-numeric text is
/// `#VALUE!`, and an error argument propagates (coercion contract).
pub fn iseven(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match parity_is_odd(&subject(&args[0])) {
        Ok(is_odd) => CellValue::Bool(!is_odd),
        Err(e) => CellValue::Error(e),
    }
}

/// `ISODD(number)` — `TRUE` iff the **truncated** number is odd (ECMA-376
/// §18.17.7). The exact complement of [`iseven`]: same coercion, truncation,
/// and error rules; tests for an odd integer part.
pub fn isodd(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match parity_is_odd(&subject(&args[0])) {
        Ok(is_odd) => CellValue::Bool(is_odd),
        Err(e) => CellValue::Error(e),
    }
}

/// Shared parity core for `ISEVEN`/`ISODD`: coerce to a number, truncate
/// toward zero, and report whether the integer part is odd. The truncation
/// uses `f64::trunc` (drop the fraction, keep the sign) — `ISODD(3.9)` is
/// `TRUE`, `ISEVEN(-2.5)` is `TRUE`. Parity is read off the low bit of the
/// truncated integer (computed in `i64`-safe space; magnitudes beyond `i64`
/// are even by construction since they are exact even doubles).
fn parity_is_odd(v: &CellValue) -> Result<bool, CellError> {
    let x = coerce::to_number(v)?.trunc();
    // A truncated f64 with |x| ≥ 2^53 has no odd representable values (it is an
    // exact even integer), so the rem-2 test on the f64 is exact and total.
    Ok((x % 2.0).abs() == 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_str::CompactString;
    use sheet_core::{CellRef, DateSystem};

    fn cr_at(sheet: u16, row: u32, col: u32) -> CellRef {
        CellRef {
            sheet,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn ctx() -> EvalCtx {
        EvalCtx::new(DateSystem::Date1900, cr_at(0, 0, 0), 45000.5, 42)
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
    fn n_coercions() {
        assert_eq!(n(&[s(num(7.0))], &ctx()), num(7.0));
        assert_eq!(n(&[s(b(true))], &ctx()), num(1.0));
        assert_eq!(n(&[s(b(false))], &ctx()), num(0.0));
        assert_eq!(n(&[s(CellValue::Empty)], &ctx()), num(0.0));
        // Excel ruling: text → 0, even numeric text (N does not parse it).
        assert_eq!(n(&[s(txt("7"))], &ctx()), num(0.0));
        assert_eq!(n(&[s(txt("hello"))], &ctx()), num(0.0));
    }

    #[test]
    fn n_propagates_error() {
        assert_eq!(
            n(&[s(CellValue::Error(CellError::Div0))], &ctx()),
            CellValue::Error(CellError::Div0)
        );
    }

    #[test]
    fn type_codes() {
        assert_eq!(type_fn(&[s(num(1.0))], &ctx()), num(1.0));
        assert_eq!(type_fn(&[s(txt("x"))], &ctx()), num(2.0));
        assert_eq!(type_fn(&[s(b(true))], &ctx()), num(4.0));
        // Inspects an error subject → 16 (never propagates).
        assert_eq!(
            type_fn(&[s(CellValue::Error(CellError::Na))], &ctx()),
            num(16.0)
        );
        // A blank cell classifies as a number.
        assert_eq!(type_fn(&[s(CellValue::Empty)], &ctx()), num(1.0));
    }

    #[test]
    fn error_type_indices() {
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Null))], &ctx()),
            num(1.0)
        );
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Div0))], &ctx()),
            num(2.0)
        );
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Value))], &ctx()),
            num(3.0)
        );
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Ref))], &ctx()),
            num(4.0)
        );
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Name))], &ctx()),
            num(5.0)
        );
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Num))], &ctx()),
            num(6.0)
        );
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Na))], &ctx()),
            num(7.0)
        );
    }

    #[test]
    fn error_type_non_error_is_na() {
        assert_eq!(
            error_type(&[s(num(5.0))], &ctx()),
            CellValue::Error(CellError::Na)
        );
        assert_eq!(
            error_type(&[s(txt("x"))], &ctx()),
            CellValue::Error(CellError::Na)
        );
        // The ruled #SPILL! → #N/A (no classic 1..7 index).
        assert_eq!(
            error_type(&[s(CellValue::Error(CellError::Spill))], &ctx()),
            CellValue::Error(CellError::Na)
        );
    }

    #[test]
    fn sheet_no_arg_is_current_plus_one() {
        // current.sheet == 0 → SHEET() == 1 (1-based).
        let c = EvalCtx::new(DateSystem::Date1900, cr_at(2, 0, 0), 0.0, 1);
        assert_eq!(sheet(&[], &c), num(3.0));
    }

    #[test]
    fn sheet_range_arg_uses_origin_sheet() {
        use crate::arg::RangeView;
        let cells = [num(1.0)];
        // Origin on sheet id 4 → SHEET(ref) == 5.
        let rv = RangeView::from_slice(cr_at(4, 0, 0), 1, 1, &cells);
        assert_eq!(sheet(&[Arg::Range(rv)], &ctx()), num(5.0));
    }

    #[test]
    fn sheet_scalar_arg_falls_back_to_current() {
        // T1: a scalar arg has no recoverable reference → current cell sheet.
        let c = EvalCtx::new(DateSystem::Date1900, cr_at(1, 0, 0), 0.0, 1);
        assert_eq!(sheet(&[s(num(99.0))], &c), num(2.0));
    }

    #[test]
    fn sheets_is_one_t1() {
        // T1 limitation: always 1 (no 3-D range, no workbook sheet count).
        assert_eq!(sheets(&[], &ctx()), num(1.0));
        use crate::arg::RangeView;
        let cells = [num(1.0), num(2.0)];
        let rv = RangeView::from_slice(cr_at(0, 0, 0), 1, 2, &cells);
        assert_eq!(sheets(&[Arg::Range(rv)], &ctx()), num(1.0));
    }

    #[test]
    fn iseven_isodd_basic_and_truncation() {
        assert_eq!(iseven(&[s(num(4.0))], &ctx()), b(true));
        assert_eq!(iseven(&[s(num(3.0))], &ctx()), b(false));
        assert_eq!(isodd(&[s(num(3.0))], &ctx()), b(true));
        assert_eq!(isodd(&[s(num(4.0))], &ctx()), b(false));
        // Truncation toward zero: 3.9 → 3 (odd), -2.5 → -2 (even).
        assert_eq!(isodd(&[s(num(3.9))], &ctx()), b(true));
        assert_eq!(iseven(&[s(num(-2.5))], &ctx()), b(true));
        // Zero is even.
        assert_eq!(iseven(&[s(num(0.0))], &ctx()), b(true));
    }

    #[test]
    fn iseven_isodd_coercion_and_errors() {
        // Numeric text coerces (the general to_number, unlike N).
        assert_eq!(iseven(&[s(txt("8"))], &ctx()), b(true));
        assert_eq!(isodd(&[s(b(true))], &ctx()), b(true)); // TRUE → 1 → odd
                                                           // Non-numeric text → #VALUE!.
        assert_eq!(
            iseven(&[s(txt("x"))], &ctx()),
            CellValue::Error(CellError::Value)
        );
        // An error argument propagates.
        assert_eq!(
            isodd(&[s(CellValue::Error(CellError::Ref))], &ctx()),
            CellValue::Error(CellError::Ref)
        );
    }

    #[test]
    fn range_arg_uses_top_left() {
        use crate::arg::RangeView;
        let cells = [
            CellValue::Text(CompactString::new("a")),
            CellValue::Number(2.0),
        ];
        let rv = RangeView::from_slice(cr_at(0, 0, 0), 1, 2, &cells);
        // Top-left is text → TYPE 2.
        assert_eq!(type_fn(&[Arg::Range(rv)], &ctx()), num(2.0));
    }
}
