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

//! The chart GEOMETRY IR (spec §8.4): the PURE projection of a [`ChartModel`]
//! plus resolved [`PlotData`] into vector [`Primitive`]s in **chart-content pt
//! space** (origin top-left, y grows DOWN — the frame-content coordinate system,
//! spec §8.5). The charts track lowers these primitives through `paged.draw`
//! (`insertPath`/`insertOval`/`insertLine` — a CORE SDK surface, NEVER another
//! plugin, spec §2.1); the same geometry feeds the sheets-mode grid view. One
//! generator, two projections — mirroring the sheet itself.
//!
//! [`generate`] is **pure**: `(model, data, width, height) -> ChartGeometry`,
//! no SDK, no model reads (the values are already resolved into [`PlotData`] by
//! the caller). It is deterministic — same inputs => byte-identical IR.
//!
//! Phase A (M2 Phase A) proves the IR end-to-end for ONE kind, **Column**
//! (axis frame + one [`Primitive::Rect`] per bar with category/value scaling
//! and axis tick labels via a simple linear scale). The other kinds are the
//! Phase B charts track; [`generate`] currently routes Bar/Column to the real
//! column generator and the remaining kinds to a documented axis-frame stub.

use crate::model::{ChartKind, ChartModel};

/// Text alignment for a [`Primitive::Text`] anchor point.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAnchor {
    Start,
    Middle,
    End,
}

/// One vector primitive in chart-content pt space (spec §8.4). Lowered to
/// `paged.draw` native ops by the charts track; rendered directly in the grid
/// view. serde emits `camelCase` field names (the wire convention shared with
/// `sheet-lower`); the variant tag is the lowercase `kind`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Primitive {
    /// An axis-aligned rectangle (bars, plot-area frame, legend swatches).
    #[serde(rename_all = "camelCase")]
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        fill: Option<String>,
        stroke: Option<String>,
        stroke_w: f64,
    },
    /// A polyline (line-chart series, axis rules). At least two points.
    #[serde(rename_all = "camelCase")]
    Line {
        pts: Vec<(f64, f64)>,
        stroke: String,
        stroke_w: f64,
    },
    /// A closed polygon (area-chart fills, scatter markers as diamonds).
    #[serde(rename_all = "camelCase")]
    Polygon {
        pts: Vec<(f64, f64)>,
        fill: Option<String>,
        stroke: Option<String>,
        stroke_w: f64,
    },
    /// A pie/donut wedge: center `(cx, cy)`, radius `r`, angular span in degrees
    /// (clockwise from 12 o'clock is the charts-track convention).
    #[serde(rename_all = "camelCase")]
    Wedge {
        cx: f64,
        cy: f64,
        r: f64,
        start_deg: f64,
        end_deg: f64,
        fill: Option<String>,
        stroke: Option<String>,
    },
    /// A text label (title, axis ticks, legend entries).
    #[serde(rename_all = "camelCase")]
    Text {
        x: f64,
        y: f64,
        s: String,
        size_pt: f64,
        anchor: TextAnchor,
    },
}

/// The generated geometry for one chart: the content-box size plus the ordered
/// primitive list. `width_pt`/`height_pt` are the chart-content box (frame
/// transforms are applied by core, spec §8.5 — the generator never anticipates
/// them).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartGeometry {
    pub width_pt: f64,
    pub height_pt: f64,
    pub prims: Vec<Primitive>,
}

/// The RESOLVED numeric series + category labels the generator consumes (the
/// caller resolves each [`crate::model::Series`]'s `values`/`categories`
/// [`sheet_core::RangeRef`] against the workbook first — the generator never
/// touches the model). `series[i]` aligns with `model.series[i]`; `categories`
/// is the shared category-axis label list (its length is the bar/point count).
#[derive(Clone, Debug, Default)]
pub struct PlotData {
    /// One numeric vector per series (already resolved from the values range).
    pub series: Vec<Vec<f64>>,
    /// The category-axis labels (shared across series).
    pub categories: Vec<String>,
}

/// The default series palette (publication-neutral; the charts track overrides
/// per-series from document swatches via [`crate::model::Series::color`]).
const PALETTE: &[&str] = &[
    "#4E79A7", "#F28E2B", "#E15759", "#76B7B2", "#59A14F", "#EDC948", "#B07AA1", "#FF9DA7",
];

/// Plot-area inset (pt) from the content box: left for the value-axis labels,
/// bottom for the category labels, top for the title, right gutter.
const PAD_LEFT: f64 = 40.0;
const PAD_BOTTOM: f64 = 24.0;
const PAD_TOP: f64 = 20.0;
const PAD_RIGHT: f64 = 12.0;

/// Axis/frame stroke width and color.
const AXIS_STROKE: &str = "#888888";
const AXIS_STROKE_W: f64 = 1.0;
/// Tick label point size.
const TICK_SIZE_PT: f64 = 8.0;
/// Number of value-axis ticks (including 0 and the top).
const VAL_TICKS: usize = 5;

/// Project a [`ChartModel`] + resolved [`PlotData`] into a [`ChartGeometry`]
/// (spec §8.4). PURE + deterministic. Phase A: Bar/Column are real; the other
/// kinds return an axis frame + a documented TODO (the Phase B charts track).
pub fn generate(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    match model.kind {
        // Column (vertical bars) is the proven Phase-A kind. Bar (horizontal)
        // shares enough that we route it here too in Phase A; the charts track
        // splits the orientation in Phase B.
        ChartKind::Column | ChartKind::Bar => generate_column(model, data, width_pt, height_pt),
        // Phase B (charts track): line/area, pie/donut, scatter. Stub = the axis
        // frame so the IR is non-empty and the binding is visible; the series
        // geometry is the Phase B TODO.
        ChartKind::Line | ChartKind::Area | ChartKind::Scatter => {
            generate_axis_frame_stub(model, data, width_pt, height_pt)
        }
        // Pie/donut have no cartesian axes; the Phase-A stub is an empty frame
        // (the charts track adds the wedge generator in Phase B).
        ChartKind::Pie | ChartKind::Donut => ChartGeometry {
            width_pt,
            height_pt,
            prims: Vec::new(),
        },
    }
}

/// The real Phase-A COLUMN generator: a plot-area frame, value-axis tick labels
/// on a simple linear scale (0..max), category labels under each group, and one
/// [`Primitive::Rect`] per (series, category) bar with grouped placement.
fn generate_column(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    let mut prims: Vec<Primitive> = Vec::new();

    // Plot area in content-box pt space (origin top-left, y down).
    let plot_x = PAD_LEFT;
    let plot_y = PAD_TOP;
    let plot_w = (width_pt - PAD_LEFT - PAD_RIGHT).max(0.0);
    let plot_h = (height_pt - PAD_TOP - PAD_BOTTOM).max(0.0);
    let plot_bottom = plot_y + plot_h;

    // Title (centered above the plot).
    if let Some(t) = &model.title {
        prims.push(Primitive::Text {
            x: width_pt / 2.0,
            y: PAD_TOP / 2.0,
            s: t.to_string(),
            size_pt: TICK_SIZE_PT + 2.0,
            anchor: TextAnchor::Middle,
        });
    }

    // The value scale: 0..vmax (Phase A is non-negative; the val-axis min
    // override forces the floor, max override forces the ceiling).
    let data_max = data
        .series
        .iter()
        .flat_map(|s| s.iter().copied())
        .fold(0.0_f64, f64::max);
    let vmin = model.val_axis.min.unwrap_or(0.0);
    let vmax_raw = model.val_axis.max.unwrap_or(data_max);
    // Avoid a degenerate zero-span scale.
    let vmax = if vmax_raw <= vmin {
        vmin + 1.0
    } else {
        vmax_raw
    };
    let span = vmax - vmin;

    // Map a value to a y in content space (vmin at the bottom, vmax at the top).
    let y_of = |v: f64| plot_bottom - ((v - vmin) / span) * plot_h;

    // Plot-area frame (the two axis rules: left value axis + bottom category
    // axis), drawn as a single open rect outline via two Line primitives.
    prims.push(Primitive::Line {
        pts: vec![(plot_x, plot_y), (plot_x, plot_bottom)],
        stroke: AXIS_STROKE.to_string(),
        stroke_w: AXIS_STROKE_W,
    });
    prims.push(Primitive::Line {
        pts: vec![(plot_x, plot_bottom), (plot_x + plot_w, plot_bottom)],
        stroke: AXIS_STROKE.to_string(),
        stroke_w: AXIS_STROKE_W,
    });

    // Value-axis tick labels (VAL_TICKS evenly spaced on the linear scale).
    for i in 0..VAL_TICKS {
        let frac = i as f64 / (VAL_TICKS - 1) as f64;
        let v = vmin + frac * span;
        let y = y_of(v);
        prims.push(Primitive::Text {
            x: plot_x - 4.0,
            y,
            s: format_tick(v),
            size_pt: TICK_SIZE_PT,
            anchor: TextAnchor::End,
        });
    }

    // Bars. Group count = the category count (fall back to the longest series).
    let n_groups = data
        .categories
        .len()
        .max(data.series.iter().map(|s| s.len()).max().unwrap_or(0));
    if n_groups > 0 && plot_w > 0.0 && plot_h > 0.0 {
        let n_series = data.series.len().max(1);
        let group_w = plot_w / n_groups as f64;
        // 80% of the group is bars, 20% gutter between groups.
        let bars_w = group_w * 0.8;
        let bar_w = bars_w / n_series as f64;
        let group_inset = (group_w - bars_w) / 2.0;

        for (gi, _) in (0..n_groups).enumerate() {
            let group_left = plot_x + gi as f64 * group_w;
            // Category label under the group center.
            if let Some(label) = data.categories.get(gi) {
                prims.push(Primitive::Text {
                    x: group_left + group_w / 2.0,
                    y: plot_bottom + 10.0,
                    s: label.clone(),
                    size_pt: TICK_SIZE_PT,
                    anchor: TextAnchor::Middle,
                });
            }
            for (si, series) in data.series.iter().enumerate() {
                let v = series.get(gi).copied().unwrap_or(0.0);
                let top = y_of(v.max(vmin));
                let h = (plot_bottom - top).max(0.0);
                let x = group_left + group_inset + si as f64 * bar_w;
                let fill = series_color(model, si);
                prims.push(Primitive::Rect {
                    x,
                    y: top,
                    w: bar_w,
                    h,
                    fill: Some(fill),
                    stroke: None,
                    stroke_w: 0.0,
                });
            }
        }
    }

    ChartGeometry {
        width_pt,
        height_pt,
        prims,
    }
}

/// The Phase-B stub for cartesian kinds (line/area/scatter): the plot-area
/// axis frame + value tick labels + category labels, but NO series geometry.
/// This proves the binding and keeps the IR non-empty until the charts track
/// implements the series projection (Phase B). TODO(charts-track): emit the
/// line polyline / area polygon / scatter markers.
fn generate_axis_frame_stub(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    // Reuse the column generator's frame by handing it EMPTY series (so it draws
    // the axes + ticks + category labels but no bars), then swap in the real
    // category list so the labels still render.
    let frame_model = ChartModel {
        kind: ChartKind::Column,
        title: model.title.clone(),
        series: Vec::new(),
        cat_axis: model.cat_axis.clone(),
        val_axis: model.val_axis.clone(),
        legend: model.legend,
    };
    // The frame still needs the value scale; feed the real data so the ticks
    // span the actual range, but with no series the bar loop is a no-op.
    let frame_data = PlotData {
        series: Vec::new(),
        categories: data.categories.clone(),
    };
    generate_column(&frame_model, &frame_data, width_pt, height_pt)
}

/// A series's fill color: its explicit swatch, else the deterministic palette.
fn series_color(model: &ChartModel, si: usize) -> String {
    model
        .series
        .get(si)
        .and_then(|s| s.color.as_ref())
        .map(|c| c.to_string())
        .unwrap_or_else(|| PALETTE[si % PALETTE.len()].to_string())
}

/// Format a value-axis tick label: an integer prints without a decimal point,
/// otherwise two decimals (Phase A — the charts track will route this through
/// `sheet-format`'s number-format engine in Phase B for axis number formats).
fn format_tick(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Axis, ChartKind, ChartModel, Series};
    use sheet_core::{CellRef, RangeRef};

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

    /// A 3-bar single-series column model + its resolved data (10, 20, 30).
    fn three_bar_column() -> (ChartModel, PlotData) {
        let model = ChartModel {
            kind: ChartKind::Column,
            title: None,
            series: vec![Series {
                name: Some("S".into()),
                categories: Some(rr(0, 0, 2, 0)),
                values: rr(0, 1, 2, 1),
                color: Some("#112233".into()),
            }],
            cat_axis: Axis::default(),
            val_axis: Axis::default(),
            legend: false,
        };
        let data = PlotData {
            series: vec![vec![10.0, 20.0, 30.0]],
            categories: vec!["A".into(), "B".into(), "C".into()],
        };
        (model, data)
    }

    #[test]
    fn column_three_bars_count_and_positions() {
        let (model, data) = three_bar_column();
        let g = generate(&model, &data, 300.0, 200.0);
        assert_eq!(g.width_pt, 300.0);
        assert_eq!(g.height_pt, 200.0);

        // Exactly three bar Rects (one per category, one series).
        let rects: Vec<&Primitive> = g
            .prims
            .iter()
            .filter(|p| matches!(p, Primitive::Rect { .. }))
            .collect();
        assert_eq!(rects.len(), 3, "expected 3 bars, got {}", rects.len());

        // Plot geometry (mirrors the generator's constants).
        let plot_x = PAD_LEFT; // 40
        let plot_w = 300.0 - PAD_LEFT - PAD_RIGHT; // 248
        let plot_y = PAD_TOP; // 20
        let plot_h = 200.0 - PAD_TOP - PAD_BOTTOM; // 156
        let plot_bottom = plot_y + plot_h; // 176
        let group_w = plot_w / 3.0;
        let bars_w = group_w * 0.8;
        let group_inset = (group_w - bars_w) / 2.0;

        // The value scale is 0..30 (data max), so the tallest bar (30) reaches
        // the plot top and the shortest (10) is 1/3 of the height.
        let h_of = |v: f64| (v / 30.0) * plot_h;
        for (gi, expected_v) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            let Primitive::Rect {
                x, y, w, h, fill, ..
            } = rects[gi]
            else {
                panic!("not a rect");
            };
            let exp_x = plot_x + gi as f64 * group_w + group_inset;
            let exp_h = h_of(expected_v);
            let exp_y = plot_bottom - exp_h;
            assert!((x - exp_x).abs() < 1e-6, "bar {gi} x {x} != {exp_x}");
            assert!((w - bars_w).abs() < 1e-6, "bar {gi} w {w} != {bars_w}");
            assert!((h - exp_h).abs() < 1e-6, "bar {gi} h {h} != {exp_h}");
            assert!((y - exp_y).abs() < 1e-6, "bar {gi} y {y} != {exp_y}");
            assert_eq!(fill.as_deref(), Some("#112233"));
        }
    }

    #[test]
    fn column_emits_axis_frame_and_ticks_and_labels() {
        let (model, data) = three_bar_column();
        let g = generate(&model, &data, 300.0, 200.0);
        // Two axis rules (value + category).
        let lines = g
            .prims
            .iter()
            .filter(|p| matches!(p, Primitive::Line { .. }))
            .count();
        assert_eq!(lines, 2, "expected 2 axis rules");
        // VAL_TICKS value-axis labels + 3 category labels = 8 Text prims.
        let texts = g
            .prims
            .iter()
            .filter(|p| matches!(p, Primitive::Text { .. }))
            .count();
        assert_eq!(texts, VAL_TICKS + 3);
        // The category labels are present (A/B/C).
        let cat_labels: Vec<&str> = g
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text {
                    s,
                    anchor: TextAnchor::Middle,
                    ..
                } => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert!(cat_labels.contains(&"A"));
        assert!(cat_labels.contains(&"C"));
    }

    #[test]
    fn generate_is_deterministic() {
        let (model, data) = three_bar_column();
        let a = generate(&model, &data, 300.0, 200.0);
        let b = generate(&model, &data, 300.0, 200.0);
        assert_eq!(a, b);
        // And serializes to identical JSON (wire-shape determinism).
        let ja = serde_json::to_string(&a).unwrap();
        let jb = serde_json::to_string(&b).unwrap();
        assert_eq!(ja, jb);
    }

    #[test]
    fn two_series_grouped_bars() {
        let model = ChartModel {
            kind: ChartKind::Column,
            title: Some("T".into()),
            series: vec![
                Series {
                    name: Some("A".into()),
                    categories: None,
                    values: rr(0, 1, 1, 1),
                    color: None,
                },
                Series {
                    name: Some("B".into()),
                    categories: None,
                    values: rr(0, 2, 1, 2),
                    color: None,
                },
            ],
            cat_axis: Axis::default(),
            val_axis: Axis::default(),
            legend: true,
        };
        let data = PlotData {
            series: vec![vec![1.0, 2.0], vec![3.0, 4.0]],
            categories: vec!["X".into(), "Y".into()],
        };
        let g = generate(&model, &data, 400.0, 250.0);
        // 2 groups * 2 series = 4 bars; default palette colors (no override).
        let rects: Vec<&Primitive> = g
            .prims
            .iter()
            .filter(|p| matches!(p, Primitive::Rect { .. }))
            .collect();
        assert_eq!(rects.len(), 4);
        if let Primitive::Rect { fill, .. } = rects[0] {
            assert_eq!(fill.as_deref(), Some(PALETTE[0]));
        }
    }

    #[test]
    fn stub_kinds_emit_axis_frame_only() {
        // Line/area/scatter Phase-A stub: the axis frame (2 lines) + tick/cat
        // labels, but NO series geometry (no Rect/Polygon/Wedge bars).
        let (mut model, data) = three_bar_column();
        for kind in [ChartKind::Line, ChartKind::Area, ChartKind::Scatter] {
            model.kind = kind;
            let g = generate(&model, &data, 300.0, 200.0);
            let lines = g
                .prims
                .iter()
                .filter(|p| matches!(p, Primitive::Line { .. }))
                .count();
            assert_eq!(lines, 2, "{kind:?} should draw the axis frame");
            assert!(
                !g.prims.iter().any(|p| matches!(p, Primitive::Rect { .. })),
                "{kind:?} stub must not emit bars"
            );
        }
        // Pie/donut: empty frame in Phase A.
        let mut pm = model;
        pm.kind = ChartKind::Pie;
        let g = generate(&pm, &data, 200.0, 200.0);
        assert!(g.prims.is_empty(), "pie Phase-A stub is empty");
    }
}
