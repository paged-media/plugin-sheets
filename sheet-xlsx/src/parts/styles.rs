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

//! `styles.xml` — the workbook style sheet (ECMA-376 §18.8).
//!
//! Two products come out of this part:
//!
//! 1. The interned [`StyleTable`] (`numFmt` resolution + the opaque
//!    `font`/`fill`/`border` `u32` slots, frozen `CellStyle` shape). A
//!    cell's `s="N"` indexes `cellXfs`; entry N carries the `numFmtId` we
//!    resolve (custom > built-in) and the raw sub-table ids.
//! 2. The **visual** model ([`VisualStyles`], M1 style-map track, spec
//!    §8.3): the `<fonts>`/`<fills>`/`<borders>` sub-tables are now
//!    *interpreted* — bold/italic, point size, font name, fill/text colours
//!    (resolved to `#RRGGBB`), and per-edge border presence — keyed by the
//!    cell's resolved [`StyleId`] so the lowering can turn a cell into a
//!    [`sheet_lower`-shaped] `LoweredStyle` on BOTH surfaces (the page lower
//!    and the grid scene resolve the SAME entry — the cross-surface-parity
//!    row). The frozen `CellStyle` keeps `font`/`fill`/`border` as `u32`
//!    indices; we DO NOT change it — `VisualStyles` is a separate side table
//!    returned alongside (`XlsxDocument::visual_styles`).
//!
//! Round-trip fidelity is unchanged: re-emitting the original `styles.xml`
//! verbatim when undirtied keeps every byte; the visual model is read-only
//! derived state for lowering, never written back.
//!
//! ## Colour resolution (spec §8.3, documented rulings)
//!
//! - `rgb="AARRGGBB"` — the leading alpha byte is dropped → `#RRGGBB`.
//! - `indexed="N"` — the legacy 56-entry palette (ECMA-376 §18.8.27,
//!   "Color (Indexed Colors)"); we ship the standard table
//!   [`indexed_color`]. Indices 64/65 (system foreground/background) have no
//!   fixed RGB → resolved to `None` (fall through to the document default).
//! - `theme="N"` — the document theme palette is in `theme1.xml`, which T0
//!   does not parse; we map the SIX common slots Excel writes by convention
//!   ([`theme_color`], documented as a best-effort default) and leave the
//!   rest `None`. A real theme parse is later-tier (flagged in the registry).
//! - `auto="1"` / nothing → `None` (the document default colour wins).

use crate::error::XlsxError;
use crate::opc::attr;
use sheet_core::style::{CellStyle, StyleTable};
use sheet_core::StyleId;
use std::collections::BTreeMap;

/// The standard built-in number formats (ECMA-376 §18.8.30, Table). Ids not
/// listed (5–8, 23–36, 41–44, 50–58) are locale/currency-dependent and have
/// no canonical code in the spec table; we leave them to fall back to
/// "General" in T0 (real workbooks that use them carry an explicit custom
/// numFmt or rely on the consumer's locale — out of T0 format scope).
pub fn builtin_num_fmt(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// The legacy indexed colour palette (ECMA-376 §18.8.27). Returns the
/// `#RRGGBB` string for a palette index, or `None` for indices with no fixed
/// RGB (64 = system foreground, 65 = system background, anything past the
/// table). This is the standard default palette Excel ships; a workbook MAY
/// override it via `<indexedColors>`, which T0 does not parse (documented).
pub fn indexed_color(idx: u32) -> Option<&'static str> {
    // ECMA-376 Table — indices 0..=63. 0..=7 duplicate 8..=15 by spec.
    const PALETTE: &[&str] = &[
        "#000000", "#FFFFFF", "#FF0000", "#00FF00", "#0000FF", "#FFFF00", "#FF00FF", "#00FFFF",
        "#000000", "#FFFFFF", "#FF0000", "#00FF00", "#0000FF", "#FFFF00", "#FF00FF", "#00FFFF",
        "#800000", "#008000", "#000080", "#808000", "#800080", "#008080", "#C0C0C0", "#808080",
        "#9999FF", "#993366", "#FFFFCC", "#CCFFFF", "#660066", "#FF8080", "#0066CC", "#CCCCFF",
        "#000080", "#FF00FF", "#FFFF00", "#00FFFF", "#800080", "#800000", "#008080", "#0000FF",
        "#00CCFF", "#CCFFFF", "#CCFFCC", "#FFFF99", "#99CCFF", "#FF99CC", "#CC99FF", "#FFCC99",
        "#3366FF", "#33CCCC", "#99CC00", "#FFCC00", "#FF9900", "#FF6600", "#666699", "#969696",
        "#003366", "#339966", "#003300", "#333300", "#993300", "#993366", "#333399", "#333333",
    ];
    PALETTE.get(idx as usize).copied()
}

/// Best-effort theme-colour resolution (spec §8.3 documented fallback). The
/// real palette lives in `xl/theme/theme1.xml`, which T0 does not parse; we
/// map the six slots Excel writes by convention (the Office default theme).
/// Slots beyond these (accent colours etc.) resolve to `None` so the
/// document default colour wins rather than guessing. A theme1.xml parse is
/// later-tier (registry: `sheet.style.xlsx-visual-parse` note).
pub fn theme_color(idx: u32) -> Option<&'static str> {
    Some(match idx {
        0 => "#FFFFFF", // dk1 / lt1 ordering varies; 0/1 are the text/bg pair
        1 => "#000000",
        2 => "#E7E6E6", // lt2 (light background 2)
        3 => "#44546A", // dk2 (dark text 2)
        4 => "#4472C4", // accent1 (the Office default blue)
        5 => "#ED7D31", // accent2 (orange)
        _ => return None,
    })
}

/// One cell's interpreted visual style (spec §8.3). All fields are
/// already-resolved, host-ready: colours are `#RRGGBB`, sizes are points.
/// `None`/`false` mean "no explicit attribute" — the document default wins.
/// This mirrors `sheet_lower::LoweredStyle` field-for-field (minus the wire
/// `key`) so the lowering maps one to the other trivially.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VisualStyle {
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

impl VisualStyle {
    /// True if this style carries NO visual override (every field default) —
    /// the lowering folds these onto the default style key 0.
    pub fn is_default(&self) -> bool {
        *self == VisualStyle::default()
    }
}

/// The visual style side table (M1 style-map track): a cell's resolved
/// [`StyleId`] -> its [`VisualStyle`]. Returned alongside the `StyleTable`
/// (NOT folded into the frozen `CellStyle`); the lowering reads it to build
/// the IR-v2 styles table. A `StyleId` absent from the map (or mapping to a
/// default `VisualStyle`) carries no visual styling.
#[derive(Clone, Debug, Default)]
pub struct VisualStyles {
    by_style: BTreeMap<u32, VisualStyle>,
}

impl VisualStyles {
    /// The visual style for a resolved `StyleId`, or `None` (the default).
    pub fn get(&self, id: StyleId) -> Option<&VisualStyle> {
        self.by_style.get(&id.0).filter(|v| !v.is_default())
    }

    /// Record a style id -> visual style (skips pure-default entries so
    /// `get` stays a "has visual styling?" test).
    fn insert(&mut self, id: StyleId, vs: VisualStyle) {
        if !vs.is_default() {
            self.by_style.insert(id.0, vs);
        }
    }

    /// True if NO cell carries visual styling (e.g. an unstyled workbook).
    pub fn is_empty(&self) -> bool {
        self.by_style.is_empty()
    }
}

/// One [`VisualStyle`] → the dialect-neutral `sheet_lower::VisualAttrs` (the
/// resolver's input). Field-for-field; this is the only place the two mirror
/// types meet (see the crate `Cargo.toml` note on the `sheet-lower` dep).
impl VisualStyle {
    /// Public alias of [`to_attrs`](Self::to_attrs) for callers that resolve a
    /// `<dxf>` override into the lowering's [`sheet_lower::VisualAttrs`] (the
    /// conditional-formatting bridge, `conditional_format::to_lower`).
    pub fn to_lower_attrs(&self) -> sheet_lower::VisualAttrs {
        self.to_attrs()
    }

    fn to_attrs(&self) -> sheet_lower::VisualAttrs {
        sheet_lower::VisualAttrs {
            bold: self.bold,
            italic: self.italic,
            font_size_pt: self.font_size_pt,
            font_name: self.font_name.clone(),
            fill_rgb: self.fill_rgb.clone(),
            text_rgb: self.text_rgb.clone(),
            border_top: self.border_top,
            border_right: self.border_right,
            border_bottom: self.border_bottom,
            border_left: self.border_left,
        }
    }
}

/// Make [`VisualStyles`] a [`sheet_lower::VisualStyleSource`] so the page
/// lowering (`lower_range_styled`) and the grid scene (`grid_scene_styled`)
/// resolve the parsed visual styles through ONE trait — a cell's `StyleId`
/// yields an IDENTICAL `LoweredStyle` on both surfaces (spec §8.3
/// cross-surface parity). A `StyleId` with no visual styling resolves to
/// `None` (folds onto the default key 0).
impl sheet_lower::VisualStyleSource for VisualStyles {
    fn visual(&self, id: StyleId) -> Option<sheet_lower::VisualAttrs> {
        self.get(id).map(VisualStyle::to_attrs)
    }
}

/// The parsed style sheet, ready to fold into the model's `StyleTable`.
pub struct ParsedStyles {
    /// `StyleId` per `cellXfs` index N (so `s="N"` -> styles[N]).
    pub xf_to_style: Vec<StyleId>,
    /// The interpreted visual model (M1 style-map track), keyed by the
    /// resolved `StyleId`. Empty for an unstyled workbook.
    pub visual: VisualStyles,
    /// The differential-format table (`<dxfs>`, ECMA-376 §18.8.15), indexed
    /// by `dxfId` (the index a `cfRule dxfId=` references; M2 conditional-
    /// formatting track, spec §10.4). Each entry is the OVERRIDE a matched
    /// conditional-format rule applies on top of the cell's base style —
    /// bold/italic + an explicit font/fill colour. Empty when the workbook
    /// has no conditional formatting (or no `<dxfs>`).
    pub dxfs: Vec<VisualStyle>,
}

/// Parse `styles.xml`, interning each `cellXfs` entry into `styles` AND
/// building the [`VisualStyles`] side table from the font/fill/border
/// sub-tables.
pub fn parse(xml: &[u8], styles: &mut StyleTable) -> Result<ParsedStyles, XlsxError> {
    let parsed = parse_raw(xml)?;

    let mut xf_to_style = Vec::with_capacity(parsed.xfs.len());
    let mut visual = VisualStyles::default();
    for xf in &parsed.xfs {
        // Custom code wins (ids >= 164 are always custom); else built-in.
        let code: String = parsed
            .custom_fmts
            .get(&xf.num_fmt_id)
            .cloned()
            .or_else(|| builtin_num_fmt(xf.num_fmt_id).map(str::to_owned))
            .unwrap_or_else(|| "General".to_owned());
        let num_fmt = styles.intern_num_fmt(&code);
        let style = CellStyle {
            num_fmt,
            font: xf.font_id,
            fill: xf.fill_id,
            border: xf.border_id,
            align: sheet_core::Align::General,
        };
        let id = styles.intern_style(style);
        xf_to_style.push(id);

        // The visual interpretation for this xf, resolved through the
        // sub-tables. `applyFont`/`applyFill`/`applyBorder` (when present and
        // "0") suppress the corresponding facet — Excel's "this xf does NOT
        // override the font/fill/border" flag (ECMA-376 §18.8.45).
        let vs = visual_of(&parsed, xf);
        visual.insert(id, vs);
    }

    // The differential-format (`<dxfs>`) table for conditional formatting. A
    // `<dxf>` is NOT a base style: each facet PRESENT is an override (no
    // `apply*` flags, no default-font folding — `<dxf>` only ever carries the
    // attributes the rule changes). Resolved into the same host-ready
    // `VisualStyle` shape so the lowering layers it on the base style.
    let dxfs = parsed.dxfs.iter().map(dxf_visual).collect();

    Ok(ParsedStyles {
        xf_to_style,
        visual,
        dxfs,
    })
}

/// Build a [`VisualStyle`] OVERRIDE from one parsed `<dxf>`. Unlike a base
/// `cellXfs` entry, every facet PRESENT in the `<dxf>` is an override the rule
/// applies (a `<dxf>` carries only the changed attributes); there is no
/// default-font folding or `apply*` suppression (ECMA-376 §18.8.14, `dxf`).
fn dxf_visual(d: &DxfRec) -> VisualStyle {
    VisualStyle {
        bold: d.font.as_ref().map(|f| f.bold).unwrap_or(false),
        italic: d.font.as_ref().map(|f| f.italic).unwrap_or(false),
        // A `<dxf>` font rarely restates size/name (it overrides emphasis +
        // colour); carry them when present so an explicit size/name applies.
        font_size_pt: d.font.as_ref().and_then(|f| f.size_pt),
        font_name: d.font.as_ref().and_then(|f| f.name.clone()),
        fill_rgb: d.fill.as_ref().and_then(|f| f.fg_rgb.clone()),
        text_rgb: d.font.as_ref().and_then(|f| f.color.clone()),
        border_top: d.border.as_ref().map(|b| b.top).unwrap_or(false),
        border_right: d.border.as_ref().map(|b| b.right).unwrap_or(false),
        border_bottom: d.border.as_ref().map(|b| b.bottom).unwrap_or(false),
        border_left: d.border.as_ref().map(|b| b.left).unwrap_or(false),
    }
}

/// Build a [`VisualStyle`] for one `cellXfs` entry by resolving its
/// font/fill/border ids against the parsed sub-tables, honouring the
/// `apply*` suppression flags.
///
/// ## The default-font ruling (spec §8.3 — constrained local overrides)
///
/// Font index 0 in the `<fonts>` table is the WORKBOOK DEFAULT font (the
/// document base; ECMA-376 §18.8.23). Its size + name are the document
/// default, NOT a per-cell override — capturing them on every cell would
/// "silently splatter ten thousand local overrides" (spec §8.3), exactly
/// what the publishing principle forbids. So we only record a cell's font
/// SIZE/NAME when it DIFFERS from the default font; bold/italic and an
/// explicit text colour are always overrides (the default font is
/// non-bold, non-italic, and carries no explicit colour by convention).
fn visual_of(parsed: &RawStyles, xf: &XfRow) -> VisualStyle {
    let mut vs = VisualStyle::default();
    let default_font = parsed.fonts.first();

    // Font (bold/italic/size/name/colour). `applyFont="0"` suppresses it.
    if xf.apply_font.unwrap_or(true) {
        if let Some(f) = parsed.fonts.get(xf.font_id as usize) {
            vs.bold = f.bold;
            vs.italic = f.italic;
            // Size/name only when they differ from the document default font
            // (otherwise they are the default, not an override).
            if f.size_pt != default_font.and_then(|d| d.size_pt) {
                vs.font_size_pt = f.size_pt;
            }
            if f.name != default_font.and_then(|d| d.name.clone()) {
                vs.font_name = f.name.clone();
            }
            vs.text_rgb = f.color.clone();
        }
    }

    // Fill (the solid pattern foreground colour). `applyFill="0"` suppresses.
    if xf.apply_fill.unwrap_or(true) {
        if let Some(fill) = parsed.fills.get(xf.fill_id as usize) {
            vs.fill_rgb = fill.fg_rgb.clone();
        }
    }

    // Border (per-edge presence). `applyBorder="0"` suppresses.
    if xf.apply_border.unwrap_or(true) {
        if let Some(b) = parsed.borders.get(xf.border_id as usize) {
            vs.border_top = b.top;
            vs.border_right = b.right;
            vs.border_bottom = b.bottom;
            vs.border_left = b.left;
        }
    }

    vs
}

/// The raw, index-keyed sub-tables parsed from `styles.xml` before the xf
/// fold. Kept private — `parse` is the public door.
struct RawStyles {
    custom_fmts: BTreeMap<u32, String>,
    fonts: Vec<FontRec>,
    fills: Vec<FillRec>,
    borders: Vec<BorderRec>,
    xfs: Vec<XfRow>,
    /// The `<dxfs>` differential formats, in `dxfId` order (M2 cond-fmt).
    dxfs: Vec<DxfRec>,
}

/// One `<dxf>` (a differential format, ECMA-376 §18.8.14): the font/fill/
/// border facets a conditional-format rule overrides. Each child is optional;
/// only the present facets are overrides. The fill colour comes from
/// `<patternFill><bgColor>` (the dxf convention), unlike a `cellXfs` solid
/// fill which uses `<fgColor>` — both are accepted here (bgColor wins).
#[derive(Default)]
struct DxfRec {
    font: Option<FontRec>,
    fill: Option<FillRec>,
    border: Option<BorderRec>,
}

/// One `<font>`: emphasis, point size, name, and resolved colour.
#[derive(Default)]
struct FontRec {
    bold: bool,
    italic: bool,
    size_pt: Option<f64>,
    name: Option<String>,
    color: Option<String>,
}

/// One `<fill>`: the solid pattern foreground colour resolved to `#RRGGBB`
/// (only `patternType="solid"` contributes a background; `none`/`gray125`
/// are layout placeholders, not a real fill).
#[derive(Default)]
struct FillRec {
    fg_rgb: Option<String>,
}

/// One `<border>`: per-edge presence (any non-empty `style` attribute marks
/// the edge present — T0 models presence, not the stroke weight/dash).
#[derive(Default)]
struct BorderRec {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

struct XfRow {
    num_fmt_id: u32,
    font_id: u32,
    fill_id: u32,
    border_id: u32,
    /// `applyFont`/`applyFill`/`applyBorder` (`Some(false)` = suppressed).
    apply_font: Option<bool>,
    apply_fill: Option<bool>,
    apply_border: Option<bool>,
}

/// Resolve a colour element's attributes (`rgb`/`indexed`/`theme`/`auto`) to
/// `#RRGGBB`, or `None` (the document default). See the module docs for the
/// per-attribute rulings.
fn resolve_color(e: &quick_xml::events::BytesStart<'_>) -> Result<Option<String>, XlsxError> {
    if let Some(rgb) = attr(e, b"rgb")? {
        return Ok(normalize_rgb(&rgb));
    }
    if let Some(indexed) = attr(e, b"indexed")? {
        if let Ok(i) = indexed.parse::<u32>() {
            return Ok(indexed_color(i).map(str::to_owned));
        }
    }
    if let Some(theme) = attr(e, b"theme")? {
        if let Ok(t) = theme.parse::<u32>() {
            return Ok(theme_color(t).map(str::to_owned));
        }
    }
    // `auto="1"` or an empty <color/> → no explicit colour.
    Ok(None)
}

/// Normalize an xlsx `rgb` attribute (`AARRGGBB` or `RRGGBB`) to `#RRGGBB`.
/// Drops the leading alpha byte; returns `None` if not a valid hex colour.
fn normalize_rgb(rgb: &str) -> Option<String> {
    let hex = rgb.trim();
    let body = match hex.len() {
        8 => &hex[2..], // strip the AA alpha byte
        6 => hex,
        _ => return None,
    };
    if body.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", body.to_ascii_uppercase()))
    } else {
        None
    }
}

/// The pure XML walk: bytes -> [`RawStyles`]. Streamed with quick-xml so a
/// nested `<patternFill><fgColor/></patternFill>` and the empty-tag
/// `<b/>`/`<i/>` forms are both handled.
fn parse_raw(xml: &[u8]) -> Result<RawStyles, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    let mut custom_fmts: BTreeMap<u32, String> = BTreeMap::new();
    let mut fonts: Vec<FontRec> = Vec::new();
    let mut fills: Vec<FillRec> = Vec::new();
    let mut borders: Vec<BorderRec> = Vec::new();
    let mut xfs: Vec<XfRow> = Vec::new();
    let mut dxfs: Vec<DxfRec> = Vec::new();

    // Section flags. `<xf>` appears in both `<cellXfs>` and `<cellStyleXfs>`;
    // we only model the former. `<color>` appears inside fonts AND borders, so
    // we track which sub-table we are inside to route it correctly. `<font>`/
    // `<fill>`/`<border>` ALSO appear inside `<dxfs>` -> `<dxf>`; `in_dxfs`
    // routes a completed record into the current dxf instead of the global
    // table (M2 conditional-formatting track).
    let mut in_cell_xfs = false;
    let mut in_fonts = false;
    let mut in_fills = false;
    let mut in_borders = false;
    let mut in_dxfs = false;
    // The dxf currently being built (Some inside a `<dxf>`).
    let mut cur_dxf: Option<DxfRec> = None;
    // The font/fill/border currently being built (None when between records).
    let mut cur_font: Option<FontRec> = None;
    let mut cur_fill: Option<FillRec> = None;
    let mut cur_border: Option<BorderRec> = None;
    // Inside a `<fill>`, only a `solid` patternFill contributes a colour.
    let mut fill_is_solid = false;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"numFmt" => read_num_fmt(&e, &mut custom_fmts)?,
                    b"fonts" => in_fonts = true,
                    b"fills" => in_fills = true,
                    b"borders" => in_borders = true,
                    b"cellXfs" => in_cell_xfs = true,
                    b"dxfs" => in_dxfs = true,
                    b"dxf" if in_dxfs => cur_dxf = Some(DxfRec::default()),
                    // A `<dxf>` child font/fill/border starts a fresh record
                    // (routed into `cur_dxf` on its End). Inside a dxf, a fill
                    // with NO patternType still carries a colour via <bgColor>
                    // (the dxf convention), so default `fill_is_solid` true.
                    b"font" if in_fonts || cur_dxf.is_some() => cur_font = Some(FontRec::default()),
                    b"fill" if in_fills || cur_dxf.is_some() => {
                        cur_fill = Some(FillRec::default());
                        fill_is_solid = !in_fills; // dxf fills colour without "solid"
                    }
                    b"border" if in_borders || cur_dxf.is_some() => {
                        cur_border = Some(BorderRec::default())
                    }
                    b"patternFill" if cur_fill.is_some() => {
                        // A dxf patternFill MAY omit patternType but still set a
                        // bgColor; keep "solid-or-dxf" truthy so the colour is read.
                        let solid = attr(&e, b"patternType")?.as_deref() == Some("solid");
                        fill_is_solid = solid || cur_dxf.is_some();
                    }
                    // `<color>` with children is rare but legal; treat as empty.
                    b"color" if cur_font.is_some() => {
                        if let Some(f) = cur_font.as_mut() {
                            f.color = resolve_color(&e)?;
                        }
                    }
                    b"xf" if in_cell_xfs => xfs.push(read_xf(&e)?),
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"numFmt" => read_num_fmt(&e, &mut custom_fmts)?,
                    // Font facets (empty-tag forms: <b/> <i/> <sz/> <name/>).
                    b"b" if cur_font.is_some() => set_font_flag(&mut cur_font, &e, true, false)?,
                    b"i" if cur_font.is_some() => set_font_flag(&mut cur_font, &e, false, true)?,
                    b"sz" if cur_font.is_some() => {
                        if let Some(f) = cur_font.as_mut() {
                            f.size_pt = attr(&e, b"val")?.and_then(|s| s.parse::<f64>().ok());
                        }
                    }
                    b"name" if cur_font.is_some() => {
                        if let Some(f) = cur_font.as_mut() {
                            f.name = attr(&e, b"val")?;
                        }
                    }
                    b"color" if cur_font.is_some() => {
                        if let Some(f) = cur_font.as_mut() {
                            f.color = resolve_color(&e)?;
                        }
                    }
                    // Fill: an empty solid patternFill has no fgColor child.
                    b"patternFill" if cur_fill.is_some() => {
                        let solid = attr(&e, b"patternType")?.as_deref() == Some("solid");
                        fill_is_solid = solid || cur_dxf.is_some();
                    }
                    b"fgColor" if cur_fill.is_some() && fill_is_solid => {
                        if let Some(fill) = cur_fill.as_mut() {
                            fill.fg_rgb = resolve_color(&e)?;
                        }
                    }
                    // A dxf solid fill carries its colour in <bgColor> (the
                    // dxf convention; ECMA-376 §18.8.20). Only read it inside a
                    // dxf and only if a fgColor has not already set the colour.
                    b"bgColor" if cur_fill.is_some() && cur_dxf.is_some() => {
                        if let Some(fill) = cur_fill.as_mut() {
                            if fill.fg_rgb.is_none() {
                                fill.fg_rgb = resolve_color(&e)?;
                            }
                        }
                    }
                    // Border edges (empty-tag forms: <top/> <left/> with an
                    // optional style attr; presence = a non-empty `style`).
                    b"top" if cur_border.is_some() => {
                        set_border_edge(&mut cur_border, &e, Edge::Top)?
                    }
                    b"right" if cur_border.is_some() => {
                        set_border_edge(&mut cur_border, &e, Edge::Right)?
                    }
                    b"bottom" if cur_border.is_some() => {
                        set_border_edge(&mut cur_border, &e, Edge::Bottom)?
                    }
                    b"left" if cur_border.is_some() => {
                        set_border_edge(&mut cur_border, &e, Edge::Left)?
                    }
                    b"xf" if in_cell_xfs => xfs.push(read_xf(&e)?),
                    _ => {}
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"fonts" => in_fonts = false,
                b"fills" => in_fills = false,
                b"borders" => in_borders = false,
                b"cellXfs" => in_cell_xfs = false,
                b"dxfs" => in_dxfs = false,
                b"dxf" => {
                    if let Some(d) = cur_dxf.take() {
                        dxfs.push(d);
                    }
                }
                // A completed font/fill/border routes into the current dxf when
                // inside one, else into the global sub-table.
                b"font" => {
                    if let Some(f) = cur_font.take() {
                        match cur_dxf.as_mut() {
                            Some(d) => d.font = Some(f),
                            None => fonts.push(f),
                        }
                    }
                }
                b"fill" => {
                    if let Some(f) = cur_fill.take() {
                        match cur_dxf.as_mut() {
                            Some(d) => d.fill = Some(f),
                            None => fills.push(f),
                        }
                    }
                }
                b"border" => {
                    if let Some(b) = cur_border.take() {
                        match cur_dxf.as_mut() {
                            Some(d) => d.border = Some(b),
                            None => borders.push(b),
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(RawStyles {
        custom_fmts,
        fonts,
        fills,
        borders,
        xfs,
        dxfs,
    })
}

/// Which border edge a `<top>`/`<right>`/`<bottom>`/`<left>` element sets.
enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// Mark a border edge present if it carries a non-empty `style` attribute
/// (e.g. `style="thin"`). An empty `<top/>` (no style) is "no border".
fn set_border_edge(
    cur: &mut Option<BorderRec>,
    e: &quick_xml::events::BytesStart<'_>,
    edge: Edge,
) -> Result<(), XlsxError> {
    let present = attr(e, b"style")?.is_some_and(|s| !s.is_empty());
    if let Some(b) = cur.as_mut() {
        match edge {
            Edge::Top => b.top = present,
            Edge::Right => b.right = present,
            Edge::Bottom => b.bottom = present,
            Edge::Left => b.left = present,
        }
    }
    Ok(())
}

/// Mark bold/italic on the current font (the `<b/>`/`<i/>` flag elements;
/// a `val="0"` explicitly turns the flag OFF, ECMA-376 boolean convention).
fn set_font_flag(
    cur: &mut Option<FontRec>,
    e: &quick_xml::events::BytesStart<'_>,
    sets_bold: bool,
    sets_italic: bool,
) -> Result<(), XlsxError> {
    // Absent `val` means true; `val="0"`/`"false"` means false.
    let on = match attr(e, b"val")? {
        None => true,
        Some(v) => !matches!(v.as_str(), "0" | "false"),
    };
    if let Some(f) = cur.as_mut() {
        if sets_bold {
            f.bold = on;
        }
        if sets_italic {
            f.italic = on;
        }
    }
    Ok(())
}

/// Read a `<numFmt numFmtId formatCode>` into the custom-format map.
fn read_num_fmt(
    e: &quick_xml::events::BytesStart<'_>,
    custom_fmts: &mut BTreeMap<u32, String>,
) -> Result<(), XlsxError> {
    let id = attr(e, b"numFmtId")?.and_then(|s| s.parse::<u32>().ok());
    let code = attr(e, b"formatCode")?;
    if let (Some(id), Some(code)) = (id, code) {
        custom_fmts.insert(id, code);
    }
    Ok(())
}

/// Read one `<xf>` (in `<cellXfs>`): the sub-table ids + the `apply*` flags.
fn read_xf(e: &quick_xml::events::BytesStart<'_>) -> Result<XfRow, XlsxError> {
    let u32_attr = |key: &[u8]| -> Result<u32, XlsxError> {
        Ok(attr(e, key)?
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0))
    };
    let bool_attr = |key: &[u8]| -> Result<Option<bool>, XlsxError> {
        Ok(attr(e, key)?.map(|v| !matches!(v.as_str(), "0" | "false")))
    };
    Ok(XfRow {
        num_fmt_id: u32_attr(b"numFmtId")?,
        font_id: u32_attr(b"fontId")?,
        fill_id: u32_attr(b"fillId")?,
        border_id: u32_attr(b"borderId")?,
        apply_font: bool_attr(b"applyFont")?,
        apply_fill: bool_attr(b"applyFill")?,
        apply_border: bool_attr(b"applyBorder")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_spot_checks() {
        assert_eq!(builtin_num_fmt(0), Some("General"));
        assert_eq!(builtin_num_fmt(2), Some("0.00"));
        assert_eq!(builtin_num_fmt(9), Some("0%"));
        assert_eq!(builtin_num_fmt(14), Some("mm-dd-yy"));
        assert_eq!(builtin_num_fmt(49), Some("@"));
        // locale/currency ids have no canonical code in T0
        assert_eq!(builtin_num_fmt(5), None);
        assert_eq!(builtin_num_fmt(164), None);
    }

    #[test]
    fn indexed_and_theme_palettes() {
        assert_eq!(indexed_color(2), Some("#FF0000")); // red
        assert_eq!(indexed_color(5), Some("#FFFF00")); // yellow
        assert_eq!(indexed_color(64), None); // system foreground — no fixed RGB
        assert_eq!(theme_color(1), Some("#000000"));
        assert_eq!(theme_color(99), None); // beyond the mapped slots
    }

    #[test]
    fn normalize_rgb_drops_alpha() {
        assert_eq!(normalize_rgb("FFFFFF00").as_deref(), Some("#FFFF00"));
        assert_eq!(normalize_rgb("00FF00").as_deref(), Some("#00FF00"));
        assert_eq!(normalize_rgb("ffff0000").as_deref(), Some("#FF0000"));
        assert_eq!(normalize_rgb("nothex!!").as_deref(), None);
        assert_eq!(normalize_rgb("FFF").as_deref(), None);
    }

    #[test]
    fn parse_custom_and_builtin_cellxfs() {
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <numFmts count="1">
    <numFmt numFmtId="164" formatCode="&quot;$&quot;#,##0.00"/>
  </numFmts>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="3">
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
    <xf numFmtId="2" fontId="1" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>
    <xf numFmtId="164" fontId="0" fillId="2" borderId="1" xfId="0" applyNumberFormat="1"/>
  </cellXfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();
        assert_eq!(p.xf_to_style.len(), 3);
        // xf 0 -> General
        assert_eq!(st.num_fmt_of(p.xf_to_style[0]), "General");
        // xf 1 -> built-in 2
        assert_eq!(st.num_fmt_of(p.xf_to_style[1]), "0.00");
        // xf 2 -> custom 164, with raw font/fill/border slots captured
        assert_eq!(st.num_fmt_of(p.xf_to_style[2]), "\"$\"#,##0.00");
        let s2 = st.style(p.xf_to_style[2]);
        assert_eq!(s2.fill, 2);
        assert_eq!(s2.border, 1);
    }

    /// The full visual parse: fonts (bold/size/name), fills (solid fgColor),
    /// borders (per-edge thin), threaded through `cellXfs` into a
    /// `VisualStyles` keyed by the resolved `StyleId`. Mirrors the corpus
    /// fixture 03-styles.xlsx exactly.
    #[test]
    fn parse_visual_fonts_fills_borders() {
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <numFmts count="2">
    <numFmt numFmtId="164" formatCode="&quot;$&quot;#,##0.00"/>
    <numFmt numFmtId="165" formatCode="0.000%"/>
  </numFmts>
  <fonts count="3">
    <font><sz val="11"/><name val="Calibri"/></font>
    <font><b/><sz val="11"/><name val="Calibri"/></font>
    <font><sz val="18"/><name val="Cambria"/></font>
  </fonts>
  <fills count="3">
    <fill><patternFill patternType="none"/></fill>
    <fill><patternFill patternType="gray125"/></fill>
    <fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/></patternFill></fill>
  </fills>
  <borders count="2">
    <border><left/><right/><top/><bottom/><diagonal/></border>
    <border><left style="thin"/><right style="thin"/><top style="thin"/><bottom style="thin"/><diagonal/></border>
  </borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="6">
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
    <xf numFmtId="2" fontId="1" fillId="0" borderId="1" xfId="0"/>
    <xf numFmtId="164" fontId="0" fillId="2" borderId="0" xfId="0"/>
    <xf numFmtId="9" fontId="0" fillId="0" borderId="0" xfId="0"/>
    <xf numFmtId="165" fontId="0" fillId="0" borderId="0" xfId="0"/>
    <xf numFmtId="0" fontId="2" fillId="0" borderId="0" xfId="0"/>
  </cellXfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();

        // xf 0 — plain (default font, no emphasis/fill/border) → no entry.
        // The default font (index 0)'s size/name are the document default,
        // NOT a per-cell override (the §8.3 minimal-override ruling).
        assert!(p.visual.get(p.xf_to_style[0]).is_none());

        // xf 1 — bold + thin border all sides, no fill. The font shares the
        // DEFAULT size/name (Calibri 11), so only `bold` is recorded — the
        // size/name fold to the document default (minimal override).
        let v1 = p
            .visual
            .get(p.xf_to_style[1])
            .expect("xf1 has visual style");
        assert!(v1.bold);
        assert!(!v1.italic);
        assert_eq!(v1.font_size_pt, None, "default size is not an override");
        assert_eq!(v1.font_name, None, "default name is not an override");
        assert!(v1.border_top && v1.border_right && v1.border_bottom && v1.border_left);
        assert_eq!(v1.fill_rgb, None);

        // xf 2 — yellow solid fill (FFFFFF00 -> #FFFF00), no border, no bold.
        let v2 = p
            .visual
            .get(p.xf_to_style[2])
            .expect("xf2 has visual style");
        assert_eq!(v2.fill_rgb.as_deref(), Some("#FFFF00"));
        assert!(!v2.bold);
        assert!(!v2.border_top);

        // xf 5 — font 2 is a genuinely DISTINCT face (Cambria 18) → its
        // size + name ARE overrides (they differ from the default font).
        let v5 = p
            .visual
            .get(p.xf_to_style[5])
            .expect("xf5 has visual style");
        assert_eq!(v5.font_size_pt, Some(18.0));
        assert_eq!(v5.font_name.as_deref(), Some("Cambria"));
        assert!(!v5.bold);

        // xf 3/4 — only a number format differs; visually default → no entry.
        assert!(p.visual.get(p.xf_to_style[3]).is_none());
        assert!(p.visual.get(p.xf_to_style[4]).is_none());
    }

    #[test]
    fn apply_font_zero_suppresses_emphasis() {
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><b/><sz val="14"/><name val="Arial"/></font></fonts>
  <fills count="1"><fill><patternFill patternType="none"/></fill></fills>
  <borders count="1"><border/></borders>
  <cellXfs count="1">
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0" applyFont="0"/>
  </cellXfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();
        // applyFont="0" suppresses the font facet → visually default.
        assert!(p.visual.get(p.xf_to_style[0]).is_none());
    }

    #[test]
    fn parse_dxfs_for_conditional_formatting() {
        // `<dxfs>` is the differential-format table a cfRule dxfId references.
        // dxf 0: bold + red text + yellow fill (bgColor, the dxf convention).
        // dxf 1: italic only. Each PRESENT facet is an override (no apply* /
        // default-font folding — a dxf carries only what the rule changes).
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="1"><fill><patternFill patternType="none"/></fill></fills>
  <borders count="1"><border/></borders>
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs>
  <dxfs count="2">
    <dxf>
      <font><b/><color rgb="FFFF0000"/></font>
      <fill><patternFill><bgColor rgb="FFFFFF00"/></patternFill></fill>
    </dxf>
    <dxf>
      <font><i/></font>
    </dxf>
  </dxfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();
        assert_eq!(p.dxfs.len(), 2);

        // dxf 0 — bold + red text + yellow fill, nothing else.
        let d0 = &p.dxfs[0];
        assert!(d0.bold);
        assert!(!d0.italic);
        assert_eq!(d0.text_rgb.as_deref(), Some("#FF0000"));
        assert_eq!(d0.fill_rgb.as_deref(), Some("#FFFF00"));
        assert_eq!(d0.font_size_pt, None);
        assert!(!d0.border_top);

        // dxf 1 — italic only.
        let d1 = &p.dxfs[1];
        assert!(d1.italic);
        assert!(!d1.bold);
        assert_eq!(d1.fill_rgb, None);
        assert_eq!(d1.text_rgb, None);
    }

    #[test]
    fn dxf_solid_fill_fgcolor_also_accepted() {
        // Some writers use <fgColor> in a dxf fill; accept it too.
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs>
  <dxfs count="1">
    <dxf><fill><patternFill patternType="solid"><fgColor rgb="FF00FF00"/></patternFill></fill></dxf>
  </dxfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();
        assert_eq!(p.dxfs.len(), 1);
        assert_eq!(p.dxfs[0].fill_rgb.as_deref(), Some("#00FF00"));
    }

    #[test]
    fn no_dxfs_yields_empty_table() {
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();
        assert!(p.dxfs.is_empty());
    }

    #[test]
    fn indexed_and_theme_colors_in_fill_and_font() {
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><color indexed="2"/><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="2">
    <fill><patternFill patternType="none"/></fill>
    <fill><patternFill patternType="solid"><fgColor theme="4"/></patternFill></fill>
  </fills>
  <borders count="1"><border/></borders>
  <cellXfs count="1">
    <xf numFmtId="0" fontId="0" fillId="1" borderId="0"/>
  </cellXfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();
        let v = p.visual.get(p.xf_to_style[0]).expect("colours present");
        assert_eq!(v.text_rgb.as_deref(), Some("#FF0000")); // indexed 2 = red
        assert_eq!(v.fill_rgb.as_deref(), Some("#4472C4")); // theme 4 = accent1
    }
}
