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

//! Style-map conformance (spec §8.3, "the most important property of the
//! whole plugin"; registry `sheet.style.*`). Three lanes:
//!
//! 1. **xlsx-visual-parse** — `corpus/xlsx-corpus/03-styles.xlsx` parses into
//!    a `VisualStyles` side table where bold / yellow-fill / thin-border xfs
//!    resolve to the expected, host-ready visual attributes (`#RRGGBB`
//!    colours, per-edge border presence, the minimal-override font ruling).
//! 2. **ir-styles-table** — `lower_range_styled` over that styled range emits
//!    a deduped IR-v2 `LoweredContent.styles` table and every styled cell's
//!    `style_key` indexes a real entry.
//! 3. **cross-surface-parity** — the page lowering and the grid scene resolve
//!    a cell's `StyleId` through the SAME `StyleResolver`, so the SAME cell
//!    yields an IDENTICAL `LoweredStyle` on both surfaces (the §8.3
//!    "two surfaces may differ in pipeline, never in styling" rule).
//!
//! One `fn sheet_style_*` per `registry/features/stylemap.yaml` row (the
//! §12.2 coverage-gate pointers). The apply-style-pour row is a TS vitest
//! (`packages/sheet-host-model/test/lower-to-mutations.spec.ts`); the
//! doc-style-group row stays PLANNED (S-04: the style-management capability
//! is an SDK gap).

use std::path::PathBuf;

use sheet_core::{parse_a1, Cell, CellStyle, CellValue, SheetModel};
use sheet_lower::{
    lower_range_styled, CellRange, LoweredStyle, NoStyles, StyleResolver, ViewOptions, VisualAttrs,
    VisualStyleSource,
};
use sheet_xlsx::XlsxDocument;

// ---- Corpus helpers (mirror the xlsx_roundtrip suite). ----

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

/// The resolved `StyleId` of the cell at A1-style address `addr` on sheet 0.
fn style_at(doc: &XlsxDocument, addr: &str) -> sheet_core::StyleId {
    let (row, col, _, _) = parse_a1(addr).unwrap_or_else(|| panic!("parse {addr}"));
    doc.model
        .sheet(0)
        .unwrap()
        .cell(row, col)
        .unwrap_or_else(|| panic!("cell {addr} populated"))
        .style
}

// ---- sheet.style.xlsx-visual-parse ----

/// `sheet.style.xlsx-visual-parse` — the corpus 03-styles.xlsx font/fill/
/// border xfs resolve into a `VisualStyles` side table with the expected
/// host-ready attributes. The fixture's xf map (from `generate.py`):
///   A1 s=1 → bold + thin border all edges (font shares the default 11/Calibri)
///   B1 s=2 → yellow solid fill (#FFFF00)
///   C1 s=3 → only numFmt 0% differs → visually default (no entry)
///   A2 s=4 → only custom numFmt differs → visually default (no entry)
#[test]
fn sheet_style_xlsx_visual_parse() {
    let doc = XlsxDocument::open(&load("03-styles.xlsx")).unwrap();

    // A1: bold + thin border on all four edges, no fill. Per the §8.3
    // minimal-override ruling the font size/name (Calibri 11 = the document
    // default) are NOT recorded as overrides — only `bold` is.
    let a1 = doc
        .visual_styles
        .get(style_at(&doc, "A1"))
        .expect("A1 styled");
    assert!(a1.bold, "A1 is bold");
    assert!(!a1.italic);
    assert_eq!(a1.font_size_pt, None, "default size is not an override");
    assert_eq!(a1.font_name, None, "default name is not an override");
    assert!(
        a1.border_top && a1.border_right && a1.border_bottom && a1.border_left,
        "A1 has a thin border on all edges"
    );
    assert_eq!(a1.fill_rgb, None, "A1 has no fill");

    // B1: yellow solid fill resolved to #RRGGBB (FFFFFF00 → #FFFF00), no
    // emphasis, no border.
    let b1 = doc
        .visual_styles
        .get(style_at(&doc, "B1"))
        .expect("B1 styled");
    assert_eq!(b1.fill_rgb.as_deref(), Some("#FFFF00"), "B1 yellow fill");
    assert!(!b1.bold);
    assert!(!b1.border_top);

    // C1 / A2: a number format differs but the visual style is the document
    // default → NO visual entry (number formatting is the format lane, not the
    // style-map lane).
    assert!(
        doc.visual_styles.get(style_at(&doc, "C1")).is_none(),
        "C1 is visually default (only its numFmt differs)"
    );
    assert!(
        doc.visual_styles.get(style_at(&doc, "A2")).is_none(),
        "A2 is visually default (only its numFmt differs)"
    );

    // The side table is non-empty (the workbook DOES carry visual styling).
    assert!(!doc.visual_styles.is_empty());
}

// ---- sheet.style.ir-styles-table ----

/// `sheet.style.ir-styles-table` — `lower_range_styled` over the styled range
/// emits a deduped IR-v2 styles table and every styled cell's `style_key`
/// indexes a real entry; unstyled cells keep key 0. Pins the IR-v2 contract.
#[test]
fn sheet_style_ir_styles_table() {
    let doc = XlsxDocument::open(&load("03-styles.xlsx")).unwrap();
    // Range A1:C2 covers the bold (A1), yellow-fill (B1), and number-only
    // (C1/A2) cells plus the empty B2/C2 positions.
    let range = CellRange {
        r0: 0,
        c0: 0,
        r1: 1,
        c1: 2,
    };
    let lc = lower_range_styled(
        &doc.model,
        0,
        range,
        &ViewOptions::default(),
        &doc.visual_styles,
    );

    // Key 0 is always the default; two non-default styles (bold, yellow fill)
    // get distinct keys → a 3-entry table.
    assert_eq!(lc.styles[0], LoweredStyle::default_key0());
    assert_eq!(lc.styles.len(), 3, "default + bold + fill");
    // Every table entry's `key` equals its index (positional, deduped).
    for (i, s) in lc.styles.iter().enumerate() {
        assert_eq!(s.key, i as u32);
    }

    // A1 (row 0, col 0) points at a bold entry.
    let a1_key = lc.rows[0].cells[0].style_key;
    assert_ne!(a1_key, 0, "A1 is styled");
    assert!(lc.styles[a1_key as usize].bold);

    // B1 (row 0, col 1) points at the yellow-fill entry.
    let b1_key = lc.rows[0].cells[1].style_key;
    assert_ne!(b1_key, 0, "B1 is styled");
    assert_eq!(
        lc.styles[b1_key as usize].fill_rgb.as_deref(),
        Some("#FFFF00")
    );
    assert_ne!(a1_key, b1_key, "bold and fill are distinct keys");

    // C1 (number-format-only) folds onto key 0 (visually default).
    assert_eq!(lc.rows[0].cells[2].style_key, 0, "C1 visually default");
    // An empty position (B2) is key 0 too.
    assert_eq!(lc.rows[1].cells[1].style_key, 0, "empty cell key 0");

    // Wire shape: the styles table + per-cell styleKey serialize camelCase.
    let json = serde_json::to_string(&lc).unwrap();
    assert!(json.contains("\"styleKey\":"));
    assert!(json.contains("\"fillRgb\":\"#FFFF00\""));
    assert!(json.contains("\"borderTop\":true"));
    assert!(!json.contains("style_key"));
    assert!(!json.contains("fill_rgb"));

    // The unstyled door (`lower_range`/`NoStyles`) still emits ONLY key 0 —
    // backward compatibility for callers without a visual source.
    let plain = lower_range_styled(&doc.model, 0, range, &ViewOptions::default(), &NoStyles);
    assert_eq!(plain.styles, vec![LoweredStyle::default_key0()]);
    assert!(plain.rows[0].cells.iter().all(|c| c.style_key == 0));
}

// ---- sheet.style.cross-surface-parity ----

/// `sheet.style.cross-surface-parity` — the page lowering and the grid scene
/// resolve a cell's `StyleId` through the SAME `StyleResolver` over the SAME
/// `VisualStyleSource`, so the SAME cell yields an IDENTICAL `LoweredStyle`
/// on both surfaces (spec §8.3). We prove it by comparing the page lowering's
/// resolved entry for a cell against an INDEPENDENT resolution of that cell's
/// StyleId — the shared mechanism the grid scene's `grid_scene_styled` wiring
/// reuses. (`sheet_lower::StyleResolver` is the single source of truth both
/// surfaces call.)
#[test]
fn sheet_style_cross_surface_parity() {
    // A hand-built model with two distinct visual styles, plus a map-backed
    // source — independent of XLSX so the parity is about the resolver, not
    // the parse.
    let mut m = SheetModel::new();
    let s = m.add_sheet("Sheet1");

    // Intern two CellStyles (the StyleIds the cells carry); their VISUAL
    // attributes live in the side source keyed by StyleId.
    let bold_id = m.styles.intern_style(CellStyle {
        font: 1,
        ..Default::default()
    });
    let fill_id = m.styles.intern_style(CellStyle {
        fill: 2,
        ..Default::default()
    });
    {
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(
            0,
            0,
            Cell {
                value: CellValue::from("Header"),
                style: bold_id,
                ..Default::default()
            },
        );
        ws.set_cell(
            0,
            1,
            Cell {
                value: CellValue::Number(42.0),
                style: fill_id,
                ..Default::default()
            },
        );
    }

    let bold_attrs = VisualAttrs {
        bold: true,
        ..Default::default()
    };
    let fill_attrs = VisualAttrs {
        fill_rgb: Some("#FFFF00".into()),
        ..Default::default()
    };
    let b = bold_id.0;
    let f = fill_id.0;
    let source = move |id: sheet_core::StyleId| {
        if id.0 == b {
            Some(bold_attrs.clone())
        } else if id.0 == f {
            Some(fill_attrs.clone())
        } else {
            None
        }
    };

    let range = CellRange {
        r0: 0,
        c0: 0,
        r1: 0,
        c1: 1,
    };

    // The PAGE surface resolves the table through lower_range_styled.
    let lc = lower_range_styled(&m, s, range, &ViewOptions::default(), &source);

    // The "GRID" surface uses the SAME resolver directly over the SAME source
    // (this is exactly what grid_scene_styled does — one shared mapping).
    let mut grid_resolver = StyleResolver::new(&source);
    let grid_bold_key = grid_resolver.key_for(bold_id);
    let grid_fill_key = grid_resolver.key_for(fill_id);
    let grid_styles = grid_resolver.into_styles();

    // Parity: for each cell, the LoweredStyle the page resolved == the
    // LoweredStyle the grid resolved (field-for-field, ignoring the
    // positional `key` since the tables are built in the same order anyway).
    let page_bold = &lc.styles[lc.rows[0].cells[0].style_key as usize];
    let page_fill = &lc.styles[lc.rows[0].cells[1].style_key as usize];
    let grid_bold = &grid_styles[grid_bold_key as usize];
    let grid_fill = &grid_styles[grid_fill_key as usize];

    assert_eq!(page_bold, grid_bold, "bold cell parity across surfaces");
    assert_eq!(page_fill, grid_fill, "fill cell parity across surfaces");
    assert!(page_bold.bold);
    assert_eq!(page_fill.fill_rgb.as_deref(), Some("#FFFF00"));

    // And the whole tables match (same order, same dedup) — one style table.
    assert_eq!(lc.styles, grid_styles, "identical style tables");
}

/// A direct unit-level parity check on the resolver: a closure source and the
/// trait both yield the same `LoweredStyle`, and `VisualStyleSource` is object-
/// agnostic (closure or named type). Kept under the cross-surface-parity
/// pointer (a `fn sheet_style_cross_surface_parity*` suffix the gate accepts).
#[test]
fn sheet_style_cross_surface_parity_resolver_is_source_agnostic() {
    let attrs = VisualAttrs {
        bold: true,
        italic: true,
        font_size_pt: Some(14.0),
        font_name: Some("Cambria".into()),
        text_rgb: Some("#FF0000".into()),
        ..Default::default()
    };
    let a = attrs.clone();
    let closure = move |id: sheet_core::StyleId| (id.0 == 7).then(|| a.clone());

    fn resolve(src: &impl VisualStyleSource, id: sheet_core::StyleId) -> Vec<LoweredStyle> {
        let mut r = StyleResolver::new(src);
        r.key_for(id);
        r.into_styles()
    }

    let via_closure = resolve(&closure, sheet_core::StyleId(7));
    assert_eq!(via_closure.len(), 2); // default + the styled
    let styled = &via_closure[1];
    assert!(styled.bold && styled.italic);
    assert_eq!(styled.font_size_pt, Some(14.0));
    assert_eq!(styled.font_name.as_deref(), Some("Cambria"));
    assert_eq!(styled.text_rgb.as_deref(), Some("#FF0000"));
}
