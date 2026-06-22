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

//! `<conditionalFormatting>` — conditional-format rules on a worksheet
//! (ECMA-376 §18.3.1.18 `conditionalFormatting`, §18.3.1.10 `cfRule`; M2
//! conditional-formatting track, spec §10.4).
//!
//! A worksheet carries zero or more `<conditionalFormatting sqref="A1:B10">`
//! blocks, each holding one or more `<cfRule>`. A rule has a `type`, a
//! `priority` (lower = higher precedence; ties broken by document order), and
//! — for the kinds that paint a differential format — a `dxfId` into the
//! workbook `<dxfs>` table (parsed in `styles.rs`). This module parses those
//! blocks into the [`SheetConditionalFormats`] model; the lowering
//! (`sheet_lower::condfmt`) evaluates active rules against the already-computed
//! cell values and folds the matched override into the lowered style.
//!
//! ## What we read (the T2 honest subset, spec §10.4)
//!
//! - **`cellIs`** — a cell-value comparison (`operator` greaterThan / lessThan
//!   / equal / between / …) against one or two `<formula>` operands. T2
//!   evaluates the operands as numeric literals (the publishing-relevant
//!   common case); a non-numeric operand makes the rule inert (documented).
//! - **`expression`** — a free-form `<formula>` predicate. T2 lowers ONLY the
//!   simple comparison forms (`A1>5`, `$A1<=10`, …) that reduce to a
//!   value-vs-literal test; an arbitrary formula is DEFERRED (recorded as
//!   inert here, [`CfRuleKind::ExpressionUnsupported`]) — lowering never calls
//!   the calc engine (`sheet-lower` is pure, no `sheet-calc` dep).
//! - **`colorScale`** — a 2- or 3-color scale; we read the `<cfvo>` value
//!   objects + `<color>` stops. The interpolated per-cell fill is computed in
//!   lowering across the range's value domain.
//! - **`dataBar`** — read the min/max `<cfvo>` + bar `<color>`; lowering emits
//!   a drawn rect proportional to the value (handled by the lower track).
//! - **`iconSet`** — PRESERVED only (T2 floor): we record the rule exists so a
//!   future tier can render it, but it carries no override (no icon asset
//!   pipeline in T2). Round-trips via the worksheet's verbatim capture.
//!
//! ## Round-trip
//!
//! Parsing here is ADDITIVE: the worksheet still captures the
//! `<conditionalFormatting>` subtree verbatim (preserve.rs), so the XML
//! re-emits byte-identical on round-trip whether or not this model is used.
//! This module never writes back — it is read-only derived state for lowering
//! (the same discipline as `VisualStyles`).

use crate::error::XlsxError;
use crate::opc::attr;
use crate::parts::styles::{indexed_color, theme_color, VisualStyle};
use sheet_core::parse_a1;

/// The comparison operator of a `cellIs` rule (ECMA-376 §18.18.15
/// ST_ConditionalFormattingOperator, the value-comparison subset). The two
/// range operators (`between`/`notBetween`) take two operands; the rest take
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfOperator {
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Equal,
    NotEqual,
    Between,
    NotBetween,
}

impl CfOperator {
    /// Parse the `operator=` attribute token, or `None` for an unknown /
    /// text-only operator (`containsText` etc. are out of the T2 subset).
    pub fn parse(s: &str) -> Option<CfOperator> {
        Some(match s {
            "greaterThan" => CfOperator::GreaterThan,
            "greaterThanOrEqual" => CfOperator::GreaterThanOrEqual,
            "lessThan" => CfOperator::LessThan,
            "lessThanOrEqual" => CfOperator::LessThanOrEqual,
            "equal" => CfOperator::Equal,
            "notEqual" => CfOperator::NotEqual,
            "between" => CfOperator::Between,
            "notBetween" => CfOperator::NotBetween,
            _ => return None,
        })
    }

    /// True for the two-operand range operators.
    pub fn is_range(self) -> bool {
        matches!(self, CfOperator::Between | CfOperator::NotBetween)
    }
}

/// One value object (`<cfvo>`) of a color-scale / data-bar: how to derive a
/// domain endpoint (ECMA-376 §18.3.1.11 `cfvo`). T2 supports the numeric
/// endpoints that publishing scales use; `percent`/`percentile`/`formula` are
/// read but lowered as min/max fallbacks (documented in the lower track).
#[derive(Clone, Debug, PartialEq)]
pub struct CfValueObject {
    /// The `type` token (`min`, `max`, `num`, `percent`, `percentile`,
    /// `formula`). Kept as a string so the lower track can decide policy.
    pub kind: String,
    /// The `val` attribute parsed as a number, when present + numeric.
    pub val: Option<f64>,
}

/// A color-scale rule's stops: 2 or 3 `<cfvo>` + matching `<color>` (`#RRGGBB`).
#[derive(Clone, Debug, PartialEq)]
pub struct ColorScale {
    /// The value objects (domain endpoints), in order (low → high).
    pub cfvos: Vec<CfValueObject>,
    /// The colour at each stop, INDEX-ALIGNED with `cfvos` (one entry per
    /// `<color>` element). `Some("#RRGGBB")` is a resolved colour; `None` is a
    /// stop whose colour could not be resolved to a fixed RGB (an unmapped
    /// `theme`/`indexed`, an `auto`/empty colour) — kept as a placeholder so
    /// the alignment with `cfvos` is preserved (FINDING 2). The lowering maps
    /// a `None` to a documented default.
    pub colors: Vec<Option<String>>,
}

/// A data-bar rule: min/max value objects + the bar colour.
#[derive(Clone, Debug, PartialEq)]
pub struct DataBar {
    pub cfvos: Vec<CfValueObject>,
    /// The bar fill colour (`#RRGGBB`), or `None` (the document default blue).
    pub color: Option<String>,
}

/// The interpreted kind of one `cfRule` (the T2 subset; everything else is a
/// preserve-only floor recorded as [`CfRuleKind::Preserved`]).
#[derive(Clone, Debug, PartialEq)]
pub enum CfRuleKind {
    /// `cellIs` — a value comparison; the operands are numeric literals.
    CellIs {
        op: CfOperator,
        /// One operand for a comparison; two for between/notBetween.
        operands: Vec<f64>,
    },
    /// `expression` reduced to a simple `cell <op> literal` test (the only
    /// expression form T2 lowers without a formula evaluator).
    Expression { op: CfOperator, operand: f64 },
    /// `expression` we could NOT reduce to a simple comparison — DEFERRED
    /// (lowering never calls the calc engine). Carried so it round-trips and a
    /// later tier can implement it; it applies no override in T2.
    ExpressionUnsupported,
    /// `colorScale` — interpolated per-cell fill (computed in lowering).
    ColorScale(ColorScale),
    /// `dataBar` — a drawn proportional rect (computed in the lower track).
    DataBar(DataBar),
    /// `iconSet` — preserve-only floor (parsed, not rendered in T2).
    IconSet,
    /// Any other rule type (`top10`, `containsText`, `duplicateValues`, …):
    /// preserve-only; no override in T2.
    Preserved,
}

/// One parsed `<cfRule>`: its kind, its precedence `priority`, and the
/// `dxfId` (the differential format to apply when it matches, for the dxf-
/// painting kinds). `priority` orders rules; the lowest priority that matches
/// a cell wins (ECMA-376 §18.3.1.10).
#[derive(Clone, Debug, PartialEq)]
pub struct CfRule {
    pub kind: CfRuleKind,
    pub priority: i64,
    /// Index into the workbook `<dxfs>` table, or `None` (scale/bar/icon kinds
    /// paint their own visual, not a dxf).
    pub dxf_id: Option<u32>,
}

/// One `<conditionalFormatting>` block: the cell range it applies to (the
/// `sqref`, which may be a space-separated list of ranges) + its rules.
#[derive(Clone, Debug, PartialEq)]
pub struct CfBlock {
    /// The ranges this block applies to, each `(r0, c0, r1, c1)` 0-based
    /// inclusive. A single-cell `sqref` yields a 1x1 box.
    pub ranges: Vec<(u32, u32, u32, u32)>,
    pub rules: Vec<CfRule>,
}

impl CfBlock {
    /// True if `(row, col)` falls inside any range of this block.
    pub fn contains(&self, row: u32, col: u32) -> bool {
        self.ranges
            .iter()
            .any(|&(r0, c0, r1, c1)| row >= r0 && row <= r1 && col >= c0 && col <= c1)
    }
}

/// All conditional-format blocks parsed from one worksheet (M2 track). Empty
/// for a sheet with no conditional formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetConditionalFormats {
    pub blocks: Vec<CfBlock>,
}

impl SheetConditionalFormats {
    /// True if no sheet carries any conditional formatting.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Convert the parsed cf model into the `sheet-lower` mirror the lowering
    /// consumes ([`sheet_lower::SheetCondFmt`]), resolving each rule's `dxfId`
    /// against the workbook `<dxfs>` table (`dxfs`) into a concrete
    /// [`sheet_lower::VisualAttrs`] override. This is the cf analogue of
    /// `VisualStyle::to_attrs` — the one place the two mirror models meet. A
    /// `dxfId` out of range resolves to the default (inert) override.
    pub fn to_lower(&self, dxfs: &[VisualStyle]) -> sheet_lower::SheetCondFmt {
        sheet_lower::SheetCondFmt {
            blocks: self
                .blocks
                .iter()
                .map(|b| sheet_lower::CfBlock {
                    ranges: b.ranges.clone(),
                    rules: b.rules.iter().map(|r| r.to_lower(dxfs)).collect(),
                })
                .collect(),
        }
    }
}

impl CfRule {
    /// Lower one rule, resolving its `dxfId` to a concrete override.
    fn to_lower(&self, dxfs: &[VisualStyle]) -> sheet_lower::CfRule {
        let dxf = self
            .dxf_id
            .and_then(|id| dxfs.get(id as usize))
            .map(VisualStyle::to_lower_attrs)
            .unwrap_or_default();
        sheet_lower::CfRule {
            kind: self.kind.to_lower(),
            priority: self.priority,
            dxf,
        }
    }
}

impl CfRuleKind {
    /// Lower the rule kind into the `sheet-lower` mirror. Color scales resolve
    /// their `#RRGGBB` stops into `(r, g, b)` bytes; the dxf-painting kinds map
    /// straight across; the preserve-only floors collapse to `Preserved`.
    fn to_lower(&self) -> sheet_lower::CfRuleKind {
        match self {
            CfRuleKind::CellIs { op, operands } => sheet_lower::CfRuleKind::CellIs {
                op: op.to_lower(),
                operands: operands.clone(),
            },
            CfRuleKind::Expression { op, operand } => sheet_lower::CfRuleKind::Expression {
                op: op.to_lower(),
                operand: *operand,
            },
            CfRuleKind::ExpressionUnsupported => sheet_lower::CfRuleKind::ExpressionUnsupported,
            CfRuleKind::ColorScale(cs) => sheet_lower::CfRuleKind::ColorScale(cs.to_lower()),
            // dataBar lowers to the page-draw GEOMETRY lane (a drawn rect), not
            // a style override (spec §8.2). Its min/max endpoints + bar colour
            // cross into the `sheet-lower` mirror; the lowering draws the rect.
            CfRuleKind::DataBar(db) => sheet_lower::CfRuleKind::DataBar(db.to_lower()),
            // iconSet (preserve floor) + other rule kinds carry no override.
            CfRuleKind::IconSet | CfRuleKind::Preserved => sheet_lower::CfRuleKind::Preserved,
        }
    }
}

impl DataBar {
    /// Lower the data bar: resolve the min/max `<cfvo>` endpoints (an explicit
    /// numeric `type="num" val=N` becomes an absolute endpoint; `min`/`max`/
    /// `percent`/… stay `None` and derive from the covering range's domain in
    /// the lowering) + the bar colour (`#RRGGBB` → `(r, g, b)` bytes; the
    /// document-default blue `#638EC6` when the rule omits the colour).
    fn to_lower(&self) -> sheet_lower::DataBar {
        // A data bar carries its endpoints in `<cfvo>` order (min then max).
        let endpoint = |i: usize| {
            self.cfvos
                .get(i)
                .filter(|cfvo| cfvo.kind == "num")
                .and_then(|cfvo| cfvo.val)
        };
        sheet_lower::DataBar {
            min: endpoint(0),
            max: endpoint(1),
            rgb: parse_hex_rgb(self.color.as_deref().unwrap_or("#638EC6")),
        }
    }
}

impl ColorScale {
    /// Lower the colour scale: each `<cfvo>` becomes a stop with the resolved
    /// `#RRGGBB` colour (parsed to `(r, g, b)` bytes) and its absolute value (a
    /// numeric `<cfvo val>`; `min`/`max`/`percent` resolve to `None`, deriving
    /// from the range domain in the lowering). Mismatched cfvo/colour counts
    /// pair up to the shorter length (a defensive, lossless truncation).
    fn to_lower(&self) -> sheet_lower::ColorScale {
        // `cfvos` and `colors` are kept INDEX-ALIGNED at parse time (one color
        // entry per `<color>`, `None` for an unresolved stop — FINDING 2), so
        // the positional zip is now sound. A `None` colour (an unmapped
        // theme/indexed slot, or auto/empty) falls back to black `#000000`
        // (the documented default — `parse_hex_rgb`'s own malformed floor).
        let stops = self
            .cfvos
            .iter()
            .zip(self.colors.iter())
            .map(|(cfvo, color)| sheet_lower::ScaleStop {
                // Only an explicit numeric endpoint (`type="num" val=N`) is an
                // absolute value; `min`/`max`/`percent`/… derive from the range.
                value: if cfvo.kind == "num" { cfvo.val } else { None },
                rgb: parse_hex_rgb(color.as_deref().unwrap_or("#000000")),
            })
            .collect();
        sheet_lower::ColorScale { stops }
    }
}

impl CfOperator {
    /// The `sheet-lower` mirror of this operator.
    fn to_lower(self) -> sheet_lower::CfOperator {
        match self {
            CfOperator::GreaterThan => sheet_lower::CfOperator::GreaterThan,
            CfOperator::GreaterThanOrEqual => sheet_lower::CfOperator::GreaterThanOrEqual,
            CfOperator::LessThan => sheet_lower::CfOperator::LessThan,
            CfOperator::LessThanOrEqual => sheet_lower::CfOperator::LessThanOrEqual,
            CfOperator::Equal => sheet_lower::CfOperator::Equal,
            CfOperator::NotEqual => sheet_lower::CfOperator::NotEqual,
            CfOperator::Between => sheet_lower::CfOperator::Between,
            CfOperator::NotBetween => sheet_lower::CfOperator::NotBetween,
        }
    }
}

/// Parse a `#RRGGBB` colour to `(r, g, b)` bytes; black on a malformed string
/// (the lowering already dropped non-hex stops, so this is the safe floor).
fn parse_hex_rgb(s: &str) -> (u8, u8, u8) {
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&h[0..2], 16),
            u8::from_str_radix(&h[2..4], 16),
            u8::from_str_radix(&h[4..6], 16),
        ) {
            return (r, g, b);
        }
    }
    (0, 0, 0)
}

/// Parse one captured `<conditionalFormatting>` subtree (the verbatim bytes the
/// worksheet preserved) into a [`CfBlock`]. Returns `None` if the element is
/// not a `conditionalFormatting` block (so the caller can feed every captured
/// subtree and keep only the cf ones). A malformed rule is skipped (the bytes
/// still round-trip verbatim) rather than failing the whole parse.
pub fn parse_block(xml: &[u8]) -> Result<Option<CfBlock>, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    let mut is_cf = false;
    let mut ranges: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut rules: Vec<CfRule> = Vec::new();

    // The rule currently being built (Some between <cfRule> start and end).
    let mut cur: Option<RuleAccum> = None;
    // Text accumulation target inside a rule (a `<formula>` body).
    let mut in_formula = false;
    let mut formula_text = String::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"conditionalFormatting" => {
                    is_cf = true;
                    if let Some(sqref) = attr(&e, b"sqref")? {
                        ranges = parse_sqref(&sqref);
                    }
                }
                b"cfRule" => {
                    cur = Some(RuleAccum::start(&e)?);
                }
                b"formula" => {
                    in_formula = true;
                    formula_text.clear();
                }
                b"cfvo" => {
                    if let Some(c) = cur.as_mut() {
                        c.cfvos.push(CfValueObject {
                            kind: attr(&e, b"type")?.unwrap_or_else(|| "num".into()),
                            val: attr(&e, b"val")?.and_then(|v| v.trim().parse::<f64>().ok()),
                        });
                    }
                }
                b"color" => {
                    if let Some(c) = cur.as_mut() {
                        // FINDING 2 — push ONE entry per `<color>` element
                        // (even when it doesn't resolve to a fixed RGB), so
                        // `colors` stays index-aligned with `cfvos`. A theme/
                        // indexed colour now resolves; an unresolved one is a
                        // `None` placeholder, never a dropped stop.
                        c.colors.push(read_rgb(&e)?);
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_formula {
                    formula_text.push_str(&t.unescape().map_err(XlsxError::Xml)?);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"formula" => {
                    if let Some(c) = cur.as_mut() {
                        c.formulas.push(formula_text.trim().to_string());
                    }
                    in_formula = false;
                }
                b"cfRule" => {
                    if let Some(c) = cur.take() {
                        if let Some(rule) = c.finish() {
                            rules.push(rule);
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

    if !is_cf {
        return Ok(None);
    }
    Ok(Some(CfBlock { ranges, rules }))
}

/// Parse all conditional-format blocks out of an iterator of captured subtree
/// byte slices (the worksheet's `AfterSheetData` captures). Non-cf subtrees
/// are ignored. The blocks keep their source order.
pub fn parse_all<'a>(
    captured: impl Iterator<Item = &'a [u8]>,
) -> Result<SheetConditionalFormats, XlsxError> {
    let mut blocks = Vec::new();
    for bytes in captured {
        if let Some(block) = parse_block(bytes)? {
            blocks.push(block);
        }
    }
    Ok(SheetConditionalFormats { blocks })
}

/// Mutable accumulator for one `<cfRule>` while its children stream in.
struct RuleAccum {
    ty: String,
    operator: Option<CfOperator>,
    priority: i64,
    dxf_id: Option<u32>,
    formulas: Vec<String>,
    cfvos: Vec<CfValueObject>,
    /// One entry per `<color>` element, index-aligned with `cfvos`. `None` is
    /// a stop whose colour did not resolve to a fixed RGB (FINDING 2 — kept as
    /// a placeholder so the cfvo↔color alignment survives).
    colors: Vec<Option<String>>,
}

impl RuleAccum {
    fn start(e: &quick_xml::events::BytesStart<'_>) -> Result<RuleAccum, XlsxError> {
        Ok(RuleAccum {
            ty: attr(e, b"type")?.unwrap_or_default(),
            operator: attr(e, b"operator")?.and_then(|s| CfOperator::parse(&s)),
            // Absent priority sorts last (a large sentinel); real workbooks
            // always carry one (ECMA-376 makes it required).
            priority: attr(e, b"priority")?
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(i64::MAX),
            dxf_id: attr(e, b"dxfId")?.and_then(|s| s.trim().parse::<u32>().ok()),
            formulas: Vec::new(),
            cfvos: Vec::new(),
            colors: Vec::new(),
        })
    }

    /// Resolve the accumulated rule into a typed [`CfRule`], or `None` to drop
    /// an unusable rule (it still round-trips via the verbatim capture).
    fn finish(self) -> Option<CfRule> {
        let kind = match self.ty.as_str() {
            "cellIs" => {
                let op = self.operator?;
                let operands = self.numeric_formulas();
                let need = if op.is_range() { 2 } else { 1 };
                if operands.len() < need {
                    // A non-numeric / missing operand makes the rule inert in
                    // T2 (documented limitation); preserve-only.
                    CfRuleKind::Preserved
                } else {
                    CfRuleKind::CellIs { op, operands }
                }
            }
            "expression" => reduce_expression(self.formulas.first().map(String::as_str)),
            "colorScale" => CfRuleKind::ColorScale(ColorScale {
                cfvos: self.cfvos,
                colors: self.colors,
            }),
            "dataBar" => CfRuleKind::DataBar(DataBar {
                cfvos: self.cfvos,
                // The single bar colour (first `<color>`); flatten the
                // placeholder `Option` (a data bar's colour is itself optional).
                color: self.colors.into_iter().next().flatten(),
            }),
            "iconSet" => CfRuleKind::IconSet,
            _ => CfRuleKind::Preserved,
        };
        Some(CfRule {
            kind,
            priority: self.priority,
            dxf_id: self.dxf_id,
        })
    }

    /// The `<formula>` operands parsed as numeric literals, in order. A
    /// non-numeric operand stops the list (a comparison needs numbers in T2).
    fn numeric_formulas(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for f in &self.formulas {
            match f.trim().parse::<f64>() {
                Ok(n) => out.push(n),
                Err(_) => break,
            }
        }
        out
    }
}

/// Reduce an `expression` rule's formula to a simple `cell <op> literal` test,
/// the only expression form T2 lowers without a formula evaluator. The
/// predicate references the top-left cell of the sqref relatively (e.g.
/// `A1>5`); we only need the operator + the literal operand — the cell side is
/// the cell under test. Anything we cannot reduce becomes
/// [`CfRuleKind::ExpressionUnsupported`] (DEFERRED, applies no override).
fn reduce_expression(formula: Option<&str>) -> CfRuleKind {
    let Some(f) = formula else {
        return CfRuleKind::ExpressionUnsupported;
    };
    let f = f.trim();
    // Try the two-char operators first so `>=`/`<=`/`<>` win over `>`/`<`.
    for (token, op) in [
        (">=", CfOperator::GreaterThanOrEqual),
        ("<=", CfOperator::LessThanOrEqual),
        ("<>", CfOperator::NotEqual),
        (">", CfOperator::GreaterThan),
        ("<", CfOperator::LessThan),
        ("=", CfOperator::Equal),
    ] {
        if let Some((lhs, rhs)) = f.split_once(token) {
            // The lhs must be a bare relative cell reference (the cell under
            // test) and the rhs a numeric literal — anything else is DEFERRED.
            if is_simple_cell_ref(lhs.trim()) {
                if let Ok(n) = rhs.trim().parse::<f64>() {
                    return CfRuleKind::Expression { op, operand: n };
                }
            }
            return CfRuleKind::ExpressionUnsupported;
        }
    }
    CfRuleKind::ExpressionUnsupported
}

/// True if `s` is a single A1 cell reference, optionally with `$` anchors
/// (e.g. `A1`, `$A1`, `$A$1`). Not a range, not a function call.
fn is_simple_cell_ref(s: &str) -> bool {
    !s.is_empty() && !s.contains([':', '(', ' ', ',']) && parse_a1(s).is_some()
}

/// Parse a `sqref` (space-separated A1 ranges) into 0-based inclusive boxes.
/// A bare cell `A1` becomes a 1x1 box. Unparseable tokens are skipped.
fn parse_sqref(sqref: &str) -> Vec<(u32, u32, u32, u32)> {
    sqref
        .split_whitespace()
        .filter_map(parse_one_range)
        .collect()
}

/// Parse one `A1` or `A1:B2` token into `(r0, c0, r1, c1)` 0-based inclusive.
fn parse_one_range(tok: &str) -> Option<(u32, u32, u32, u32)> {
    match tok.split_once(':') {
        Some((a, b)) => {
            let (r0, c0, _, _) = parse_a1(a)?;
            let (r1, c1, _, _) = parse_a1(b)?;
            Some((r0.min(r1), c0.min(c1), r0.max(r1), c0.max(c1)))
        }
        None => {
            let (r, c, _, _) = parse_a1(tok)?;
            Some((r, c, r, c))
        }
    }
}

/// Resolve a `<color>` element's colour to `#RRGGBB`, or `None` when no fixed
/// RGB is resolvable. Handles ALL the attribute forms a colour-scale stop can
/// carry — explicit `rgb=` (alpha byte dropped), legacy `indexed=` (the
/// ECMA-376 §18.8.27 palette, reused from `styles`), and `theme=` (the
/// best-effort Office-default mapping in `styles::theme_color`). A `theme`
/// slot beyond the mapped set, an `auto`/empty colour, or a malformed hex all
/// resolve to `None`.
///
/// FINDING 2 — `read_rgb` used to read ONLY `rgb=`; a `theme`/`indexed` stop
/// returned `None` and (combined with a push-only-on-`Some` caller) was
/// DROPPED, which then misaligned the positional `cfvo`↔`color` zip in
/// `ColorScale::to_lower` (every later stop got the wrong colour, the last
/// vanished). The caller now ALWAYS pushes one entry per `<color>` so the
/// `cfvos`/`colors` vecs stay index-aligned even when this returns `None`.
fn read_rgb(e: &quick_xml::events::BytesStart<'_>) -> Result<Option<String>, XlsxError> {
    if let Some(rgb) = attr(e, b"rgb")? {
        let hex = rgb.trim();
        let body = match hex.len() {
            8 => &hex[2..],
            6 => hex,
            _ => return Ok(None),
        };
        return Ok(if body.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(format!("#{}", body.to_ascii_uppercase()))
        } else {
            None
        });
    }
    if let Some(indexed) = attr(e, b"indexed")? {
        if let Ok(i) = indexed.trim().parse::<u32>() {
            return Ok(indexed_color(i).map(str::to_owned));
        }
    }
    if let Some(theme) = attr(e, b"theme")? {
        if let Ok(t) = theme.trim().parse::<u32>() {
            return Ok(theme_color(t).map(str::to_owned));
        }
    }
    // `auto="1"` or an empty <color/> → no fixed colour.
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cellis_greater_than() {
        let xml = br#"<conditionalFormatting sqref="A1:B3" xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <cfRule type="cellIs" dxfId="0" priority="1" operator="greaterThan"><formula>5</formula></cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().expect("is a cf block");
        assert_eq!(block.ranges, vec![(0, 0, 2, 1)]);
        assert!(block.contains(0, 0));
        assert!(block.contains(2, 1));
        assert!(!block.contains(3, 0));
        assert_eq!(block.rules.len(), 1);
        let r = &block.rules[0];
        assert_eq!(r.priority, 1);
        assert_eq!(r.dxf_id, Some(0));
        assert_eq!(
            r.kind,
            CfRuleKind::CellIs {
                op: CfOperator::GreaterThan,
                operands: vec![5.0]
            }
        );
    }

    #[test]
    fn parse_cellis_between_two_operands() {
        let xml = br#"<conditionalFormatting sqref="A1">
  <cfRule type="cellIs" dxfId="2" priority="3" operator="between">
    <formula>10</formula><formula>20</formula></cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        assert_eq!(
            block.rules[0].kind,
            CfRuleKind::CellIs {
                op: CfOperator::Between,
                operands: vec![10.0, 20.0]
            }
        );
    }

    #[test]
    fn cellis_missing_operand_is_preserved() {
        // between with only one numeric operand cannot evaluate → preserve-only.
        let xml = br#"<conditionalFormatting sqref="A1">
  <cfRule type="cellIs" dxfId="0" priority="1" operator="between"><formula>10</formula></cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        assert_eq!(block.rules[0].kind, CfRuleKind::Preserved);
    }

    #[test]
    fn parse_expression_simple_comparison() {
        let xml = br#"<conditionalFormatting sqref="C1:C5">
  <cfRule type="expression" dxfId="1" priority="2"><formula>C1&gt;100</formula></cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        assert_eq!(
            block.rules[0].kind,
            CfRuleKind::Expression {
                op: CfOperator::GreaterThan,
                operand: 100.0
            }
        );
    }

    #[test]
    fn parse_expression_unsupported_is_deferred() {
        // An arbitrary formula predicate (function call, range) is DEFERRED.
        let xml = br#"<conditionalFormatting sqref="A1:A9">
  <cfRule type="expression" dxfId="0" priority="1"><formula>AND(A1&gt;0,B1&lt;5)</formula></cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        assert_eq!(block.rules[0].kind, CfRuleKind::ExpressionUnsupported);
    }

    #[test]
    fn parse_two_color_scale() {
        let xml = br#"<conditionalFormatting sqref="A1:A10">
  <cfRule type="colorScale" priority="1">
    <colorScale>
      <cfvo type="min"/><cfvo type="max"/>
      <color rgb="FFFFFFFF"/><color rgb="FFFF0000"/>
    </colorScale>
  </cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        match &block.rules[0].kind {
            CfRuleKind::ColorScale(cs) => {
                assert_eq!(cs.cfvos.len(), 2);
                assert_eq!(cs.cfvos[0].kind, "min");
                assert_eq!(
                    cs.colors,
                    vec![Some("#FFFFFF".to_string()), Some("#FF0000".to_string())]
                );
            }
            other => panic!("expected ColorScale, got {other:?}"),
        }
    }

    #[test]
    fn parse_three_color_scale_with_num_cfvo() {
        let xml = br#"<conditionalFormatting sqref="B2:B8">
  <cfRule type="colorScale" priority="1">
    <colorScale>
      <cfvo type="num" val="0"/><cfvo type="num" val="50"/><cfvo type="num" val="100"/>
      <color rgb="FFF8696B"/><color rgb="FFFFEB84"/><color rgb="FF63BE7B"/>
    </colorScale>
  </cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        match &block.rules[0].kind {
            CfRuleKind::ColorScale(cs) => {
                assert_eq!(cs.cfvos.len(), 3);
                assert_eq!(cs.cfvos[1].val, Some(50.0));
                assert_eq!(cs.colors.len(), 3);
            }
            other => panic!("expected ColorScale, got {other:?}"),
        }
    }

    /// FINDING 2 regression — a 3-colour scale whose MIDDLE stop is a `theme`
    /// colour (no `rgb=`). Pre-fix `read_rgb` returned `None` for it and the
    /// push-only-on-`Some` caller DROPPED it, so `colors` had 2 entries against
    /// 3 cfvos and the positional zip in `to_lower` misaligned (the middle stop
    /// got the LAST colour and the last stop vanished). The parser must keep one
    /// colour entry per `<color>` (resolving the theme/indexed where it can) so
    /// the alignment survives.
    #[test]
    fn sheet_lower_condfmt_colorscale_theme_indexed_stops_align() {
        // theme="4" is the Office accent1 blue (#4472C4); indexed="2" is red
        // (#FF0000) from the legacy palette. Both lack `rgb=`.
        let xml = br#"<conditionalFormatting sqref="D1:D9">
  <cfRule type="colorScale" priority="1">
    <colorScale>
      <cfvo type="num" val="0"/><cfvo type="num" val="50"/><cfvo type="num" val="100"/>
      <color rgb="FFF8696B"/><color theme="4"/><color indexed="2"/>
    </colorScale>
  </cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        let cs = match &block.rules[0].kind {
            CfRuleKind::ColorScale(cs) => cs,
            other => panic!("expected ColorScale, got {other:?}"),
        };
        // ONE colour entry per cfvo — the alignment invariant.
        assert_eq!(cs.cfvos.len(), 3);
        assert_eq!(cs.colors.len(), 3, "one colour per cfvo (no dropped stop)");
        // The theme + indexed stops resolved (not dropped, not None).
        assert_eq!(
            cs.colors,
            vec![
                Some("#F8696B".to_string()), // explicit rgb (alpha stripped)
                Some("#4472C4".to_string()), // theme=4 → accent1 blue
                Some("#FF0000".to_string()), // indexed=2 → red
            ]
        );

        // The lowered stops stay correctly paired (the bug surfaced HERE — a
        // misaligned zip put the wrong colour on each stop).
        let lowered = cs.to_lower();
        assert_eq!(lowered.stops.len(), 3);
        assert_eq!(lowered.stops[0].rgb, (0xF8, 0x69, 0x6B));
        assert_eq!(lowered.stops[1].rgb, (0x44, 0x72, 0xC4));
        assert_eq!(lowered.stops[2].rgb, (0xFF, 0x00, 0x00));
        assert_eq!(lowered.stops[1].value, Some(50.0));
    }

    /// FINDING 2 — an UNRESOLVABLE stop (a theme slot past the mapped set) is
    /// kept as a `None` PLACEHOLDER, not dropped, so later stops stay aligned;
    /// the lowering falls back to the documented `#000000` default for it.
    #[test]
    fn sheet_lower_condfmt_colorscale_unresolved_stop_holds_alignment() {
        // theme="9" is past the mapped Office slots → unresolved.
        let xml = br#"<conditionalFormatting sqref="E1:E9">
  <cfRule type="colorScale" priority="1">
    <colorScale>
      <cfvo type="num" val="0"/><cfvo type="num" val="50"/><cfvo type="num" val="100"/>
      <color rgb="FF00FF00"/><color theme="9"/><color rgb="FF0000FF"/>
    </colorScale>
  </cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        let cs = match &block.rules[0].kind {
            CfRuleKind::ColorScale(cs) => cs,
            other => panic!("expected ColorScale, got {other:?}"),
        };
        assert_eq!(cs.colors.len(), 3, "unresolved stop kept as placeholder");
        assert_eq!(cs.colors[1], None);
        // The THIRD stop still gets its own blue (not shifted up onto stop 2).
        assert_eq!(cs.colors[2], Some("#0000FF".to_string()));

        let lowered = cs.to_lower();
        assert_eq!(lowered.stops[0].rgb, (0x00, 0xFF, 0x00)); // green
        assert_eq!(lowered.stops[1].rgb, (0x00, 0x00, 0x00)); // None → #000000
        assert_eq!(lowered.stops[2].rgb, (0x00, 0x00, 0xFF)); // blue, aligned
    }

    #[test]
    fn parse_databar() {
        let xml = br#"<conditionalFormatting sqref="C1:C4">
  <cfRule type="dataBar" priority="1">
    <dataBar><cfvo type="min"/><cfvo type="max"/><color rgb="FF638EC6"/></dataBar>
  </cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        match &block.rules[0].kind {
            CfRuleKind::DataBar(db) => {
                assert_eq!(db.cfvos.len(), 2);
                assert_eq!(db.color.as_deref(), Some("#638EC6"));
            }
            other => panic!("expected DataBar, got {other:?}"),
        }
    }

    #[test]
    fn parse_iconset_preserve_only() {
        let xml = br#"<conditionalFormatting sqref="A1:A3">
  <cfRule type="iconSet" priority="1">
    <iconSet iconSet="3TrafficLights1">
      <cfvo type="percent" val="0"/><cfvo type="percent" val="33"/><cfvo type="percent" val="67"/>
    </iconSet>
  </cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        assert_eq!(block.rules[0].kind, CfRuleKind::IconSet);
    }

    #[test]
    fn non_cf_subtree_returns_none() {
        let xml = br#"<pageMargins left="0.7"/>"#;
        assert_eq!(parse_block(xml).unwrap(), None);
    }

    #[test]
    fn multi_range_sqref() {
        let xml = br#"<conditionalFormatting sqref="A1:A2 C1:C2">
  <cfRule type="cellIs" dxfId="0" priority="1" operator="equal"><formula>1</formula></cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        assert_eq!(block.ranges, vec![(0, 0, 1, 0), (0, 2, 1, 2)]);
        assert!(block.contains(0, 0));
        assert!(block.contains(1, 2));
        assert!(!block.contains(0, 1));
    }

    #[test]
    fn to_lower_resolves_dxf_and_scale() {
        // A cellIs rule referencing dxf 0 (yellow fill) + a 2-colour scale.
        let xml = br#"<conditionalFormatting sqref="A1:A5">
  <cfRule type="cellIs" dxfId="0" priority="2" operator="greaterThan"><formula>5</formula></cfRule>
  <cfRule type="colorScale" priority="1">
    <colorScale><cfvo type="min"/><cfvo type="max"/>
    <color rgb="FFFFFFFF"/><color rgb="FFFF0000"/></colorScale>
  </cfRule>
</conditionalFormatting>"#;
        let block = parse_block(xml).unwrap().unwrap();
        let sheet = SheetConditionalFormats {
            blocks: vec![block],
        };

        // dxf 0 = yellow fill.
        let dxfs = vec![VisualStyle {
            fill_rgb: Some("#FFFF00".into()),
            ..Default::default()
        }];
        let lowered = sheet.to_lower(&dxfs);
        assert_eq!(lowered.blocks.len(), 1);
        let rules = &lowered.blocks[0].rules;
        assert_eq!(rules.len(), 2);

        // cellIs rule carries the resolved dxf override (yellow fill).
        let cellis = rules
            .iter()
            .find(|r| matches!(r.kind, sheet_lower::CfRuleKind::CellIs { .. }))
            .unwrap();
        assert_eq!(cellis.priority, 2);
        assert_eq!(cellis.dxf.fill_rgb.as_deref(), Some("#FFFF00"));

        // colorScale rule carries two stops with parsed RGB bytes.
        let scale = rules
            .iter()
            .find_map(|r| match &r.kind {
                sheet_lower::CfRuleKind::ColorScale(cs) => Some(cs),
                _ => None,
            })
            .unwrap();
        assert_eq!(scale.stops.len(), 2);
        assert_eq!(scale.stops[0].rgb, (0xFF, 0xFF, 0xFF));
        assert_eq!(scale.stops[1].rgb, (0xFF, 0x00, 0x00));
        assert_eq!(scale.stops[0].value, None); // min → derive from range
    }

    #[test]
    fn to_lower_databar_geometry_iconset_preserved() {
        // dataBar lowers to the GEOMETRY lane (a `sheet_lower::DataBar`);
        // iconSet stays preserve-only.
        let xml = br#"<conditionalFormatting sqref="A1:A3">
  <cfRule type="dataBar" priority="1"><dataBar><cfvo type="num" val="0"/><cfvo type="num" val="100"/><color rgb="FF638EC6"/></dataBar></cfRule>
  <cfRule type="iconSet" priority="2"><iconSet iconSet="3Arrows"/></cfRule>
</conditionalFormatting>"#;
        let sheet = SheetConditionalFormats {
            blocks: vec![parse_block(xml).unwrap().unwrap()],
        };
        let lowered = sheet.to_lower(&[]);
        // Both rules sit in ONE block (one <conditionalFormatting> element).
        // The data bar carries its endpoints + colour into the geometry mirror.
        assert_eq!(
            lowered.blocks[0].rules[0].kind,
            sheet_lower::CfRuleKind::DataBar(sheet_lower::DataBar {
                min: Some(0.0),
                max: Some(100.0),
                rgb: (0x63, 0x8E, 0xC6),
            })
        );
        // The iconSet stays preserve-only.
        assert_eq!(
            lowered.blocks[0].rules[1].kind,
            sheet_lower::CfRuleKind::Preserved
        );
    }

    #[test]
    fn to_lower_databar_default_color_and_derived_endpoints() {
        // A data bar with min/max cfvo (no explicit num) + no colour → the
        // endpoints derive (None) and the colour is the document default blue.
        let xml = br#"<conditionalFormatting sqref="A1:A3">
  <cfRule type="dataBar" priority="1"><dataBar><cfvo type="min"/><cfvo type="max"/></dataBar></cfRule>
</conditionalFormatting>"#;
        let sheet = SheetConditionalFormats {
            blocks: vec![parse_block(xml).unwrap().unwrap()],
        };
        let lowered = sheet.to_lower(&[]);
        assert_eq!(
            lowered.blocks[0].rules[0].kind,
            sheet_lower::CfRuleKind::DataBar(sheet_lower::DataBar {
                min: None,
                max: None,
                rgb: (0x63, 0x8E, 0xC6), // #638EC6 default
            })
        );
    }

    #[test]
    fn parse_all_filters_to_cf_blocks() {
        let cf = br#"<conditionalFormatting sqref="A1"><cfRule type="cellIs" dxfId="0" priority="1" operator="equal"><formula>1</formula></cfRule></conditionalFormatting>"#;
        let pm = br#"<pageMargins left="0.7"/>"#;
        let subtrees: Vec<&[u8]> = vec![pm, cf, pm];
        let parsed = parse_all(subtrees.into_iter()).unwrap();
        assert_eq!(parsed.blocks.len(), 1);
        assert!(!parsed.is_empty());
    }
}
