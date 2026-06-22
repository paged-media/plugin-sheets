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

//! Range-argument views (spec §6.2/§7). When the evaluator materializes a
//! function argument that is a range (an [`Expr::Range`], or a bare
//! [`Expr::Ref`] for a `ref_args` function), it must hand the kernel a
//! [`sheet_fn::RangeView`] that reads the **already-computed** cell values
//! out of the model. `topo` order guarantees those values are fresh.
//!
//! A `RangeView` borrows its backing for `'a` (it never owns the cells), so a
//! `from_fn` view would need a closure that outlives the dispatch call but
//! borrows the model immutably. Inside [`crate::eval`] we already hold a
//! `&SheetModel`, so the simplest correct construction is to **materialize**
//! the range into an owned row-major `Vec<CellValue>` and lend a
//! [`RangeView::from_slice`] over it. This module is the one place that
//! materialization lives, with the `origin` set to the range's top-left
//! [`CellRef`] so `ROW`/`COLUMN`/`OFFSET`-style kernels recover real
//! addresses.
//!
//! T0 ruling — a `RangeView` is capped at the sheet's populated bounding box
//! intersected with the requested range, so a "whole-column" style reference
//! does not allocate a million empty cells; out-of-box reads still resolve to
//! [`CellValue::Empty`] through the `RangeView`'s own out-of-bounds clamp.

use sheet_core::{CellRef, CellValue, RangeRef, SheetModel};

/// An owned, row-major snapshot of a range's current cell values plus the
/// geometry a [`sheet_fn::RangeView`] needs. Build with [`materialize_range`],
/// then call [`RangeBuf::view`] to lend a borrowing view to a kernel.
pub struct RangeBuf {
    origin: CellRef,
    rows: u32,
    cols: u32,
    cells: Vec<CellValue>,
}

impl RangeBuf {
    /// Lend a [`sheet_fn::RangeView`] over this buffer. The view borrows the
    /// buffer for as long as the returned value lives.
    pub fn view(&self) -> sheet_fn::RangeView<'_> {
        sheet_fn::RangeView::from_slice(self.origin, self.rows, self.cols, &self.cells)
    }

    /// Geometry accessors (used by `ROW`/`COLUMN` materialization in eval).
    pub fn rows(&self) -> u32 {
        self.rows
    }
    pub fn cols(&self) -> u32 {
        self.cols
    }
    pub fn origin(&self) -> CellRef {
        self.origin
    }
}

/// Read a single cell's current value out of the model (`Empty` if blank or
/// the sheet does not exist). The evaluator relies on `topo` order so this is
/// the *fresh* value, never a stale one.
pub fn cell_value(model: &SheetModel, cell: CellRef) -> CellValue {
    model
        .sheet(cell.sheet)
        .and_then(|ws| ws.cell(cell.row, cell.col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

/// Materialize a (possibly un-normalized) range into a [`RangeBuf`]. The
/// geometry is the FULL requested range (so `rows`/`cols` match what the
/// kernel expects), but only populated cells are read from the model — blanks
/// stay [`CellValue::Empty`]. The `origin` is the normalized top-left.
pub fn materialize_range(model: &SheetModel, range: RangeRef) -> RangeBuf {
    let n = range.normalized();
    let rows = n.rows();
    let cols = n.cols();
    let mut cells = Vec::with_capacity(rows.saturating_mul(cols) as usize);
    for r in n.start.row..=n.end.row {
        for c in n.start.col..=n.end.col {
            cells.push(cell_value(
                model,
                CellRef {
                    sheet: n.start.sheet,
                    row: r,
                    col: c,
                    row_abs: false,
                    col_abs: false,
                },
            ));
        }
    }
    RangeBuf {
        origin: n.start,
        rows,
        cols,
        cells,
    }
}

/// Materialize a range like [`materialize_range`], but MASK each cell for which
/// `mask` returns `true` to [`CellValue::Empty`]. Used by SUBTOTAL / AGGREGATE
/// to EXCLUDE cells that are themselves nested SUBTOTAL/AGGREGATE results
/// (ECMA-376 §18.17.7 — a SUBTOTAL never re-aggregates another SUBTOTAL inside
/// its range). A masked cell reads as blank, so every inner aggregate skips it
/// (value aggregates and COUNTA alike — `scan_refs` treats `Empty` as blank).
/// `mask` is called with the cell's absolute [`CellRef`]; the geometry is still
/// the FULL requested range (the kernel's row/col expectations are unchanged).
pub fn materialize_range_masked(
    model: &SheetModel,
    range: RangeRef,
    mask: &mut dyn FnMut(CellRef) -> bool,
) -> RangeBuf {
    let n = range.normalized();
    let rows = n.rows();
    let cols = n.cols();
    let mut cells = Vec::with_capacity(rows.saturating_mul(cols) as usize);
    for r in n.start.row..=n.end.row {
        for c in n.start.col..=n.end.col {
            let cell = CellRef {
                sheet: n.start.sheet,
                row: r,
                col: c,
                row_abs: false,
                col_abs: false,
            };
            cells.push(if mask(cell) {
                CellValue::Empty
            } else {
                cell_value(model, cell)
            });
        }
    }
    RangeBuf {
        origin: n.start,
        rows,
        cols,
        cells,
    }
}

/// Materialize a single cell as a 1×1 [`RangeBuf`] carrying its origin — used
/// for `ref_args` functions (`ROW`/`COLUMN`) handed a bare cell reference: the
/// kernel needs the *reference*, not the value.
pub fn materialize_ref_1x1(model: &SheetModel, cell: CellRef) -> RangeBuf {
    RangeBuf {
        origin: cell,
        rows: 1,
        cols: 1,
        cells: vec![cell_value(model, cell)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::Cell;

    fn cr(sheet: u16, row: u32, col: u32) -> CellRef {
        CellRef {
            sheet,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn model_with(cells: &[(u32, u32, CellValue)]) -> SheetModel {
        let mut m = SheetModel::new();
        m.add_sheet("Sheet1");
        let ws = m.sheet_mut(0).unwrap();
        for (r, c, v) in cells {
            ws.set_cell(
                *r,
                *c,
                Cell {
                    value: v.clone(),
                    ..Default::default()
                },
            );
        }
        m
    }

    #[test]
    fn materialize_row_major_with_blanks() {
        let m = model_with(&[
            (0, 0, CellValue::Number(1.0)),
            (0, 1, CellValue::Number(2.0)),
            // (1,0) blank
            (1, 1, CellValue::Number(4.0)),
        ]);
        let range = RangeRef {
            start: cr(0, 0, 0),
            end: cr(0, 1, 1),
        };
        let buf = materialize_range(&m, range);
        let v = buf.view();
        assert_eq!(v.rows(), 2);
        assert_eq!(v.cols(), 2);
        assert_eq!(v.get(0, 0), CellValue::Number(1.0));
        assert_eq!(v.get(0, 1), CellValue::Number(2.0));
        assert_eq!(v.get(1, 0), CellValue::Empty);
        assert_eq!(v.get(1, 1), CellValue::Number(4.0));
        assert_eq!(v.origin(), cr(0, 0, 0));
    }

    #[test]
    fn ref_1x1_carries_origin() {
        let m = model_with(&[(3, 4, CellValue::Number(9.0))]);
        let buf = materialize_ref_1x1(&m, cr(0, 3, 4));
        assert_eq!(buf.origin(), cr(0, 3, 4));
        assert_eq!(buf.view().get(0, 0), CellValue::Number(9.0));
    }

    #[test]
    fn cell_value_of_missing_is_empty() {
        let m = model_with(&[]);
        assert_eq!(cell_value(&m, cr(0, 0, 0)), CellValue::Empty);
        // Non-existent sheet -> Empty, never a panic.
        assert_eq!(cell_value(&m, cr(9, 0, 0)), CellValue::Empty);
    }
}
