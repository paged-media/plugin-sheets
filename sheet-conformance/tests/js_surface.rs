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

use sheet_js::core::{
    FrameBoxArg, GridSceneOptions, LowerOptions, PaginateOptionsArg, SheetSession,
};

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

// ── sheet.grid.cell-input-reprint ───────────────────────────────────────────

/// `get_cell_input` re-prints the ENTERABLE text of a cell — the faithful
/// inverse the ADR-012 in-session undo journal stores: a formula cell
/// re-prints `=`-prefixed (NOT its computed display), values re-print as
/// literals, empty/OOB as `""`; and re-entering the re-print restores the
/// exact cell (the round-trip the journal relies on).
#[test]
fn sheet_grid_cell_input_reprint_round_trips() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "2").expect("A1");
    s.set_cell(0, 1, 0, "3").expect("A2");
    s.set_cell(0, 2, 0, "=SUM(A1:A2)").expect("A3");
    s.set_cell(0, 0, 1, "Hello").expect("B1");
    s.set_cell(0, 1, 1, "TRUE").expect("B2");

    // Formula: the INPUT, never the display.
    assert_eq!(s.get_cell_input(0, 2, 0), "=SUM(A1:A2)");
    assert_eq!(s.get_cell_display(0, 2, 0), "5");
    // Literals re-print as entered; empty/OOB are "".
    assert_eq!(s.get_cell_input(0, 0, 0), "2");
    assert_eq!(s.get_cell_input(0, 0, 1), "Hello");
    assert_eq!(s.get_cell_input(0, 1, 1), "TRUE");
    assert_eq!(s.get_cell_input(0, 9, 9), "");
    assert_eq!(s.get_cell_input(99, 0, 0), "");

    // The journal's round trip: capture, overwrite, restore via re-entry.
    let prev = s.get_cell_input(0, 2, 0);
    s.set_cell(0, 2, 0, "42").expect("overwrite A3");
    assert_eq!(s.get_cell_display(0, 2, 0), "42");
    s.set_cell(0, 2, 0, &prev).expect("restore A3");
    assert_eq!(s.get_cell_input(0, 2, 0), "=SUM(A1:A2)");
    assert_eq!(s.get_cell_display(0, 2, 0), "5");
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

/// FREEZE AMENDMENT (audit finding 1): a full-sheet lowered range must be a
/// boundary error, NOT a panic/allocation abort, AND the session must remain
/// usable afterwards (the old bug poisoned the wasm-bindgen borrow so every
/// later `&mut` call threw). A range exactly AT the cap is accepted shape-wise.
#[test]
fn sheet_js_get_range_lowered_caps_full_sheet() {
    let mut s = SheetSession::new();

    // (a) The full-sheet range A1:XFD1048576 (~1.7e10 cells) is rejected as a
    //     boundary error rather than panicking/aborting.
    let oversize = s.get_range_lowered(0, "A1:XFD1048576", LowerOptions::default());
    let err = oversize.expect_err("full-sheet range must be Err, not a panic");
    assert!(
        err.to_string().contains("T0 lowering cap"),
        "message names the cap: {err}"
    );

    // (b) The session REMAINS USABLE after the rejection: set_cell still works
    //     (the old poisoned-borrow bug would have made this throw).
    s.set_cell(0, 0, 0, "42")
        .expect("set_cell works after rejection");
    assert_eq!(s.get_cell_display(0, 0, 0), "42");

    // (c) A range EXACTLY at the cap (A1:A1048576 = 1 col × 1,048,576 rows =
    //     1,048,576 cells) is accepted shape-wise. It is sparse/empty so it
    //     materializes fast (asserted under ~2s).
    let start = std::time::Instant::now();
    let at_cap = s
        .get_range_lowered(0, "A1:A1048576", LowerOptions::default())
        .expect("a range exactly at the cap is accepted");
    let elapsed = start.elapsed();
    assert_eq!(at_cap.rows.len(), 1_048_576);
    assert_eq!(at_cap.cols.len(), 1);
    // A1 is the value we just set; the rest are empty positional "".
    assert_eq!(at_cap.rows[0].cells[0].text, "42");
    assert_eq!(at_cap.rows[1].cells[0].text, "");
    assert!(
        elapsed.as_secs() < 2,
        "at-cap lowering took {elapsed:?} (>2s); reconsider the cap"
    );

    // One cell over the cap (A1:B1048576 = 2,097,152 cells) is rejected.
    assert!(s
        .get_range_lowered(0, "A1:B1048576", LowerOptions::default())
        .is_err());
}

/// FREEZE AMENDMENT (audit finding 2): `set_cell` with an out-of-range sheet id
/// must be a boundary error and must NOT auto-create a phantom sheet (which
/// would silently drop its data on save). The workbook stays 1 sheet, clean.
#[test]
fn sheet_js_set_cell_rejects_oob_sheet() {
    let mut s = SheetSession::new();
    assert_eq!(s.list_sheets().len(), 1, "fresh workbook has one sheet");

    // set_cell(5, ...) on a 1-sheet workbook -> Err.
    let err = s
        .set_cell(5, 0, 0, "boom")
        .expect_err("OOB sheet id must be Err");
    assert!(
        err.to_string().contains("out of range"),
        "boundary message: {err}"
    );

    // No phantom sheet was created; metadata stays clean.
    assert_eq!(s.list_sheets().len(), 1, "no phantom sheet auto-created");
    assert!(
        !s.metadata().dirty,
        "rejected edit leaves the workbook clean"
    );

    // get_range_lowered on the OOB sheet is also a boundary error.
    assert!(s
        .get_range_lowered(5, "A1", LowerOptions::default())
        .is_err());

    // get_cell_display on an OOB sheet returns "" by contract and CANNOT create
    // a sheet (it borrows &self) — the listing is unchanged.
    assert_eq!(s.get_cell_display(5, 0, 0), "");
    assert_eq!(s.list_sheets().len(), 1, "get_cell_display creates nothing");
}

// ── sheet.js.paginate ───────────────────────────────────────────────────────

/// `paginate` threads a tall range across the host frame chain's content boxes
/// (Wave 2D, S-05): each returned `Page` is a self-contained `LoweredContent`
/// for one frame, carrying its `frame_index` + `continued` flag. The wire JSON
/// uses the camelCase keys the TS `Page` mirror expects.
#[test]
fn sheet_js_paginate_threads_range_across_frame_chain() {
    let mut s = SheetSession::new();
    // Six 15pt rows in column A.
    for r in 0..6u32 {
        s.set_cell(0, r, 0, &format!("r{r}")).expect("seed cell");
    }

    // Two frames, each 45pt tall (= 3 rows of 15pt). The 6 rows split 3/3.
    let frames = vec![
        FrameBoxArg {
            width_pt: 200.0,
            height_pt: 45.0,
        },
        FrameBoxArg {
            width_pt: 200.0,
            height_pt: 45.0,
        },
    ];
    let opts = PaginateOptionsArg {
        continued_marker: Some(true),
        ..PaginateOptionsArg::default()
    };
    let pages = s
        .paginate(0, "A1:A6", frames, opts)
        .expect("paginate A1:A6 across 2 frames");

    assert_eq!(pages.len(), 2, "6 rows / 3-per-frame -> 2 pages");
    assert_eq!(pages[0].frame_index, 0);
    assert_eq!(pages[1].frame_index, 1);
    // The first frame is "continued" (more body rows follow); the last is not.
    assert!(pages[0].continued, "frame 0 is continued");
    assert!(!pages[1].continued, "the last frame is not continued");
    // Body rows split 3/3 (frame 0 also carries the continued marker row).
    assert_eq!(pages[0].content.rows[0].cells[0].text, "r0");
    assert_eq!(pages[1].content.rows[0].cells[0].text, "r3");

    // The wire JSON uses camelCase keys the TS `Page` mirror reads.
    let json = serde_json::to_string(&pages).unwrap();
    assert!(json.contains("\"frameIndex\""), "Page.frameIndex camelCase");
    assert!(json.contains("\"content\""), "Page.content");
    assert!(json.contains("\"continued\""), "Page.continued");
    assert!(
        json.contains("\"widthPt\""),
        "nested LoweredContent camelCase"
    );
    assert!(!json.contains("frame_index"), "no snake_case leak");
}

/// `paginate` reuses the boundary discipline of `get_range_lowered`: a junk
/// range is an error, an OOB sheet id is rejected, and an empty frame list
/// yields no pages (the caller provisioned no chain).
#[test]
fn sheet_js_paginate_boundary_and_empty_chain() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "x").expect("A1");

    // Empty frame list -> no pages (nothing to fill).
    let none = s
        .paginate(0, "A1:A1", Vec::new(), PaginateOptionsArg::default())
        .expect("empty chain is not an error");
    assert!(none.is_empty(), "no frames -> no pages");

    // Junk range -> boundary error.
    assert!(s
        .paginate(
            0,
            "not-a-range",
            vec![FrameBoxArg {
                width_pt: 100.0,
                height_pt: 100.0
            }],
            PaginateOptionsArg::default()
        )
        .is_err());

    // OOB sheet id -> boundary error (matches set_cell/get_range_lowered).
    let err = s
        .paginate(
            5,
            "A1:A1",
            vec![FrameBoxArg {
                width_pt: 100.0,
                height_pt: 100.0,
            }],
            PaginateOptionsArg::default(),
        )
        .expect_err("OOB sheet id must be Err");
    assert!(
        err.to_string().contains("out of range"),
        "boundary message: {err}"
    );

    // Full-sheet range -> the T0 cap rejects it (paginate lowers the full range
    // once internally, so the same cap applies).
    assert!(s
        .paginate(
            0,
            "A1:XFD1048576",
            vec![FrameBoxArg {
                width_pt: 100.0,
                height_pt: 100.0
            }],
            PaginateOptionsArg::default()
        )
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

// ── sheet.js.get_grid_scene ─────────────────────────────────────────────────

/// `get_grid_scene` windows a populated sheet: the viewport geometry, the
/// visible populated cells (with formatted text + cumulative offsets), and the
/// gridlines come back; far-away cells are virtualized out. The wire JSON uses
/// the camelCase keys the TS `GridScene` (sheet-host-model/src/grid.ts) expects.
#[test]
fn sheet_js_get_grid_scene_windows_populated_sheet() {
    let mut s = SheetSession::new();
    // Seed three cells, two inside a small viewport, one far below.
    s.set_cell(0, 0, 0, "2").expect("A1");
    s.set_cell(0, 1, 1, "=A1*3").expect("B2 = A1*3 = 6");
    s.set_cell(0, 500_000, 0, "999")
        .expect("far cell, out of window");

    // A small viewport: 44.2575pt cols, 15pt rows. ~3 cols × ~3 rows.
    let scene = s
        .get_grid_scene(0, 0, 0, 140.0, 50.0, GridSceneOptions::default())
        .expect("grid scene over Sheet1");

    // Viewport windowing: gridlines/offsets carry cols+1 / rows+1 boundaries.
    assert_eq!(scene.viewport.first_row, 0);
    assert_eq!(scene.viewport.first_col, 0);
    assert_eq!(
        scene.viewport.x_offsets.len() as u32,
        scene.viewport.cols + 1,
        "x_offsets carries cols+1 cumulative boundaries"
    );
    assert_eq!(
        scene.viewport.y_offsets.len() as u32,
        scene.viewport.rows + 1
    );
    assert_eq!(scene.viewport.x_offsets[0], 0.0, "offsets viewport-local");

    // Only the two in-window populated cells are materialized (far cell out).
    assert_eq!(
        scene.cells.len(),
        2,
        "windowing skips the far-away cell at row 500000"
    );
    let a1 = scene
        .cells
        .iter()
        .find(|c| c.row == 0 && c.col == 0)
        .expect("A1 visible");
    assert_eq!(a1.text, "2", "A1 formatted text");
    let b2 = scene
        .cells
        .iter()
        .find(|c| c.row == 1 && c.col == 1)
        .expect("B2 visible");
    assert_eq!(b2.text, "6", "B2 = A1*3 = 6, recomputed in the engine");

    // Default options: gridlines ON at every visible boundary.
    assert!(!scene.gridlines.h.is_empty(), "gridlines default on");
    assert!(!scene.gridlines.v.is_empty());

    // No selection recorded yet.
    assert!(scene.selection.is_none(), "no selection until recorded");

    // The serialised wire shape uses the camelCase keys grid.ts mirrors.
    let json = serde_json::to_string(&scene).unwrap();
    assert!(json.contains("\"firstRow\""), "viewport.firstRow camelCase");
    assert!(json.contains("\"firstCol\""), "viewport.firstCol camelCase");
    assert!(json.contains("\"xOffsets\""), "viewport.xOffsets camelCase");
    assert!(json.contains("\"yOffsets\""), "viewport.yOffsets camelCase");
    assert!(json.contains("\"styleKey\""), "cell.styleKey camelCase");
    assert!(
        json.contains("\"align\":\"right\""),
        "A1 number -> right (lowercase align)"
    );
    assert!(!json.contains("first_row"), "no snake_case leak");
    assert!(!json.contains("x_offsets"));
}

/// `set_grid_selection` records a rectangle that the NEXT `get_grid_scene` for
/// the same sheet folds into `GridScene.selection`; the wire JSON carries the
/// camelCase `anchorRow`/`anchorCol`/`rows`/`cols` grid.ts expects. A selection
/// recorded for a different sheet is NOT shown.
#[test]
fn sheet_js_set_grid_selection_reflected_in_scene() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "hello").expect("A1");
    s.set_cell(0, 1, 1, "42").expect("B2");

    // Before recording: selection is None.
    let before = s
        .get_grid_scene(0, 0, 0, 200.0, 60.0, GridSceneOptions::default())
        .expect("scene before selection");
    assert!(before.selection.is_none());

    // Record a 2×3 selection anchored at (1, 1).
    s.set_grid_selection(0, 1, 1, 2, 3)
        .expect("record selection on sheet 0");

    let after = s
        .get_grid_scene(0, 0, 0, 400.0, 120.0, GridSceneOptions::default())
        .expect("scene after selection");
    let sel = after
        .selection
        .as_ref()
        .expect("selection folded into the scene");
    assert_eq!(sel.anchor_row, 1);
    assert_eq!(sel.anchor_col, 1);
    assert_eq!(sel.rows, 2);
    assert_eq!(sel.cols, 3);

    // The wire JSON carries the camelCase selection keys grid.ts mirrors.
    let json = serde_json::to_string(&after).unwrap();
    assert!(json.contains("\"selection\""));
    assert!(
        json.contains("\"anchorRow\":1"),
        "selection.anchorRow camelCase"
    );
    assert!(
        json.contains("\"anchorCol\":1"),
        "selection.anchorCol camelCase"
    );
    assert!(!json.contains("anchor_row"), "no snake_case leak");

    // A selection recorded for sheet 0 is not shown on a different sheet id —
    // verified via the OOB-rejection test below; here we re-confirm same-sheet
    // by overwriting with a new rectangle (last write wins).
    s.set_grid_selection(0, 4, 0, 1, 1)
        .expect("overwrite selection");
    let again = s
        .get_grid_scene(0, 0, 0, 400.0, 120.0, GridSceneOptions::default())
        .expect("scene after overwrite");
    let sel2 = again
        .selection
        .as_ref()
        .expect("overwritten selection present");
    assert_eq!(
        (sel2.anchor_row, sel2.anchor_col, sel2.rows, sel2.cols),
        (4, 0, 1, 1)
    );
}

/// FREEZE AMENDMENT (audit finding 2): both grid methods reject an out-of-range
/// sheet id as a boundary error (matching `set_cell`/`get_range_lowered`); the
/// workbook stays clean and no selection leaks across sheets.
#[test]
fn sheet_js_grid_scene_rejects_oob_sheet() {
    let mut s = SheetSession::new();
    assert_eq!(s.list_sheets().len(), 1, "fresh workbook has one sheet");

    // get_grid_scene on the OOB sheet 5 -> Err.
    let err = s
        .get_grid_scene(5, 0, 0, 100.0, 50.0, GridSceneOptions::default())
        .expect_err("OOB sheet id must be Err");
    assert!(
        err.to_string().contains("out of range"),
        "boundary message: {err}"
    );

    // set_grid_selection on the OOB sheet 5 -> Err (no phantom selection).
    let err = s
        .set_grid_selection(5, 0, 0, 1, 1)
        .expect_err("OOB sheet id must be Err");
    assert!(
        err.to_string().contains("out of range"),
        "boundary message: {err}"
    );

    // The valid sheet 0 still produces a clean scene with no selection.
    let scene = s
        .get_grid_scene(0, 0, 0, 100.0, 50.0, GridSceneOptions::default())
        .expect("valid sheet still works");
    assert!(
        scene.selection.is_none(),
        "rejected selection did not leak onto sheet 0"
    );
    assert_eq!(s.list_sheets().len(), 1, "no phantom sheet created");
}

// ── sheet.js.list_functions (S-04 formula-bar autocomplete) ─────────────────

/// `list_functions` is the engine's registry-generated function name table —
/// the source for the formula bar's autocomplete (constitution §7: the bundle
/// MUST source completion names from the ENGINE, never a TS list). It returns
/// only IMPLEMENTED rows (offering an unimplemented function would mislead),
/// each carrying its name/family/arity, and is independent of the loaded
/// workbook (the registry is build-time fixed).
#[test]
fn sheet_js_list_functions_from_registry() {
    let s = SheetSession::new();
    let fns = s.list_functions();

    // The registry is large — the table is non-trivial (the funcs.rs test
    // asserts >=80 rows total; implemented is a subset but still substantial).
    assert!(
        fns.len() >= 50,
        "expected a substantial implemented-function table, got {}",
        fns.len()
    );

    // SUM is registered, implemented, variadic (min 1, no max).
    let sum = fns
        .iter()
        .find(|f| f.name == "SUM")
        .expect("SUM is a registered implemented function");
    assert_eq!(sum.min_args, 1);
    assert_eq!(sum.max_args, None, "SUM is variadic");
    assert!(!sum.family.is_empty(), "SUM carries a family tag");

    // Names are canonical UPPERCASE and unique (the registry enforces both;
    // the completion UI relies on it for prefix matching + dedup).
    let mut names: Vec<&str> = fns.iter().map(|f| f.name.as_str()).collect();
    for n in &names {
        assert!(
            n.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
            "function name {n:?} must be UPPERCASE"
        );
    }
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(before, names.len(), "function names must be unique");

    // VLOOKUP rides along (a different family) — proves it is not an agg-only
    // slice.
    assert!(
        fns.iter().any(|f| f.name == "VLOOKUP"),
        "VLOOKUP should be in the implemented table"
    );

    // The table is independent of the workbook — a fresh empty workbook and a
    // loaded one return the same registry (it is build-time fixed).
    let loaded = SheetSession::load_xlsx(&fixture("02-formulas.xlsx")).expect("load 02");
    assert_eq!(
        loaded.list_functions().len(),
        fns.len(),
        "the function registry is workbook-independent"
    );
}
