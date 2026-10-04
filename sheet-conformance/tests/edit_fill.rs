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

//! Wave 5 editing conformance (`sheet.edit.clipboard.*`, `sheet.edit.fill.*`)
//! through the native [`sheet_js::core::SheetSession`] — the surface the wasm
//! shim forwards. Test-fn names are the `registry/features/edit.yaml`
//! pointers. Expected values follow Excel's documented AutoFill behaviour
//! (Microsoft "Fill data automatically in worksheet cells"): one number
//! copies, two continue the linear trend, dates step by day or by month,
//! weekday/month names continue their list, "Item 1" counts on, formulas
//! re-address relative references.

use sheet_js::core::SheetSession;

fn session_with(cells: &[(u32, u32, &str)]) -> SheetSession {
    let mut s = SheetSession::new();
    for &(row, col, input) in cells {
        s.set_cell(0, row, col, input).expect("seed cell");
    }
    s
}

fn col_inputs(s: &SheetSession, col: u32, rows: std::ops::Range<u32>) -> Vec<String> {
    rows.map(|r| s.get_cell_input(0, r, col)).collect()
}

fn col_displays(s: &SheetSession, col: u32, rows: std::ops::Range<u32>) -> Vec<String> {
    rows.map(|r| s.get_cell_display(0, r, col)).collect()
}

// ── sheet.edit.clipboard.range-inputs ───────────────────────────────────────

/// A copy snapshots re-enterable inputs: formulas with `=`, literals as
/// typed, blanks as "".
#[test]
fn sheet_edit_clipboard_range_inputs() {
    let s = session_with(&[(0, 0, "1"), (0, 1, "=A1*2"), (1, 0, "text")]);
    let got = s.get_range_inputs(0, "A1:B2").unwrap();
    assert_eq!(got, vec![vec!["1", "=A1*2"], vec!["text", ""]]);
    assert!(
        s.get_range_inputs(5, "A1").is_err(),
        "an OOB sheet is a boundary error"
    );
    assert!(
        s.get_range_inputs(0, "nope").is_err(),
        "junk range is a boundary error"
    );
}

// ── sheet.edit.clipboard.shift ──────────────────────────────────────────────

/// A paste re-addresses relative references by the paste offset; `$` parts
/// stay; off-grid becomes #REF!; values and unparseable text are verbatim.
#[test]
fn sheet_edit_clipboard_shift() {
    let s = session_with(&[]);
    let inputs = vec![vec![
        "=A1+B$1+$C2+$D$4".to_string(),
        "=SUM(A1:A3)".to_string(),
        "42".to_string(),
        "=(".to_string(),
    ]];
    let out = s.shift_formulas(0, &inputs, 2, 1).unwrap();
    assert_eq!(out[0][0], "=B3+C$1+$C4+$D$4");
    assert_eq!(out[0][1], "=SUM(B3:B5)");
    assert_eq!(out[0][2], "42");
    assert_eq!(out[0][3], "=(");
    // Pasting up past row 1 → #REF!.
    let up = s
        .shift_formulas(0, &[vec!["=A1".to_string()]], -1, 0)
        .unwrap();
    assert_eq!(up[0][0], "=#REF!");
}

// ── sheet.edit.fill.numbers ─────────────────────────────────────────────────

/// One number copies; two continue the step; three fit the linear trend.
#[test]
fn sheet_edit_fill_numbers() {
    let mut s = session_with(&[(0, 0, "5"), (0, 1, "1"), (1, 1, "3")]);
    s.fill_range(0, "A1", "A1:A4", true).unwrap();
    assert_eq!(col_inputs(&s, 0, 0..4), ["5", "5", "5", "5"]);
    s.fill_range(0, "B1:B2", "B1:B5", true).unwrap();
    assert_eq!(col_inputs(&s, 1, 0..5), ["1", "3", "5", "7", "9"]);
    let mut s = session_with(&[
        (0, 0, "1"),
        (1, 0, "2"),
        (2, 0, "4"),
        (0, 1, "0.1"),
        (1, 1, "0.2"),
    ]);
    s.fill_range(0, "A1:A3", "A1:A5", true).unwrap();
    assert_eq!(
        col_inputs(&s, 0, 3..5),
        ["5.33333333333333", "6.83333333333333"]
    );
    s.fill_range(0, "B1:B2", "B1:B4", true).unwrap();
    assert_eq!(col_inputs(&s, 1, 2..4), ["0.3", "0.4"]);
}

// ── sheet.edit.fill.dates ───────────────────────────────────────────────────

/// One date steps a day; dates on one day of the month step by month, the
/// day clamped to short months;
/// the filled cells carry the source's date format (they DISPLAY as dates).
#[test]
fn sheet_edit_fill_dates() {
    let mut s = SheetSession::load_csv(
        "2024-01-30\t2023-10-31\n\t2023-12-31\n",
        Some('\t'),
        "en-US",
        "Sheet1",
        0.0,
    )
    .unwrap();
    let d0 = s.get_cell_display(0, 0, 0);
    s.fill_range(0, "A1", "A1:A3", true).unwrap();
    let shown = col_displays(&s, 0, 0..3);
    assert_eq!(shown[0], d0);
    assert_ne!(
        shown[1],
        s.get_cell_input(0, 1, 0),
        "a filled date displays as a date"
    );
    // Day step: serials +1, +2.
    let base: f64 = s.get_cell_input(0, 0, 0).parse().unwrap();
    assert_eq!(s.get_cell_input(0, 1, 0), (base + 1.0).to_string());
    assert_eq!(s.get_cell_input(0, 2, 0), (base + 2.0).to_string());
    // Month step (same day of month, 2 months apart): Oct 31, Dec 31 →
    // Feb 29 2024 (the day clamps to the month), Apr 30.
    s.fill_range(0, "B1:B2", "B1:B4", true).unwrap();
    let dec31: f64 = s.get_cell_input(0, 1, 1).parse().unwrap();
    let feb29 = s.get_cell_input(0, 2, 1).parse::<f64>().unwrap();
    let apr30 = s.get_cell_input(0, 3, 1).parse::<f64>().unwrap();
    assert_eq!(feb29 - dec31, 60.0, "Dec 31 → Feb 29 (clamped)");
    assert_eq!(apr30 - feb29, 61.0, "Feb 29 → Apr 30 (clamped)");
    // The carried date format survives a save.
    let shown = s.get_cell_display(0, 3, 1);
    let bytes = s.save_xlsx().unwrap();
    let back = SheetSession::load_xlsx(&bytes).unwrap();
    assert_eq!(back.get_cell_display(0, 3, 1), shown);
}

// ── sheet.edit.fill.names ───────────────────────────────────────────────────

/// Weekday and month names continue their list (English and German), with
/// the case of the last source kept.
#[test]
fn sheet_edit_fill_names() {
    let mut s = session_with(&[
        (0, 0, "Fri"),
        (0, 1, "Januar"),
        (1, 1, "März"),
        (0, 2, "MONDAY"),
    ]);
    s.fill_range(0, "A1", "A1:A4", true).unwrap();
    assert_eq!(col_inputs(&s, 0, 0..4), ["Fri", "Sat", "Sun", "Mon"]);
    s.fill_range(0, "B1:B2", "B1:B4", true).unwrap();
    assert_eq!(col_inputs(&s, 1, 2..4), ["Mai", "Juli"]);
    s.fill_range(0, "C1", "C1:C2", true).unwrap();
    assert_eq!(s.get_cell_input(0, 1, 2), "TUESDAY");
}

// ── sheet.edit.fill.numbered-text ───────────────────────────────────────────

/// Text ending in an integer counts on ("Item 1" → "Item 2"), keeping padding.
#[test]
fn sheet_edit_fill_numbered_text() {
    let mut s = session_with(&[(0, 0, "Item 1"), (0, 1, "Q01"), (1, 1, "Q03")]);
    s.fill_range(0, "A1", "A1:A3", true).unwrap();
    assert_eq!(col_inputs(&s, 0, 0..3), ["Item 1", "Item 2", "Item 3"]);
    s.fill_range(0, "B1:B2", "B1:B4", true).unwrap();
    assert_eq!(col_inputs(&s, 1, 2..4), ["Q05", "Q07"]);
}

// ── sheet.edit.fill.formulas ────────────────────────────────────────────────

/// Formulas repeat with relative references re-addressed per target row;
/// `$` parts stay; the values recalc.
#[test]
fn sheet_edit_fill_formulas() {
    let mut s = session_with(&[(0, 0, "1"), (1, 0, "2"), (2, 0, "3"), (0, 1, "=A1*$A$1*10")]);
    let r = s.fill_range(0, "B1", "B1:B3", true).unwrap();
    assert_eq!(
        col_inputs(&s, 1, 0..3),
        ["=A1*$A$1*10", "=A2*$A$1*10", "=A3*$A$1*10"]
    );
    assert_eq!(col_displays(&s, 1, 0..3), ["10", "20", "30"]);
    assert_eq!(
        r.edits.len(),
        2,
        "one edit per written cell (one undo group)"
    );
    assert_eq!(r.edits[0].prev_input, "");
    assert_eq!(r.edits[0].next_input, "=A2*$A$1*10");
}

// ── sheet.edit.fill.copy ────────────────────────────────────────────────────

/// `series = false` (fill down / right) repeats the source — no series.
#[test]
fn sheet_edit_fill_copy() {
    let mut s = session_with(&[(0, 0, "1"), (1, 0, "2"), (0, 1, "Mon")]);
    s.fill_range(0, "A1:A2", "A1:A5", false).unwrap();
    assert_eq!(col_inputs(&s, 0, 0..5), ["1", "2", "1", "2", "1"]);
    s.fill_range(0, "B1", "B1:B3", false).unwrap();
    assert_eq!(col_inputs(&s, 1, 0..3), ["Mon", "Mon", "Mon"]);
}

// ── sheet.edit.fill.direction ───────────────────────────────────────────────

/// Fill runs up and left too (the series continues AWAY from the source);
/// a target that does not extend the source in one direction is refused,
/// model untouched.
#[test]
fn sheet_edit_fill_direction() {
    let mut s = session_with(&[(4, 0, "1"), (5, 0, "2"), (0, 4, "Tue"), (0, 5, "Wed")]);
    s.fill_range(0, "A5:A6", "A3:A6", true).unwrap();
    assert_eq!(col_inputs(&s, 0, 2..4), ["-1", "0"]);
    s.fill_range(0, "E1:F1", "C1:F1", true).unwrap();
    assert_eq!(s.get_cell_input(0, 0, 2), "Sun");
    assert_eq!(s.get_cell_input(0, 0, 3), "Mon");
    let err = s.fill_range(0, "A5:A6", "B5:B9", true).unwrap_err();
    assert!(err.to_string().contains("one direction"), "{err}");
    let err = s.fill_range(0, "A5:A6", "A5:B9", true).unwrap_err();
    assert!(err.to_string().contains("one direction"), "{err}");
    assert_eq!(
        s.get_cell_input(0, 6, 0),
        "",
        "a refused fill writes nothing"
    );
}

// ── sheet.edit.fill.lanes ───────────────────────────────────────────────────

/// Each column fills on its own; a mixed lane repeats.
#[test]
fn sheet_edit_fill_lanes() {
    let mut s = session_with(&[
        (0, 0, "1"),
        (1, 0, "2"),
        (0, 1, "Jan"),
        (1, 1, "Feb"),
        (0, 2, "a"),
        (1, 2, "1"),
    ]);
    s.fill_range(0, "A1:C2", "A1:C4", true).unwrap();
    assert_eq!(col_inputs(&s, 0, 2..4), ["3", "4"]);
    assert_eq!(col_inputs(&s, 1, 2..4), ["Mar", "Apr"]);
    assert_eq!(col_inputs(&s, 2, 2..4), ["a", "1"]);
}
