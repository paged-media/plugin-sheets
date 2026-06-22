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
//! ## The layout engine ([`plotters`]) + the custom backend
//!
//! The axis/scale/coordinate layout is delegated to the pure-Rust [`plotters`]
//! crate (orchestrator decision 2026-06-08; plotters is MIT/Apache and adds NO
//! native/bitmap/font backend — `default-features = false`). Rather than
//! rasterize, we drive plotters with a CUSTOM [`crate::backend::GeomBackend`]
//! ([`plotters_backend::DrawingBackend`]) that captures every draw call as a
//! [`Primitive`] in pt space (1 px = 1 pt). The cartesian kinds build a
//! `ChartBuilder` over the backend; plotters' coordinate translator
//! (`map_coordinate`, the auto-scaled value range, the segmented category axis)
//! is the single source of truth for value→pt and category→pt placement, and
//! the series/axis/labels are emitted as plotters ELEMENTS (`Rectangle`,
//! `PathElement`, `Polygon`, `Text`) that flow through the backend. This reuses
//! plotters' mature scale/axis layout while keeping the output a deterministic,
//! swatch-coherent VECTOR primitive list (each [`Primitive`] keeps its
//! `#RRGGBB`; no opaque bitmap is ever produced).
//!
//! Text metrics are approximated in the backend (no font feature — see
//! [`crate::backend`]); approximate advance widths are acceptable for T2 (the
//! real host font facility is the S-13 BREAKAGE).
//!
//! ## Kinds (M2 — the publishing-curated set, spec §8.4)
//!
//! - **Column** (vertical bars) / **Bar** (horizontal bars) — a cartesian axis
//!   frame + one [`Primitive::Rect`] per (series, category) value, placed by
//!   plotters' segmented category axis and linear value scale. Column scales on
//!   Y (bars grow up); Bar transposes (the value axis runs along X).
//! - **Line** — a [`Primitive::Line`] polyline per series over plotters'
//!   cartesian frame; markers omitted (publishing-clean lines).
//! - **Area** — the Line polyline PLUS a closed [`Primitive::Polygon`] dropped
//!   to the value-axis baseline.
//! - **Pie** / **Donut** — one [`Primitive::Wedge`] per category of the FIRST
//!   series, angles proportional to each value's share of the total, clockwise
//!   from 12 o'clock. plotters has no clean pie path for a custom backend, so
//!   the wedge geometry is emitted DIRECTLY (documented below); donut adds a
//!   center-hole ratio.
//! - **Scatter** — one diamond [`Primitive::Polygon`] marker per point in
//!   (x, y) value space over a plotters cartesian frame; series 0 supplies X,
//!   series 1 Y (the ECMA `c:scatterChart` pairing); a single series plots
//!   index-vs-value. No category axis.
//!
//! Every cartesian kind shares plotters' axis frame + value scale; the
//! **legend** (when `model.legend`) is a swatch + label row appended across
//! kinds. The palette is deterministic; a series' explicit `color` swatch
//! overrides it (publication-coherent chart colors, spec §8.3).

use std::cell::RefCell;
use std::rc::Rc;

use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

use crate::backend::{GeomBackend, Sink};
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
/// bottom for the category labels, top for the title, right gutter. These feed
/// plotters' label-area sizing so the plotting rectangle matches the prior
/// generator's content box (downstream placement is unchanged).
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
/// curated kind is REAL; the dispatch routes to the per-kind generator.
pub fn generate(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    match model.kind {
        // Vertical bars.
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

// ── plotters glue ─────────────────────────────────────────────────────────

/// A fresh primitive sink + the matching [`GeomBackend`]. The caller keeps the
/// returned `Sink` clone to read primitives back after plotters consumes the
/// backend (`into_drawing_area`).
fn new_backend(width_pt: f64, height_pt: f64) -> (Sink, GeomBackend) {
    let sink: Sink = Rc::new(RefCell::new(Vec::new()));
    let backend = GeomBackend::new(width_pt, height_pt, Rc::clone(&sink));
    (sink, backend)
}

/// Drain the shared sink into the owned `Vec` once drawing is done (the backend
/// has been dropped by then, so the only remaining handle is `sink`).
fn drain(sink: Sink) -> Vec<Primitive> {
    Rc::try_unwrap(sink)
        .map(|c| c.into_inner())
        .unwrap_or_else(|rc| rc.borrow().clone())
}

/// A `#RRGGBB` string → a plotters [`RGBColor`] (hex parse; falls back to the
/// axis grey on a malformed string — never panics).
fn rgb(hex: &str) -> RGBColor {
    let parse = |s: &str| u8::from_str_radix(s, 16).ok();
    let h = hex.strip_prefix('#').unwrap_or(hex);
    if h.len() == 6 {
        if let (Some(r), Some(g), Some(b)) = (parse(&h[0..2]), parse(&h[2..4]), parse(&h[4..6])) {
            return RGBColor(r, g, b);
        }
    }
    RGBColor(0x88, 0x88, 0x88)
}

/// The pt-space text style for a chart label at `size_pt` with horizontal
/// `anchor`, bound to `area` so plotters can measure it (via the backend's
/// approximate metrics). Vertical anchor is Bottom — plotters reports the
/// anchor at the text baseline, matching the IR's `(x, y)` convention.
fn label_style<'a, DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    size_pt: f64,
    anchor: TextAnchor,
) -> TextStyle<'a> {
    let h = match anchor {
        TextAnchor::Start => HPos::Left,
        TextAnchor::Middle => HPos::Center,
        TextAnchor::End => HPos::Right,
    };
    // Text color is not part of the IR's `Primitive::Text` (the lowering /
    // grid view paint labels in the document's text color), so we leave the
    // plotters default (black) — the backend ignores text color anyway.
    ("sans-serif", size_pt)
        .into_text_style(area)
        .pos(Pos::new(h, VPos::Bottom))
}

// ── shared scale / plot-area helpers (the scale DECISIONS, fed to plotters) ──

/// The plot-area inset in content-box pt space — the rectangle plotters draws
/// the series into. Mirrors the prior generator's content box so downstream
/// placement is unchanged; fed to plotters as the label-area sizes.
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

/// A linear value scale `vmin..vmax`, derived from the data extent and the
/// optional axis min/max overrides — the scale DECISION the prior generator
/// made, now handed to plotters as the cartesian value range so plotters owns
/// the value→pt mapping and tick selection.
#[derive(Copy, Clone)]
struct ValueScale {
    vmin: f64,
    vmax: f64,
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
        ValueScale { vmin, vmax }
    }
    fn span(&self) -> f64 {
        self.vmax - self.vmin
    }
    /// The plotters range for this scale.
    fn range(&self) -> std::ops::Range<f64> {
        self.vmin..self.vmax
    }
}

/// The group count for a category chart: the category label count, falling
/// back to the longest series (a chart with values but no labels still bars).
fn group_count(data: &PlotData) -> usize {
    data.categories
        .len()
        .max(data.series.iter().map(|s| s.len()).max().unwrap_or(0))
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
/// series). Slice 0 honors the first series' explicit swatch; the rest cycle
/// the deterministic palette.
fn series_color_idx(model: &ChartModel, i: usize) -> String {
    if i == 0 {
        if let Some(c) = model.series.first().and_then(|s| s.color.as_ref()) {
            return c.to_string();
        }
    }
    PALETTE[i % PALETTE.len()].to_string()
}

/// Format a value-axis tick label: an integer prints without a decimal point,
/// otherwise two decimals (the charts track will route this through
/// `sheet-format`'s number-format engine for axis number formats, Phase B).
fn format_tick(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

// ── element-drawing helpers (every emission flows through the backend) ───────

/// Push a [`Primitive::Text`] by drawing a plotters `Text` element in SCREEN
/// (pt) coordinates onto `area`. The element routes through the backend's
/// `draw_text`, capturing the label with its anchor + size.
fn draw_label<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    x: f64,
    y: f64,
    s: &str,
    size_pt: f64,
    anchor: TextAnchor,
) {
    let style = label_style(area, size_pt, anchor);
    // plotters' screen `DrawingArea` maps `(i32, i32)` 1:1 to backend pixels.
    let _ = area.draw(&Text::new(
        s.to_string(),
        (x.round() as i32, y.round() as i32),
        &style,
    ));
}

/// Push the two cartesian axis rules (left value axis + bottom category axis)
/// as two screen-space `PathElement`s — exactly two [`Primitive::Line`]s.
fn draw_axis_frame<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    plot: &PlotArea,
) {
    let stroke = rgb(AXIS_STROKE).stroke_width(AXIS_STROKE_W.round().max(1.0) as u32);
    let p = |x: f64, y: f64| (x.round() as i32, y.round() as i32);
    let _ = area.draw(&PathElement::new(
        vec![p(plot.x, plot.y), p(plot.x, plot.bottom())],
        stroke,
    ));
    let _ = area.draw(&PathElement::new(
        vec![p(plot.x, plot.bottom()), p(plot.right(), plot.bottom())],
        stroke,
    ));
}

/// Push the chart title (centered above the plot) when present.
fn draw_title<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    model: &ChartModel,
    width_pt: f64,
) {
    if let Some(t) = &model.title {
        draw_label(
            area,
            width_pt / 2.0,
            PAD_TOP / 2.0,
            t,
            TICK_SIZE_PT + 2.0,
            TextAnchor::Middle,
        );
    }
}

/// Push the legend (a swatch + series-name row per series) at the plot's
/// top-right, when `model.legend`. Each swatch is a filled `Rectangle`; the
/// label a `Text` — both flow through the backend.
fn draw_legend<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    model: &ChartModel,
    plot: &PlotArea,
) {
    if !model.legend || model.series.is_empty() {
        return;
    }
    let mut y = plot.y + 2.0;
    let swatch_x = (plot.right() - 60.0).max(plot.x);
    for (si, series) in model.series.iter().enumerate() {
        draw_swatch(area, swatch_x, y, &series_color(model, si));
        let label = series
            .name
            .as_ref()
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("Series {}", si + 1));
        draw_label(
            area,
            swatch_x + LEGEND_SWATCH + LEGEND_GAP,
            y + LEGEND_SWATCH,
            &label,
            TICK_SIZE_PT,
            TextAnchor::Start,
        );
        y += LEGEND_ROW_H;
    }
}

/// A pie's per-WEDGE legend: one swatch + category label per slice (each wedge
/// is a category). Falls back to the series legend when there are no labels.
fn draw_legend_pie<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    model: &ChartModel,
    data: &PlotData,
    plot: &PlotArea,
) {
    if !model.legend {
        return;
    }
    if data.categories.is_empty() {
        draw_legend(area, model, plot);
        return;
    }
    let mut y = plot.y + 2.0;
    let swatch_x = (plot.right() - 60.0).max(plot.x);
    for (i, label) in data.categories.iter().enumerate() {
        draw_swatch(area, swatch_x, y, &series_color_idx(model, i));
        draw_label(
            area,
            swatch_x + LEGEND_SWATCH + LEGEND_GAP,
            y + LEGEND_SWATCH,
            label,
            TICK_SIZE_PT,
            TextAnchor::Start,
        );
        y += LEGEND_ROW_H;
    }
}

/// A filled legend swatch `Rectangle` (LEGEND_SWATCH pt square) in screen pt.
fn draw_swatch<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    x: f64,
    y: f64,
    color: &str,
) {
    let p = |x: f64, y: f64| (x.round() as i32, y.round() as i32);
    let _ = area.draw(&Rectangle::new(
        [p(x, y), p(x + LEGEND_SWATCH, y + LEGEND_SWATCH)],
        rgb(color).filled(),
    ));
}

// ── the cartesian kinds (plotters coordinate engine) ────────────────────────

/// The COLUMN generator (vertical bars). plotters builds the cartesian
/// coordinate system (segmented category axis × linear value scale); we read
/// the per-bar pixel rect via `map_coordinate` and emit one filled
/// [`Primitive::Rect`] per (series, category), grouped within each category
/// slot. The axis frame + tick/category labels are drawn directly so the
/// emitted primitive set stays exactly the IR contract.
fn generate_column(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    let plot = PlotArea::of(width_pt, height_pt);
    let scale = ValueScale::of(data, &model.val_axis, false);
    let n_groups = group_count(data);
    let (sink, backend) = new_backend(width_pt, height_pt);
    {
        let root = backend.into_drawing_area();
        let n = n_groups.max(1) as f64;
        // plotters owns the value→pt map (0..n category space, value range).
        let built = ChartBuilder::on(&root)
            .x_label_area_size(PAD_BOTTOM)
            .y_label_area_size(PAD_LEFT)
            .margin_top(PAD_TOP)
            .margin_right(PAD_RIGHT)
            .build_cartesian_2d(0f64..n, scale.range());
        if let Ok(chart) = built {
            let pa = chart.plotting_area();
            draw_title(&root, model, width_pt);
            draw_axis_frame(&root, &plot);
            draw_value_ticks(&root, pa, &plot, &scale);

            if n_groups > 0 && plot.nonempty() {
                let n_series = data.series.len().max(1);
                let group_w = plot.w / n_groups as f64;
                let bars_w = group_w * 0.8;
                let bar_w = bars_w / n_series as f64;
                let group_inset = (group_w - bars_w) / 2.0;
                let base = pa.map_coordinate(&(0.0, scale.vmin));

                for gi in 0..n_groups {
                    let group_left = plot.x + gi as f64 * group_w;
                    if let Some(label) = data.categories.get(gi) {
                        draw_label(
                            &root,
                            group_left + group_w / 2.0,
                            plot.bottom() + 10.0,
                            label,
                            TICK_SIZE_PT,
                            TextAnchor::Middle,
                        );
                    }
                    for (si, series) in data.series.iter().enumerate() {
                        let v = series
                            .get(gi)
                            .copied()
                            .unwrap_or(scale.vmin)
                            .max(scale.vmin);
                        let top = pa.map_coordinate(&(0.0, v)).1 as f64;
                        let h = (base.1 as f64 - top).max(0.0);
                        let x = group_left + group_inset + si as f64 * bar_w;
                        emit_bar(&sink, x, top, bar_w, h, &series_color(model, si));
                    }
                }
            }
            draw_legend(&root, model, &plot);
            let _ = root.present();
        }
    }
    ChartGeometry {
        width_pt,
        height_pt,
        prims: drain(sink),
    }
}

/// The BAR generator (horizontal bars): the transpose of [`generate_column`].
fn generate_bar(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    let plot = PlotArea::of(width_pt, height_pt);
    let scale = ValueScale::of(data, &model.val_axis, false);
    let n_groups = group_count(data);
    let (sink, backend) = new_backend(width_pt, height_pt);
    {
        let root = backend.into_drawing_area();
        let n = n_groups.max(1) as f64;
        // Value axis along X; category axis (0..n) down Y.
        let built = ChartBuilder::on(&root)
            .x_label_area_size(PAD_BOTTOM)
            .y_label_area_size(PAD_LEFT)
            .margin_top(PAD_TOP)
            .margin_right(PAD_RIGHT)
            .build_cartesian_2d(scale.range(), 0f64..n);
        if let Ok(chart) = built {
            let pa = chart.plotting_area();
            draw_title(&root, model, width_pt);
            draw_axis_frame(&root, &plot);
            draw_value_ticks_x(&root, pa, &plot, &scale);

            if n_groups > 0 && plot.nonempty() {
                let n_series = data.series.len().max(1);
                let group_h = plot.h / n_groups as f64;
                let bars_h = group_h * 0.8;
                let bar_h = bars_h / n_series as f64;
                let group_inset = (group_h - bars_h) / 2.0;
                let base_x = pa.map_coordinate(&(scale.vmin, 0.0)).0 as f64;

                for gi in 0..n_groups {
                    let group_top = plot.y + gi as f64 * group_h;
                    if let Some(label) = data.categories.get(gi) {
                        draw_label(
                            &root,
                            plot.x - 4.0,
                            group_top + group_h / 2.0,
                            label,
                            TICK_SIZE_PT,
                            TextAnchor::End,
                        );
                    }
                    for (si, series) in data.series.iter().enumerate() {
                        let v = series
                            .get(gi)
                            .copied()
                            .unwrap_or(scale.vmin)
                            .max(scale.vmin);
                        let right = pa.map_coordinate(&(v, 0.0)).0 as f64;
                        let w = (right - base_x).max(0.0);
                        let y = group_top + group_inset + si as f64 * bar_h;
                        emit_bar(&sink, base_x, y, w, bar_h, &series_color(model, si));
                    }
                }
            }
            draw_legend(&root, model, &plot);
            let _ = root.present();
        }
    }
    ChartGeometry {
        width_pt,
        height_pt,
        prims: drain(sink),
    }
}

/// The LINE / AREA generator: a [`Primitive::Line`] polyline per series across
/// the category slot centers (mapped by plotters). When `fill`, ALSO emit a
/// closed [`Primitive::Polygon`] dropped to the value-axis baseline.
fn generate_line_area(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
    fill: bool,
) -> ChartGeometry {
    let plot = PlotArea::of(width_pt, height_pt);
    let scale = ValueScale::of(data, &model.val_axis, true);
    let n_groups = group_count(data);
    let (sink, backend) = new_backend(width_pt, height_pt);
    {
        let root = backend.into_drawing_area();
        let n = n_groups.max(1) as f64;
        let built = ChartBuilder::on(&root)
            .x_label_area_size(PAD_BOTTOM)
            .y_label_area_size(PAD_LEFT)
            .margin_top(PAD_TOP)
            .margin_right(PAD_RIGHT)
            .build_cartesian_2d(0f64..n, scale.range());
        if let Ok(chart) = built {
            let pa = chart.plotting_area();
            draw_title(&root, model, width_pt);
            draw_axis_frame(&root, &plot);
            draw_value_ticks(&root, pa, &plot, &scale);

            if n_groups > 0 && plot.nonempty() {
                // Category slot centers in plotters' 0..n category space.
                let cx = |gi: usize| gi as f64 + 0.5;
                let pt = |gi: usize, v: f64| {
                    let p = pa.map_coordinate(&(cx(gi), v));
                    (p.0 as f64, p.1 as f64)
                };
                let baseline_y = pa.map_coordinate(&(0.0, scale.vmin)).1 as f64;

                for gi in 0..n_groups {
                    if let Some(label) = data.categories.get(gi) {
                        let p = pa.map_coordinate(&(cx(gi), scale.vmin));
                        draw_label(
                            &root,
                            p.0 as f64,
                            plot.bottom() + 10.0,
                            label,
                            TICK_SIZE_PT,
                            TextAnchor::Middle,
                        );
                    }
                }

                for (si, series) in data.series.iter().enumerate() {
                    let pts: Vec<(f64, f64)> = (0..n_groups)
                        .filter_map(|gi| series.get(gi).map(|&v| pt(gi, v)))
                        .collect();
                    if pts.len() < 2 {
                        continue;
                    }
                    let color = series_color(model, si);
                    if fill {
                        let mut poly = pts.clone();
                        let (last_x, _) = *pts.last().expect("len >= 2");
                        let (first_x, _) = pts[0];
                        poly.push((last_x, baseline_y));
                        poly.push((first_x, baseline_y));
                        emit_polygon(&sink, poly, Some(color.clone()), None, 0.0);
                    }
                    emit_line(&sink, pts, color, SERIES_STROKE_W);
                }
            }
            draw_legend(&root, model, &plot);
            let _ = root.present();
        }
    }
    ChartGeometry {
        width_pt,
        height_pt,
        prims: drain(sink),
    }
}

/// The SCATTER generator: a diamond [`Primitive::Polygon`] marker per point in
/// (x, y) value space. Series 0 = X, series 1 = Y; a single series => index vs
/// value. plotters' two-axis cartesian frame maps both coordinates.
fn generate_scatter(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
) -> ChartGeometry {
    let plot = PlotArea::of(width_pt, height_pt);
    let (xs, ys): (Vec<f64>, Vec<f64>) = match (data.series.first(), data.series.get(1)) {
        (Some(x), Some(y)) => {
            let n = x.len().min(y.len());
            (x[..n].to_vec(), y[..n].to_vec())
        }
        (Some(x), None) => ((0..x.len()).map(|i| i as f64).collect(), x.clone()),
        _ => (Vec::new(), Vec::new()),
    };

    let (x_lo, x_hi) = axis_extent(&xs);
    let x_span = (x_hi - x_lo).max(1.0);
    let y_ext = axis_extent(&ys);
    let y_scale = ValueScale {
        vmin: y_ext.0,
        vmax: y_ext.0 + (y_ext.1 - y_ext.0).max(1.0),
    };

    let (sink, backend) = new_backend(width_pt, height_pt);
    {
        let root = backend.into_drawing_area();
        let built = ChartBuilder::on(&root)
            .x_label_area_size(PAD_BOTTOM)
            .y_label_area_size(PAD_LEFT)
            .margin_top(PAD_TOP)
            .margin_right(PAD_RIGHT)
            .build_cartesian_2d(x_lo..(x_lo + x_span), y_scale.range());
        if let Ok(chart) = built {
            let pa = chart.plotting_area();
            draw_title(&root, model, width_pt);
            draw_axis_frame(&root, &plot);

            if !xs.is_empty() && plot.nonempty() {
                draw_value_ticks(&root, pa, &plot, &y_scale);
                let color = series_color(model, 0);
                for (&x, &y) in xs.iter().zip(ys.iter()) {
                    let p = pa.map_coordinate(&(x, y));
                    let (px, py) = (p.0 as f64, p.1 as f64);
                    emit_polygon(
                        &sink,
                        vec![
                            (px, py - MARKER_R),
                            (px + MARKER_R, py),
                            (px, py + MARKER_R),
                            (px - MARKER_R, py),
                        ],
                        Some(color.clone()),
                        None,
                        SERIES_STROKE_W,
                    );
                }
            }
            draw_legend(&root, model, &plot);
            let _ = root.present();
        }
    }
    ChartGeometry {
        width_pt,
        height_pt,
        prims: drain(sink),
    }
}

/// The center-hole fill of a donut (the page background it "punches" through
/// the ring). White is the publishing-neutral page color.
const DONUT_HOLE_FILL: &str = "#FFFFFF";

/// The PIE / DONUT generator. plotters has no clean pie path for a CUSTOM
/// backend (its pie helper assumes the bitmap/element drawing model), so the
/// wedge geometry is emitted DIRECTLY as [`Primitive::Wedge`]s — proportional
/// angles, clockwise from 12 o'clock — which is both simpler and exactly the
/// frozen IR. The title + legend still flow through the backend (so the text
/// metrics path is shared). DONUT adds a page-colored center disc.
fn generate_pie(
    model: &ChartModel,
    data: &PlotData,
    width_pt: f64,
    height_pt: f64,
    hole_ratio: f64,
) -> ChartGeometry {
    let plot = PlotArea::of(width_pt, height_pt);
    let (sink, backend) = new_backend(width_pt, height_pt);
    {
        let root = backend.into_drawing_area();
        draw_title(&root, model, width_pt);

        let series = data.series.first();
        let total: f64 = series
            .map(|s| s.iter().map(|v| v.abs()).sum())
            .unwrap_or(0.0);
        if let (Some(series), true) = (series, total > 0.0) {
            let avail_top = PAD_TOP;
            let avail_h = (height_pt - avail_top - PAD_BOTTOM).max(0.0);
            let avail_w = (width_pt - PAD_RIGHT - PAD_RIGHT).max(0.0);
            let r = (avail_w.min(avail_h) / 2.0).max(0.0);
            let cx = width_pt / 2.0;
            let cy = avail_top + avail_h / 2.0;

            let mut acc = 0.0_f64;
            for (i, &v) in series.iter().enumerate() {
                let share = v.abs() / total;
                if share <= 0.0 {
                    continue;
                }
                let start_deg = acc * 360.0;
                acc += share;
                let end_deg = acc * 360.0;
                sink.borrow_mut().push(Primitive::Wedge {
                    cx,
                    cy,
                    r,
                    start_deg,
                    end_deg,
                    fill: Some(series_color_idx(model, i)),
                    stroke: Some(AXIS_STROKE.to_string()),
                });
            }

            if hole_ratio > 0.0 {
                sink.borrow_mut().push(Primitive::Wedge {
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

        draw_legend_pie(&root, model, data, &plot);
        let _ = root.present();
    }
    ChartGeometry {
        width_pt,
        height_pt,
        prims: drain(sink),
    }
}

// ── value-axis tick labels (positioned by plotters' value scale) ────────────

/// Push `VAL_TICKS` value-axis tick labels along the LEFT (Y) axis, positioned
/// by plotters' value→pt map (`map_coordinate`). End-anchored at `plot.x - 4`.
fn draw_value_ticks<DB: DrawingBackend, CT>(
    root: &DrawingArea<DB, plotters::coord::Shift>,
    pa: &DrawingArea<DB, CT>,
    plot: &PlotArea,
    scale: &ValueScale,
) where
    CT: plotters::coord::CoordTranslate<From = (f64, f64)>,
{
    for i in 0..VAL_TICKS {
        let frac = i as f64 / (VAL_TICKS - 1) as f64;
        let v = scale.vmin + frac * scale.span();
        let y = pa.map_coordinate(&(0.0, v)).1 as f64;
        draw_label(
            root,
            plot.x - 4.0,
            y,
            &format_tick(v),
            TICK_SIZE_PT,
            TextAnchor::End,
        );
    }
}

/// Push `VAL_TICKS` value-axis tick labels along the BOTTOM (X) axis (the Bar
/// orientation), positioned by plotters' value→pt map. Middle-anchored.
fn draw_value_ticks_x<DB: DrawingBackend, CT>(
    root: &DrawingArea<DB, plotters::coord::Shift>,
    pa: &DrawingArea<DB, CT>,
    plot: &PlotArea,
    scale: &ValueScale,
) where
    CT: plotters::coord::CoordTranslate<From = (f64, f64)>,
{
    for i in 0..VAL_TICKS {
        let frac = i as f64 / (VAL_TICKS - 1) as f64;
        let v = scale.vmin + frac * scale.span();
        let x = pa.map_coordinate(&(v, 0.0)).0 as f64;
        draw_label(
            root,
            x,
            plot.bottom() + 10.0,
            &format_tick(v),
            TICK_SIZE_PT,
            TextAnchor::Middle,
        );
    }
}

// ── direct primitive emission (deterministic, exact-count series geometry) ──

/// Emit one filled bar [`Primitive::Rect`] straight into the sink. The bar
/// positions come from plotters' coordinate map; emitting directly keeps the
/// grouped-multi-series layout + exact bar count under the generator's control.
fn emit_bar(sink: &Sink, x: f64, y: f64, w: f64, h: f64, color: &str) {
    sink.borrow_mut().push(Primitive::Rect {
        x,
        y,
        w,
        h,
        fill: Some(color.to_string()),
        stroke: None,
        stroke_w: 0.0,
    });
}

/// Emit a series polyline [`Primitive::Line`].
fn emit_line(sink: &Sink, pts: Vec<(f64, f64)>, stroke: String, stroke_w: f64) {
    sink.borrow_mut().push(Primitive::Line {
        pts,
        stroke,
        stroke_w,
    });
}

/// Emit a [`Primitive::Polygon`] (area band / scatter marker).
fn emit_polygon(
    sink: &Sink,
    pts: Vec<(f64, f64)>,
    fill: Option<String>,
    stroke: Option<String>,
    stroke_w: f64,
) {
    sink.borrow_mut().push(Primitive::Polygon {
        pts,
        fill,
        stroke,
        stroke_w,
    });
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

        // Plot geometry (mirrors the generator's constants; plotters maps the
        // value scale, so bar y/h are checked against the plotters-mapped top).
        let plot_x = PAD_LEFT; // 40
        let plot_w = 300.0 - PAD_LEFT - PAD_RIGHT; // 248
        let plot_y = PAD_TOP; // 20
        let plot_h = 200.0 - PAD_TOP - PAD_BOTTOM; // 156
        let plot_bottom = plot_y + plot_h; // 176
        let group_w = plot_w / 3.0;
        let bars_w = group_w * 0.8;
        let group_inset = (group_w - bars_w) / 2.0;

        // The value scale is 0..30 (data max); the tallest bar (30) reaches the
        // plot top and the shortest (10) is ~1/3 of the height. plotters rounds
        // to integer pixels, so allow a ±1pt tolerance on the mapped heights.
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
            assert!((h - exp_h).abs() < 1.5, "bar {gi} h {h} != ~{exp_h}");
            assert!((y - exp_y).abs() < 1.5, "bar {gi} y {y} != ~{exp_y}");
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
        // Value scale 0..30; widths proportional to 10/20/30 (±1.5pt for the
        // plotters integer-pixel rounding).
        for (gi, expected_v) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            let Primitive::Rect { x, w, .. } = rects[gi] else {
                panic!("not a rect");
            };
            assert!((x - plot_x).abs() < 1.5, "bar {gi} starts at the floor");
            let exp_w = (expected_v / 30.0) * plot_w;
            assert!((w - exp_w).abs() < 1.5, "bar {gi} w {w} != ~{exp_w}");
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
