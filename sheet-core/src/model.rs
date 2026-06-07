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

//! The workbook document model (spec §5.1). `SheetModel` owns the sheets
//! plus the workbook-level interned tables (formulas, strings, styles,
//! names), calc settings, and the opaque XLSX preservation payload. The
//! cell grid is sparse — a `BTreeMap` keyed by `(row, col)` so iteration
//! is deterministic (row-major) and empty cells cost nothing.

use std::collections::BTreeMap;

use crate::ast::Formula;
use crate::calc_settings::CalcSettings;
use crate::cell::{Cell, FormulaId};
use crate::intern::Interner;
use crate::names::NameTable;
use crate::preserved::PreservedParts;
use crate::refs::{RangeRef, SheetId};
use crate::style::StyleTable;
use compact_str::CompactString;

/// The whole workbook.
#[derive(Default)]
pub struct SheetModel {
    pub sheets: Vec<Worksheet>,
    pub names: NameTable,
    pub styles: StyleTable,
    pub formulas: Interner<Formula>,
    pub strings: Interner<CompactString>,
    pub calc: CalcSettings,
    pub preserved: PreservedParts,
}

// `StyleTable::default()` already seeds the defaults via `new()`, and
// `CalcSettings` derives Excel defaults, so the derived `SheetModel` Default
// is correct. The contract calls for a manual impl; we make it explicit so
// the seeding intent is documented and cannot silently regress.
// (Kept as a thin wrapper rather than deriving.)
impl SheetModel {
    /// A fresh, empty workbook (no sheets). Style/calc tables are seeded.
    pub fn new() -> Self {
        SheetModel {
            sheets: Vec::new(),
            names: NameTable::default(),
            styles: StyleTable::new(),
            formulas: Interner::new(),
            strings: Interner::new(),
            calc: CalcSettings::default(),
            preserved: PreservedParts::default(),
        }
    }

    /// Append a worksheet, returning its id (its index).
    pub fn add_sheet(&mut self, name: impl Into<CompactString>) -> SheetId {
        let id = self.sheets.len() as SheetId;
        self.sheets.push(Worksheet {
            name: name.into(),
            ..Default::default()
        });
        id
    }

    pub fn sheet(&self, id: SheetId) -> Option<&Worksheet> {
        self.sheets.get(id as usize)
    }

    pub fn sheet_mut(&mut self, id: SheetId) -> Option<&mut Worksheet> {
        self.sheets.get_mut(id as usize)
    }

    /// Resolve a sheet name to its id. Case-insensitive (Excel sheet-name
    /// semantics); first match wins.
    pub fn sheet_id(&self, name: &str) -> Option<SheetId> {
        self.sheets
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
            .map(|i| i as SheetId)
    }

    /// Intern a formula, returning its (deduped) id.
    pub fn intern_formula(&mut self, formula: Formula) -> FormulaId {
        FormulaId(self.formulas.intern(formula))
    }

    pub fn formula(&self, id: FormulaId) -> Option<&Formula> {
        self.formulas.get(id.0)
    }
}

/// The bounding box of a worksheet's populated cells, as 0-based
/// `(row0, col0, row1, col1)` inclusive corners.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UsedRange {
    pub row0: u32,
    pub col0: u32,
    pub row1: u32,
    pub col1: u32,
}

impl UsedRange {
    /// View this box as a sheet-0 `RangeRef` (relative refs).
    pub fn as_range(&self, sheet: SheetId) -> RangeRef {
        use crate::refs::CellRef;
        let mk = |row, col| CellRef {
            sheet,
            row,
            col,
            row_abs: false,
            col_abs: false,
        };
        RangeRef {
            start: mk(self.row0, self.col0),
            end: mk(self.row1, self.col1),
        }
    }
}

/// A single worksheet: a sparse cell grid plus column/row sizing and merges.
#[derive(Debug, Default)]
pub struct Worksheet {
    pub name: CompactString,
    /// Sparse cells keyed by `(row, col)` (row-major iteration order).
    pub cells: BTreeMap<(u32, u32), Cell>,
    /// Column widths in characters (XLSX convention).
    pub col_widths: BTreeMap<u32, f64>,
    /// Row heights in points.
    pub row_heights: BTreeMap<u32, f64>,
    pub merges: Vec<RangeRef>,
}

impl Worksheet {
    pub fn cell(&self, row: u32, col: u32) -> Option<&Cell> {
        self.cells.get(&(row, col))
    }

    pub fn set_cell(&mut self, row: u32, col: u32, cell: Cell) {
        self.cells.insert((row, col), cell);
    }

    pub fn remove_cell(&mut self, row: u32, col: u32) -> Option<Cell> {
        self.cells.remove(&(row, col))
    }

    /// Bounding box of all populated cells, or `None` when empty.
    pub fn used_range(&self) -> Option<UsedRange> {
        let mut it = self.cells.keys();
        let &(r0, c0) = it.next()?;
        let (mut row0, mut col0, mut row1, mut col1) = (r0, c0, r0, c0);
        for &(r, c) in self.cells.keys() {
            row0 = row0.min(r);
            row1 = row1.max(r);
            col0 = col0.min(c);
            col1 = col1.max(c);
        }
        Some(UsedRange {
            row0,
            col0,
            row1,
            col1,
        })
    }

    /// Iterate populated cells as `((row, col), &Cell)` in row-major order.
    pub fn iter_cells(&self) -> impl Iterator<Item = (&(u32, u32), &Cell)> {
        self.cells.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, LitValue, OrderedF64};
    use crate::value::CellValue;

    #[test]
    fn add_sheet_and_sheet_id_case_insensitive() {
        let mut m = SheetModel::new();
        let a = m.add_sheet("Sheet1");
        let b = m.add_sheet("Data");
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(m.sheet_id("sheet1"), Some(0));
        assert_eq!(m.sheet_id("DATA"), Some(1));
        assert_eq!(m.sheet_id("missing"), None);
        assert!(m.sheet(a).is_some());
        assert!(m.sheet_mut(b).is_some());
    }

    #[test]
    fn default_seeds_style_table() {
        // Derived/explicit Default must still seed StyleId(0) == "General".
        let m = SheetModel::default();
        assert_eq!(m.styles.num_fmt_of(crate::cell::StyleId(0)), "General");
    }

    #[test]
    fn used_range_over_sparse_sheet() {
        let mut ws = Worksheet::default();
        assert_eq!(ws.used_range(), None);
        ws.set_cell(2, 5, Cell::default());
        ws.set_cell(7, 1, Cell::default());
        ws.set_cell(4, 9, Cell::default());
        let ur = ws.used_range().unwrap();
        assert_eq!(
            ur,
            UsedRange {
                row0: 2,
                col0: 1,
                row1: 7,
                col1: 9
            }
        );
        assert_eq!(ws.iter_cells().count(), 3);
        assert!(ws.cell(2, 5).is_some());
        ws.remove_cell(2, 5);
        assert!(ws.cell(2, 5).is_none());
    }

    #[test]
    fn intern_formula_dedups_structurally_equal() {
        let mut m = SheetModel::new();
        let mk = || Formula {
            root: Expr::Binary(
                crate::ast::BinOp::Add,
                Box::new(Expr::Lit(LitValue::Number(OrderedF64::new(1.0)))),
                Box::new(Expr::Lit(LitValue::Number(OrderedF64::new(2.0)))),
            ),
        };
        let a = m.intern_formula(mk());
        let b = m.intern_formula(mk());
        assert_eq!(a, b);
        // A structurally different formula gets a fresh id (exercises
        // OrderedF64 Hash/Eq distinguishing 2.0 from 3.0).
        let c = m.intern_formula(Formula {
            root: Expr::Lit(LitValue::Number(OrderedF64::new(3.0))),
        });
        assert_ne!(a, c);
        assert!(matches!(
            m.formula(a),
            Some(Formula {
                root: Expr::Binary(..)
            })
        ));
    }

    #[test]
    fn set_cell_with_value() {
        let mut ws = Worksheet::default();
        let c = Cell {
            value: CellValue::Number(3.5),
            ..Default::default()
        };
        ws.set_cell(0, 0, c);
        assert_eq!(ws.cell(0, 0).unwrap().value, CellValue::Number(3.5));
    }
}
