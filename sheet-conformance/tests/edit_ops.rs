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

//! Bulk edit-op conformance (`sheet.edit.*` — range sort + find/replace).
//! End-to-end through the native [`sheet_js::core::SheetSession`] — the SAME
//! surface the wasm shim forwards, so the bundle's behavior is pinned here
//! without a wasm runtime. Test-fn names are the `registry/features/edit.yaml`
//! pointers (the coverage gate greps the `sheet_edit_*` prefixes).
//!
//! The headline rulings exercised:
//! - sort: STABLE, numbers-then-text typed order, blanks last BOTH
//!   directions, header pinning, the honest case-insensitive code-point
//!   collation (no ICU), and the FORMULA/SPILL REFUSAL boundary (the engine
//!   has no copy/move reference rewrite yet — `rewrite_fill` is a T1 stub —
//!   so sorting formula cells would corrupt relative refs; we refuse).
//! - find: display-vs-input surface (`in_formulas`), case toggle, entire-cell.
//! - replace: operates on INPUT text through the normal `set_cell` lane; a
//!   replacement that does not parse SKIPS the cell (reported, untouched);
//!   spill output is skipped, never shadowed.

use sheet_js::core::{FindOptions, SheetSession};

// ---- harness ---------------------------------------------------------------

/// A fresh one-sheet session with cells entered via the normal lane.
fn session_with(cells: &[(u32, u32, &str)]) -> SheetSession {
    let mut s = SheetSession::new();
    for &(row, col, input) in cells {
        s.set_cell(0, row, col, input).expect("seed cell");
    }
    s
}

/// The display grid of a column, top to bottom.
fn col_displays(s: &SheetSession, col: u32, rows: u32) -> Vec<String> {
    (0..rows).map(|r| s.get_cell_display(0, r, col)).collect()
}

fn opts(match_case: bool, entire_cell: bool, in_formulas: bool) -> FindOptions {
    FindOptions {
        match_case,
        entire_cell,
        in_formulas,
    }
}

// ── sheet.edit.sort.values ───────────────────────────────────────────────────

/// Numbers sort numerically (not lexically: 9 < 10 < 100); the row payload
/// moves with the key; an EXTERNAL formula depending on a sorted cell
/// recalcs and shows up in `changed`; `edits` carries the prev/next inputs.
#[test]
fn sheet_edit_sort_values_numeric_rows_move_and_dependents_recalc() {
    let mut s = session_with(&[
        (0, 0, "100"),
        (0, 1, "hundred"),
        (1, 0, "9"),
        (1, 1, "nine"),
        (2, 0, "10"),
        (2, 1, "ten"),
        // D1 — OUTSIDE the sorted range, depends on A1.
        (0, 3, "=A1*2"),
    ]);

    let res = s.sort_range(0, "A1:B3", 0, true, false).expect("sort");

    assert_eq!(col_displays(&s, 0, 3), ["9", "10", "100"]);
    assert_eq!(col_displays(&s, 1, 3), ["nine", "ten", "hundred"]);
    // The external dependent recomputed (A1 is now 9 → D1 = 18) and is on
    // the changed list with its FINAL display.
    let d1 = res
        .changed
        .iter()
        .find(|c| c.row == 0 && c.col == 3)
        .expect("external dependent in changed");
    assert_eq!(d1.display, "18");
    assert_eq!(s.get_cell_display(0, 0, 3), "18");
    // Every edit record is a faithful prev/next input pair (A1: 100 → 9).
    let a1 = res
        .edits
        .iter()
        .find(|e| e.row == 0 && e.col == 0)
        .expect("A1 edit recorded");
    assert_eq!((a1.prev_input.as_str(), a1.next_input.as_str()), ("100", "9"));
}

/// Descending reverses the order; the sort is STABLE — duplicate keys keep
/// their original relative row order (the second column proves it).
#[test]
fn sheet_edit_sort_values_descending_is_stable_on_duplicate_keys() {
    let mut s = session_with(&[
        (0, 0, "1"),
        (0, 1, "first-one"),
        (1, 0, "2"),
        (1, 1, "two"),
        (2, 0, "1"),
        (2, 1, "second-one"),
    ]);

    s.sort_range(0, "A1:B3", 0, false, false).expect("sort");

    assert_eq!(col_displays(&s, 0, 3), ["2", "1", "1"]);
    // Both key-1 rows kept their original relative order.
    assert_eq!(col_displays(&s, 1, 3), ["two", "first-one", "second-one"]);
}

/// A key column outside the range's width is a boundary error.
#[test]
fn sheet_edit_sort_values_key_col_out_of_range_is_boundary() {
    let mut s = session_with(&[(0, 0, "2"), (1, 0, "1")]);
    let err = s.sort_range(0, "A1:B2", 2, true, false).unwrap_err();
    assert!(err.0.contains("key column"), "got: {}", err.0);
}

// ── sheet.edit.sort.header ───────────────────────────────────────────────────

/// `has_header` pins the range's first row — it never moves; the body sorts.
#[test]
fn sheet_edit_sort_header_row_is_pinned() {
    let mut s = session_with(&[
        (0, 0, "Amount"),
        (1, 0, "30"),
        (2, 0, "10"),
        (3, 0, "20"),
    ]);

    s.sort_range(0, "A1:A4", 0, true, true).expect("sort");

    assert_eq!(col_displays(&s, 0, 4), ["Amount", "10", "20", "30"]);
}

// ── sheet.edit.sort.blanks-last ──────────────────────────────────────────────

/// Blank key cells sink to the END in BOTH directions (Excel's blanks rule —
/// a descending sort does NOT float blanks to the top).
#[test]
fn sheet_edit_sort_blanks_last_in_both_directions() {
    let cells: &[(u32, u32, &str)] = &[
        (0, 0, "20"),
        (0, 1, "twenty"),
        // Row 2 (index 1) — blank key, populated payload.
        (1, 1, "blank-key"),
        (2, 0, "10"),
        (2, 1, "ten"),
    ];

    let mut asc = session_with(cells);
    asc.sort_range(0, "A1:B3", 0, true, false).expect("asc");
    assert_eq!(col_displays(&asc, 1, 3), ["ten", "twenty", "blank-key"]);

    let mut desc = session_with(cells);
    desc.sort_range(0, "A1:B3", 0, false, false).expect("desc");
    assert_eq!(col_displays(&desc, 1, 3), ["twenty", "ten", "blank-key"]);
}

// ── sheet.edit.sort.collation ────────────────────────────────────────────────

/// The documented honest collation: case-INSENSITIVE (char-wise Unicode
/// default case folding), code-point order, ties broken by exact text —
/// "Apple" < "banana" < "Cherry" regardless of case; numbers sort BEFORE
/// any text (typed ranks).
#[test]
fn sheet_edit_sort_collation_case_insensitive_codepoint_order() {
    let mut s = session_with(&[
        (0, 0, "banana"),
        (1, 0, "Cherry"),
        (2, 0, "Apple"),
        (3, 0, "42"),
    ]);

    s.sort_range(0, "A1:A4", 0, true, false).expect("sort");

    assert_eq!(col_displays(&s, 0, 4), ["42", "Apple", "banana", "Cherry"]);
}

// ── sheet.edit.sort.formula-boundary ─────────────────────────────────────────

/// A formula cell among the movable rows REFUSES the sort with a clean
/// boundary error and the model is untouched (the honest subset — the
/// engine has no copy/move ref rewrite; silent corruption is never an
/// option).
#[test]
fn sheet_edit_sort_formula_boundary_refuses_and_leaves_model_intact() {
    let mut s = session_with(&[(0, 0, "5"), (1, 0, "=A1+1"), (2, 0, "1")]);

    let err = s.sort_range(0, "A1:A3", 0, true, false).unwrap_err();
    assert!(
        err.0.contains("sort over formulas not yet supported"),
        "got: {}",
        err.0
    );
    // Untouched: values AND the formula's input survive.
    assert_eq!(col_displays(&s, 0, 3), ["5", "6", "1"]);
    assert_eq!(s.get_cell_input(0, 1, 0), "=A1+1");
}

/// A header-pinned formula row is FINE (it never moves) — only movable rows
/// are scanned for the boundary.
#[test]
fn sheet_edit_sort_formula_boundary_pinned_header_formula_is_allowed() {
    let mut s = session_with(&[(0, 0, "=1+1"), (1, 0, "30"), (2, 0, "10")]);

    s.sort_range(0, "A1:A3", 0, true, true).expect("sort");

    assert_eq!(col_displays(&s, 0, 3), ["2", "10", "30"]);
    assert_eq!(s.get_cell_input(0, 0, 0), "=1+1");
}

/// Engine-owned spill output among the movable rows also refuses (sorting
/// spilled values would shadow the anchor formula's region).
#[test]
fn sheet_edit_sort_formula_boundary_spilled_region_refuses() {
    let mut s = SheetSession::new();
    // A1 spills A1:A3 = 1,2,3 (anchor holds the top-left).
    s.set_cell(0, 0, 0, "=SEQUENCE(3,1)").expect("spill anchor");
    assert_eq!(s.get_cell_display(0, 1, 0), "2");

    // A2:A3 holds NO formula cells — but they are spill-owned.
    let err = s.sort_range(0, "A2:A3", 0, true, false).unwrap_err();
    assert!(err.0.contains("spilled"), "got: {}", err.0);
    assert_eq!(s.get_cell_display(0, 1, 0), "2");
}

// ── sheet.edit.find.values ───────────────────────────────────────────────────

/// Default find matches the formatted DISPLAY text — a formula cell is found
/// by its computed display, a literal by its display; hits come back in
/// row-major order with excerpts.
#[test]
fn sheet_edit_find_values_matches_display_text() {
    let s = session_with(&[
        (0, 0, "total 30"),
        (1, 0, "=10+20"), // displays "30"
        (2, 0, "nothing"),
    ]);

    let hits = s.find_all(Some(0), "30", opts(false, false, false)).expect("find");
    let coords: Vec<(u32, u32)> = hits.iter().map(|h| (h.row, h.col)).collect();
    assert_eq!(coords, [(0, 0), (1, 0)]);
    assert_eq!(hits[0].excerpt, "total 30");
    assert_eq!(hits[1].excerpt, "30"); // the display, not the formula text

    // An empty needle is a boundary error.
    assert!(s.find_all(Some(0), "", opts(false, false, false)).is_err());
}

// ── sheet.edit.find.formulas ─────────────────────────────────────────────────

/// `in_formulas` switches the match surface to the re-enterable INPUT text:
/// "SUM" finds the `=SUM(…)` cell (and not without the flag).
#[test]
fn sheet_edit_find_formulas_matches_input_text() {
    let s = session_with(&[(0, 0, "1"), (1, 0, "2"), (2, 0, "=SUM(A1:A2)")]);

    let without = s.find_all(Some(0), "SUM", opts(false, false, false)).expect("find");
    assert!(without.is_empty(), "display surface must not expose the formula text");

    let with = s.find_all(Some(0), "SUM", opts(false, false, true)).expect("find");
    assert_eq!(with.len(), 1);
    assert_eq!((with[0].row, with[0].col), (2, 0));
    assert_eq!(with[0].excerpt, "=SUM(A1:A2)");
}

// ── sheet.edit.find.case ─────────────────────────────────────────────────────

/// `match_case` toggles case sensitivity; the default fold is char-wise
/// Unicode case folding (so "STRASSE" ≠ "straße" — documented bounds).
#[test]
fn sheet_edit_find_case_sensitivity_toggle() {
    let s = session_with(&[(0, 0, "Alpha"), (1, 0, "ALPHA"), (2, 0, "alpha")]);

    let insensitive = s.find_all(Some(0), "alpha", opts(false, false, false)).expect("find");
    assert_eq!(insensitive.len(), 3);

    let sensitive = s.find_all(Some(0), "alpha", opts(true, false, false)).expect("find");
    assert_eq!(sensitive.len(), 1);
    assert_eq!(sensitive[0].row, 2);
}

// ── sheet.edit.find.entire-cell ──────────────────────────────────────────────

/// `entire_cell` requires the needle to equal the WHOLE cell text (Excel's
/// "Match entire cell contents"), composing with the case toggle.
#[test]
fn sheet_edit_find_entire_cell_requires_whole_text() {
    let s = session_with(&[(0, 0, "net"), (1, 0, "network"), (2, 0, "NET")]);

    let partial = s.find_all(Some(0), "net", opts(false, false, false)).expect("find");
    assert_eq!(partial.len(), 3);

    let entire = s.find_all(Some(0), "net", opts(false, true, false)).expect("find");
    let rows: Vec<u32> = entire.iter().map(|h| h.row).collect();
    assert_eq!(rows, [0, 2]); // "network" excluded; "NET" folds equal

    let entire_cased = s.find_all(Some(0), "net", opts(true, true, false)).expect("find");
    assert_eq!(entire_cased.len(), 1);
    assert_eq!(entire_cased[0].row, 0);
}

// ── sheet.edit.replace.inputs ────────────────────────────────────────────────

/// Replace operates on INPUT text re-entered through the normal set_cell
/// lane: a formula's TEXT is edited (with `in_formulas`) and the dependents
/// recalc; literals re-type (a text edit producing a number IS a number).
#[test]
fn sheet_edit_replace_inputs_rewrites_formula_text_and_recalcs() {
    let mut s = session_with(&[(0, 0, "1"), (0, 1, "2"), (1, 0, "=A1*10")]);

    let res = s
        .replace_all(Some(0), "A1", "B1", opts(false, false, true))
        .expect("replace");

    assert_eq!(res.occurrences, 1);
    assert_eq!(res.edits.len(), 1);
    assert_eq!(res.edits[0].prev_input, "=A1*10");
    assert_eq!(res.edits[0].next_input, "=B1*10");
    assert!(res.skipped.is_empty());
    assert_eq!(s.get_cell_input(0, 1, 0), "=B1*10");
    assert_eq!(s.get_cell_display(0, 1, 0), "20");
}

/// Without `in_formulas` formula cells are never touched — only literal
/// inputs rewrite; counts and changed displays reflect the final state.
#[test]
fn sheet_edit_replace_inputs_values_lane_leaves_formulas_alone() {
    let mut s = session_with(&[
        (0, 0, "draft copy"),
        (1, 0, "draft"),
        (2, 0, "=\"draft\"&A1"),
    ]);

    let res = s
        .replace_all(Some(0), "draft", "final", opts(false, false, false))
        .expect("replace");

    assert_eq!(res.occurrences, 2);
    assert_eq!(res.edits.len(), 2);
    assert_eq!(s.get_cell_display(0, 0, 0), "final copy");
    assert_eq!(s.get_cell_display(0, 1, 0), "final");
    // The formula text survived untouched (its DISPLAY now shows the new A1).
    assert_eq!(s.get_cell_input(0, 2, 0), "=\"draft\"&A1");
    assert_eq!(s.get_cell_display(0, 2, 0), "draftfinal copy");
}

// ── sheet.edit.replace.skip ──────────────────────────────────────────────────

/// A replacement that breaks a formula's parse SKIPS that cell — reported
/// with a reason, the cell untouched (never half-applied); other matched
/// cells still apply.
#[test]
fn sheet_edit_replace_skip_unparseable_replacement_leaves_cell_intact() {
    let mut s = session_with(&[(0, 0, "SUM note"), (1, 0, "=SUM(A3:A4)")]);

    let res = s
        .replace_all(Some(0), "SUM", "SUM((", opts(false, false, true))
        .expect("replace");

    // The literal applied; the formula skipped.
    assert_eq!(res.occurrences, 1);
    assert_eq!(res.edits.len(), 1);
    assert_eq!(res.skipped.len(), 1);
    assert_eq!((res.skipped[0].row, res.skipped[0].col), (1, 0));
    assert!(
        res.skipped[0].reason.contains("does not parse"),
        "got: {}",
        res.skipped[0].reason
    );
    assert_eq!(s.get_cell_display(0, 0, 0), "SUM(( note");
    assert_eq!(s.get_cell_input(0, 1, 0), "=SUM(A3:A4)");
}

/// Engine-owned spill output is skipped with a reason — replacing a spilled
/// value would shadow the anchor formula's region.
#[test]
fn sheet_edit_replace_skip_spilled_cells_reported_not_shadowed() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "=SEQUENCE(3,1)").expect("spill anchor");
    assert_eq!(s.get_cell_display(0, 1, 0), "2");

    let res = s
        .replace_all(Some(0), "2", "9", opts(false, false, false))
        .expect("replace");

    assert_eq!(res.occurrences, 0);
    assert!(res.edits.is_empty());
    assert_eq!(res.skipped.len(), 1);
    assert!(res.skipped[0].reason.contains("spilled"), "got: {}", res.skipped[0].reason);
    assert_eq!(s.get_cell_display(0, 1, 0), "2"); // intact
}
