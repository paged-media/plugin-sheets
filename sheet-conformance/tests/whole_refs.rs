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

// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
#![allow(non_snake_case)]

//! Whole-column (`A:A`, `Sheet!B:D`, `$A:$A`) and whole-row (`1:1`)
//! references through the ENGINE: they parse, evaluate over the populated
//! rows only, track edits through the dependency index, and survive
//! structural edits. (Before round 2 every such formula was refused by the
//! parser and an imported workbook kept Excel's cached value.)

use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellValue, SheetModel};
use sheet_parser::Edit;

fn engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    m.add_sheet("Data");
    Engine::new(m, EngineConfig::default())
}

fn val(e: &Engine, sheet: u16, row: u32, col: u32) -> CellValue {
    e.model()
        .sheet(sheet)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

fn n(v: f64) -> CellValue {
    CellValue::Number(v)
}

/// A1:A5 = 1..5, B1:B5 = "a".."e", Data!B2:B4 = 10,20,30.
fn seeded() -> Engine {
    let mut e = engine();
    for i in 0..5u32 {
        e.enter(0, i, 0, &(i + 1).to_string()).unwrap();
        e.enter(0, i, 1, &((b'a' + i as u8) as char).to_string())
            .unwrap();
    }
    for (i, v) in [10, 20, 30].iter().enumerate() {
        e.enter(1, i as u32 + 1, 1, &v.to_string()).unwrap();
    }
    e
}

#[test]
fn whole_column_aggregates_and_lookups__feat__sheet_calc_engine() {
    let mut e = seeded();
    let cases: &[(&str, CellValue)] = &[
        ("=SUM(A:A)", n(15.0)),
        ("=SUM($A:$A)", n(15.0)),
        ("=MAX(A:B)", n(5.0)),
        ("=COUNTA(A:B)", n(10.0)),
        ("=COUNT(A:A)", n(5.0)),
        ("=AVERAGE(A:A)", n(3.0)),
        ("=SUM(Data!B:B)", n(60.0)),
        ("=SUM(Data!A:C)", n(60.0)),
        ("=VLOOKUP(3,A:B,2,FALSE)", CellValue::from("c")),
        ("=MATCH(4,A:A,0)", n(4.0)),
        ("=INDEX(A:A,2,1)", n(2.0)),
        ("=INDEX(B:B,5)", CellValue::from("e")),
        ("=ROWS(A:C)", n(1_048_576.0)),
        ("=COLUMNS(A:C)", n(3.0)),
        ("=COLUMNS(1:1)", n(16_384.0)),
        ("=ROWS(2:4)", n(3.0)),
        ("=SUM(3:3)", n(3.0)),
        ("=SUM($1:$2)", n(3.0)),
        ("=COUNTIF(A:A,\">2\")", n(3.0)),
        ("=SUMIF(B:B,\"c\",A:A)", n(3.0)),
        ("=COUNTBLANK(A:A)", n(1_048_571.0)),
        // Blank-matching criteria count / select the rows below the data.
        ("=COUNTIF(A:A,\"\")", n(1_048_571.0)),
        ("=COUNTIF(A:A,\"<>\")", n(5.0)),
        // Approximate lookups over a whole column floor within the data, not
        // on the blank rows below it.
        ("=VLOOKUP(3.5,A:B,2)", CellValue::from("c")),
        ("=VLOOKUP(99,A:B,2,TRUE)", CellValue::from("e")),
        ("=MATCH(3.5,A:A)", n(3.0)),
        (
            "=MATCH(\"zz\",B:B,0)",
            CellValue::Error(sheet_core::CellError::Na),
        ),
    ];
    for (i, (f, want)) in cases.iter().enumerate() {
        e.enter(0, 20 + i as u32, 5, f)
            .unwrap_or_else(|err| panic!("{f}: {err:?}"));
        assert_eq!(val(&e, 0, 20 + i as u32, 5), *want, "{f}");
    }
}

#[test]
fn blank_criteria_reach_targets_below_the_criteria_data__feat__sheet_calc_engine() {
    let mut e = seeded();
    // H holds values in rows where A is blank (and one where it is not).
    e.enter(0, 2, 7, "100").unwrap();
    e.enter(0, 40, 7, "7").unwrap();
    e.enter(0, 900, 7, "3").unwrap();
    e.enter(0, 0, 5, "=SUMIF(A:A,\"\",H:H)").unwrap();
    e.enter(0, 1, 5, "=AVERAGEIF(A:A,\"\",H:H)").unwrap();
    e.enter(0, 2, 5, "=SUMIF(A:A,\">=3\",H:H)").unwrap();
    assert_eq!(val(&e, 0, 0, 5), n(10.0));
    assert_eq!(val(&e, 0, 1, 5), n(5.0));
    assert_eq!(val(&e, 0, 2, 5), n(100.0));
}

#[test]
fn whole_column_dependents_recalc_on_any_row__feat__sheet_calc_engine() {
    let mut e = seeded();
    e.enter(0, 0, 5, "=SUM(A:A)").unwrap();
    e.enter(0, 9, 5, "=SUM(2:2)").unwrap();
    // A write far below the populated rows is still inside A:A.
    e.enter(0, 50_000, 0, "100").unwrap();
    assert_eq!(val(&e, 0, 0, 5), n(115.0));
    // A write in row 2, any column, reaches SUM(2:2).
    e.enter(0, 1, 9_000, "7").unwrap();
    assert_eq!(val(&e, 0, 9, 5), n(9.0));
    // A write outside both bands reaches neither.
    e.enter(0, 3, 7, "1").unwrap();
    assert_eq!(val(&e, 0, 0, 5), n(115.0));
    assert_eq!(val(&e, 0, 9, 5), n(9.0));
}

#[test]
fn whole_bands_survive_structural_edits__feat__sheet_calc_engine() {
    let mut e = seeded();
    e.enter(0, 0, 5, "=SUM(A:A)").unwrap();
    e.enter(0, 1, 5, "=SUM(3:3)").unwrap();
    e.apply_edit(&Edit::InsertRows {
        sheet: 0,
        at: 0,
        n: 2,
    });
    // F1/F2 moved to F3/F4; A:A is still A:A, 3:3 followed its row to 5:5.
    assert_eq!(val(&e, 0, 2, 5), n(15.0));
    assert_eq!(val(&e, 0, 3, 5), n(3.0));
    e.apply_edit(&Edit::InsertCols {
        sheet: 0,
        at: 0,
        n: 1,
    });
    // The values moved to column B; the sum moved with them.
    assert_eq!(val(&e, 0, 2, 6), n(15.0));
    assert_eq!(val(&e, 0, 3, 6), n(3.0));
}
