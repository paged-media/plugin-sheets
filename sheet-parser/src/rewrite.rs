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

//! Structural rewrite (spec §6.1/§6.3): adjust the references in a formula
//! when rows/columns are inserted or deleted. Rulings:
//!
//! - **Absolute (`$`) flags do NOT exempt a ref from a structural shift.**
//!   In Excel `$` affects copy/fill, not insert/delete — `$A$1` still moves
//!   down when a row is inserted above it. (Copy/fill rewrite, where `$`
//!   *does* matter, is T1 — see [`rewrite_fill`], a documented stub.)
//! - **Out-of-bounds shift → `#REF!`.** A ref pushed past `MAX_ROW`/`MAX_COL`
//!   by an insert collapses to `Expr::Lit(Error(Ref))`.
//! - **A ref inside a deleted span → `#REF!`.** A range fully inside the span
//!   collapses; a range partially overlapping the span clips to the survivor.
//! - Only refs on the SAME `sheet` as the edit move.

use sheet_core::ast::{Expr, Formula, LitValue};
use sheet_core::{CellError, CellRef, RangeRef, SheetId, MAX_COL, MAX_ROW};

/// A structural edit. `at` is the 0-based first affected row/col; `n` the
/// count. `sheet` scopes the edit — refs on other sheets are untouched.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    InsertRows { sheet: SheetId, at: u32, n: u32 },
    DeleteRows { sheet: SheetId, at: u32, n: u32 },
    InsertCols { sheet: SheetId, at: u32, n: u32 },
    DeleteCols { sheet: SheetId, at: u32, n: u32 },
}

/// Which axis an edit operates on.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Axis {
    Row,
    Col,
}

/// Insert vs delete.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Kind {
    Insert,
    Delete,
}

struct Plan {
    sheet: SheetId,
    axis: Axis,
    kind: Kind,
    at: u32,
    n: u32,
}

impl Edit {
    fn plan(&self) -> Plan {
        match *self {
            Edit::InsertRows { sheet, at, n } => Plan {
                sheet,
                axis: Axis::Row,
                kind: Kind::Insert,
                at,
                n,
            },
            Edit::DeleteRows { sheet, at, n } => Plan {
                sheet,
                axis: Axis::Row,
                kind: Kind::Delete,
                at,
                n,
            },
            Edit::InsertCols { sheet, at, n } => Plan {
                sheet,
                axis: Axis::Col,
                kind: Kind::Insert,
                at,
                n,
            },
            Edit::DeleteCols { sheet, at, n } => Plan {
                sheet,
                axis: Axis::Col,
                kind: Kind::Delete,
                at,
                n,
            },
        }
    }
}

/// Rewrite every reference in `f` for the structural `edit`.
pub fn rewrite(f: &Formula, edit: &Edit) -> Formula {
    let plan = edit.plan();
    Formula {
        root: rewrite_expr(&f.root, &plan),
    }
}

/// Copy/fill rewrite (T1 stub, spec §6.1). Unlike insert/delete, copy/fill
/// honours `$` flags: relative coordinates shift by the paste delta, absolute
/// ones do not. Signature reserved; not implemented in T0.
#[allow(dead_code)]
pub(crate) fn rewrite_fill(_f: &Formula, _drow: i64, _dcol: i64) -> Formula {
    unimplemented!("copy/fill rewrite is T1 (spec §6.1)")
}

fn rewrite_expr(e: &Expr, plan: &Plan) -> Expr {
    match e {
        Expr::Lit(_) | Expr::Name(_) => e.clone(),
        Expr::Ref(r) => match shift_cell(*r, plan) {
            Some(c) => Expr::Ref(c),
            None => ref_error(),
        },
        Expr::Range(r) => match shift_range(*r, plan) {
            Some(rr) => Expr::Range(rr),
            None => ref_error(),
        },
        Expr::Unary(op, inner) => Expr::Unary(*op, Box::new(rewrite_expr(inner, plan))),
        Expr::Binary(op, a, b) => Expr::Binary(
            *op,
            Box::new(rewrite_expr(a, plan)),
            Box::new(rewrite_expr(b, plan)),
        ),
        Expr::Func(fid, args) => {
            Expr::Func(*fid, args.iter().map(|a| rewrite_expr(a, plan)).collect())
        }
        Expr::Array(rows) => Expr::Array(
            rows.iter()
                .map(|row| row.iter().map(|el| rewrite_expr(el, plan)).collect())
                .collect(),
        ),
        // Structured refs are NAME-anchored (they address a table by name and
        // a column by label, not by A1 geometry), so a row/col insert/delete
        // does not shift them — the table model moves with the edit and the
        // ref re-resolves through the (unchanged) name. Pass through verbatim.
        Expr::StructuredRef(_) => e.clone(),
        // A spill ref's geometry is its anchor's; rewrite the inner anchor and
        // keep the `#` (spill) wrapper.
        Expr::SpillRef(inner) => Expr::SpillRef(Box::new(rewrite_expr(inner, plan))),
    }
}

fn ref_error() -> Expr {
    Expr::Lit(LitValue::Error(CellError::Ref))
}

/// The coordinate of `cell` on `plan.axis`, plus the axis maximum.
fn coord(cell: &CellRef, axis: Axis) -> (u32, u32) {
    match axis {
        Axis::Row => (cell.row, MAX_ROW),
        Axis::Col => (cell.col, MAX_COL),
    }
}

/// Shift a single cell. Returns `None` (→ `#REF!`) if the cell is deleted or
/// pushed out of bounds. Cells on a different sheet are unchanged.
fn shift_cell(cell: CellRef, plan: &Plan) -> Option<CellRef> {
    if cell.sheet != plan.sheet {
        return Some(cell);
    }
    let (c, max) = coord(&cell, plan.axis);
    let nc = shift_coord(c, plan, max)?;
    Some(set_coord(cell, plan.axis, nc))
}

/// Shift a single coordinate (a cell's row or col). `None` = the cell is
/// inside a deleted span, or an insert pushed it past `max`.
fn shift_coord(c: u32, plan: &Plan, max: u32) -> Option<u32> {
    match plan.kind {
        Kind::Insert => {
            if c >= plan.at {
                let nc = c as u64 + plan.n as u64;
                if nc > max as u64 {
                    None // pushed off the grid
                } else {
                    Some(nc as u32)
                }
            } else {
                Some(c)
            }
        }
        Kind::Delete => {
            let end = plan.at as u64 + plan.n as u64; // exclusive
            if (c as u64) < plan.at as u64 {
                Some(c) // before the span: unchanged
            } else if (c as u64) < end {
                None // inside the span: deleted
            } else {
                Some((c as u64 - plan.n as u64) as u32) // after: shift back
            }
        }
    }
}

/// Shift a range. Endpoints are normalized on `plan.axis` first; a range
/// fully inside a deleted span → `None`; a partial overlap clips to the
/// surviving sub-interval; an insert that pushes the end off-grid → `None`.
fn shift_range(r: RangeRef, plan: &Plan) -> Option<RangeRef> {
    if r.start.sheet != plan.sheet {
        return Some(r);
    }
    let n = r.normalized();
    let (s, max) = coord(&n.start, plan.axis);
    let (e, _) = coord(&n.end, plan.axis);

    let (ns, ne) = match plan.kind {
        Kind::Insert => {
            // Both endpoints shift if at/after `at`; an insert never deletes.
            let ns = shift_coord(s, plan, max)?;
            let ne = shift_coord(e, plan, max)?;
            (ns, ne)
        }
        Kind::Delete => {
            let span_end = plan.at as u64 + plan.n as u64; // exclusive
            let s64 = s as u64;
            let e64 = e as u64;
            // Fully inside the deleted span → whole range gone.
            if s64 >= plan.at as u64 && e64 < span_end {
                return None;
            }
            // New start: if before the span, unchanged; if inside, clip to
            // the span's left edge (`at`, which after deletion is the first
            // surviving row); if after, shift back.
            let ns = if s64 < plan.at as u64 {
                s
            } else if s64 < span_end {
                plan.at // clip up to the boundary
            } else {
                (s64 - plan.n as u64) as u32
            };
            // New end: if before the span, unchanged; if inside, clip to the
            // last row before the span (`at - 1`, guaranteed >= ns here); if
            // after, shift back.
            let ne = if e64 < plan.at as u64 {
                e
            } else if e64 < span_end {
                plan.at.saturating_sub(1)
            } else {
                (e64 - plan.n as u64) as u32
            };
            (ns, ne)
        }
    };

    let start = set_coord(n.start, plan.axis, ns);
    let end = set_coord(n.end, plan.axis, ne);
    Some(RangeRef { start, end })
}

/// Return `cell` with its `axis` coordinate replaced.
fn set_coord(mut cell: CellRef, axis: Axis, v: u32) -> CellRef {
    match axis {
        Axis::Row => cell.row = v,
        Axis::Col => cell.col = v,
    }
    cell
}
