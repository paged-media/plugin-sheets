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

//! Pagination conformance (spec §8.2 "the killer feature"; registry
//! `sheet.lower.paginate.*`). Drives [`sheet_lower::paginate`] — the
//! multi-frame threading of a tall range across a caller-supplied ordered
//! frame list (S-05: chain topology is an SDK gap, so the frames are the
//! caller's). Asserts the split geometry, repeated headers, the continued
//! marker, keep-together blocks, the oversize-row pathological case, and the
//! convergence property (proptest). Self-contained — no `sheet-conformance`
//! lib import.
//!
//! One `fn sheet_lower_paginate_*` per `registry/features/paginate.yaml` row
//! (the §12.2 coverage-gate pointers).

use proptest::prelude::*;
use sheet_core::{Cell, CellValue, SheetId, SheetModel};
use sheet_lower::{paginate, CellRange, FrameBox, PaginateOptions};

// ---- Builders. ----

/// A fresh 1-sheet workbook; returns `(model, sheet_id)`.
fn workbook() -> (SheetModel, SheetId) {
    let mut m = SheetModel::new();
    let s = m.add_sheet("Sheet1");
    (m, s)
}

fn num(n: f64) -> Cell {
    Cell {
        value: CellValue::Number(n),
        ..Default::default()
    }
}

fn text(t: &str) -> Cell {
    Cell {
        value: CellValue::from(t),
        ..Default::default()
    }
}

/// A column-0 sheet of `n` rows numbered `0..n` (default 15 pt rows).
fn linear_sheet(n: u32) -> (SheetModel, SheetId) {
    let (mut m, s) = workbook();
    {
        let ws = m.sheet_mut(s).unwrap();
        for r in 0..n {
            ws.set_cell(r, 0, num(r as f64));
        }
    }
    (m, s)
}

/// `n` frames of uniform `height_pt` (width fixed; only height paginates).
fn frames(n: usize, height_pt: f64) -> Vec<FrameBox> {
    vec![
        FrameBox {
            width_pt: 200.0,
            height_pt,
        };
        n
    ]
}

/// Range A1:A`n` (column 0, rows `0..n`).
fn col0_range(n: u32) -> CellRange {
    CellRange {
        r0: 0,
        c0: 0,
        r1: n - 1,
        c1: 0,
    }
}

/// Collect, across all pages, the BODY row texts (header-repeat rows are the
/// leading `header_rows` rows of every continuation frame — this helper is
/// used by tests with no header so every emitted row is a body row).
fn all_row_texts(pages: &[sheet_lower::Page]) -> Vec<String> {
    pages
        .iter()
        .flat_map(|p| p.content.rows.iter().map(|r| r.cells[0].text.clone()))
        .collect()
}

// ---- Registry-pointer test fns (one per `sheet.lower.paginate.*` row). ----

/// `sheet.lower.paginate.split` — a 400-row range across a 12-frame chain
/// splits at row boundaries: no row spans a frame break, every body row lands
/// exactly once, frames fill up to (not past) their height. (The 400/12 case
/// the spec names; here each frame is sized so the 400 rows just fit across
/// the 12-frame chain.)
#[test]
fn sheet_lower_paginate_split() {
    let (m, s) = linear_sheet(400);
    // 400 rows * 15 pt = 6000 pt total; 12 frames -> ~500 pt each. Use 525 pt
    // (35 rows) per frame so all 400 rows fit across the 12 frames.
    let opts = PaginateOptions::default();
    let pages = paginate(&m, s, col0_range(400), &frames(12, 525.0), &opts);

    // Every page targets its frame in order, fills with whole rows only.
    for (i, p) in pages.iter().enumerate() {
        assert_eq!(p.frame_index, i, "page targets frame {i}");
        let used: f64 = p.content.rows.iter().map(|r| r.height_pt).sum();
        assert!(used <= 525.0 + 1e-9, "page {i} overfills: {used}pt");
        assert!(!p.oversize, "no oversize frames in the clean split");
    }

    // No row is split: every body row appears exactly once, 0..400 in order.
    let texts = all_row_texts(&pages);
    assert_eq!(texts.len(), 400, "all 400 rows placed exactly once");
    let expected: Vec<String> = (0..400).map(|r| (r as f64).to_string()).collect();
    assert_eq!(texts, expected, "rows in order, none split or duplicated");

    // 400 rows / 35 per frame = 12 frames (the last partially filled).
    assert_eq!(pages.len(), 12);
}

/// `sheet.lower.paginate.repeated-headers` — with `repeated_header_rows > 0`,
/// the header band is re-emitted at the top of EVERY continuation frame (and
/// costs its height there), while the first frame shows it once inline. Body
/// rows are never duplicated.
#[test]
fn sheet_lower_paginate_repeated_headers() {
    let (mut m, s) = workbook();
    {
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(0, 0, text("HDR")); // the single header row
        for r in 1..21 {
            ws.set_cell(r, 0, num(r as f64)); // 20 body rows
        }
    }
    // 21 rows total, 15 pt each. Frames of 75 pt hold 5 rows. Frame 0: HDR +
    // 4 body. Continuation frames: HDR (repeated) + 4 body each.
    let opts = PaginateOptions {
        repeated_header_rows: 1,
        ..Default::default()
    };
    let pages = paginate(&m, s, col0_range(21), &frames(8, 75.0), &opts);

    assert!(pages.len() >= 2, "20 body rows need several 4-row frames");

    // Frame 0: first row is the header, shown inline (not a re-emit).
    assert_eq!(pages[0].content.rows[0].cells[0].text, "HDR");

    // Every CONTINUATION frame repeats the header at its top.
    for p in &pages[1..] {
        assert_eq!(
            p.content.rows[0].cells[0].text, "HDR",
            "continuation frame {} repeats the header",
            p.frame_index
        );
    }

    // Body rows 1..=20 appear exactly once, in order, across all frames —
    // counting NON-header rows (a header row's text is "HDR").
    let body: Vec<String> = pages
        .iter()
        .flat_map(|p| {
            // The leading header row of a continuation frame is a re-emit; on
            // frame 0 the header is the genuine first row. Filter "HDR".
            p.content
                .rows
                .iter()
                .map(|r| r.cells[0].text.clone())
                .filter(|t| t != "HDR")
        })
        .collect();
    let expected: Vec<String> = (1..21).map(|r| (r as f64).to_string()).collect();
    assert_eq!(body, expected, "body rows once, in order, headers excluded");
}

/// `sheet.lower.paginate.continued-marker` — when `continued_marker` is set,
/// every frame followed by more body rows is flagged `continued` and carries
/// an appended marker row; the LAST frame is not flagged.
#[test]
fn sheet_lower_paginate_continued_marker() {
    let (m, s) = linear_sheet(10);
    // 10 rows * 15 pt; frames of 45 pt hold 3 rows -> 4 frames (3,3,3,1).
    let opts = PaginateOptions {
        continued_marker: true,
        ..Default::default()
    };
    let pages = paginate(&m, s, col0_range(10), &frames(6, 45.0), &opts);

    assert!(pages.len() >= 2, "10 rows over 3-row frames need >1 frame");

    // Every frame but the last is `continued`; the last is not.
    let last = pages.len() - 1;
    for (i, p) in pages.iter().enumerate() {
        if i < last {
            assert!(p.continued, "frame {i} should be flagged continued");
        } else {
            assert!(!p.continued, "the final frame is not continued");
        }
    }

    // A continued frame carries an APPENDED marker row beyond its body rows.
    // Frame 0 holds: continued marker reduces body capacity is NOT modeled —
    // the marker is appended after the placed body rows, so a continued
    // frame's row count exceeds the rows it would carry uncontinued. Assert
    // the marker row exists (blank, default height) as the last row.
    let p0 = &pages[0];
    let marker = p0.content.rows.last().unwrap();
    assert_eq!(marker.cells[0].text, "", "marker row is blank");

    // No body row is lost or duplicated: 0..10 appear once across all frames,
    // ignoring the blank marker rows.
    let body: Vec<String> = pages
        .iter()
        .flat_map(|p| p.content.rows.iter().map(|r| r.cells[0].text.clone()))
        .filter(|t| !t.is_empty())
        .collect();
    let expected: Vec<String> = (0..10).map(|r| (r as f64).to_string()).collect();
    assert_eq!(body, expected);
}

/// `sheet.lower.paginate.keep-rows` — a keep-together block never splits
/// across a frame break: a block that does not fit in the remaining space
/// moves WHOLESALE to the next frame.
#[test]
fn sheet_lower_paginate_keep_rows() {
    let (m, s) = linear_sheet(9);
    // 9 rows * 15 pt; frames of 60 pt hold 4 rows. Keep rows 2..=4 together
    // (a 3-row block). Frame 0 would fit rows 0,1 then the block (2,3,4)
    // would overflow (2 used + 3 = 5 > 4 capacity), so the block moves
    // wholesale to frame 1: frame 0 = {0,1}, frame 1 = {2,3,4, then 5}.
    let opts = PaginateOptions {
        keep_rows_together: vec![(2, 4)],
        ..Default::default()
    };
    let pages = paginate(&m, s, col0_range(9), &frames(6, 60.0), &opts);

    // Find the frame holding row "2"; rows "3" and "4" must be on the SAME
    // frame (the block stayed whole).
    let frame_of = |t: &str| {
        pages
            .iter()
            .position(|p| p.content.rows.iter().any(|r| r.cells[0].text == t))
            .unwrap()
    };
    let f2 = frame_of("2");
    assert_eq!(frame_of("3"), f2, "block row 3 stays with row 2");
    assert_eq!(frame_of("4"), f2, "block row 4 stays with row 2");
    // The block was pushed off frame 0 (rows 0,1 fit; the block did not).
    assert!(f2 >= 1, "the 3-row block moved off the first frame");
    assert_eq!(frame_of("0"), 0);
    assert_eq!(frame_of("1"), 0);

    // No row dropped or duplicated.
    let texts = all_row_texts(&pages);
    let expected: Vec<String> = (0..9).map(|r| (r as f64).to_string()).collect();
    assert_eq!(texts, expected);
}

/// `sheet.lower.paginate.tall-row` — a single row taller than a whole frame
/// is placed ALONE on its own frame and flagged `oversize` (the spec's named
/// pathological case; no infinite loop). Surrounding rows paginate normally.
#[test]
fn sheet_lower_paginate_tall_row() {
    let (mut m, s) = workbook();
    {
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(0, 0, num(0.0));
        ws.set_cell(1, 0, num(1.0));
        ws.set_cell(2, 0, num(2.0)); // the giant row
        ws.set_cell(3, 0, num(3.0));
        // Row 2 is 500 pt — taller than the 100 pt frames.
        ws.row_heights.insert(2, 500.0);
    }
    let opts = PaginateOptions::default();
    let pages = paginate(&m, s, col0_range(4), &frames(8, 100.0), &opts);

    // The giant row 2 sits ALONE on an oversize-flagged frame.
    let giant = pages
        .iter()
        .find(|p| p.content.rows.iter().any(|r| r.cells[0].text == "2"))
        .unwrap();
    assert!(giant.oversize, "the tall row's frame is flagged oversize");
    assert_eq!(
        giant.content.rows.len(),
        1,
        "the tall row is placed alone (nothing packed with it)"
    );

    // Every row still placed exactly once, in order — no drop, no loop.
    let texts = all_row_texts(&pages);
    assert_eq!(texts, vec!["0", "1", "2", "3"]);
}

/// `sheet.lower.paginate.convergence` — the property the spec names: for
/// RANDOM row heights and frame sizes, pagination TERMINATES and every body
/// row lands on exactly one frame (no drops, no duplicates), as long as the
/// frame chain is long enough. A bounded loop (rows consumed monotonically)
/// underwrites this.
#[test]
fn sheet_lower_paginate_convergence() {
    proptest!(ProptestConfig::with_cases(200), |(
        // 1..=60 rows, each 1..=120 pt tall.
        heights in prop::collection::vec(1u32..=120, 1..=60),
        // Frame height 1..=200 pt (deliberately includes frames smaller than
        // some rows, to exercise the oversize path).
        frame_h in 1u32..=200,
    )| {
        let n = heights.len() as u32;
        let (mut m, s) = workbook();
        {
            let ws = m.sheet_mut(s).unwrap();
            for (r, &h) in heights.iter().enumerate() {
                ws.set_cell(r as u32, 0, num(r as f64));
                ws.row_heights.insert(r as u32, h as f64);
            }
        }
        // Provide MORE than enough frames: even if every row were oversize
        // (one row per frame), n frames suffice. Give n + a slack frame.
        let fr = frames((n + 1) as usize, frame_h as f64);
        let opts = PaginateOptions::default();

        // Terminates (this call returning is the termination proof).
        let pages = paginate(&m, s, col0_range(n), &fr, &opts);

        // Every body row appears EXACTLY once, in order.
        let texts: Vec<String> = pages
            .iter()
            .flat_map(|p| p.content.rows.iter().map(|r| r.cells[0].text.clone()))
            .collect();
        let expected: Vec<String> = (0..n).map(|r| (r as f64).to_string()).collect();
        prop_assert_eq!(texts, expected);

        // Frame indices are strictly increasing and within range.
        let mut prev: Option<usize> = None;
        for p in &pages {
            if let Some(pv) = prev {
                prop_assert!(p.frame_index > pv, "frame indices strictly increase");
            }
            prop_assert!(p.frame_index < fr.len());
            prev = Some(p.frame_index);
        }
    });
}
