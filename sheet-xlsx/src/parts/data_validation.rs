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

//! `<dataValidations>` — data-validation rules on a worksheet (ECMA-376
//! §18.3.1.32 `dataValidations`, §18.3.1.33 `dataValidation`).
//!
//! ## PUBLISHING-FIRST SCOPE — PRESERVE, NEVER INTERPRET (the constitution)
//!
//! Data validation is on the spec's PERMANENT exclusion list (§1.1 / §11 /
//! T∞): *"pivot tables, **data validation**, what-if, external links … round-
//! trip preserved, **none interpreted, rendered, or editable**."* This is a
//! product decision, not a deferral. So:
//!
//! - **PRESERVE (the launch property).** `<dataValidations>` is an unmodeled
//!   `<worksheet>` child captured verbatim (`Anchor::AfterSheetData`), so it
//!   round-trips BYTE-IDENTICAL whether or not this module runs. This parse is
//!   ADDITIVE + READ-ONLY and never writes back — the same discipline as the cf
//!   model / `VisualStyles`.
//! - **NO RUNTIME ENFORCEMENT, NO GRID RENDERING.** We do NOT block edits, we
//!   do NOT render dropdown affordances in the grid, we do NOT evaluate the
//!   constraints. Per the spec these are never "rendered or editable".
//! - **What this read IS for:** a read-only INVENTORY a panel can show so a
//!   user knows the workbook *carries* validations that Paged preserves but
//!   does not enforce (preservation transparency, not interpretation). The
//!   panel surfaces "this sheet has N data validations (preserved, not
//!   enforced)" — nothing more.
//!
//! NOTE (scope decision recorded in the registry row + reported to the
//! orchestrator): the base spec forbids RENDERING the dropdown affordance, so
//! we stop at the preserve-and-inventory line — strictly less than a runtime
//! dropdown. If the product later re-rules validation into scope, the parsed
//! model here is the foundation.

use crate::error::XlsxError;
use crate::opc::attr;
use sheet_core::parse_a1;

/// The `type` of a data validation (ECMA-376 §18.18.20 ST_DataValidationType).
/// Kept as a small enum for the inventory summary; `Custom`/`List`/whole/decimal
/// /date/time/textLength are the publishing-relevant kinds, everything else is
/// `Other` (the raw token preserved on the original bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DvKind {
    /// `list` — an in-cell dropdown over an explicit list or a range.
    List,
    /// `whole` — a whole-number constraint.
    Whole,
    /// `decimal` — a decimal-number constraint.
    Decimal,
    /// `date` — a date constraint.
    Date,
    /// `time` — a time constraint.
    Time,
    /// `textLength` — a text-length constraint.
    TextLength,
    /// `custom` — a formula predicate.
    Custom,
    /// Any other / `none` type token (preserved verbatim on the bytes).
    Other,
}

impl DvKind {
    /// Parse the `type=` token.
    pub fn parse(s: &str) -> DvKind {
        match s {
            "list" => DvKind::List,
            "whole" => DvKind::Whole,
            "decimal" => DvKind::Decimal,
            "date" => DvKind::Date,
            "time" => DvKind::Time,
            "textLength" => DvKind::TextLength,
            "custom" => DvKind::Custom,
            _ => DvKind::Other,
        }
    }

    /// The lowercase tag (for the panel inventory + serde).
    pub fn tag(self) -> &'static str {
        match self {
            DvKind::List => "list",
            DvKind::Whole => "whole",
            DvKind::Decimal => "decimal",
            DvKind::Date => "date",
            DvKind::Time => "time",
            DvKind::TextLength => "textLength",
            DvKind::Custom => "custom",
            DvKind::Other => "other",
        }
    }
}

/// One parsed `<dataValidation>` rule — READ-ONLY inventory (NOT enforced, NOT
/// rendered as a runtime dropdown; spec §1.1 preserve-only). Carries the kind,
/// the ranges it applies to, and the (raw, un-evaluated) operand formulas — the
/// minimum a panel needs to SHOW that the validation exists.
#[derive(Clone, Debug, PartialEq)]
pub struct DataValidation {
    pub kind: DvKind,
    /// The `operator=` token (`between`, `greaterThan`, …) as a raw string, or
    /// `None`. We never evaluate it; it rides for the inventory display.
    pub operator: Option<String>,
    /// The ranges (`sqref`) this rule applies to, `(r0, c0, r1, c1)` 0-based
    /// inclusive; a single-cell sqref yields a 1x1 box.
    pub ranges: Vec<(u32, u32, u32, u32)>,
    /// The raw `<formula1>` text (e.g. `"Yes,No,Maybe"` for a list, `1` for a
    /// whole-number min), un-parsed + un-evaluated. `None` when absent.
    pub formula1: Option<String>,
    /// The raw `<formula2>` text (the upper bound for `between`), un-evaluated.
    pub formula2: Option<String>,
}

/// All data-validation rules parsed from one worksheet (READ-ONLY inventory).
/// Empty for a sheet with no `<dataValidations>`. The `<dataValidations>` XML
/// round-trips byte-identical via the worksheet verbatim capture regardless.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetDataValidations {
    pub rules: Vec<DataValidation>,
}

impl SheetDataValidations {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The count of validation rules (the headline inventory number).
    pub fn len(&self) -> usize {
        self.rules.len()
    }
}

/// Parse one captured `<dataValidations>` subtree (the verbatim worksheet bytes)
/// into its rules. Returns `None` if the element is not a `dataValidations`
/// block (so the caller can feed every captured AfterSheetData subtree and keep
/// only the dv ones). A malformed rule is skipped (the bytes still round-trip).
pub fn parse_block(xml: &[u8]) -> Result<Option<SheetDataValidations>, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    let mut is_dv = false;
    let mut rules: Vec<DataValidation> = Vec::new();
    let mut cur: Option<RuleAccum> = None;
    let mut text_target: Option<u8> = None; // 1 = formula1, 2 = formula2
    let mut text = String::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            // An OPEN tag: <dataValidation> wraps formula children; <formula1/2>
            // open a text target.
            Event::Start(e) => match e.local_name().as_ref() {
                b"dataValidations" => is_dv = true,
                b"dataValidation" => cur = Some(RuleAccum::start(&e)?),
                b"formula1" => {
                    text_target = Some(1);
                    text.clear();
                }
                b"formula2" => {
                    text_target = Some(2);
                    text.clear();
                }
                _ => {}
            },
            // A SELF-CLOSING tag: a <dataValidation .../> with no formula
            // children (rare) is finalized here; an empty <dataValidations/> is
            // still a dv block (zero rules).
            Event::Empty(e) => match e.local_name().as_ref() {
                b"dataValidations" => is_dv = true,
                b"dataValidation" => rules.push(RuleAccum::start(&e)?.finish()),
                _ => {}
            },
            Event::Text(t) => {
                if text_target.is_some() {
                    text.push_str(&t.unescape().map_err(XlsxError::Xml)?);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"formula1" => {
                    if let Some(c) = cur.as_mut() {
                        c.formula1 = Some(text.trim().to_string());
                    }
                    text_target = None;
                }
                b"formula2" => {
                    if let Some(c) = cur.as_mut() {
                        c.formula2 = Some(text.trim().to_string());
                    }
                    text_target = None;
                }
                b"dataValidation" => {
                    if let Some(c) = cur.take() {
                        rules.push(c.finish());
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if !is_dv {
        return Ok(None);
    }
    Ok(Some(SheetDataValidations { rules }))
}

/// Parse all data-validation rules out of an iterator of captured subtree byte
/// slices (the worksheet's AfterSheetData captures). Non-dv subtrees are
/// ignored. Rules keep their source order across blocks.
pub fn parse_all<'a>(
    captured: impl Iterator<Item = &'a [u8]>,
) -> Result<SheetDataValidations, XlsxError> {
    let mut rules = Vec::new();
    for bytes in captured {
        if let Some(block) = parse_block(bytes)? {
            rules.extend(block.rules);
        }
    }
    Ok(SheetDataValidations { rules })
}

/// Mutable accumulator for one `<dataValidation>` while its children stream in.
struct RuleAccum {
    kind: DvKind,
    operator: Option<String>,
    ranges: Vec<(u32, u32, u32, u32)>,
    formula1: Option<String>,
    formula2: Option<String>,
}

impl RuleAccum {
    fn start(e: &quick_xml::events::BytesStart<'_>) -> Result<RuleAccum, XlsxError> {
        let kind = DvKind::parse(&attr(e, b"type")?.unwrap_or_default());
        let operator = attr(e, b"operator")?.filter(|s| !s.is_empty());
        let ranges = attr(e, b"sqref")?
            .map(|s| parse_sqref(&s))
            .unwrap_or_default();
        Ok(RuleAccum {
            kind,
            operator,
            ranges,
            formula1: None,
            formula2: None,
        })
    }

    fn finish(self) -> DataValidation {
        DataValidation {
            kind: self.kind,
            operator: self.operator,
            ranges: self.ranges,
            formula1: self.formula1,
            formula2: self.formula2,
        }
    }
}

/// Parse a `sqref` (space-separated A1 ranges) into 0-based inclusive boxes.
fn parse_sqref(sqref: &str) -> Vec<(u32, u32, u32, u32)> {
    sqref.split_whitespace().filter_map(parse_one_range).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_validation() {
        let xml = br#"<dataValidations count="1">
  <dataValidation type="list" allowBlank="1" sqref="B2:B4">
    <formula1>"Yes,No,Maybe"</formula1>
  </dataValidation>
</dataValidations>"#;
        let dv = parse_block(xml).unwrap().expect("is a dv block");
        assert_eq!(dv.rules.len(), 1);
        let r = &dv.rules[0];
        assert_eq!(r.kind, DvKind::List);
        assert_eq!(r.ranges, vec![(1, 1, 3, 1)]);
        assert_eq!(r.formula1.as_deref(), Some("\"Yes,No,Maybe\""));
        assert!(r.formula2.is_none());
    }

    #[test]
    fn parse_whole_between_two_formulas() {
        let xml = br#"<dataValidations count="1">
  <dataValidation type="whole" operator="between" sqref="B6">
    <formula1>1</formula1><formula2>100</formula2>
  </dataValidation>
</dataValidations>"#;
        let dv = parse_block(xml).unwrap().unwrap();
        let r = &dv.rules[0];
        assert_eq!(r.kind, DvKind::Whole);
        assert_eq!(r.operator.as_deref(), Some("between"));
        assert_eq!(r.formula1.as_deref(), Some("1"));
        assert_eq!(r.formula2.as_deref(), Some("100"));
    }

    #[test]
    fn parse_date_validation() {
        let xml = br#"<dataValidations count="1">
  <dataValidation type="date" operator="greaterThan" sqref="B8">
    <formula1>43000</formula1></dataValidation>
</dataValidations>"#;
        let dv = parse_block(xml).unwrap().unwrap();
        assert_eq!(dv.rules[0].kind, DvKind::Date);
        assert_eq!(dv.rules[0].ranges, vec![(7, 1, 7, 1)]);
    }

    #[test]
    fn multiple_rules_and_kind_tags() {
        let xml = br#"<dataValidations count="3">
  <dataValidation type="list" sqref="A1"><formula1>"x,y"</formula1></dataValidation>
  <dataValidation type="decimal" operator="lessThan" sqref="A2"><formula1>9.5</formula1></dataValidation>
  <dataValidation type="custom" sqref="A3"><formula1>ISNUMBER(A3)</formula1></dataValidation>
</dataValidations>"#;
        let dv = parse_block(xml).unwrap().unwrap();
        assert_eq!(dv.len(), 3);
        let tags: Vec<&str> = dv.rules.iter().map(|r| r.kind.tag()).collect();
        assert_eq!(tags, vec!["list", "decimal", "custom"]);
    }

    #[test]
    fn non_dv_subtree_returns_none() {
        let xml = br#"<pageMargins left="0.7"/>"#;
        assert_eq!(parse_block(xml).unwrap(), None);
    }

    #[test]
    fn parse_all_filters_to_dv_blocks() {
        let dv = br#"<dataValidations count="1"><dataValidation type="list" sqref="A1"><formula1>"a,b"</formula1></dataValidation></dataValidations>"#;
        let pm = br#"<pageMargins left="0.7"/>"#;
        let subtrees: Vec<&[u8]> = vec![pm, dv, pm];
        let parsed = parse_all(subtrees.into_iter()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(!parsed.is_empty());
    }
}
