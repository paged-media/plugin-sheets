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

//! Structural-rewrite conformance (spec §6.1/§6.3). The registry row
//! `sheet.calc.rewrite.structural` points here at `sheet_calc_rewrite_structural`.
//! Exercises [`sheet_calc::Engine::apply_edit`]: cells physically shift (incl.
//! `col_widths`/`row_heights`/`merges`), formulas rewrite (a deleted-span ref
//! becomes `#REF!`), and dependents recalculate correctly afterward.

use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellError, CellRef, CellValue, RangeRef, SheetModel};
use sheet_parser::Edit;

fn engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    Engine::new(m, EngineConfig::default())
}

fn cr(row: u32, col: u32) -> CellRef {
    CellRef {
        sheet: 0,
        row,
        col,
        row_abs: false,
        col_abs: false,
    }
}

fn val(e: &Engine, row: u32, col: u32) -> CellValue {
    e.model()
        .sheet(0)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

fn num(n: f64) -> CellValue {
    CellValue::Number(n)
}

// =================================================================
// sheet.calc.rewrite.structural
// =================================================================

#[test]
fn sheet_calc_rewrite_structural() {
    // ---- insert rows: cells shift down, formulas track ----
    let mut e = engine();
    e.enter(0, 0, 0, "10").unwrap(); // A1 = 10
    e.enter(0, 1, 0, "20").unwrap(); // A2 = 20
    e.enter(0, 2, 0, "=A1+A2").unwrap(); // A3 = A1+A2 = 30
    assert_eq!(val(&e, 2, 0), num(30.0));

    // Insert 1 row at row 0: everything moves down one. The formula (now A4)
    // must still read A2+A3 and compute 30.
    e.apply_edit(&Edit::InsertRows {
        sheet: 0,
        at: 0,
        n: 1,
    });
    assert_eq!(val(&e, 1, 0), num(10.0), "old A1 now at A2");
    assert_eq!(val(&e, 2, 0), num(20.0), "old A2 now at A3");
    assert_eq!(val(&e, 3, 0), num(30.0), "formula moved to A4, still 30");
    // The original A3 slot is now empty.
    assert_eq!(val(&e, 2, 0), num(20.0));

    // A fresh write that the moved formula now depends on recalcs it.
    e.enter(0, 1, 0, "100").unwrap(); // new A2 (old A1)
    assert_eq!(val(&e, 3, 0), num(120.0), "formula recalcs after edit");

    // ---- delete a row inside a referenced span -> #REF! ----
    let mut e2 = engine();
    e2.enter(0, 0, 0, "1").unwrap(); // A1
    e2.enter(0, 1, 0, "2").unwrap(); // A2
    e2.enter(0, 2, 0, "=A2").unwrap(); // A3 references A2
    assert_eq!(val(&e2, 2, 0), num(2.0));
    // Delete row 1 (A2): A3's reference is to a deleted cell -> #REF!.
    e2.apply_edit(&Edit::DeleteRows {
        sheet: 0,
        at: 1,
        n: 1,
    });
    // A3 moved up to A2 and now evaluates to #REF!.
    assert_eq!(
        val(&e2, 1, 0),
        CellValue::Error(CellError::Ref),
        "deleted-span ref becomes #REF!"
    );

    // ---- insert columns shifts col-keyed formulas and dependents ----
    let mut e3 = engine();
    e3.enter(0, 0, 0, "5").unwrap(); // A1
    e3.enter(0, 0, 1, "=A1*2").unwrap(); // B1 = A1*2 = 10
    assert_eq!(val(&e3, 0, 1), num(10.0));
    e3.apply_edit(&Edit::InsertCols {
        sheet: 0,
        at: 0,
        n: 2,
    });
    // A1 -> C1 ; B1 -> D1, still reads C1.
    assert_eq!(val(&e3, 0, 2), num(5.0), "old A1 now at C1");
    assert_eq!(val(&e3, 0, 3), num(10.0), "formula at D1 reads C1");
    e3.enter(0, 0, 2, "50").unwrap(); // new C1
    assert_eq!(val(&e3, 0, 3), num(100.0), "D1 recalcs after column insert");
}

#[test]
fn sheet_calc_rewrite_structural_shifts_sizing_and_merges() {
    // Sizing/merge maps are not on the public engine cell-entry surface, so we
    // seed a model carrying a row height (row 5), a col width (col 0), and a
    // merge (A3:B4), then wrap a fresh engine and apply the structural edit.
    let mut model = SheetModel::new();
    model.add_sheet("Sheet1");
    {
        let ws = model.sheet_mut(0).unwrap();
        ws.row_heights.insert(5, 22.0);
        ws.col_widths.insert(0, 14.0);
        ws.merges.push(RangeRef {
            start: cr(2, 0),
            end: cr(3, 1),
        });
    }
    let mut e2 = Engine::new(model, EngineConfig::default());
    // Insert 2 rows at row 0.
    e2.apply_edit(&Edit::InsertRows {
        sheet: 0,
        at: 0,
        n: 2,
    });
    let ws = e2.model().sheet(0).unwrap();
    // Row height moved from row 5 to row 7.
    assert_eq!(ws.row_heights.get(&7), Some(&22.0), "row height shifted");
    assert!(!ws.row_heights.contains_key(&5));
    // Col width is on the col axis — unaffected by a ROW insert.
    assert_eq!(ws.col_widths.get(&0), Some(&14.0), "col width unchanged");
    // Merge A3:B4 moved down 2 rows -> A5:B6.
    assert_eq!(ws.merges.len(), 1);
    let merged = ws.merges[0].normalized();
    assert_eq!(merged.start.row, 4);
    assert_eq!(merged.end.row, 5);
    assert_eq!(merged.start.col, 0);
    assert_eq!(merged.end.col, 1);
}

#[test]
fn sheet_calc_rewrite_structural_deleted_range_clips() {
    // SUM over a range that loses one row clips and stays correct.
    let mut e = engine();
    e.enter(0, 0, 0, "1").unwrap(); // A1
    e.enter(0, 1, 0, "2").unwrap(); // A2
    e.enter(0, 2, 0, "3").unwrap(); // A3
    e.enter(0, 4, 0, "=SUM(A1:A3)").unwrap(); // A5 = 6
    assert_eq!(val(&e, 4, 0), num(6.0));
    // Delete row 1 (A2): the range clips to A1:A2 (the surviving A1 + old A3),
    // which after deletion holds 1 and 3 -> 4. The formula moved up to A4.
    e.apply_edit(&Edit::DeleteRows {
        sheet: 0,
        at: 1,
        n: 1,
    });
    assert_eq!(
        val(&e, 3, 0),
        num(4.0),
        "SUM clips the deleted row and recomputes (1+3)"
    );
}
