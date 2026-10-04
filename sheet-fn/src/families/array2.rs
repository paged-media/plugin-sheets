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

//! Wave-7 array-shaping family (spec §6.4): `VSTACK`, `HSTACK`, `TAKE`,
//! `DROP`, `CHOOSECOLS`, `CHOOSEROWS`, `TOCOL`, `TOROW`, `MMULT`.
//! Provenance: the public Microsoft documentation for each function. Every
//! kernel is `fn(&[Arg], &EvalCtx) -> FnResult` (registry
//! `returns_array: true`); a scalar argument is a 1×1 array.
//!
//! ## Rulings
//!
//! - **Errors are carried element-wise.** These are structural transforms: an
//!   error cell in a source lands in the output at its new position (Excel),
//!   it does not collapse the whole result.
//! - **Padding.** `VSTACK`/`HSTACK` pad a short source with `#N/A` (Excel).
//! - **The `#CALC!` grounding** (see `array`): an empty result (`TAKE(x,0)`,
//!   dropping everything, `TOCOL` ignoring every cell) is `#VALUE!`.
//! - **Out-of-range indices.** `CHOOSECOLS`/`CHOOSEROWS` with `0` or `|n|`
//!   beyond the extent are `#VALUE!`; a negative index counts from the end.
//! - **MMULT** needs `cols(a) == rows(b)` and every cell numeric (an empty or
//!   text cell is `#VALUE!`; an error cell propagates).

use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;
use crate::result::FnResult;

type Grid = Vec<Vec<CellValue>>;

#[inline]
fn err(e: CellError) -> FnResult {
    FnResult::Scalar(CellValue::Error(e))
}

fn grid(arg: &Arg) -> Grid {
    match arg {
        Arg::Scalar(v) => vec![vec![v.clone()]],
        Arg::Range(r) => (0..r.rows())
            .map(|rr| (0..r.cols()).map(|cc| r.get(rr, cc)).collect())
            .collect(),
    }
}

fn scalar_num(arg: &Arg) -> Result<f64, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_number(v),
        Arg::Range(r) => coerce::to_number(&r.get(0, 0)),
    }
}

fn width(g: &Grid) -> usize {
    g.iter().map(|r| r.len()).max().unwrap_or(0)
}

fn transpose(g: &Grid) -> Grid {
    let w = width(g);
    (0..w)
        .map(|c| {
            g.iter()
                .map(|r| r.get(c).cloned().unwrap_or(CellValue::Empty))
                .collect()
        })
        .collect()
}

fn non_empty(g: Grid) -> FnResult {
    if g.is_empty() || g.iter().all(|r| r.is_empty()) {
        err(CellError::Value)
    } else {
        FnResult::Array(g)
    }
}

// ---- VSTACK / HSTACK -------------------------------------------------------

/// `VSTACK(array1, [array2], …)` (registry `sheet.fn.array.vstack`).
pub fn vstack(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let grids: Vec<Grid> = args.iter().map(grid).collect();
    let w = grids.iter().map(width).max().unwrap_or(0);
    let mut out = Grid::new();
    for g in grids {
        for mut row in g {
            row.resize(w, CellValue::Error(CellError::Na));
            out.push(row);
        }
    }
    non_empty(out)
}

/// `HSTACK(array1, [array2], …)` (registry `sheet.fn.array.hstack`).
pub fn hstack(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let grids: Vec<Grid> = args.iter().map(grid).collect();
    let h = grids.iter().map(|g| g.len()).max().unwrap_or(0);
    let mut out: Grid = vec![Vec::new(); h];
    for g in grids {
        let w = width(&g);
        for (r, orow) in out.iter_mut().enumerate() {
            match g.get(r) {
                Some(row) => {
                    let mut row = row.clone();
                    row.resize(w, CellValue::Error(CellError::Na));
                    orow.extend(row);
                }
                None => orow.extend(std::iter::repeat_n(CellValue::Error(CellError::Na), w)),
            }
        }
    }
    non_empty(out)
}

// ---- TAKE / DROP -----------------------------------------------------------

/// Keep `n` leading (n > 0) or trailing (n < 0) items of `v`.
fn take_n<T: Clone>(v: &[T], n: i64) -> Vec<T> {
    let len = v.len() as i64;
    if n >= 0 {
        v[..n.min(len) as usize].to_vec()
    } else {
        v[(len + n).max(0) as usize..].to_vec()
    }
}

/// Drop `n` leading (n > 0) or trailing (n < 0) items of `v`.
fn drop_n<T: Clone>(v: &[T], n: i64) -> Vec<T> {
    let len = v.len() as i64;
    if n >= 0 {
        v[n.min(len) as usize..].to_vec()
    } else {
        v[..(len + n).max(0) as usize].to_vec()
    }
}

fn take_drop(args: &[Arg], take: bool) -> FnResult {
    let g = grid(&args[0]);
    let rows = match scalar_num(&args[1]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return err(e),
    };
    let cols = match args.get(2) {
        Some(a) => match scalar_num(a) {
            Ok(n) => Some(n.trunc() as i64),
            Err(e) => return err(e),
        },
        None => None,
    };
    if take && (rows == 0 || cols == Some(0)) {
        return err(CellError::Value);
    }
    let op = |v: &[CellValue], n: i64| if take { take_n(v, n) } else { drop_n(v, n) };
    let picked: Grid = if take {
        take_n(&g, rows)
    } else {
        drop_n(&g, rows)
    };
    let out: Grid = match cols {
        Some(c) => picked.iter().map(|r| op(r, c)).collect(),
        None => picked,
    };
    non_empty(out)
}

/// `TAKE(array, rows, [columns])` (registry `sheet.fn.array.take`).
pub fn take(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    take_drop(args, true)
}

/// `DROP(array, rows, [columns])` (registry `sheet.fn.array.drop`).
pub fn drop(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    take_drop(args, false)
}

// ---- CHOOSECOLS / CHOOSEROWS -----------------------------------------------

/// Resolve the 1-based (negative = from the end) indices in `args[1..]`
/// against an extent of `len`. Each index argument may itself be an array.
fn indices(args: &[Arg], len: usize) -> Result<Vec<usize>, CellError> {
    let mut out = Vec::new();
    for a in &args[1..] {
        for row in grid(a) {
            for v in row {
                let n = coerce::to_number(&v)?.trunc() as i64;
                let idx = if n > 0 && n as usize <= len {
                    n as usize - 1
                } else if n < 0 && (-n) as usize <= len {
                    len - (-n) as usize
                } else {
                    return Err(CellError::Value);
                };
                out.push(idx);
            }
        }
    }
    Ok(out)
}

/// `CHOOSECOLS(array, col_num1, [col_num2], …)` (registry
/// `sheet.fn.array.choosecols`).
pub fn choosecols(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let g = grid(&args[0]);
    let idx = match indices(args, width(&g)) {
        Ok(i) => i,
        Err(e) => return err(e),
    };
    let out: Grid = g
        .iter()
        .map(|r| {
            idx.iter()
                .map(|&i| r.get(i).cloned().unwrap_or(CellValue::Empty))
                .collect()
        })
        .collect();
    non_empty(out)
}

/// `CHOOSEROWS(array, row_num1, [row_num2], …)` (registry
/// `sheet.fn.array.chooserows`).
pub fn chooserows(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let g = grid(&args[0]);
    let idx = match indices(args, g.len()) {
        Ok(i) => i,
        Err(e) => return err(e),
    };
    non_empty(idx.iter().map(|&i| g[i].clone()).collect())
}

// ---- TOCOL / TOROW ---------------------------------------------------------

/// Flatten for `TOCOL`/`TOROW`: `ignore` 0 keep all, 1 skip blanks, 2 skip
/// errors, 3 skip both; `by_col` scans column-major.
fn flatten(args: &[Arg]) -> Result<Vec<CellValue>, CellError> {
    let mut g = grid(&args[0]);
    let ignore = match args.get(1) {
        Some(a) => scalar_num(a)?.trunc() as i64,
        None => 0,
    };
    if !(0..=3).contains(&ignore) {
        return Err(CellError::Value);
    }
    let by_col = match args.get(2) {
        Some(Arg::Scalar(v)) => coerce::to_bool(v)?,
        Some(Arg::Range(r)) => coerce::to_bool(&r.get(0, 0))?,
        None => false,
    };
    if by_col {
        g = transpose(&g);
    }
    let out: Vec<CellValue> = g
        .into_iter()
        .flatten()
        .filter(|v| match v {
            CellValue::Empty => ignore & 1 == 0,
            CellValue::Error(_) => ignore & 2 == 0,
            _ => true,
        })
        .collect();
    if out.is_empty() {
        return Err(CellError::Value);
    }
    Ok(out)
}

/// `TOCOL(array, [ignore], [scan_by_column])` (registry `sheet.fn.array.tocol`).
pub fn tocol(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    match flatten(args) {
        Ok(v) => FnResult::Array(v.into_iter().map(|c| vec![c]).collect()),
        Err(e) => err(e),
    }
}

/// `TOROW(array, [ignore], [scan_by_column])` (registry `sheet.fn.array.torow`).
pub fn torow(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    match flatten(args) {
        Ok(v) => FnResult::Array(vec![v]),
        Err(e) => err(e),
    }
}

// ---- MMULT -----------------------------------------------------------------

fn numeric_grid(g: &Grid) -> Result<Vec<Vec<f64>>, CellError> {
    g.iter()
        .map(|r| {
            r.iter()
                .map(|v| match v {
                    CellValue::Number(n) => Ok(*n),
                    CellValue::Error(e) => Err(*e),
                    _ => Err(CellError::Value),
                })
                .collect()
        })
        .collect()
}

/// `MMULT(array1, array2)` (registry `sheet.fn.array.mmult`).
pub fn mmult(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let a = match numeric_grid(&grid(&args[0])) {
        Ok(g) => g,
        Err(e) => return err(e),
    };
    let b = match numeric_grid(&grid(&args[1])) {
        Ok(g) => g,
        Err(e) => return err(e),
    };
    let inner = a.first().map_or(0, |r| r.len());
    if inner == 0 || inner != b.len() {
        return err(CellError::Value);
    }
    let bw = b.first().map_or(0, |r| r.len());
    let out: Grid = a
        .iter()
        .map(|row| {
            (0..bw)
                .map(|j| {
                    let s: f64 = (0..inner).map(|k| row[k] * b[k][j]).sum();
                    if s.is_finite() {
                        CellValue::Number(s)
                    } else {
                        CellValue::Error(CellError::Num)
                    }
                })
                .collect()
        })
        .collect();
    FnResult::Array(out)
}
