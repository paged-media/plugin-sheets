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

use sheet_core::ast::{BinOp, Expr, FuncId, UnOp};
use sheet_core::names::NameTarget;
use sheet_core::{CellError, CellRef, CellValue, RangeRef, SheetModel};
use sheet_fn::{coerce, Arg, EvalCtx};

use crate::argview::{self, RangeBuf};

/// Evaluate a formula root `expr` for the cell at `current`, with the given
/// clock/seed context. Reads dependency values straight out of `model` (fresh
/// by topo order).
pub fn eval_expr(model: &SheetModel, expr: &Expr, ctx: &EvalCtx) -> CellValue {
    eval(model, expr, ctx)
}

fn eval(model: &SheetModel, e: &Expr, ctx: &EvalCtx) -> CellValue {
    match e {
        Expr::Lit(lit) => lit_to_value(lit),
        Expr::Ref(r) => argview::cell_value(model, *r),
        // A range (or array/union/intersection) in scalar position is #VALUE!
        // (T0 ruling: ranges are only meaningful as function arguments).
        Expr::Range(_) | Expr::Array(_) => CellValue::Error(CellError::Value),
        Expr::Name(nid) => eval_name(model, *nid),
        Expr::Unary(op, inner) => eval_unary(model, *op, inner, ctx),
        Expr::Binary(op, a, b) => eval_binary(model, *op, a, b, ctx),
        Expr::Func(fid, args) => eval_func(model, *fid, args, ctx),
        // M1 Phase B (spill/tables tracks) wires these. Until then a
        // structured or spill reference evaluates to #NAME? (it is parsed
        // but not yet resolvable).
        Expr::StructuredRef(_) | Expr::SpillRef(_) => CellValue::Error(CellError::Name),
    }
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

fn eval_unary(model: &SheetModel, op: UnOp, inner: &Expr, ctx: &EvalCtx) -> CellValue {
    let v = eval(model, inner, ctx);
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

fn eval_binary(model: &SheetModel, op: BinOp, a: &Expr, b: &Expr, ctx: &EvalCtx) -> CellValue {
    let lhs = eval(model, a, ctx);
    let rhs = eval(model, b, ctx);

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

/// Evaluate a function call: materialize each argument, then dispatch through
/// the frozen `sheet_fn` table. Arity is enforced inside `dispatch`.
fn eval_func(model: &SheetModel, fid: FuncId, args: &[Expr], ctx: &EvalCtx) -> CellValue {
    let meta = sheet_core::funcs::meta(fid);

    // Owned backing for any range args (the borrowing `Arg::Range` view needs
    // a buffer that outlives the dispatch call).
    let mut bufs: Vec<RangeBuf> = Vec::new();
    // A parallel plan describing how each arg materializes, so we can build the
    // `&[Arg]` slice in one pass after the buffers are allocated.
    enum Plan {
        Scalar(CellValue),
        BufAt(usize),
    }
    let mut plans: Vec<Plan> = Vec::with_capacity(args.len());

    for arg in args {
        match arg {
            Expr::Range(r) => {
                let resolved = resolve_range_for_arg(model, *r);
                bufs.push(argview::materialize_range(model, resolved));
                plans.push(Plan::BufAt(bufs.len() - 1));
            }
            // A defined-name that targets a range materializes as a range arg.
            Expr::Name(nid) => match name_range_target(model, *nid) {
                Some(r) => {
                    bufs.push(argview::materialize_range(model, r));
                    plans.push(Plan::BufAt(bufs.len() - 1));
                }
                None => plans.push(Plan::Scalar(eval(model, arg, ctx))),
            },
            // For ref_args functions, a bare cell ref becomes a 1x1 range
            // carrying its origin (ROW/COLUMN need the reference).
            Expr::Ref(r) if meta.ref_args => {
                bufs.push(argview::materialize_ref_1x1(model, *r));
                plans.push(Plan::BufAt(bufs.len() - 1));
            }
            _ => plans.push(Plan::Scalar(eval(model, arg, ctx))),
        }
    }

    let built: Vec<Arg> = plans
        .iter()
        .map(|p| match p {
            Plan::Scalar(v) => Arg::Scalar(v.clone()),
            Plan::BufAt(i) => Arg::Range(bufs[*i].view()),
        })
        .collect();

    sheet_fn::dispatch(fid, &built, ctx)
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

    fn num(n: f64) -> Expr {
        Expr::Lit(LitValue::Number(OrderedF64::new(n)))
    }

    #[test]
    fn arithmetic_and_div0() {
        let m = model();
        let add = Expr::Binary(BinOp::Add, Box::new(num(2.0)), Box::new(num(3.0)));
        assert_eq!(eval_expr(&m, &add, &ctx()), CellValue::Number(5.0));
        let div = Expr::Binary(BinOp::Div, Box::new(num(1.0)), Box::new(num(0.0)));
        assert_eq!(
            eval_expr(&m, &div, &ctx()),
            CellValue::Error(CellError::Div0)
        );
    }

    #[test]
    fn pow_domain_rulings() {
        let m = model();
        let zz = Expr::Binary(BinOp::Pow, Box::new(num(0.0)), Box::new(num(0.0)));
        assert_eq!(eval_expr(&m, &zz, &ctx()), CellValue::Error(CellError::Num));
        let neg_frac = Expr::Binary(BinOp::Pow, Box::new(num(-2.0)), Box::new(num(0.5)));
        assert_eq!(
            eval_expr(&m, &neg_frac, &ctx()),
            CellValue::Error(CellError::Num)
        );
        let ok = Expr::Binary(BinOp::Pow, Box::new(num(2.0)), Box::new(num(10.0)));
        assert_eq!(eval_expr(&m, &ok, &ctx()), CellValue::Number(1024.0));
    }

    #[test]
    fn ref_reads_current_value_and_empty_is_zero() {
        let mut m = model();
        set(&mut m, 0, 0, CellValue::Number(7.0));
        let refa1 = Expr::Ref(cr(0, 0));
        assert_eq!(eval_expr(&m, &refa1, &ctx()), CellValue::Number(7.0));
        // Empty ref in arithmetic coerces to 0.
        let add = Expr::Binary(
            BinOp::Add,
            Box::new(Expr::Ref(cr(5, 5))),
            Box::new(num(1.0)),
        );
        assert_eq!(eval_expr(&m, &add, &ctx()), CellValue::Number(1.0));
    }

    #[test]
    fn comparison_excel_text_equality() {
        let m = model();
        let eq = Expr::Binary(
            BinOp::Eq,
            Box::new(Expr::Lit(LitValue::Text("ABC".into()))),
            Box::new(Expr::Lit(LitValue::Text("abc".into()))),
        );
        // Text equality is case-insensitive in Excel.
        assert_eq!(eval_expr(&m, &eq, &ctx()), CellValue::Bool(true));
    }

    #[test]
    fn concat_coerces_to_text() {
        let m = model();
        let cat = Expr::Binary(
            BinOp::Concat,
            Box::new(num(1.0)),
            Box::new(Expr::Lit(LitValue::Text("x".into()))),
        );
        assert_eq!(eval_expr(&m, &cat, &ctx()), CellValue::from("1x"));
    }

    #[test]
    fn range_in_scalar_position_is_value_error() {
        let m = model();
        let rng = Expr::Range(RangeRef {
            start: cr(0, 0),
            end: cr(2, 0),
        });
        assert_eq!(
            eval_expr(&m, &rng, &ctx()),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn func_sum_over_range() {
        let mut m = model();
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
        assert_eq!(eval_expr(&m, &call, &ctx()), CellValue::Number(6.0));
    }

    #[test]
    fn error_propagation_left_first() {
        let mut m = model();
        set(&mut m, 0, 0, CellValue::Error(CellError::Div0));
        set(&mut m, 0, 1, CellValue::Error(CellError::Na));
        let add = Expr::Binary(
            BinOp::Add,
            Box::new(Expr::Ref(cr(0, 0))),
            Box::new(Expr::Ref(cr(0, 1))),
        );
        assert_eq!(
            eval_expr(&m, &add, &ctx()),
            CellValue::Error(CellError::Div0)
        );
    }
}
