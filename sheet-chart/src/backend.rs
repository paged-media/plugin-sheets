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

//! [`GeomBackend`] — a custom [`plotters`] [`DrawingBackend`] that captures
//! plotters' draw calls as the FROZEN [`crate::geometry`] vector
//! [`Primitive`]s instead of rasterizing to pixels (spec §8.4). This is how
//! sheet-chart reuses plotters' mature axis/scale/series layout engine while
//! keeping the output a publication-coherent VECTOR primitive list (every
//! primitive carries its `#RRGGBB` so the TS translator can later reference
//! document swatches — no opaque bitmap is ever produced).
//!
//! ## Coordinate space
//!
//! plotters works in `i32` pixels over a `(w, h)` drawing area. We size that
//! area to `(width_pt, height_pt)` and treat **1 px = 1 pt**, so the captured
//! primitives are already in chart-content pt space (origin top-left, y DOWN —
//! the frame-content system, spec §8.5) matching the IR's `width_pt`/
//! `height_pt` convention. No scaling step is needed.
//!
//! ## Vector overrides (no rasterizer, no font backend)
//!
//! plotters' [`DrawingBackend`] default methods are pixel-based: `draw_line`/
//! `draw_rect`/`draw_path`/`fill_polygon`/`draw_circle` fall back to a
//! rasterizer, and `draw_text`/`estimate_text_size` call into the font layout
//! (which requires the `ttf`/`ab_glyph` feature we deliberately do NOT enable).
//! `GeomBackend` OVERRIDES every one of them so a draw call becomes a vector
//! [`Primitive`] and text is measured by a lightweight advance-width estimate.
//! `draw_pixel` is the only required method; it is a no-op (vector charts never
//! plot single pixels), and [`DrawingBackend::blit_bitmap`] is left at its
//! default — vector charts never blit, so that code path is never reached.
//!
//! ## Text metrics
//!
//! With no font feature there is no real glyph metrics source. [`GeomBackend`]
//! approximates: advance width ≈ `0.55 em × size_pt` per character, height ≈
//! `size_pt`. Approximate metrics are ACCEPTABLE for T2 (the prior hand-rolled
//! generator had the same limitation; the real host font facility is the S-13
//! BREAKAGE). plotters uses these only to size the gap before tick labels —
//! the generator sizes the label gutters itself via fixed pads.

use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::rc::Rc;

use plotters_backend::{
    text_anchor, BackendColor, BackendCoord, BackendStyle, BackendTextStyle, DrawingBackend,
    DrawingErrorKind,
};

use crate::geometry::{Primitive, TextAnchor};

/// The shared primitive sink. plotters' [`DrawingBackend::into_drawing_area`]
/// CONSUMES the backend, so the captured primitives live behind a shared
/// handle the caller keeps a clone of (interior mutability is the documented
/// plotters pattern for stateful custom backends).
pub(crate) type Sink = Rc<RefCell<Vec<Primitive>>>;

/// The backend's error type. Capturing into a `Vec` never fails, so this is
/// inhabited only to satisfy the trait bound; it is never returned.
#[derive(Debug)]
pub(crate) struct GeomError;

impl fmt::Display for GeomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sheet-chart GeomBackend error (unreachable)")
    }
}

impl Error for GeomError {}

/// Approximate per-character advance as a fraction of the em (the size in pt).
/// 0.55 is a typical sans-serif average; see the module text-metrics note.
const ADVANCE_EM: f64 = 0.55;

/// A custom [`plotters`] [`DrawingBackend`] capturing draw calls as vector
/// [`Primitive`]s in pt space. See the module docs for the full mapping.
pub(crate) struct GeomBackend {
    w: u32,
    h: u32,
    sink: Sink,
}

impl GeomBackend {
    /// A backend over a `(width_pt, height_pt)` area (1 px = 1 pt) capturing
    /// into `sink`. The caller keeps its own clone of `sink` to read the
    /// captured primitives back after plotters consumes the backend.
    pub(crate) fn new(width_pt: f64, height_pt: f64, sink: Sink) -> GeomBackend {
        GeomBackend {
            w: width_pt.max(0.0).round() as u32,
            h: height_pt.max(0.0).round() as u32,
            sink,
        }
    }
}

/// A plotters [`BackendColor`] → `#RRGGBB`. Alpha is DROPPED (the IR's color
/// strings are opaque `#RRGGBB`; translucency is the lowering's concern, e.g.
/// area fills, and is reintroduced there). A fully transparent color still
/// maps to its rgb hex — callers gate on alpha before drawing.
fn hex(c: BackendColor) -> String {
    let (r, g, b) = c.rgb;
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Map a plotters horizontal anchor to the IR's [`TextAnchor`]. plotters only
/// drives the horizontal anchor for axis/legend labels; vertical placement is
/// folded into the captured `y`.
fn anchor_of<TStyle: BackendTextStyle>(style: &TStyle) -> TextAnchor {
    match style.anchor().h_pos {
        text_anchor::HPos::Left => TextAnchor::Start,
        text_anchor::HPos::Center => TextAnchor::Middle,
        text_anchor::HPos::Right => TextAnchor::End,
    }
}

impl DrawingBackend for GeomBackend {
    type ErrorType = GeomError;

    fn get_size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn ensure_prepared(&mut self) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        Ok(())
    }

    fn present(&mut self) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        Ok(())
    }

    /// No-op: a vector chart never plots single pixels. plotters only reaches
    /// here through the rasterizer fallbacks, which we have all overridden.
    fn draw_pixel(
        &mut self,
        _point: BackendCoord,
        _color: BackendColor,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        Ok(())
    }

    /// A straight line → a two-point [`Primitive::Line`].
    fn draw_line<S: BackendStyle>(
        &mut self,
        from: BackendCoord,
        to: BackendCoord,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha == 0.0 {
            return Ok(());
        }
        self.sink.borrow_mut().push(Primitive::Line {
            pts: vec![(from.0 as f64, from.1 as f64), (to.0 as f64, to.1 as f64)],
            stroke: hex(style.color()),
            stroke_w: style.stroke_width() as f64,
        });
        Ok(())
    }

    /// A rectangle → a [`Primitive::Rect`]. `fill` decides whether the color
    /// lands on `fill` (filled bars/swatches) or `stroke` (outline frames).
    fn draw_rect<S: BackendStyle>(
        &mut self,
        upper_left: BackendCoord,
        bottom_right: BackendCoord,
        style: &S,
        fill: bool,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha == 0.0 {
            return Ok(());
        }
        let (x0, y0) = (upper_left.0 as f64, upper_left.1 as f64);
        let (x1, y1) = (bottom_right.0 as f64, bottom_right.1 as f64);
        let color = hex(style.color());
        let (fillc, strokec, stroke_w) = if fill {
            (Some(color), None, 0.0)
        } else {
            (None, Some(color), style.stroke_width() as f64)
        };
        self.sink.borrow_mut().push(Primitive::Rect {
            x: x0.min(x1),
            y: y0.min(y1),
            w: (x1 - x0).abs(),
            h: (y1 - y0).abs(),
            fill: fillc,
            stroke: strokec,
            stroke_w,
        });
        Ok(())
    }

    /// An open polyline (≥ 2 points) → a [`Primitive::Line`]. This is the path
    /// a `LineSeries` lowers to.
    fn draw_path<S: BackendStyle, I: IntoIterator<Item = BackendCoord>>(
        &mut self,
        path: I,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha == 0.0 {
            return Ok(());
        }
        let pts: Vec<(f64, f64)> = path.into_iter().map(|p| (p.0 as f64, p.1 as f64)).collect();
        if pts.len() >= 2 {
            self.sink.borrow_mut().push(Primitive::Line {
                pts,
                stroke: hex(style.color()),
                stroke_w: style.stroke_width() as f64,
            });
        }
        Ok(())
    }

    /// A filled polygon (≥ 2 points) → a filled [`Primitive::Polygon`]. This is
    /// the path an `AreaSeries` band lowers to.
    fn fill_polygon<S: BackendStyle, I: IntoIterator<Item = BackendCoord>>(
        &mut self,
        vert: I,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha == 0.0 {
            return Ok(());
        }
        let pts: Vec<(f64, f64)> = vert.into_iter().map(|p| (p.0 as f64, p.1 as f64)).collect();
        if pts.len() >= 2 {
            self.sink.borrow_mut().push(Primitive::Polygon {
                pts,
                fill: Some(hex(style.color())),
                stroke: None,
                stroke_w: 0.0,
            });
        }
        Ok(())
    }

    /// A circle → a closed regular polygon approximation (a [`Primitive::Polygon`]
    /// of `CIRCLE_STEPS` points). Scatter markers are drawn as small circles by
    /// plotters' point series; an octagon is a publishing-clean dot. (Pie/donut
    /// wedges are emitted directly as [`Primitive::Wedge`] by the generator, NOT
    /// through this path — see the generator's pie note.)
    fn draw_circle<S: BackendStyle>(
        &mut self,
        center: BackendCoord,
        radius: u32,
        style: &S,
        fill: bool,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha == 0.0 {
            return Ok(());
        }
        const CIRCLE_STEPS: usize = 8;
        let (cx, cy, r) = (center.0 as f64, center.1 as f64, radius as f64);
        let pts: Vec<(f64, f64)> = (0..CIRCLE_STEPS)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / CIRCLE_STEPS as f64;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        let color = hex(style.color());
        self.sink.borrow_mut().push(Primitive::Polygon {
            pts,
            fill: if fill { Some(color.clone()) } else { None },
            stroke: if fill { None } else { Some(color) },
            stroke_w: if fill {
                0.0
            } else {
                style.stroke_width() as f64
            },
        });
        Ok(())
    }

    /// A text label → a [`Primitive::Text`]. plotters has already resolved the
    /// anchor point `pos`; we keep its horizontal anchor and the size in pt.
    fn draw_text<TStyle: BackendTextStyle>(
        &mut self,
        text: &str,
        style: &TStyle,
        pos: BackendCoord,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha == 0.0 || text.is_empty() {
            return Ok(());
        }
        self.sink.borrow_mut().push(Primitive::Text {
            x: pos.0 as f64,
            y: pos.1 as f64,
            s: text.to_string(),
            size_pt: style.size(),
            anchor: anchor_of(style),
        });
        Ok(())
    }

    /// Approximate text metrics (no font backend): advance width ≈
    /// `ADVANCE_EM × size × chars`, height ≈ `size`. See the module note. This
    /// override is what lets plotters lay out labels with the `ttf` feature OFF
    /// — otherwise the default would call into the (absent) font rasterizer.
    fn estimate_text_size<TStyle: BackendTextStyle>(
        &self,
        text: &str,
        style: &TStyle,
    ) -> Result<(u32, u32), DrawingErrorKind<Self::ErrorType>> {
        let size = style.size();
        let w = (text.chars().count() as f64 * size * ADVANCE_EM)
            .ceil()
            .max(0.0) as u32;
        let h = size.ceil().max(0.0) as u32;
        Ok((w, h))
    }
}
