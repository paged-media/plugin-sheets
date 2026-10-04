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
// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
#![allow(non_snake_case)]

//! Regression tests for defects the Excel oracle lanes found and Wave 3b
//! fixed (the oracle lanes themselves pin the agreement counts; these pin
//! the mechanism in a form that names it).

use sheet_js::core::SheetSession;

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../corpus/xlsx-recalc")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// POI's SharedFormulaTest: `DY2` is the master `DZ2*2` of `DY2:DY8`, the
/// members carry only `si="1"`. Each member must evaluate ITS row, and a
/// dirty re-encode (any edit on the sheet) must write each member's own
/// formula — not the master text, which would point every row at row 2.
#[test]
fn shared_formula_members_shift_and_survive_a_dirty_save__feat__sheet_xlsx_roundtrip() {
    let mut s = SheetSession::load_xlsx(&fixture("SharedFormulaTest.xls.xlsx")).unwrap();
    for r in 2..=8u32 {
        assert_eq!(
            s.get_cell_input(0, r - 1, 128),
            format!("=DZ{r}*2"),
            "DY{r} after load"
        );
    }
    let before: Vec<String> = (1..8).map(|r| s.get_cell_display(0, r, 128)).collect();
    s.set_cell(0, 20, 0, "1").unwrap(); // dirties sheet 1
    let bytes = s.save_xlsx().unwrap();
    let r = SheetSession::load_xlsx(&bytes).unwrap();
    for row in 2..=8u32 {
        assert_eq!(
            r.get_cell_input(0, row - 1, 128),
            format!("=DZ{row}*2"),
            "DY{row} after save + reload"
        );
    }
    let after: Vec<String> = (1..8).map(|row| r.get_cell_display(0, row, 128)).collect();
    assert_eq!(before, after);
}

/// A minimal package: `sheets` are (name, `<sheetData>` inner XML, extra XML
/// after `</sheetData>`); `defined` is the `<definedNames>` inner XML;
/// `parts` are extra (path, body, content type, owning sheet index) parts
/// related from their sheet (tables).
fn pkg(
    defined: &str,
    sheets: &[(&str, &str, &str)],
    parts: &[(&str, &str, &str, usize)],
) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        let mut add = |name: &str, body: String| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        const WS: &str =
            "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
        let mut ct = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
        );
        for i in 0..sheets.len() {
            ct += &format!(
                r#"<Override PartName="/xl/worksheets/sheet{}.xml" ContentType="{WS}"/>"#,
                i + 1
            );
        }
        for (path, _, ctype, _) in parts {
            ct += &format!(r#"<Override PartName="/{path}" ContentType="{ctype}"/>"#);
        }
        ct += "</Types>";
        add("[Content_Types].xml", ct);
        add("_rels/.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into());
        let mut wb = String::from(
            r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
        );
        let mut rels = String::from(
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (i, (name, _, _)) in sheets.iter().enumerate() {
            wb += &format!(
                r#"<sheet name="{name}" sheetId="{}" r:id="rId{}"/>"#,
                i + 1,
                i + 1
            );
            rels += &format!(
                r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{}.xml"/>"#,
                i + 1,
                i + 1
            );
        }
        wb += "</sheets>";
        if !defined.is_empty() {
            wb += &format!("<definedNames>{defined}</definedNames>");
        }
        wb += "</workbook>";
        rels += "</Relationships>";
        add("xl/workbook.xml", wb);
        add("xl/_rels/workbook.xml.rels", rels);
        for (i, (_, data, extra)) in sheets.iter().enumerate() {
            let mut srels = String::new();
            let mut tparts = String::new();
            for (j, (path, _, _, owner)) in parts.iter().enumerate() {
                if *owner == i {
                    let target = path.trim_start_matches("xl/");
                    srels += &format!(
                        r#"<Relationship Id="rT{j}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../{target}"/>"#
                    );
                    tparts += &format!(r#"<tablePart r:id="rT{j}"/>"#);
                }
            }
            let tp = if tparts.is_empty() {
                String::new()
            } else {
                format!("<tableParts>{tparts}</tableParts>")
            };
            add(
                &format!("xl/worksheets/sheet{}.xml", i + 1),
                format!(
                    r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData>{data}</sheetData>{extra}{tp}</worksheet>"#
                ),
            );
            if !srels.is_empty() {
                add(
                    &format!("xl/worksheets/_rels/sheet{}.xml.rels", i + 1),
                    format!(
                        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{srels}</Relationships>"#
                    ),
                );
            }
        }
        for (path, body, _, _) in parts {
            add(path, body.to_string());
        }
        zip.finish().unwrap();
    }
    buf
}

fn num(r: u32, c: &str, v: f64) -> String {
    format!(r#"<c r="{c}{r}"><v>{v}</v></c>"#)
}

fn fml(r: u32, c: &str, f: &str) -> String {
    format!(r#"<c r="{c}{r}"><f>{f}</f></c>"#)
}

/// Defined names whose target is a CONSTANT, a FORMULA, a reference-valued
/// formula (OFFSET), an array constant, and a sheet-scoped name shadowing a
/// workbook one. Every one evaluated to `#NAME?` (4,268 corpus cells): only
/// plain-reference names were resolved.
#[test]
fn defined_names_with_formula_and_constant_targets_evaluate__feat__sheet_names_define() {
    let data = [
        format!(
            "<row r=\"1\">{}{}</row>",
            num(1, "A", 1.0),
            fml(1, "B", "Rate*2")
        ),
        format!(
            "<row r=\"2\">{}{}</row>",
            num(2, "A", 2.0),
            fml(2, "B", "Total")
        ),
        format!(
            "<row r=\"3\">{}{}</row>",
            num(3, "A", 3.0),
            fml(3, "B", "SUM(Dyn)")
        ),
        format!("<row r=\"4\">{}</row>", fml(4, "B", "SUM(Arr)")),
        format!("<row r=\"5\">{}</row>", fml(5, "B", "Total+Rate+Local")),
        format!("<row r=\"6\">{}</row>", fml(6, "B", "Chain")),
    ]
    .concat();
    let names = r#"<definedName name="Rate">0.5</definedName><definedName name="Total">SUM(Sheet1!$A$1:$A$3)</definedName><definedName name="Dyn">OFFSET(Sheet1!$A$1,0,0,3,1)</definedName><definedName name="Arr">{1,2,3}</definedName><definedName name="Local">100</definedName><definedName name="Local" localSheetId="0">7</definedName><definedName name="Chain">Total*Rate</definedName>"#;
    let bytes = pkg(names, &[("Sheet1", &data, "")], &[]);
    let mut s = SheetSession::load_xlsx(&bytes).unwrap();
    let col_b: Vec<String> = (0..6).map(|r| s.get_cell_display(0, r, 1)).collect();
    assert_eq!(col_b, ["1", "6", "6", "6", "13.5", "3"], "names after load");
    // A name's precedents are dependencies: editing A1 reaches Total, Dyn,
    // and the name-of-a-name Chain.
    s.set_cell(0, 0, 0, "10").unwrap();
    let col_b: Vec<String> = (0..6).map(|r| s.get_cell_display(0, r, 1)).collect();
    assert_eq!(
        col_b,
        ["1", "15", "15", "6", "22.5", "7.5"],
        "names after an edit"
    );
}

/// A name defined in terms of itself must not overflow the stack.
#[test]
fn self_referencing_defined_name_terminates__feat__sheet_names_define() {
    let data = format!("<row r=\"1\">{}</row>", fml(1, "A", "Loop+1"));
    let names = r#"<definedName name="Loop">Loop+1</definedName>"#;
    let s = SheetSession::load_xlsx(&pkg(names, &[("Sheet1", &data, "")], &[])).unwrap();
    assert!(s.get_cell_display(0, 0, 0).starts_with('#'));
}

/// A relative reference in a stored name is relative to A1 and wraps around
/// the grid: Excel writes "the cell above" as `$A1048576` (Office templates
/// use it for running balances). Read from row 3 it is `$A2`.
#[test]
fn relative_defined_name_rebases_on_the_referring_cell__feat__sheet_names_define() {
    let data = [
        format!("<row r=\"1\">{}</row>", num(1, "A", 5.0)),
        format!(
            "<row r=\"2\">{}{}</row>",
            num(2, "A", 7.0),
            fml(2, "B", "Above*10")
        ),
        format!(
            "<row r=\"3\">{}{}</row>",
            num(3, "A", 9.0),
            fml(3, "B", "Above+Same")
        ),
    ]
    .concat();
    let names = r#"<definedName name="Above">Sheet1!$A1048576</definedName><definedName name="Same">IF(TRUE,Sheet1!$A1)</definedName>"#;
    let mut s = SheetSession::load_xlsx(&pkg(names, &[("Sheet1", &data, "")], &[])).unwrap();
    assert_eq!(s.get_cell_display(0, 1, 1), "50");
    assert_eq!(s.get_cell_display(0, 2, 1), "16");
    s.set_cell(0, 1, 0, "1").unwrap(); // A2: B3 reads it through `Above`
    assert_eq!(s.get_cell_display(0, 2, 1), "10");
}

/// ROWS/COLUMNS/ROW/COLUMN read only a reference's GEOMETRY, so a formula
/// that measures a range containing its own cell is not circular in Excel
/// (`=ROWS($B$1:B3)` in B3 is 3). The engine registered the range as a value
/// dependency and stored the cycle ruling `#REF!` — through a defined name
/// (`PlanYear = ROWS(Calc!$B$15:$B1)`) this voided 2,667 cells of one
/// Office template.
#[test]
fn geometry_functions_over_their_own_cell_are_not_cycles__feat__sheet_calc_engine() {
    let mut s = SheetSession::new();
    s.set_cell(0, 2, 1, "=ROWS($B$1:B3)").unwrap();
    s.set_cell(0, 2, 2, "=COLUMNS(A3:C3)+ROW(C3)+COLUMN(C1:C9)")
        .unwrap();
    assert_eq!(s.get_cell_display(0, 2, 1), "3");
    assert_eq!(s.get_cell_display(0, 2, 2), "9");
}

const TABLE_CT: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml";

/// A 3-row table `Items` (displayName; internal name `Table1`) over A1:C5:
/// header row 1, body rows 2-4, totals row 5. Columns `Item`, `Unit Price`,
/// `Qty`.
fn items_workbook(formulas: &[(&str, &str)]) -> Vec<u8> {
    let s = |r: u32, c: &str, t: &str| {
        format!(r#"<c r="{c}{r}" t="inlineStr"><is><t>{t}</t></is></c>"#)
    };
    let mut rows = vec![
        format!(
            "<row r=\"1\">{}{}{}</row>",
            s(1, "A", "Item"),
            s(1, "B", "Unit Price"),
            s(1, "C", "Qty")
        ),
        format!(
            "<row r=\"2\">{}{}{}</row>",
            s(2, "A", "pen"),
            num(2, "B", 2.0),
            num(2, "C", 3.0)
        ),
        format!(
            "<row r=\"3\">{}{}{}</row>",
            s(3, "A", "ink"),
            num(3, "B", 5.0),
            num(3, "C", 1.0)
        ),
        format!(
            "<row r=\"4\">{}{}{}</row>",
            s(4, "A", "pad"),
            num(4, "B", 4.0),
            num(4, "C", 2.0)
        ),
        format!(
            "<row r=\"5\">{}{}</row>",
            s(5, "A", "Total"),
            num(5, "C", 6.0)
        ),
    ];
    // Formulas go in column E, one row each from row 1.
    for (i, (_, f)) in formulas.iter().enumerate() {
        let r = i as u32 + 1;
        let cell = fml(
            r,
            "E",
            &f.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
        );
        if r <= 5 {
            rows[i] = rows[i].replace("</row>", &format!("{cell}</row>"));
        } else {
            rows.push(format!("<row r=\"{r}\">{cell}</row>"));
        }
    }
    let table = r#"<?xml version="1.0"?><table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Table1" displayName="Items" ref="A1:C5" totalsRowCount="1"><tableColumns count="3"><tableColumn id="1" name="Item"/><tableColumn id="2" name="Unit Price"/><tableColumn id="3" name="Qty"/></tableColumns></table>"#;
    pkg(
        "",
        &[("Sheet1", &rows.concat(), "")],
        &[("xl/tables/table1.xml", table, TABLE_CT, 0)],
    )
}

/// Structured references as Excel writes them (419 corpus cells were
/// unparsed or `#REF!`/`#NAME?`). Every expected value is what Excel shows.
#[test]
fn structured_references_resolve_like_excel__feat__sheet_table_structured() {
    let cases: &[(&str, &str)] = &[
        ("display name", "SUM(Items[Qty])"),
        ("spaced column", "SUM(Items[Unit Price])"),
        ("escaped bracket form", "SUM(Items[[Unit Price]])"),
        ("headers", "Items[[#Headers],[Qty]]"),
        ("totals", "Items[[#Totals],[Qty]]"),
        ("column span", "SUM(Items[[Unit Price]:[Qty]])"),
        ("all", "ROWS(Items[#All])"),
        ("empty brackets = data", "ROWS(Items[])"),
        ("data rows count", "COUNTA(Items[Item])"),
        ("headers row", "COUNTA(Items[#Headers])"),
    ];
    // Not modelled: two-area specifiers (`[[#Headers],[#Data],[Qty]]`) — the
    // frozen TableArea has no combined area; none occur in the corpus.
    let want = ["6", "11", "11", "Qty", "6", "17", "5", "3", "3", "3"];
    let s = SheetSession::load_xlsx(&items_workbook(cases)).unwrap();
    let got: Vec<String> = (0..cases.len() as u32)
        .map(|r| s.get_cell_display(0, r, 4))
        .collect();
    let wrong: Vec<String> = cases
        .iter()
        .zip(&got)
        .zip(want)
        .filter(|((_, g), w)| g.as_str() != *w)
        .map(|(((label, f), g), w)| format!("{label}: ={f} -> {g}, Excel {w}"))
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// `[@Col]` / `[[#This Row],[Col]]` inside the table's rows (a calculated
/// column) intersect with the formula's own row.
#[test]
fn this_row_references_intersect_the_formula_row__feat__sheet_table_structured() {
    let s = |r: u32, c: &str, t: &str| {
        format!(r#"<c r="{c}{r}" t="inlineStr"><is><t>{t}</t></is></c>"#)
    };
    let mut rows = vec![format!(
        "<row r=\"1\">{}{}{}</row>",
        s(1, "A", "Unit Price"),
        s(1, "B", "Qty"),
        s(1, "C", "Line")
    )];
    for (r, (p, q)) in [(2u32, (2.0, 3.0)), (3, (5.0, 1.0))] {
        let f = if r == 2 {
            "Items[[#This Row],[Unit Price]]*Items[[#This Row],[Qty]]"
        } else {
            "[@[Unit Price]]*[@Qty]"
        };
        rows.push(format!(
            "<row r=\"{r}\">{}{}{}</row>",
            num(r, "A", p),
            num(r, "B", q),
            fml(r, "C", f)
        ));
    }
    let table = r#"<?xml version="1.0"?><table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Table1" displayName="Items" ref="A1:C3"><tableColumns count="3"><tableColumn id="1" name="Unit Price"/><tableColumn id="2" name="Qty"/><tableColumn id="3" name="Line"/></tableColumns></table>"#;
    let bytes = pkg(
        "",
        &[("Sheet1", &rows.concat(), "")],
        &[("xl/tables/table1.xml", table, TABLE_CT, 0)],
    );
    let s = SheetSession::load_xlsx(&bytes).unwrap();
    assert_eq!(s.get_cell_display(0, 1, 2), "6");
    assert_eq!(s.get_cell_display(0, 2, 2), "5");
}

/// Operators and scalar functions over a range inside a function argument
/// evaluate element-wise (Excel 365 reads every formula so; CSE formulas
/// always did). Each was `#VALUE!`.
#[test]
fn array_arguments_evaluate_element_wise__feat__sheet_calc_spill() {
    let mut s = SheetSession::new();
    for (r, v) in ["1", "5", "3"].iter().enumerate() {
        s.set_cell(0, r as u32, 0, v).unwrap();
    }
    let cases = [
        ("=SUM(A1:A3*2)", "18"),
        ("=SUM(IF(A1:A3>1,A1:A3))", "8"),
        ("=SUMPRODUCT((A1:A3>1)*1)", "2"),
        ("=INDEX(A1:A3,MATCH(MAX(A1:A3*1),A1:A3,0))", "5"),
        ("=SUM(--(A1:A3>=3))", "2"),
        ("=MAX(LEN(A1:A3&\"xx\"))", "3"),
        ("=SUM(A1:A3*{1;10;100})", "351"),
    ];
    let mut wrong = Vec::new();
    for (i, (f, want)) in cases.iter().enumerate() {
        s.set_cell(0, i as u32, 3, f).unwrap();
        let got = s.get_cell_display(0, i as u32, 3);
        if got != *want {
            wrong.push(format!("{f} -> {got}, Excel {want}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Legacy array (CSE) formulas, `<f t="array" ref=…>`: evaluated
/// element-wise over their FIXED area — a one-cell area shows the block's
/// top-left; a larger area is filled (a scalar repeats, positions past the
/// block are `#N/A`) instead of spilling into the file's cached values
/// (`#SPILL!`). The type and area survive a dirty save.
#[test]
fn legacy_array_formulas_fill_their_area__feat__sheet_calc_spill() {
    let arr = |r: u32, c: &str, area: &str, f: &str, v: f64| {
        format!(r#"<c r="{c}{r}"><f t="array" ref="{area}">{f}</f><v>{v}</v></c>"#)
    };
    let data = [
        format!(
            "<row r=\"1\">{}{}{}{}{}</row>",
            num(1, "A", 1.0),
            num(1, "B", 2.0),
            arr(1, "C", "C1", "SUM(A1:A3*B1:B3)", 0.0),
            arr(1, "D", "D1:D4", "A1:A3*10", 0.0),
            arr(1, "E", "E1:E2", "SUM(A1:A3)", 0.0)
        ),
        format!(
            "<row r=\"2\">{}{}{}{}</row>",
            num(2, "A", 5.0),
            num(2, "B", 1.0),
            num(2, "D", 99.0),
            num(2, "E", 99.0)
        ),
        format!(
            "<row r=\"3\">{}{}{}</row>",
            num(3, "A", 3.0),
            num(3, "B", 4.0),
            num(3, "D", 99.0)
        ),
        format!("<row r=\"4\">{}</row>", num(4, "D", 99.0)),
    ]
    .concat();
    let mut s = SheetSession::load_xlsx(&pkg("", &[("Sheet1", &data, "")], &[])).unwrap();
    let look = |s: &SheetSession| -> Vec<String> {
        [(0, 2), (0, 3), (1, 3), (2, 3), (3, 3), (0, 4), (1, 4)]
            .iter()
            .map(|&(r, c)| s.get_cell_display(0, r, c))
            .collect()
    };
    let want = ["19", "10", "50", "30", "#N/A", "9", "9"];
    assert_eq!(look(&s), want, "after load");
    s.set_cell(0, 0, 0, "2").unwrap();
    assert_eq!(
        look(&s),
        ["21", "20", "50", "30", "#N/A", "10", "10"],
        "after an edit"
    );
    let bytes = s.save_xlsx().unwrap();
    let xml = {
        use std::io::Read;
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut x = String::new();
        z.by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut x)
            .unwrap();
        x
    };
    assert!(xml.contains(r#"<f t="array" ref="D1:D4">"#), "{xml}");
    let r = SheetSession::load_xlsx(&bytes).unwrap();
    assert_eq!(
        look(&r),
        ["21", "20", "50", "30", "#N/A", "10", "10"],
        "after save + reload"
    );
}

/// External-workbook references (`[1]Sheet1!A1`, 1,990 unparsed corpus
/// cells) parse and evaluate from the values Excel cached in the workbook's
/// externalLink parts — never by opening the source.
#[test]
fn external_workbook_refs_evaluate_from_the_cache__feat__sheet_xlsx_roundtrip() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/xlsx-corpus/10-extlink.xlsx");
    let mut s = SheetSession::load_xlsx(&std::fs::read(p).unwrap()).unwrap();
    // A1 = [1]Sheet1!A1 is now a live formula over the cached 42.
    assert!(s.get_cell_input(0, 0, 0).starts_with('='), "A1 parsed");
    assert_eq!(s.get_cell_display(0, 0, 0), "42");
    s.set_cell(0, 5, 5, "=[1]Sheet1!A1*2+'[1]Costs'!C3").unwrap();
    assert_eq!(s.get_cell_display(0, 5, 5), "87.5");
    s.set_cell(0, 6, 5, "=COUNTA([1]Sheet1!A1:B2)").unwrap();
    assert_eq!(s.get_cell_display(0, 6, 5), "4"); // 42, "hello", TRUE, #DIV/0!
    // Nothing about the shadow sheets reaches the saved workbook.
    let saved = s.save_xlsx().unwrap();
    let r = SheetSession::load_xlsx(&saved).unwrap();
    assert_eq!(r.list_sheets().len(), s.list_sheets().len());
    assert_eq!(r.get_cell_display(0, 5, 5), "87.5");
}
