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
//! ## Kinds (M2 — the publishing-curated set, spec §8.4)
//!
//! Phase A proved the IR end-to-end for **Column**; M2 completes the rest, so
//! [`generate`] is now REAL for every curated kind:
//!
//! - **Column** (vertical bars) / **Bar** (horizontal bars) — a cartesian axis
//!   frame + one [`Primitive::Rect`] per (series, category) value with linear
//!   scaling. Column scales on Y (bars grow up); Bar transposes — the value
//!   axis runs along X (bars grow right), the category axis down the left.
//! - **Line** — a [`Primitive::Line`] polyline per series over the cartesian
//!   frame; a marker per point is omitted (publishing-clean lines).
//! - **Area** — the Line polyline PLUS a closed [`Primitive::Polygon`] dropped
//!   to the value-axis baseline (a translucent-by-fill series band).
//! - **Pie** / **Donut** — one [`Primitive::Wedge`] per category of the FIRST
//!   series, angles proportional to each value's share of the total, clockwise
//!   from 12 o'clock. Donut is a pie with a center-hole ratio (the lowering /
//!   grid view paints the hole; the wedge geometry is identical).
//! - **Scatter** — one diamond [`Primitive::Polygon`] marker per point in
//!   (x, y) value space: series 0 supplies the X values, series 1 the Y values
//!   (the ECMA `c:scatterChart` xVal/yVal pairing); a single series plots
//!   index-vs-value. No category axis.
//!
//! Every cartesian kind shares one axis frame + value scale; the **legend**
//! (when `model.legend`) is a swatch + label row appended across kinds. The
//! palette is deterministic; a series' explicit `color` swatch overrides it
//! (publication-coherent chart colors, spec §8.3).

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

/// Series line / marker stroke width (line, area outline, scatter markers).
const SERIES_STROKE_W: f64 = 1.5;
/// Scatter marker half-extent (pt) — the diamond's radius from its center.
const MARKER_R: f64 = 3.0;
/// The donut center-hole radius as a fraction of the outer radius (spec §8.4:
/// a donut is a pie with a hole — the wedge geometry is identical, the hole is
/// painted by the lowering / grid view from this ratio).
const DONUT_HOLE_RATIO: f64 = 0.5;
/// Legend swatch box side (pt) and the gap to its label.
const LEGEND_SWATCH: f64 = 7.0;
const LEGEND_GAP: f64 = 3.0;
/// Legend row pitch (pt) and the inset from the content box's right edge.
const LEGEND_ROW_H: f64 = 11.0;

/// Project a [`ChartModel`] + resolved [`PlotData`] into a [`ChartGeometry`]
/// (spec §8.4). PURE + deterministic — same inputs => byte-identical IR. Every
/// curated kind is REAL (M2); the dispatch routes to the per-kind generator.
pub fn generate(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    match model.kind {
        // Vertical bars (the proven Phase-A kind).
        ChartKind::Column => generate_column(model, data, width_pt, height_pt),
        // Horizontal bars — the transpose of Column (value axis along X).
        ChartKind::Bar => generate_bar(model, data, width_pt, height_pt),
        // Polyline series (no fill) / polyline + baseline polygon (filled).
        ChartKind::Line => generate_line_area(model, data, width_pt, height_pt, false),
        ChartKind::Area => generate_line_area(model, data, width_pt, height_pt, true),
        // Proportional wedges (donut = pie with a center-hole ratio).
        ChartKind::Pie => generate_pie(model, data, width_pt, height_pt, 0.0),
        ChartKind::Donut => generate_pie(model, data, width_pt, height_pt, DONUT_HOLE_RATIO),
        // Diamond markers in (x, y) value space (no category axis).
        ChartKind::Scatter => generate_scatter(model, data, width_pt, height_pt),
    }
}

/// The plot area in content-box pt space (origin top-left, y down): the inset
/// rectangle the series geometry is drawn into. Shared by every cartesian kind.
#[derive(Copy, Clone)]
struct PlotArea {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl PlotArea {
    fn of(width_pt: f64, height_pt: f64) -> PlotArea {
        PlotArea {
            x: PAD_LEFT,
            y: PAD_TOP,
            w: (width_pt - PAD_LEFT - PAD_RIGHT).max(0.0),
            h: (height_pt - PAD_TOP - PAD_BOTTOM).max(0.0),
        }
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn nonempty(&self) -> bool {
        self.w > 0.0 && self.h > 0.0
    }
}

/// A linear value scale `vmin..vmax` over a pixel span, derived from the data
/// extent and the optional axis min/max overrides. Shared so every kind maps
/// values identically (the axis-override contract is decided once).
#[derive(Copy, Clone)]
struct ValueScale {
    vmin: f64,
    span: f64,
}

impl ValueScale {
    /// Build a scale over the data's value extent. `include_negative` lets the
    /// floor drop below 0 (line/area/scatter can go negative); bar/column keep
    /// the 0 floor (Excel's default for category value axes). An explicit axis
    /// min/max override always wins.
    fn of(data: &PlotData, axis: &crate::model::Axis, include_negative: bool) -> ValueScale {
        let mut data_max = f64::NEG_INFINITY;
        let mut data_min = f64::INFINITY;
        for v in data.series.iter().flat_map(|s| s.iter().copied()) {
            data_max = data_max.max(v);
            data_min = data_min.min(v);
        }
        if !data_max.is_finite() {
            data_max = 1.0;
        }
        if !data_min.is_finite() {
            data_min = 0.0;
        }
        let floor = if include_negative {
            data_min.min(0.0)
        } else {
            0.0
        };
        let vmin = axis.min.unwrap_or(floor);
        let vmax_raw = axis.max.unwrap_or(data_max);
        let vmax = if vmax_raw <= vmin {
            vmin + 1.0
        } else {
            vmax_raw
        };
        ValueScale {
            vmin,
            span: vmax - vmin,
        }
    }
    /// The fraction `0..1` of a value within the scale (clamped span-safe).
    fn frac(&self, v: f64) -> f64 {
        (v - self.vmin) / self.span
    }
}

/// Push the chart title (centered above the plot) when present.
fn push_title(prims: &mut Vec<Primitive>, model: &ChartModel, width_pt: f64) {
    if let Some(t) = &model.title {
        prims.push(Primitive::Text {
            x: width_pt / 2.0,
            y: PAD_TOP / 2.0,
            s: t.to_string(),
            size_pt: TICK_SIZE_PT + 2.0,
            anchor: TextAnchor::Middle,
        });
    }
}

/// Push the two cartesian axis rules (left value axis + bottom category axis).
fn push_axis_frame(prims: &mut Vec<Primitive>, plot: &PlotArea) {
    prims.push(Primitive::Line {
        pts: vec![(plot.x, plot.y), (plot.x, plot.bottom())],
        stroke: AXIS_STROKE.to_string(),
        stroke_w: AXIS_STROKE_W,
    });
    prims.push(Primitive::Line {
        pts: vec![(plot.x, plot.bottom()), (plot.right(), plot.bottom())],
        stroke: AXIS_STROKE.to_string(),
        stroke_w: AXIS_STROKE_W,
    });
}

/// Push the legend (a swatch + series-name row per series) at the plot's
/// top-right, when `model.legend`. One row per series with a name; index-only
/// series ("Series N") still get a row so the palette is documented.
fn push_legend(prims: &mut Vec<Primitive>, model: &ChartModel, plot: &PlotArea) {
    if !model.legend || model.series.is_empty() {
        return;
    }
    let mut y = plot.y + 2.0;
    let swatch_x = (plot.right() - 60.0).max(plot.x);
    for (si, series) in model.series.iter().enumerate() {
        prims.push(Primitive::Rect {
            x: swatch_x,
            y,
            w: LEGEND_SWATCH,
            h: LEGEND_SWATCH,
            fill: Some(series_color(model, si)),
            stroke: None,
            stroke_w: 0.0,
        });
        let label = series
            .name
            .as_ref()
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("Series {}", si + 1));
        prims.push(Primitive::Text {
            x: swatch_x + LEGEND_SWATCH + LEGEND_GAP,
            y: y + LEGEND_SWATCH,
            s: label,
            size_pt: TICK_SIZE_PT,
            anchor: TextAnchor::Start,
        });
        y += LEGEND_ROW_H;
    }
}

/// The group count for a category chart: the category label count, falling
/// back to the longest series (a chart with values but no labels still bars).
fn group_count(data: &PlotData) -> usize {
    data.categories
        .len()
        .max(data.series.iter().map(|s| s.len()).max().unwrap_or(0))
}

/// The COLUMN generator (vertical bars): a plot-area frame, value-axis tick
/// labels on a linear 0..max scale, category labels under each group, and one
/// [`Primitive::Rect`] per (series, category) bar with grouped placement.
fn generate_column(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    let mut prims: Vec<Primitive> = Vec::new();
    let plot = PlotArea::of(width_pt, height_pt);
    let scale = ValueScale::of(data, &model.val_axis, false);

    push_title(&mut prims, model, width_pt);
    push_axis_frame(&mut prims, &plot);

    // Value-axis tick labels (vmin at the bottom, vmax at the top).
    for i in 0..VAL_TICKS {
        let frac = i as f64 / (VAL_TICKS - 1) as f64;
        let v = scale.vmin + frac * scale.span;
        let y = plot.bottom() - frac * plot.h;
        prims.push(Primitive::Text {
            x: plot.x - 4.0,
            y,
            s: format_tick(v),
            size_pt: TICK_SIZE_PT,
            anchor: TextAnchor::End,
        });
    }

    let n_groups = group_count(data);
    if n_groups > 0 && plot.nonempty() {
        let n_series = data.series.len().max(1);
        let group_w = plot.w / n_groups as f64;
        let bars_w = group_w * 0.8; // 80% bars, 20% gutter
        let bar_w = bars_w / n_series as f64;
        let group_inset = (group_w - bars_w) / 2.0;
        let base_y = plot.bottom() - scale.frac(scale.vmin).clamp(0.0, 1.0) * plot.h;

        for gi in 0..n_groups {
            let group_left = plot.x + gi as f64 * group_w;
            if let Some(label) = data.categories.get(gi) {
                prims.push(Primitive::Text {
                    x: group_left + group_w / 2.0,
                    y: plot.bottom() + 10.0,
                    s: label.clone(),
                    size_pt: TICK_SIZE_PT,
                    anchor: TextAnchor::Middle,
                });
            }
            for (si, series) in data.series.iter().enumerate() {
                let v = series
                    .get(gi)
                    .copied()
                    .unwrap_or(scale.vmin)
                    .max(scale.vmin);
                let top = plot.bottom() - scale.frac(v).clamp(0.0, 1.0) * plot.h;
                let h = (base_y - top).max(0.0);
                prims.push(Primitive::Rect {
                    x: group_left + group_inset + si as f64 * bar_w,
                    y: top,
                    w: bar_w,
                    h,
                    fill: Some(series_color(model, si)),
                    stroke: None,
                    stroke_w: 0.0,
                });
            }
        }
    }

    push_legend(&mut prims, model, &plot);
    ChartGeometry {
        width_pt,
        height_pt,
        prims,
    }
}

/// The BAR generator (horizontal bars): the transpose of [`generate_column`].
/// The value axis runs along X (bars grow rightward from the left edge); the
/// category axis runs down the left, one group per category top-to-bottom.
fn generate_bar(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    let mut prims: Vec<Primitive> = Vec::new();
    let plot = PlotArea::of(width_pt, height_pt);
    let scale = ValueScale::of(data, &model.val_axis, false);

    push_title(&mut prims, model, width_pt);
    push_axis_frame(&mut prims, &plot);

    // Value-axis tick labels run along the BOTTOM (the X axis).
    for i in 0..VAL_TICKS {
        let frac = i as f64 / (VAL_TICKS - 1) as f64;
        let v = scale.vmin + frac * scale.span;
        let x = plot.x + frac * plot.w;
        prims.push(Primitive::Text {
            x,
            y: plot.bottom() + 10.0,
            s: format_tick(v),
            size_pt: TICK_SIZE_PT,
            anchor: TextAnchor::Middle,
        });
    }

    let n_groups = group_count(data);
    if n_groups > 0 && plot.nonempty() {
        let n_series = data.series.len().max(1);
        let group_h = plot.h / n_groups as f64;
        let bars_h = group_h * 0.8;
        let bar_h = bars_h / n_series as f64;
        let group_inset = (group_h - bars_h) / 2.0;
        let base_x = plot.x + scale.frac(scale.vmin).clamp(0.0, 1.0) * plot.w;

        for gi in 0..n_groups {
            let group_top = plot.y + gi as f64 * group_h;
            // Category label to the LEFT of the group center.
            if let Some(label) = data.categories.get(gi) {
                prims.push(Primitive::Text {
                    x: plot.x - 4.0,
                    y: group_top + group_h / 2.0,
                    s: label.clone(),
                    size_pt: TICK_SIZE_PT,
                    anchor: TextAnchor::End,
                });
            }
            for (si, series) in data.series.iter().enumerate() {
                let v = series
                    .get(gi)
                    .copied()
                    .unwrap_or(scale.vmin)
                    .max(scale.vmin);
                let right = plot.x + scale.frac(v).clamp(0.0, 1.0) * plot.w;
                let w = (right - base_x).max(0.0);
                prims.push(Primitive::Rect {
                    x: base_x,
                    y: group_top + group_inset + si as f64 * bar_h,
                    w,
                    h: bar_h,
                    fill: Some(series_color(model, si)),
                    stroke: None,
                    stroke_w: 0.0,
                });
            }
        }
    }

    push_legend(&mut prims, model, &plot);
    ChartGeometry {
        width_pt,
        height_pt,
        prims,
    }
}

/// The LINE / AREA generator: a [`Primitive::Line`] polyline per series across
/// the category positions. When `fill`, ALSO emit a closed
/// [`Primitive::Polygon`] dropped to the value-axis baseline (an area band).
/// Points sit at category-slot CENTERS so a line and a column chart of the same
/// data align column-to-vertex.
fn generate_line_area(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
    fill: bool,
) -> ChartGeometry {
    let mut prims: Vec<Primitive> = Vec::new();
    let plot = PlotArea::of(width_pt, height_pt);
    let scale = ValueScale::of(data, &model.val_axis, true);

    push_title(&mut prims, model, width_pt);
    push_axis_frame(&mut prims, &plot);

    for i in 0..VAL_TICKS {
        let frac = i as f64 / (VAL_TICKS - 1) as f64;
        let v = scale.vmin + frac * scale.span;
        let y = plot.bottom() - frac * plot.h;
        prims.push(Primitive::Text {
            x: plot.x - 4.0,
            y,
            s: format_tick(v),
            size_pt: TICK_SIZE_PT,
            anchor: TextAnchor::End,
        });
    }

    let n_groups = group_count(data);
    if n_groups > 0 && plot.nonempty() {
        let slot_w = plot.w / n_groups as f64;
        let x_of = |gi: usize| plot.x + (gi as f64 + 0.5) * slot_w;
        let y_of = |v: f64| plot.bottom() - scale.frac(v).clamp(0.0, 1.0) * plot.h;
        let baseline_y = plot.bottom() - scale.frac(scale.vmin).clamp(0.0, 1.0) * plot.h;

        for gi in 0..n_groups {
            if let Some(label) = data.categories.get(gi) {
                prims.push(Primitive::Text {
                    x: x_of(gi),
                    y: plot.bottom() + 10.0,
                    s: label.clone(),
                    size_pt: TICK_SIZE_PT,
                    anchor: TextAnchor::Middle,
                });
            }
        }

        for (si, series) in data.series.iter().enumerate() {
            let pts: Vec<(f64, f64)> = (0..n_groups)
                .filter_map(|gi| series.get(gi).map(|&v| (x_of(gi), y_of(v))))
                .collect();
            if pts.len() < 2 {
                continue;
            }
            let color = series_color(model, si);
            if fill {
                // Close the band: across the top points, then down to the
                // baseline at the last x, back along the baseline to the first.
                let mut poly = pts.clone();
                let (last_x, _) = *pts.last().expect("len >= 2");
                let (first_x, _) = pts[0];
                poly.push((last_x, baseline_y));
                poly.push((first_x, baseline_y));
                prims.push(Primitive::Polygon {
                    pts: poly,
                    fill: Some(color.clone()),
                    stroke: None,
                    stroke_w: 0.0,
                });
            }
            prims.push(Primitive::Line {
                pts,
                stroke: color,
                stroke_w: SERIES_STROKE_W,
            });
        }
    }

    push_legend(&mut prims, model, &plot);
    ChartGeometry {
        width_pt,
        height_pt,
        prims,
    }
}

/// The center-hole fill of a donut (the page background it "punches" through
/// the ring). White is the publishing-neutral page color; the lowering paints
/// it as an opaque disc over the wedge centers.
const DONUT_HOLE_FILL: &str = "#FFFFFF";

/// The PIE / DONUT generator: one [`Primitive::Wedge`] per category of the
/// FIRST series, each spanning an angle proportional to its share of the
/// non-negative total, clockwise from 12 o'clock (the charts-track convention,
/// see [`Primitive::Wedge`]). Negative / zero values contribute nothing (we
/// take each value's MAGNITUDE for the angle so a stray negative does not
/// invert the layout — the publishing reading of a pie).
///
/// DONUT (`hole_ratio > 0`): the [`Primitive::Wedge`] IR carries a SINGLE
/// radius (frozen), so the ring is expressed self-describingly — full-radius
/// wedges PLUS a final center disc ([`Primitive::Wedge`] 0..360 at
/// `r * hole_ratio`) filled with the page color. Both projections (the
/// paged.draw lowering AND the grid view) render the ring identically from the
/// primitive list alone, with no model knowledge (the generator stays the
/// single source of truth).
fn generate_pie(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
    hole_ratio: f64,
) -> ChartGeometry {
    let mut prims: Vec<Primitive> = Vec::new();
    push_title(&mut prims, model, width_pt);

    let series = data.series.first();
    let total: f64 = series
        .map(|s| s.iter().map(|v| v.abs()).sum())
        .unwrap_or(0.0);
    if let (Some(series), true) = (series, total > 0.0) {
        // The pie box: the largest centered square under the title.
        let avail_top = PAD_TOP;
        let avail_h = (height_pt - avail_top - PAD_BOTTOM).max(0.0);
        let avail_w = (width_pt - PAD_RIGHT - PAD_RIGHT).max(0.0);
        let r = (avail_w.min(avail_h) / 2.0).max(0.0);
        let cx = width_pt / 2.0;
        let cy = avail_top + avail_h / 2.0;

        // Wedge i occupies its share of 360°, clockwise from 12 o'clock.
        let mut acc = 0.0_f64;
        for (i, &v) in series.iter().enumerate() {
            let share = v.abs() / total;
            if share <= 0.0 {
                continue;
            }
            let start_deg = acc * 360.0;
            acc += share;
            let end_deg = acc * 360.0;
            prims.push(Primitive::Wedge {
                cx,
                cy,
                r,
                start_deg,
                end_deg,
                fill: Some(series_color_idx(model, i)),
                stroke: Some(AXIS_STROKE.to_string()),
            });
        }

        // Donut: punch the center hole with a page-colored full disc.
        if hole_ratio > 0.0 {
            prims.push(Primitive::Wedge {
                cx,
                cy,
                r: r * hole_ratio,
                start_deg: 0.0,
                end_deg: 360.0,
                fill: Some(DONUT_HOLE_FILL.to_string()),
                stroke: None,
            });
        }
    }

    push_legend_pie(&mut prims, model, data, width_pt, height_pt);
    ChartGeometry {
        width_pt,
        height_pt,
        prims,
    }
}

/// The SCATTER generator: a diamond [`Primitive::Polygon`] marker per point in
/// (x, y) value space (no category axis). Series 0 supplies X, series 1 Y (the
/// ECMA `c:scatterChart` xVal/yVal pairing); a lone series plots index-vs-value.
fn generate_scatter(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    let mut prims: Vec<Primitive> = Vec::new();
    let plot = PlotArea::of(width_pt, height_pt);

    push_title(&mut prims, model, width_pt);
    push_axis_frame(&mut prims, &plot);

    // Pair (x, y): series 0 = X, series 1 = Y. A single series => index vs value.
    let (xs, ys): (Vec<f64>, Vec<f64>) = match (data.series.first(), data.series.get(1)) {
        (Some(x), Some(y)) => {
            let n = x.len().min(y.len());
            (x[..n].to_vec(), y[..n].to_vec())
        }
        (Some(x), None) => ((0..x.len()).map(|i| i as f64).collect(), x.clone()),
        _ => (Vec::new(), Vec::new()),
    };

    if !xs.is_empty() && plot.nonempty() {
        let x_scale = axis_extent(&xs);
        let y_scale = ValueScale {
            vmin: axis_extent(&ys).0,
            span: (axis_extent(&ys).1 - axis_extent(&ys).0).max(1.0),
        };
        let x_span = (x_scale.1 - x_scale.0).max(1.0);
        let to_px = |x: f64| plot.x + ((x - x_scale.0) / x_span) * plot.w;
        let to_py = |y: f64| plot.bottom() - y_scale.frac(y).clamp(0.0, 1.0) * plot.h;
        let color = series_color(model, 0);

        // Value-axis (Y) tick labels.
        for i in 0..VAL_TICKS {
            let frac = i as f64 / (VAL_TICKS - 1) as f64;
            let v = y_scale.vmin + frac * y_scale.span;
            let y = plot.bottom() - frac * plot.h;
            prims.push(Primitive::Text {
                x: plot.x - 4.0,
                y,
                s: format_tick(v),
                size_pt: TICK_SIZE_PT,
                anchor: TextAnchor::End,
            });
        }

        for (&x, &y) in xs.iter().zip(ys.iter()) {
            let (px, py) = (to_px(x), to_py(y));
            prims.push(Primitive::Polygon {
                pts: vec![
                    (px, py - MARKER_R),
                    (px + MARKER_R, py),
                    (px, py + MARKER_R),
                    (px - MARKER_R, py),
                ],
                fill: Some(color.clone()),
                stroke: None,
                stroke_w: SERIES_STROKE_W,
            });
        }
    }

    push_legend(&mut prims, model, &plot);
    ChartGeometry {
        width_pt,
        height_pt,
        prims,
    }
}

/// The min/max extent of a value list (for the scatter X axis). Empty => 0..1.
fn axis_extent(vs: &[f64]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &v in vs {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if !lo.is_finite() || !hi.is_finite() {
        (0.0, 1.0)
    } else {
        (lo, hi)
    }
}

/// A pie's per-WEDGE legend: one swatch + category label per slice (the pie has
/// no series legend — each wedge is a category). Falls back to the series
/// legend when there are no category labels.
fn push_legend_pie(
    prims: &mut Vec<Primitive>,
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) {
    if !model.legend {
        return;
    }
    let plot = PlotArea::of(width_pt, height_pt);
    if data.categories.is_empty() {
        push_legend(prims, model, &plot);
        return;
    }
    let mut y = plot.y + 2.0;
    let swatch_x = (plot.right() - 60.0).max(plot.x);
    for (i, label) in data.categories.iter().enumerate() {
        prims.push(Primitive::Rect {
            x: swatch_x,
            y,
            w: LEGEND_SWATCH,
            h: LEGEND_SWATCH,
            fill: Some(series_color_idx(model, i)),
            stroke: None,
            stroke_w: 0.0,
        });
        prims.push(Primitive::Text {
            x: swatch_x + LEGEND_SWATCH + LEGEND_GAP,
            y: y + LEGEND_SWATCH,
            s: label.clone(),
            size_pt: TICK_SIZE_PT,
            anchor: TextAnchor::Start,
        });
        y += LEGEND_ROW_H;
    }
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

/// A per-SLICE fill color for pie/donut (one color per category of the single
/// series). Slice 0 honors the first series' explicit swatch (so a fixed
/// document color still leads); the rest cycle the deterministic palette.
fn series_color_idx(model: &ChartModel, i: usize) -> String {
    if i == 0 {
        if let Some(c) = model.series.first().and_then(|s| s.color.as_ref()) {
            return c.to_string();
        }
    }
    PALETTE[i % PALETTE.len()].to_string()
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
        // Filter OUT the legend swatch rects (LEGEND_SWATCH-wide) so we count
        // bars only — the legend is on (legend: true).
        let bars: Vec<&Primitive> = g
            .prims
            .iter()
            .filter(|p| matches!(p, Primitive::Rect { w, .. } if (*w - LEGEND_SWATCH).abs() > 1e-6))
            .collect();
        assert_eq!(bars.len(), 4);
        if let Primitive::Rect { fill, .. } = bars[0] {
            assert_eq!(fill.as_deref(), Some(PALETTE[0]));
        }
    }

    fn count<F: Fn(&Primitive) -> bool>(g: &ChartGeometry, f: F) -> usize {
        g.prims.iter().filter(|p| f(p)).count()
    }
    fn is_rect(p: &Primitive) -> bool {
        matches!(p, Primitive::Rect { .. })
    }
    fn is_line(p: &Primitive) -> bool {
        matches!(p, Primitive::Line { .. })
    }
    fn is_poly(p: &Primitive) -> bool {
        matches!(p, Primitive::Polygon { .. })
    }
    fn is_wedge(p: &Primitive) -> bool {
        matches!(p, Primitive::Wedge { .. })
    }

    #[test]
    fn geometry_bar_column_horizontal_transposes() {
        // Bar (horizontal): one Rect per (series, category); the bars grow
        // along X from the left edge, so every bar's x == the value-axis floor.
        let (mut model, data) = three_bar_column();
        model.kind = ChartKind::Bar;
        let g = generate(&model, &data, 300.0, 200.0);
        let rects: Vec<&Primitive> = g.prims.iter().filter(|p| is_rect(p)).collect();
        assert_eq!(rects.len(), 3, "3 horizontal bars");
        let plot_x = PAD_LEFT;
        let plot_w = 300.0 - PAD_LEFT - PAD_RIGHT;
        // Value scale 0..30; widths proportional to 10/20/30.
        for (gi, expected_v) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            let Primitive::Rect { x, w, .. } = rects[gi] else {
                panic!("not a rect");
            };
            assert!((x - plot_x).abs() < 1e-6, "bar {gi} starts at the floor");
            let exp_w = (expected_v / 30.0) * plot_w;
            assert!((w - exp_w).abs() < 1e-6, "bar {gi} w {w} != {exp_w}");
        }
        // Two axis rules, no wedges/polys.
        assert_eq!(count(&g, is_line), 2);
        assert_eq!(count(&g, is_wedge), 0);
    }

    #[test]
    fn geometry_line_area_polyline_and_fill() {
        let (mut model, data) = three_bar_column();
        // LINE: one polyline (2 axis rules + 1 series line = 3 lines), no poly.
        model.kind = ChartKind::Line;
        let g = generate(&model, &data, 300.0, 200.0);
        assert_eq!(count(&g, is_line), 3, "2 axis rules + 1 series polyline");
        assert_eq!(count(&g, is_poly), 0, "line has no fill polygon");
        // The series polyline has one vertex per category (3).
        let series_line = g
            .prims
            .iter()
            .find_map(|p| match p {
                Primitive::Line { pts, stroke_w, .. } if *stroke_w == SERIES_STROKE_W => Some(pts),
                _ => None,
            })
            .expect("a series polyline");
        assert_eq!(series_line.len(), 3);

        // AREA: the polyline PLUS a closed baseline polygon.
        model.kind = ChartKind::Area;
        let g = generate(&model, &data, 300.0, 200.0);
        assert_eq!(count(&g, is_line), 3, "2 axis rules + 1 series polyline");
        assert_eq!(count(&g, is_poly), 1, "area fills one band polygon");
        // The band closes the 3 vertices down to the baseline (3 + 2 = 5 pts).
        let band = g
            .prims
            .iter()
            .find_map(|p| match p {
                Primitive::Polygon { pts, .. } => Some(pts),
                _ => None,
            })
            .expect("a band polygon");
        assert_eq!(band.len(), 5);
    }

    #[test]
    fn geometry_pie_donut_proportional_wedges() {
        // A single-series pie of [10, 20, 30] => shares 1/6, 2/6, 3/6 of 360°.
        let model = ChartModel {
            kind: ChartKind::Pie,
            title: None,
            series: vec![Series {
                name: Some("S".into()),
                categories: Some(rr(0, 0, 2, 0)),
                values: rr(0, 1, 2, 1),
                color: None,
            }],
            cat_axis: Axis::default(),
            val_axis: Axis::default(),
            legend: false,
        };
        let data = PlotData {
            series: vec![vec![10.0, 20.0, 30.0]],
            categories: vec!["A".into(), "B".into(), "C".into()],
        };
        let g = generate(&model, &data, 200.0, 200.0);
        let wedges: Vec<(f64, f64)> = g
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Wedge {
                    start_deg, end_deg, ..
                } => Some((*start_deg, *end_deg)),
                _ => None,
            })
            .collect();
        assert_eq!(wedges.len(), 3, "one wedge per slice (pie, no hole)");
        // Contiguous, full sweep, proportional spans.
        assert!((wedges[0].0 - 0.0).abs() < 1e-6);
        assert!((wedges[0].1 - 60.0).abs() < 1e-6, "10/60 share = 60°");
        assert!(
            (wedges[1].1 - 180.0).abs() < 1e-6,
            "cumulative 30/60 = 180°"
        );
        assert!((wedges[2].1 - 360.0).abs() < 1e-6, "full sweep");

        // DONUT: same wedges PLUS a center-hole disc (a 0..360 wedge).
        let mut dm = model;
        dm.kind = ChartKind::Donut;
        let gd = generate(&dm, &data, 200.0, 200.0);
        assert_eq!(count(&gd, is_wedge), 4, "3 slices + 1 hole disc");
        let hole = gd
            .prims
            .iter()
            .rev()
            .find_map(|p| match p {
                Primitive::Wedge {
                    r,
                    start_deg,
                    end_deg,
                    fill,
                    ..
                } if (*end_deg - *start_deg - 360.0).abs() < 1e-6 => Some((*r, fill.clone())),
                _ => None,
            })
            .expect("the donut hole disc");
        assert!(hole.0 > 0.0, "hole has a positive inner radius");
        assert_eq!(hole.1.as_deref(), Some(DONUT_HOLE_FILL));
    }

    #[test]
    fn geometry_scatter_xy_markers() {
        // Two series: X = [1,2,3], Y = [4,5,6] => 3 diamond markers.
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
        // 3 diamond markers (4-point polygons), 2 axis rules, no bars.
        let markers: Vec<&Primitive> = g.prims.iter().filter(|p| is_poly(p)).collect();
        assert_eq!(markers.len(), 3, "one marker per (x,y) point");
        if let Primitive::Polygon { pts, .. } = markers[0] {
            assert_eq!(pts.len(), 4, "diamond marker has 4 vertices");
        }
        assert_eq!(count(&g, is_line), 2, "the cartesian axis frame");
        assert_eq!(count(&g, is_rect), 0);
    }

    #[test]
    fn legend_emits_one_row_per_series() {
        let (mut model, data) = three_bar_column();
        model.legend = true;
        model.series[0].name = Some("Revenue".into());
        let g = generate(&model, &data, 300.0, 200.0);
        // The legend swatch is a Rect at the plot's top-right; its label is a
        // Start-anchored text equal to the series name.
        let labels: Vec<&str> = g
            .prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text {
                    s,
                    anchor: TextAnchor::Start,
                    ..
                } => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"Revenue"), "legend shows the series name");
    }

    #[test]
    fn every_kind_is_deterministic_and_serializes() {
        let (mut model, data) = three_bar_column();
        model.legend = true;
        for kind in [
            ChartKind::Column,
            ChartKind::Bar,
            ChartKind::Line,
            ChartKind::Area,
            ChartKind::Pie,
            ChartKind::Donut,
            ChartKind::Scatter,
        ] {
            model.kind = kind;
            let a = generate(&model, &data, 320.0, 240.0);
            let b = generate(&model, &data, 320.0, 240.0);
            assert_eq!(a, b, "{kind:?} is deterministic");
            let ja = serde_json::to_string(&a).unwrap();
            assert_eq!(ja, serde_json::to_string(&b).unwrap());
            assert!(!a.prims.is_empty(), "{kind:?} emits geometry");
        }
    }

    #[test]
    fn empty_data_never_panics() {
        // A model with no resolved data lowers to a non-panicking (possibly
        // frame-only) geometry for every kind (recalc resilience).
        let (mut model, _) = three_bar_column();
        let empty = PlotData::default();
        for kind in [
            ChartKind::Column,
            ChartKind::Bar,
            ChartKind::Line,
            ChartKind::Area,
            ChartKind::Pie,
            ChartKind::Donut,
            ChartKind::Scatter,
        ] {
            model.kind = kind;
            let g = generate(&model, &empty, 200.0, 150.0);
            assert_eq!(g.width_pt, 200.0);
            assert_eq!(g.height_pt, 150.0);
        }
    }
}
