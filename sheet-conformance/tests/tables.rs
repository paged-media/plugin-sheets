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

//! Structured-table conformance (spec §6.4 / §11, T1 tables track). Drives the
//! FROZEN public surfaces end-to-end: `sheet_parser` (parse/print/extract/
//! rewrite of structured refs), `sheet_calc::Engine` (resolution + reflow), and
//! `sheet_xlsx::XlsxDocument` (the `table` part + round-trip preservation).
//! Test-fn names are the `registry/features/tables.yaml` pointers the coverage
//! gate (§12.2) greps:
//! - `sheet_table_model`                  (sheet.table.model)
//! - `sheet_table_xlsx_part`              (sheet.table.xlsx-part)
//! - `sheet_table_structured_ref_parse`   (sheet.table.structured-ref.parse)
//! - `sheet_table_structured_ref_eval`    (sheet.table.structured-ref.eval)
//! - `sheet_table_structured_ref_print`   (sheet.table.structured-ref.print)
//! - `sheet_table_rewrite`                (sheet.table.rewrite)

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use sheet_calc::{Engine, EngineConfig};
use sheet_core::ast::{Expr, StructuredRef, TableArea};
use sheet_core::{CellRef, CellValue, NameId, RangeRef, SheetId, SheetModel, Table};
use sheet_parser::{extract_refs, parse, print, rewrite, Edit, ParseCtx, SheetNames};
use sheet_xlsx::XlsxDocument;

// ── parse/print context ─────────────────────────────────────────────────────

/// A tiny parse/print context: one sheet "Sheet1" (home id 0), no names.
struct Ctx;

impl ParseCtx for Ctx {
    fn sheet_id(&self, name: &str) -> Option<SheetId> {
        (name == "Sheet1").then_some(0)
    }
    fn name_id(&self, _name: &str) -> Option<NameId> {
        None
    }
    fn current_sheet(&self) -> SheetId {
        0
    }
}

impl SheetNames for Ctx {
    fn sheet_name(&self, id: SheetId) -> Option<&str> {
        (id == 0).then_some("Sheet1")
    }
}

fn p(s: &str) -> sheet_core::ast::Formula {
    parse(s, &Ctx).unwrap_or_else(|e| panic!("parse {s:?} failed: {e}"))
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

// ── corpus ──────────────────────────────────────────────────────────────────

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join("xlsx-corpus")
}

fn load(name: &str) -> Vec<u8> {
    let p = corpus_dir().join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read fixture {}: {e}", p.display()))
}

fn unzip_parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_owned();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        out.insert(name, data);
    }
    out
}

// ── shared model builder ─────────────────────────────────────────────────────

/// A `Sales` table on sheet 0 (A1:C4): header row + 3 data rows, columns
/// Region/Units/Total, data seeded with numbers (Units 10/20/30, Total
/// 100/200/300). Used by the eval + reflow tests.
fn table_model() -> SheetModel {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    let set = |m: &mut SheetModel, r: u32, c: u32, v: CellValue| {
        m.sheet_mut(0).unwrap().set_cell(
            r,
            c,
            sheet_core::Cell {
                value: v,
                ..Default::default()
            },
        );
    };
    set(&mut m, 0, 0, CellValue::from("Region"));
    set(&mut m, 0, 1, CellValue::from("Units"));
    set(&mut m, 0, 2, CellValue::from("Total"));
    for (i, (u, t)) in [(10.0, 100.0), (20.0, 200.0), (30.0, 300.0)]
        .iter()
        .enumerate()
    {
        let r = 1 + i as u32;
        set(&mut m, r, 0, CellValue::from("Row"));
        set(&mut m, r, 1, CellValue::Number(*u));
        set(&mut m, r, 2, CellValue::Number(*t));
    }
    m.sheet_mut(0).unwrap().tables.push(Table {
        name: "Sales".into(),
        range: RangeRef {
            start: cr(0, 0),
            end: cr(3, 2),
        },
        columns: vec!["Region".into(), "Units".into(), "Total".into()],
        header_row: true,
        totals_row: false,
        style_name: Some("TableStyleMedium2".into()),
    });
    m
}

fn val(e: &Engine, row: u32, col: u32) -> CellValue {
    e.model()
        .sheet(0)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

// ═════════════════════════════════════════════════════════════════════════════
// sheet.table.model — the Table type + name/column resolution helpers.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn sheet_table_model() {
    let m = table_model();
    // resolve_table is workbook-scoped + case-insensitive.
    let (sid, t) = m.resolve_table("sales").expect("table resolves");
    assert_eq!(sid, 0);
    assert_eq!(t.name.as_str(), "Sales");
    assert!(t.header_row && !t.totals_row);
    // column_index is a case-insensitive offset from the range's left edge.
    assert_eq!(t.column_index("Region"), Some(0));
    assert_eq!(t.column_index("units"), Some(1));
    assert_eq!(t.column_index("TOTAL"), Some(2));
    assert_eq!(t.column_index("nope"), None);
    // The full extent includes the header row (row 0).
    assert_eq!(t.range.normalized().rows(), 4);
    // An unknown table name resolves to None.
    assert!(m.resolve_table("ghost").is_none());
}

// ═════════════════════════════════════════════════════════════════════════════
// sheet.table.xlsx-part — parse + preserve the xl/tables/tableN.xml part.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn sheet_table_xlsx_part() {
    let bytes = load("07-tables.xlsx");
    let doc = XlsxDocument::open(&bytes).unwrap();

    // The table part parsed into the model.
    let ws = doc.model.sheet(0).unwrap();
    assert_eq!(ws.tables.len(), 1, "Sales table modeled");
    let t = &ws.tables[0];
    assert_eq!(t.name.as_str(), "Sales");
    assert!(t.header_row && !t.totals_row);
    let cols: Vec<&str> = t.columns.iter().map(|c| c.as_str()).collect();
    assert_eq!(cols, vec!["Region", "Units", "Total"]);
    // ref="A1:C4" anchored to sheet 0.
    let r = t.range.normalized();
    assert_eq!(
        (r.start.row, r.start.col, r.end.row, r.end.col),
        (0, 0, 3, 2)
    );
    assert_eq!(r.start.sheet, 0);

    // The structured-ref formula text was captured (sheet-xlsx never parses it).
    assert_eq!(
        doc.formula_texts.get(&(0, 0, 4)).map(String::as_str),
        Some("SUM(Sales[Units])")
    );
    // workbook-scoped resolution works through the loaded model.
    assert!(doc.model.resolve_table("Sales").is_some());

    // ── round-trip preservation (spec §10.2): the table part survives a
    //    zero-edit save byte-identical, and the worksheet's <tableParts> too.
    let orig = unzip_parts(&bytes);
    let out = doc.save().unwrap();
    let saved = unzip_parts(&out);
    let table_part = "xl/tables/table1.xml";
    assert_eq!(
        orig.get(table_part),
        saved.get(table_part),
        "table part not byte-identical after round-trip"
    );
    // The worksheet still references the table (the <tableParts> child + the
    // sheet .rels both survive).
    let ws_xml = String::from_utf8(saved.get("xl/worksheets/sheet1.xml").unwrap().clone()).unwrap();
    assert!(
        ws_xml.contains("tableParts"),
        "tableParts lost on round-trip"
    );
    assert!(saved.contains_key("xl/worksheets/_rels/sheet1.xml.rels"));

    // Re-opening the saved bytes still models the table.
    let doc2 = XlsxDocument::open(&out).unwrap();
    assert_eq!(doc2.model.sheet(0).unwrap().tables.len(), 1);
    assert!(doc2.model.resolve_table("Sales").is_some());
}

// ═════════════════════════════════════════════════════════════════════════════
// sheet.table.structured-ref.parse — every structured-ref form parses.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn sheet_table_structured_ref_parse() {
    // Bare column (default Data area).
    assert_eq!(
        p("Table1[Region]").root,
        Expr::StructuredRef(StructuredRef {
            table: "Table1".into(),
            area: TableArea::Data,
            col_start: Some("Region".into()),
            col_end: None,
        })
    );
    // Each canonical form parses to a StructuredRef.
    for s in [
        "Table1[Region]",
        "Table1[[#All]]",
        "Table1[[#Headers]]",
        "Table1[[#Totals]]",
        "Table1[[#Data],[Units]]",
        "Table1[[#Headers],[Region]]",
        "Table1[[Region]:[Total]]",
        "Table1[[#Data],[Region]:[Total]]",
        "Table1[[#This Row],[Region]]",
        "Table1[#Totals]",
        "[@Region]",
        "Sales[[Net Amount]]",
    ] {
        assert!(
            matches!(p(s).root, Expr::StructuredRef(_)),
            "{s:?} did not parse to a StructuredRef"
        );
    }

    // ThisRow forms: the `@` shorthand carries an empty table name; the
    // `[[#This Row],[Col]]` form keeps its name.
    let Expr::StructuredRef(at) = p("[@Units]").root else {
        panic!()
    };
    assert_eq!(at.area, TableArea::ThisRow);
    assert!(at.table.is_empty());
    assert_eq!(at.col_start.as_deref(), Some("Units"));

    let Expr::StructuredRef(tr) = p("Sales[[#This Row],[Units]]").root else {
        panic!()
    };
    assert_eq!(tr.area, TableArea::ThisRow);
    assert_eq!(tr.table.as_str(), "Sales");

    // A column span carries both endpoints.
    let Expr::StructuredRef(span) = p("Sales[[Region]:[Total]]").root else {
        panic!()
    };
    assert_eq!(span.col_start.as_deref(), Some("Region"));
    assert_eq!(span.col_end.as_deref(), Some("Total"));

    // extract_refs records the table name as a dependency (empty for `[@…]`).
    let set = extract_refs(&p("SUM(Sales[Units])"));
    assert_eq!(set.tables.len(), 1);
    assert_eq!(set.tables[0].as_str(), "Sales");
    assert!(extract_refs(&p("[@Units]")).tables.is_empty());

    // Malformed structured refs are parse errors.
    assert!(parse("Sales[Region", &Ctx).is_err());
    assert!(parse("Sales[[A],[B],[C]]", &Ctx).is_err());
}

// ═════════════════════════════════════════════════════════════════════════════
// sheet.table.structured-ref.eval — resolve table + column + #-area, reflow.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn sheet_table_structured_ref_eval() {
    // ── A `Table1[Col]` SUM over the data body (excludes the header row).
    let mut e = Engine::new(table_model(), EngineConfig::default());
    e.enter(0, 0, 4, "=SUM(Sales[Units])").unwrap(); // E1
    assert_eq!(val(&e, 0, 4), CellValue::Number(60.0));
    // The Total column sums to 600.
    e.enter(0, 1, 4, "=SUM(Sales[Total])").unwrap(); // E2
    assert_eq!(val(&e, 1, 4), CellValue::Number(600.0));

    // ── ThisRow `[@Col]`: a helper column adjacent to the table, row-aligned.
    //    Row index 1 is the FIRST data row (Units=10); `[@Units]*10` → 100.
    e.enter(0, 1, 3, "=[@Units]*10").unwrap(); // D2 (1st data row)
    assert_eq!(val(&e, 1, 3), CellValue::Number(100.0));
    //    Row index 2 is the 2nd data row (Units=20); `[@Units]*10` → 200.
    e.enter(0, 2, 3, "=[@Units]*10").unwrap();
    assert_eq!(val(&e, 2, 3), CellValue::Number(200.0));

    // ── A write INTO the table reflows the structured-ref dependents (the dep
    //    edge resolves the table NAME to its box). Set the 1st data Units cell
    //    (row index 1, col 1) to 99.
    e.enter(0, 1, 1, "99").unwrap();
    assert_eq!(val(&e, 0, 4), CellValue::Number(149.0)); // SUM = 99+20+30
    assert_eq!(val(&e, 1, 3), CellValue::Number(990.0)); // [@Units]*10 = 99*10

    // ── #-area resolution: a header cell read.
    let mut e2 = Engine::new(table_model(), EngineConfig::default());
    e2.enter(0, 5, 5, "=Sales[[#Headers],[Units]]").unwrap();
    assert_eq!(val(&e2, 5, 5), CellValue::from("Units"));

    // ── ThisRow outside the table data body is #VALUE!.
    let mut e3 = Engine::new(table_model(), EngineConfig::default());
    e3.enter(0, 9, 9, "=[@Units]").unwrap(); // row 9 is below the table
    assert_eq!(
        val(&e3, 9, 9),
        CellValue::Error(sheet_core::CellError::Value)
    );

    // ── Unknown table → #NAME?, unknown column → #REF!.
    let mut e4 = Engine::new(table_model(), EngineConfig::default());
    e4.enter(0, 0, 4, "=SUM(Ghost[Units])").unwrap();
    assert_eq!(
        val(&e4, 0, 4),
        CellValue::Error(sheet_core::CellError::Name)
    );
    e4.enter(0, 1, 4, "=SUM(Sales[Missing])").unwrap();
    assert_eq!(val(&e4, 1, 4), CellValue::Error(sheet_core::CellError::Ref));
}

// ═════════════════════════════════════════════════════════════════════════════
// sheet.table.structured-ref.print — parse→print canonical fixpoint.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn sheet_table_structured_ref_print() {
    let pr = |s: &str| print(&p(s), 0, &Ctx);

    // Canonical spellings are string fixpoints.
    for s in [
        "Table1[Region]",
        "Table1[[#All]]",
        "Table1[[#Headers]]",
        "Table1[[#Totals]]",
        "Table1[[#Headers],[Region]]",
        "Table1[[Region]:[Total]]",
        "Table1[[#This Row],[Region]]",
        "Sales[[Net Amount]]",
        "[@Region]",
    ] {
        assert_eq!(pr(s), s, "fixpoint for {s}");
    }

    // Non-canonical (valid) spellings normalize to the canonical form, then
    // become string fixpoints, and the AST is unchanged across the normalize
    // (an AST-level fixpoint).
    for (input, canonical) in [
        ("Table1[[#Data],[Units]]", "Table1[Units]"),
        ("Sales[Net Amount]", "Sales[[Net Amount]]"),
        ("Table1[[Region]]", "Table1[Region]"),
    ] {
        assert_eq!(pr(input), canonical, "canonical print for {input}");
        assert_eq!(pr(canonical), canonical, "fixpoint for {canonical}");
        assert_eq!(p(input).root, p(canonical).root, "AST fixpoint for {input}");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// sheet.table.rewrite — structured refs survive row/col insert/delete unchanged.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn sheet_table_rewrite() {
    // A structured ref is NAME-anchored: a structural edit leaves it untouched,
    // unlike an A1 reference which shifts. We rewrite a formula that mixes both.
    let f = p("SUM(Sales[Units])+A10");
    let edits = [
        Edit::InsertRows {
            sheet: 0,
            at: 0,
            n: 5,
        },
        Edit::DeleteRows {
            sheet: 0,
            at: 0,
            n: 2,
        },
        Edit::InsertCols {
            sheet: 0,
            at: 0,
            n: 3,
        },
        Edit::DeleteCols {
            sheet: 0,
            at: 0,
            n: 1,
        },
    ];
    for edit in &edits {
        let g = rewrite(&f, edit);
        // Drill into the `SUM(Sales[Units]) + A10` tree: the SUM arg (the
        // structured ref) is unchanged; only the A1 ref `A10` may shift.
        let Expr::Binary(_, lhs, _) = &g.root else {
            panic!("expected a binary +, got {:?}", g.root)
        };
        let Expr::Func(_, args) = lhs.as_ref() else {
            panic!("expected SUM(...) on the lhs")
        };
        assert_eq!(
            args[0],
            Expr::StructuredRef(StructuredRef {
                table: "Sales".into(),
                area: TableArea::Data,
                col_start: Some("Units".into()),
                col_end: None,
            }),
            "structured ref must survive {edit:?} unchanged"
        );
    }

    // A pure structured ref round-trips identically through every edit.
    let pure = p("Sales[[#Headers],[Region]:[Total]]");
    for edit in &edits {
        assert_eq!(
            rewrite(&pure, edit).root,
            pure.root,
            "name-anchored ref shifted on {edit:?}"
        );
    }
}
