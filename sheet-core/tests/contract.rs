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

//! Integration smoke test over the frozen public surface (spec §5). Builds
//! a tiny workbook end-to-end through the re-exported crate-root API.

use sheet_core::{
    col_to_a1, funcs, parse_a1, Cell, CellValue, Expr, Formula, LitValue, OrderedF64, SheetModel,
};

#[test]
fn build_a_tiny_workbook() {
    let mut model = SheetModel::new();
    let sid = model.add_sheet("Budget");

    // A1 = 10, A2 = =A1*2 (formula cell, value cached as 20).
    let fid = model.intern_formula(Formula {
        root: Expr::Binary(
            sheet_core::BinOp::Mul,
            Box::new(Expr::Ref(sheet_core::CellRef {
                sheet: sid,
                row: 0,
                col: 0,
                row_abs: false,
                col_abs: false,
            })),
            Box::new(Expr::Lit(LitValue::Number(OrderedF64::new(2.0)))),
        ),
    });

    let ws = model.sheet_mut(sid).unwrap();
    ws.set_cell(
        0,
        0,
        Cell {
            value: CellValue::Number(10.0),
            ..Default::default()
        },
    );
    ws.set_cell(
        1,
        0,
        Cell {
            value: CellValue::Number(20.0),
            formula: Some(fid),
            ..Default::default()
        },
    );

    let ws = model.sheet(sid).unwrap();
    let ur = ws.used_range().unwrap();
    assert_eq!((ur.row0, ur.col0, ur.row1, ur.col1), (0, 0, 1, 0));
    assert_eq!(col_to_a1(ur.col0).as_str(), "A");

    // The formula cell carries a resolvable FormulaId.
    let stored = ws.cell(1, 0).unwrap().formula.unwrap();
    assert!(model.formula(stored).is_some());

    // A1 parse sanity.
    assert_eq!(parse_a1("A2"), Some((1, 0, false, false)));
}

#[test]
fn registry_lookup_through_reexport() {
    let id = funcs::lookup_func("AVERAGE").unwrap();
    let m = funcs::meta(id);
    assert_eq!(m.name, "AVERAGE");
    assert!(m.range_aware);
    assert_eq!(funcs::lookup_func("not_a_fn"), None);
}
