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

//! The tree-walk evaluator (spec §6.2). Given a formula's [`Expr`] and a
//! [`SheetModel`] whose dependency values are already fresh (the `topo` order
//! guarantees this), [`eval_expr`] computes the cell's [`CellValue`].
//!
//! ## Operator semantics (through `sheet_fn::coerce`)
//!
//! - `Add/Sub/Mul/Div/Pow` operate on [`coerce::to_number`]. `Div` by `0` →
//!   `#DIV/0!`; `0^0` → `#NUM!`; a negative base to a fractional exponent →
//!   `#NUM!`.
//! - `Concat` (`&`) operates on [`coerce::to_text`].
//! - Comparisons (`= <> < <= > >=`) go through [`coerce::compare`] (the
//!   Excel cross-type total order with case-insensitive text equality).
//! - Unary `Neg`/`Plus` operate on [`coerce::to_number`]; `Percent` divides
//!   by 100.
//! - **Error propagation:** any errored operand wins, left operand first.
//! - An empty cell reference in arithmetic propagates the `Empty` VALUE to the
//!   op, and the op coerces it (`to_number(Empty) == 0`).
//! - `Range`/`Array`/`Union`/`Isect` in a SCALAR position → `#VALUE!` (T0
//!   parse-only ruling — these are only meaningful as function arguments).
//!
//! ## Function calls (eager evaluation)
//!
//! Arguments are evaluated BEFORE dispatch. `Expr::Range` (and a bare
//! `Expr::Ref` for a `ref_args` function) materializes to an
//! [`sheet_fn::Arg::Range`] via [`crate::argview`]; every other expression
//! evaluates to a scalar [`sheet_fn::Arg::Scalar`]. This is value-correct for
//! the T0 logical family (`IF`/`AND`/`OR`/`IFERROR` select among
//! already-computed values; the condition's own error still propagates in
//! `if_fn`, and `IFERROR` receives the error AS a value and catches it). True
//! short-circuit (not evaluating a discarded branch) is a `sheet-calc` Phase-2
//! special form — see the `sheet_fn::families::logical` module docs; the
//! corpus runner confirms the value-correctness of the eager path.

use sheet_core::ast::{BinOp, Expr, FuncId, StructuredRef, UnOp};
use sheet_core::names::NameTarget;
use sheet_core::{CellError, CellRef, CellValue, RangeRef, SheetId, SheetModel, Table};
use sheet_fn::{coerce, Arg, EvalCtx, FnResult};

use crate::argview::{self, RangeBuf};
use crate::spill::SpillState;

/// Evaluate a formula root `expr` for the cell at `current`, with the given
/// clock/seed context. Reads dependency values straight out of `model` (fresh
/// by topo order). Spill references resolve against `spills` (a `SpillRef` in
/// scalar position yields the anchor value; the spill region is only meaningful
/// as a range argument — see [`eval_spill_ref`]).
pub fn eval_expr(model: &SheetModel, expr: &Expr, ctx: &EvalCtx, spills: &SpillState) -> CellValue {
    eval(model, expr, ctx, spills)
}

/// Evaluate a formula ROOT through the rich (array) door (spec §6.4, M1 spill
/// track). Returns [`FnResult::Array`] when the root is a `returns_array`
/// function call or an array literal that produces a 2-D block; otherwise the
/// scalar value wrapped in [`FnResult::Scalar`]. The engine uses this for
/// cells whose formula is detected as *spilling* ([`expr_spills`]); every
/// other cell stays on the scalar [`eval_expr`] path.
pub fn eval_expr_rich(
    model: &SheetModel,
    expr: &Expr,
    ctx: &EvalCtx,
    spills: &SpillState,
) -> FnResult {
    match expr {
        Expr::Func(fid, args) => eval_func_rich(model, *fid, args, ctx, spills),
        Expr::Array(rows) => eval_array_literal(model, rows, ctx, spills),
        // Not a spilling root — fall back to the scalar evaluation.
        other => FnResult::Scalar(eval(model, other, ctx, spills)),
    }
}

/// Whether a formula ROOT spills (T1 ruling, `sheet.calc.spill.materialize`): a
/// `returns_array` function call, or an array literal. The engine consults this
/// to choose the rich vs scalar evaluation path; only these roots can ever
/// produce a `FnResult::Array`.
pub fn expr_spills(expr: &Expr) -> bool {
    match expr {
        Expr::Func(fid, _) => sheet_core::funcs::meta(*fid).returns_array,
        Expr::Array(_) => true,
        _ => false,
    }
}

fn eval(model: &SheetModel, e: &Expr, ctx: &EvalCtx, spills: &SpillState) -> CellValue {
    match e {
        Expr::Lit(lit) => lit_to_value(lit),
        Expr::Ref(r) => argview::cell_value(model, *r),
        // A range (or array/union/intersection) in scalar position is #VALUE!
        // (T0 ruling: ranges are only meaningful as function arguments).
        Expr::Range(_) | Expr::Array(_) => CellValue::Error(CellError::Value),
        Expr::Name(nid) => eval_name(model, *nid),
        Expr::Unary(op, inner) => eval_unary(model, *op, inner, ctx, spills),
        Expr::Binary(op, a, b) => eval_binary(model, *op, a, b, ctx, spills),
        Expr::Func(fid, args) => eval_func(model, *fid, args, ctx, spills),
        // A structured (table) reference in SCALAR position (spec §6.4, M1
        // tables track). It resolves to a concrete range; a multi-cell range
        // in a scalar slot is `#VALUE!` (the same ruling as `Expr::Range`),
        // EXCEPT the `ThisRow`/`[@Col]` form, which implicit-intersects with
        // the formula's own row to a single cell.
        Expr::StructuredRef(s) => eval_structured_ref_scalar(model, s, ctx),
        // A spill reference `A1#` in SCALAR position yields the anchor's value
        // (`sheet.calc.spill.ref-operator`); its real use is as a range
        // argument, materialized in `eval_func`.
        Expr::SpillRef(inner) => eval_spill_ref_scalar(model, inner, ctx, spills),
    }
}

/// A `SpillRef` in scalar position: it denotes the anchor's whole spill region,
/// which is a RANGE, not a scalar. Excel yields `#VALUE!` for a spill-range
/// reference used where a single value is expected, EXCEPT that the bare anchor
/// READ is the anchor's stored value. We resolve the inner expression's anchor
/// cell: if it is a live spill anchor whose region is more than 1×1, scalar use
/// is `#VALUE!`; a 1×1 (or non-anchor) read yields the cell's value.
fn eval_spill_ref_scalar(
    model: &SheetModel,
    inner: &Expr,
    _ctx: &EvalCtx,
    spills: &SpillState,
) -> CellValue {
    let Some(anchor) = spill_anchor_of(inner) else {
        return CellValue::Error(CellError::Ref);
    };
    match spills.region_of(anchor) {
        Some(rect) if !rect.is_single() => CellValue::Error(CellError::Value),
        // A 1×1 spill (or no live region) reads the anchor cell's value.
        _ => argview::cell_value(model, anchor),
    }
}

/// The anchor cell a `SpillRef`'s inner expression denotes (only a bare cell
/// reference is a valid spill anchor in T1).
fn spill_anchor_of(inner: &Expr) -> Option<CellRef> {
    match inner {
        Expr::Ref(r) => Some(*r),
        _ => None,
    }
}

/// A structured (table) reference in SCALAR position (spec §6.4 / ECMA-376
/// §18.17.2.4). The reference resolves to a concrete range; a single-cell
/// resolution reads that cell, while a multi-cell resolution is `#VALUE!` —
/// the same scalar-position ruling as a plain `Expr::Range`. The `ThisRow`
/// (`[@Col]`) form is the one that legitimately yields a single cell: it
/// intersects the table's data rows with the formula's own row.
fn eval_structured_ref_scalar(model: &SheetModel, s: &StructuredRef, ctx: &EvalCtx) -> CellValue {
    match resolve_structured_ref(model, s, ctx) {
        Ok(r) => {
            let n = r.normalized();
            if n.rows() == 1 && n.cols() == 1 {
                argview::cell_value(model, n.start)
            } else {
                // A multi-cell area in a scalar slot — only meaningful as a
                // function argument (no implicit intersection in T0).
                CellValue::Error(CellError::Value)
            }
        }
        Err(e) => CellValue::Error(e),
    }
}

/// Resolve a [`StructuredRef`] against the workbook's table model to a concrete
/// [`RangeRef`] (spec §6.4). Errors map to the Excel rulings:
///
/// - unknown table (and not the in-table `[@…]` form whose table is resolved
///   from the formula cell) → `#NAME?`;
/// - unknown column name → `#REF!` (ECMA-376: a structured reference to a
///   missing column is a `#REF!`);
/// - the `ThisRow` form when the formula's own row is outside the table's data
///   body → `#VALUE!`.
///
/// Area semantics (over the table's FULL [`Table::range`], which includes the
/// header row when [`Table::header_row`] and the totals row when
/// [`Table::totals_row`]):
///
/// - `Data` — the body: the range minus the header/totals edge rows;
/// - `All` — the whole extent;
/// - `Headers` / `Totals` — the single edge row (`#REF!` if absent);
/// - `ThisRow` — the body row aligned with `ctx.current.row`.
///
/// The column span (`col_start`/`col_end`, both `None` = every column) clips the
/// horizontal extent; column offsets come from [`Table::column_index`].
fn resolve_structured_ref(
    model: &SheetModel,
    s: &StructuredRef,
    ctx: &EvalCtx,
) -> Result<RangeRef, CellError> {
    use sheet_core::ast::TableArea;

    // Resolve the table. The bare `[@Col]` / `[[#…],[Col]]` forms carry an
    // empty table name: anchor them to the table containing the formula's row.
    // When the formula is in no table at all, the ThisRow form is "current row
    // outside the table" → `#VALUE!` (the prompt's ruling); any other bare area
    // form is `#REF!`.
    let (sheet, table) = if s.table.is_empty() {
        match table_containing(model, ctx.current) {
            Some(pair) => pair,
            None if s.area == TableArea::ThisRow => return Err(CellError::Value),
            None => return Err(CellError::Ref),
        }
    } else {
        let (sid, t) = model.resolve_table(&s.table).ok_or(CellError::Name)?;
        (sid, t)
    };

    let full = table.range.normalized();
    // Edge-row offsets within `full` (header is the first row, totals the last).
    let header_rows = u32::from(table.header_row);
    let totals_rows = u32::from(table.totals_row);

    // Vertical extent for the requested area, as inclusive absolute rows.
    let (row0, row1) = match s.area {
        TableArea::All => (full.start.row, full.end.row),
        TableArea::Headers => {
            if !table.header_row {
                return Err(CellError::Ref);
            }
            (full.start.row, full.start.row)
        }
        TableArea::Totals => {
            if !table.totals_row {
                return Err(CellError::Ref);
            }
            (full.end.row, full.end.row)
        }
        TableArea::Data => data_body_rows(&full, header_rows, totals_rows)?,
        TableArea::ThisRow => {
            let (d0, d1) = data_body_rows(&full, header_rows, totals_rows)?;
            let cur = ctx.current.row;
            if ctx.current.sheet != sheet || cur < d0 || cur > d1 {
                // The formula's own row is outside this table's data body.
                return Err(CellError::Value);
            }
            (cur, cur)
        }
    };

    // Horizontal extent from the column span (offsets relative to `full`'s
    // left edge); both `None` selects every column.
    let (col0, col1) = match (&s.col_start, &s.col_end) {
        (None, _) => (full.start.col, full.end.col),
        (Some(c0), None) => {
            let off = table.column_index(c0).ok_or(CellError::Ref)?;
            let abs = full.start.col + off;
            (abs, abs)
        }
        (Some(c0), Some(c1)) => {
            let o0 = table.column_index(c0).ok_or(CellError::Ref)?;
            let o1 = table.column_index(c1).ok_or(CellError::Ref)?;
            let (lo, hi) = if o0 <= o1 { (o0, o1) } else { (o1, o0) };
            (full.start.col + lo, full.start.col + hi)
        }
    };

    Ok(RangeRef {
        start: CellRef {
            sheet,
            row: row0,
            col: col0,
            row_abs: false,
            col_abs: false,
        },
        end: CellRef {
            sheet,
            row: row1,
            col: col1,
            row_abs: false,
            col_abs: false,
        },
    })
}

/// The inclusive absolute row span of a table's DATA body: the full extent
/// minus the header row (if any) and totals row (if any). A table with no body
/// rows (header/totals consume the whole extent) is `#REF!`.
fn data_body_rows(
    full: &RangeRef,
    header_rows: u32,
    totals_rows: u32,
) -> Result<(u32, u32), CellError> {
    let top = full.start.row + header_rows;
    // Saturating: a totals row at/over the top collapses the body.
    let bottom = full
        .end
        .row
        .checked_sub(totals_rows)
        .ok_or(CellError::Ref)?;
    if top > bottom {
        return Err(CellError::Ref);
    }
    Ok((top, bottom))
}

/// The table the in-table `[@Col]` (ThisRow) form anchors to. ECMA-376 ties the
/// bare form to the table CONTAINING the formula, but Excel also resolves it for
/// a formula in a column just OUTSIDE the table's columns yet ROW-aligned with
/// it (the common "helper column next to the table" case). We therefore anchor
/// by ROW span: the first table on the formula's sheet whose full extent rows
/// include the current row. The column intersection then happens in
/// [`resolve_structured_ref`]. `None` if the row is in no table (eval → `#REF!`).
fn table_containing(model: &SheetModel, cell: CellRef) -> Option<(sheet_core::SheetId, &Table)> {
    let ws = model.sheet(cell.sheet)?;
    let t = ws.tables.iter().find(|t| {
        let n = t.range.normalized();
        n.start.sheet == cell.sheet && cell.row >= n.start.row && cell.row <= n.end.row
    })?;
    Some((cell.sheet, t))
}

/// Map a literal AST value to a stored [`CellValue`].
fn lit_to_value(lit: &sheet_core::ast::LitValue) -> CellValue {
    use sheet_core::ast::LitValue;
    match lit {
        LitValue::Number(n) => CellValue::Number(n.get()),
        LitValue::Text(t) => CellValue::Text(t.clone()),
        LitValue::Bool(b) => CellValue::Bool(*b),
        LitValue::Error(e) => CellValue::Error(*e),
    }
}

/// Resolve a defined name in scalar position. `Range` names collapse to their
/// top-left cell (a value context); `Formula` names are T1 → `#NAME?`.
fn eval_name(model: &SheetModel, nid: sheet_core::ast::NameId) -> CellValue {
    match model.names.get(nid) {
        Some(def) => match &def.target {
            NameTarget::Range(r) => argview::cell_value(model, r.normalized().start),
            NameTarget::Formula(_) => CellValue::Error(CellError::Name),
        },
        None => CellValue::Error(CellError::Name),
    }
}

fn eval_unary(
    model: &SheetModel,
    op: UnOp,
    inner: &Expr,
    ctx: &EvalCtx,
    spills: &SpillState,
) -> CellValue {
    let v = eval(model, inner, ctx, spills);
    if let CellValue::Error(e) = v {
        return CellValue::Error(e);
    }
    match op {
        UnOp::Neg => match coerce::to_number(&v) {
            Ok(n) => CellValue::Number(-n),
            Err(e) => CellValue::Error(e),
        },
        UnOp::Plus => match coerce::to_number(&v) {
            Ok(n) => CellValue::Number(n),
            Err(e) => CellValue::Error(e),
        },
        UnOp::Percent => match coerce::to_number(&v) {
            Ok(n) => CellValue::Number(n / 100.0),
            Err(e) => CellValue::Error(e),
        },
    }
}

fn eval_binary(
    model: &SheetModel,
    op: BinOp,
    a: &Expr,
    b: &Expr,
    ctx: &EvalCtx,
    spills: &SpillState,
) -> CellValue {
    let lhs = eval(model, a, ctx, spills);
    let rhs = eval(model, b, ctx, spills);

    // Error propagation: left operand first.
    if let CellValue::Error(e) = lhs {
        return CellValue::Error(e);
    }
    if let CellValue::Error(e) = rhs {
        return CellValue::Error(e);
    }

    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => arith(op, &lhs, &rhs),
        BinOp::Concat => {
            let mut s = coerce::to_text(&lhs).to_string();
            s.push_str(coerce::to_text(&rhs).as_str());
            CellValue::Text(s.into())
        }
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            compare(op, &lhs, &rhs)
        }
        // Reference operators in a value position are #VALUE! (parse-only T0).
        BinOp::Range | BinOp::Union | BinOp::Isect => CellValue::Error(CellError::Value),
    }
}

/// The five arithmetic operators on coerced numbers, with the Excel error
/// rulings (`#DIV/0!`, `#NUM!`).
fn arith(op: BinOp, lhs: &CellValue, rhs: &CellValue) -> CellValue {
    let x = match coerce::to_number(lhs) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let y = match coerce::to_number(rhs) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    match op {
        BinOp::Add => CellValue::Number(x + y),
        BinOp::Sub => CellValue::Number(x - y),
        BinOp::Mul => CellValue::Number(x * y),
        BinOp::Div => {
            if y == 0.0 {
                CellValue::Error(CellError::Div0)
            } else {
                CellValue::Number(x / y)
            }
        }
        BinOp::Pow => power(x, y),
        _ => unreachable!("arith called with non-arithmetic op"),
    }
}

/// `x^y` with Excel's domain rulings: `0^0` → `#NUM!`; a negative base to a
/// non-integer exponent → `#NUM!`; a non-finite result → `#NUM!`.
fn power(x: f64, y: f64) -> CellValue {
    if x == 0.0 && y == 0.0 {
        return CellValue::Error(CellError::Num);
    }
    if x < 0.0 && y.fract() != 0.0 {
        return CellValue::Error(CellError::Num);
    }
    let r = x.powf(y);
    if r.is_finite() {
        CellValue::Number(r)
    } else {
        CellValue::Error(CellError::Num)
    }
}

/// Comparison operators via the cross-type total order in `coerce::compare`.
///
/// Excel comparison semantics for the `=`/`<>`/`<`/`<=`/`>`/`>=` OPERATORS are
/// **case-insensitive for text** (`"a"="A"` is TRUE, `"a"<"A"` is FALSE). The
/// frozen `coerce::compare` is a *total* order, so it breaks a case-only tie by
/// raw bytes (`"A" < "a"`) to stay antisymmetric — correct for sorting, but it
/// would wrongly make `"a"="A"` FALSE. So at the operator layer we COLLAPSE the
/// case-only tie-break to `Equal` for two case-insensitively-equal texts, which
/// repairs every operator at once (registry `sheet.calc.recalc.topo` note: the
/// operator semantics are sheet-calc's, the total order is sheet-fn's).
fn compare(op: BinOp, lhs: &CellValue, rhs: &CellValue) -> CellValue {
    use std::cmp::Ordering;
    let ord = excel_ordering(lhs, rhs);
    let result = match op {
        BinOp::Eq => ord == Ordering::Equal,
        BinOp::Ne => ord != Ordering::Equal,
        BinOp::Lt => ord == Ordering::Less,
        BinOp::Le => ord != Ordering::Greater,
        BinOp::Gt => ord == Ordering::Greater,
        BinOp::Ge => ord != Ordering::Less,
        _ => unreachable!("compare called with non-comparison op"),
    };
    CellValue::Bool(result)
}

/// The Excel-operator ordering: `coerce::compare`, but with the case-only
/// byte tie-break between two equal-under-fold texts collapsed to `Equal`.
fn excel_ordering(lhs: &CellValue, rhs: &CellValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let ord = coerce::compare(lhs, rhs);
    if let (CellValue::Text(a), CellValue::Text(b)) = (lhs, rhs) {
        if a.eq_ignore_ascii_case(b) {
            return Ordering::Equal;
        }
    }
    ord
}

/// How one function argument materializes: a ready scalar value, or an index
/// into the owned [`RangeBuf`] backing list (a range view is lent from it only
/// once the buffers are allocated, so borrows do not overlap allocation).
enum ArgPlan {
    Scalar(CellValue),
    BufAt(usize),
}

/// Whether `fid` is SUBTOTAL or AGGREGATE — the two aggregation functions that
/// EXCLUDE nested SUBTOTAL/AGGREGATE results from their ranges (ECMA-376
/// §18.17.7). The evaluator masks those cells at range materialization
/// (`plan_args`) so the pure kernels never double-count them.
fn is_subtotal_family(fid: FuncId) -> bool {
    matches!(sheet_core::funcs::meta(fid).name, "SUBTOTAL" | "AGGREGATE")
}

/// True when the cell at `cell` holds a formula whose ROOT call is itself a
/// SUBTOTAL or AGGREGATE — i.e. the cell IS a subtotal result, which an OUTER
/// SUBTOTAL/AGGREGATE must skip (the Excel nested-exclusion rule:
/// `SUBTOTAL(9, A1:A6)` where `A6 = SUBTOTAL(9, A1:A5)` sums A1:A5 ONCE, not
/// twice). Only the direct root is matched (the standard `=SUBTOTAL(...)`
/// total-row shape); a SUBTOTAL buried inside a larger expression
/// (`=SUBTOTAL(...)+1`) is NOT treated as a subtotal result, matching Excel's
/// documented behaviour. Reads the cell's stored formula by address — the same
/// model door `ISFORMULA` / `FORMULATEXT` use.
fn cell_is_subtotal_result(model: &SheetModel, cell: CellRef) -> bool {
    let Some(ws) = model.sheet(cell.sheet) else {
        return false;
    };
    let Some(c) = ws.cell(cell.row, cell.col) else {
        return false;
    };
    let Some(fid) = c.formula else {
        return false;
    };
    let Some(f) = model.formula(fid) else {
        return false;
    };
    matches!(&f.root, Expr::Func(ffid, _) if is_subtotal_family(*ffid))
}

/// Materialize a call's arguments into the owned [`RangeBuf`] backings plus a
/// parallel [`ArgPlan`] list. Shared by [`eval_func`] and [`eval_func_rich`] so
/// the scalar and array doors build their `&[Arg]` slice identically. The caller
/// lends views from `bufs` in a second pass (so `bufs` outlives every `Arg`).
fn plan_args(
    model: &SheetModel,
    fid: FuncId,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> (Vec<RangeBuf>, Vec<ArgPlan>) {
    let meta = sheet_core::funcs::meta(fid);
    let mut bufs: Vec<RangeBuf> = Vec::new();
    let mut plans: Vec<ArgPlan> = Vec::with_capacity(args.len());

    // SUBTOTAL / AGGREGATE EXCLUDE nested SUBTOTAL/AGGREGATE results from their
    // ranges (ECMA-376 §18.17.7). We mask those cells to blank HERE — at range
    // materialization, where the evaluator holds the model — so the pure
    // kernels stay pure (they just see blanks, which every inner aggregate
    // already skips). FINDING 3: pre-fix the kernel saw plain values and
    // double-counted nested subtotals (the only flagged scope limit that
    // returned a confidently-wrong scalar — `SUBTOTAL(9,A1:A6)` with a nested
    // A6 gave 30 instead of 15).
    let exclude_subtotals = is_subtotal_family(fid);
    // Materialize a range, masking nested subtotal cells iff this is a
    // SUBTOTAL/AGGREGATE call. (A non-subtotal caller never pays the per-cell
    // formula lookup.)
    let materialize = |model: &SheetModel, r: RangeRef| -> RangeBuf {
        if exclude_subtotals {
            argview::materialize_range_masked(model, r, &mut |cell| {
                cell_is_subtotal_result(model, cell)
            })
        } else {
            argview::materialize_range(model, r)
        }
    };

    for arg in args {
        match arg {
            Expr::Range(r) => {
                let resolved = resolve_range_for_arg(model, *r);
                bufs.push(materialize(model, resolved));
                plans.push(ArgPlan::BufAt(bufs.len() - 1));
            }
            // A defined-name that targets a range materializes as a range arg.
            Expr::Name(nid) => match name_range_target(model, *nid) {
                Some(r) => {
                    bufs.push(materialize(model, r));
                    plans.push(ArgPlan::BufAt(bufs.len() - 1));
                }
                None => plans.push(ArgPlan::Scalar(eval(model, arg, ctx, spills))),
            },
            // A spill reference `A1#` materializes the WHOLE live spill region
            // as a range argument (`sheet.calc.spill.ref-operator`). With no
            // live region the anchor read degrades to its scalar value.
            Expr::SpillRef(inner) => match spill_ref_range(spills, inner) {
                Some(rect) => {
                    bufs.push(materialize(model, spill_rect_to_range(rect)));
                    plans.push(ArgPlan::BufAt(bufs.len() - 1));
                }
                None => plans.push(ArgPlan::Scalar(eval(model, arg, ctx, spills))),
            },
            // A structured (table) reference materializes its resolved area as a
            // range argument (spec §6.4): `SUM(Table1[Amount])` resolves the
            // column's data range and aggregates it. An unresolvable table /
            // column degrades to the scalar `#REF!`/`#NAME?` the eval path emits.
            Expr::StructuredRef(s) => match resolve_structured_ref(model, s, ctx) {
                Ok(r) => {
                    bufs.push(materialize(model, r));
                    plans.push(ArgPlan::BufAt(bufs.len() - 1));
                }
                Err(e) => plans.push(ArgPlan::Scalar(CellValue::Error(e))),
            },
            // For ref_args functions, a bare cell ref becomes a 1x1 range
            // carrying its origin (ROW/COLUMN need the reference).
            Expr::Ref(r) if meta.ref_args => {
                bufs.push(argview::materialize_ref_1x1(model, *r));
                plans.push(ArgPlan::BufAt(bufs.len() - 1));
            }
            // SUBTOTAL/AGGREGATE: a bare single-cell ref that IS a nested
            // subtotal result is excluded too (not just cells inside a range
            // arg). Mask it to blank so the kernel skips it; otherwise it reads
            // as its scalar value. (FINDING 3 — `SUBTOTAL(9, A1:A3, A7)` with
            // `A7 = SUBTOTAL(...)` excludes A7.)
            Expr::Ref(r) if exclude_subtotals => {
                let v = if cell_is_subtotal_result(model, *r) {
                    CellValue::Empty
                } else {
                    argview::cell_value(model, *r)
                };
                plans.push(ArgPlan::Scalar(v));
            }
            // A reference-returning special form (OFFSET/INDIRECT) as an
            // argument materializes its RESOLVED range, so a range-aware outer
            // function (`SUM(OFFSET(A1,0,0,3,1))`) sees the whole area, not just
            // the top-left (M2 Phase A). An unresolvable target degrades to the
            // scalar `#REF!`/`#VALUE!` the special-form scalar path emits.
            Expr::Func(ffid, _) if sheet_core::funcs::meta(*ffid).special_form => {
                match eval_as_ref(model, arg, ctx, spills) {
                    Some(r) => {
                        bufs.push(materialize(model, r));
                        plans.push(ArgPlan::BufAt(bufs.len() - 1));
                    }
                    None => plans.push(ArgPlan::Scalar(eval(model, arg, ctx, spills))),
                }
            }
            _ => plans.push(ArgPlan::Scalar(eval(model, arg, ctx, spills))),
        }
    }
    (bufs, plans)
}

/// Lend the borrowing `&[Arg]` slice from `bufs`/`plans` (the second pass — see
/// [`plan_args`]). `bufs` MUST outlive the returned `Vec<Arg>`.
fn build_args<'a>(bufs: &'a [RangeBuf], plans: &[ArgPlan]) -> Vec<Arg<'a>> {
    plans
        .iter()
        .map(|p| match p {
            ArgPlan::Scalar(v) => Arg::Scalar(v.clone()),
            ArgPlan::BufAt(i) => Arg::Range(bufs[*i].view()),
        })
        .collect()
}

/// Evaluate a function call: materialize each argument, then dispatch through
/// the frozen `sheet_fn` table. Arity is enforced inside `dispatch`.
///
/// EVALUATOR SPECIAL FORMS (M2 Phase A) are intercepted FIRST: a function whose
/// registry row carries `special_form: true` reads the MODEL (a reference or a
/// cell's formula by address) and so cannot be a pure `fn(&[Arg], &EvalCtx)`
/// kernel. We handle it here — where the evaluator already holds the model —
/// BEFORE materializing args and calling `dispatch` (mirroring how `ref_args`
/// and ranges are already special-cased above). The pure dispatch door for
/// such a row returns `#NAME?`; it must never be reached.
fn eval_func(
    model: &SheetModel,
    fid: FuncId,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> CellValue {
    if sheet_core::funcs::meta(fid).special_form {
        return eval_special_form(model, fid, args, ctx, spills);
    }
    let (bufs, plans) = plan_args(model, fid, args, ctx, spills);
    let built = build_args(&bufs, &plans);
    sheet_fn::dispatch(fid, &built, ctx)
}

/// Evaluate a function call through the RICH (array) door (spec §6.4). Same
/// argument materialization as [`eval_func`], but the result is a [`FnResult`]
/// (so a `returns_array` kernel's 2-D block survives to the spill engine).
fn eval_func_rich(
    model: &SheetModel,
    fid: FuncId,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> FnResult {
    // Special forms (M2 Phase A) intercept here too — they always ground to a
    // scalar (a single value, never a 2-D block in T1/T2).
    if sheet_core::funcs::meta(fid).special_form {
        return FnResult::Scalar(eval_special_form(model, fid, args, ctx, spills));
    }
    let (bufs, plans) = plan_args(model, fid, args, ctx, spills);
    let built = build_args(&bufs, &plans);
    sheet_fn::dispatch_rich(fid, &built, ctx)
}

/// The live spill region a `SpillRef`'s inner anchor owns, if any (only a bare
/// cell ref is a valid anchor in T1).
fn spill_ref_range(spills: &SpillState, inner: &Expr) -> Option<crate::spill::SpillRect> {
    let anchor = spill_anchor_of(inner)?;
    spills.region_of(anchor)
}

/// A [`crate::spill::SpillRect`] as a [`RangeRef`] for argument materialization.
fn spill_rect_to_range(rect: crate::spill::SpillRect) -> RangeRef {
    RangeRef {
        start: CellRef {
            sheet: rect.sheet,
            row: rect.row0,
            col: rect.col0,
            row_abs: false,
            col_abs: false,
        },
        end: CellRef {
            sheet: rect.sheet,
            row: rect.row1,
            col: rect.col1,
            row_abs: false,
            col_abs: false,
        },
    }
}

/// Evaluate an array literal `{1,2;3,4}` (or a legacy CSE array formula that
/// parsed to one) into a [`FnResult::Array`] (spec §6.4,
/// `sheet.calc.spill.cse-parse`). Each element is evaluated in scalar position;
/// ragged rows are NOT padded (the parser produces rectangular literals). An
/// empty literal degrades to `#VALUE!`.
fn eval_array_literal(
    model: &SheetModel,
    rows: &[Vec<Expr>],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> FnResult {
    if rows.is_empty() || rows.iter().all(|r| r.is_empty()) {
        return FnResult::Scalar(CellValue::Error(CellError::Value));
    }
    let grid: Vec<Vec<CellValue>> = rows
        .iter()
        .map(|row| row.iter().map(|e| eval(model, e, ctx, spills)).collect())
        .collect();
    FnResult::Array(grid)
}

/// Resolve a `RangeRef` for argument materialization. A range whose start
/// sheet differs from its end sheet is malformed in T0; we use the start
/// sheet (the parser never produces a cross-sheet range, but stay total).
fn resolve_range_for_arg(_model: &SheetModel, r: RangeRef) -> RangeRef {
    r
}

/// If a defined name targets a range, return it (normalized); otherwise `None`
/// (a `Formula` name has no range-arg materialization in T0).
fn name_range_target(model: &SheetModel, nid: sheet_core::ast::NameId) -> Option<RangeRef> {
    match model.names.get(nid) {
        Some(def) => match &def.target {
            NameTarget::Range(r) => Some(r.normalized()),
            NameTarget::Formula(_) => None,
        },
        None => None,
    }
}

// ============================================================================
// Evaluator special forms (spec §13 M2 Phase A)
// ============================================================================
//
// OFFSET / INDIRECT / FORMULATEXT / ISFORMULA inherently READ THE MODEL (a
// reference, an address parsed at runtime, or a cell's formula source) and so
// cannot be pure `fn(&[Arg], &EvalCtx) -> CellValue` kernels. Rather than ripple
// a model reader through the FROZEN `EvalCtx` (rejected: it touches every
// kernel), they are handled HERE in the evaluator, which already holds the
// model. Registry rows carry `special_form: true`; `eval_func`/`eval_func_rich`
// intercept BEFORE materializing args + calling dispatch (the pure dispatch door
// returns `#NAME?` for these rows — it must never be reached).
//
// VOLATILITY: OFFSET and INDIRECT are `volatility: volatile` in the registry;
// `extract_refs` flags those names, so the dirty tracker recalcs cells using
// them every pass — the special-form path does not change that contract.

/// Dispatch a special-form function call against the model (M2 Phase A). Arity
/// is range-checked here (the pure dispatch door no longer guards these rows).
/// An arity violation is `#VALUE!`, matching the pure door's convention.
fn eval_special_form(
    model: &SheetModel,
    fid: FuncId,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> CellValue {
    let meta = sheet_core::funcs::meta(fid);
    if args.len() < meta.min_args as usize || meta.max_args.is_some_and(|m| args.len() > m as usize)
    {
        return CellValue::Error(CellError::Value);
    }
    match meta.name {
        "OFFSET" => eval_offset(model, args, ctx, spills),
        "INDIRECT" => eval_indirect(model, args, ctx, spills),
        "FORMULATEXT" => eval_formulatext(model, args, ctx),
        "ISFORMULA" => eval_isformula(model, args, ctx),
        // A registered special form with no handler is an internal invariant
        // break (a new `special_form: true` row landed without wiring eval).
        _ => CellValue::Error(CellError::Name),
    }
}

/// Resolve an argument expression AS A REFERENCE (not a value): the building
/// block special forms need. Handles a bare cell ref, a range, a defined name
/// that targets a range, a structured (table) reference, a spill reference, and
/// a NESTED special form (`OFFSET`/`INDIRECT`) — so `OFFSET(INDIRECT("A1"),…)`
/// and `OFFSET(OFFSET(…),…)` resolve. Returns `None` for any expression that is
/// not reference-shaped (the caller maps that to the function's own error).
fn eval_as_ref(
    model: &SheetModel,
    expr: &Expr,
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Option<RangeRef> {
    match expr {
        Expr::Ref(r) => Some(RangeRef { start: *r, end: *r }),
        Expr::Range(r) => Some(r.normalized()),
        Expr::Name(nid) => name_range_target(model, *nid),
        Expr::StructuredRef(s) => resolve_structured_ref(model, s, ctx).ok(),
        Expr::SpillRef(inner) => spill_ref_range(spills, inner).map(spill_rect_to_range),
        // A nested reference-returning special form (OFFSET / INDIRECT).
        Expr::Func(fid, fargs) => {
            let meta = sheet_core::funcs::meta(*fid);
            if !meta.special_form {
                return None;
            }
            match meta.name {
                "OFFSET" => offset_target(model, fargs, ctx, spills).ok(),
                "INDIRECT" => indirect_target(model, fargs, ctx, spills).ok(),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `OFFSET(reference, rows, cols, [height], [width])` (ECMA-376 §18.17.7,
/// VOLATILE). Resolves the base reference, shifts its top-left by `(rows, cols)`,
/// and optionally resizes to `height × width` (defaulting to the base shape).
/// Returns the resulting range's VALUE: the top-left cell for a single cell;
/// a multi-cell area in a scalar slot is `#VALUE!` (the range-in-scalar ruling —
/// a range-aware OUTER function instead receives the materialized range when
/// OFFSET is ITS argument, via `eval_as_ref`). `#REF!` when the target leaves
/// the sheet bounds.
fn eval_offset(model: &SheetModel, args: &[Expr], ctx: &EvalCtx, spills: &SpillState) -> CellValue {
    match offset_target(model, args, ctx, spills) {
        Ok(r) => range_value_scalar(model, r),
        Err(e) => CellValue::Error(e),
    }
}

/// Compute the `RangeRef` an `OFFSET(...)` call denotes (the shared core, also
/// used when OFFSET is nested under another reference function). Errors:
/// `#REF!` (unresolvable base, out-of-bounds target, non-positive height/width),
/// `#VALUE!` (a non-numeric rows/cols/height/width argument).
fn offset_target(
    model: &SheetModel,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Result<RangeRef, CellError> {
    let base = eval_as_ref(model, &args[0], ctx, spills).ok_or(CellError::Ref)?;
    let base = base.normalized();
    let d_rows = arg_to_i64(model, &args[1], ctx, spills)?;
    let d_cols = arg_to_i64(model, &args[2], ctx, spills)?;
    // Optional height/width default to the base range's dimensions.
    let height = match args.get(3) {
        Some(e) => arg_to_i64(model, e, ctx, spills)?,
        None => base.rows() as i64,
    };
    let width = match args.get(4) {
        Some(e) => arg_to_i64(model, e, ctx, spills)?,
        None => base.cols() as i64,
    };
    if height <= 0 || width <= 0 {
        return Err(CellError::Ref);
    }

    let new_top = base.start.row as i64 + d_rows;
    let new_left = base.start.col as i64 + d_cols;
    let new_bottom = new_top + height - 1;
    let new_right = new_left + width - 1;

    // Out of the grid (either corner) → #REF!.
    if new_top < 0
        || new_left < 0
        || new_bottom > sheet_core::MAX_ROW as i64
        || new_right > sheet_core::MAX_COL as i64
    {
        return Err(CellError::Ref);
    }

    let sheet = base.start.sheet;
    Ok(RangeRef {
        start: CellRef {
            sheet,
            row: new_top as u32,
            col: new_left as u32,
            row_abs: false,
            col_abs: false,
        },
        end: CellRef {
            sheet,
            row: new_bottom as u32,
            col: new_right as u32,
            row_abs: false,
            col_abs: false,
        },
    })
}

/// `INDIRECT(ref_text, [a1])` (ECMA-376 §18.17.7, VOLATILE). Evaluates `arg0`
/// to text, parses it as an A1 cell or range reference against the model, and
/// returns its value. `#REF!` on an unparseable address. The `a1` argument
/// defaults to TRUE (A1 style); `a1 = FALSE` (R1C1) is a documented T2
/// limitation → `#REF!`.
fn eval_indirect(
    model: &SheetModel,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> CellValue {
    match indirect_target(model, args, ctx, spills) {
        Ok(r) => range_value_scalar(model, r),
        Err(e) => CellValue::Error(e),
    }
}

/// Compute the `RangeRef` an `INDIRECT(...)` call denotes (shared with nesting).
/// `#REF!` for an unparseable address or an R1C1 (`a1 = FALSE`) request (T2
/// limitation); `#VALUE!` only via the text coercion of a hard error argument.
fn indirect_target(
    model: &SheetModel,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Result<RangeRef, CellError> {
    // Evaluate arg0 to a scalar; an error argument propagates.
    let v = eval(model, &args[0], ctx, spills);
    if let CellValue::Error(e) = v {
        return Err(e);
    }
    // The `a1` flag: default TRUE; FALSE (R1C1) is a documented T2 limitation.
    if let Some(a1_expr) = args.get(1) {
        let a1v = eval(model, a1_expr, ctx, spills);
        if let CellValue::Error(e) = a1v {
            return Err(e);
        }
        if let Ok(n) = coerce::to_number(&a1v) {
            if n == 0.0 {
                // R1C1 addressing — not parsed in T2.
                return Err(CellError::Ref);
            }
        }
    }
    let text = coerce::to_text(&v);
    parse_a1_reference(model, text.as_str(), ctx.current.sheet).ok_or(CellError::Ref)
}

/// Parse an A1 reference string (`"A1"`, `"A1:B3"`, `"Sheet2!A1"`, a
/// `"Sheet!A1:B3"` range) into a `RangeRef` against the model. The sheet prefix
/// is optional (defaults to `home`). Returns `None` for any unparseable form
/// (INDIRECT maps that to `#REF!`). R1C1 is not handled here (the caller rejects
/// `a1 = FALSE` first).
fn parse_a1_reference(model: &SheetModel, text: &str, home: SheetId) -> Option<RangeRef> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // Split an optional `Sheet!` prefix (quoted or bare). Only the LAST `!`
    // separates the sheet from the address (sheet names may not contain `!`).
    let (sheet, addr) = match text.rsplit_once('!') {
        Some((name, rest)) => {
            let name = unquote_sheet(name);
            let sid = model.sheet_id(&name)?;
            (sid, rest)
        }
        None => (home, text),
    };
    // A range `A1:B3` or a single cell `A1`.
    if let Some((a, b)) = addr.split_once(':') {
        let (r0, c0, ra0, ca0) = sheet_core::parse_a1(a.trim())?;
        let (r1, c1, ra1, ca1) = sheet_core::parse_a1(b.trim())?;
        Some(RangeRef {
            start: CellRef {
                sheet,
                row: r0,
                col: c0,
                row_abs: ra0,
                col_abs: ca0,
            },
            end: CellRef {
                sheet,
                row: r1,
                col: c1,
                row_abs: ra1,
                col_abs: ca1,
            },
        })
    } else {
        let (row, col, row_abs, col_abs) = sheet_core::parse_a1(addr.trim())?;
        let c = CellRef {
            sheet,
            row,
            col,
            row_abs,
            col_abs,
        };
        Some(RangeRef { start: c, end: c })
    }
}

/// Strip the surrounding `'…'` of a quoted sheet name and unescape `''` → `'`.
fn unquote_sheet(name: &str) -> String {
    let name = name.trim();
    if name.len() >= 2 && name.starts_with('\'') && name.ends_with('\'') {
        name[1..name.len() - 1].replace("''", "'")
    } else {
        name.to_string()
    }
}

/// `FORMULATEXT(reference)` (Microsoft public docs). Resolves the argument to a
/// CellRef (the top-left of a multi-cell reference, Excel semantics), and returns
/// the cell's formula PRINTED with a leading `=`. `#N/A` if the cell holds no
/// formula (a literal or blank), and `#N/A` if the argument is not a reference.
fn eval_formulatext(model: &SheetModel, args: &[Expr], ctx: &EvalCtx) -> CellValue {
    let spills = SpillState::new();
    let Some(r) = eval_as_ref(model, &args[0], ctx, &spills) else {
        // FORMULATEXT of a non-reference is #N/A (Excel: #VALUE! for a literal;
        // ruling: #N/A — "the formula text is unavailable", documented).
        return CellValue::Error(CellError::Na);
    };
    let cell = r.normalized().start;
    let Some(ws) = model.sheet(cell.sheet) else {
        return CellValue::Error(CellError::Na);
    };
    let Some(c) = ws.cell(cell.row, cell.col) else {
        return CellValue::Error(CellError::Na);
    };
    let Some(fid) = c.formula else {
        return CellValue::Error(CellError::Na);
    };
    let Some(f) = model.formula(fid) else {
        return CellValue::Error(CellError::Na);
    };
    let names = ModelSheetNames { model };
    let mut text = String::with_capacity(1);
    text.push('=');
    text.push_str(&sheet_parser::print(f, cell.sheet, &names));
    CellValue::Text(text.into())
}

/// `ISFORMULA(reference)` (Microsoft public docs). Resolves the argument to a
/// CellRef and returns `TRUE` iff that cell stores a formula. A non-reference
/// argument is `#VALUE!` (Excel semantics for ISFORMULA of a non-reference).
fn eval_isformula(model: &SheetModel, args: &[Expr], ctx: &EvalCtx) -> CellValue {
    let spills = SpillState::new();
    let Some(r) = eval_as_ref(model, &args[0], ctx, &spills) else {
        return CellValue::Error(CellError::Value);
    };
    let cell = r.normalized().start;
    let is_formula = model
        .sheet(cell.sheet)
        .and_then(|ws| ws.cell(cell.row, cell.col))
        .map(|c| c.formula.is_some())
        .unwrap_or(false);
    CellValue::Bool(is_formula)
}

/// The scalar value of a resolved `RangeRef`: the top-left cell's value for a
/// single cell; a multi-cell area in a scalar slot is `#VALUE!` (the same
/// range-in-scalar ruling as `Expr::Range`). A range-aware OUTER function that
/// takes OFFSET/INDIRECT as an argument receives the whole range instead (via
/// `eval_as_ref` in `plan_args` — see the special-form arg handling).
fn range_value_scalar(model: &SheetModel, r: RangeRef) -> CellValue {
    let n = r.normalized();
    if n.rows() == 1 && n.cols() == 1 {
        argview::cell_value(model, n.start)
    } else {
        CellValue::Error(CellError::Value)
    }
}

/// Coerce a special-form numeric argument (rows/cols/height/width) to a
/// truncated `i64`. An error argument propagates; a non-numeric is `#VALUE!`.
/// (OFFSET truncates toward zero, Excel semantics.)
fn arg_to_i64(
    model: &SheetModel,
    expr: &Expr,
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Result<i64, CellError> {
    let v = eval(model, expr, ctx, spills);
    if let CellValue::Error(e) = v {
        return Err(e);
    }
    let n = coerce::to_number(&v)?;
    Ok(n.trunc() as i64)
}

/// A [`sheet_parser::SheetNames`] view over the model, for FORMULATEXT's printer.
struct ModelSheetNames<'a> {
    model: &'a SheetModel,
}

impl sheet_parser::SheetNames for ModelSheetNames<'_> {
    fn sheet_name(&self, id: SheetId) -> Option<&str> {
        self.model.sheet(id).map(|ws| ws.name.as_str())
    }
}

/// The cell currently being evaluated — exposed for the engine to build the
/// per-cell [`EvalCtx`]. Kept here so the eval layer owns the convention.
pub fn ctx_for(model: &SheetModel, current: CellRef, now_serial: f64, rng_seed: u64) -> EvalCtx {
    EvalCtx::new(model.calc.date_system, current, now_serial, rng_seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::ast::{LitValue, OrderedF64};
    use sheet_core::Cell;

    fn cr(row: u32, col: u32) -> CellRef {
        CellRef {
            sheet: 0,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn model() -> SheetModel {
        let mut m = SheetModel::new();
        m.add_sheet("Sheet1");
        m
    }

    fn set(m: &mut SheetModel, row: u32, col: u32, v: CellValue) {
        m.sheet_mut(0).unwrap().set_cell(
            row,
            col,
            Cell {
                value: v,
                ..Default::default()
            },
        );
    }

    fn ctx() -> EvalCtx {
        EvalCtx::new(sheet_core::DateSystem::Date1900, cr(0, 0), 0.0, 1)
    }

    /// An empty spill ledger — the resting state for non-spilling eval tests.
    fn spills() -> SpillState {
        SpillState::new()
    }

    fn num(n: f64) -> Expr {
        Expr::Lit(LitValue::Number(OrderedF64::new(n)))
    }

    #[test]
    fn arithmetic_and_div0() {
        let m = model();
        let s = spills();
        let add = Expr::Binary(BinOp::Add, Box::new(num(2.0)), Box::new(num(3.0)));
        assert_eq!(eval_expr(&m, &add, &ctx(), &s), CellValue::Number(5.0));
        let div = Expr::Binary(BinOp::Div, Box::new(num(1.0)), Box::new(num(0.0)));
        assert_eq!(
            eval_expr(&m, &div, &ctx(), &s),
            CellValue::Error(CellError::Div0)
        );
    }

    #[test]
    fn pow_domain_rulings() {
        let m = model();
        let s = spills();
        let zz = Expr::Binary(BinOp::Pow, Box::new(num(0.0)), Box::new(num(0.0)));
        assert_eq!(
            eval_expr(&m, &zz, &ctx(), &s),
            CellValue::Error(CellError::Num)
        );
        let neg_frac = Expr::Binary(BinOp::Pow, Box::new(num(-2.0)), Box::new(num(0.5)));
        assert_eq!(
            eval_expr(&m, &neg_frac, &ctx(), &s),
            CellValue::Error(CellError::Num)
        );
        let ok = Expr::Binary(BinOp::Pow, Box::new(num(2.0)), Box::new(num(10.0)));
        assert_eq!(eval_expr(&m, &ok, &ctx(), &s), CellValue::Number(1024.0));
    }

    #[test]
    fn ref_reads_current_value_and_empty_is_zero() {
        let mut m = model();
        let s = spills();
        set(&mut m, 0, 0, CellValue::Number(7.0));
        let refa1 = Expr::Ref(cr(0, 0));
        assert_eq!(eval_expr(&m, &refa1, &ctx(), &s), CellValue::Number(7.0));
        // Empty ref in arithmetic coerces to 0.
        let add = Expr::Binary(
            BinOp::Add,
            Box::new(Expr::Ref(cr(5, 5))),
            Box::new(num(1.0)),
        );
        assert_eq!(eval_expr(&m, &add, &ctx(), &s), CellValue::Number(1.0));
    }

    #[test]
    fn comparison_excel_text_equality() {
        let m = model();
        let s = spills();
        let eq = Expr::Binary(
            BinOp::Eq,
            Box::new(Expr::Lit(LitValue::Text("ABC".into()))),
            Box::new(Expr::Lit(LitValue::Text("abc".into()))),
        );
        // Text equality is case-insensitive in Excel.
        assert_eq!(eval_expr(&m, &eq, &ctx(), &s), CellValue::Bool(true));
    }

    #[test]
    fn concat_coerces_to_text() {
        let m = model();
        let s = spills();
        let cat = Expr::Binary(
            BinOp::Concat,
            Box::new(num(1.0)),
            Box::new(Expr::Lit(LitValue::Text("x".into()))),
        );
        assert_eq!(eval_expr(&m, &cat, &ctx(), &s), CellValue::from("1x"));
    }

    #[test]
    fn range_in_scalar_position_is_value_error() {
        let m = model();
        let s = spills();
        let rng = Expr::Range(RangeRef {
            start: cr(0, 0),
            end: cr(2, 0),
        });
        assert_eq!(
            eval_expr(&m, &rng, &ctx(), &s),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn func_sum_over_range() {
        let mut m = model();
        let s = spills();
        set(&mut m, 0, 0, CellValue::Number(1.0));
        set(&mut m, 1, 0, CellValue::Number(2.0));
        set(&mut m, 2, 0, CellValue::Number(3.0));
        let fid = sheet_core::funcs::lookup_func("SUM").unwrap();
        let call = Expr::Func(
            fid,
            vec![Expr::Range(RangeRef {
                start: cr(0, 0),
                end: cr(2, 0),
            })],
        );
        assert_eq!(eval_expr(&m, &call, &ctx(), &s), CellValue::Number(6.0));
    }

    #[test]
    fn error_propagation_left_first() {
        let mut m = model();
        let s = spills();
        set(&mut m, 0, 0, CellValue::Error(CellError::Div0));
        set(&mut m, 0, 1, CellValue::Error(CellError::Na));
        let add = Expr::Binary(
            BinOp::Add,
            Box::new(Expr::Ref(cr(0, 0))),
            Box::new(Expr::Ref(cr(0, 1))),
        );
        assert_eq!(
            eval_expr(&m, &add, &ctx(), &s),
            CellValue::Error(CellError::Div0)
        );
    }

    #[test]
    fn array_literal_evaluates_to_array() {
        // `{1,2;3,4}` evaluates element-wise to a 2x2 FnResult::Array
        // (sheet.calc.spill.cse-parse — legacy CSE arrays use this path too).
        let m = model();
        let s = spills();
        let lit = Expr::Array(vec![vec![num(1.0), num(2.0)], vec![num(3.0), num(4.0)]]);
        assert!(expr_spills(&lit));
        match eval_expr_rich(&m, &lit, &ctx(), &s) {
            FnResult::Array(grid) => {
                assert_eq!(grid.len(), 2);
                assert_eq!(
                    grid[0],
                    vec![CellValue::Number(1.0), CellValue::Number(2.0)]
                );
                assert_eq!(
                    grid[1],
                    vec![CellValue::Number(3.0), CellValue::Number(4.0)]
                );
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn expr_spills_detects_array_roots_only() {
        // A returns_array function root spills; a scalar function root does not.
        let seq = sheet_core::funcs::lookup_func("SEQUENCE").unwrap();
        let sum = sheet_core::funcs::lookup_func("SUM").unwrap();
        assert!(expr_spills(&Expr::Func(seq, vec![])));
        assert!(!expr_spills(&Expr::Func(sum, vec![])));
        assert!(!expr_spills(&num(1.0)));
    }

    // ---- Structured (table) references (spec §6.4, tables track) ----

    use sheet_core::ast::TableArea;
    use sheet_core::Table;

    /// A model with `Sales` on sheet 0: header row at row 0, data rows 1..=3,
    /// columns [Region, Units, Total] in cols A..C. Cells seeded with numbers.
    fn table_model() -> SheetModel {
        let mut m = model();
        // Header labels (row 0) + data (rows 1..3).
        set(&mut m, 0, 0, CellValue::from("Region"));
        set(&mut m, 0, 1, CellValue::from("Units"));
        set(&mut m, 0, 2, CellValue::from("Total"));
        for (i, (u, t)) in [(10.0, 100.0), (20.0, 200.0), (30.0, 300.0)]
            .iter()
            .enumerate()
        {
            let r = 1 + i as u32;
            set(&mut m, r, 0, CellValue::from("R"));
            set(&mut m, r, 1, CellValue::Number(*u));
            set(&mut m, r, 2, CellValue::Number(*t));
        }
        let table = Table {
            name: "Sales".into(),
            range: RangeRef {
                start: cr(0, 0),
                end: cr(3, 2),
            }, // A1:C4 (header + 3 data rows)
            columns: vec!["Region".into(), "Units".into(), "Total".into()],
            header_row: true,
            totals_row: false,
            style_name: None,
        };
        m.sheet_mut(0).unwrap().tables.push(table);
        m
    }

    fn sref(table: &str, area: TableArea, col: Option<&str>) -> Expr {
        Expr::StructuredRef(StructuredRef {
            table: table.into(),
            area,
            col_start: col.map(Into::into),
            col_end: None,
        })
    }

    #[test]
    fn structured_ref_column_sum() {
        // SUM(Sales[Units]) over the data body (rows 1..=3) = 60.
        let m = table_model();
        let s = spills();
        let fid = sheet_core::funcs::lookup_func("SUM").unwrap();
        let call = Expr::Func(fid, vec![sref("Sales", TableArea::Data, Some("Units"))]);
        assert_eq!(eval_expr(&m, &call, &ctx(), &s), CellValue::Number(60.0));
        // The Total column sums to 600.
        let call2 = Expr::Func(fid, vec![sref("Sales", TableArea::Data, Some("Total"))]);
        assert_eq!(eval_expr(&m, &call2, &ctx(), &s), CellValue::Number(600.0));
    }

    #[test]
    fn structured_ref_data_excludes_header() {
        // The Data area excludes the header row: COUNT(Sales[Units]) counts only
        // the 3 numeric data cells (the header "Units" text is not counted).
        let m = table_model();
        let s = spills();
        let fid = sheet_core::funcs::lookup_func("COUNT").unwrap();
        let call = Expr::Func(fid, vec![sref("Sales", TableArea::Data, Some("Units"))]);
        assert_eq!(eval_expr(&m, &call, &ctx(), &s), CellValue::Number(3.0));
    }

    #[test]
    fn structured_ref_thisrow_intersects_current_row() {
        // `[@Units]` evaluated at row 2 reads the single data cell (row 2, col 1)
        // = 20. The empty table name is resolved from the formula's own cell.
        let m = table_model();
        let s = spills();
        let ctx_row2 = EvalCtx::new(sheet_core::DateSystem::Date1900, cr(2, 5), 0.0, 1);
        let e = sref("", TableArea::ThisRow, Some("Units"));
        assert_eq!(eval_expr(&m, &e, &ctx_row2, &s), CellValue::Number(20.0));
    }

    #[test]
    fn structured_ref_thisrow_outside_table_is_value_error() {
        // `[@Units]` evaluated at a row OUTSIDE the table data body → #VALUE!.
        let m = table_model();
        let s = spills();
        // Row 0 is the header (not data); row 9 is below the table.
        let e = sref("", TableArea::ThisRow, Some("Units"));
        let ctx_hdr = EvalCtx::new(sheet_core::DateSystem::Date1900, cr(0, 5), 0.0, 1);
        assert_eq!(
            eval_expr(&m, &e, &ctx_hdr, &s),
            CellValue::Error(CellError::Value)
        );
        let ctx_below = EvalCtx::new(sheet_core::DateSystem::Date1900, cr(9, 5), 0.0, 1);
        assert_eq!(
            eval_expr(&m, &e, &ctx_below, &s),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn structured_ref_multicell_in_scalar_is_value_error() {
        // A multi-row column in a SCALAR slot is #VALUE! (range-in-scalar ruling).
        let m = table_model();
        let s = spills();
        let e = sref("Sales", TableArea::Data, Some("Units"));
        assert_eq!(
            eval_expr(&m, &e, &ctx(), &s),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn structured_ref_unknown_table_is_name_error() {
        let m = table_model();
        let s = spills();
        let e = sref("Nope", TableArea::Data, Some("Units"));
        assert_eq!(
            eval_expr(&m, &e, &ctx(), &s),
            CellValue::Error(CellError::Name)
        );
    }

    #[test]
    fn structured_ref_unknown_column_is_ref_error() {
        let m = table_model();
        let s = spills();
        let fid = sheet_core::funcs::lookup_func("SUM").unwrap();
        let call = Expr::Func(fid, vec![sref("Sales", TableArea::Data, Some("Missing"))]);
        assert_eq!(
            eval_expr(&m, &call, &ctx(), &s),
            CellValue::Error(CellError::Ref)
        );
    }

    #[test]
    fn structured_ref_headers_area_reads_header_cell() {
        // Sales[[#Headers],[Units]] is the single header cell (row 0, col 1).
        let m = table_model();
        let s = spills();
        let e = sref("Sales", TableArea::Headers, Some("Units"));
        assert_eq!(eval_expr(&m, &e, &ctx(), &s), CellValue::from("Units"));
    }
}
