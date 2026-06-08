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

//! # sheet-lower — pure model -> `LoweredContent` IR (spec §8.2/§8.3, T0)
//!
//! Lowers one `(sheet, range, view options)` binding into the
//! [`LoweredContent`] IR a host frame is compiled from. T0 is a **single
//! frame**: no threading/pagination (that is T1 — `header_rows` is parsed
//! and carried but unused, documented below). Lowering is **pure** —
//! `&SheetModel -> LoweredContent`, no SDK, no mutation, no evaluation
//! (formula cells lower their *cached* `sheet_core::Cell::value`; spec:
//! lowered content is derived state).
//!
//! The Rust IR is the source of truth for the wire shape; the TS mirror
//! `packages/sheet-host-model/src/lowered.ts` is the SAME contract. serde
//! emits `camelCase` (so `widthPt`/`heightPt`/`rowSpan`/`colSpan`) and the
//! `align` field renders the LOWERCASE `Align` variant name
//! (`"general"`/`"left"`/`"center"`/`"right"`) via [`serialize_align`] —
//! `sheet_core::Align`'s own derive would emit `"General"`, so we override
//! it here without touching the frozen leaf crate.
//!
//! ## T0 rulings (each documented at its use site)
//!
//! - **Column width** (spec §8.2): model `col_widths` are xlsx character
//!   units; `width_pt = chars * `[`CHAR_TO_PT`]. Absent => the xlsx default
//!   [`DEFAULT_COL_CHARS`] (8.43 ch => 44.2575 pt).
//! - **Row height** (spec §8.2): model `row_heights` are already points;
//!   absent => [`DEFAULT_ROW_PT`] (15.0 pt).
//! - **Cell text** (spec §8.3): "the lowered text *is* the formatted
//!   value" — each cell's style resolves to a number-format code
//!   (`StyleTable::num_fmt_of`), compiled via a local [`FormatCache`] and
//!   rendered by [`format_value`] with the workbook date system. Empty
//!   cells lower to `""` so the IR's `cells` vector covers the FULL range
//!   width positionally (the TS tab-joiner relies on positional columns).
//! - **Align** (Excel default): the style's `align` if not `General`,
//!   otherwise type-derived — numbers/dates right, text left, bool/error
//!   centered. T0 has no per-cell numeric-vs-date split beyond the format,
//!   so any `Number` is treated as right-aligned (dates are numbers).
//! - **Rules** (spec §8.2/§8.5, content space): with
//!   [`ViewOptions::include_grid_rules`], a horizontal rule sits at every
//!   row boundary `0..=rows` (`at` = cumulative y, spanning the full
//!   width) and a vertical rule at every column boundary `0..=cols`
//!   (`at` = cumulative x, spanning the full height). Origin is the
//!   range's top-left (frame-content coordinates).
//! - **Merges** (spec §8.2): model merges intersecting the range, clipped
//!   to the range and re-based so `row`/`col` are offsets *within* the
//!   range.
//!
//! ## Determinism (spec §12.4)
//!
//! Output is a deterministic function of `(model, sheet, range, opts)`:
//! the worksheet cell grid is a `BTreeMap` (ordered iteration) and the
//! lowering walks rows/cols in index order, so two runs yield
//! `serde_json`-equal output (asserted in tests).

use sheet_core::{Align, Cell, CellValue, SheetId, SheetModel};
use sheet_format::{FormatCache, FormatCtx};

// ---- Multi-frame pagination (spec §8.2, T1; the "killer feature"). ----
//
// `lower_range` (this file) is the single-frame primitive; `paginate` threads
// a tall range across an ordered list of frame content-boxes (S-05: the chain
// TOPOLOGY is still an SDK gap, so the caller supplies the ordered frames).
pub mod paginate;
pub use paginate::{paginate, FrameBox, Page, PaginateOptions};

// ---- Style resolution (IR v2, M1 style-map track; spec §8.3). ----
//
// The resolver that turns a cell's `StyleId` + a `VisualStyleSource` into the
// deduplicated `LoweredContent.styles` table + per-cell `style_key`. The SAME
// resolver feeds the grid scene (cross-surface parity).
pub mod style;
pub use style::{NoStyles, StyleResolver, VisualAttrs, VisualStyleSource};

// ---- Conditional formatting → style overrides (T2 cond-fmt track; §10.4). ----
//
// Pure value-comparison + colour-scale evaluation that folds a matched cf rule's
// differential format into the lowered style (no `sheet-calc` / `sheet-xlsx`
// dep — the xlsx side converts its parsed cf model into these mirror types).
pub mod condfmt;
pub use condfmt::{CfBlock, CfOperator, CfRule, CfRuleKind, ColorScale, ScaleStop, SheetCondFmt};

// ---- T0 geometry rulings (spec §8.2). ----

/// xlsx character-unit -> point conversion for column widths (T0 ruling,
/// spec §8.2). One xlsx "character" ~= 7 px at 96 dpi; in points
/// `7 px * 72/96 = 5.25 pt`.
pub const CHAR_TO_PT: f64 = 5.25;

/// The xlsx default column width in characters (`<sheetFormatPr
/// defaultColWidth>` absent => Excel's 8.43). Used when a column has no
/// explicit width — `8.43 * 5.25 = 44.2575 pt`.
pub const DEFAULT_COL_CHARS: f64 = 8.43;

/// The default row height in points (Excel's 15 pt for the default font).
/// Model `row_heights` are already points, so this is a direct fallback.
pub const DEFAULT_ROW_PT: f64 = 15.0;

// ---- The IR (FROZEN wire shape; mirrored by lowered.ts). ----

/// The complete lowered region for one frame (spec §8.2). Column + row
/// geometry, grid rules, and merges — pure data the host translator
/// (`lower-to-mutations.ts`) compiles into Mutations. It computes none of
/// this; the engine already did.
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoweredContent {
    pub cols: Vec<LoweredCol>,
    pub rows: Vec<LoweredRow>,
    pub rules: RuleSet,
    pub merges: Vec<MergeSpan>,
    /// The style map (IR v2, M1 style-map track). Indexed by
    /// [`LoweredCell::style_key`]; key 0 is the default style. T0 emits a
    /// one-entry table (`[LoweredStyle::default_key0()]`); the style-map
    /// track populates real styles in Phase B. Additive — the TS mirror
    /// keeps it optional so existing fixtures stay valid.
    pub styles: Vec<LoweredStyle>,
}

/// One column's geometry: its range-relative index and lowered width (pt).
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoweredCol {
    pub index: u32,
    pub width_pt: f64,
}

/// One row's geometry plus its cells. The `cells` vector covers the FULL
/// range width positionally (empty cells carry `text: ""`) so the host
/// tab-joiner aligns columns (spec §8.3 / S-03 degradation).
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoweredRow {
    pub index: u32,
    pub height_pt: f64,
    pub cells: Vec<LoweredCell>,
}

/// One lowered cell: the range-relative column, the FORMATTED text (the
/// number-format output IS the text, spec §8.3), and the resolved
/// alignment.
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoweredCell {
    pub col: u32,
    pub text: String,
    /// Serialized as the lowercase variant name to match `lowered.ts`'s
    /// `Align` union — `sheet_core::Align`'s own derive would emit
    /// PascalCase, so we override only the wire form here.
    #[serde(serialize_with = "serialize_align")]
    pub align: Align,
    /// Index into [`LoweredContent::styles`] (IR v2, M1 style-map track).
    /// `0` = the default style. T0 emits `0` everywhere (real per-cell
    /// styles arrive with the style-map track in Phase B). serde camelCase
    /// → `styleKey`.
    pub style_key: u32,
}

/// One visual cell style (IR v2, M1 style-map track). A flat, host-ready
/// description (bold/italic, font, fills, borders) the translator maps to
/// frame styling. T0 only ever emits the default ([`LoweredStyle::
/// default_key0`]); the XLSX visual-parse + IR-styles track fills real
/// entries in Phase B. All `Option`/`bool` fields are additive on the wire
/// (the TS mirror keeps them optional).
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoweredStyle {
    pub key: u32,
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

impl LoweredStyle {
    /// The default style at key 0: no emphasis, no explicit font/fill/text
    /// colour, no borders. T0 emits exactly this single entry.
    pub fn default_key0() -> Self {
        LoweredStyle {
            key: 0,
            bold: false,
            italic: false,
            font_size_pt: None,
            font_name: None,
            fill_rgb: None,
            text_rgb: None,
            border_top: false,
            border_right: false,
            border_bottom: false,
            border_left: false,
        }
    }
}

/// The horizontal/vertical rule sets (spec §8.2 grid rules).
#[derive(serde::Serialize, PartialEq, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleSet {
    /// Horizontal rules — run along x at a given y (`at`).
    pub h: Vec<Rule>,
    /// Vertical rules — run along y at a given x (`at`).
    pub v: Vec<Rule>,
}

/// One drawn line. `at` is the offset along the rule's cross-axis (y for an
/// h-rule, x for a v-rule); `from`..`to` is the extent along its own axis.
/// All content-space pt with origin at the range's top-left.
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub at: f64,
    pub from: f64,
    pub to: f64,
}

/// A merged span anchored at its top-left cell, re-based to range-relative
/// coordinates (spec §8.2).
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MergeSpan {
    pub row: u32,
    pub col: u32,
    pub row_span: u32,
    pub col_span: u32,
}

/// Per-frame view options (spec §8.2). `header_rows` is **parsed and
/// carried but UNUSED in T0**: repeated-header threading is a T1 feature
/// (`sheet.lower.pagination.*`); keeping the field now freezes the call
/// shape so T1 needs no signature change.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    pub include_grid_rules: bool,
    pub header_rows: u32,
}

impl Default for ViewOptions {
    /// Rules on, no header threading — the plain single-frame default.
    fn default() -> Self {
        ViewOptions {
            include_grid_rules: true,
            header_rows: 0,
        }
    }
}

/// An inclusive, 0-based cell range to lower. Endpoints are NOT required to
/// be ordered; [`lower_range`] normalizes them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CellRange {
    pub r0: u32,
    pub c0: u32,
    pub r1: u32,
    pub c1: u32,
}

impl CellRange {
    /// `(top, left, bottom, right)` with `top <= bottom` and
    /// `left <= right` — the canonical inclusive box.
    fn normalized(&self) -> (u32, u32, u32, u32) {
        (
            self.r0.min(self.r1),
            self.c0.min(self.c1),
            self.r0.max(self.r1),
            self.c0.max(self.c1),
        )
    }
}

/// serde hook: render `Align` as the agreed lowercase variant name. The TS
/// contract (`lowered.ts`) is the wire authority — `"general"`/`"left"`/
/// `"center"`/`"right"`.
fn serialize_align<S>(align: &Align, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_str(align_str(*align))
}

/// `Align` -> its lowercase wire token.
fn align_str(align: Align) -> &'static str {
    match align {
        Align::General => "general",
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
    }
}

/// Lower a `(sheet, range, view options)` binding to the [`LoweredContent`]
/// IR for one frame (spec §8.2, T0). Pure: reads the model, never mutates
/// or evaluates. An unknown `sheet` lowers to an empty-but-shaped region
/// (column/row geometry from the range, no cells/merges) rather than
/// panicking — lowering is best-effort derived state.
///
/// This is the unstyled door (frozen signature): every cell resolves to the
/// default style (key 0). Callers with a [`VisualStyleSource`] (the parsed
/// XLSX visual styles) use [`lower_range_styled`] to populate the IR-v2
/// styles table.
pub fn lower_range(
    model: &SheetModel,
    sheet: SheetId,
    range: CellRange,
    opts: &ViewOptions,
) -> LoweredContent {
    lower_range_styled(model, sheet, range, opts, &style::NoStyles)
}

/// Lower a `(sheet, range, view options)` binding, resolving REAL visual
/// styles through `visual` into the IR-v2 [`LoweredContent::styles`] table +
/// per-cell `style_key` (spec §8.3, the style-map track). Identical to
/// [`lower_range`] in every other respect; passing [`style::NoStyles`]
/// reproduces it exactly. The grid scene resolves the SAME styles for the
/// SAME cells (`sheet_grid::grid_scene_styled`) — the cross-surface-parity
/// contract.
pub fn lower_range_styled(
    model: &SheetModel,
    sheet: SheetId,
    range: CellRange,
    opts: &ViewOptions,
    visual: &impl style::VisualStyleSource,
) -> LoweredContent {
    let (top, left, bottom, right) = range.normalized();
    let ws = model.sheet(sheet);

    let ctx = FormatCtx {
        date_system: model.calc.date_system,
    };
    let mut cache = FormatCache::default();
    // The style resolver builds the deduped IR-v2 styles table as cells are
    // walked (in row/col index order — deterministic).
    let mut style_resolver = style::StyleResolver::new(visual);

    // ---- Columns: range-relative index + width (pt). ----
    let mut cols = Vec::with_capacity((right - left + 1) as usize);
    let mut col_x: Vec<f64> = Vec::with_capacity((right - left + 2) as usize);
    let mut x = 0.0_f64;
    for c in left..=right {
        col_x.push(x);
        let chars = ws
            .and_then(|w| w.col_widths.get(&c).copied())
            .unwrap_or(DEFAULT_COL_CHARS);
        let width_pt = chars * CHAR_TO_PT;
        cols.push(LoweredCol {
            index: c - left,
            width_pt,
        });
        x += width_pt;
    }
    col_x.push(x); // trailing boundary
    let total_width = x;

    // ---- Rows: range-relative index + height (pt) + cells. ----
    let mut rows = Vec::with_capacity((bottom - top + 1) as usize);
    let mut row_y: Vec<f64> = Vec::with_capacity((bottom - top + 2) as usize);
    let mut y = 0.0_f64;
    for r in top..=bottom {
        row_y.push(y);
        let height_pt = ws
            .and_then(|w| w.row_heights.get(&r).copied())
            .unwrap_or(DEFAULT_ROW_PT);

        // Cells cover the FULL range width positionally (empty => "").
        let mut cells = Vec::with_capacity((right - left + 1) as usize);
        for c in left..=right {
            let (text, align, style_key) = match ws.and_then(|w| w.cell(r, c)) {
                Some(cell) => {
                    let (text, align) = lower_cell(model, cell, &mut cache, &ctx);
                    // Resolve the cell's StyleId to a deduped IR-v2 key. An
                    // empty cell has nothing to style, so it stays key 0.
                    (text, align, style_resolver.key_for(cell.style))
                }
                None => (String::new(), Align::General, 0),
            };
            cells.push(LoweredCell {
                col: c - left,
                text,
                align,
                style_key,
            });
        }

        rows.push(LoweredRow {
            index: r - top,
            height_pt,
            cells,
        });
        y += height_pt;
    }
    row_y.push(y); // trailing boundary
    let total_height = y;

    // ---- Grid rules (content-space, origin = range top-left). ----
    let rules = if opts.include_grid_rules {
        let h = row_y
            .iter()
            .map(|&at| Rule {
                at,
                from: 0.0,
                to: total_width,
            })
            .collect();
        let v = col_x
            .iter()
            .map(|&at| Rule {
                at,
                from: 0.0,
                to: total_height,
            })
            .collect();
        RuleSet { h, v }
    } else {
        RuleSet::default()
    };

    // ---- Merges: intersect the range, clip, re-base to range-relative. ----
    let merges = lower_merges(ws, sheet, top, left, bottom, right);

    LoweredContent {
        cols,
        rows,
        rules,
        merges,
        // The deduped IR-v2 styles table the cells' `style_key`s index into.
        // With `NoStyles` this is exactly `[LoweredStyle::default_key0()]`
        // (the frozen-`lower_range` behaviour); a real source populates it.
        styles: style_resolver.into_styles(),
    }
}

/// Lower a `(sheet, range, view options)` binding with CONDITIONAL FORMATTING
/// folded into the styles (spec §10.4, §8.3; the M2 cond-fmt track). For every
/// cell, the base visual style (`visual`) is resolved, then any matching cf
/// rule's differential format is folded ON TOP (the §8.3 "constrained local
/// override"). Cells whose cf result equals their base style keep the base key;
/// cf-painted cells get their own deduped key — so two cells with the same base
/// style but different cf outcomes correctly carry DIFFERENT `style_key`s
/// (which the `StyleId`-cached resolver alone cannot express).
///
/// Identical to [`lower_range_styled`] in geometry/text/rules/merges; passing an
/// empty [`condfmt::SheetCondFmt`] reproduces it exactly (every cell's effective
/// style equals its base). Pure: cf evaluation reads ONLY the cell's cached
/// value + the range's value domain — never the calc engine (`sheet-lower` has
/// no `sheet-calc` dep). `expression` rules that need a formula evaluator are
/// DEFERRED (apply no override; documented in `condfmt`).
pub fn lower_range_condfmt(
    model: &SheetModel,
    sheet: SheetId,
    range: CellRange,
    opts: &ViewOptions,
    visual: &impl style::VisualStyleSource,
    cf: &condfmt::SheetCondFmt,
) -> LoweredContent {
    // No conditional formatting → exactly the styled path (no extra work, no
    // table churn). Keeps the common case identical to `lower_range_styled`.
    if cf.is_empty() {
        return lower_range_styled(model, sheet, range, opts, visual);
    }

    let (top, left, bottom, right) = range.normalized();
    let ws = model.sheet(sheet);

    let ctx = FormatCtx {
        date_system: model.calc.date_system,
    };
    let mut cache = FormatCache::default();

    // The cf-aware style table: deduped EFFECTIVE styles (base folded with cf).
    let mut styles: Vec<LoweredStyle> = vec![LoweredStyle::default_key0()];
    let mut dedup: std::collections::HashMap<StyleKeyless, u32> = std::collections::HashMap::new();
    // The colour-scale domain is computed lazily per covering range, then cached
    // (a range usually appears once, but a block with many cells re-queries it).
    let mut domain_cache: std::collections::HashMap<condfmt::CfRange, Option<(f64, f64)>> =
        std::collections::HashMap::new();

    // ---- Columns: range-relative index + width (pt). ----
    let mut cols = Vec::with_capacity((right - left + 1) as usize);
    let mut col_x: Vec<f64> = Vec::with_capacity((right - left + 2) as usize);
    let mut x = 0.0_f64;
    for c in left..=right {
        col_x.push(x);
        let chars = ws
            .and_then(|w| w.col_widths.get(&c).copied())
            .unwrap_or(DEFAULT_COL_CHARS);
        let width_pt = chars * CHAR_TO_PT;
        cols.push(LoweredCol {
            index: c - left,
            width_pt,
        });
        x += width_pt;
    }
    col_x.push(x);
    let total_width = x;

    // ---- Rows: range-relative index + height (pt) + cells (cf-styled). ----
    let mut rows = Vec::with_capacity((bottom - top + 1) as usize);
    let mut row_y: Vec<f64> = Vec::with_capacity((bottom - top + 2) as usize);
    let mut y = 0.0_f64;
    for r in top..=bottom {
        row_y.push(y);
        let height_pt = ws
            .and_then(|w| w.row_heights.get(&r).copied())
            .unwrap_or(DEFAULT_ROW_PT);

        let mut cells = Vec::with_capacity((right - left + 1) as usize);
        for c in left..=right {
            let (text, align, style_key) = match ws.and_then(|w| w.cell(r, c)) {
                Some(cell) => {
                    let (text, align) = lower_cell(model, cell, &mut cache, &ctx);
                    let base = visual.visual(cell.style).unwrap_or_default();
                    // Evaluate cf against the cell's CACHED value at its MODEL
                    // (absolute) coordinates — cf ranges are sheet-relative.
                    let effective = match condfmt::override_for(cf, r, c, &cell.value, &mut |rng| {
                        cf_domain(ws, rng, &mut domain_cache)
                    }) {
                        Some(over) => condfmt::fold_override(&base, &over),
                        None => base,
                    };
                    let key = intern_effective(&mut styles, &mut dedup, effective);
                    (text, align, key)
                }
                None => (String::new(), Align::General, 0),
            };
            cells.push(LoweredCell {
                col: c - left,
                text,
                align,
                style_key,
            });
        }

        rows.push(LoweredRow {
            index: r - top,
            height_pt,
            cells,
        });
        y += height_pt;
    }
    row_y.push(y);
    let total_height = y;

    let rules = if opts.include_grid_rules {
        let h = row_y
            .iter()
            .map(|&at| Rule {
                at,
                from: 0.0,
                to: total_width,
            })
            .collect();
        let v = col_x
            .iter()
            .map(|&at| Rule {
                at,
                from: 0.0,
                to: total_height,
            })
            .collect();
        RuleSet { h, v }
    } else {
        RuleSet::default()
    };

    let merges = lower_merges(ws, sheet, top, left, bottom, right);

    LoweredContent {
        cols,
        rows,
        rules,
        merges,
        styles,
    }
}

/// The cf colour-scale domain `(min, max)` of a covering range: the min/max of
/// the NUMERIC cell values in that range (model coordinates), memoized. `None`
/// when the range holds no numeric cells (a degenerate scale paints nothing).
fn cf_domain(
    ws: Option<&sheet_core::Worksheet>,
    rng: condfmt::CfRange,
    cache: &mut std::collections::HashMap<condfmt::CfRange, Option<(f64, f64)>>,
) -> Option<(f64, f64)> {
    if let Some(&hit) = cache.get(&rng) {
        return hit;
    }
    let (r0, c0, r1, c1) = rng;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    if let Some(ws) = ws {
        for r in r0..=r1 {
            for c in c0..=c1 {
                if let Some(&CellValue::Number(n)) = ws.cell(r, c).map(|cell| &cell.value) {
                    lo = lo.min(n);
                    hi = hi.max(n);
                }
            }
        }
    }
    let out = if lo.is_finite() && hi.is_finite() {
        Some((lo, hi))
    } else {
        None
    };
    cache.insert(rng, out);
    out
}

/// Intern an EFFECTIVE [`VisualAttrs`] (base folded with cf) into the cf-aware
/// styles table, deduping by visual content (key 0 stays the default). The
/// dedup mirrors `style::StyleResolver` but keys on the effective attrs (not the
/// `StyleId`), since cf makes the same `StyleId` resolve to different styles.
fn intern_effective(
    styles: &mut Vec<LoweredStyle>,
    dedup: &mut std::collections::HashMap<StyleKeyless, u32>,
    attrs: VisualAttrs,
) -> u32 {
    if attrs.is_default() {
        return 0;
    }
    let lowered = LoweredStyle {
        key: 0,
        bold: attrs.bold,
        italic: attrs.italic,
        font_size_pt: attrs.font_size_pt,
        font_name: attrs.font_name,
        fill_rgb: attrs.fill_rgb,
        text_rgb: attrs.text_rgb,
        border_top: attrs.border_top,
        border_right: attrs.border_right,
        border_bottom: attrs.border_bottom,
        border_left: attrs.border_left,
    };
    let dk = StyleKeyless::of(&lowered);
    if let Some(&existing) = dedup.get(&dk) {
        return existing;
    }
    let new_key = styles.len() as u32;
    styles.push(LoweredStyle {
        key: new_key,
        ..lowered
    });
    dedup.insert(dk, new_key);
    new_key
}

/// A [`LoweredStyle`] without its positional `key`, the dedup map key for the
/// cf-aware table (mirrors the private `style::StyleKeyless`; re-declared here
/// because that one is module-private and this path keys on effective attrs).
#[derive(Clone, PartialEq, Eq, Hash)]
struct StyleKeyless {
    bold: bool,
    italic: bool,
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

/// Lower one cell to `(formatted text, resolved align)`. The text is the
/// number-format render of the cell's CACHED value (spec §8.3 — never
/// evaluated); the align is the Excel default resolution.
fn lower_cell(
    model: &SheetModel,
    cell: &Cell,
    cache: &mut FormatCache,
    ctx: &FormatCtx,
) -> (String, Align) {
    let code = model.styles.num_fmt_of(cell.style);
    // A malformed stored format code must not break lowering — fall back to
    // General (its compile cannot fail), matching the calc/format engines'
    // never-panic posture.
    let text = match cache.get(code) {
        Ok(fmt) => sheet_format::format_value(&cell.value, fmt, ctx),
        Err(_) => sheet_format::format_general(&cell.value),
    };
    let style_align = model.styles.style(cell.style).align;
    let align = resolve_align(style_align, &cell.value);
    (text, align)
}

/// Excel-default alignment (T0 ruling): an explicit non-`General` style
/// alignment wins; otherwise it is type-derived — numbers/dates right
/// (dates are numbers in T0, so any `Number` is right), text left,
/// bool/error centered, and a blank/`Empty` cell stays `General`
/// (nothing to align).
fn resolve_align(style_align: Align, value: &CellValue) -> Align {
    if style_align != Align::General {
        return style_align;
    }
    match value {
        CellValue::Number(_) => Align::Right,
        CellValue::Text(_) => Align::Left,
        CellValue::Bool(_) | CellValue::Error(_) => Align::Center,
        CellValue::Empty => Align::General,
    }
}

/// Collect the model merges that intersect the range, clipped to it and
/// re-based so `(row, col)` are offsets within the range (spec §8.2). A
/// merge that overlaps a range edge is clipped to the visible portion; its
/// span counts only the in-range rows/cols. Iteration order follows the
/// stored `merges` vector (deterministic).
fn lower_merges(
    ws: Option<&sheet_core::Worksheet>,
    sheet: SheetId,
    top: u32,
    left: u32,
    bottom: u32,
    right: u32,
) -> Vec<MergeSpan> {
    let Some(ws) = ws else { return Vec::new() };
    let mut out = Vec::new();
    for m in &ws.merges {
        let n = m.normalized();
        // Merges are sheet-local in the model; guard the sheet match
        // defensively (RangeRef carries a sheet on each endpoint).
        if n.start.sheet != sheet {
            continue;
        }
        let (mr0, mc0, mr1, mc1) = (n.start.row, n.start.col, n.end.row, n.end.col);
        // Intersect with the range box.
        let ir0 = mr0.max(top);
        let ic0 = mc0.max(left);
        let ir1 = mr1.min(bottom);
        let ic1 = mc1.min(right);
        if ir0 > ir1 || ic0 > ic1 {
            continue; // no overlap
        }
        out.push(MergeSpan {
            row: ir0 - top,
            col: ic0 - left,
            row_span: ir1 - ir0 + 1,
            col_span: ic1 - ic0 + 1,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::{CellStyle, StyleId};

    /// Build a 1-sheet model with the given cells set at `(row, col)`.
    fn model_with(cells: &[(u32, u32, Cell)]) -> (SheetModel, SheetId) {
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let ws = m.sheet_mut(s).unwrap();
        for (r, c, cell) in cells {
            ws.set_cell(*r, *c, cell.clone());
        }
        (m, s)
    }

    fn num(n: f64) -> Cell {
        Cell {
            value: CellValue::Number(n),
            ..Default::default()
        }
    }

    fn text(s: &str) -> Cell {
        Cell {
            value: CellValue::from(s),
            ..Default::default()
        }
    }

    #[test]
    fn cols_rows_geometry_defaults() {
        let (m, s) = model_with(&[]);
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 1,
                c1: 1,
            },
            &ViewOptions::default(),
        );
        assert_eq!(lc.cols.len(), 2);
        assert_eq!(lc.rows.len(), 2);
        // Default col width 8.43 ch * 5.25 = 44.2575 pt.
        assert!((lc.cols[0].width_pt - 44.2575).abs() < 1e-9);
        assert_eq!(lc.cols[1].index, 1);
        // Default row height 15 pt.
        assert!((lc.rows[0].height_pt - 15.0).abs() < 1e-9);
        // Every row carries a full-width cells vector (positional).
        assert_eq!(lc.rows[0].cells.len(), 2);
        assert_eq!(lc.rows[0].cells[0].col, 0);
        assert_eq!(lc.rows[0].cells[0].text, "");
        assert_eq!(lc.rows[0].cells[0].align, Align::General);
    }

    #[test]
    fn explicit_col_width_and_row_height() {
        let (mut m, s) = model_with(&[]);
        let ws = m.sheet_mut(s).unwrap();
        ws.col_widths.insert(0, 10.0); // 10 ch -> 52.5 pt
        ws.row_heights.insert(0, 30.0); // already pt
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 0,
            },
            &ViewOptions::default(),
        );
        assert!((lc.cols[0].width_pt - 52.5).abs() < 1e-9);
        assert!((lc.rows[0].height_pt - 30.0).abs() < 1e-9);
    }

    #[test]
    fn normalizes_swapped_range() {
        let (m, s) = model_with(&[(0, 0, num(1.0))]);
        let forward = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 1,
                c1: 1,
            },
            &ViewOptions::default(),
        );
        let swapped = lower_range(
            &m,
            s,
            CellRange {
                r0: 1,
                c0: 1,
                r1: 0,
                c1: 0,
            },
            &ViewOptions::default(),
        );
        assert_eq!(forward, swapped);
    }

    #[test]
    fn align_type_derived() {
        let (m, s) = model_with(&[
            (0, 0, num(42.0)),
            (0, 1, text("hi")),
            (
                0,
                2,
                Cell {
                    value: CellValue::Bool(true),
                    ..Default::default()
                },
            ),
        ]);
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 2,
            },
            &ViewOptions::default(),
        );
        let cells = &lc.rows[0].cells;
        assert_eq!(cells[0].align, Align::Right); // number
        assert_eq!(cells[1].align, Align::Left); // text
        assert_eq!(cells[2].align, Align::Center); // bool
    }

    #[test]
    fn explicit_style_align_overrides_type() {
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let style = m.styles.intern_style(CellStyle {
            align: Align::Center,
            ..Default::default()
        });
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(
            0,
            0,
            Cell {
                value: CellValue::Number(1.0),
                style,
                ..Default::default()
            },
        );
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 0,
            },
            &ViewOptions::default(),
        );
        // Number would default Right; the explicit Center style wins.
        assert_eq!(lc.rows[0].cells[0].align, Align::Center);
    }

    #[test]
    fn text_is_number_formatted() {
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let fmt = m.styles.intern_num_fmt("0.00");
        let style = m.styles.intern_style(CellStyle {
            num_fmt: fmt,
            ..Default::default()
        });
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(
            0,
            0,
            Cell {
                value: CellValue::Number(3.5),
                style,
                ..Default::default()
            },
        );
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 0,
            },
            &ViewOptions::default(),
        );
        assert_eq!(lc.rows[0].cells[0].text, "3.50");
    }

    #[test]
    fn formula_cell_lowers_cached_value() {
        // A formula cell carries a cached value; lowering renders THAT,
        // never re-evaluating (spec: derived state).
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let fid = m.intern_formula(sheet_core::Formula {
            root: sheet_core::Expr::Lit(sheet_core::LitValue::Number(sheet_core::OrderedF64::new(
                999.0,
            ))),
        });
        let ws = m.sheet_mut(s).unwrap();
        ws.set_cell(
            0,
            0,
            Cell {
                value: CellValue::Number(7.0), // cached result != formula literal
                formula: Some(fid),
                style: StyleId(0),
            },
        );
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 0,
            },
            &ViewOptions::default(),
        );
        assert_eq!(lc.rows[0].cells[0].text, "7");
    }

    #[test]
    fn rules_at_every_boundary() {
        let (m, s) = model_with(&[]);
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 1,
                c1: 1,
            },
            &ViewOptions::default(),
        );
        // 2 rows -> 3 horizontal boundaries; 2 cols -> 3 vertical.
        assert_eq!(lc.rules.h.len(), 3);
        assert_eq!(lc.rules.v.len(), 3);
        // First h-rule at y=0, last at total height (2 * 15 = 30).
        assert_eq!(lc.rules.h[0].at, 0.0);
        assert!((lc.rules.h[2].at - 30.0).abs() < 1e-9);
        // h-rules span the full width (2 * 44.2575).
        let w = 2.0 * 44.2575;
        assert!((lc.rules.h[0].to - w).abs() < 1e-9);
        // v-rules span the full height.
        assert!((lc.rules.v[0].to - 30.0).abs() < 1e-9);
        assert!((lc.rules.v[2].at - w).abs() < 1e-9);
    }

    #[test]
    fn rules_omitted_when_disabled() {
        let (m, s) = model_with(&[]);
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 1,
                c1: 1,
            },
            &ViewOptions {
                include_grid_rules: false,
                header_rows: 0,
            },
        );
        assert!(lc.rules.h.is_empty());
        assert!(lc.rules.v.is_empty());
    }

    #[test]
    fn merges_clipped_and_rebased() {
        use sheet_core::{CellRef, RangeRef};
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let ws = m.sheet_mut(s).unwrap();
        // A merge B2:D4 (rows 1..=3, cols 1..=3) in absolute model coords.
        let mk = |row, col| CellRef {
            sheet: s,
            row,
            col,
            row_abs: false,
            col_abs: false,
        };
        ws.merges.push(RangeRef {
            start: mk(1, 1),
            end: mk(3, 3),
        });
        // Lower range C3:E5 (rows 2..=4, cols 2..=4) — overlaps the merge in
        // rows 2..=3, cols 2..=3.
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 2,
                c0: 2,
                r1: 4,
                c1: 4,
            },
            &ViewOptions::default(),
        );
        assert_eq!(lc.merges.len(), 1);
        let ms = &lc.merges[0];
        // Clipped overlap is rows 2..=3, cols 2..=3 -> range-relative
        // (top=2,left=2): row 0, col 0, 2x2.
        assert_eq!(ms.row, 0);
        assert_eq!(ms.col, 0);
        assert_eq!(ms.row_span, 2);
        assert_eq!(ms.col_span, 2);
    }

    #[test]
    fn merge_outside_range_dropped() {
        use sheet_core::{CellRef, RangeRef};
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let ws = m.sheet_mut(s).unwrap();
        let mk = |row, col| CellRef {
            sheet: s,
            row,
            col,
            row_abs: false,
            col_abs: false,
        };
        // Merge far away (rows 10..=11).
        ws.merges.push(RangeRef {
            start: mk(10, 10),
            end: mk(11, 11),
        });
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 2,
                c1: 2,
            },
            &ViewOptions::default(),
        );
        assert!(lc.merges.is_empty());
    }

    #[test]
    fn unknown_sheet_lowers_empty_shape() {
        let (m, _s) = model_with(&[]);
        // Sheet id 99 does not exist.
        let lc = lower_range(
            &m,
            99,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 1,
                c1: 1,
            },
            &ViewOptions::default(),
        );
        // Geometry still derives from the range; no cells/merges crash.
        assert_eq!(lc.cols.len(), 2);
        assert_eq!(lc.rows.len(), 2);
        assert_eq!(lc.rows[0].cells.len(), 2);
        assert!(lc.merges.is_empty());
    }

    #[test]
    fn align_serializes_lowercase() {
        let cell = LoweredCell {
            col: 0,
            text: "x".into(),
            align: Align::Right,
            style_key: 0,
        };
        let json = serde_json::to_string(&cell).unwrap();
        // camelCase keys + lowercase align variant + the additive styleKey.
        assert_eq!(json, r#"{"col":0,"text":"x","align":"right","styleKey":0}"#);
    }

    #[test]
    fn sheet_lower_ir_styles_table() {
        // IR v2 (M1 style-map track): the content carries a styles table and
        // every cell carries a style_key. T0 emits the single default entry
        // (key 0) and style_key 0 everywhere.
        let (m, s) = model_with(&[(0, 0, num(1.0))]);
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 0,
            },
            &ViewOptions::default(),
        );
        assert_eq!(lc.styles, vec![LoweredStyle::default_key0()]);
        assert_eq!(lc.styles[0].key, 0);
        assert_eq!(lc.rows[0].cells[0].style_key, 0);
        // The wire shape carries camelCase `styleKey` and a `styles` array.
        let json = serde_json::to_string(&lc).unwrap();
        assert!(json.contains("\"styleKey\":0"));
        assert!(json.contains("\"styles\":["));
        assert!(!json.contains("style_key"));
        assert!(!json.contains("font_size_pt"));
    }

    #[test]
    fn camelcase_keys() {
        let (m, s) = model_with(&[(0, 0, num(1.0))]);
        let lc = lower_range(
            &m,
            s,
            CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 0,
            },
            &ViewOptions::default(),
        );
        let json = serde_json::to_string(&lc).unwrap();
        assert!(json.contains("\"widthPt\""));
        assert!(json.contains("\"heightPt\""));
        // No snake_case leaks.
        assert!(!json.contains("width_pt"));
        assert!(!json.contains("height_pt"));
    }

    #[test]
    fn deterministic_two_runs() {
        let (m, s) = model_with(&[(0, 0, num(1.0)), (1, 1, text("z")), (0, 1, num(2.5))]);
        let range = CellRange {
            r0: 0,
            c0: 0,
            r1: 1,
            c1: 1,
        };
        let a = lower_range(&m, s, range, &ViewOptions::default());
        let b = lower_range(&m, s, range, &ViewOptions::default());
        assert_eq!(a, b);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}
