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

//! Defects found by the oracle/property lanes that are NOT a row of the Excel
//! oracle's `divergences.tsv` (those are listed there, checked both ways).
//!
//! Each test states the CORRECT behaviour and is `#[should_panic]` because
//! the engine does not have it yet. When the defect is fixed the test stops
//! panicking and FAILS ("should panic"), so the fix has to delete the
//! attribute in the same commit — the record cannot go stale silently. No
//! `#[ignore]`.

use sheet_js::core::SheetSession;

/// Found by `xlsx_roundtrip_prop` (minimal case below). A cell whose range
/// includes itself (`A3 = SUM($A$1:B3)`) is a cycle. The workbook entered
/// incrementally and the SAME workbook loaded from its saved bytes (a full
/// `recalc_all`) must settle the cycle the same way — instead a dependent
/// shows `#NAME?` (the first error in range order) on one path and `#REF!`
/// on the other. The display a user sees depends on how the workbook got
/// into memory.
#[test]
#[should_panic(expected = "incremental and full recalc disagree")]
fn defect_cycle_settles_differently_after_reload__feat__sheet_calc_engine() {
    let mut s = SheetSession::new();
    s.set_cell(0, 2, 0, "=SUM($A$1:B3)").unwrap();
    s.set_cell(0, 0, 0, "#NAME?").unwrap();
    s.set_cell(0, 0, 2, "=SUM($A$1:B3)").unwrap();
    let before: Vec<String> = [(2, 0), (0, 2)]
        .iter()
        .map(|&(r, c)| s.get_cell_display(0, r, c))
        .collect();
    let bytes = s.save_xlsx().unwrap();
    let r = SheetSession::load_xlsx(&bytes).unwrap();
    let after: Vec<String> = [(2, 0), (0, 2)]
        .iter()
        .map(|&(r2, c)| r.get_cell_display(0, r2, c))
        .collect();
    assert_eq!(
        before, after,
        "incremental and full recalc disagree on a cycle: before {before:?}, after reload {after:?}"
    );
}

/// Found by `format_prop` and confirmed against desktop Excel 16.113 (en-US,
/// `=TEXT(A1,"0.00")`, 2026-10-04). Excel rounds the value as DISPLAYED to 15
/// significant digits — 1.005 is stored as 1.00499999999999989... but shows
/// 1.01 — while the formatter rounds the binary value and shows 1.00. Every
/// pair below is Excel's answer.
#[test]
#[should_panic(expected = "formatter rounds the binary value")]
fn defect_fixed_decimals_round_the_binary_value__feat__sheet_format_engine() {
    use sheet_core::{CellValue, DateSystem, Locale};
    use sheet_format::{compile, format_value, FormatCtx};
    let ctx = FormatCtx::new(DateSystem::Date1900, Locale::EnUs);
    let f = compile("0.00").unwrap();
    let excel = [
        (297527943.585, "297527943.59"),
        (-297527943.585, "-297527943.59"),
        (663825561.895, "663825561.90"),
        (1.005, "1.01"),
        (-1.005, "-1.01"),
        (2.675, "2.68"),
        (0.125, "0.13"),
        (1234567.125, "1234567.13"),
    ];
    let wrong: Vec<String> = excel
        .iter()
        .filter_map(|&(v, want)| {
            let got = format_value(&CellValue::Number(v), &f, &ctx);
            (got != want).then(|| format!("{v} -> {got}, Excel {want}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "formatter rounds the binary value, not Excel's 15-digit decimal: {wrong:?}"
    );
}

/// Found by the `parse_formula` fuzz target (`fuzz/regressions/
/// parse_formula-union-print-not-reparsable.txt`). A parenthesised union
/// `(1/0,-100,-1000)` parses, but the printer emits `1/0,(-100,(-1000))` —
/// the outer parentheses are dropped, and the printed text no longer parses
/// ("unexpected trailing token"). The xlsx writer re-emits edited formulas
/// through this printer, so such a cell would be saved as a formula Excel
/// (and this engine) cannot read back.
#[test]
fn defect_union_prints_without_its_parentheses__feat__sheet_parser_dialect() {
    use sheet_core::{NameId, SheetId};
    use sheet_parser::{parse, print, ParseCtx, SheetNames};
    struct Ctx;
    impl ParseCtx for Ctx {
        fn sheet_id(&self, _: &str) -> Option<SheetId> {
            None
        }
        fn name_id(&self, _: &str) -> Option<NameId> {
            None
        }
        fn current_sheet(&self) -> SheetId {
            0
        }
    }
    impl SheetNames for Ctx {
        fn sheet_name(&self, _: SheetId) -> Option<&str> {
            Some("Sheet1")
        }
        fn defined_name(&self, _: NameId) -> Option<&str> {
            None
        }
    }
    let f = parse("(1/0,-100,-1000)", &Ctx).expect("the union parses");
    let printed = print(&f, 0, &Ctx);
    assert!(
        parse(&printed, &Ctx).is_ok(),
        "printed union does not reparse: {printed:?}"
    );
}
