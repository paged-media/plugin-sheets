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

//! Chart conformance (M2 charts track, spec §8.4). Test-fn names are the
//! registry pointers for `registry/features/chart.yaml` (the coverage gate
//! greps these prefixes). Covers: the frozen chart MODEL, the XLSX chart-part
//! parse + round-trip preservation (`corpus/xlsx-corpus/09-chart.xlsx`), the
//! pure geometry generator for every curated kind, the geometry IR wire shape
//! the paged.draw TS lowering consumes, the grid-view projection (the same
//! generator), and live-to-recalc regeneration.
//!
//! The geometry generator is PURE Rust (`sheet_chart::generate`); its
//! `paged.draw` LOWERING half is the TS `chartGeometryToMutations`
//! (`packages/sheet-host-model/test/chart.spec.ts`, the registry's vitest
//! pointer). This file proves the Rust contract that lowering translates.

use std::path::PathBuf;

use sheet_chart::model::{Axis, ChartKind, ChartModel, Series};
use sheet_chart::{generate, ChartGeometry, PlotData, Primitive};
use sheet_core::{CellRef, RangeRef};
use sheet_js::core::SheetSession;
use sheet_xlsx::XlsxDocument;

// ── fixtures ────────────────────────────────────────────────────────────────

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

/// A `RangeRef` on sheet 0 from 0-based corners.
fn rr(r0: u32, c0: u32, r1: u32, c1: u32) -> RangeRef {
    let cr = |row, col| CellRef {
        sheet: 0,
        row,
        col,
        row_abs: false,
        col_abs: false,
    };
    RangeRef {
        start: cr(r0, c0),
        end: cr(r1, c1),
    }
}

fn count<F: Fn(&Primitive) -> bool>(g: &ChartGeometry, f: F) -> usize {
    g.prims.iter().filter(|p| f(p)).count()
}

// ── sheet.chart.model ───────────────────────────────────────────────────────

/// The frozen chart MODEL: kind + series/axis bindings + legend construct and
/// expose their fields. A model is the IR the geometry generator projects.
#[test]
fn sheet_chart_model() {
    let m = ChartModel {
        kind: ChartKind::Column,
        title: Some("Q1 Revenue".into()),
        series: vec![Series {
            name: Some("Revenue".into()),
            categories: Some(rr(1, 0, 3, 0)),
            values: rr(1, 1, 3, 1),
            color: Some("#3366CC".into()),
        }],
        cat_axis: Axis::default(),
        val_axis: Axis {
            title: Some("EUR".into()),
            min: Some(0.0),
            max: None,
        },
        legend: true,
    };
    assert_eq!(m.kind, ChartKind::Column);
    assert_eq!(m.series.len(), 1);
    assert_eq!(m.series[0].values.rows(), 3);
    assert_eq!(m.series[0].color.as_deref(), Some("#3366CC"));
    assert!(m.legend);
    assert_eq!(m.val_axis.min, Some(0.0));
}

// ── sheet.chart.xlsx-part ───────────────────────────────────────────────────

/// The XLSX chart part parses into the chart model: 09-chart.xlsx's column
/// barChart is discovered through the worksheet→drawing→chart relationship
/// chain, its series binds to the right Sheet1 ranges, and the title / legend /
/// color / cached series name come through. The chart + drawing parts stay
/// OPAQUE so a zero-edit round-trip is byte-identical (preservation invariant).
#[test]
fn sheet_chart_xlsx_part() {
    let doc = XlsxDocument::open(&load("09-chart.xlsx")).expect("open 09-chart");
    assert_eq!(
        doc.charts.len(),
        1,
        "one chart discovered via the rels chain"
    );
    let chart = &doc.charts[0];
    assert_eq!(chart.host_sheet, 0, "anchored to Sheet1");

    let m = &chart.model;
    assert_eq!(m.kind, ChartKind::Column);
    assert_eq!(m.title.as_deref(), Some("Q1 Revenue"));
    assert!(m.legend, "the <c:legend> is present");
    assert_eq!(m.series.len(), 1);
    let s = &m.series[0];
    assert_eq!(s.name.as_deref(), Some("Revenue"));
    assert_eq!(s.color.as_deref(), Some("#3366CC"));
    // values = Sheet1!$B$2:$B$4 → rows 1..3, col 1.
    assert_eq!(s.values.start.row, 1);
    assert_eq!(s.values.start.col, 1);
    assert_eq!(s.values.end.row, 3);
    // categories = Sheet1!$A$2:$A$4 → col 0.
    let cats = s.categories.expect("categories range");
    assert_eq!(cats.start.col, 0);
    assert_eq!(cats.end.row, 3);

    // Round-trip preserves the OPAQUE chart + drawing parts byte-identical.
    let out = doc.save().expect("save");
    let orig = unzip(&load("09-chart.xlsx"));
    let saved = unzip(&out);
    for part in ["xl/charts/chart1.xml", "xl/drawings/drawing1.xml"] {
        let a = orig.iter().find(|(n, _)| n == part).map(|(_, b)| b);
        let b = saved.iter().find(|(n, _)| n == part).map(|(_, b)| b);
        assert_eq!(a, b, "{part} must round-trip byte-identical (preservation)");
    }
    // The chart re-parses from the saved bytes too (the model still resolves).
    let doc2 = XlsxDocument::open(&out).expect("reopen saved 09-chart");
    assert_eq!(doc2.charts.len(), 1);
    assert_eq!(doc2.charts[0].model.kind, ChartKind::Column);
}

/// Unzip a package to `(name, bytes)` (skips dirs) — for the per-part identity
/// assertion.
fn unzip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
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

// ── sheet.chart.geometry.* ──────────────────────────────────────────────────

/// A 3-category, single-series model + its resolved data (10, 20, 30).
fn three_value_model(kind: ChartKind) -> (ChartModel, PlotData) {
    let model = ChartModel {
        kind,
        title: None,
        series: vec![Series {
            name: Some("S".into()),
            categories: Some(rr(1, 0, 3, 0)),
            values: rr(1, 1, 3, 1),
            color: Some("#4E79A7".into()),
        }],
        cat_axis: Axis::default(),
        val_axis: Axis::default(),
        legend: false,
    };
    let data = PlotData {
        series: vec![vec![10.0, 20.0, 30.0]],
        categories: vec!["Q1".into(), "Q2".into(), "Q3".into()],
    };
    (model, data)
}

/// Bar/column geometry: one bar Rect per (series, category), an axis frame, and
/// tick/category labels — the proven generator, here for both orientations.
#[test]
fn sheet_chart_geometry_bar_column() {
    for kind in [ChartKind::Column, ChartKind::Bar] {
        let (model, data) = three_value_model(kind);
        let g = generate(&model, &data, 300.0, 200.0);
        let bars = count(&g, |p| matches!(p, Primitive::Rect { .. }));
        assert_eq!(bars, 3, "{kind:?}: 3 bars");
        let lines = count(&g, |p| matches!(p, Primitive::Line { .. }));
        assert_eq!(lines, 2, "{kind:?}: the cartesian axis frame");
        // The bars carry the explicit series color.
        let colored = g
            .prims
            .iter()
            .any(|p| matches!(p, Primitive::Rect { fill: Some(f), .. } if f == "#4E79A7"));
        assert!(colored, "{kind:?}: bars use the series swatch");
    }
}

/// Line/area geometry: a polyline per series; area additionally fills a closed
/// baseline polygon.
#[test]
fn sheet_chart_geometry_line_area() {
    let (line_model, data) = three_value_model(ChartKind::Line);
    let gl = generate(&line_model, &data, 300.0, 200.0);
    // 2 axis rules + 1 series polyline = 3 Line prims; no fill polygon.
    assert_eq!(count(&gl, |p| matches!(p, Primitive::Line { .. })), 3);
    assert_eq!(count(&gl, |p| matches!(p, Primitive::Polygon { .. })), 0);

    let (area_model, data) = three_value_model(ChartKind::Area);
    let ga = generate(&area_model, &data, 300.0, 200.0);
    assert_eq!(count(&ga, |p| matches!(p, Primitive::Line { .. })), 3);
    // The area band is one closed polygon (the series dropped to baseline).
    assert_eq!(count(&ga, |p| matches!(p, Primitive::Polygon { .. })), 1);
}

/// Pie/donut geometry: one Wedge per slice with proportional angles; donut adds
/// a center-hole disc.
#[test]
fn sheet_chart_geometry_pie_donut() {
    let (pie_model, data) = three_value_model(ChartKind::Pie);
    let gp = generate(&pie_model, &data, 200.0, 200.0);
    let wedges: Vec<(f64, f64)> = gp
        .prims
        .iter()
        .filter_map(|p| match p {
            Primitive::Wedge {
                start_deg, end_deg, ..
            } => Some((*start_deg, *end_deg)),
            _ => None,
        })
        .collect();
    assert_eq!(wedges.len(), 3, "pie: one wedge per slice, no hole");
    // Shares 10/20/30 of 60 => 60°, 120°, 180° cumulative to 360°.
    assert!((wedges[0].1 - 60.0).abs() < 1e-6);
    assert!((wedges[2].1 - 360.0).abs() < 1e-6, "full sweep");

    let (donut_model, data) = three_value_model(ChartKind::Donut);
    let gd = generate(&donut_model, &data, 200.0, 200.0);
    // 3 slices + 1 center-hole disc.
    assert_eq!(count(&gd, |p| matches!(p, Primitive::Wedge { .. })), 4);
}

/// Scatter geometry: a diamond marker (4-point polygon) per (x, y) point; X
/// from series 0, Y from series 1; no category axis.
#[test]
fn sheet_chart_geometry_scatter() {
    let model = ChartModel {
        kind: ChartKind::Scatter,
        title: None,
        series: vec![
            Series {
                name: Some("X".into()),
                categories: None,
                values: rr(0, 0, 2, 0),
                color: None,
            },
            Series {
                name: Some("Y".into()),
                categories: None,
                values: rr(0, 1, 2, 1),
                color: None,
            },
        ],
        cat_axis: Axis::default(),
        val_axis: Axis::default(),
        legend: false,
    };
    let data = PlotData {
        series: vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]],
        categories: vec![],
    };
    let g = generate(&model, &data, 300.0, 200.0);
    let markers = count(&g, |p| matches!(p, Primitive::Polygon { .. }));
    assert_eq!(markers, 3, "one diamond marker per (x,y) point");
    assert_eq!(count(&g, |p| matches!(p, Primitive::Rect { .. })), 0);
}

// ── sheet.chart.lower.paged-draw ────────────────────────────────────────────

/// The geometry IR serializes to the camelCase wire shape the TS paged.draw
/// lowering (`chartGeometryToMutations`) consumes: `{widthPt, heightPt, prims:
/// [{kind, ...}]}` with lowercase primitive tags and camelCase fields. This is
/// the Rust contract the TS translator (registry vitest pointer
/// `packages/sheet-host-model/test/chart.spec.ts`) turns into insertPath /
/// insertTextFrame native ops — paged.draw is a CORE SDK surface, never another
/// plugin (spec §2.1).
#[test]
fn sheet_chart_lower_paged_draw() {
    let (model, data) = three_value_model(ChartKind::Pie);
    let g = generate(&model, &data, 200.0, 200.0);
    let json = serde_json::to_value(&g).expect("serialize geometry");

    // The content-box size crosses the wire as camelCase scalars.
    assert_eq!(json["widthPt"], 200.0);
    assert_eq!(json["heightPt"], 200.0);
    let prims = json["prims"].as_array().expect("prims array");
    assert!(!prims.is_empty());

    // A wedge primitive carries the lowercase tag + camelCase angle fields the
    // TS `WedgePrim` mirror expects (the lowering flattens these into an arc).
    let wedge = prims
        .iter()
        .find(|p| p["kind"] == "wedge")
        .expect("a wedge prim");
    assert!(wedge.get("cx").is_some());
    assert!(wedge.get("startDeg").is_some(), "camelCase startDeg");
    assert!(wedge.get("endDeg").is_some(), "camelCase endDeg");

    // A bar/column rect carries the camelCase `strokeW` field name.
    let (bar_model, bar_data) = three_value_model(ChartKind::Column);
    let bg = generate(&bar_model, &bar_data, 300.0, 200.0);
    let bjson = serde_json::to_value(&bg).unwrap();
    let rect = bjson["prims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "rect")
        .expect("a rect prim");
    assert!(rect.get("strokeW").is_some(), "camelCase strokeW");
    assert!(rect.get("fill").is_some());
}

// ── sheet.chart.grid-view ───────────────────────────────────────────────────

/// The grid-view projection is the SAME generator: the engine resolves a parsed
/// chart's series ranges against the live model and runs `sheet_chart::generate`
/// (the §8.1 surface consumes the identical IR the page lowering does — one
/// generator, two projections). Drive it through the engine session for the
/// corpus chart and assert real geometry comes back.
#[test]
fn sheet_chart_grid_view() {
    let session = SheetSession::load_xlsx(&load("09-chart.xlsx")).expect("load 09-chart");
    let charts = session.list_charts();
    assert_eq!(charts.len(), 1, "the corpus chart is listed");
    assert_eq!(charts[0].kind, "column");
    assert_eq!(charts[0].series_count, 1);
    assert_eq!(charts[0].title.as_deref(), Some("Q1 Revenue"));

    let g = session
        .get_chart_geometry(0, 300.0, 200.0)
        .expect("geometry for chart 0");
    assert_eq!(g.width_pt, 300.0);
    assert_eq!(g.height_pt, 200.0);
    // The column chart's values (10, 20, 30) resolve to 3 bars. The corpus
    // chart has a legend on, which adds a small swatch Rect (7pt-wide); count
    // bars only (the wide Rects), excluding the legend swatch.
    let bars = count(&g, |p| matches!(p, Primitive::Rect { w, .. } if *w > 7.5));
    assert_eq!(bars, 3, "3 bars for the 3 resolved values");

    // An out-of-range chart index is a boundary error (finding-2 discipline).
    assert!(session.get_chart_geometry(9, 100.0, 100.0).is_err());
}

// ── sheet.chart.recalc-live ─────────────────────────────────────────────────

/// Charts are LIVE to recalculation (spec §8.4): editing a value cell in a
/// series' range re-resolves the ranges and regenerates the geometry. The
/// tallest bar tracks the changed value.
#[test]
fn sheet_chart_recalc_live() {
    let mut session = SheetSession::load_xlsx(&load("09-chart.xlsx")).expect("load 09-chart");

    // Before the edit the values are 10/20/30; the chart geometry is fixed.
    let before = session.get_chart_geometry(0, 300.0, 200.0).expect("geom");

    // Edit B4 (row 3, col 1) to 100 — far above the old max (30).
    session.set_cell(0, 3, 1, "100").expect("set B4");

    let after = session.get_chart_geometry(0, 300.0, 200.0).expect("geom2");

    // The geometry REGENERATED against the recalculated value — it is no longer
    // byte-identical to the pre-edit geometry (live to recalc, spec §8.4).
    assert_ne!(before, after, "chart geometry tracks recalculated values");

    // The new value (100) is now the scale max, so its bar reaches the plot top
    // — the tallest bar now spans (near) the full plot height.
    let tallest_after = max_bar_height(&after);
    // Plot height = 200 - PAD_TOP(20) - PAD_BOTTOM(24) = 156; the 100 bar at a
    // 0..100 scale spans the whole plot.
    assert!(
        tallest_after > 150.0,
        "the edited value's bar spans the plot ({tallest_after})"
    );
}

/// The tallest bar Rect height in a chart geometry (0 when there are no bars).
fn max_bar_height(g: &ChartGeometry) -> f64 {
    g.prims
        .iter()
        .filter_map(|p| match p {
            Primitive::Rect { h, .. } => Some(*h),
            _ => None,
        })
        .fold(0.0_f64, f64::max)
}
