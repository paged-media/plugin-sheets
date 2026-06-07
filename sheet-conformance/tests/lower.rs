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

//! Lowering conformance (spec §8.2/§8.3; registry `sheet.lower.*`). Drives
//! [`sheet_lower::lower_range`] over hand-built [`SheetModel`]s and asserts
//! both the IR values and the serialized wire shape (the contract the TS
//! mirror `packages/sheet-host-model/src/lowered.ts` consumes:
//! `camelCase` keys, lowercase `align` variants). Self-contained — no
//! `sheet-conformance` lib import.
//!
//! One `fn sheet_lower_*` per `registry/features/lower.yaml` row (the
//! §12.2 coverage-gate pointers).

use sheet_core::{
    Cell, CellRef, CellStyle, CellValue, Expr, Formula, LitValue, OrderedF64, RangeRef, SheetId,
    SheetModel, StyleId,
};
use sheet_lower::{lower_range, CellRange, ViewOptions};

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

/// A model `RangeRef` (sheet-local, relative refs) for a merge span.
fn merge(sheet: SheetId, r0: u32, c0: u32, r1: u32, c1: u32) -> RangeRef {
    let mk = |row, col| CellRef {
        sheet,
        row,
        col,
        row_abs: false,
        col_abs: false,
    };
    RangeRef {
        start: mk(r0, c0),
        end: mk(r1, c1),
    }
}

// ---- Registry-pointer test fns (one per `sheet.lower.*` row). ----

/// `sheet.lower.range.single-frame` — a populated range lowers to one
/// frame's IR: column/row geometry, positional cells, no threading. Also
/// pins the wire shape (camelCase keys) the TS mirror reads.
#[test]
fn sheet_lower_range_single_frame() {
    let (mut m, s) = workbook();
    {
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(0, 0, text("Name"));
        ws.set_cell(0, 1, text("Qty"));
        ws.set_cell(1, 0, text("Widget"));
        ws.set_cell(1, 1, num(3.0));
        ws.col_widths.insert(0, 12.0); // 12 ch -> 63.0 pt
        ws.row_heights.insert(0, 20.0); // header row taller
    }

    let lc = lower_range(
        &m,
        s,
        CellRange {
            r0: 0,
            c0: 0,
            r1: 1,
            c1: 1,
        },
        &ViewOptions::default(),
    );

    // 2x2 region, range-relative indices from 0.
    assert_eq!(lc.cols.len(), 2);
    assert_eq!(lc.rows.len(), 2);
    assert_eq!(lc.cols[0].index, 0);
    assert_eq!(lc.cols[1].index, 1);
    assert!((lc.cols[0].width_pt - 63.0).abs() < 1e-9); // explicit 12 ch
    assert!((lc.cols[1].width_pt - 44.2575).abs() < 1e-9); // default 8.43 ch
    assert!((lc.rows[0].height_pt - 20.0).abs() < 1e-9); // explicit
    assert!((lc.rows[1].height_pt - 15.0).abs() < 1e-9); // default

    // Each row carries the FULL range width positionally.
    assert_eq!(lc.rows[0].cells.len(), 2);
    assert_eq!(lc.rows[0].cells[0].text, "Name");
    assert_eq!(lc.rows[1].cells[1].text, "3");

    // Wire shape: camelCase keys, lowercase align (the lowered.ts contract).
    let json = serde_json::to_string(&lc).unwrap();
    assert!(json.contains("\"widthPt\""), "expected camelCase widthPt");
    assert!(json.contains("\"heightPt\""), "expected camelCase heightPt");
    assert!(!json.contains("width_pt"), "snake_case must not leak");
    assert!(
        json.contains("\"align\":\"left\""),
        "text cell aligns left (lowercase variant)"
    );
}

/// `sheet.lower.text.formatted` — the lowered cell text IS the
/// number-formatted value (spec §8.3), resolved through the cell's style's
/// num_fmt code. Formula cells lower their CACHED value (never re-evaluate).
#[test]
fn sheet_lower_text_formatted() {
    let (mut m, s) = workbook();

    // Style A: 2-decimal currency-ish "0.00".
    let two_dp = {
        let id = m.styles.intern_num_fmt("0.00");
        m.styles.intern_style(CellStyle {
            num_fmt: id,
            ..Default::default()
        })
    };
    // Style B: a date code yyyy-mm-dd.
    let date = {
        let id = m.styles.intern_num_fmt("yyyy-mm-dd");
        m.styles.intern_style(CellStyle {
            num_fmt: id,
            ..Default::default()
        })
    };

    // A formula cell: cached value 7, distinct from the formula literal —
    // lowering must render the CACHE, proving it never evaluates.
    let fid = m.intern_formula(Formula {
        root: Expr::Lit(LitValue::Number(OrderedF64::new(999.0))),
    });

    {
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(
            0,
            0,
            Cell {
                value: CellValue::Number(3.5),
                style: two_dp,
                ..Default::default()
            },
        );
        ws.set_cell(
            0,
            1,
            Cell {
                value: CellValue::Number(44197.0), // 2021-01-01 serial
                style: date,
                ..Default::default()
            },
        );
        ws.set_cell(
            0,
            2,
            Cell {
                value: CellValue::Number(7.0), // cached formula result
                formula: Some(fid),
                style: StyleId(0), // General
            },
        );
    }

    let lc = lower_range(
        &m,
        s,
        CellRange {
            r0: 0,
            c0: 0,
            r1: 0,
            c1: 2,
        },
        &ViewOptions::default(),
    );

    let cells = &lc.rows[0].cells;
    assert_eq!(cells[0].text, "3.50"); // 0.00 format
    assert_eq!(cells[1].text, "2021-01-01"); // date format
    assert_eq!(cells[2].text, "7"); // cached value via General, NOT 999
}

/// `sheet.lower.rules.grid` — grid rules (h/v lines) at every row/column
/// boundary, in frame-content coordinates (origin at the range top-left).
#[test]
fn sheet_lower_rules_grid() {
    let (m, s) = workbook(); // empty: pure default geometry
    let range = CellRange {
        r0: 0,
        c0: 0,
        r1: 2, // 3 rows
        c1: 1, // 2 cols
    };

    let lc = lower_range(&m, s, range, &ViewOptions::default());

    // 3 rows -> 4 h-boundaries; 2 cols -> 3 v-boundaries.
    assert_eq!(lc.rules.h.len(), 4);
    assert_eq!(lc.rules.v.len(), 3);

    let total_w = 2.0 * 44.2575;
    let total_h = 3.0 * 15.0;

    // h-rules: at = cumulative y (0,15,30,45), spanning the full width.
    for (i, rule) in lc.rules.h.iter().enumerate() {
        assert!((rule.at - (i as f64 * 15.0)).abs() < 1e-9, "h-rule {i} at");
        assert_eq!(rule.from, 0.0);
        assert!((rule.to - total_w).abs() < 1e-9);
    }
    // v-rules: at = cumulative x, spanning the full height.
    for (i, rule) in lc.rules.v.iter().enumerate() {
        assert!(
            (rule.at - (i as f64 * 44.2575)).abs() < 1e-9,
            "v-rule {i} at"
        );
        assert_eq!(rule.from, 0.0);
        assert!((rule.to - total_h).abs() < 1e-9);
    }

    // Toggling include_grid_rules off drops every rule.
    let off = lower_range(
        &m,
        s,
        range,
        &ViewOptions {
            include_grid_rules: false,
            header_rows: 0,
        },
    );
    assert!(off.rules.h.is_empty());
    assert!(off.rules.v.is_empty());
}

/// `sheet.lower.merges` — model merges intersecting the range are clipped
/// to it and re-based to range-relative offsets (spec §8.2). Spans wholly
/// outside the range are dropped; partial overlaps are clipped.
#[test]
fn sheet_lower_merges() {
    let (mut m, s) = workbook();
    {
        let ws = m.sheet_mut(s).unwrap();
        // (a) A merge wholly inside the lowered range: B2:C2 (row 1, cols 1..=2).
        ws.merges.push(merge(s, 1, 1, 1, 2));
        // (b) A merge straddling the bottom-right edge: D4:F6 (rows 3..=5,
        //     cols 3..=5) — the lowered range is A1:E5 (rows 0..=4, cols
        //     0..=4), so it clips to rows 3..=4, cols 3..=4.
        ws.merges.push(merge(s, 3, 3, 5, 5));
        // (c) A merge wholly outside: H10:I11 — dropped.
        ws.merges.push(merge(s, 9, 7, 10, 8));
    }

    let lc = lower_range(
        &m,
        s,
        CellRange {
            r0: 0,
            c0: 0,
            r1: 4,
            c1: 4,
        },
        &ViewOptions::default(),
    );

    assert_eq!(lc.merges.len(), 2, "two merges overlap the range");

    // (a) B2:C2 -> range-relative row 1, col 1, 1x2.
    let a = &lc.merges[0];
    assert_eq!((a.row, a.col, a.row_span, a.col_span), (1, 1, 1, 2));

    // (b) D4:F6 clipped to rows 3..=4, cols 3..=4 -> row 3, col 3, 2x2.
    let b = &lc.merges[1];
    assert_eq!((b.row, b.col, b.row_span, b.col_span), (3, 3, 2, 2));

    // Wire shape: rowSpan/colSpan camelCase.
    let json = serde_json::to_string(&lc).unwrap();
    assert!(json.contains("\"rowSpan\""));
    assert!(json.contains("\"colSpan\""));
}

/// `sheet.lower.deterministic` — same model+range+options yields IR that is
/// both structurally equal AND serde_json-byte-equal across two runs
/// (spec §12.4). Exercises a mixed grid so BTreeMap iteration order matters.
#[test]
fn sheet_lower_deterministic() {
    let (mut m, s) = workbook();
    {
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(0, 0, num(1.0));
        ws.set_cell(0, 2, text("z"));
        ws.set_cell(2, 1, num(2.5));
        ws.set_cell(1, 1, text("mid"));
        ws.col_widths.insert(1, 9.0);
        ws.row_heights.insert(2, 22.0);
        ws.merges.push(merge(s, 0, 0, 1, 0));
    }

    let range = CellRange {
        r0: 0,
        c0: 0,
        r1: 2,
        c1: 2,
    };
    let opts = ViewOptions::default();

    let a = lower_range(&m, s, range, &opts);
    let b = lower_range(&m, s, range, &opts);

    assert_eq!(a, b, "structurally equal");
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "serde_json-byte-equal"
    );
}
