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

#![allow(non_snake_case)] // `__feat__<id>` test-name links (cockpit rule)

//! Wave 4 session operations, end to end through `SheetSession` (the logic
//! the wasm shim forwards to): the page-lowering door, the host clock,
//! `<calcPr>` iteration, worksheet add/rename/delete, and the row/column
//! insert/delete door — each followed through `save_xlsx` and a reload, so
//! the package the writer produces is what the assertions read.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;

use sheet_js::core::{LowerOptions, SheetSession, StructuralEdit};

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join("xlsx-corpus")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), b);
    }
    out
}

fn part_text(bytes: &[u8], name: &str) -> String {
    String::from_utf8(parts(bytes).remove(name).unwrap_or_default()).unwrap()
}

/// A minimal package with `workbook_extra` spliced into `<workbook>` after
/// `<sheets>` and one worksheet whose `<sheetData>` is `sheet_data`.
fn package(workbook_extra: &str, sheet_data: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        let mut add = |name: &str, body: String| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.into());
        add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into());
        add(
            "xl/workbook.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>{workbook_extra}</workbook>"#
            ),
        );
        add("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.into());
        add(
            "xl/worksheets/sheet1.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{sheet_data}</sheetData></worksheet>"#
            ),
        );
        zip.finish().unwrap();
    }
    buf
}

// ── 1. the page door ─────────────────────────────────────────────────────

/// The placed table's door resolves the workbook's REAL styles and folds
/// conditional formatting on top; the frozen `get_range_lowered` door does
/// neither (its key-0 contract is unchanged).
#[test]
fn page_door_folds_condfmt_over_real_styles__feat__sheet_lower_page() {
    let s = SheetSession::load_xlsx(&fixture("08-condfmt.xlsx")).unwrap();
    let page = s
        .get_range_page(0, "A1:E5", LowerOptions::default())
        .unwrap();
    let fill = |r: usize, c: usize| {
        let key = page.rows[r].cells[c].style_key as usize;
        page.styles[key].fill_rgb.clone()
    };
    // Column A: 8, 3, 10, 5, 7 under `cellIs > 5` → dxf 0 (yellow).
    assert_eq!(fill(0, 0).as_deref(), Some("#FFFF00"));
    assert_eq!(fill(1, 0), None);
    assert_eq!(fill(2, 0).as_deref(), Some("#FFFF00"));
    // Grid rules default ON for the page.
    assert!(!page.rules.h.is_empty());

    let frozen = s
        .get_range_lowered(0, "A1:E5", LowerOptions::default())
        .unwrap();
    assert_eq!(frozen.styles.len(), 1, "the frozen door stays key-0 only");
}

/// Real base styles (03-styles: a bold header with a fill) reach the page
/// door even where there is no conditional formatting.
#[test]
fn page_door_carries_base_styles__feat__sheet_lower_page() {
    let s = SheetSession::load_xlsx(&fixture("03-styles.xlsx")).unwrap();
    let page = s
        .get_range_page(0, "A1:D6", LowerOptions::default())
        .unwrap();
    assert!(
        page.styles.len() > 1,
        "styled cells resolve to real style keys: {:?}",
        page.styles
    );
}

// ── 4. the host clock ────────────────────────────────────────────────────

/// `set_clock` turns the host's epoch-ms + timezone offset into the LOCAL
/// serial of the workbook's date system, and `recalc_volatile` re-evaluates
/// `TODAY()` against it (until Wave 4 it evaluated at serial 0).
#[test]
fn host_clock_drives_today__feat__sheet_calc_engine() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "=TODAY()").unwrap();
    assert_eq!(s.get_cell_display(0, 0, 0), "0", "no clock yet: serial 0");

    // 2026-10-04T23:30Z in a UTC-2 zone (offset +120) is still 2026-10-04
    // locally at 21:30; in UTC+2 (offset -120) it is already 2026-10-05.
    let unix_ms = 1_791_156_600_000.0; // 2026-10-04T23:30:00Z
    let serial = s.set_clock(unix_ms, 120.0);
    assert!((serial - (46299.0 + 21.5 / 24.0)).abs() < 1e-9, "{serial}");
    let r = s.recalc_volatile();
    assert!(r.changed.iter().any(|c| (c.row, c.col) == (0, 0)));
    assert_eq!(s.get_cell_display(0, 0, 0), "46299");
    s.set_clock(unix_ms, -120.0);
    s.recalc_volatile();
    assert_eq!(s.get_cell_display(0, 0, 0), "46300");
}

/// A load evaluates volatile cells against the clock already set
/// (`load_xlsx_at` — what the wasm shim does when the host set it first).
#[test]
fn load_evaluates_volatile_against_the_clock__feat__sheet_calc_engine() {
    let bytes = package(
        "",
        r#"<row r="1"><c r="A1"><f>TODAY()</f><v>0</v></c></row>"#,
    );
    let s = SheetSession::load_xlsx_at(&bytes, 46299.25).unwrap();
    assert_eq!(s.get_cell_display(0, 0, 0), "46299");
}

// ── 7. <calcPr> iteration ────────────────────────────────────────────────

const CYCLE: &str =
    r#"<row r="1"><c r="A1"><f>B1/2+1</f><v>0</v></c><c r="B1"><f>A1</f><v>0</v></c></row>"#;

/// A workbook whose `<calcPr>` enables iteration converges its cycle on
/// load; the knobs round-trip byte-identically on a zero-edit save.
#[test]
fn calc_pr_iteration_is_honoured_and_round_trips__feat__sheet_calc_iterative() {
    let bytes = package(
        r#"<calcPr calcId="191029" iterate="1" iterateCount="200" iterateDelta="0.000001"/>"#,
        CYCLE,
    );
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    let c = s.calc_settings();
    assert!(c.iterative);
    assert_eq!(c.max_iter, 200);
    let a1: f64 = s.get_cell_display(0, 0, 0).parse().unwrap();
    assert!(
        (a1 - 2.0).abs() < 1e-4,
        "converged to the fixed point 2, got {a1}"
    );

    let saved = s.save_xlsx().unwrap();
    assert_eq!(
        part_text(&saved, "xl/workbook.xml"),
        part_text(&bytes, "xl/workbook.xml"),
        "untouched calcPr: workbook part verbatim"
    );
}

/// Without `<calcPr iterate>` the same cycle is the circular ruling
/// (`#REF!`); switching iteration on is written back into `<calcPr>`, other
/// attributes kept, and a reload honours it.
#[test]
fn calc_pr_toggle_writes_back__feat__sheet_calc_iterative() {
    let bytes = package(r#"<calcPr calcId="191029"/>"#, CYCLE);
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    assert_eq!(s.get_cell_display(0, 0, 0), "#REF!");
    s.set_iterative(true, 100, 0.0001);
    assert!(s.metadata().dirty, "a settings change is a pending save");
    let saved = s.save_xlsx().unwrap();
    let wb = part_text(&saved, "xl/workbook.xml");
    assert!(
        wb.contains(
            r#"<calcPr calcId="191029" iterate="1" iterateCount="100" iterateDelta="0.0001"/>"#
        ),
        "{wb}"
    );
    let s2 = SheetSession::load_xlsx(&saved).unwrap();
    assert!(s2.calc_settings().iterative);
    let a1: f64 = s2.get_cell_display(0, 0, 0).parse().unwrap();
    assert!((a1 - 2.0).abs() < 1e-3);

    // A workbook with NO <calcPr> gains one in schema position.
    let mut s3 = SheetSession::load_xlsx(&package("", CYCLE)).unwrap();
    s3.set_iterative(true, 50, 0.001);
    let wb3 = part_text(&s3.save_xlsx().unwrap(), "xl/workbook.xml");
    assert!(
        wb3.contains(r#"</sheets><calcPr iterate="1" iterateCount="50" iterateDelta="0.001"/>"#),
        "{wb3}"
    );
}

// ── 5. worksheets ────────────────────────────────────────────────────────

/// Add → write → rename (formulas follow) → save → reload.
#[test]
fn add_and_rename_sheet_round_trip__feat__sheet_workbook_sheets() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "21").unwrap();
    let id = s.add_sheet("").unwrap();
    assert_eq!(id, 1);
    assert_eq!(s.list_sheets()[1].name, "Sheet2");
    s.set_cell(1, 0, 0, "=Sheet1!A1*2").unwrap();
    assert_eq!(s.get_cell_display(1, 0, 0), "42");

    s.rename_sheet(0, "Data Q1").unwrap();
    assert_eq!(s.get_cell_input(1, 0, 0), "='Data Q1'!A1*2");
    // Validation: illegal characters, duplicates, the reserved name.
    assert!(s.rename_sheet(1, "a/b").is_err());
    assert!(s.rename_sheet(1, "data q1").is_err());
    assert!(s.add_sheet("History").is_err());

    let saved = s.save_xlsx().unwrap();
    let wb = part_text(&saved, "xl/workbook.xml");
    assert!(wb.contains(r#"name="Data Q1""#), "{wb}");
    assert!(wb.contains(r#"name="Sheet2""#), "{wb}");
    let p = parts(&saved);
    assert!(p.contains_key("xl/worksheets/sheet2.xml"));
    assert!(part_text(&saved, "[Content_Types].xml").contains("/xl/worksheets/sheet2.xml"));
    assert!(part_text(&saved, "xl/_rels/workbook.xml.rels").contains("worksheets/sheet2.xml"));

    let s2 = SheetSession::load_xlsx(&saved).unwrap();
    let names: Vec<String> = s2.list_sheets().into_iter().map(|i| i.name).collect();
    assert_eq!(names, vec!["Data Q1", "Sheet2"]);
    assert_eq!(s2.get_cell_input(1, 0, 0), "='Data Q1'!A1*2");
    assert_eq!(s2.get_cell_display(1, 0, 0), "42");
}

/// Delete: references into the deleted sheet become #REF!, later sheets
/// shift down, the part leaves the package, and the last sheet is kept.
#[test]
fn delete_sheet_refs_become_ref_error__feat__sheet_workbook_sheets() {
    let mut s = SheetSession::new();
    s.add_sheet("Two").unwrap();
    s.add_sheet("Three").unwrap();
    s.set_cell(1, 0, 0, "5").unwrap();
    s.set_cell(2, 0, 0, "7").unwrap();
    s.set_cell(0, 0, 0, "=Two!A1+1").unwrap();
    s.set_cell(0, 1, 0, "=Three!A1+1").unwrap();
    let saved_before = s.save_xlsx().unwrap();
    let parts_before = parts(&saved_before).len();

    s.delete_sheet(1).unwrap();
    let names: Vec<String> = s.list_sheets().into_iter().map(|i| i.name).collect();
    assert_eq!(names, vec!["Sheet1", "Three"]);
    assert_eq!(s.get_cell_display(0, 0, 0), "#REF!");
    assert_eq!(
        s.get_cell_display(0, 1, 0),
        "8",
        "Three shifted to id 1, ref intact"
    );
    assert_eq!(s.get_cell_input(0, 1, 0), "=Three!A1+1");

    let saved = s.save_xlsx().unwrap();
    assert_eq!(
        parts(&saved).len(),
        parts_before - 1,
        "one worksheet part fewer"
    );
    let s2 = SheetSession::load_xlsx(&saved).unwrap();
    assert_eq!(s2.list_sheets().len(), 2);
    assert_eq!(s2.get_cell_display(0, 1, 0), "8");
    assert_eq!(s2.get_cell_display(1, 0, 0), "7");

    let mut one = SheetSession::new();
    assert!(one.delete_sheet(0).is_err(), "the last sheet stays");
}

// ── 6. rows and columns ──────────────────────────────────────────────────

/// Insert a row inside a SUM's range: values shift, references widen, a
/// cross-sheet reference follows, and the saved package reloads to the same
/// workbook.
#[test]
fn insert_rows_rewrites_refs_and_saves__feat__sheet_calc_rewrite_structural_door() {
    let mut s = SheetSession::new();
    s.add_sheet("Other").unwrap();
    for (r, v) in [(0, "1"), (1, "2"), (2, "3")] {
        s.set_cell(0, r, 0, v).unwrap();
    }
    s.set_cell(0, 0, 1, "=SUM(A1:A3)").unwrap();
    s.set_cell(1, 0, 0, "=Sheet1!A3*10").unwrap();
    // Save once so the formula texts are the stored ones a structural edit
    // must re-print (not just pending edits).
    let s_bytes = s.save_xlsx().unwrap();
    let mut s = SheetSession::load_xlsx(&s_bytes).unwrap();

    s.structural_edit(0, StructuralEdit::InsertRows, 1, 1)
        .unwrap();
    assert_eq!(s.get_cell_display(0, 1, 0), "", "inserted row is blank");
    assert_eq!(s.get_cell_display(0, 3, 0), "3");
    assert_eq!(s.get_cell_input(0, 0, 1), "=SUM(A1:A4)");
    assert_eq!(s.get_cell_input(1, 0, 0), "=Sheet1!A4*10");

    let saved = s.save_xlsx().unwrap();
    let s2 = SheetSession::load_xlsx(&saved).unwrap();
    assert_eq!(s2.get_cell_input(0, 0, 1), "=SUM(A1:A4)");
    assert_eq!(s2.get_cell_display(0, 0, 1), "6");
    assert_eq!(s2.get_cell_input(1, 0, 0), "=Sheet1!A4*10");
    assert_eq!(s2.get_cell_display(1, 0, 0), "30");
    assert_eq!(s2.get_cell_display(0, 3, 0), "3");
    // No stale <f> left at the old positions.
    let ws = part_text(&saved, "xl/worksheets/sheet1.xml");
    assert_eq!(ws.matches("<f>").count(), 1, "{ws}");
}

/// Delete a column a formula points into → #REF!; delete rows above a
/// formula shifts it up.
#[test]
fn delete_cols_and_rows__feat__sheet_calc_rewrite_structural_door() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "4").unwrap();
    s.set_cell(0, 0, 1, "5").unwrap();
    s.set_cell(0, 3, 2, "=B1*2").unwrap();
    s.structural_edit(0, StructuralEdit::DeleteRows, 1, 2)
        .unwrap();
    assert_eq!(s.get_cell_input(0, 1, 2), "=B1*2");
    assert_eq!(s.get_cell_display(0, 1, 2), "10");
    s.structural_edit(0, StructuralEdit::DeleteCols, 1, 1)
        .unwrap();
    assert_eq!(s.get_cell_display(0, 1, 1), "#REF!");
    let s2 = SheetSession::load_xlsx(&s.save_xlsx().unwrap()).unwrap();
    assert_eq!(s2.get_cell_display(0, 1, 1), "#REF!");
    assert_eq!(s2.get_cell_display(0, 0, 0), "4");
}

/// A sheet carrying preserved, cell-addressed content (data validations)
/// REFUSES a structural edit and is left untouched.
#[test]
fn structural_edit_refuses_preserved_addresses__feat__sheet_calc_rewrite_structural_door() {
    let mut s = SheetSession::load_xlsx(&fixture("12-datavalidation.xlsx")).unwrap();
    let before: Vec<String> = (0..6).map(|r| s.get_cell_display(0, r, 0)).collect();
    let err = s
        .structural_edit(0, StructuralEdit::InsertRows, 0, 1)
        .unwrap_err();
    assert!(err.to_string().contains("dataValidations"), "{err}");
    let after: Vec<String> = (0..6).map(|r| s.get_cell_display(0, r, 0)).collect();
    assert_eq!(before, after);
    assert!(!s.metadata().dirty);
}

// ── 8. CSV / TSV import ──────────────────────────────────────────────────

/// A German semicolon CSV: sniffed delimiter, decimal comma, a dd.mm.yyyy
/// date rendered in the locale's pattern, a percent, quoted text; it saves
/// as a real workbook and reloads to the same values.
#[test]
fn csv_import_types_by_locale_and_saves__feat__sheet_import_csv() {
    let text = "Region;Umsatz;Datum;Anteil\r\nNord;1.234,50;04.10.2026;12,5%\r\n\"S\u{fc}d; West\";99;2026-01-31;7%\r\n";
    let mut s = SheetSession::load_csv(text, None, "de-DE", "Umsatz", 0.0).unwrap();
    assert_eq!(s.list_sheets()[0].name, "Umsatz");
    assert_eq!(s.get_cell_input(0, 1, 1), "1234.5");
    assert_eq!(s.get_cell_input(0, 1, 2), "46299");
    assert_eq!(s.get_cell_display(0, 1, 2), "04.10.2026");
    assert_eq!(s.get_cell_input(0, 1, 3), "0.125");
    assert_eq!(s.get_cell_display(0, 2, 0), "S\u{fc}d; West");
    assert!(s.metadata().dirty, "an import is unsaved content");
    let s2 = SheetSession::load_xlsx(&s.save_xlsx().unwrap()).unwrap();
    assert_eq!(s2.get_cell_display(0, 1, 2), "04.10.2026");
    assert_eq!(s2.get_cell_input(0, 2, 2), "46053");

    let tsv =
        SheetSession::load_csv("a\tb\n1.5\t=SUM(A1)\n", Some('\t'), "en-US", "", 0.0).unwrap();
    assert_eq!(tsv.list_sheets()[0].name, "Sheet1");
    assert_eq!(tsv.get_cell_input(0, 1, 0), "1.5");
    assert_eq!(
        tsv.get_cell_display(0, 1, 1),
        "=SUM(A1)",
        "a CSV formula stays text"
    );
}
