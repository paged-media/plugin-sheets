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

//! The argument calling convention (spec §7). A function kernel is the pure
//! signature `fn(&[Arg], &EvalCtx) -> CellValue`: each [`Arg`] is either a
//! single [`CellValue`] scalar or a [`RangeView`] window onto evaluated
//! cells. Kernels never see the dependency graph, the scheduler, or the
//! SDK — `sheet-calc` builds the `&[Arg]` slice, dispatches through
//! [`crate::dispatch`], and stores the returned scalar (spill ranges are a
//! T1 concern handled above this layer).
//!
//! # FREEZE NOTICE — this surface is FROZEN (M0 phase 0, Track FN-CONV)
//!
//! Seven family agents and `sheet-calc` build against [`Arg`],
//! [`RangeView`], and [`crate::ctx::EvalCtx`] *exactly as written here*. Do
//! NOT add/rename/retype anything in this module or `ctx.rs` as a drive-by
//! edit — changes go through the orchestrator as a versioned amendment
//! (repo constitution, CLAUDE.md §"Interface freeze"). A range arrives
//! either fully materialized ([`RangeView::from_slice`]) or as a lazy
//! getter ([`RangeView::from_fn`]); kernels read it through the same
//! [`RangeView::get`] / [`RangeView::iter`] regardless of backing.
//!
//! **Amendment (round 2, 2026-10-05) — live rows.** A view may carry the
//! relative row span outside of which every cell is guaranteed blank
//! ([`RangeView::with_live_rows`], set by `sheet-calc` from the sheet's
//! populated rows). Kernels that skip blanks scan [`RangeView::iter_live`]
//! instead of [`RangeView::iter`], so a whole-column `SUM(A:A)` costs the
//! populated rows, not 1 048 576. Geometry (`rows`/`cols`) is unchanged.

use sheet_core::{CellRef, CellValue};

/// One evaluated argument to a function kernel (spec §7). `Scalar` is a
/// single resolved value; `Range` is a rectangular window of evaluated
/// cells. Implicit intersection (range → scalar in a scalar context) is the
/// caller's concern (`sheet-calc`); a kernel sees whatever the convention
/// handed it and decides per its registry `range_aware` flag.
pub enum Arg<'a> {
    Scalar(CellValue),
    Range(RangeView<'a>),
}

/// How a [`RangeView`]'s cells are sourced. `Slice` is a fully materialized
/// row-major buffer; `Fn` is a lazy getter (relative coords) so a kernel
/// can scan a huge range without the caller exploding it cell-by-cell.
enum Backing<'a> {
    Slice(&'a [CellValue]),
    Fn(&'a dyn Fn(u32, u32) -> CellValue),
}

/// A read-only rectangular window onto evaluated cells (spec §7). Indexed by
/// **relative** `(row, col)` from `[0, rows) × [0, cols)`; out-of-bounds
/// reads yield [`CellValue::Empty`]. `origin` is the absolute top-left
/// [`CellRef`] (so range-aware kernels like `ROW`/`COLUMN`/`OFFSET` can
/// recover real addresses). A `RangeView` borrows its backing for `'a` — it
/// never owns or copies the cells.
pub struct RangeView<'a> {
    rows: u32,
    cols: u32,
    origin: CellRef,
    backing: Backing<'a>,
    /// Relative rows `[lo, hi)` that may hold a value; every cell outside is
    /// blank. `None` = unknown (all rows may).
    live: Option<(u32, u32)>,
}

impl<'a> RangeView<'a> {
    /// Build a view over a materialized row-major buffer. `cells.len()` MUST
    /// equal `rows * cols` (debug-asserted); the element at relative
    /// `(r, c)` is `cells[r * cols + c]`.
    pub fn from_slice(origin: CellRef, rows: u32, cols: u32, cells: &'a [CellValue]) -> Self {
        debug_assert_eq!(
            cells.len() as u64,
            rows as u64 * cols as u64,
            "RangeView::from_slice: cells.len() must equal rows*cols"
        );
        RangeView {
            rows,
            cols,
            origin,
            backing: Backing::Slice(cells),
            live: None,
        }
    }

    /// Build a lazy view: `get(r, c)` is called on demand with **relative**
    /// coords. The closure is responsible for resolving the cell; this view
    /// still clamps out-of-bounds relative reads to [`CellValue::Empty`]
    /// before calling it.
    pub fn from_fn(
        origin: CellRef,
        rows: u32,
        cols: u32,
        get: &'a dyn Fn(u32, u32) -> CellValue,
    ) -> Self {
        RangeView {
            rows,
            cols,
            origin,
            backing: Backing::Fn(get),
            live: None,
        }
    }

    /// Declare that every cell outside relative rows `[lo, hi)` is blank
    /// (clamped to the view; `lo >= hi` = no row holds a value). A promise
    /// the caller must keep: kernels skip those rows unread.
    pub fn with_live_rows(mut self, lo: u32, hi: u32) -> Self {
        let hi = hi.min(self.rows);
        self.live = Some((lo.min(hi), hi));
        self
    }

    /// The relative rows that may hold a value (every row when unknown).
    pub fn live_rows(&self) -> std::ops::Range<u32> {
        match self.live {
            Some((lo, hi)) => lo..hi,
            None => 0..self.rows,
        }
    }

    /// How many cells lie OUTSIDE [`RangeView::live_rows`] — all blank.
    pub fn blank_cells_outside_live(&self) -> u64 {
        let live = self.live_rows();
        u64::from(self.rows - (live.end - live.start)) * u64::from(self.cols)
    }

    /// Row-major iterator over the cells of [`RangeView::live_rows`] only —
    /// [`RangeView::iter`] minus cells known to be blank. For kernels that
    /// skip blanks (SUM, COUNT, MAX, …), where the result is identical.
    pub fn iter_live(&self) -> impl Iterator<Item = CellValue> + '_ {
        let cols = self.cols;
        self.live_rows()
            .flat_map(move |r| (0..cols).map(move |c| self.get(r, c)))
    }

    /// Row count of the window.
    #[inline]
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Column count of the window.
    #[inline]
    pub fn cols(&self) -> u32 {
        self.cols
    }

    /// The absolute top-left address of the window.
    #[inline]
    pub fn origin(&self) -> CellRef {
        self.origin
    }

    /// Read the cell at **relative** `(r, c)`. Out-of-bounds (≥ `rows` or
    /// ≥ `cols`) yields [`CellValue::Empty`] — never panics, so kernels can
    /// scan a bounding box without edge-case branches.
    pub fn get(&self, r: u32, c: u32) -> CellValue {
        if r >= self.rows || c >= self.cols {
            return CellValue::Empty;
        }
        match self.backing {
            Backing::Slice(cells) => {
                let idx = r as usize * self.cols as usize + c as usize;
                cells.get(idx).cloned().unwrap_or(CellValue::Empty)
            }
            Backing::Fn(get) => get(r, c),
        }
    }

    /// Row-major iterator over every cell in the window (`rows * cols`
    /// items). Reads through [`RangeView::get`], so it works identically for
    /// slice- and fn-backed views.
    pub fn iter(&self) -> impl Iterator<Item = CellValue> + '_ {
        let cols = self.cols;
        (0..self.rows).flat_map(move |r| (0..cols).map(move |c| self.get(r, c)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_str::CompactString;

    fn cr(row: u32, col: u32) -> CellRef {
        CellRef {
            sheet: 0,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    #[test]
    fn from_slice_row_major_and_oob() {
        let cells = [
            CellValue::Number(1.0),
            CellValue::Number(2.0),
            CellValue::Number(3.0),
            CellValue::Number(4.0),
        ];
        let v = RangeView::from_slice(cr(2, 5), 2, 2, &cells);
        assert_eq!(v.rows(), 2);
        assert_eq!(v.cols(), 2);
        assert_eq!(v.origin(), cr(2, 5));
        assert_eq!(v.get(0, 0), CellValue::Number(1.0));
        assert_eq!(v.get(0, 1), CellValue::Number(2.0));
        assert_eq!(v.get(1, 0), CellValue::Number(3.0));
        assert_eq!(v.get(1, 1), CellValue::Number(4.0));
        // OOB on both axes -> Empty.
        assert_eq!(v.get(2, 0), CellValue::Empty);
        assert_eq!(v.get(0, 2), CellValue::Empty);
    }

    #[test]
    fn iter_is_row_major() {
        let cells = [
            CellValue::from("a"),
            CellValue::from("b"),
            CellValue::from("c"),
            CellValue::from("d"),
            CellValue::from("e"),
            CellValue::from("f"),
        ];
        let v = RangeView::from_slice(cr(0, 0), 2, 3, &cells);
        let got: Vec<CellValue> = v.iter().collect();
        assert_eq!(
            got,
            vec![
                CellValue::Text(CompactString::new("a")),
                CellValue::Text(CompactString::new("b")),
                CellValue::Text(CompactString::new("c")),
                CellValue::Text(CompactString::new("d")),
                CellValue::Text(CompactString::new("e")),
                CellValue::Text(CompactString::new("f")),
            ]
        );
    }

    #[test]
    fn live_rows_bound_the_scan_not_the_geometry() {
        let get = |r: u32, _c: u32| {
            if (3..5).contains(&r) {
                CellValue::Number(f64::from(r))
            } else {
                CellValue::Empty
            }
        };
        let v = RangeView::from_fn(cr(0, 0), 1_000, 2, &get).with_live_rows(3, 5);
        assert_eq!(v.rows(), 1_000);
        assert_eq!(v.live_rows(), 3..5);
        assert_eq!(v.iter_live().count(), 4);
        assert_eq!(v.blank_cells_outside_live(), 1_996);
        // Without the promise every row is live.
        let w = RangeView::from_fn(cr(0, 0), 7, 2, &get);
        assert_eq!(w.live_rows(), 0..7);
        assert_eq!(w.blank_cells_outside_live(), 0);
        // An empty span and a span past the end clamp.
        assert_eq!(
            RangeView::from_fn(cr(0, 0), 7, 1, &get)
                .with_live_rows(9, 12)
                .live_rows(),
            7..7
        );
    }

    #[test]
    fn from_fn_lazy_backing() {
        // A 3x3 view whose cell value is r*10 + c.
        let get = |r: u32, c: u32| CellValue::Number((r * 10 + c) as f64);
        let v = RangeView::from_fn(cr(7, 7), 3, 3, &get);
        assert_eq!(v.get(0, 0), CellValue::Number(0.0));
        assert_eq!(v.get(2, 1), CellValue::Number(21.0));
        // OOB short-circuits before calling the closure.
        assert_eq!(v.get(3, 0), CellValue::Empty);
        let sum: f64 = v
            .iter()
            .map(|c| match c {
                CellValue::Number(n) => n,
                _ => 0.0,
            })
            .sum();
        // (0+1+2)+(10+11+12)+(20+21+22) = 99
        assert_eq!(sum, 99.0);
    }
}
