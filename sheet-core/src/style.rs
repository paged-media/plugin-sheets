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

//! Minimal interned cell style (spec §5.1, T0). A `CellStyle` is a tuple of
//! sub-table indices (font/fill/border are opaque `u32` slots filled in by
//! `sheet-xlsx`); the whole struct is interned to a `StyleId`. Number
//! formats are a separate dedup table keyed by raw format code.

use crate::cell::StyleId;
use crate::intern::Interner;
use compact_str::CompactString;

/// Index into the number-format table. `NumFmtId(0)` is always "General".
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct NumFmtId(pub u32);

/// Horizontal alignment (T0 subset).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default, serde::Serialize)]
pub enum Align {
    #[default]
    General,
    Left,
    Center,
    Right,
}

/// A fully resolved cell style. `font`/`fill`/`border` are opaque indices
/// into sub-tables owned by the XLSX layer (T0 keeps them as raw `u32`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct CellStyle {
    pub num_fmt: NumFmtId,
    pub font: u32,
    pub fill: u32,
    pub border: u32,
    pub align: Align,
}

/// The workbook style registry. `styles` interns `CellStyle` to `StyleId`
/// (id 0 == the default style). `num_fmts` maps `NumFmtId` -> raw format
/// code (index 0 == "General").
#[derive(Debug)]
pub struct StyleTable {
    styles: Interner<CellStyle>,
    num_fmts: Vec<CompactString>,
}

impl StyleTable {
    /// Seed the defaults so `StyleId(0)` and `NumFmtId(0)` are valid:
    /// num_fmt[0] = "General", and the default `CellStyle` interns to id 0.
    pub fn new() -> Self {
        let mut styles = Interner::new();
        let id0 = styles.intern(CellStyle::default());
        debug_assert_eq!(id0, 0);
        StyleTable {
            styles,
            num_fmts: vec![CompactString::new("General")],
        }
    }

    /// Intern a style, returning its (deduped) id.
    pub fn intern_style(&mut self, style: CellStyle) -> StyleId {
        StyleId(self.styles.intern(style))
    }

    /// Resolve a `StyleId`. Out-of-range ids fall back to the default style.
    pub fn style(&self, id: StyleId) -> &CellStyle {
        self.styles.get(id.0).unwrap_or_else(|| {
            // id 0 is always present (seeded in `new`).
            self.styles.get(0).expect("default style seeded")
        })
    }

    /// Intern a raw number-format code, deduping against existing codes.
    pub fn intern_num_fmt(&mut self, code: &str) -> NumFmtId {
        if let Some(pos) = self.num_fmts.iter().position(|c| c == code) {
            return NumFmtId(pos as u32);
        }
        let id = self.num_fmts.len() as u32;
        self.num_fmts.push(CompactString::new(code));
        NumFmtId(id)
    }

    /// Resolve a `NumFmtId` to its raw code. Out-of-range falls back to
    /// "General".
    pub fn num_fmt(&self, id: NumFmtId) -> &str {
        self.num_fmts
            .get(id.0 as usize)
            .map(CompactString::as_str)
            .unwrap_or("General")
    }

    /// The number-format code of a style (convenience).
    pub fn num_fmt_of(&self, id: StyleId) -> &str {
        self.num_fmt(self.style(id).num_fmt)
    }
}

impl Default for StyleTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_seeded() {
        let t = StyleTable::new();
        assert_eq!(*t.style(StyleId(0)), CellStyle::default());
        assert_eq!(t.num_fmt(NumFmtId(0)), "General");
        assert_eq!(t.num_fmt_of(StyleId(0)), "General");
    }

    #[test]
    fn intern_style_dedups_and_assigns() {
        let mut t = StyleTable::new();
        let s = CellStyle {
            font: 3,
            ..Default::default()
        };
        let a = t.intern_style(s.clone());
        let b = t.intern_style(s);
        assert_eq!(a, b);
        assert_ne!(a, StyleId(0)); // distinct from default
                                   // re-interning the default returns id 0
        assert_eq!(t.intern_style(CellStyle::default()), StyleId(0));
    }

    #[test]
    fn intern_num_fmt_dedups() {
        let mut t = StyleTable::new();
        let a = t.intern_num_fmt("0.00");
        let b = t.intern_num_fmt("0.00");
        assert_eq!(a, b);
        assert_ne!(a, NumFmtId(0));
        assert_eq!(t.num_fmt(a), "0.00");
        // "General" dedups against the seeded entry.
        assert_eq!(t.intern_num_fmt("General"), NumFmtId(0));
    }
}
