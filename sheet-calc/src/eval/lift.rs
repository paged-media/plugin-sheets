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

//! Element-wise ARRAY evaluation — Excel's array semantics for operators and
//! scalar-parameter functions fed a range or an array: `A1:A3*2`,
//! `IF(A1:A3>1,A1:A3)`, `ISNUMBER(A1:A3)` each yield a block, computed per
//! element with Excel's broadcasting (a 1-row or 1-column operand repeats
//! along that axis; a position past a smaller operand's edge is `#N/A`).
//!
//! Where it applies:
//! - an ARGUMENT of a function (`SUM(A1:A3*2)`, `MIN(IF(…))`,
//!   `SUMPRODUCT((A1:A3>1)*1)`), which is how dynamic-array Excel reads every
//!   such formula and how a legacy CSE formula was always read;
//! - the ROOT of a legacy array formula (`<f t="array" ref=…>`, the engine's
//!   CSE anchors).
//!
//! A formula's root outside a CSE anchor keeps the scalar ruling (a range in a
//! value position is `#VALUE!`).

use sheet_core::ast::{BinOp, Expr};
use sheet_core::{CellError, CellValue, RangeRef, SheetModel};
use sheet_fn::{Arg, EvalCtx, FnResult};

use super::{
    binary_values, build_args, deref_name, eval, eval_array_literal, eval_func_rich,
    name_range_target, plan_args, resolve_structured_ref, spill_rect_to_range, spill_ref_range,
    unary_value, with_name_depth, ArgPlan,
};
use crate::argview;
use crate::spill::SpillState;

/// The largest block an element-wise evaluation builds (cells).
const MAX_CELLS: u64 = 1 << 22;

/// Whether `fid` takes scalars only, so an array argument lifts it element
/// by element (a range-aware function consumes ranges itself).
fn liftable(fid: sheet_core::ast::FuncId) -> bool {
    let m = sheet_core::funcs::meta(fid);
    !m.range_aware && !m.ref_args && !m.special_form && !m.returns_array
}

/// The SCALAR parameters (0-based) of range-aware functions whose other
/// parameters take ranges: an array there lifts the call over that
/// parameter only (`MATCH($M20:$M119, K20:K119, 0)` is a block of positions).
fn scalar_params(fid: sheet_core::ast::FuncId) -> &'static [usize] {
    match sheet_core::funcs::meta(fid).name {
        "MATCH" | "XMATCH" => &[0, 2],
        "INDEX" => &[1, 2, 3],
        "VLOOKUP" | "HLOOKUP" => &[0, 2, 3],
        "XLOOKUP" => &[0, 4, 5],
        "LOOKUP" => &[0],
        "COUNTIF" => &[1],
        "SUMIF" | "AVERAGEIF" => &[1],
        "LARGE" | "SMALL" | "PERCENTILE" | "PERCENTILE.INC" | "PERCENTILE.EXC" | "QUARTILE"
        | "QUARTILE.INC" | "QUARTILE.EXC" => &[1],
        "RANK" | "RANK.EQ" | "RANK.AVG" => &[0, 2],
        _ => &[],
    }
}

fn value_op(op: BinOp) -> bool {
    !matches!(op, BinOp::Range | BinOp::Union | BinOp::Isect)
}

/// Whether an operand is array-shaped (a multi-cell reference or an array).
fn array_operand(model: &SheetModel, e: &Expr, depth: u32) -> bool {
    match e {
        Expr::Range(r) => {
            let n = r.normalized();
            n.rows() > 1 || n.cols() > 1
        }
        Expr::Array(_) | Expr::StructuredRef(_) | Expr::SpillRef(_) => true,
        Expr::Name(nid) => match name_range_target(model, *nid) {
            Some(r) => r.rows() > 1 || r.cols() > 1,
            None => {
                depth < crate::names::MAX_NAME_DEPTH
                    && crate::names::name_expr(model, *nid).is_some_and(|inner| {
                        array_operand(model, inner, depth + 1)
                            || contains_array(model, inner, depth + 1)
                    })
            }
        },
        _ => false,
    }
}

/// Whether an operator / scalar-function tree holds an array operand that
/// is not consumed by a range-aware call inside it.
fn contains_array(model: &SheetModel, e: &Expr, depth: u32) -> bool {
    match e {
        Expr::Unary(_, a) => array_operand(model, a, depth) || contains_array(model, a, depth),
        Expr::Binary(op, a, b) if value_op(*op) => [a, b]
            .iter()
            .any(|x| array_operand(model, x, depth) || contains_array(model, x, depth)),
        Expr::Func(fid, args) if liftable(*fid) => args
            .iter()
            .any(|x| array_operand(model, x, depth) || contains_array(model, x, depth)),
        Expr::Func(fid, args) => scalar_params(*fid).iter().any(|&i| {
            args.get(i)
                .is_some_and(|x| array_operand(model, x, depth) || contains_array(model, x, depth))
        }),
        _ => false,
    }
}

/// Whether evaluating `e` as an argument should go through [`eval_array`]: an
/// operator or scalar-function tree over an array operand.
pub(super) fn needs_lift(model: &SheetModel, e: &Expr) -> bool {
    contains_array(model, e, 0)
}

/// The values of a reference as a block (`Scalar` for a single cell).
fn range_block(model: &SheetModel, r: RangeRef) -> FnResult {
    let n = r.normalized();
    if n.rows() == 1 && n.cols() == 1 {
        return FnResult::Scalar(argview::cell_value(model, n.start));
    }
    if n.rows() as u64 * n.cols() as u64 > MAX_CELLS {
        return FnResult::Scalar(CellValue::Error(CellError::Num));
    }
    let grid = (0..n.rows())
        .map(|dr| {
            (0..n.cols())
                .map(|dc| {
                    let mut c = n.start;
                    c.row += dr;
                    c.col += dc;
                    argview::cell_value(model, c)
                })
                .collect()
        })
        .collect();
    FnResult::Array(grid)
}

/// Evaluate `e` in ARRAY context (operators and scalar functions over a
/// range or array yield a block).
pub fn eval_array(model: &SheetModel, e: &Expr, ctx: &EvalCtx, spills: &SpillState) -> FnResult {
    match e {
        Expr::Range(r) => range_block(model, *r),
        Expr::StructuredRef(s) => match resolve_structured_ref(model, s, ctx) {
            Ok(r) => range_block(model, r),
            Err(err) => FnResult::Scalar(CellValue::Error(err)),
        },
        Expr::SpillRef(inner) => match spill_ref_range(spills, inner) {
            Some(rect) => range_block(model, spill_rect_to_range(rect)),
            None => FnResult::Scalar(eval(model, e, ctx, spills)),
        },
        Expr::Name(nid) => match name_range_target(model, *nid) {
            Some(r) => range_block(model, r),
            None => {
                let inner = deref_name(model, e, ctx.current);
                if matches!(&*inner, Expr::Name(_)) {
                    return FnResult::Scalar(eval(model, e, ctx, spills));
                }
                with_name_depth(FnResult::Scalar(CellValue::Error(CellError::Name)), || {
                    eval_array(model, &inner, ctx, spills)
                })
            }
        },
        Expr::Array(rows) => eval_array_literal(model, rows, ctx, spills),
        Expr::Unary(op, a) => map1(eval_array(model, a, ctx, spills), |v| unary_value(*op, v)),
        Expr::Binary(op, a, b) if value_op(*op) => {
            let l = eval_array(model, a, ctx, spills);
            let r = eval_array(model, b, ctx, spills);
            map_n(&[l, r], |vals| {
                binary_values(*op, vals[0].clone(), vals[1].clone())
            })
        }
        Expr::Func(fid, args) if sheet_core::funcs::meta(*fid).returns_array => {
            eval_func_rich(model, *fid, args, ctx, spills)
        }
        Expr::Func(fid, args)
            if liftable(*fid)
                && args
                    .iter()
                    .any(|a| array_operand(model, a, 0) || contains_array(model, a, 0)) =>
        {
            let vals: Vec<FnResult> = args
                .iter()
                .map(|a| eval_array(model, a, ctx, spills))
                .collect();
            map_n(&vals, |scalars| {
                let built: Vec<Arg<'_>> = scalars.iter().cloned().map(Arg::Scalar).collect();
                sheet_fn::dispatch(*fid, &built, ctx)
            })
        }
        Expr::Func(fid, args)
            if scalar_params(*fid).iter().any(|&i| {
                args.get(i)
                    .is_some_and(|a| array_operand(model, a, 0) || contains_array(model, a, 0))
            }) =>
        {
            // Lift over the scalar parameters only; the range parameters are
            // materialized once, as for a plain call.
            let lifted: Vec<usize> = scalar_params(*fid)
                .iter()
                .copied()
                .filter(|&i| i < args.len())
                .collect();
            let mut plain: Vec<Expr> = args.to_vec();
            for &i in &lifted {
                plain[i] = Expr::Lit(sheet_core::ast::LitValue::Number(
                    sheet_core::ast::OrderedF64::new(0.0),
                ));
            }
            let (bufs, mut plans) = plan_args(model, *fid, &plain, ctx, spills);
            let vals: Vec<FnResult> = lifted
                .iter()
                .map(|&i| eval_array(model, &args[i], ctx, spills))
                .collect();
            map_n(&vals, |scalars| {
                for (k, &i) in lifted.iter().enumerate() {
                    plans[i] = ArgPlan::Scalar(scalars[k].clone());
                }
                let built = build_args(&bufs, &plans);
                sheet_fn::dispatch(*fid, &built, ctx)
            })
        }
        _ => FnResult::Scalar(eval(model, e, ctx, spills)),
    }
}

fn map1(v: FnResult, f: impl Fn(CellValue) -> CellValue) -> FnResult {
    match v {
        FnResult::Scalar(x) => FnResult::Scalar(f(x)),
        FnResult::Array(g) => FnResult::Array(
            g.into_iter()
                .map(|row| row.into_iter().map(&f).collect())
                .collect(),
        ),
    }
}

/// Apply `f` element-wise over operands with Excel's broadcasting.
fn map_n(vals: &[FnResult], mut f: impl FnMut(&[CellValue]) -> CellValue) -> FnResult {
    let dims = |v: &FnResult| match v {
        FnResult::Scalar(_) => (1usize, 1usize),
        FnResult::Array(g) => (g.len(), g.first().map_or(0, Vec::len)),
    };
    if vals.iter().all(|v| matches!(v, FnResult::Scalar(_))) {
        let s: Vec<CellValue> = vals
            .iter()
            .map(|v| match v {
                FnResult::Scalar(x) => x.clone(),
                FnResult::Array(_) => unreachable!(),
            })
            .collect();
        return FnResult::Scalar(f(&s));
    }
    let rows = vals.iter().map(|v| dims(v).0).max().unwrap_or(1);
    let cols = vals.iter().map(|v| dims(v).1).max().unwrap_or(1);
    if rows as u64 * cols as u64 > MAX_CELLS {
        return FnResult::Scalar(CellValue::Error(CellError::Num));
    }
    let at = |v: &FnResult, i: usize, j: usize| -> CellValue {
        match v {
            FnResult::Scalar(x) => x.clone(),
            FnResult::Array(g) => {
                let (r, c) = (g.len(), g.first().map_or(0, Vec::len));
                let ii = if r == 1 { 0 } else { i };
                let jj = if c == 1 { 0 } else { j };
                g.get(ii)
                    .and_then(|row| row.get(jj))
                    .cloned()
                    .unwrap_or(CellValue::Error(CellError::Na))
            }
        }
    };
    let grid = (0..rows)
        .map(|i| {
            (0..cols)
                .map(|j| {
                    let s: Vec<CellValue> = vals.iter().map(|v| at(v, i, j)).collect();
                    f(&s)
                })
                .collect()
        })
        .collect();
    FnResult::Array(grid)
}
