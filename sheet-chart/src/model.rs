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

//! The chart MODEL (spec §8.4). The publishing-curated chart type set
//! (bar/column, line/area, pie/donut, scatter) plus its series and axis
//! bindings — the data the M2 charts track populates from xlsx chart parts
//! (`/charts/chartN.xml`). This is the FROZEN IR the geometry generator
//! ([`crate::geometry`]) projects; the M2 chart-build track builds against it.
//!
//! These types hold [`RangeRef`] series bindings (they are model-internal,
//! resolved against the workbook on recalc), so they deliberately do NOT derive
//! `serde::Serialize` — the SHAPE that crosses the wire is the pure geometry IR
//! in [`crate::geometry`] (numbers/strings only). Keeping `RangeRef` un-serded
//! also avoids touching the FROZEN `sheet-core` leaf types.

use compact_str::CompactString;
use sheet_core::RangeRef;

/// The publishing-curated chart kinds (spec §8.4 / D-4). `Bar` is horizontal
/// bars; `Column` is vertical bars; `Donut` is a `Pie` with a center hole.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChartKind {
    Bar,
    Column,
    Line,
    Area,
    Pie,
    Donut,
    Scatter,
}

/// One chart: its kind, optional title, the series, the two axes, and whether a
/// legend is drawn. Populated from an xlsx chart part by the charts track.
#[derive(Clone, Debug)]
pub struct ChartModel {
    pub kind: ChartKind,
    pub title: Option<CompactString>,
    pub series: Vec<Series>,
    pub cat_axis: Axis,
    pub val_axis: Axis,
    pub legend: bool,
}

/// One data series: an optional name, an optional category-label range, the
/// VALUES range (required), and an optional fixed color (`#RRGGBB` from a
/// document swatch — publication-coherent chart colors, spec §8.3).
#[derive(Clone, Debug)]
pub struct Series {
    pub name: Option<CompactString>,
    pub categories: Option<RangeRef>,
    pub values: RangeRef,
    /// `#RRGGBB` resolved from a document swatch; `None` => the generator picks
    /// a default from the deterministic palette ([`crate::geometry`]).
    pub color: Option<CompactString>,
}

/// An axis: an optional title and optional fixed min/max (a `None` bound is
/// auto-scaled from the data by the generator).
#[derive(Clone, Debug, Default)]
pub struct Axis {
    pub title: Option<CompactString>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::CellRef;

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

    #[test]
    fn model_constructs() {
        let m = ChartModel {
            kind: ChartKind::Column,
            title: Some("Q1".into()),
            series: vec![Series {
                name: Some("Revenue".into()),
                categories: Some(rr(0, 0, 2, 0)),
                values: rr(0, 1, 2, 1),
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
    }
}
