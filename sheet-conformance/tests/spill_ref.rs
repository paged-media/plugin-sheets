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

//! The spill-range operator `X#` answers as Excel does.
//!
//! Excel 16 (recorded in `corpus/xlsx-dynamic/spill-ref.excel.xlsx`):
//! - `X#` over a live spill (or a legacy CSE array) is its whole area;
//! - over any other FORMULA cell — a plain `=1+1`, a one-cell spill, a
//!   blocked spill — it is that one cell (ROWS 1, ISREF TRUE; a blocked
//!   spill's `#SPILL!` reads through);
//! - over an empty cell or a constant it is `#REF!`, wherever it is used
//!   (SUM, ROWS, COLUMNS, INDEX, LET, an operator, bare) and ISREF is FALSE;
//! - a bare `=X#` spills the area.
//!
//! Before this fix `X#` over an empty cell or a constant read the cell's
//! value (`SUM(AE1#)` was 0, `ROWS(L1#)` 1), a bare `=E1#` over a live spill
//! was `#VALUE!` instead of spilling, and a dynamic array from the file whose
//! root is an operator (`E1#+0`) kept its saved one-cell area instead of
//! spilling (here into the probe below: `#SPILL!`).

use sheet_js::core::SheetSession;

fn corpus(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../corpus/xlsx-dynamic")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn rc(a1: &str) -> (u32, u32) {
    let (r, c, _, _) = sheet_core::parse_a1(a1).unwrap_or_else(|| panic!("bad address {a1}"));
    (r, c)
}

fn show(s: &SheetSession, a1: &str) -> String {
    let (r, c) = rc(a1);
    s.get_cell_display(0, r, c)
}

fn set(s: &mut SheetSession, a1: &str, input: &str) {
    let (r, c) = rc(a1);
    s.set_cell(0, r, c, input)
        .unwrap_or_else(|e| panic!("{a1} := {input}: {e}"));
}

/// What Excel 16 computed for each probe of `spill-ref.xlsx` (column O,
/// labelled in column N). Excel caches a `#SPILL!` result as `#VALUE!` with a
/// rich value naming the real error; those are written `#SPILL!` here.
const EXCEL: &[(&str, &str)] = &[
    ("SUM(L1#)", "#REF!"),
    ("ROWS(L1#)", "#REF!"),
    ("COLUMNS(L1#)", "#REF!"),
    ("INDEX(L1#,1)", "#REF!"),
    ("LET(x,L1#,SUM(x))", "#REF!"),
    ("L1#+0", "#REF!"),
    ("ISREF(L1#)", "FALSE"),
    ("SUM(A1#)", "#REF!"),
    ("ROWS(A1#)", "#REF!"),
    ("COLUMNS(A1#)", "#REF!"),
    ("INDEX(A1#,1)", "#REF!"),
    ("LET(x,A1#,SUM(x))", "#REF!"),
    ("A1#+0", "#REF!"),
    ("ISREF(A1#)", "FALSE"),
    ("SUM(B1#)", "2"),
    ("ROWS(B1#)", "1"),
    ("COLUMNS(B1#)", "1"),
    ("INDEX(B1#,1)", "2"),
    ("LET(x,B1#,SUM(x))", "2"),
    ("B1#+0", "2"),
    ("ISREF(B1#)", "TRUE"),
    ("SUM(C1#)", "1"),
    ("ROWS(C1#)", "1"),
    ("COLUMNS(C1#)", "1"),
    ("INDEX(C1#,1)", "1"),
    ("LET(x,C1#,SUM(x))", "1"),
    ("C1#+0", "1"),
    ("ISREF(C1#)", "TRUE"),
    ("SUM(D1#)", "#SPILL!"),
    ("ROWS(D1#)", "1"),
    ("COLUMNS(D1#)", "1"),
    ("INDEX(D1#,1)", "#SPILL!"),
    ("LET(x,D1#,SUM(x))", "#SPILL!"),
    ("D1#+0", "#SPILL!"),
    ("ISREF(D1#)", "TRUE"),
    ("SUM(E1#)", "10"),
    ("ROWS(E1#)", "4"),
    ("COLUMNS(E1#)", "1"),
    ("INDEX(E1#,1)", "1"),
    ("LET(x,E1#,SUM(x))", "10"),
    // A dynamic array spills its block — here into the probe below it.
    ("E1#+0", "#SPILL!"),
    ("ISREF(E1#)", "TRUE"),
    ("SUM(F1#)", "21"),
    ("ROWS(F1#)", "2"),
    ("COLUMNS(F1#)", "3"),
    ("INDEX(F1#,1)", "#REF!"),
    ("LET(x,F1#,SUM(x))", "21"),
    ("F1#+0", "#SPILL!"),
    ("ISREF(F1#)", "TRUE"),
    ("SUM(K1#)", "60"),
    ("ROWS(K1#)", "3"),
    ("COLUMNS(K1#)", "1"),
    ("INDEX(K1#,1)", "10"),
    ("LET(x,K1#,SUM(x))", "60"),
    ("K1#+0", "#SPILL!"),
    ("ISREF(K1#)", "TRUE"),
];

/// Where Paged still answers differently, and why. `INDEX` over a 2-D
/// REFERENCE with one selector is `#REF!` in Excel; the `INDEX` kernel reads
/// column 1 (the same for `INDEX(F1:H2,1)`) — an `INDEX` matter, not the
/// spill operator's.
const KNOWN: &[(&str, &str)] = &[("INDEX(F1#,1)", "1")];

#[test]
fn the_spill_range_operator_answers_as_excel__feat__sheet_calc_spill() {
    let bytes = corpus("spill-ref.excel.xlsx");

    // The table above IS the file: every probe's label and Excel's cached
    // value (a rich `#SPILL!` caches as `#VALUE!`).
    let cached = sheet_xlsx::XlsxDocument::open(&bytes).unwrap().model;
    let ws = &cached.sheets[0];
    for (i, (label, want)) in EXCEL.iter().enumerate() {
        let r = i as u32;
        let shown_label = match ws.cell(r, 13).map(|c| &c.value) {
            Some(sheet_core::CellValue::Text(t)) => t.to_string(),
            other => panic!("N{}: {other:?}", r + 1),
        };
        assert_eq!(&shown_label, label, "the table follows the file's order");
        let file = ws.cell(r, 14).map(|c| c.value.clone()).unwrap_or_default();
        let file = match file {
            sheet_core::CellValue::Error(e) => e.as_str().to_string(),
            sheet_core::CellValue::Bool(b) => (if b { "TRUE" } else { "FALSE" }).to_string(),
            sheet_core::CellValue::Number(n) => n.to_string(),
            v => format!("{v:?}"),
        };
        let want_file = if *want == "#SPILL!" { "#VALUE!" } else { want };
        assert_eq!(file, want_file, "Excel cached O{} = {label}", r + 1);
    }

    let s = SheetSession::load_xlsx(&bytes).unwrap();
    let mut wrong = Vec::new();
    for (i, (label, want)) in EXCEL.iter().enumerate() {
        let got = s.get_cell_display(0, i as u32, 14);
        let want = KNOWN
            .iter()
            .find(|(l, _)| l == label)
            .map_or(*want, |(_, ours)| *ours);
        if got != want {
            wrong.push(format!("O{} {label}: {got}, Excel {want}", i + 1));
        }
    }
    // Bare `=X#` (row 2 of each column): a reference that spills its area.
    for (a1, want) in [
        ("Q2", "#REF!"),
        ("U2", "#REF!"),
        ("Y2", "2"),
        ("AC2", "1"),
        ("AG2", "#SPILL!"),
        ("AK2", "1"),
        ("AK5", "4"),
        ("AO2", "1"),
        ("AQ3", "6"),
        ("AS2", "10"),
        ("AS4", "30"),
    ] {
        let got = show(&s, a1);
        if got != want {
            wrong.push(format!("{a1}: {got}, Excel {want}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The reference follows its anchor: no spill → `#REF!`; a spill → its area;
/// a constant typed over it → `#REF!` again.
#[test]
fn a_spill_reference_follows_what_its_anchor_holds__feat__sheet_calc_spill() {
    let mut s = SheetSession::new();
    set(&mut s, "B1", "=SUM(AE1#)");
    set(&mut s, "B2", "=ROWS(AE1#)");
    set(&mut s, "B3", "=LET(x,AE1#,SUM(x))");
    set(&mut s, "C1", "=AE1#");
    let probes = |s: &SheetSession| ["B1", "B2", "B3", "C1", "C2"].map(|a| show(s, a));
    assert_eq!(probes(&s), ["#REF!", "#REF!", "#REF!", "#REF!", ""]);

    set(&mut s, "AE1", "=SEQUENCE(2)");
    assert_eq!(probes(&s), ["3", "2", "3", "1", "2"], "C1 spills AE1#");

    set(&mut s, "AE1", "5");
    assert_eq!(probes(&s), ["#REF!", "#REF!", "#REF!", "#REF!", ""]);

    set(&mut s, "AE1", "=1+1");
    assert_eq!(probes(&s), ["2", "1", "2", "2", ""]);
}
