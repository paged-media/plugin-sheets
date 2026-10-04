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

//! `LET`, `LAMBDA` and the lambda helpers (`MAP`, `REDUCE`, `SCAN`, `BYROW`,
//! `BYCOL`, `MAKEARRAY`) — evaluator special forms (Wave 7; Microsoft LET /
//! LAMBDA / helper-function docs).
//!
//! ## Evaluation by substitution
//!
//! A bound name is replaced in the AST by the expression of its value before
//! the body is evaluated, so the rest of the evaluator never sees an
//! environment:
//!
//! - a **reference-shaped** binding (`A1`, `A1:B3`, a range name, a
//!   structured/spill reference, `OFFSET`/`INDIRECT`) substitutes the resolved
//!   reference — `LET(r, A1:A3, SUM(r))` still hands `SUM` a range;
//! - a **lambda** binding substitutes the `LAMBDA(...)` expression itself (its
//!   free names were already substituted by the enclosing scopes — a closure);
//! - any other binding is evaluated ONCE and substitutes literals (a block
//!   becomes an array literal), so `LET(x, RAND(), x - x)` is `0`.
//!
//! Substitution respects shadowing: an inner `LET` re-binding the name, or a
//! `LAMBDA` parameter of the same name, stops it (names compare
//! case-insensitively).
//!
//! ## Rulings
//!
//! - A cell whose value is a lambda (`=LAMBDA(x, x)`) is Excel's `#CALC!`,
//!   grounded to `#VALUE!` (the wire enum has no `#CALC!`; see `array`).
//! - Calling with the wrong number of arguments, or calling something that is
//!   not a lambda, is `#VALUE!`.
//! - A helper whose lambda returns a block for one element (`MAP` / `BYROW` /
//!   …) puts `#VALUE!` (grounded `#CALC!`) in that slot.
//! - `MAP` over a reference passes each cell as a single-cell reference,
//!   `BYROW`/`BYCOL` pass the row/column as a reference; over a computed block
//!   they pass values (a blank computed value is `0`).
//! - `MAKEARRAY` beyond 1,048,576 cells is `#NUM!` (an allocation guard).

use sheet_core::ast::{FuncId, LitValue, OrderedF64};

use super::*;

/// The most cells a lambda helper will build (`MAKEARRAY` guard).
const MAX_CELLS: u64 = 1 << 20;

/// What an expression reduces to: a lambda (parameters + body) or a value.
enum Reduced {
    Lambda(Vec<String>, Expr),
    Value(FnResult),
}

fn err(e: CellError) -> FnResult {
    FnResult::Scalar(CellValue::Error(e))
}

/// A block result in a scalar slot: 1×1 is its value, larger is `#VALUE!`.
pub(super) fn collapse(r: FnResult) -> CellValue {
    match r {
        FnResult::Scalar(v) => v,
        FnResult::Array(g) => {
            if g.len() == 1 && g[0].len() == 1 {
                g.into_iter().next().unwrap().into_iter().next().unwrap()
            } else {
                CellValue::Error(CellError::Value)
            }
        }
    }
}

fn lambda_fid() -> FuncId {
    sheet_core::funcs::lookup_func("LAMBDA").expect("LAMBDA is registered")
}

/// The special forms this module owns, evaluated through the rich door.
/// `None` for every other function (the caller falls through).
pub(super) fn special_rich(
    model: &SheetModel,
    fid: FuncId,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Option<FnResult> {
    let meta = sheet_core::funcs::meta(fid);
    let name = meta.name;
    if !matches!(
        name,
        "LET" | "LAMBDA" | "MAP" | "REDUCE" | "SCAN" | "BYROW" | "BYCOL" | "MAKEARRAY"
    ) {
        return None;
    }
    if args.len() < meta.min_args as usize || meta.max_args.is_some_and(|m| args.len() > m as usize)
    {
        return Some(err(CellError::Value));
    }
    Some(match name {
        "LET" => match reduce_let(model, args, ctx, spills) {
            Reduced::Value(v) => v,
            // A lambda as a cell value: #CALC! grounded to #VALUE!.
            Reduced::Lambda(..) => err(CellError::Value),
        },
        "LAMBDA" => err(CellError::Value),
        "MAP" => map(model, args, ctx, spills),
        "REDUCE" => reduce_scan(model, args, ctx, spills, false),
        "SCAN" => reduce_scan(model, args, ctx, spills, true),
        "BYROW" => by_line(model, args, ctx, spills, true),
        "BYCOL" => by_line(model, args, ctx, spills, false),
        "MAKEARRAY" => makearray(model, args, ctx, spills),
        _ => unreachable!(),
    })
}

/// Evaluate a call `callee(args)`.
pub(super) fn eval_call(
    model: &SheetModel,
    callee: &Expr,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> FnResult {
    match reduce_call(model, callee, args, ctx, spills) {
        Reduced::Value(v) => v,
        Reduced::Lambda(..) => err(CellError::Value),
    }
}

// ---- reduction -------------------------------------------------------------

fn is_func(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Func(fid, _) if sheet_core::funcs::meta(*fid).name == name)
}

/// Reduce an expression: a lambda stays a lambda, LET and calls are applied,
/// anything else is evaluated (a reference to its values).
fn reduce(model: &SheetModel, e: &Expr, ctx: &EvalCtx, spills: &SpillState) -> Reduced {
    match e {
        Expr::Func(_, args) if is_func(e, "LAMBDA") => {
            let (params, body) = args.split_at(args.len() - 1);
            let mut names = Vec::with_capacity(params.len());
            for p in params {
                match p {
                    Expr::Local(n) => names.push(n.to_string()),
                    _ => return Reduced::Value(err(CellError::Value)),
                }
            }
            Reduced::Lambda(names, body[0].clone())
        }
        Expr::Func(_, args) if is_func(e, "LET") => reduce_let(model, args, ctx, spills),
        Expr::Call(callee, args) => reduce_call(model, callee, args, ctx, spills),
        _ => Reduced::Value(eval_value(model, e, ctx, spills)),
    }
}

fn reduce_let(model: &SheetModel, args: &[Expr], ctx: &EvalCtx, spills: &SpillState) -> Reduced {
    // name1, value1, …, body — an odd count with Local name slots.
    if args.len() < 3 || args.len().is_multiple_of(2) {
        return Reduced::Value(err(CellError::Value));
    }
    let mut rest: Vec<Expr> = args.to_vec();
    let last = rest.len() - 1;
    let mut i = 0;
    while i < last {
        let name = match &rest[i] {
            Expr::Local(n) => n.to_string(),
            _ => return Reduced::Value(err(CellError::Value)),
        };
        let bound = bind(model, &rest[i + 1], ctx, spills);
        let mut j = i + 2;
        while j <= last {
            if j < last && j % 2 == 0 {
                // A later declaration slot: re-binding the same name shadows
                // it for everything after its own value.
                if matches!(&rest[j], Expr::Local(n) if n.eq_ignore_ascii_case(&name)) {
                    rest[j + 1] = subst(&rest[j + 1], &name, &bound);
                    break;
                }
            } else {
                rest[j] = subst(&rest[j], &name, &bound);
            }
            j += 1;
        }
        i += 2;
    }
    reduce(model, &rest[last], ctx, spills)
}

fn reduce_call(
    model: &SheetModel,
    callee: &Expr,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Reduced {
    let (params, body) = match reduce(model, callee, ctx, spills) {
        Reduced::Lambda(p, b) => (p, b),
        // Calling a non-lambda (or an unbound name).
        Reduced::Value(FnResult::Scalar(CellValue::Error(e))) => {
            return Reduced::Value(err(e));
        }
        Reduced::Value(_) => return Reduced::Value(err(CellError::Value)),
    };
    let bound: Vec<Expr> = args.iter().map(|a| bind(model, a, ctx, spills)).collect();
    apply(model, &params, &body, &bound, ctx, spills)
}

/// Apply a lambda to already-bound argument expressions.
fn apply(
    model: &SheetModel,
    params: &[String],
    body: &Expr,
    bound: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Reduced {
    if params.len() != bound.len() {
        return Reduced::Value(err(CellError::Value));
    }
    let mut b = body.clone();
    for (p, v) in params.iter().zip(bound) {
        b = subst(&b, p, v);
    }
    reduce(model, &b, ctx, spills)
}

/// Apply and ground to a value (a lambda result is `#VALUE!`).
fn apply_value(
    model: &SheetModel,
    params: &[String],
    body: &Expr,
    bound: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
) -> FnResult {
    match apply(model, params, body, bound, ctx, spills) {
        Reduced::Value(v) => v,
        Reduced::Lambda(..) => err(CellError::Value),
    }
}

// ---- binding values ----------------------------------------------------------

/// Whether an expression denotes a reference (so a binding keeps it as one).
fn is_ref_shaped(e: &Expr) -> bool {
    match e {
        Expr::Ref(_) | Expr::Range(_) | Expr::Name(_) | Expr::StructuredRef(_) => true,
        Expr::SpillRef(_) => true,
        Expr::Func(fid, _) => matches!(sheet_core::funcs::meta(*fid).name, "OFFSET" | "INDIRECT"),
        _ => false,
    }
}

/// The reference an expression resolves to, as an expression (`Ref` for a
/// single cell so scalar use reads the value).
fn ref_expr(r: RangeRef) -> Expr {
    let n = r.normalized();
    if n.rows() == 1 && n.cols() == 1 {
        Expr::Ref(n.start)
    } else {
        Expr::Range(n)
    }
}

/// The expression substituted for a bound name (see the module docs).
fn bind(model: &SheetModel, e: &Expr, ctx: &EvalCtx, spills: &SpillState) -> Expr {
    if is_ref_shaped(e) {
        if let Some(r) = eval_as_ref(model, e, ctx, spills) {
            return ref_expr(r);
        }
    }
    match reduce(model, e, ctx, spills) {
        Reduced::Lambda(params, body) => {
            let mut args: Vec<Expr> = params.into_iter().map(|p| Expr::Local(p.into())).collect();
            args.push(body);
            Expr::Func(lambda_fid(), args)
        }
        Reduced::Value(v) => value_expr(v),
    }
}

fn lit(v: &CellValue) -> Expr {
    Expr::Lit(match v {
        CellValue::Number(n) => LitValue::Number(OrderedF64::new(*n)),
        CellValue::Text(t) => LitValue::Text(t.clone()),
        CellValue::Bool(b) => LitValue::Bool(*b),
        CellValue::Error(e) => LitValue::Error(*e),
        // A blank computed value reads as 0 (see the module rulings).
        CellValue::Empty => LitValue::Number(OrderedF64::new(0.0)),
    })
}

/// A value as a literal expression (a block → an array literal).
fn value_expr(v: FnResult) -> Expr {
    match v {
        FnResult::Scalar(v) => lit(&v),
        FnResult::Array(g) => {
            if g.len() == 1 && g[0].len() == 1 {
                lit(&g[0][0])
            } else {
                Expr::Array(g.iter().map(|r| r.iter().map(lit).collect()).collect())
            }
        }
    }
}

/// Evaluate to a value: a reference yields its cells (a block, or the single
/// value), everything else goes through the rich door.
fn eval_value(model: &SheetModel, e: &Expr, ctx: &EvalCtx, spills: &SpillState) -> FnResult {
    if is_ref_shaped(e) {
        if let Some(r) = eval_as_ref(model, e, ctx, spills) {
            let n = r.normalized();
            if n.rows() == 1 && n.cols() == 1 {
                return FnResult::Scalar(argview::cell_value(model, n.start));
            }
            let buf = argview::materialize_range(model, n);
            let view = buf.view();
            let grid = (0..view.rows())
                .map(|rr| (0..view.cols()).map(|cc| view.get(rr, cc)).collect())
                .collect();
            return FnResult::Array(grid);
        }
    }
    eval_expr_rich(model, e, ctx, spills)
}

// ---- substitution ------------------------------------------------------------

/// Replace every free `Local(name)` in `e` by `repl`, honouring shadowing.
fn subst(e: &Expr, name: &str, repl: &Expr) -> Expr {
    let s = |x: &Expr| subst(x, name, repl);
    match e {
        Expr::Local(n) if n.eq_ignore_ascii_case(name) => repl.clone(),
        Expr::Lit(_)
        | Expr::Ref(_)
        | Expr::Range(_)
        | Expr::Name(_)
        | Expr::StructuredRef(_)
        | Expr::Local(_) => e.clone(),
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(s(a))),
        Expr::Binary(op, a, b) => Expr::Binary(*op, Box::new(s(a)), Box::new(s(b))),
        Expr::SpillRef(a) => Expr::SpillRef(Box::new(s(a))),
        Expr::Array(rows) => Expr::Array(rows.iter().map(|r| r.iter().map(s).collect()).collect()),
        Expr::Call(c, args) => Expr::Call(Box::new(s(c)), args.iter().map(s).collect()),
        Expr::Func(fid, args) if is_func(e, "LAMBDA") => {
            let (params, _) = args.split_at(args.len().saturating_sub(1));
            let shadowed = params
                .iter()
                .any(|p| matches!(p, Expr::Local(n) if n.eq_ignore_ascii_case(name)));
            if shadowed {
                e.clone()
            } else {
                let mut out = args.to_vec();
                if let Some(body) = out.last_mut() {
                    *body = s(body);
                }
                Expr::Func(*fid, out)
            }
        }
        Expr::Func(fid, args) if is_func(e, "LET") => {
            let mut out = args.to_vec();
            let last = out.len().saturating_sub(1);
            let mut j = 0;
            while j <= last && !out.is_empty() {
                if j < last && j % 2 == 0 {
                    if matches!(&out[j], Expr::Local(n) if n.eq_ignore_ascii_case(name)) {
                        // Re-bound: substitute into its value, then stop.
                        out[j + 1] = s(&out[j + 1]);
                        break;
                    }
                } else {
                    out[j] = s(&out[j]);
                }
                j += 1;
            }
            Expr::Func(*fid, out)
        }
        Expr::Func(fid, args) => Expr::Func(*fid, args.iter().map(s).collect()),
    }
}

// ---- the helpers ---------------------------------------------------------------

/// The elements of an array argument as substitutable expressions.
struct Elements {
    rows: u32,
    cols: u32,
    /// `Some(range)` when the source is a reference (elements are cell refs).
    source: Option<RangeRef>,
    values: Vec<Vec<CellValue>>,
}

impl Elements {
    fn of(
        model: &SheetModel,
        e: &Expr,
        ctx: &EvalCtx,
        spills: &SpillState,
    ) -> Result<Self, CellError> {
        if is_ref_shaped(e) {
            if let Some(r) = eval_as_ref(model, e, ctx, spills) {
                let n = r.normalized();
                return Ok(Elements {
                    rows: n.rows(),
                    cols: n.cols(),
                    source: Some(n),
                    values: Vec::new(),
                });
            }
        }
        match reduce(model, e, ctx, spills) {
            Reduced::Lambda(..) => Err(CellError::Value),
            Reduced::Value(FnResult::Scalar(CellValue::Error(e))) => Err(e),
            Reduced::Value(FnResult::Scalar(v)) => Ok(Elements {
                rows: 1,
                cols: 1,
                source: None,
                values: vec![vec![v]],
            }),
            Reduced::Value(FnResult::Array(g)) => Ok(Elements {
                rows: g.len() as u32,
                cols: g.iter().map(|r| r.len()).max().unwrap_or(0) as u32,
                source: None,
                values: g,
            }),
        }
    }

    fn cell_ref(src: RangeRef, r: u32, c: u32) -> CellRef {
        CellRef {
            sheet: src.start.sheet,
            row: src.start.row + r,
            col: src.start.col + c,
            row_abs: false,
            col_abs: false,
        }
    }

    /// Element `(r, c)` (out of range → `#N/A`).
    fn at(&self, r: u32, c: u32) -> Expr {
        if r >= self.rows || c >= self.cols {
            return lit(&CellValue::Error(CellError::Na));
        }
        match self.source {
            Some(src) => Expr::Ref(Self::cell_ref(src, r, c)),
            None => lit(self.values[r as usize]
                .get(c as usize)
                .unwrap_or(&CellValue::Empty)),
        }
    }

    /// Row `r` (as a reference or a 1×N literal).
    fn row(&self, r: u32) -> Expr {
        match self.source {
            Some(src) => ref_expr(RangeRef {
                start: Self::cell_ref(src, r, 0),
                end: Self::cell_ref(src, r, self.cols - 1),
            }),
            None => value_expr(FnResult::Array(vec![self.values[r as usize].clone()])),
        }
    }

    /// Column `c` (as a reference or an N×1 literal).
    fn col(&self, c: u32) -> Expr {
        match self.source {
            Some(src) => ref_expr(RangeRef {
                start: Self::cell_ref(src, 0, c),
                end: Self::cell_ref(src, self.rows - 1, c),
            }),
            None => value_expr(FnResult::Array(
                self.values
                    .iter()
                    .map(|row| vec![row.get(c as usize).cloned().unwrap_or(CellValue::Empty)])
                    .collect(),
            )),
        }
    }
}

/// The trailing lambda argument of a helper.
fn lambda_arg(
    model: &SheetModel,
    e: &Expr,
    ctx: &EvalCtx,
    spills: &SpillState,
) -> Result<(Vec<String>, Expr), CellError> {
    match reduce(model, e, ctx, spills) {
        Reduced::Lambda(p, b) => Ok((p, b)),
        Reduced::Value(FnResult::Scalar(CellValue::Error(e))) => Err(e),
        Reduced::Value(_) => Err(CellError::Value),
    }
}

/// `MAP(array1, [array2, …], lambda)`.
fn map(model: &SheetModel, args: &[Expr], ctx: &EvalCtx, spills: &SpillState) -> FnResult {
    let (arrays, lam) = args.split_at(args.len() - 1);
    let (params, body) = match lambda_arg(model, &lam[0], ctx, spills) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let mut els = Vec::with_capacity(arrays.len());
    for a in arrays {
        match Elements::of(model, a, ctx, spills) {
            Ok(x) => els.push(x),
            Err(e) => return err(e),
        }
    }
    let rows = els.iter().map(|x| x.rows).max().unwrap_or(0);
    let cols = els.iter().map(|x| x.cols).max().unwrap_or(0);
    if rows as u64 * cols as u64 > MAX_CELLS {
        return err(CellError::Num);
    }
    let grid = (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| {
                    let bound: Vec<Expr> = els.iter().map(|x| x.at(r, c)).collect();
                    collapse(apply_value(model, &params, &body, &bound, ctx, spills))
                })
                .collect()
        })
        .collect();
    FnResult::Array(grid)
}

/// `REDUCE(initial, array, lambda(acc, value))` / `SCAN(...)`.
fn reduce_scan(
    model: &SheetModel,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
    scan: bool,
) -> FnResult {
    let (params, body) = match lambda_arg(model, &args[2], ctx, spills) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let els = match Elements::of(model, &args[1], ctx, spills) {
        Ok(x) => x,
        Err(e) => return err(e),
    };
    let mut acc = bind(model, &args[0], ctx, spills);
    let mut last: Option<FnResult> = None;
    let mut out: Vec<Vec<CellValue>> = Vec::new();
    for r in 0..els.rows {
        let mut orow = Vec::new();
        for c in 0..els.cols {
            let res = apply_value(
                model,
                &params,
                &body,
                &[acc.clone(), els.at(r, c)],
                ctx,
                spills,
            );
            if scan {
                orow.push(collapse(res.clone()));
            }
            acc = value_expr(res.clone());
            last = Some(res);
        }
        if scan {
            out.push(orow);
        }
    }
    if scan {
        return FnResult::Array(out);
    }
    match last {
        Some(v) => v,
        None => eval_value(model, &acc, ctx, spills),
    }
}

/// `BYROW(array, lambda(row))` (`rows` = true) / `BYCOL(array, lambda(col))`.
fn by_line(
    model: &SheetModel,
    args: &[Expr],
    ctx: &EvalCtx,
    spills: &SpillState,
    rows: bool,
) -> FnResult {
    let (params, body) = match lambda_arg(model, &args[1], ctx, spills) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let els = match Elements::of(model, &args[0], ctx, spills) {
        Ok(x) => x,
        Err(e) => return err(e),
    };
    if els.rows == 0 || els.cols == 0 {
        return err(CellError::Value);
    }
    if rows {
        let grid = (0..els.rows)
            .map(|r| {
                vec![collapse(apply_value(
                    model,
                    &params,
                    &body,
                    &[els.row(r)],
                    ctx,
                    spills,
                ))]
            })
            .collect();
        FnResult::Array(grid)
    } else {
        let row = (0..els.cols)
            .map(|c| {
                collapse(apply_value(
                    model,
                    &params,
                    &body,
                    &[els.col(c)],
                    ctx,
                    spills,
                ))
            })
            .collect();
        FnResult::Array(vec![row])
    }
}

/// `MAKEARRAY(rows, cols, lambda(r, c))`.
fn makearray(model: &SheetModel, args: &[Expr], ctx: &EvalCtx, spills: &SpillState) -> FnResult {
    let dim = |e: &Expr| -> Result<i64, CellError> {
        let v = eval(model, e, ctx, spills);
        if let CellValue::Error(e) = v {
            return Err(e);
        }
        Ok(coerce::to_number(&v)?.trunc() as i64)
    };
    let (rows, cols) = match (dim(&args[0]), dim(&args[1])) {
        (Ok(r), Ok(c)) => (r, c),
        (Err(e), _) | (_, Err(e)) => return err(e),
    };
    if rows < 1 || cols < 1 {
        return err(CellError::Value);
    }
    if rows as u64 * cols as u64 > MAX_CELLS {
        return err(CellError::Num);
    }
    let (params, body) = match lambda_arg(model, &args[2], ctx, spills) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let num = |n: i64| lit(&CellValue::Number(n as f64));
    let grid = (1..=rows)
        .map(|r| {
            (1..=cols)
                .map(|c| {
                    collapse(apply_value(
                        model,
                        &params,
                        &body,
                        &[num(r), num(c)],
                        ctx,
                        spills,
                    ))
                })
                .collect()
        })
        .collect();
    FnResult::Array(grid)
}
