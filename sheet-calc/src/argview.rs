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
//! ## Borrowed, not copied
//!
//! A [`RangeBuf`] does NOT copy the range. It holds a lazy getter that reads
//! the model's sparse grid on demand, and lends it to the kernel through
//! [`sheet_fn::RangeView::from_fn`]. A kernel that reads three cells of a
//! 10 000-cell lookup table reads three cells; nothing is allocated per cell
//! and a blank is never stored. (Before 2026-10, every evaluation copied
//! every cell of every range argument, blanks included — a VLOOKUP over a
//! 1000×2 table copied 2000 cells per evaluation.)
//!
//! ## Used-range cap
//!
//! The view's GEOMETRY is the full requested range (`rows`/`cols` are what
//! the kernel expects — `ROWS`, `INDEX`, `COUNTBLANK` and `MATCH` positions
//! depend on it), but READS are capped at the sheet's populated rows: a read
//! above the first or below the last populated row answers
//! [`CellValue::Empty`] without touching the grid. The row bounds are the
//! sparse map's first and last keys (`O(log n)`, computed once per view), so
//! a whole-column reference costs nothing beyond the populated rows' lookups.
//! Columns are not capped (a column bound needs a full scan of the grid).
//!
//! `materialize_*` keep their names — they are what the evaluator calls — but
//! they build views, and the `cells_materialized` work counter (cells COPIED)
//! stays at zero; the cells a kernel actually reads are counted as
//! `cells_read`.

use sheet_core::{CellRef, CellValue, RangeRef, SheetModel, Worksheet};

/// A lazy getter over relative `(row, col)` coordinates.
type Getter<'m> = Box<dyn Fn(u32, u32) -> CellValue + 'm>;

/// A borrowed range argument: the geometry a [`sheet_fn::RangeView`] needs
/// plus a getter that reads the model on demand. Build with
/// [`materialize_range`], then call [`RangeBuf::view`] to lend a view to a
/// kernel. Borrows the model for `'m`.
pub struct RangeBuf<'m> {
    origin: CellRef,
    rows: u32,
    cols: u32,
    get: Getter<'m>,
}

impl<'m> RangeBuf<'m> {
    /// Lend a [`sheet_fn::RangeView`] over this range. The view borrows the
    /// buffer for as long as the returned value lives.
    pub fn view(&self) -> sheet_fn::RangeView<'_> {
        sheet_fn::RangeView::from_fn(self.origin, self.rows, self.cols, &*self.get)
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

    /// Own an evaluated 2-D block (a nested dynamic-array result or an array
    /// literal used as an argument) so a kernel reads it through the same
    /// [`sheet_fn::RangeView`] door as a sheet range. Ragged rows pad with
    /// `Empty`; `origin` is only an anchor (the block has no sheet address).
    pub fn from_grid(origin: CellRef, grid: Vec<Vec<CellValue>>) -> RangeBuf<'m> {
        let rows = grid.len() as u32;
        let cols = grid.iter().map(|r| r.len()).max().unwrap_or(0) as u32;
        let mut cells = Vec::with_capacity(rows as usize * cols as usize);
        for mut row in grid {
            row.resize(cols as usize, CellValue::Empty);
            cells.extend(row);
        }
        // The block is owned by the getter (it is a computed value, not a
        // sheet range — there is no model to borrow).
        let get: Getter<'m> = Box::new(move |r: u32, c: u32| -> CellValue {
            cells
                .get(r as usize * cols as usize + c as usize)
                .cloned()
                .unwrap_or(CellValue::Empty)
        });
        RangeBuf {
            origin,
            rows,
            cols,
            get,
        }
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

/// The populated row span of a worksheet (`None` when it has no cells): the
/// first and last keys of the row-major sparse map, `O(log n)`.
fn populated_rows(ws: &Worksheet) -> Option<(u32, u32)> {
    let first = ws.cells.keys().next()?.0;
    let last = ws.cells.keys().next_back()?.0;
    Some((first, last))
}

/// Build the lazy getter for the normalized range `n`, optionally masking
/// cells (read as blank) for which `mask` answers `true`.
fn window<'m>(
    model: &'m SheetModel,
    n: RangeRef,
    mask: Option<fn(&SheetModel, CellRef) -> bool>,
) -> RangeBuf<'m> {
    perf_count!(ranges_materialized, 1);
    let (sheet, row0, col0) = (n.start.sheet, n.start.row, n.start.col);
    let ws = model.sheet(sheet);
    // Reads outside the populated rows answer Empty without a lookup.
    let bounds = ws.and_then(populated_rows);
    let get: Getter<'m> = Box::new(move |r: u32, c: u32| -> CellValue {
        let (Some(ws), Some((lo, hi))) = (ws, bounds) else {
            perf_count!(cells_visited, 1);
            return CellValue::Empty;
        };
        perf_count!(cells_visited, 1);
        let (row, col) = (row0.saturating_add(r), col0.saturating_add(c));
        if row < lo || row > hi {
            return CellValue::Empty;
        }
        perf_count!(cells_read, 1);
        let Some(cell) = ws.cell(row, col) else {
            return CellValue::Empty;
        };
        if let Some(mask) = mask {
            let at = CellRef {
                sheet,
                row,
                col,
                row_abs: false,
                col_abs: false,
            };
            if mask(model, at) {
                return CellValue::Empty;
            }
        }
        cell.value.clone()
    });
    RangeBuf {
        origin: n.start,
        rows: n.rows(),
        cols: n.cols(),
        get,
    }
}

/// View a (possibly un-normalized) range as a [`RangeBuf`]. The geometry is
/// the FULL requested range (so `rows`/`cols` match what the kernel expects);
/// cells are read from the model on demand, blanks as [`CellValue::Empty`].
/// The `origin` is the normalized top-left.
pub fn materialize_range(model: &SheetModel, range: RangeRef) -> RangeBuf<'_> {
    window(model, range.normalized(), None)
}

/// View a range like [`materialize_range`], but MASK each cell for which
/// `mask` returns `true` to [`CellValue::Empty`]. Used by SUBTOTAL / AGGREGATE
/// to EXCLUDE cells that are themselves nested SUBTOTAL/AGGREGATE results
/// (ECMA-376 §18.17.7 — a SUBTOTAL never re-aggregates another SUBTOTAL inside
/// its range). A masked cell reads as blank, so every inner aggregate skips it
/// (value aggregates and COUNTA alike — `scan_refs` treats `Empty` as blank).
/// `mask` is called with the model and the cell's absolute [`CellRef`], for
/// populated cells only (a blank reads blank either way); the geometry is
/// still the FULL requested range.
pub fn materialize_range_masked(
    model: &SheetModel,
    range: RangeRef,
    mask: fn(&SheetModel, CellRef) -> bool,
) -> RangeBuf<'_> {
    window(model, range.normalized(), Some(mask))
}

/// View a single cell as a 1×1 [`RangeBuf`] carrying its origin — used for
/// `ref_args` functions (`ROW`/`COLUMN`) handed a bare cell reference: the
/// kernel needs the *reference*, not the value.
pub fn materialize_ref_1x1(model: &SheetModel, cell: CellRef) -> RangeBuf<'_> {
    let n = RangeRef {
        start: cell,
        end: cell,
    };
    // Not normalized: the origin keeps the caller's reference flags.
    window(model, n, None)
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
