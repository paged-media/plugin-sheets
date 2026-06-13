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

//! Conditional formatting → per-cell style overrides (spec §10.4, §8.3; M2).
//!
//! This module evaluates a worksheet's conditional-format rules against the
//! cell values ALREADY computed in the model and folds the matching rule's
//! differential format into the lowered style — the §8.3 principle exactly:
//! "direct formatting lowers as a constrained local override". The result is a
//! [`crate::LoweredContent`] whose `styles` table carries the cf-painted styles
//! and whose cells point at them via `style_key`.
//!
//! ## Purity (the dependency line)
//!
//! `sheet-lower` is a PURE leaf IR crate — core + format only, NO `sheet-calc`,
//! NO `sheet-xlsx`. So this module:
//!
//! - mirrors the xlsx cf model with `sheet-lower`-local types ([`CfRule`],
//!   [`CfRuleKind`], …) exactly as [`crate::VisualAttrs`] mirrors
//!   `sheet_xlsx::VisualStyle` — the xlsx side converts its parsed model into
//!   these (see `sheet_xlsx`'s `to_lower_*` impls);
//! - evaluates ONLY value comparisons (`cellIs`, the reducible `expression`
//!   forms) and colour-scale interpolation — these need just the cell value,
//!   which lowering already has. **Arbitrary `expression` predicates that need
//!   a formula evaluator are DEFERRED** ([`CfRuleKind::ExpressionUnsupported`]
//!   applies no override) — lowering never calls the calc engine. This is the
//!   honest T2 line, documented in the registry row `sheet.lower.condfmt.*`.
//!
//! ## Precedence (ECMA-376 §18.3.1.10)
//!
//! Multiple rules may cover a cell. The lowest `priority` that MATCHES wins for
//! a given facet; lower priority = higher precedence. T2 applies the single
//! winning dxf-painting rule's override (it does not blend multiple dxfs).
//! Colour-scale / data-bar rules paint a fill independent of the dxf rules; a
//! cell under both a matching cellIs and a colour scale takes the cellIs dxf
//! (the higher-precedence override semantics — documented).

use crate::style::VisualAttrs;
use sheet_core::CellValue;

/// A cell range as `(r0, c0, r1, c1)`, 0-based inclusive (model coordinates).
pub type CfRange = (u32, u32, u32, u32);

/// A lazily-computed colour-scale domain lookup: maps a covering range to its
/// `(min, max)` numeric value domain, or `None` (no numeric cells). The
/// lowering passes one to [`override_for`] so a scale interpolates over the live
/// value domain without `sheet-lower` walking the model itself.
pub type DomainFn<'a> = dyn FnMut(CfRange) -> Option<(f64, f64)> + 'a;

/// The comparison operator of a `cellIs` / reduced-`expression` rule
/// (`sheet-lower` mirror of `sheet_xlsx::CfOperator`).
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
    /// Evaluate `value <op> operands` against a numeric cell value. Returns
    /// `false` for a non-numeric cell (cf value comparisons only apply to
    /// numbers in T2). `operands` is `[a]` for the single-operand operators,
    /// `[a, b]` for between / notBetween.
    fn matches(self, value: f64, operands: &[f64]) -> bool {
        match self {
            CfOperator::GreaterThan => operands.first().is_some_and(|&a| value > a),
            CfOperator::GreaterThanOrEqual => operands.first().is_some_and(|&a| value >= a),
            CfOperator::LessThan => operands.first().is_some_and(|&a| value < a),
            CfOperator::LessThanOrEqual => operands.first().is_some_and(|&a| value <= a),
            CfOperator::Equal => operands.first().is_some_and(|&a| value == a),
            CfOperator::NotEqual => operands.first().is_some_and(|&a| value != a),
            CfOperator::Between => match operands {
                [a, b, ..] => value >= a.min(*b) && value <= a.max(*b),
                _ => false,
            },
            CfOperator::NotBetween => match operands {
                [a, b, ..] => value < a.min(*b) || value > a.max(*b),
                _ => false,
            },
        }
    }
}

/// One colour stop of a colour scale: the absolute domain value + the colour.
/// (The lowering computes the domain from the block's range when a stop is
/// `min`/`max`; the xlsx→lower conversion resolves the numeric stops.)
#[derive(Clone, Debug, PartialEq)]
pub struct ScaleStop {
    /// The absolute domain value of this stop, or `None` (= derive from the
    /// range min/max, in stop order low→high).
    pub value: Option<f64>,
    /// The stop colour as `(r, g, b)` bytes.
    pub rgb: (u8, u8, u8),
}

/// A 2- or 3-colour scale (the stops in low→high order).
#[derive(Clone, Debug, PartialEq)]
pub struct ColorScale {
    pub stops: Vec<ScaleStop>,
}

/// A data-bar rule (`sheet-lower` mirror of `sheet_xlsx::DataBar`): the
/// min/max domain endpoints + the bar fill colour. The bar lowers to a DRAWN
/// RECT (the page-draw geometry lane, spec §8.2) — NOT a style fill — so it
/// lives on its own `CfRuleKind::DataBar` variant, evaluated by
/// [`databar_rect_fraction`] against the covering range's value domain.
#[derive(Clone, Debug, PartialEq)]
pub struct DataBar {
    /// The bar's domain min endpoint, or `None` (= derive the range min).
    pub min: Option<f64>,
    /// The bar's domain max endpoint, or `None` (= derive the range max).
    pub max: Option<f64>,
    /// The bar fill colour as `(r, g, b)` bytes (the document default blue
    /// `#638EC6` when the XLSX rule omits the colour).
    pub rgb: (u8, u8, u8),
}

/// The interpreted kind of one cf rule (the T2 lowering subset).
#[derive(Clone, Debug, PartialEq)]
pub enum CfRuleKind {
    /// A value comparison; matches paint [`CfRule::dxf`].
    CellIs { op: CfOperator, operands: Vec<f64> },
    /// A reduced `expression` (`cell <op> literal`); matches paint the dxf.
    Expression { op: CfOperator, operand: f64 },
    /// An `expression` we cannot evaluate without a formula engine — DEFERRED
    /// (no override; round-trips via the xlsx verbatim capture).
    ExpressionUnsupported,
    /// A colour scale; matches paint an interpolated fill (not the dxf).
    ColorScale(ColorScale),
    /// A data bar — a proportional DRAWN RECT in the cell (the page-draw
    /// geometry lane, spec §8.2). Lowered to a [`crate::DataBarRect`], NOT a
    /// style override.
    DataBar(DataBar),
    /// Preserve-only (icon set / other rule kinds carry no override in T2).
    Preserved,
}

/// One cf rule (a `sheet-lower` mirror of `sheet_xlsx::CfRule` joined with its
/// resolved dxf override): kind + precedence + the differential format to apply
/// on a match (for the dxf-painting kinds).
#[derive(Clone, Debug, PartialEq)]
pub struct CfRule {
    pub kind: CfRuleKind,
    pub priority: i64,
    /// The resolved differential format (the dxf the xlsx side looked up from
    /// `dxfId`), applied on top of the base style when this rule matches. A
    /// scale/preserved rule carries a default (unused) override.
    pub dxf: VisualAttrs,
}

/// One cf block: the ranges it applies to + its rules (mirror of
/// `sheet_xlsx::CfBlock` with dxf overrides resolved). Ranges are `(r0, c0, r1,
/// c1)` 0-based inclusive in MODEL (sheet) coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CfBlock {
    pub ranges: Vec<(u32, u32, u32, u32)>,
    pub rules: Vec<CfRule>,
}

impl CfBlock {
    fn contains(&self, row: u32, col: u32) -> bool {
        self.ranges
            .iter()
            .any(|&(r0, c0, r1, c1)| row >= r0 && row <= r1 && col >= c0 && col <= c1)
    }
}

/// All cf blocks for one worksheet (mirror of
/// `sheet_xlsx::SheetConditionalFormats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetCondFmt {
    pub blocks: Vec<CfBlock>,
}

impl SheetCondFmt {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// The result of evaluating cf at one cell: an optional [`VisualAttrs`]
/// override to fold onto the base style. `None` = no rule painted this cell.
pub fn override_for(
    cf: &SheetCondFmt,
    row: u32,
    col: u32,
    value: &CellValue,
    domain: &mut DomainFn,
) -> Option<VisualAttrs> {
    // The numeric value (the only kind cf value comparisons + scales apply to).
    let n = match value {
        CellValue::Number(n) => Some(*n),
        _ => None,
    };

    // Collect every matching dxf-painting rule (cellIs / reduced expression)
    // that covers this cell, then pick the lowest priority (highest precedence).
    let mut best_dxf: Option<(i64, &VisualAttrs)> = None;
    // A colour-scale rule covering this cell (lowest priority wins among scales).
    let mut best_scale: Option<(i64, &ColorScale, CfRange)> = None;

    for block in &cf.blocks {
        if !block.contains(row, col) {
            continue;
        }
        // The covering range of this block (for scale-domain derivation): the
        // first range that contains the cell.
        let covering = block
            .ranges
            .iter()
            .copied()
            .find(|&(r0, c0, r1, c1)| row >= r0 && row <= r1 && col >= c0 && col <= c1);

        for rule in &block.rules {
            match &rule.kind {
                CfRuleKind::CellIs { op, operands } => {
                    if let Some(v) = n {
                        if op.matches(v, operands) && is_better(&best_dxf, rule.priority) {
                            best_dxf = Some((rule.priority, &rule.dxf));
                        }
                    }
                }
                CfRuleKind::Expression { op, operand } => {
                    if let Some(v) = n {
                        if op.matches(v, &[*operand]) && is_better(&best_dxf, rule.priority) {
                            best_dxf = Some((rule.priority, &rule.dxf));
                        }
                    }
                }
                CfRuleKind::ColorScale(cs) => {
                    if n.is_some() {
                        if let Some(cov) = covering {
                            let better = best_scale.as_ref().is_none_or(|b| rule.priority < b.0);
                            if better {
                                best_scale = Some((rule.priority, cs, cov));
                            }
                        }
                    }
                }
                // Data bars are GEOMETRY (the drawn-rect lane, `databar_for`),
                // not a style override — they never paint a cell fill here.
                CfRuleKind::DataBar(_)
                | CfRuleKind::ExpressionUnsupported
                | CfRuleKind::Preserved => {}
            }
        }
    }

    // A matching cellIs/expression dxf wins over a colour scale (precedence).
    if let Some((_, attrs)) = best_dxf {
        return Some(attrs.clone());
    }
    if let (Some(v), Some((_, cs, cov))) = (n, best_scale) {
        if let Some(rgb) = interpolate_scale(cs, v, cov, domain) {
            return Some(VisualAttrs {
                fill_rgb: Some(rgb),
                ..VisualAttrs::default()
            });
        }
    }
    None
}

/// True if `priority` is a strictly higher-precedence (lower-numbered) winner
/// than the current best (or there is no best yet).
fn is_better(best: &Option<(i64, &VisualAttrs)>, priority: i64) -> bool {
    best.as_ref().is_none_or(|b| priority < b.0)
}

/// The data bar to DRAW at one cell, if any rule covering it is a data bar
/// (spec §8.2 — the drawn-rect geometry lane, NOT a style fill). Returns the
/// `(fill_fraction, rgb)` for the lowest-priority data bar covering the cell,
/// where `fill_fraction ∈ [0, 1]` is the bar length as a share of the cell
/// width — `(value - lo) / (hi - lo)`, clamped, computed against the data
/// bar's domain endpoints (explicit `min`/`max`, else the covering range's
/// numeric min/max via `domain`). `None` for a non-numeric cell, a cell no
/// data bar covers, or a degenerate (zero-width / no-numeric) domain.
///
/// Distinct lane from [`override_for`]: data bars are GEOMETRY, so the lowering
/// keeps the cell's style untouched and draws a rect on top. A cell under both
/// a dxf rule and a data bar gets BOTH (the fill is the bar, the style is the
/// dxf) — they do not compete (Excel layers a data bar over the cell fill).
pub fn databar_for(
    cf: &SheetCondFmt,
    row: u32,
    col: u32,
    value: &CellValue,
    domain: &mut DomainFn,
) -> Option<(f64, (u8, u8, u8))> {
    let v = match value {
        CellValue::Number(n) => *n,
        _ => return None,
    };

    let mut best: Option<(i64, &DataBar, CfRange)> = None;
    for block in &cf.blocks {
        if !block.contains(row, col) {
            continue;
        }
        let covering = block
            .ranges
            .iter()
            .copied()
            .find(|&(r0, c0, r1, c1)| row >= r0 && row <= r1 && col >= c0 && col <= c1);
        for rule in &block.rules {
            if let (CfRuleKind::DataBar(db), Some(cov)) = (&rule.kind, covering) {
                if best.as_ref().is_none_or(|b| rule.priority < b.0) {
                    best = Some((rule.priority, db, cov));
                }
            }
        }
    }

    let (_, db, cov) = best?;
    let (dmin, dmax) = domain(cov)?;
    let lo = db.min.unwrap_or(dmin);
    let hi = db.max.unwrap_or(dmax);
    if (hi - lo).abs() < f64::EPSILON {
        return None; // degenerate domain — no bar (avoids div-by-zero)
    }
    let frac = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    Some((frac, db.rgb))
}

/// Interpolate a 2-/3-colour scale at `value` over the covering range's domain.
/// Stops with explicit values use them; `min`/`max` stops (value `None`) derive
/// from the range's `(min, max)` (in stop order). Returns the `#RRGGBB` fill.
fn interpolate_scale(
    cs: &ColorScale,
    value: f64,
    covering: CfRange,
    domain: &mut DomainFn,
) -> Option<String> {
    if cs.stops.len() < 2 {
        return None;
    }
    let (dmin, dmax) = domain(covering)?;

    // Resolve each stop's absolute domain position. A `None`-value stop takes
    // the range min for the FIRST stop and max for the LAST; a middle None stop
    // (rare) takes the midpoint.
    let last = cs.stops.len() - 1;
    let positions: Vec<f64> = cs
        .stops
        .iter()
        .enumerate()
        .map(|(i, s)| {
            s.value.unwrap_or(if i == 0 {
                dmin
            } else if i == last {
                dmax
            } else {
                (dmin + dmax) / 2.0
            })
        })
        .collect();

    // Clamp the value into the domain and find the bracketing pair of stops.
    let v = value.clamp(positions[0], positions[last]);
    for w in 0..last {
        let (p0, p1) = (positions[w], positions[w + 1]);
        if v >= p0 && v <= p1 {
            let t = if (p1 - p0).abs() < f64::EPSILON {
                0.0
            } else {
                (v - p0) / (p1 - p0)
            };
            let c = lerp_rgb(cs.stops[w].rgb, cs.stops[w + 1].rgb, t);
            return Some(format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2));
        }
    }
    // Fallback: the nearest endpoint colour.
    let c = if v <= positions[0] {
        cs.stops[0].rgb
    } else {
        cs.stops[last].rgb
    };
    Some(format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2))
}

/// Linear-interpolate two RGB triples at `t` in `[0, 1]` (per-channel, rounded).
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Fold a cf override onto a base [`VisualAttrs`]: each override facet that is
/// SET wins; an unset override facet leaves the base value. This is the §8.3
/// "constrained local override" — the dxf changes only the attributes it names.
pub fn fold_override(base: &VisualAttrs, over: &VisualAttrs) -> VisualAttrs {
    VisualAttrs {
        bold: base.bold || over.bold,
        italic: base.italic || over.italic,
        font_size_pt: over.font_size_pt.or(base.font_size_pt),
        font_name: over.font_name.clone().or_else(|| base.font_name.clone()),
        fill_rgb: over.fill_rgb.clone().or_else(|| base.fill_rgb.clone()),
        text_rgb: over.text_rgb.clone().or_else(|| base.text_rgb.clone()),
        border_top: base.border_top || over.border_top,
        border_right: base.border_right || over.border_right,
        border_bottom: base.border_bottom || over.border_bottom,
        border_left: base.border_left || over.border_left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dxf_fill(rgb: &str) -> VisualAttrs {
        VisualAttrs {
            fill_rgb: Some(rgb.into()),
            ..Default::default()
        }
    }

    fn block_cellis(
        range: (u32, u32, u32, u32),
        op: CfOperator,
        operands: Vec<f64>,
        priority: i64,
        dxf: VisualAttrs,
    ) -> CfBlock {
        CfBlock {
            ranges: vec![range],
            rules: vec![CfRule {
                kind: CfRuleKind::CellIs { op, operands },
                priority,
                dxf,
            }],
        }
    }

    #[test]
    fn cellis_greater_than_matches() {
        let cf = SheetCondFmt {
            blocks: vec![block_cellis(
                (0, 0, 2, 0),
                CfOperator::GreaterThan,
                vec![5.0],
                1,
                dxf_fill("#FFFF00"),
            )],
        };
        // 7 > 5 → painted; 3 not.
        let hit = override_for(&cf, 0, 0, &CellValue::Number(7.0), &mut |_| None);
        assert_eq!(hit.unwrap().fill_rgb.as_deref(), Some("#FFFF00"));
        let miss = override_for(&cf, 1, 0, &CellValue::Number(3.0), &mut |_| None);
        assert!(miss.is_none());
        // Outside the range: no override even if the value would match.
        let outside = override_for(&cf, 9, 9, &CellValue::Number(99.0), &mut |_| None);
        assert!(outside.is_none());
        // Non-numeric cell: cf value comparison never matches.
        let text = override_for(&cf, 0, 0, &CellValue::from("x"), &mut |_| None);
        assert!(text.is_none());
    }

    #[test]
    fn cellis_between_inclusive() {
        let cf = SheetCondFmt {
            blocks: vec![block_cellis(
                (0, 0, 0, 0),
                CfOperator::Between,
                vec![10.0, 20.0],
                1,
                dxf_fill("#00FF00"),
            )],
        };
        for (v, want) in [
            (10.0, true),
            (15.0, true),
            (20.0, true),
            (9.9, false),
            (20.1, false),
        ] {
            let got = override_for(&cf, 0, 0, &CellValue::Number(v), &mut |_| None).is_some();
            assert_eq!(got, want, "value {v}");
        }
    }

    #[test]
    fn lowest_priority_dxf_wins() {
        // Two matching cellIs rules; priority 1 beats priority 5.
        let cf = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 0, 0)],
                rules: vec![
                    CfRule {
                        kind: CfRuleKind::CellIs {
                            op: CfOperator::GreaterThan,
                            operands: vec![0.0],
                        },
                        priority: 5,
                        dxf: dxf_fill("#AAAAAA"),
                    },
                    CfRule {
                        kind: CfRuleKind::CellIs {
                            op: CfOperator::GreaterThan,
                            operands: vec![0.0],
                        },
                        priority: 1,
                        dxf: dxf_fill("#BBBBBB"),
                    },
                ],
            }],
        };
        let hit = override_for(&cf, 0, 0, &CellValue::Number(3.0), &mut |_| None).unwrap();
        assert_eq!(hit.fill_rgb.as_deref(), Some("#BBBBBB"));
    }

    #[test]
    fn expression_reduced_comparison_matches() {
        let cf = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 4, 0)],
                rules: vec![CfRule {
                    kind: CfRuleKind::Expression {
                        op: CfOperator::GreaterThan,
                        operand: 100.0,
                    },
                    priority: 1,
                    dxf: dxf_fill("#FF0000"),
                }],
            }],
        };
        assert!(override_for(&cf, 0, 0, &CellValue::Number(150.0), &mut |_| None).is_some());
        assert!(override_for(&cf, 0, 0, &CellValue::Number(50.0), &mut |_| None).is_none());
    }

    #[test]
    fn expression_unsupported_paints_nothing() {
        let cf = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 0, 0)],
                rules: vec![CfRule {
                    kind: CfRuleKind::ExpressionUnsupported,
                    priority: 1,
                    dxf: dxf_fill("#FF0000"),
                }],
            }],
        };
        assert!(override_for(&cf, 0, 0, &CellValue::Number(999.0), &mut |_| None).is_none());
    }

    #[test]
    fn two_color_scale_interpolates_midpoint() {
        // White (#FFFFFF) → red (#FF0000) over domain [0, 100]; at 50 → #FF8080.
        let cf = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 9, 0)],
                rules: vec![CfRule {
                    kind: CfRuleKind::ColorScale(ColorScale {
                        stops: vec![
                            ScaleStop {
                                value: None,
                                rgb: (0xFF, 0xFF, 0xFF),
                            },
                            ScaleStop {
                                value: None,
                                rgb: (0xFF, 0x00, 0x00),
                            },
                        ],
                    }),
                    priority: 1,
                    dxf: VisualAttrs::default(),
                }],
            }],
        };
        let mut domain = |_: (u32, u32, u32, u32)| Some((0.0, 100.0));
        let mid = override_for(&cf, 0, 0, &CellValue::Number(50.0), &mut domain).unwrap();
        // 0xFF, round(0xFF*0.5)=0x80, 0x80.
        assert_eq!(mid.fill_rgb.as_deref(), Some("#FF8080"));
        // At the min stop → white; at the max → red.
        let lo = override_for(&cf, 1, 0, &CellValue::Number(0.0), &mut domain).unwrap();
        assert_eq!(lo.fill_rgb.as_deref(), Some("#FFFFFF"));
        let hi = override_for(&cf, 2, 0, &CellValue::Number(100.0), &mut domain).unwrap();
        assert_eq!(hi.fill_rgb.as_deref(), Some("#FF0000"));
    }

    #[test]
    fn three_color_scale_uses_middle_stop() {
        // 0=red, 50=yellow, 100=green; at 25 halfway red→yellow = #FF8000.
        let cf = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 9, 0)],
                rules: vec![CfRule {
                    kind: CfRuleKind::ColorScale(ColorScale {
                        stops: vec![
                            ScaleStop {
                                value: Some(0.0),
                                rgb: (0xFF, 0x00, 0x00),
                            },
                            ScaleStop {
                                value: Some(50.0),
                                rgb: (0xFF, 0xFF, 0x00),
                            },
                            ScaleStop {
                                value: Some(100.0),
                                rgb: (0x00, 0xFF, 0x00),
                            },
                        ],
                    }),
                    priority: 1,
                    dxf: VisualAttrs::default(),
                }],
            }],
        };
        let mut domain = |_: (u32, u32, u32, u32)| Some((0.0, 100.0));
        let q = override_for(&cf, 0, 0, &CellValue::Number(25.0), &mut domain).unwrap();
        assert_eq!(q.fill_rgb.as_deref(), Some("#FF8000"));
    }

    #[test]
    fn dxf_rule_wins_over_color_scale() {
        // A cell under both a matching cellIs and a colour scale takes the dxf.
        let cf = SheetCondFmt {
            blocks: vec![
                block_cellis(
                    (0, 0, 0, 0),
                    CfOperator::GreaterThan,
                    vec![0.0],
                    2,
                    dxf_fill("#123456"),
                ),
                CfBlock {
                    ranges: vec![(0, 0, 0, 0)],
                    rules: vec![CfRule {
                        kind: CfRuleKind::ColorScale(ColorScale {
                            stops: vec![
                                ScaleStop {
                                    value: None,
                                    rgb: (0, 0, 0),
                                },
                                ScaleStop {
                                    value: None,
                                    rgb: (255, 255, 255),
                                },
                            ],
                        }),
                        priority: 1,
                        dxf: VisualAttrs::default(),
                    }],
                },
            ],
        };
        let mut domain = |_: (u32, u32, u32, u32)| Some((0.0, 10.0));
        let hit = override_for(&cf, 0, 0, &CellValue::Number(5.0), &mut domain).unwrap();
        assert_eq!(hit.fill_rgb.as_deref(), Some("#123456"));
    }

    #[test]
    fn sheet_lower_condfmt_databar_geometry_fraction_over_domain() {
        // A data bar over A1:A5 with derived (min/max) endpoints; the cell's
        // fill fraction is (value - lo) / (hi - lo), clamped, over the domain.
        let cf = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 4, 0)],
                rules: vec![CfRule {
                    kind: CfRuleKind::DataBar(DataBar {
                        min: None,
                        max: None,
                        rgb: (0x63, 0x8E, 0xC6),
                    }),
                    priority: 1,
                    dxf: VisualAttrs::default(),
                }],
            }],
        };
        let mut domain = |_: (u32, u32, u32, u32)| Some((0.0, 100.0));
        // value 0 → 0%, 50 → 50%, 100 → 100% over [0,100].
        let (f0, rgb) = databar_for(&cf, 0, 0, &CellValue::Number(0.0), &mut domain).unwrap();
        assert!((f0 - 0.0).abs() < 1e-9);
        assert_eq!(rgb, (0x63, 0x8E, 0xC6));
        let (f50, _) = databar_for(&cf, 1, 0, &CellValue::Number(50.0), &mut domain).unwrap();
        assert!((f50 - 0.5).abs() < 1e-9);
        let (f100, _) = databar_for(&cf, 2, 0, &CellValue::Number(100.0), &mut domain).unwrap();
        assert!((f100 - 1.0).abs() < 1e-9);
        // A value outside the domain clamps into [0, 1].
        let (over, _) = databar_for(&cf, 3, 0, &CellValue::Number(250.0), &mut domain).unwrap();
        assert!((over - 1.0).abs() < 1e-9);
        // Non-numeric / uncovered cells produce no bar.
        assert!(databar_for(&cf, 0, 0, &CellValue::from("x"), &mut domain).is_none());
        assert!(databar_for(&cf, 9, 9, &CellValue::Number(5.0), &mut domain).is_none());
    }

    #[test]
    fn sheet_lower_condfmt_databar_geometry_explicit_endpoints_and_degenerate() {
        // Explicit num endpoints win over the derived domain.
        let cf = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 0, 0)],
                rules: vec![CfRule {
                    kind: CfRuleKind::DataBar(DataBar {
                        min: Some(10.0),
                        max: Some(20.0),
                        rgb: (1, 2, 3),
                    }),
                    priority: 1,
                    dxf: VisualAttrs::default(),
                }],
            }],
        };
        // The domain fn would say [0,1000], but explicit [10,20] is used: 15 → 0.5.
        let mut domain = |_: (u32, u32, u32, u32)| Some((0.0, 1000.0));
        let (f, _) = databar_for(&cf, 0, 0, &CellValue::Number(15.0), &mut domain).unwrap();
        assert!((f - 0.5).abs() < 1e-9);

        // A degenerate (zero-width) domain paints no bar (no div-by-zero).
        let degen = SheetCondFmt {
            blocks: vec![CfBlock {
                ranges: vec![(0, 0, 0, 0)],
                rules: vec![CfRule {
                    kind: CfRuleKind::DataBar(DataBar {
                        min: Some(5.0),
                        max: Some(5.0),
                        rgb: (0, 0, 0),
                    }),
                    priority: 1,
                    dxf: VisualAttrs::default(),
                }],
            }],
        };
        assert!(databar_for(&degen, 0, 0, &CellValue::Number(5.0), &mut domain).is_none());
    }

    #[test]
    fn fold_override_layers_facets() {
        let base = VisualAttrs {
            bold: true,
            font_name: Some("Calibri".into()),
            fill_rgb: Some("#EEEEEE".into()),
            ..Default::default()
        };
        let over = VisualAttrs {
            italic: true,
            fill_rgb: Some("#FFFF00".into()), // override wins
            text_rgb: Some("#FF0000".into()),
            ..Default::default()
        };
        let f = fold_override(&base, &over);
        assert!(f.bold); // base
        assert!(f.italic); // override
        assert_eq!(f.font_name.as_deref(), Some("Calibri")); // base (override unset)
        assert_eq!(f.fill_rgb.as_deref(), Some("#FFFF00")); // override wins
        assert_eq!(f.text_rgb.as_deref(), Some("#FF0000")); // override
    }
}
