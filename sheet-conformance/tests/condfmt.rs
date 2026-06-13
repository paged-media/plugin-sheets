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

//! Conditional-formatting conformance (spec §10.4, §8.3; M2 cond-fmt track).
//! Test-fn names are the registry pointers for `registry/features/condfmt.yaml`
//! (the coverage gate greps these prefixes). The fixture is
//! `corpus/xlsx-corpus/08-condfmt.xlsx` (built by `generate.py`): five cf
//! blocks over distinct columns — cellIs, expression, colorScale, dataBar,
//! iconSet — plus a `<dxfs>` table the cellIs/expression rules reference.
//!
//! The honest T2 line (documented in the registry rows):
//! - `cellIs` + reducible `expression` + `colorScale` LOWER to per-cell style
//!   overrides (evaluated against the already-computed cell values — no formula
//!   evaluator; `sheet-lower` has no `sheet-calc` dep);
//! - `dataBar` lowers to a DRAWN RECT (the page-draw geometry lane, spec §8.2 —
//!   `LoweredContent.databars`) — NOT a style override; the style path leaves
//!   the cell unfilled, the geometry carries one proportional rect per cell;
//! - `iconSet` is preserve-only (round-trips, not rendered);
//! - the cf XML round-trips byte-identical via the worksheet's verbatim capture.

use sheet_lower::{lower_range_condfmt, CellRange, ViewOptions};
use sheet_xlsx::{CfRuleKind, XlsxDocument};
use std::io::Read;
use std::path::PathBuf;

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

fn open_fixture() -> XlsxDocument {
    XlsxDocument::open(&load("08-condfmt.xlsx")).expect("08-condfmt.xlsx opens")
}

/// The full lowered region for sheet 0 over A1:E5 with conditional formatting
/// folded in. Returns `(content, fill_at)` where `fill_at(r, c)` is the fill
/// colour of the styled cell at range-relative `(r, c)`.
fn lower_full(doc: &XlsxDocument) -> sheet_lower::LoweredContent {
    let cf = doc.lowered_conditional_formats(0);
    lower_range_condfmt(
        &doc.model,
        0,
        CellRange {
            r0: 0,
            c0: 0,
            r1: 4,
            c1: 4,
        },
        &ViewOptions::default(),
        &doc.visual_styles,
        &cf,
    )
}

/// The fill colour of the cell at range-relative `(row, col)` in a lowered
/// region (via its `style_key` into the styles table), or `None`.
fn fill_at(lc: &sheet_lower::LoweredContent, row: usize, col: usize) -> Option<String> {
    let key = lc.rows[row].cells[col].style_key as usize;
    lc.styles[key].fill_rgb.clone()
}

// ── sheet.xlsx.conditional-formatting ───────────────────────────────────────

/// Parse `08-condfmt.xlsx` → the five conditional-format blocks are present on
/// the sheet, each typed to its rule kind, AND the `<dxfs>` table resolved the
/// dxf overrides. Also: the cf XML round-trips byte-identical (the verbatim
/// capture is untouched by the additive parse).
#[test]
fn sheet_xlsx_conditional_formatting() {
    let doc = open_fixture();
    let cf = doc
        .conditional_formats
        .get(&0)
        .expect("sheet 0 has conditional formatting");
    assert_eq!(cf.blocks.len(), 5, "five cf blocks parsed");

    // Each block's first rule kind, in column order.
    let kinds: Vec<&CfRuleKind> = cf.blocks.iter().map(|b| &b.rules[0].kind).collect();
    assert!(matches!(kinds[0], CfRuleKind::CellIs { .. }), "A: cellIs");
    assert!(
        matches!(kinds[1], CfRuleKind::Expression { .. }),
        "B: reducible expression"
    );
    assert!(
        matches!(kinds[2], CfRuleKind::ColorScale(_)),
        "C: colorScale"
    );
    assert!(matches!(kinds[3], CfRuleKind::DataBar(_)), "D: dataBar");
    assert!(matches!(kinds[4], CfRuleKind::IconSet), "E: iconSet");

    // The `<dxfs>` table resolved (dxf 0 = yellow fill + red text + bold).
    assert!(doc.dxfs.len() >= 2, "two dxfs parsed");
    assert!(doc.dxfs[0].bold);
    assert_eq!(doc.dxfs[0].fill_rgb.as_deref(), Some("#FFFF00"));
    assert_eq!(doc.dxfs[0].text_rgb.as_deref(), Some("#FF0000"));
    assert_eq!(doc.dxfs[1].fill_rgb.as_deref(), Some("#00FF00"));

    // Round-trip: the conditionalFormatting XML survives byte-identical in the
    // re-saved worksheet part (the additive parse never touched the capture).
    let out = doc.save().expect("save");
    let ws = read_part(&out, "xl/worksheets/sheet1.xml");
    let ws = String::from_utf8(ws).unwrap();
    assert_eq!(
        ws.matches("conditionalFormatting").count(),
        10,
        "all five cf blocks (open+close) re-emit"
    );
    assert!(ws.contains(r#"<cfRule type="cellIs" dxfId="0" priority="1" operator="greaterThan">"#));
    assert!(ws.contains(r#"<cfRule type="colorScale" priority="3">"#));
    assert!(ws.contains(r#"<cfRule type="iconSet" priority="5">"#));
}

/// Read one decompressed part out of a re-saved package.
fn read_part(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    let mut f = zip.by_name(name).unwrap_or_else(|_| panic!("part {name}"));
    let mut data = Vec::new();
    f.read_to_end(&mut data).unwrap();
    data
}

// ── sheet.lower.condfmt.cfrule-parse ────────────────────────────────────────

/// The cfRule model captures type + priority + dxfId + operands. cellIs A>5
/// (priority 1, dxf 0); the reducible expression B>100 (priority 2, dxf 1);
/// and the deferred-vs-reducible distinction is exercised in the unit tests.
#[test]
fn sheet_lower_condfmt_cfrule_parse() {
    let doc = open_fixture();
    let cf = doc.conditional_formats.get(&0).unwrap();

    // A: cellIs greaterThan 5, priority 1, dxf 0.
    let a = &cf.blocks[0].rules[0];
    assert_eq!(a.priority, 1);
    assert_eq!(a.dxf_id, Some(0));
    match &a.kind {
        CfRuleKind::CellIs { operands, .. } => assert_eq!(operands, &[5.0]),
        other => panic!("expected cellIs, got {other:?}"),
    }

    // B: expression reduced to `cell > 100`, priority 2, dxf 1.
    let b = &cf.blocks[1].rules[0];
    assert_eq!(b.priority, 2);
    assert_eq!(b.dxf_id, Some(1));
    match &b.kind {
        CfRuleKind::Expression { operand, .. } => assert_eq!(*operand, 100.0),
        other => panic!("expected expression, got {other:?}"),
    }

    // The cf range maps to model coordinates (A1:A5 → rows 0..=4, col 0).
    assert_eq!(cf.blocks[0].ranges, vec![(0, 0, 4, 0)]);
}

// ── sheet.lower.condfmt.cellis ──────────────────────────────────────────────

/// A cellIs `>5` rule paints the matching cells with the rule's dxf (bold + red
/// text + yellow fill) and leaves non-matching cells at their base style. The
/// reducible-expression `>100` block behaves identically over column B.
#[test]
fn sheet_lower_condfmt_cellis() {
    let doc = open_fixture();
    let lc = lower_full(&doc);

    // Column A values: 8, 3, 10, 5, 7 → >5 matches rows 0, 2, 4.
    let painted_a = [0usize, 2, 4];
    for r in 0..5 {
        let key = lc.rows[r].cells[0].style_key as usize;
        let style = &lc.styles[key];
        if painted_a.contains(&r) {
            assert_eq!(
                style.fill_rgb.as_deref(),
                Some("#FFFF00"),
                "A row {r} (value matches >5) carries the dxf fill"
            );
            assert!(style.bold, "A row {r} dxf bold");
            assert_eq!(
                style.text_rgb.as_deref(),
                Some("#FF0000"),
                "A row {r} dxf text"
            );
        } else {
            // Non-matching cells keep the base (default) style → no fill.
            assert_eq!(style.fill_rgb, None, "A row {r} (value <= 5) unpainted");
        }
    }

    // Column B values: 150, 50, 200, 99, 120 → >100 matches rows 0, 2, 4
    // (the reducible `expression` rule), painted with dxf 1 (green).
    let painted_b = [0usize, 2, 4];
    for r in 0..5 {
        let want = if painted_b.contains(&r) {
            Some("#00FF00")
        } else {
            None
        };
        assert_eq!(fill_at(&lc, r, 1).as_deref(), want, "B row {r}");
    }
}

// ── sheet.lower.condfmt.colorscale ──────────────────────────────────────────

/// A 2-colour scale (white→red) over column C interpolates each cell's fill
/// across the column's value domain. C values 0,50,100,25,75 over domain
/// [0,100]: 0→white, 100→red, 50→halfway (#FF8080).
#[test]
fn sheet_lower_condfmt_colorscale() {
    let doc = open_fixture();
    let lc = lower_full(&doc);

    // Domain of column C is [0, 100]; white #FFFFFF → red #FF0000.
    // value 0   → #FFFFFF
    assert_eq!(fill_at(&lc, 0, 2).as_deref(), Some("#FFFFFF"));
    // value 100 → #FF0000
    assert_eq!(fill_at(&lc, 2, 2).as_deref(), Some("#FF0000"));
    // value 50  → halfway: R=0xFF, G=round(0xFF*0.5)=0x80, B=0x80 → #FF8080
    assert_eq!(fill_at(&lc, 1, 2).as_deref(), Some("#FF8080"));
    // value 25  → quarter toward red: G=B=round(0xFF*0.75)=0xBF → #FFBFBF
    assert_eq!(fill_at(&lc, 3, 2).as_deref(), Some("#FFBFBF"));
    // value 75  → three-quarters: G=B=round(0xFF*0.25)=0x40 → #FF4040
    assert_eq!(fill_at(&lc, 4, 2).as_deref(), Some("#FF4040"));
}

// ── sheet.lower.condfmt.lower-to-style-override ──────────────────────────────

/// The shared lowering seam: cf results lower as constrained local overrides on
/// the IR-v2 styles table (spec §8.3). Cells sharing a base style but different
/// cf outcomes get DIFFERENT `style_key`s; an empty cf reproduces the styled
/// path exactly; the override layers onto (does not erase) the base style.
#[test]
fn sheet_lower_condfmt_lower_to_style_override() {
    let doc = open_fixture();
    let lc = lower_full(&doc);

    // Cells A1 (matches >5) and A2 (does not) share the default base style but
    // get DIFFERENT keys — the cf-aware path expresses what a StyleId-keyed
    // resolver cannot (same StyleId, different effective style).
    let k_match = lc.rows[0].cells[0].style_key;
    let k_nomatch = lc.rows[1].cells[0].style_key;
    assert_ne!(k_match, k_nomatch, "matched vs unmatched get distinct keys");
    assert_eq!(k_nomatch, 0, "unmatched default cell stays key 0");

    // Identical cf outcomes dedup to ONE table entry: A1 and A3 both match >5.
    assert_eq!(
        lc.rows[0].cells[0].style_key, lc.rows[2].cells[0].style_key,
        "two cells with the same cf outcome share a key (dedup)"
    );

    // Determinism (spec §12.4): two runs are serde_json-byte-equal.
    let again = lower_full(&doc);
    assert_eq!(
        serde_json::to_string(&lc).unwrap(),
        serde_json::to_string(&again).unwrap()
    );

    // An EMPTY cf reproduces the plain styled path exactly (no extra keys).
    let plain = lower_range_condfmt(
        &doc.model,
        0,
        CellRange {
            r0: 0,
            c0: 0,
            r1: 4,
            c1: 4,
        },
        &ViewOptions::default(),
        &doc.visual_styles,
        &sheet_lower::SheetCondFmt::default(),
    );
    let styled = sheet_lower::lower_range_styled(
        &doc.model,
        0,
        CellRange {
            r0: 0,
            c0: 0,
            r1: 4,
            c1: 4,
        },
        &ViewOptions::default(),
        &doc.visual_styles,
    );
    assert_eq!(plain, styled, "empty cf == lower_range_styled");
}

// ── sheet.lower.condfmt.databar (drawn-rect GEOMETRY lane) ───────────────────

/// A dataBar rule lowers to a DRAWN RECT (the page-draw geometry lane, spec
/// §8.2) — NOT a style fill. So column D cells keep their base style (no fill
/// override), and the lowered content carries one `DataBarRect` per numeric D
/// cell, proportional to the column's value domain. The rule still round-trips
/// byte-identical (asserted in `sheet_xlsx_conditional_formatting`).
#[test]
fn sheet_lower_condfmt_databar() {
    let doc = open_fixture();
    let lc = lower_full(&doc);

    // Style path: column D under a dataBar applies NO style fill (it is drawn
    // geometry, not a cell fill).
    for r in 0..5 {
        assert_eq!(
            fill_at(&lc, r, 3),
            None,
            "D row {r}: dataBar paints no style fill (drawn-rect track)"
        );
    }

    // The lowered cf carries the data bar as a GEOMETRY rule (not Preserved):
    // block 3 (column D) is the dataBar; it resolves to a `sheet_lower::DataBar`.
    let low = doc.lowered_conditional_formats(0);
    assert!(
        matches!(
            low.blocks[3].rules[0].kind,
            sheet_lower::CfRuleKind::DataBar(_)
        ),
        "the dataBar rule lowers to the geometry lane"
    );

    // The lowered content carries one drawn rect per numeric D cell (col index
    // 3 in the A1:E5 range). D values are 10,40,70,20,90 over domain [10,90].
    let d_bars: Vec<&sheet_lower::DataBarRect> =
        lc.databars.iter().filter(|b| b.col == 3).collect();
    assert_eq!(d_bars.len(), 5, "one data bar rect per D-column cell");

    // The fractions are (value - 10) / (90 - 10): D1=10→0.0, D3=70→0.75,
    // D5=90→1.0. Bars are sorted by the row-major lowering walk (row asc).
    let frac_at = |row: u32| d_bars.iter().find(|b| b.row == row).unwrap().fill_fraction;
    assert!((frac_at(0) - 0.0).abs() < 1e-9, "D1 value 10 → 0%");
    assert!((frac_at(2) - 0.75).abs() < 1e-9, "D3 value 70 → 75%");
    assert!((frac_at(4) - 1.0).abs() < 1e-9, "D5 value 90 → 100%");

    // Each bar is the document-default blue #638EC6 (the fixture's bar colour)
    // and is positioned in content-space (x/y inside the D column's cell box).
    for b in &d_bars {
        assert_eq!(b.fill, "#638EC6");
        assert!(b.w >= 0.0 && b.h > 0.0);
        // The bar's width never exceeds the cell's available width.
        let cell_w = lc.cols[3].width_pt;
        assert!(b.w <= cell_w, "bar width within the cell");
    }

    // Determinism: two runs are serde_json-byte-equal (databars included).
    let again = lower_full(&doc);
    assert_eq!(
        serde_json::to_string(&lc).unwrap(),
        serde_json::to_string(&again).unwrap()
    );
}

// ── sheet.lower.condfmt.iconset (preserve-only T2 floor) ─────────────────────

/// An iconSet rule is preserve-only in T2: parsed + round-tripped, never
/// rendered (no icon-glyph asset pipeline). Its cells keep their base style.
#[test]
fn sheet_lower_condfmt_iconset() {
    let doc = open_fixture();
    let lc = lower_full(&doc);
    for r in 0..5 {
        assert_eq!(
            fill_at(&lc, r, 4),
            None,
            "E row {r}: iconSet paints no style override (preserve-only)"
        );
    }
    // Parsed as IconSet (so a later tier can render it); lowers to Preserved.
    let doc2 = open_fixture();
    let cf = doc2.conditional_formats.get(&0).unwrap();
    assert!(matches!(cf.blocks[4].rules[0].kind, CfRuleKind::IconSet));
    let low = doc2.lowered_conditional_formats(0);
    assert_eq!(
        low.blocks[4].rules[0].kind,
        sheet_lower::CfRuleKind::Preserved
    );
}
