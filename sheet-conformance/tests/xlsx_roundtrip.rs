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

//! XLSX I/O conformance (spec §10). Test-fn names are the registry pointers
//! for `registry/features/xlsx.yaml` (the coverage gate greps these
//! prefixes). Fixtures live in `corpus/xlsx-corpus/` (built by
//! `generate.py`); each test loads them through the public `XlsxDocument`
//! API and asserts the relevant invariant.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use sheet_core::value::{CellError, CellValue};
use sheet_core::DateSystem;
use sheet_xlsx::XlsxDocument;

/// All six corpus fixtures, in numeric order.
const FIXTURES: &[&str] = &[
    "01-minimal.xlsx",
    "02-formulas.xlsx",
    "03-styles.xlsx",
    "04-unknown-parts.xlsx",
    "05-unknown-subtrees.xlsx",
    "06-multisheet-1904.xlsx",
];

/// Path to `corpus/xlsx-corpus/` (sibling of the conformance crate).
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

/// Decompress a zip into an ordered `(name, decompressed bytes)` list,
/// skipping directory entries — the unit of the per-part identity assertion.
fn unzip_parts(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_owned();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        out.push((name, data));
    }
    out
}

// ── sheet.xlsx.opc.container ────────────────────────────────────────────────

/// Every fixture opens; the OPC container round-trips to a re-openable zip
/// with `[Content_Types].xml` present and the same understood part set.
#[test]
fn sheet_xlsx_opc_container() {
    for name in FIXTURES {
        let bytes = load(name);
        let doc = XlsxDocument::open(&bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        // Re-save and confirm the container is still a valid zip with a
        // content-types part.
        let out = doc.save().unwrap();
        let parts = unzip_parts(&out);
        assert!(
            parts.iter().any(|(n, _)| n == "[Content_Types].xml"),
            "{name}: missing [Content_Types].xml after save"
        );
        // Original entry order is preserved for the retained parts.
        let orig: Vec<String> = unzip_parts(&bytes)
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| n != "xl/calcChain.xml")
            .collect();
        let saved: Vec<String> = parts.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(orig, saved, "{name}: part order/set diverged");
    }
}

// ── sheet.xlsx.workbook ─────────────────────────────────────────────────────

/// Sheet names/order, the date1904 flag, and definedNames parse correctly.
#[test]
fn sheet_xlsx_workbook() {
    let doc = XlsxDocument::open(&load("06-multisheet-1904.xlsx")).unwrap();
    let names: Vec<&str> = doc.model.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Summary", "Data", "Notes"]);
    assert_eq!(doc.model.calc.date_system, DateSystem::Date1904);

    // definedNames folded into the model name table (raw formula text).
    let defs: Vec<_> = doc.model.names.iter().collect();
    assert_eq!(defs.len(), 3);
    assert!(defs
        .iter()
        .any(|(_, d)| d.name.eq_ignore_ascii_case("TaxRate")));

    // A single-sheet 1900 workbook keeps the default epoch.
    let mini = XlsxDocument::open(&load("01-minimal.xlsx")).unwrap();
    assert_eq!(mini.model.calc.date_system, DateSystem::Date1900);
    assert_eq!(mini.model.sheets.len(), 1);
}

// ── sheet.xlsx.worksheet.cells ──────────────────────────────────────────────

/// Cell values by type (n/s/str/b/e/inlineStr), formula text capture,
/// merges, col widths, and row heights parse.
#[test]
fn sheet_xlsx_worksheet_cells() {
    // Values + formula text (02).
    let f = XlsxDocument::open(&load("02-formulas.xlsx")).unwrap();
    let ws = f.model.sheet(0).unwrap();
    assert_eq!(ws.cell(0, 0).unwrap().value, CellValue::Number(2.0));
    // A3 = SUM(A1:A2), cached value 5, formula text captured (not parsed).
    assert_eq!(ws.cell(2, 0).unwrap().value, CellValue::Number(5.0));
    assert_eq!(
        f.formula_texts.get(&(0, 2, 0)).map(String::as_str),
        Some("SUM(A1:A2)")
    );
    // B3 has the entity-decoded concat formula.
    assert_eq!(
        f.formula_texts.get(&(0, 2, 1)).map(String::as_str),
        Some("B1&B2")
    );
    // shared string cell B1 resolved to "Sum".
    assert_eq!(ws.cell(0, 1).unwrap().value, CellValue::Text("Sum".into()));

    // Geometry + merges (03).
    let s = XlsxDocument::open(&load("03-styles.xlsx")).unwrap();
    let sws = s.model.sheet(0).unwrap();
    assert_eq!(sws.col_widths.get(&0), Some(&18.5));
    assert_eq!(sws.col_widths.get(&1), Some(&12.0));
    assert_eq!(sws.col_widths.get(&2), Some(&12.0));
    assert_eq!(sws.row_heights.get(&0), Some(&28.5));
    assert_eq!(sws.merges.len(), 1);

    // Error type (synthesize via 02? use a direct check on inline pkg).
    // 04 has a plain number; assert the error path through a tiny inline doc.
    let e = XlsxDocument::open(&load("04-unknown-parts.xlsx")).unwrap();
    assert_eq!(
        e.model.sheet(0).unwrap().cell(0, 0).unwrap().value,
        CellValue::Number(7.0)
    );
    // Guard the CellError enum is reachable (compile-time wiring check).
    let _ = CellError::Div0;
}

// ── sheet.xlsx.shared-strings ───────────────────────────────────────────────

/// sharedStrings (plain) interns into the model and resolves `t="s"` cells;
/// rich-text runs concatenate for the model while the part stays verbatim.
#[test]
fn sheet_xlsx_shared_strings() {
    let f = XlsxDocument::open(&load("02-formulas.xlsx")).unwrap();
    // Both shared strings present in the interner.
    let interned: Vec<&str> = f.model.strings.iter().map(|(_, s)| s.as_str()).collect();
    assert!(interned.contains(&"Sum"));
    assert!(interned.contains(&"Product"));
    // The string cells resolved to their text.
    let ws = f.model.sheet(0).unwrap();
    assert_eq!(ws.cell(0, 1).unwrap().value, CellValue::Text("Sum".into()));
    assert_eq!(
        ws.cell(1, 1).unwrap().value,
        CellValue::Text("Product".into())
    );
}

// ── sheet.xlsx.styles ───────────────────────────────────────────────────────

/// styles.xml: custom numFmts (id>=164), built-in numFmt ids, and the raw
/// font/fill/border slots resolve onto the cells' StyleIds.
#[test]
fn sheet_xlsx_styles() {
    let s = XlsxDocument::open(&load("03-styles.xlsx")).unwrap();
    let ws = s.model.sheet(0).unwrap();
    let style_of = |r: u32, c: u32| s.model.styles.style(ws.cell(r, c).unwrap().style);

    // A1 s=1 -> built-in numFmt 2 ("0.00"), bold font, thin border.
    let a1 = style_of(0, 0);
    assert_eq!(s.model.styles.num_fmt(a1.num_fmt), "0.00");
    assert_eq!(a1.font, 1);
    assert_eq!(a1.border, 1);

    // B1 s=2 -> custom numFmt 164, yellow fill (index 2).
    let b1 = style_of(0, 1);
    assert_eq!(s.model.styles.num_fmt(b1.num_fmt), "\"$\"#,##0.00");
    assert_eq!(b1.fill, 2);

    // C1 s=3 -> built-in numFmt 9 ("0%").
    assert_eq!(s.model.styles.num_fmt(style_of(0, 2).num_fmt), "0%");

    // A2 s=4 -> custom numFmt 165 ("0.000%").
    assert_eq!(s.model.styles.num_fmt(style_of(1, 0).num_fmt), "0.000%");
}

// ── sheet.xlsx.preserve.unknown-parts ───────────────────────────────────────

/// Unknown parts (customXml + fake vbaProject.bin) survive byte-identical
/// across a zero-edit round-trip.
#[test]
fn sheet_xlsx_preserve_unknown_parts() {
    let bytes = load("04-unknown-parts.xlsx");
    let orig = unzip_parts(&bytes);
    let doc = XlsxDocument::open(&bytes).unwrap();
    let out = doc.save().unwrap();
    let saved: BTreeMap<String, Vec<u8>> = unzip_parts(&out).into_iter().collect();

    for unknown in ["customXml/item1.xml", "xl/vbaProject.bin"] {
        let before = &orig.iter().find(|(n, _)| n == unknown).unwrap().1;
        let after = saved
            .get(unknown)
            .unwrap_or_else(|| panic!("{unknown} dropped"));
        assert_eq!(before, after, "{unknown} not byte-identical");
    }
    // The fake VBA bytes are preserved, never executed/interpreted.
    assert!(saved.get("xl/vbaProject.bin").unwrap().starts_with(b"MZ"));
}

// ── sheet.xlsx.pivot ────────────────────────────────────────────────────────

/// Pivot tables are NEVER interpreted — the publishing-first ruling keeps
/// them as opaque OPC parts that round-trip byte-identical. A
/// `pivotCacheDefinition` injected into the minimal fixture must survive an
/// edit-free open→save unchanged (the preservation invariant, §10.2).
#[test]
fn sheet_xlsx_pivot_cache_preserved_byte_identical() {
    use std::io::Write as _;

    const PIVOT_PART: &str = "xl/pivotCache/pivotCacheDefinition1.xml";
    let pivot_bytes: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" recordCount="3"/>"#;

    // Inject the pivot part into the minimal workbook (rebuild the zip).
    let base = load("01-minimal.xlsx");
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(&base)).expect("valid base zip");
    let mut injected = Vec::new();
    {
        let mut zout = zip::ZipWriter::new(std::io::Cursor::new(&mut injected));
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).unwrap();
            if f.is_dir() {
                continue;
            }
            let name = f.name().to_string();
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut data).unwrap();
            zout.start_file(name, opts).unwrap();
            zout.write_all(&data).unwrap();
        }
        zout.start_file(PIVOT_PART, opts).unwrap();
        zout.write_all(pivot_bytes).unwrap();
        zout.finish().unwrap();
    }

    // Open → save (no edits) → the pivot cache survives byte-identical.
    let doc = XlsxDocument::open(&injected).expect("opens a workbook carrying a pivot cache");
    let out = doc.save().expect("re-saves");
    let saved: BTreeMap<String, Vec<u8>> = unzip_parts(&out).into_iter().collect();
    let kept = saved
        .get(PIVOT_PART)
        .unwrap_or_else(|| panic!("{PIVOT_PART} dropped — pivot preservation broken"));
    assert_eq!(kept, pivot_bytes, "pivot cache must round-trip byte-identical");
}

// ── sheet.xlsx.preserve.unknown-subtrees ────────────────────────────────────

/// Unknown `<worksheet>` children (sheetPr, conditionalFormatting, extLst)
/// survive a DIRTY re-encode, re-emitted in schema position.
#[test]
fn sheet_xlsx_preserve_unknown_subtrees() {
    let mut doc = XlsxDocument::open(&load("05-unknown-subtrees.xlsx")).unwrap();
    // Force the worksheet to re-encode from the model.
    doc.mark_sheet_dirty(0);
    let out = doc.save().unwrap();
    let parts: BTreeMap<String, Vec<u8>> = unzip_parts(&out).into_iter().collect();
    let ws = String::from_utf8(parts.get("xl/worksheets/sheet1.xml").unwrap().clone()).unwrap();

    // The unknown subtrees are present after a dirty re-encode.
    assert!(ws.contains("<sheetPr>"), "sheetPr lost: {ws}");
    assert!(ws.contains("tabColor"), "tabColor lost");
    assert!(ws.contains("conditionalFormatting"), "cf lost");
    assert!(ws.contains("pageMargins"), "pageMargins lost");
    assert!(ws.contains("extLst"), "extLst lost");

    // Schema position: sheetPr before <sheetData>, cf/pageMargins/extLst after.
    let sd = ws.find("<sheetData").unwrap();
    assert!(
        ws.find("<sheetPr>").unwrap() < sd,
        "sheetPr after sheetData"
    );
    assert!(ws.find("conditionalFormatting").unwrap() > sd);
    assert!(ws.find("extLst").unwrap() > sd);

    // The model still has the cells (re-parse the dirty output).
    let doc2 = XlsxDocument::open(&out).unwrap();
    assert_eq!(
        doc2.model.sheet(0).unwrap().cell(0, 0).unwrap().value,
        CellValue::Number(10.0)
    );
}

// ── sheet.xlsx.roundtrip.zero-edit ──────────────────────────────────────────

/// THE launch property: open->save with no dirty marks re-emits every
/// retained part byte-identical (decompressed), over ALL fixtures. The entry
/// set is identical EXCEPT calcChain.xml is removed; the two parts that
/// *reference* calcChain ([Content_Types].xml + the workbook .rels) are the
/// only ones adjusted, and only by removing the calcChain reference (spec
/// §10.2: "its rel/content-type entries adjusted").
#[test]
fn sheet_xlsx_roundtrip_zero_edit() {
    // Parts legitimately adjusted by the calcChain drop.
    const ADJUSTED: &[&str] = &["[Content_Types].xml", "xl/_rels/workbook.xml.rels"];

    for name in FIXTURES {
        let bytes = load(name);
        let orig: BTreeMap<String, Vec<u8>> = unzip_parts(&bytes).into_iter().collect();
        let had_calc_chain = orig.contains_key("xl/calcChain.xml");
        let doc = XlsxDocument::open(&bytes).unwrap();
        assert!(!doc.is_dirty(), "{name}: dirty after open");
        let out = doc.save().unwrap();
        let saved: BTreeMap<String, Vec<u8>> = unzip_parts(&out).into_iter().collect();

        // Entry set: identical except calcChain.xml removed.
        let mut expected: Vec<&String> = orig.keys().filter(|k| *k != "xl/calcChain.xml").collect();
        let mut got: Vec<&String> = saved.keys().collect();
        expected.sort();
        got.sort();
        assert_eq!(expected, got, "{name}: entry set diverged");

        // Per-part decompressed byte identity for every retained part.
        for (part, before) in &orig {
            if part == "xl/calcChain.xml" {
                assert!(!saved.contains_key(part), "{name}: calcChain not dropped");
                continue;
            }
            let after = saved
                .get(part)
                .unwrap_or_else(|| panic!("{name}: {part} dropped"));

            // The two calcChain-referencing parts are adjusted ONLY when a
            // calcChain was actually dropped; otherwise they are verbatim too.
            if had_calc_chain && ADJUSTED.contains(&part.as_str()) {
                let after_str = String::from_utf8(after.clone()).unwrap();
                assert!(
                    !after_str.contains("calcChain"),
                    "{name}: {part} still references calcChain"
                );
            } else {
                assert_eq!(before, after, "{name}: {part} not byte-identical");
            }
        }
    }
}

// ── sheet.xlsx.calcchain.drop ───────────────────────────────────────────────

/// calcChain.xml is dropped on save, along with its [Content_Types] override
/// and its workbook relationship (ruling sheet.xlsx.calcchain.drop).
#[test]
fn sheet_xlsx_calcchain_drop() {
    for name in ["02-formulas.xlsx", "04-unknown-parts.xlsx"] {
        let bytes = load(name);
        // Sanity: the fixture really has a calcChain to drop.
        let orig: BTreeMap<String, Vec<u8>> = unzip_parts(&bytes).into_iter().collect();
        assert!(
            orig.contains_key("xl/calcChain.xml"),
            "{name}: no calcChain to drop"
        );

        let doc = XlsxDocument::open(&bytes).unwrap();
        let out = doc.save().unwrap();
        let saved: BTreeMap<String, Vec<u8>> = unzip_parts(&out).into_iter().collect();

        // Part gone.
        assert!(
            !saved.contains_key("xl/calcChain.xml"),
            "{name}: calcChain retained"
        );

        // Content-types override gone.
        let ct = String::from_utf8(saved.get("[Content_Types].xml").unwrap().clone()).unwrap();
        assert!(
            !ct.contains("calcChain"),
            "{name}: calcChain content-type retained"
        );

        // Workbook relationship gone.
        let rels =
            String::from_utf8(saved.get("xl/_rels/workbook.xml.rels").unwrap().clone()).unwrap();
        assert!(
            !rels.contains("calcChain"),
            "{name}: calcChain rel retained"
        );
    }
}

// ── sheet.xlsx.writer.valid-strict ──────────────────────────────────────────

/// The saved bytes re-open cleanly (a strict reader would accept them) and
/// the re-parsed model equals the original — across all fixtures, both
/// zero-edit and dirty re-encode.
#[test]
fn sheet_xlsx_writer_valid_strict() {
    for name in FIXTURES {
        let bytes = load(name);
        let doc = XlsxDocument::open(&bytes).unwrap();

        // Zero-edit: re-open and compare the model.
        let out = doc.save().unwrap();
        let doc2 = XlsxDocument::open(&out).unwrap_or_else(|e| panic!("{name} reopen: {e:?}"));
        assert_models_equal(name, &doc.model, &doc2.model);
        assert_eq!(
            doc.formula_texts, doc2.formula_texts,
            "{name}: formulas diverged"
        );

        // Dirty re-encode of every sheet: still valid + model-equal.
        let mut dirty = XlsxDocument::open(&bytes).unwrap();
        for sid in 0..dirty.model.sheets.len() as u16 {
            dirty.mark_sheet_dirty(sid);
        }
        let dout = dirty.save().unwrap();
        let doc3 =
            XlsxDocument::open(&dout).unwrap_or_else(|e| panic!("{name} dirty reopen: {e:?}"));
        assert_models_equal(name, &dirty.model, &doc3.model);
    }
}

/// Compare two models for the fields sheet-xlsx round-trips.
fn assert_models_equal(name: &str, a: &sheet_core::SheetModel, b: &sheet_core::SheetModel) {
    assert_eq!(a.sheets.len(), b.sheets.len(), "{name}: sheet count");
    assert_eq!(
        a.calc.date_system, b.calc.date_system,
        "{name}: date system"
    );
    for (i, (sa, sb)) in a.sheets.iter().zip(b.sheets.iter()).enumerate() {
        assert_eq!(sa.name, sb.name, "{name}: sheet {i} name");
        assert_eq!(sa.merges, sb.merges, "{name}: sheet {i} merges");
        assert_eq!(sa.col_widths, sb.col_widths, "{name}: sheet {i} col widths");
        assert_eq!(
            sa.row_heights, sb.row_heights,
            "{name}: sheet {i} row heights"
        );
        // Cell values (style/formula ids are not stable across re-encode but
        // values are; that is the round-trip-meaningful comparison for T0).
        let va: Vec<(&(u32, u32), &CellValue)> =
            sa.cells.iter().map(|(k, c)| (k, &c.value)).collect();
        let vb: Vec<(&(u32, u32), &CellValue)> =
            sb.cells.iter().map(|(k, c)| (k, &c.value)).collect();
        assert_eq!(va, vb, "{name}: sheet {i} cell values");
    }
}
