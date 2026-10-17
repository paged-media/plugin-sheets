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

//! A dynamic-array spill survives an XLSX save and load.
//!
//! Excel stores a spilled formula as an ARRAY formula over its spill range
//! whose cell points at the `XLDAPR` cell metadata (`<c cm="1"><f t="array"
//! ref="A8:A31">_xlfn.SEQUENCE(24)</f>`, plus `xl/metadata.xml`); the other
//! cells of the range carry the cached values as plain `<v>`s. Before this
//! fix the writer emitted the anchor as a plain `<f>` next to those cached
//! values, so on reload the cached cells were ordinary values in the way of
//! the spill and every anchor read `#SPILL!`.
//!
//! Covered: the shapes a spilled day-book uses (three `SEQUENCE` anchors),
//! the common dynamic-array functions, a blocked spill, the spill-range
//! operator, Excel's own file (`corpus/xlsx-dynamic/`), a workbook whose
//! metadata part has no `XLDAPR` type yet, a legacy CSE array (stays CSE),
//! and files written before the fix (plain anchors over their own cached
//! values). Two writer defects that kept Excel from opening the result are
//! pinned here too: a cached `#SPILL!` and a re-encoded worksheet root that
//! dropped its namespace declarations.

use std::io::Read;

use sheet_js::core::{LowerOptions, SheetSession};

/// A1-style address → (row, col), 0-based.
fn rc(a1: &str) -> (u32, u32) {
    let (r, c, _, _) = sheet_core::parse_a1(a1).unwrap_or_else(|| panic!("bad address {a1}"));
    (r, c)
}

fn set(s: &mut SheetSession, a1: &str, input: &str) {
    let (r, c) = rc(a1);
    s.set_cell(0, r, c, input)
        .unwrap_or_else(|e| panic!("{a1} := {input}: {e}"));
}

fn show(s: &SheetSession, a1: &str) -> String {
    let (r, c) = rc(a1);
    s.get_cell_display(0, r, c)
}

/// Every cell's display over `rows × cols` from A1, as `A1=…` lines (the
/// non-empty ones), so a failing comparison names the cell.
fn grid(s: &SheetSession, rows: u32, cols: u32) -> Vec<String> {
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let d = s.get_cell_display(0, r, c);
            if !d.is_empty() {
                out.push(format!("{}{}={d}", sheet_core::col_to_a1(c), r + 1));
            }
        }
    }
    out
}

fn part(bytes: &[u8], name: &str) -> Option<String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    Some(s)
}

fn sheet1(bytes: &[u8]) -> String {
    part(bytes, "xl/worksheets/sheet1.xml").expect("sheet1.xml")
}

/// The `<c r="addr" …>` element of a worksheet part, start tag through its
/// close.
fn cell_xml<'a>(xml: &'a str, addr: &str) -> &'a str {
    let open = format!(r#"<c r="{addr}""#);
    let at = xml
        .find(&open)
        .unwrap_or_else(|| panic!("no <c r=\"{addr}\"> in {xml}"));
    let rest = &xml[at..];
    let end = match (rest.find("/>"), rest.find('>')) {
        (Some(e), Some(g)) if e < g => e + 2,
        _ => rest.find("</c>").map(|e| e + 4).unwrap(),
    };
    &rest[..end]
}

fn corpus(dir: &str, name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../corpus")
        .join(dir)
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A 24-row day-book spilled from three SEQUENCE anchors, plus the common
/// dynamic-array functions, typed into a blank workbook.
fn typed_workbook() -> SheetSession {
    let mut s = SheetSession::new();
    for (i, (g, h)) in ["b", "a", "c", "a", "b", "d"]
        .iter()
        .zip([5, 2, 8, 1, 9, 4])
        .enumerate()
    {
        set(&mut s, &format!("G{}", i + 1), g);
        set(&mut s, &format!("H{}", i + 1), &h.to_string());
    }
    // The day-book: three SEQUENCE anchors, 24 rows each.
    set(&mut s, "A8", "=SEQUENCE(24)");
    set(&mut s, "B8", "=SEQUENCE(24,1,1840,7)");
    set(&mut s, "C8", "=SEQUENCE(24,1,620,13)");
    // The formulas page's spill.
    set(&mut s, "E1", "=SEQUENCE(4,1,10,10)");
    set(&mut s, "J1", "=SORT(H1:H6)");
    set(&mut s, "K1", "=UNIQUE(G1:G6)");
    set(&mut s, "L1", "=FILTER(H1:H6,H1:H6>3)");
    set(&mut s, "M1", "=TRANSPOSE(H1:H3)");
    // A blocked spill: S2 holds a value.
    set(&mut s, "S2", "x");
    set(&mut s, "S1", "=SEQUENCE(3)");
    set(&mut s, "W1", "=SEQUENCE(1)");
    set(&mut s, "U1", "=SUM(A8#)");
    set(&mut s, "Y1", "=SEQUENCE(2,3)");
    set(&mut s, "U3", "=SUM(H1:H6)");
    // A spill whose size follows a cell.
    set(&mut s, "AC1", "3");
    set(&mut s, "AD1", "=SEQUENCE(AC1)");
    s
}

const ROWS: u32 = 32;
const COLS: u32 = 31;

#[test]
fn typed_spills_survive_save_and_reload__feat__sheet_calc_spill() {
    let mut s = typed_workbook();
    let before = grid(&s, ROWS, COLS);
    // The fixture computes what it says before anything is saved.
    assert_eq!(show(&s, "A31"), "24");
    assert_eq!(show(&s, "C31"), "919");
    assert_eq!(show(&s, "S1"), "#SPILL!");
    assert_eq!(show(&s, "U1"), "300");
    let spill_errors: Vec<&String> = before.iter().filter(|l| l.contains("#SPILL!")).collect();
    assert_eq!(spill_errors, ["S1=#SPILL!"], "only the blocked anchor");

    let bytes = s.save_xlsx().unwrap();
    let mut r = SheetSession::load_xlsx(&bytes).unwrap();
    assert_eq!(grid(&r, ROWS, COLS), before, "after save + reload");

    // The reloaded spills are live, not values: the reloaded workbook answers
    // every edit exactly as the one that was never saved.
    for (a1, input) in [
        ("AC1", "5"), // a bigger count spills further
        ("AC1", "2"), // a smaller one leaves no stale cells
        ("A8", ""),   // clearing an anchor clears what it spilled
        ("S2", ""),   // removing the blocker lets the blocked anchor spill
        ("H2", "20"), // SORT / FILTER / TRANSPOSE follow their source
    ] {
        set(&mut s, a1, input);
        set(&mut r, a1, input);
        assert_eq!(
            grid(&r, ROWS, COLS),
            grid(&s, ROWS, COLS),
            "after {a1} := {input:?}"
        );
    }
    assert_eq!(show(&r, "AD3"), "");
    assert_eq!(
        show(&r, "A9"),
        "",
        "the day-book column was a spill, not values"
    );
    assert_eq!(show(&r, "S3"), "3");
    assert_eq!(show(&r, "J6"), "20");

    // A second generation reads back the same.
    let again = r.save_xlsx().unwrap();
    let after_edits = grid(&r, ROWS, COLS);
    let r2 = SheetSession::load_xlsx(&again).unwrap();
    assert_eq!(grid(&r2, ROWS, COLS), after_edits, "second save + reload");
}

/// The bundle replaces a placed table on reopen only when the page content
/// it stamped on the frame differs from what the reloaded workbook lowers to
/// (`pageContent`: `getRangePage` with grid rules). An unchanged workbook must
/// lower identically after its own save and load — before the fix every
/// spilled cell came back `#SPILL!` and the table was rewritten.
#[test]
fn a_lowered_spill_range_is_identical_after_save_and_reload__feat__sheet_calc_spill() {
    let mut s = typed_workbook();
    let page = |s: &SheetSession, range: &str| {
        let opts = LowerOptions {
            include_grid_rules: Some(true),
            ..LowerOptions::default()
        };
        s.get_range_page(0, range, opts).unwrap()
    };
    let lowered = |s: &SheetSession, range: &str| {
        s.get_range_lowered(0, range, LowerOptions::default())
            .unwrap()
    };
    let before = [page(&s, "A8:C31"), page(&s, "A1:N6"), lowered(&s, "A8:C31")];
    let bytes = s.save_xlsx().unwrap();
    let r = SheetSession::load_xlsx(&bytes).unwrap();
    assert_eq!(page(&r, "A8:C31"), before[0]);
    assert_eq!(page(&r, "A1:N6"), before[1]);
    assert_eq!(lowered(&r, "A8:C31"), before[2]);
}

#[test]
fn spill_anchors_are_written_as_excel_dynamic_arrays__feat__sheet_xlsx_roundtrip() {
    let mut s = typed_workbook();
    let bytes = s.save_xlsx().unwrap();
    let xml = sheet1(&bytes);

    // Excel's own form: the anchor is an array formula over the spill range,
    // flagged dynamic through the cell metadata; the range holds plain values.
    assert_eq!(
        cell_xml(&xml, "A8"),
        r#"<c r="A8" cm="1"><f t="array" ref="A8:A31">_xlfn.SEQUENCE(24)</f><v>1</v></c>"#
    );
    assert_eq!(
        cell_xml(&xml, "C8"),
        r#"<c r="C8" cm="1"><f t="array" ref="C8:C31">_xlfn.SEQUENCE(24,1,620,13)</f><v>620</v></c>"#
    );
    assert_eq!(cell_xml(&xml, "A9"), r#"<c r="A9"><v>2</v></c>"#);
    assert!(cell_xml(&xml, "M1").contains(r#"<f t="array" ref="M1:O1">TRANSPOSE(H1:H3)</f>"#));
    assert!(cell_xml(&xml, "Y1").contains(r#"<f t="array" ref="Y1:AA2">_xlfn.SEQUENCE(2,3)</f>"#));
    assert!(
        cell_xml(&xml, "J1").contains(r#"<f t="array" ref="J1:J6">_xlfn._xlws.SORT(H1:H6)</f>"#)
    );
    // A blocked or one-cell spill is a dynamic array over its anchor alone.
    // Its `#SPILL!` is cached as `#VALUE!`, as Excel caches it: Excel refuses
    // to open a worksheet whose `<v>` holds `#SPILL!`.
    assert_eq!(
        cell_xml(&xml, "S1"),
        r#"<c r="S1" t="e" cm="1"><f t="array" ref="S1">_xlfn.SEQUENCE(3)</f><v>#VALUE!</v></c>"#
    );
    assert!(!xml.contains("#SPILL!"), "{xml}");
    assert!(cell_xml(&xml, "W1").contains(r#"cm="1"><f t="array" ref="W1">"#));
    // A formula that does not spill stays a plain formula.
    assert_eq!(
        cell_xml(&xml, "U3"),
        r#"<c r="U3"><f>SUM(H1:H6)</f><v>29</v></c>"#
    );

    let meta = part(&bytes, "xl/metadata.xml").expect("the dynamic-array metadata part");
    assert!(meta.contains(r#"<metadataType name="XLDAPR""#), "{meta}");
    assert!(meta.contains(r#"fDynamic="1""#), "{meta}");
    assert!(meta.contains(r#"<cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata>"#));
    let ct = part(&bytes, "[Content_Types].xml").unwrap();
    assert!(
        ct.contains(r#"<Override PartName="/xl/metadata.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/>"#),
        "{ct}"
    );
    let rels = part(&bytes, "xl/_rels/workbook.xml.rels").unwrap();
    assert!(
        rels.contains(r#"Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata" Target="metadata.xml""#),
        "{rels}"
    );

    // Saving again reuses the part instead of growing it.
    let again = SheetSession::load_xlsx(&bytes)
        .unwrap()
        .save_xlsx()
        .unwrap();
    assert_eq!(
        part(&again, "xl/metadata.xml").as_deref(),
        Some(meta.as_str())
    );
}

/// The cells of a workbook Excel computed and saved, read back through
/// `SheetSession`: every cell must show what Excel cached (the blocked spill
/// excepted — Excel caches its `#SPILL!` as a rich value over `#VALUE!`).
fn excel_cached(bytes: &[u8]) -> Vec<((u32, u32), sheet_core::CellValue)> {
    let doc = sheet_xlsx::XlsxDocument::open(bytes).unwrap();
    doc.model.sheets[0]
        .cells
        .iter()
        .map(|(k, c)| (*k, c.value.clone()))
        .collect()
}

fn assert_matches_excel(
    s: &SheetSession,
    cached: &[((u32, u32), sheet_core::CellValue)],
    when: &str,
) {
    let ws = &s.model().sheets[0];
    let mut wrong = Vec::new();
    for ((r, c), want) in cached {
        let a1 = format!("{}{}", sheet_core::col_to_a1(*c), r + 1);
        let got = ws.cell(*r, *c).map(|c| c.value.clone()).unwrap_or_default();
        let want = if a1 == "S1" {
            sheet_core::CellValue::Error(sheet_core::value::CellError::Spill)
        } else {
            want.clone()
        };
        if got != want {
            wrong.push(format!("{a1}: {got:?}, Excel {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{when}: {wrong:#?}");
}

#[test]
fn excels_own_dynamic_arrays_load_and_round_trip__feat__sheet_xlsx_roundtrip() {
    let bytes = corpus("xlsx-dynamic", "dynamic-arrays.excel.xlsx");
    let cached = excel_cached(&bytes);
    assert!(
        cached.len() > 100,
        "the fixture carries Excel's cached spills"
    );
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    assert_matches_excel(&s, &cached, "after load");

    // An edit elsewhere re-encodes the sheet from the model.
    set(&mut s, "AC40", "1");
    let out = s.save_xlsx().unwrap();
    let xml = sheet1(&out);
    for (anchor, area) in [
        ("A8", "A8:A31"),
        ("B8", "B8:B31"),
        ("E1", "E1:E4"),
        ("J1", "J1:J6"),
        ("K1", "K1:K4"),
        ("L1", "L1:L4"),
        ("M1", "M1:O1"),
        ("Q1", "Q1:Q6"),
        ("R1", "R1:R3"),
        ("S1", "S1"),
        ("U1", "U1"),
        ("W1", "W1"),
        ("Y1", "Y1:AA2"),
    ] {
        let c = cell_xml(&xml, anchor);
        assert!(
            c.contains(r#" cm="1""#) && c.contains(&format!(r#"<f t="array" ref="{area}">"#)),
            "{anchor} is written back as a dynamic array over {area}: {c}"
        );
    }
    assert_eq!(
        part(&out, "xl/metadata.xml"),
        part(&bytes, "xl/metadata.xml"),
        "Excel's metadata part is reused verbatim"
    );

    let mut r = SheetSession::load_xlsx(&out).unwrap();
    assert_matches_excel(&r, &cached, "after save + reload");

    // Excel's spills reload LIVE: a source change re-sizes the FILTER and
    // re-sorts the SORT.
    set(&mut r, "H4", "7");
    assert_eq!(
        show(&r, "L5"),
        "4",
        "FILTER(H1:H6,H1:H6>3) grew to five rows"
    );
    assert_eq!(show(&r, "J6"), "9");
    assert_eq!(show(&r, "J1"), "2");
}

/// A re-encoded worksheet keeps its root's namespace declarations: the
/// captured elements written back verbatim use them (Excel's
/// `<sheetFormatPr x14ac:dyDescent=…>`), and with the prefix undeclared the
/// part is not XML — Excel refused to open any Excel workbook Paged had
/// edited.
#[test]
fn a_re_encoded_excel_worksheet_keeps_its_namespace_declarations__feat__sheet_xlsx_roundtrip() {
    let bytes = corpus("xlsx-dynamic", "dynamic-arrays.excel.xlsx");
    let original = sheet1(&bytes);
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    set(&mut s, "AC40", "1");
    let xml = sheet1(&s.save_xlsx().unwrap());
    let root = |x: &str| {
        x[x.find("<worksheet").unwrap()..]
            .split('>')
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(root(&xml), root(&original));
    assert!(
        xml.contains("x14ac:dyDescent"),
        "the captured element is still there"
    );
    assert!(root(&xml)
        .contains(r#"xmlns:x14ac="http://schemas.microsoft.com/office/spreadsheetml/2009/9/ac""#));
}

#[test]
fn a_legacy_cse_array_stays_a_cse_array__feat__sheet_xlsx_roundtrip() {
    let bytes = corpus("xlsx-dynamic", "dynamic-arrays.excel.xlsx");
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    // A typed spill next to the file's dynamic arrays, then a CSE array the
    // file never had cannot be typed — so check the converse on a workbook
    // that has one: the POI matrix workbook's CSE arrays keep no `cm`.
    set(&mut s, "AC40", "=SEQUENCE(2)");
    let out = s.save_xlsx().unwrap();
    assert!(cell_xml(&sheet1(&out), "AC40").contains(r#"cm="1"><f t="array" ref="AC40:AC41">"#));

    let poi = corpus("xlsx-recalc", "MatrixFormulaEvalTestData.xls.xlsx");
    let mut m = SheetSession::load_xlsx(&poi).unwrap();
    m.set_cell(0, 2000, 30, "1").unwrap();
    let saved = m.save_xlsx().unwrap();
    let xml = sheet1(&saved);
    assert!(
        xml.contains(r#"<f t="array" ref="#),
        "the workbook has CSE arrays"
    );
    assert!(!xml.contains(" cm="), "no CSE array became a dynamic one");
    assert!(part(&saved, "xl/metadata.xml").is_none());
}

/// A workbook whose metadata part has only a rich-value type (Excel writes
/// one for an error with details): the dynamic-array type is appended, the
/// existing records keep their indices.
#[test]
fn a_metadata_part_without_the_dynamic_type_gains_it__feat__sheet_xlsx_roundtrip() {
    let bytes = corpus("xlsx-recalc", "LookupFunctionsTestCaseData.xls.xlsx");
    let before = part(&bytes, "xl/metadata.xml").unwrap();
    assert!(!before.contains("XLDAPR"));
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    s.set_cell(0, 2000, 30, "=SEQUENCE(3)").unwrap();
    let out = s.save_xlsx().unwrap();
    let meta = part(&out, "xl/metadata.xml").unwrap();
    assert!(
        meta.contains(r#"<metadataTypes count="2"><metadataType name="XLRICHVALUE""#),
        "the existing type keeps index 1: {meta}"
    );
    assert!(meta.contains(r#"<metadataType name="XLDAPR""#), "{meta}");
    assert!(
        meta.contains(r#"<cellMetadata count="1"><bk><rc t="2" v="0"/></bk></cellMetadata>"#),
        "{meta}"
    );
    assert!(meta.contains(r#"<valueMetadata count="2"><bk><rc t="1" v="0"/></bk>"#));
    let xml = sheet1(&out);
    let anchor = cell_xml(&xml, "AE2001");
    assert!(anchor.contains(r#" cm="1""#), "{anchor}");
    let r = SheetSession::load_xlsx(&out).unwrap();
    assert_eq!(r.get_cell_display(0, 2002, 30), "3");
}

/// A workbook written before the dynamic-array encoding: each anchor a plain
/// `<f>` and its spill range plain values. Excel reads such a file as legacy
/// formulas next to literal values (the same numbers); Paged reads the cells
/// that hold exactly what the anchor spills as its cached spill. A cell that
/// holds something else still blocks.
#[test]
fn spills_saved_without_array_metadata_read_back_as_spills__feat__sheet_calc_spill() {
    let rows: String = [
        r#"<row r="1"><c r="A1"><f>_xlfn.SEQUENCE(4)</f><v>1</v></c><c r="B1"><f>_xlfn.SEQUENCE(3,1,10,10)</f><v>10</v></c><c r="C1"><f>_xlfn.SEQUENCE(3)</f><v>1</v></c><c r="D1"><f>SUM(_xlfn.ANCHORARRAY(A1))</f><v>10</v></c></row>"#,
        r#"<row r="2"><c r="A2"><v>2</v></c><c r="B2"><v>20</v></c><c r="C2"><v>2</v></c></row>"#,
        r#"<row r="3"><c r="A3"><v>3</v></c><c r="B3"><v>30</v></c><c r="C3" t="inlineStr"><is><t>typed</t></is></c></row>"#,
        r#"<row r="4"><c r="A4"><v>4</v></c></row>"#,
    ]
    .concat();
    let bytes = minimal_xlsx(&rows);
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    assert_eq!(
        grid(&s, 4, 4),
        [
            "A1=1",
            "B1=10",
            "C1=#SPILL!",
            "D1=10",
            "A2=2",
            "B2=20",
            "C2=2",
            "A3=3",
            "B3=30",
            "C3=typed",
            "A4=4"
        ]
    );
    // A1's column is its spill now: clearing the anchor clears it.
    set(&mut s, "A1", "");
    assert_eq!(show(&s, "A4"), "");
    // …and the next save writes the anchors in Excel's form.
    let out = s.save_xlsx().unwrap();
    assert!(cell_xml(&sheet1(&out), "B1").contains(r#"cm="1"><f t="array" ref="B1:B3">"#));
}

fn minimal_xlsx(sheet_data: &str) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        let mut add = |name: &str, body: &str| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        add(
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        );
        add(
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        );
        add(
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        );
        add(
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        );
        add(
            "xl/worksheets/sheet1.xml",
            &format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData>{sheet_data}</sheetData></worksheet>"#
            ),
        );
        zip.finish().unwrap();
    }
    buf
}
