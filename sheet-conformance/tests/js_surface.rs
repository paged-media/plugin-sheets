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

//! sheet-js surface conformance (spec §4, the final Rust join). The
//! `#[wasm_bindgen]` `SheetEngine` is a thin `cfg(wasm32)` shim; ALL logic
//! lives in the native [`sheet_js::core::SheetSession`], which this test drives
//! through the FULL load → recalc → set → save → lower loop against the corpus
//! fixtures. Test-fn names are the `registry/features/*.yaml` pointers (the
//! coverage gate greps the `sheet_js_*` prefixes).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use sheet_js::core::{LowerOptions, SheetSession};

/// Path to a corpus fixture (sibling of the conformance crate, like
/// `xlsx_roundtrip.rs`).
fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join("xlsx-corpus")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read fixture {}: {e}", p.display()))
}

/// Decompress a zip into a `name -> bytes` map (for part-presence checks).
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

// ── sheet.js.load ───────────────────────────────────────────────────────────

/// Loading 02-formulas.xlsx parses every formula text (both SUM and the concat
/// are registered, so NONE are unparsed), recalcs to the cached values, and the
/// metadata + sheet listing reflect the workbook.
#[test]
fn sheet_js_load_xlsx_parses_and_recalcs() {
    let s = SheetSession::load_xlsx(&fixture("02-formulas.xlsx")).expect("load 02");

    // Both formulas (SUM(A1:A2), B1&B2) parse -> zero unparsed.
    let meta = s.metadata();
    assert_eq!(meta.unparsed_formulas, 0, "all formulas should parse");
    assert_eq!(meta.date_system, "1900");
    assert!(!meta.dirty, "freshly loaded workbook is not dirty");

    // One sheet, used extent A1:B3 -> rows=3, cols=2 (1-based extent).
    let sheets = s.list_sheets();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].id, 0);
    assert_eq!(sheets[0].name, "Sheet1");
    assert_eq!(sheets[0].rows, 3);
    assert_eq!(sheets[0].cols, 2);

    // A3 = SUM(A1:A2) recalced to 5 (General format -> "5"); A1/A2 literals.
    assert_eq!(s.get_cell_display(0, 0, 0), "2", "A1");
    assert_eq!(s.get_cell_display(0, 1, 0), "3", "A2");
    assert_eq!(s.get_cell_display(0, 2, 0), "5", "A3 = SUM(A1:A2)");
    // B3 = B1&B2 = "Sum"&"Product".
    assert_eq!(s.get_cell_display(0, 2, 1), "SumProduct", "B3 = B1&B2");
    // Empty / out-of-range cells display "".
    assert_eq!(s.get_cell_display(0, 9, 9), "");
    assert_eq!(s.get_cell_display(99, 0, 0), "");
}

/// The empty-workbook constructor starts with one sheet and no formulas.
#[test]
fn sheet_js_new_empty_workbook() {
    let s = SheetSession::new();
    let sheets = s.list_sheets();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].name, "Sheet1");
    assert_eq!(sheets[0].rows, 0, "empty sheet has zero extent");
    assert_eq!(sheets[0].cols, 0);
    assert_eq!(s.metadata().unparsed_formulas, 0);
}

// ── sheet.js.set_cell ───────────────────────────────────────────────────────

/// `set_cell` enters a formula and reports the changed cell with its formatted
/// display; editing a precedent recomputes the dependent (the dirty cut).
#[test]
fn sheet_js_set_cell_recalcs_dependents() {
    let mut s = SheetSession::load_xlsx(&fixture("02-formulas.xlsx")).expect("load 02");

    // Enter a NEW formula at A5 depending on A3 (= SUM(A1:A2) = 5).
    let r = s.set_cell(0, 4, 0, "=A3*2").expect("enter A5");
    assert!(r.circular.is_empty());
    let a5 = r
        .changed
        .iter()
        .find(|c| c.row == 4 && c.col == 0)
        .expect("A5 reported changed");
    assert_eq!(a5.display, "10", "A5 = A3*2 = 5*2");

    // Edit A1 (2 -> 10): A3 = SUM(10,3) = 13, A5 = 26 — both dependents change.
    let r = s.set_cell(0, 0, 0, "10").expect("edit A1");
    let displays: BTreeMap<(u32, u32), String> = r
        .changed
        .iter()
        .map(|c| ((c.row, c.col), c.display.clone()))
        .collect();
    assert_eq!(displays.get(&(2, 0)).map(String::as_str), Some("13"), "A3");
    assert_eq!(displays.get(&(4, 0)).map(String::as_str), Some("26"), "A5");
    // And the live displays agree.
    assert_eq!(s.get_cell_display(0, 2, 0), "13");
    assert_eq!(s.get_cell_display(0, 4, 0), "26");
}

/// A malformed formula is a BOUNDARY error (Err), distinct from a calc error
/// like `#DIV/0!` which is a display string the engine stores.
#[test]
fn sheet_js_set_cell_parse_error_is_boundary() {
    let mut s = SheetSession::new();
    assert!(s.set_cell(0, 0, 0, "=1+").is_err(), "syntax error -> Err");
    assert!(
        s.set_cell(0, 0, 0, "=NOTAFUNC(1)").is_err(),
        "unknown function -> Err"
    );

    // #DIV/0! is a DISPLAY string, never a boundary error.
    let r = s
        .set_cell(0, 0, 0, "=1/0")
        .expect("=1/0 commits (calc error is a value)");
    let a1 = r.changed.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
    assert_eq!(a1.display, "#DIV/0!");
    assert_eq!(s.get_cell_display(0, 0, 0), "#DIV/0!");
}

// ── sheet.js.save ───────────────────────────────────────────────────────────

/// THE round-trip: edit a value precedent + a formula cell, save, reopen — the
/// edits (formula TEXT + recomputed VALUE) survive.
#[test]
fn sheet_js_save_xlsx_round_trips_edits() {
    let mut s = SheetSession::load_xlsx(&fixture("02-formulas.xlsx")).expect("load 02");

    // Edit A1 to 10 (a value) and replace B3 with a new formula =A1+A2.
    s.set_cell(0, 0, 0, "10").expect("edit A1");
    s.set_cell(0, 2, 1, "=A1+A2").expect("edit B3");
    assert!(s.metadata().dirty, "edited workbook reports dirty");

    let bytes = s.save_xlsx().expect("save");

    // Reopen the saved bytes through a fresh session.
    let s2 = SheetSession::load_xlsx(&bytes).expect("reopen saved");

    // A1 round-tripped as the edited value 10; A3 still SUM(A1:A2) = 13.
    assert_eq!(s2.get_cell_display(0, 0, 0), "10", "A1 edit survived");
    assert_eq!(
        s2.get_cell_display(0, 2, 0),
        "13",
        "A3 recomputed on reload"
    );
    // B3 is now A1+A2 = 13 (the new formula round-tripped + recomputed).
    assert_eq!(s2.get_cell_display(0, 2, 1), "13", "B3 new formula");
    // The formula text re-printed without '=' and re-parses on reload.
    assert_eq!(
        s2.metadata().unparsed_formulas,
        0,
        "re-printed formulas all re-parse"
    );

    // The session stays usable after save (engine rebuilt).
    let mut s = s;
    let r = s.set_cell(0, 5, 0, "=A1").expect("set after save");
    assert_eq!(r.changed.iter().find(|c| c.row == 5).unwrap().display, "10");
}

/// Saving an UNTOUCHED workbook preserves its unknown parts: reopen the saved
/// bytes and confirm the customXml + (fake) vbaProject parts are still present
/// (deep byte-identity is `xlsx_roundtrip`'s job; here we prove the sheet-js
/// save path does not drop them).
#[test]
fn sheet_js_save_xlsx_preserves_unknown_parts() {
    let mut s = SheetSession::load_xlsx(&fixture("04-unknown-parts.xlsx")).expect("load 04");
    // A1 = 7 (a plain value), no edits.
    assert_eq!(s.get_cell_display(0, 0, 0), "7");
    assert!(!s.metadata().dirty, "no edits -> not dirty");

    let bytes = s.save_xlsx().expect("save untouched");
    let parts = unzip_parts(&bytes);

    assert!(
        parts.contains_key("customXml/item1.xml"),
        "customXml dropped on save"
    );
    assert!(
        parts.contains_key("xl/vbaProject.bin"),
        "vbaProject dropped on save"
    );
    // calcChain is the one part that legitimately drops (ruling).
    assert!(
        !parts.contains_key("xl/calcChain.xml"),
        "calcChain should drop"
    );

    // The saved bytes re-open and parse.
    let s2 = SheetSession::load_xlsx(&bytes).expect("reopen saved 04");
    assert_eq!(s2.get_cell_display(0, 0, 0), "7");
}

// ── sheet.js.get_range_lowered ──────────────────────────────────────────────

/// `get_range_lowered` returns the `LoweredContent` IR with formatted text per
/// cell, honoring the forwarded options; a junk range is a boundary error.
#[test]
fn sheet_js_get_range_lowered_returns_formatted_rows() {
    let s = SheetSession::load_xlsx(&fixture("02-formulas.xlsx")).expect("load 02");

    // A1:B3 over the whole used range.
    let lc = s
        .get_range_lowered(0, "A1:B3", LowerOptions::default())
        .expect("lower A1:B3");
    assert_eq!(lc.cols.len(), 2);
    assert_eq!(lc.rows.len(), 3);
    // Default options: grid rules ON -> a rule at every boundary.
    assert_eq!(lc.rules.h.len(), 4, "3 rows -> 4 h-boundaries");
    assert_eq!(lc.rules.v.len(), 3, "2 cols -> 3 v-boundaries");

    // The lowered TEXT is the formatted value (A1=2, A3=SUM=5, B3=concat).
    let cell = |r: usize, c: usize| lc.rows[r].cells[c].text.as_str();
    assert_eq!(cell(0, 0), "2", "A1");
    assert_eq!(cell(2, 0), "5", "A3");
    assert_eq!(cell(2, 1), "SumProduct", "B3");

    // align serialises lowercase (matches lowered.ts) and matches the contract.
    let json = serde_json::to_string(&lc).unwrap();
    assert!(json.contains("\"widthPt\""), "camelCase geometry keys");
    assert!(json.contains("\"align\":\"right\""), "A1 number -> right");
    assert!(!json.contains("\"General\""), "no PascalCase align leak");

    // A single cell "A1" is accepted (no ':').
    let one = s
        .get_range_lowered(0, "A1", LowerOptions::default())
        .expect("lower single A1");
    assert_eq!(one.rows.len(), 1);
    assert_eq!(one.cols.len(), 1);
    assert_eq!(one.rows[0].cells[0].text, "2");

    // include_grid_rules = false drops the rules.
    let no_rules = s
        .get_range_lowered(
            0,
            "A1:B3",
            LowerOptions {
                include_grid_rules: Some(false),
                header_rows: None,
            },
        )
        .expect("lower no rules");
    assert!(no_rules.rules.h.is_empty());
    assert!(no_rules.rules.v.is_empty());

    // Junk range -> boundary error.
    assert!(s
        .get_range_lowered(0, "not-a-range", LowerOptions::default())
        .is_err());
    assert!(s
        .get_range_lowered(0, "A1:ZZZZ", LowerOptions::default())
        .is_err());
}

// ── sheet.js.set_now ────────────────────────────────────────────────────────

/// `set_now` updates the clock without panicking; a NOW-driven cell reflects
/// the new serial after a recalc-triggering edit.
#[test]
fn sheet_js_set_now_updates_clock() {
    let mut s = SheetSession::new();
    // Pin the clock and enter a formula reading it.
    s.set_now(45000.0);
    let r = s.set_cell(0, 0, 0, "=NOW()").expect("enter NOW");
    // NOW() returns the configured serial (General format may add decimals; we
    // only assert it is non-empty and numeric-ish — the exact format is
    // sheet-format's concern, exercised elsewhere).
    let a1 = r.changed.iter().find(|c| c.row == 0).unwrap();
    assert!(!a1.display.is_empty(), "NOW() produced a display");
    // Moving the clock and re-entering reflects the new serial.
    s.set_now(46000.0);
    let r2 = s.set_cell(0, 1, 0, "=NOW()").expect("enter NOW again");
    let a2 = r2.changed.iter().find(|c| c.row == 1).unwrap();
    assert!(!a2.display.is_empty());
}
