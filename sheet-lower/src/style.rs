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

//! Style resolution — model + visual styles → the IR-v2 [`LoweredStyle`]
//! table (spec §8.3, "the most important property of the whole plugin").
//!
//! ## What this resolves (the cross-surface-parity contract)
//!
//! A cell carries a `sheet_core::StyleId` (an index into the workbook's
//! interned `StyleTable`). The *visual* attributes of that style —
//! bold/italic, point size, font name, fill/text colours (`#RRGGBB`),
//! per-edge border presence — live in a SIDE table parsed by `sheet-xlsx`
//! (`VisualStyles`, kept out of the frozen `CellStyle` whose
//! `font`/`fill`/`border` stay opaque `u32` slots). This module turns a
//! cell's `StyleId` into a deduplicated `Vec<LoweredStyle>` plus a per-cell
//! `style_key`, the IR v2 the host translator consumes.
//!
//! The SAME [`StyleResolver`] is used by BOTH render surfaces — the page
//! lowering ([`crate::lower_range_styled`]) and the grid scene
//! (`sheet_grid::grid_scene_styled`) — so a cell resolves to an IDENTICAL
//! `LoweredStyle` on each. That is the `sheet.style.cross-surface-parity`
//! row: "the two surfaces may differ in pipeline, never in styling"
//! (spec §8.3).
//!
//! ## The decoupling (dependency rule)
//!
//! `sheet-lower` depends only on `sheet-core` + `sheet-format` — it CANNOT
//! see `sheet_xlsx::VisualStyles`. So the resolver takes the visual
//! attributes through the [`VisualStyleSource`] trait: a `StyleId` →
//! [`VisualAttrs`] lookup. `sheet-xlsx` (or any caller) implements it; a
//! blanket impl for `Fn(StyleId) -> Option<VisualAttrs>` lets a caller pass
//! a closure when it does not want to name a type. The frozen `lower_range`
//! delegates to the styled path with the zero-source [`NoStyles`], so its
//! output is unchanged (a single default key-0 entry, `style_key` 0
//! everywhere) — backward-compatible by construction.

use crate::LoweredStyle;
use sheet_core::StyleId;
use std::collections::HashMap;

/// The flat visual attributes of one resolved style — the dialect-neutral
/// mirror of `sheet_xlsx::VisualStyle` (and of [`LoweredStyle`] minus the
/// wire `key`). A `None`/`false` field means "no explicit attribute" (the
/// document default wins). This is the value a [`VisualStyleSource`] hands
/// the resolver; keeping it a `sheet-lower`-local type is what frees the
/// crate from a `sheet-xlsx` dependency.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VisualAttrs {
    pub bold: bool,
    pub italic: bool,
    pub font_size_pt: Option<f64>,
    pub font_name: Option<String>,
    pub fill_rgb: Option<String>,
    pub text_rgb: Option<String>,
    pub border_top: bool,
    pub border_right: bool,
    pub border_bottom: bool,
    pub border_left: bool,
}

impl VisualAttrs {
    /// True if this carries NO visual override (every field default) — the
    /// resolver folds these onto the default style key 0 (no new entry).
    pub fn is_default(&self) -> bool {
        *self == VisualAttrs::default()
    }

    /// Promote to a wire [`LoweredStyle`] under the given `key`.
    fn into_lowered(self, key: u32) -> LoweredStyle {
        LoweredStyle {
            key,
            bold: self.bold,
            italic: self.italic,
            font_size_pt: self.font_size_pt,
            font_name: self.font_name,
            fill_rgb: self.fill_rgb,
            text_rgb: self.text_rgb,
            border_top: self.border_top,
            border_right: self.border_right,
            border_bottom: self.border_bottom,
            border_left: self.border_left,
        }
    }
}

/// A `StyleId` → [`VisualAttrs`] lookup. The resolver is generic over this
/// so `sheet-lower` never names `sheet_xlsx::VisualStyles` (dependency
/// rule). Returning `None` (or a default `VisualAttrs`) means "this style
/// has no visual styling" — it folds onto key 0.
pub trait VisualStyleSource {
    /// The visual attributes for a resolved `StyleId`, or `None` (default).
    fn visual(&self, id: StyleId) -> Option<VisualAttrs>;
}

/// Any `Fn(StyleId) -> Option<VisualAttrs>` is a source — a caller that does
/// not want to name a type can pass a closure.
impl<F> VisualStyleSource for F
where
    F: Fn(StyleId) -> Option<VisualAttrs>,
{
    fn visual(&self, id: StyleId) -> Option<VisualAttrs> {
        self(id)
    }
}

/// The zero source: every style is default. The frozen `lower_range`
/// resolves against this so its IR is unchanged (key-0-only table).
#[derive(Copy, Clone, Debug, Default)]
pub struct NoStyles;

impl VisualStyleSource for NoStyles {
    fn visual(&self, _id: StyleId) -> Option<VisualAttrs> {
        None
    }
}

/// Builds the deduplicated [`LoweredStyle`] table incrementally as cells are
/// lowered, assigning each distinct visual style a stable `style_key`. Key 0
/// is ALWAYS the default style ([`LoweredStyle::default_key0`]); a cell with
/// no visual styling (an unstyled `StyleId`, or a `StyleId` the source maps
/// to a default `VisualAttrs`) reuses key 0.
///
/// Determinism (spec §12.4): keys are assigned in the order cells are
/// resolved, and the page lowering / grid scene both walk cells in index
/// order, so the same `(model, range, source)` yields the same table. The
/// resolver caches by `StyleId` so two cells sharing a style share a key
/// without re-querying the source.
pub struct StyleResolver<'a, S: VisualStyleSource> {
    source: &'a S,
    /// The output table; index == `key`. Seeded with the default at key 0.
    styles: Vec<LoweredStyle>,
    /// `StyleId.0` → assigned `style_key`, memoizing the source lookup +
    /// the dedup search so each distinct `StyleId` is resolved once.
    by_style_id: HashMap<u32, u32>,
    /// `VisualAttrs` (as its wire `LoweredStyle` minus key) → `style_key`,
    /// so two DIFFERENT `StyleId`s that resolve to the SAME visual style
    /// share one table entry (true deduplication, not just per-id caching).
    dedup: HashMap<StyleKeyless, u32>,
}

/// A `LoweredStyle` without its `key`, used as the dedup map key. The `key`
/// is positional (the table index), so it must NOT participate in equality.
#[derive(Clone, PartialEq, Eq, Hash)]
struct StyleKeyless {
    bold: bool,
    italic: bool,
    /// f64 has no `Eq`/`Hash`; we key on the IEEE bit pattern (style sizes
    /// are exact small decimals from the xlsx, so bitwise equality is the
    /// right notion here — two "11.0" sizes share a bucket).
    font_size_bits: Option<u64>,
    font_name: Option<String>,
    fill_rgb: Option<String>,
    text_rgb: Option<String>,
    border_top: bool,
    border_right: bool,
    border_bottom: bool,
    border_left: bool,
}

impl StyleKeyless {
    fn of(s: &LoweredStyle) -> Self {
        StyleKeyless {
            bold: s.bold,
            italic: s.italic,
            font_size_bits: s.font_size_pt.map(f64::to_bits),
            font_name: s.font_name.clone(),
            fill_rgb: s.fill_rgb.clone(),
            text_rgb: s.text_rgb.clone(),
            border_top: s.border_top,
            border_right: s.border_right,
            border_bottom: s.border_bottom,
            border_left: s.border_left,
        }
    }
}

impl<'a, S: VisualStyleSource> StyleResolver<'a, S> {
    /// A fresh resolver, table seeded with the default style at key 0.
    pub fn new(source: &'a S) -> Self {
        StyleResolver {
            source,
            styles: vec![LoweredStyle::default_key0()],
            by_style_id: HashMap::new(),
            dedup: HashMap::new(),
            // The default style occupies key 0; record it so a source that
            // explicitly returns a default `VisualAttrs` reuses it.
        }
    }

    /// Resolve a cell's `StyleId` to its `style_key`, interning a new
    /// [`LoweredStyle`] into the table the first time a distinct visual style
    /// is seen. An unstyled / default style returns key 0.
    pub fn key_for(&mut self, id: StyleId) -> u32 {
        if let Some(&k) = self.by_style_id.get(&id.0) {
            return k;
        }
        let attrs = self.source.visual(id).unwrap_or_default();
        let key = if attrs.is_default() {
            0 // folds onto the default entry
        } else {
            let lowered = attrs.into_lowered(0); // key fixed below
            let dedup_key = StyleKeyless::of(&lowered);
            if let Some(&existing) = self.dedup.get(&dedup_key) {
                existing
            } else {
                let new_key = self.styles.len() as u32;
                self.styles.push(LoweredStyle {
                    key: new_key,
                    ..lowered
                });
                self.dedup.insert(dedup_key, new_key);
                new_key
            }
        };
        self.by_style_id.insert(id.0, key);
        key
    }

    /// Consume the resolver, yielding the finished `styles` table (index ==
    /// key; entry 0 is always the default). The page lowering puts this in
    /// `LoweredContent.styles`; the grid scene in `GridScene.styles`.
    pub fn into_styles(self) -> Vec<LoweredStyle> {
        self.styles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A map-backed source for tests (StyleId.0 → attrs).
    fn source(map: BTreeMap<u32, VisualAttrs>) -> impl VisualStyleSource {
        move |id: StyleId| map.get(&id.0).cloned()
    }

    fn bold() -> VisualAttrs {
        VisualAttrs {
            bold: true,
            font_size_pt: Some(11.0),
            font_name: Some("Calibri".into()),
            ..Default::default()
        }
    }

    fn yellow_fill() -> VisualAttrs {
        VisualAttrs {
            fill_rgb: Some("#FFFF00".into()),
            ..Default::default()
        }
    }

    #[test]
    fn default_table_has_only_key0() {
        let r = StyleResolver::new(&NoStyles);
        let styles = r.into_styles();
        assert_eq!(styles, vec![LoweredStyle::default_key0()]);
    }

    #[test]
    fn no_styles_source_folds_everything_to_key0() {
        let mut r = StyleResolver::new(&NoStyles);
        assert_eq!(r.key_for(StyleId(0)), 0);
        assert_eq!(r.key_for(StyleId(5)), 0);
        assert_eq!(r.key_for(StyleId(99)), 0);
        assert_eq!(r.into_styles().len(), 1);
    }

    #[test]
    fn distinct_styles_get_distinct_keys() {
        let mut map = BTreeMap::new();
        map.insert(1u32, bold());
        map.insert(2u32, yellow_fill());
        let src = source(map);
        let mut r = StyleResolver::new(&src);

        assert_eq!(r.key_for(StyleId(0)), 0); // default → key 0
        let kb = r.key_for(StyleId(1));
        let ky = r.key_for(StyleId(2));
        assert_ne!(kb, 0);
        assert_ne!(ky, 0);
        assert_ne!(kb, ky);

        let styles = r.into_styles();
        assert_eq!(styles.len(), 3); // default + bold + fill
        assert_eq!(styles[0], LoweredStyle::default_key0());
        assert!(styles[kb as usize].bold);
        assert_eq!(styles[ky as usize].fill_rgb.as_deref(), Some("#FFFF00"));
        // The key field equals the table index.
        for (i, s) in styles.iter().enumerate() {
            assert_eq!(s.key, i as u32);
        }
    }

    #[test]
    fn same_style_id_reuses_key() {
        let mut map = BTreeMap::new();
        map.insert(1u32, bold());
        let src = source(map);
        let mut r = StyleResolver::new(&src);
        let a = r.key_for(StyleId(1));
        let b = r.key_for(StyleId(1));
        assert_eq!(a, b);
        assert_eq!(r.into_styles().len(), 2); // default + the one bold
    }

    #[test]
    fn distinct_ids_same_visual_dedup_to_one_entry() {
        // Two different StyleIds whose visual attributes are identical share
        // ONE table entry (true dedup — the IR carries each visual style once).
        let mut map = BTreeMap::new();
        map.insert(1u32, bold());
        map.insert(7u32, bold());
        let src = source(map);
        let mut r = StyleResolver::new(&src);
        let k1 = r.key_for(StyleId(1));
        let k7 = r.key_for(StyleId(7));
        assert_eq!(k1, k7);
        assert_eq!(r.into_styles().len(), 2); // default + one shared bold
    }

    #[test]
    fn explicit_default_attrs_fold_to_key0() {
        // A source that returns a *default* VisualAttrs (not None) still
        // folds onto key 0 — `is_default` is the "has styling?" test.
        let mut map = BTreeMap::new();
        map.insert(3u32, VisualAttrs::default());
        let src = source(map);
        let mut r = StyleResolver::new(&src);
        assert_eq!(r.key_for(StyleId(3)), 0);
        assert_eq!(r.into_styles().len(), 1);
    }
}
