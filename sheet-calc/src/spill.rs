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

//! Dynamic-array spill bookkeeping (spec §6.4, M1 spill track). When a formula
//! cell evaluates (through [`sheet_fn::dispatch_rich`]) to a
//! [`sheet_fn::FnResult::Array`], the engine *materializes* that 2-D block onto
//! the sheet: the **anchor** cell (the formula's own cell) holds the top-left
//! value and OWNS a rectangular *spill region*; the other cells of the region
//! are engine-owned *spilled* values (plain `CellValue`s with no formula). This
//! module owns the ledger that records that ownership, plus the pure geometry
//! helpers (rectangle from a result, collision test). The wiring that reads/
//! writes the model lives in [`crate::Engine`].
//!
//! ## T1 scope (documented ruling — `sheet.calc.spill.materialize`)
//!
//! A formula spills iff its ROOT is a `returns_array` function call or an array
//! literal ([`Expr::Array`]); every other formula stays on the existing scalar
//! dispatch path (its result, even if it were an array, is taken as its
//! top-left scalar — but only a `returns_array` root can produce an array
//! through `dispatch_rich`). Legacy CSE array formulas parse to
//! [`Expr::Array`] and therefore evaluate through this SAME machinery
//! (`sheet.calc.spill.cse-parse`).
//!
//! ## Collision → `#SPILL!` (`sheet.calc.spill.collision`)
//!
//! Before materializing, the target rectangle (minus the anchor) must be free:
//! every cell in it is either blank OR already owned by THIS anchor (a re-spill
//! of an unchanged footprint). A cell holding a user value/formula the anchor
//! does not own is a *blocker* → the anchor stores [`CellError::Spill`]
//! (`#SPILL!`) and claims NO region. Removing the blocker and recalculating
//! re-materializes the spill.
//!
//! ## Recalc (`sheet.calc.spill.blocked-recalc`)
//!
//! On every recompute of an anchor the engine first CLEARS the anchor's prior
//! region (so a shrinking array does not leave stale spilled cells), then
//! re-evaluates and re-materializes. A write to a precedent of the anchor
//! dirties the anchor (the normal dependency path), which triggers the reflow.

use rustc_hash::{FxHashMap, FxHashSet};
use sheet_core::CellRef;

/// The rectangular footprint a spill anchor owns, in absolute 0-based
/// coordinates on the anchor's sheet. The anchor sits at `(row0, col0)`; the
/// region is the inclusive box `[row0..=row1] × [col0..=col1]`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SpillRect {
    pub sheet: sheet_core::SheetId,
    pub row0: u32,
    pub col0: u32,
    pub row1: u32,
    pub col1: u32,
}

impl SpillRect {
    /// The rectangle a `rows × cols` array anchored at `anchor` would occupy.
    /// `rows`/`cols` are clamped to ≥ 1 (a 0-sized array never reaches here —
    /// the kernel grounds it to an error first).
    pub fn for_array(anchor: CellRef, rows: u32, cols: u32) -> SpillRect {
        let rows = rows.max(1);
        let cols = cols.max(1);
        SpillRect {
            sheet: anchor.sheet,
            row0: anchor.row,
            col0: anchor.col,
            row1: anchor.row.saturating_add(rows - 1),
            col1: anchor.col.saturating_add(cols - 1),
        }
    }

    /// True if `(sheet, row, col)` falls inside this region.
    pub fn contains(&self, sheet: sheet_core::SheetId, row: u32, col: u32) -> bool {
        sheet == self.sheet
            && row >= self.row0
            && row <= self.row1
            && col >= self.col0
            && col <= self.col1
    }

    /// Iterate every absolute [`CellRef`] in the region, row-major.
    pub fn cells(&self) -> impl Iterator<Item = CellRef> + '_ {
        (self.row0..=self.row1).flat_map(move |r| {
            (self.col0..=self.col1).map(move |c| CellRef {
                sheet: self.sheet,
                row: r,
                col: c,
                row_abs: false,
                col_abs: false,
            })
        })
    }

    /// The top-left anchor cell of this region.
    pub fn anchor(&self) -> CellRef {
        CellRef {
            sheet: self.sheet,
            row: self.row0,
            col: self.col0,
            row_abs: false,
            col_abs: false,
        }
    }

    /// Whether the region is a single cell (a 1×1 "spill" — the array result
    /// degenerated to one value; it claims no real footprint beyond the
    /// anchor).
    pub fn is_single(&self) -> bool {
        self.row0 == self.row1 && self.col0 == self.col1
    }
}

/// The spill ledger: anchor → its owned region, plus a reverse index from
/// every owned (non-anchor) cell back to its anchor, so the evaluator can
/// answer "who owns this cell?" in O(1) and a write can detect a collision.
#[derive(Default)]
pub struct SpillState {
    /// Anchor cell → the region it currently owns.
    anchors: FxHashMap<CellRef, SpillRect>,
    /// Any cell INSIDE a region (including the anchor) → the owning anchor.
    owner_of: FxHashMap<CellRef, CellRef>,
    /// Anchors that wanted to spill but hit a collision (`#SPILL!`) and claimed
    /// NO region. Tracked so removing the blocker can re-trigger them: on any
    /// cell write/clear the engine re-dirties these so a freed rectangle reflows
    /// (`sheet.calc.spill.collision`).
    blocked: FxHashSet<CellRef>,
}

impl SpillState {
    pub fn new() -> Self {
        SpillState::default()
    }

    /// The region owned by `anchor`, if it is a live spill anchor.
    pub fn region_of(&self, anchor: CellRef) -> Option<SpillRect> {
        self.anchors.get(&strip(anchor)).copied()
    }

    /// The anchor that owns `cell` (the cell itself if it IS an anchor), if any.
    pub fn owner_of(&self, cell: CellRef) -> Option<CellRef> {
        self.owner_of.get(&strip(cell)).copied()
    }

    /// True if `cell` is a spill cell that is NOT its region's anchor (an
    /// engine-owned interior cell — a user must not be able to overwrite it
    /// piecemeal; the whole region reflows from the anchor).
    pub fn is_spilled_non_anchor(&self, cell: CellRef) -> bool {
        match self.owner_of(cell) {
            Some(anchor) => strip(anchor) != strip(cell),
            None => false,
        }
    }

    /// Record a freshly materialized region for `anchor`. The caller has
    /// already cleared any prior region and verified the rectangle is free.
    pub fn insert(&mut self, region: SpillRect) {
        let anchor = strip(region.anchor());
        self.anchors.insert(anchor, region);
        for c in region.cells() {
            self.owner_of.insert(strip(c), anchor);
        }
    }

    /// Drop `anchor`'s region from the ledger, returning it so the caller can
    /// clear the corresponding model cells. The reverse index entries are
    /// removed too.
    pub fn remove(&mut self, anchor: CellRef) -> Option<SpillRect> {
        let anchor = strip(anchor);
        let region = self.anchors.remove(&anchor)?;
        for c in region.cells() {
            // Only drop reverse entries that still point at THIS anchor (a new
            // anchor may have claimed an overlapping cell in between — it won't
            // here, since regions never overlap, but stay defensive).
            if self.owner_of.get(&strip(c)) == Some(&anchor) {
                self.owner_of.remove(&strip(c));
            }
        }
        Some(region)
    }

    /// Forget every record (used when the graph is rebuilt wholesale, e.g. a
    /// structural edit — spills re-materialize on the following recalc).
    pub fn clear(&mut self) {
        self.anchors.clear();
        self.owner_of.clear();
        self.blocked.clear();
    }

    /// All live anchors, sorted (deterministic iteration for cleanup passes).
    pub fn anchors_sorted(&self) -> Vec<CellRef> {
        let mut v: Vec<CellRef> = self.anchors.keys().copied().collect();
        v.sort();
        v
    }

    /// Record that `anchor` hit a `#SPILL!` collision (claimed no region). On
    /// the next cell edit the engine re-dirties it so a freed rectangle reflows.
    pub fn mark_blocked(&mut self, anchor: CellRef) {
        self.blocked.insert(strip(anchor));
    }

    /// Forget that `anchor` was blocked (it spilled successfully, or is no longer
    /// a spilling formula).
    pub fn unmark_blocked(&mut self, anchor: CellRef) {
        self.blocked.remove(&strip(anchor));
    }

    /// The blocked anchors, sorted (deterministic). The engine re-dirties these
    /// on a cell edit so a removed blocker lets the spill reflow.
    pub fn blocked_sorted(&self) -> Vec<CellRef> {
        let mut v: Vec<CellRef> = self.blocked.iter().copied().collect();
        v.sort();
        v
    }
}

/// Canonicalize a `CellRef` for ledger keys (strip the `$` absolute flags so a
/// cell keys identically however it was written) — mirrors `graph::strip`.
fn strip(c: CellRef) -> CellRef {
    CellRef {
        sheet: c.sheet,
        row: c.row,
        col: c.col,
        row_abs: false,
        col_abs: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rect_geometry() {
        let rect = SpillRect::for_array(cr(2, 3), 3, 2);
        assert_eq!(rect.row0, 2);
        assert_eq!(rect.col0, 3);
        assert_eq!(rect.row1, 4);
        assert_eq!(rect.col1, 4);
        assert!(rect.contains(0, 2, 3));
        assert!(rect.contains(0, 4, 4));
        assert!(!rect.contains(0, 5, 4));
        assert!(!rect.contains(0, 2, 5));
        assert_eq!(rect.anchor(), cr(2, 3));
        assert!(!rect.is_single());
        let cells: Vec<CellRef> = rect.cells().collect();
        assert_eq!(cells.len(), 6);
        assert_eq!(cells[0], cr(2, 3));
        assert_eq!(cells[5], cr(4, 4));
    }

    #[test]
    fn single_cell_rect() {
        let rect = SpillRect::for_array(cr(0, 0), 1, 1);
        assert!(rect.is_single());
        assert_eq!(rect.cells().count(), 1);
    }

    #[test]
    fn ledger_insert_and_lookup() {
        let mut s = SpillState::new();
        let rect = SpillRect::for_array(cr(0, 0), 3, 1);
        s.insert(rect);
        assert_eq!(s.region_of(cr(0, 0)), Some(rect));
        assert_eq!(s.owner_of(cr(1, 0)), Some(cr(0, 0)));
        assert_eq!(s.owner_of(cr(2, 0)), Some(cr(0, 0)));
        assert_eq!(s.owner_of(cr(3, 0)), None);
        // The anchor owns itself but is NOT a "spilled non-anchor".
        assert!(!s.is_spilled_non_anchor(cr(0, 0)));
        assert!(s.is_spilled_non_anchor(cr(1, 0)));
        assert!(!s.is_spilled_non_anchor(cr(9, 9)));
    }

    #[test]
    fn ledger_remove_clears_reverse_index() {
        let mut s = SpillState::new();
        let rect = SpillRect::for_array(cr(0, 0), 2, 1);
        s.insert(rect);
        let removed = s.remove(cr(0, 0));
        assert_eq!(removed, Some(rect));
        assert_eq!(s.region_of(cr(0, 0)), None);
        assert_eq!(s.owner_of(cr(1, 0)), None);
    }

    #[test]
    fn ledger_keys_ignore_absolute_flags() {
        let mut s = SpillState::new();
        s.insert(SpillRect::for_array(cr(0, 0), 2, 1));
        let abs = CellRef {
            row_abs: true,
            col_abs: true,
            ..cr(1, 0)
        };
        assert_eq!(s.owner_of(abs), Some(cr(0, 0)));
    }
}
