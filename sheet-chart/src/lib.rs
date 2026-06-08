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

//! # sheet-chart — chart model + pure geometry IR (spec §8.4, T2/M2)
//!
//! Two halves, one generator:
//!
//! - [`model`] — the FROZEN chart MODEL (kind, series/axis bindings) the M2
//!   charts track populates from xlsx chart parts (`/charts/chartN.xml`).
//! - [`geometry`] — the PURE projection `model + resolved data -> vector
//!   primitives in chart-content pt space`. The charts track lowers these
//!   primitives through `paged.draw` (`insertPath`/`insertOval`/`insertLine` —
//!   a CORE SDK surface, NEVER another plugin, spec §2.1); the SAME geometry
//!   feeds the sheets-mode grid view. One generator, two projections.
//!
//! ## Phase status (M2 charts track — complete)
//!
//! The types are FROZEN here (the M2 chart-build track builds against them).
//! [`geometry::generate`] is real END-TO-END for EVERY curated kind
//! (post-plotters-swap, 2026-06-08): **Column**/**Bar** (axis frame + per-bar
//! Rects with linear value scaling + tick/category labels), **Line**/**Area**
//! (polyline series + area-fill polygons over the cartesian frame),
//! **Pie**/**Donut** (per-slice Wedges, donut = pie with a centre hole), and
//! **Scatter** (diamond markers in (x, y) value space). No kind returns a stub;
//! the cartesian kinds share plotters' axis frame + value scale (see the
//! [`geometry`] module docs).
//!
//! ## Dependency discipline (spec §4 rule 3)
//!
//! The geometry generator is PURE (`model -> vector geometry`); only the
//! charts track's LOWERING half (in the bundle/glue, not here) sees
//! `paged.draw` via the SDK. This crate depends only on `sheet-core`
//! (types) and `sheet-format` (axis number formats, Phase B); no SDK, no
//! `sheet-calc`, no inter-plugin contact.

mod backend;
pub mod geometry;
pub mod model;

pub use geometry::{generate, ChartGeometry, PlotData, Primitive, TextAnchor};
pub use model::{Axis, ChartKind, ChartModel, Series};
