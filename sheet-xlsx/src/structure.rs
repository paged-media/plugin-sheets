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

//! Workbook-STRUCTURE writes (Wave 4): the `<calcPr>` iteration knobs, adding
//! / renaming / deleting a worksheet, and the structural-edit preservation
//! check (which preserved content would a row/column insert or delete leave
//! pointing at the wrong cells).
//!
//! Every write here is a byte splice (`splice.rs`) of the one element it
//! changes — the workbook part, the workbook `.rels` and `[Content_Types].xml`
//! otherwise re-emit verbatim, exactly as the zero-edit round-trip demands.
//!
//! The MODEL side of each operation (the sheet vector, the formula ASTs, the
//! name table) is the consumer's job (`sheet-js` owns the engine and the
//! model); these methods keep the PACKAGE in step with it.

use std::collections::{BTreeMap, BTreeSet};

use sheet_core::calc_settings::CalcSettings;
use sheet_core::{SheetId, SheetModel};

use crate::error::XlsxError;
use crate::opc::{ModeledKind, PartEntry, CONTENT_TYPES_PART};
use crate::preserve::CapturedSubtrees;
use crate::rels::{part_dir, rels_part_for, resolve_target, Relationships};
use crate::sheet_doc::SheetBinding;
use crate::splice::{
    attr_local, attrs_of, children_of, prefix_of, render_tag, replace_start_tag, root_children,
    set_attr, splice, Span,
};
use crate::XlsxDocument;

/// The worksheet content type (`[Content_Types].xml` override).
pub const CT_WORKSHEET: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
/// The workbook → worksheet relationship type.
pub const REL_TYPE_WORKSHEET: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";

/// The `<calcPr>` iteration knobs as written (ECMA-376 §18.2.2). Absent
/// attributes stay `None` so the loader can keep Excel's defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CalcPr {
    pub iterate: Option<bool>,
    pub iterate_count: Option<u32>,
    pub iterate_delta: Option<f64>,
}

impl CalcPr {
    /// Fold into the model's calc settings (absent knobs keep the default).
    pub fn apply(&self, calc: &mut CalcSettings) {
        if let Some(on) = self.iterate {
            calc.iterative = on;
        }
        if let Some(n) = self.iterate_count {
            calc.max_iter = n;
        }
        if let Some(d) = self.iterate_delta {
            calc.max_change = d;
        }
    }
}

/// `<worksheet>` children whose content addresses cells of THIS sheet and
/// which the writer re-emits verbatim — a row/column insert or delete would
/// leave them pointing at the wrong cells. (`<mergeCells>`, `<cols>` and the
/// rows are modelled and shift with the edit; a `<sheetViews>` selection is
/// cosmetic and allowed to go stale.)
const ADDRESSED_CHILDREN: &[&str] = &[
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "autoFilter",
    "sortState",
    "protectedRanges",
    "scenarios",
    "dataConsolidate",
    "customSheetViews",
    "rowBreaks",
    "colBreaks",
    "ignoredErrors",
    "smartTags",
    "cellWatches",
    "tableParts",
    "drawing",
    "legacyDrawing",
    "oleObjects",
    "controls",
    "webPublishItems",
];

/// Format `true`/`false` as the OOXML boolean the writer emits.
fn xml_bool(b: bool) -> String {
    if b { "1" } else { "0" }.to_string()
}

/// Shortest round-trip f64 text (`0.001`, `100`).
fn xml_num(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Rewrite (or insert) the workbook's `<calcPr>` so it carries `calc`'s
/// iteration knobs. Every other attribute (calcId, fullCalcOnLoad, …) and
/// every other byte of the part is kept.
pub fn patch_calc_pr(xml: &[u8], calc: &CalcSettings) -> Result<Vec<u8>, XlsxError> {
    let defaults = CalcSettings::default();
    let (kids, close) = root_children(xml)?;
    if let Some(span) = kids.iter().find(|k| k.local == "calcPr") {
        let mut a = attrs_of(xml, span)?;
        set_attr(&mut a, "iterate", Some(xml_bool(calc.iterative)));
        let keep_count = calc.iterative
            || calc.max_iter != defaults.max_iter
            || attr_local(&a, "iterateCount").is_some();
        set_attr(
            &mut a,
            "iterateCount",
            keep_count.then(|| calc.max_iter.to_string()),
        );
        let keep_delta = calc.iterative
            || calc.max_change != defaults.max_change
            || attr_local(&a, "iterateDelta").is_some();
        set_attr(
            &mut a,
            "iterateDelta",
            keep_delta.then(|| xml_num(calc.max_change)),
        );
        return Ok(replace_start_tag(
            xml,
            span,
            &render_tag(&span.qname, &a, span.empty),
        ));
    }
    // No <calcPr>: insert one in schema position — after the last of
    // sheets / functionGroups / externalReferences / definedNames
    // (ECMA-376 §18.2.27 CT_Workbook child order).
    let anchor = kids
        .iter()
        .filter(|k| {
            matches!(
                k.local.as_str(),
                "sheets" | "functionGroups" | "externalReferences" | "definedNames"
            )
        })
        .map(|k| k.end)
        .max()
        .unwrap_or(close);
    let prefix = kids
        .iter()
        .find(|k| k.local == "sheets")
        .map(|k| prefix_of(&k.qname).to_string())
        .unwrap_or_default();
    let tag = render_tag(
        &format!("{prefix}calcPr"),
        &[
            ("iterate".into(), xml_bool(calc.iterative)),
            ("iterateCount".into(), calc.max_iter.to_string()),
            ("iterateDelta".into(), xml_num(calc.max_change)),
        ],
        true,
    );
    Ok(splice(xml, anchor, anchor, tag.as_bytes()))
}

/// True when an identifier char (a sheet name run continues through it).
fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// Quote a sheet name for a formula when it needs it (`'Q1 Sales'!`).
pub fn quote_sheet(name: &str) -> String {
    let plain = !name.is_empty()
        && name.chars().all(ident_char)
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit());
    if plain {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// Rewrite every `Sheet!` / `'Sheet name'!` prefix naming `old` (case-
/// insensitive, Excel sheet-name semantics) in formula TEXT to `replacement`
/// (already quoted as needed, WITHOUT the `!`). String literals are skipped.
/// Returns the text and whether anything changed.
pub fn rewrite_sheet_refs(text: &str, old: &str, replacement: &str) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            // String literal ("" escapes a quote).
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '"' {
                    if i + 1 < chars.len() && chars[i + 1] == '"' {
                        out.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '\'' {
            // Quoted sheet name: '…' ('' escapes a quote), then `!`.
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() {
                if chars[j] == '\'' {
                    if j + 1 < chars.len() && chars[j + 1] == '\'' {
                        name.push('\'');
                        j += 2;
                        continue;
                    }
                    break;
                }
                name.push(chars[j]);
                j += 1;
            }
            if j + 1 < chars.len() && chars[j + 1] == '!' && name.eq_ignore_ascii_case(old) {
                out.push_str(replacement);
                out.push('!');
                changed = true;
                i = j + 2;
                continue;
            }
            let end = (j + 1).min(chars.len());
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if ident_char(c) && (i == 0 || !ident_char(chars[i - 1])) {
            let mut j = i;
            while j < chars.len() && ident_char(chars[j]) {
                j += 1;
            }
            let run: String = chars[i..j].iter().collect();
            if j < chars.len() && chars[j] == '!' && run.eq_ignore_ascii_case(old) {
                out.push_str(replacement);
                out.push('!');
                changed = true;
                i = j + 1;
                continue;
            }
            out.push_str(&run);
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out, changed)
}

/// Escape element text.
fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl XlsxDocument {
    /// The current bytes of a part (`None` when absent).
    pub(crate) fn part_bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.container.part(name).map(|p| p.bytes().to_vec())
    }

    /// Replace a part's bytes in place (its kind and dirty flag are kept, so
    /// a modelled part keeps re-emitting these bytes verbatim).
    pub(crate) fn set_part_bytes(&mut self, name: &str, bytes: Vec<u8>) {
        match self.container.part_mut(name) {
            Some(PartEntry::Opaque { bytes: b, .. }) => *b = bytes,
            Some(PartEntry::Modeled { raw, .. }) => *raw = bytes,
            None => {}
        }
        self.container.dirty = true;
    }

    /// The `<sheet>` element spans of `workbook.xml`, in tab order, plus the
    /// `<sheets>` span itself.
    fn sheet_spans(&self, wb: &[u8]) -> Result<(Span, Vec<Span>, usize), XlsxError> {
        let (kids, _) = root_children(wb)?;
        let sheets = kids
            .into_iter()
            .find(|k| k.local == "sheets")
            .ok_or_else(|| XlsxError::Structure("workbook has no <sheets>".into()))?;
        let (items, close) = children_of(wb, &sheets)?;
        let items = items.into_iter().filter(|s| s.local == "sheet").collect();
        Ok((sheets, items, close))
    }

    /// The workbook part bytes (it always exists after `open`).
    pub(crate) fn workbook_bytes(&self) -> Result<Vec<u8>, XlsxError> {
        self.part_bytes(&self.workbook_part)
            .ok_or_else(|| XlsxError::Structure("missing workbook part".into()))
    }

    /// Apply `f` to every `<definedName>` element of the workbook part:
    /// `f(attrs, text) -> Some((attrs, text))` rewrites it, `None` drops it.
    fn edit_defined_names(
        &mut self,
        mut f: impl FnMut(&mut Vec<(String, String)>, &str) -> Option<String>,
    ) -> Result<(), XlsxError> {
        let wb = self.workbook_bytes()?;
        let (kids, _) = root_children(&wb)?;
        let Some(dn) = kids.into_iter().find(|k| k.local == "definedNames") else {
            return Ok(());
        };
        let (items, _) = children_of(&wb, &dn)?;
        // Rebuild back to front so earlier offsets stay valid.
        let mut out = wb.clone();
        for item in items.iter().rev().filter(|s| s.local == "definedName") {
            let mut a = attrs_of(&wb, item)?;
            let close_len = format!("</{}>", item.qname).len();
            let raw_text = if item.empty {
                String::new()
            } else {
                let body = &wb[item.tag_end..item.end - close_len];
                let s = String::from_utf8_lossy(body);
                quick_xml::escape::unescape(&s)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| s.into_owned())
            };
            match f(&mut a, &raw_text) {
                Some(text) => {
                    let new = format!(
                        "{}{}</{}>",
                        render_tag(&item.qname, &a, false),
                        escape_text(&text),
                        item.qname
                    );
                    out = splice(&out, item.start, item.end, new.as_bytes());
                }
                None => out = splice(&out, item.start, item.end, b""),
            }
        }
        let wb_part = self.workbook_part.clone();
        self.set_part_bytes(&wb_part, out);
        Ok(())
    }

    /// Splice a `<Relationship>` into the workbook `.rels`; returns its id.
    pub(crate) fn add_workbook_rel(
        &mut self,
        rel_type: &str,
        target: &str,
    ) -> Result<String, XlsxError> {
        let rels_part = self.wb_rels_part.clone();
        let raw = self.part_bytes(&rels_part).unwrap_or_else(|| {
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#
                .to_vec()
        });
        let rels = Relationships::parse(&raw)?;
        let used: BTreeSet<&str> = rels.rels.iter().map(|r| r.id.as_str()).collect();
        let mut n = rels.rels.len() + 1;
        let id = loop {
            let cand = format!("rId{n}");
            if !used.contains(cand.as_str()) {
                break cand;
            }
            n += 1;
        };
        let (_, close) = root_children(&raw)?;
        let tag = render_tag(
            "Relationship",
            &[
                ("Id".into(), id.clone()),
                ("Type".into(), rel_type.into()),
                ("Target".into(), target.into()),
            ],
            true,
        );
        let out = splice(&raw, close, close, tag.as_bytes());
        if self.container.part(&rels_part).is_some() {
            self.set_part_bytes(&rels_part, out);
        } else {
            self.container.parts.push(PartEntry::Modeled {
                name: rels_part,
                kind: ModeledKind::WorkbookRels,
                raw: out,
                dirty: false,
            });
        }
        Ok(id)
    }

    /// Remove the `<Relationship Id=id>` row from a `.rels` part.
    fn remove_rel(&mut self, rels_part: &str, id: &str) -> Result<(), XlsxError> {
        let Some(raw) = self.part_bytes(rels_part) else {
            return Ok(());
        };
        let (kids, _) = root_children(&raw)?;
        for k in kids.iter().rev() {
            if k.local == "Relationship"
                && attr_local(&attrs_of(&raw, k)?, "Id").as_deref() == Some(id)
            {
                let out = splice(&raw, k.start, k.end, b"");
                self.set_part_bytes(rels_part, out);
                break;
            }
        }
        Ok(())
    }

    /// Add a `[Content_Types]` override (model + bytes).
    pub(crate) fn add_override(&mut self, part: &str, content_type: &str) -> Result<(), XlsxError> {
        let pn = format!("/{part}");
        if self.container.content_types.has_override(&pn) {
            return Ok(());
        }
        self.container
            .content_types
            .overrides
            .push((pn.clone(), content_type.to_string()));
        if let Some(raw) = self.part_bytes(CONTENT_TYPES_PART) {
            let (_, close) = root_children(&raw)?;
            let tag = render_tag(
                "Override",
                &[
                    ("PartName".into(), pn),
                    ("ContentType".into(), content_type.into()),
                ],
                true,
            );
            self.set_part_bytes(
                CONTENT_TYPES_PART,
                splice(&raw, close, close, tag.as_bytes()),
            );
        }
        Ok(())
    }

    /// Remove a `[Content_Types]` override (model + bytes).
    fn remove_override(&mut self, part: &str) -> Result<(), XlsxError> {
        let pn = format!("/{part}");
        self.container
            .content_types
            .overrides
            .retain(|(p, _)| p != &pn);
        if let Some(raw) = self.part_bytes(CONTENT_TYPES_PART) {
            let (kids, _) = root_children(&raw)?;
            for k in kids.iter().rev() {
                if k.local == "Override"
                    && attr_local(&attrs_of(&raw, k)?, "PartName").as_deref() == Some(pn.as_str())
                {
                    self.set_part_bytes(CONTENT_TYPES_PART, splice(&raw, k.start, k.end, b""));
                    break;
                }
            }
        }
        Ok(())
    }

    /// ADD a worksheet part for model sheet `sheet_id` (which the consumer has
    /// just appended to the model as the LAST sheet) named `name`: a new
    /// `xl/worksheets/sheetN.xml` (encoded from the model on save), its
    /// workbook relationship, its content-type override and its `<sheet>`
    /// row. Nothing else in the package changes.
    pub fn add_sheet_part(&mut self, sheet_id: SheetId, name: &str) -> Result<(), XlsxError> {
        let wb_dir = part_dir(&self.workbook_part);
        let mut n = 1;
        let (part, target) = loop {
            let target = format!("worksheets/sheet{n}.xml");
            let part = resolve_target(&wb_dir, &target);
            if self.container.part(&part).is_none() {
                break (part, target);
            }
            n += 1;
        };
        let rid = self.add_workbook_rel(REL_TYPE_WORKSHEET, &target)?;
        self.add_override(&part, CT_WORKSHEET)?;

        let wb = self.workbook_bytes()?;
        let (sheets, items, close) = self.sheet_spans(&wb)?;
        let mut next_id = 1u32;
        let mut rid_key = "r:id".to_string();
        for s in &items {
            let a = attrs_of(&wb, s)?;
            if let Some(v) = attr_local(&a, "sheetId").and_then(|v| v.parse::<u32>().ok()) {
                next_id = next_id.max(v + 1);
            }
            if let Some((k, _)) = a.iter().find(|(k, _)| k.ends_with(":id")) {
                rid_key = k.clone();
            }
        }
        let tag = render_tag(
            &format!("{}sheet", prefix_of(&sheets.qname)),
            &[
                ("name".into(), name.to_string()),
                ("sheetId".into(), next_id.to_string()),
                (rid_key, rid),
            ],
            true,
        );
        let out = if sheets.empty {
            return Err(XlsxError::Structure("workbook <sheets/> is empty".into()));
        } else {
            splice(&wb, close, close, tag.as_bytes())
        };
        let wb_part = self.workbook_part.clone();
        self.set_part_bytes(&wb_part, out);

        self.container.parts.push(PartEntry::Modeled {
            name: part.clone(),
            kind: ModeledKind::Worksheet,
            raw: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#
                .to_vec(),
            dirty: true,
        });
        self.bindings.push(SheetBinding {
            sheet_id,
            part_name: part,
            captured: CapturedSubtrees::default(),
        });
        self.container.dirty = true;
        Ok(())
    }

    /// RENAME the `<sheet>` row of model sheet `sheet_id` from `old` to `new`,
    /// and rewrite every defined name's text that names the sheet.
    pub fn rename_sheet_part(
        &mut self,
        sheet_id: SheetId,
        old: &str,
        new: &str,
    ) -> Result<(), XlsxError> {
        let wb = self.workbook_bytes()?;
        let (_, items, _) = self.sheet_spans(&wb)?;
        let span = items
            .get(sheet_id as usize)
            .ok_or_else(|| XlsxError::Structure(format!("no <sheet> row {sheet_id}")))?;
        let mut a = attrs_of(&wb, span)?;
        set_attr(&mut a, "name", Some(new.to_string()));
        let out = replace_start_tag(&wb, span, &render_tag(&span.qname, &a, span.empty));
        let wb_part = self.workbook_part.clone();
        self.set_part_bytes(&wb_part, out);

        let quoted = quote_sheet(new);
        self.edit_defined_names(|_, text| Some(rewrite_sheet_refs(text, old, &quoted).0))?;
        Ok(())
    }

    /// DELETE model sheet `sheet_id` (named `name`) from the package: its
    /// `<sheet>` row, workbook relationship, worksheet part, the parts only it
    /// referenced (recursively — its comments, drawing, chart, table parts),
    /// their content-type overrides; sheet-local defined names are dropped,
    /// later sheets' `localSheetId` renumbered, references to the sheet in
    /// defined names become `#REF!`, and `activeTab`/`firstSheet` stay in
    /// range. Bindings and every per-sheet side table shift down by one.
    pub fn remove_sheet_part(&mut self, sheet_id: SheetId, name: &str) -> Result<(), XlsxError> {
        let idx = sheet_id as usize;
        let wb = self.workbook_bytes()?;
        let (_, items, _) = self.sheet_spans(&wb)?;
        if items.len() <= 1 {
            return Err(XlsxError::Structure(
                "a workbook must keep at least one sheet".into(),
            ));
        }
        let span = items
            .get(idx)
            .ok_or_else(|| XlsxError::Structure(format!("no <sheet> row {sheet_id}")))?
            .clone();
        let rid = attr_local(&attrs_of(&wb, &span)?, "id").unwrap_or_default();
        let remaining = items.len() - 1;

        // 1. The <sheet> row, then activeTab / firstSheet.
        let mut out = splice(&wb, span.start, span.end, b"");
        let (kids, _) = root_children(&out)?;
        if let Some(bv) = kids.iter().find(|k| k.local == "bookViews") {
            let (views, _) = children_of(&out, bv)?;
            for v in views.iter().rev() {
                let mut a = attrs_of(&out, v)?;
                let mut touched = false;
                for key in ["activeTab", "firstSheet"] {
                    if let Some(n) = attr_local(&a, key).and_then(|s| s.parse::<usize>().ok()) {
                        let shifted = if n > idx || (n == idx && n >= remaining) {
                            n.saturating_sub(1)
                        } else {
                            n
                        };
                        if shifted != n {
                            set_attr(&mut a, key, Some(shifted.to_string()));
                            touched = true;
                        }
                    }
                }
                if touched {
                    out = replace_start_tag(&out, v, &render_tag(&v.qname, &a, v.empty));
                }
            }
        }
        let wb_part = self.workbook_part.clone();
        self.set_part_bytes(&wb_part, out);

        // 2. Defined names: drop the deleted sheet's locals, renumber later
        //    locals, #REF! the references.
        let mut drop_err: Option<XlsxError> = None;
        self.edit_defined_names(|a, text| {
            if let Some(n) = attr_local(a, "localSheetId").and_then(|s| s.parse::<usize>().ok()) {
                if n == idx {
                    return None;
                }
                if n > idx {
                    set_attr(a, "localSheetId", Some((n - 1).to_string()));
                }
            }
            Some(rewrite_sheet_refs(text, name, "#REF").0)
        })
        .unwrap_or_else(|e| drop_err = Some(e));
        if let Some(e) = drop_err {
            return Err(e);
        }

        // 3. The relationship + the worksheet part and what only it reached.
        let rels_part = self.wb_rels_part.clone();
        self.remove_rel(&rels_part, &rid)?;
        let ws_part = self
            .bindings
            .iter()
            .find(|b| b.sheet_id == sheet_id)
            .map(|b| b.part_name.clone())
            .ok_or_else(|| XlsxError::Structure(format!("sheet {sheet_id} has no part")))?;
        for part in self.parts_only_reached_from(&ws_part)? {
            self.container.parts.retain(|p| p.name() != part);
            self.remove_override(&part)?;
        }

        // 4. Bindings + per-sheet side tables shift down.
        self.bindings.retain(|b| b.sheet_id != sheet_id);
        for b in &mut self.bindings {
            if b.sheet_id > sheet_id {
                b.sheet_id -= 1;
            }
        }
        fn shift<V>(m: &mut BTreeMap<SheetId, V>, gone: SheetId) {
            let old = std::mem::take(m);
            for (k, v) in old {
                if k < gone {
                    m.insert(k, v);
                } else if k > gone {
                    m.insert(k - 1, v);
                }
            }
        }
        shift(&mut self.conditional_formats, sheet_id);
        shift(&mut self.freeze_panes, sheet_id);
        shift(&mut self.data_validations, sheet_id);
        shift(&mut self.comments, sheet_id);
        let ft = std::mem::take(&mut self.formula_texts);
        for ((s, r, c), t) in ft {
            if s < sheet_id {
                self.formula_texts.insert((s, r, c), t);
            } else if s > sheet_id {
                self.formula_texts.insert((s - 1, r, c), t);
            }
        }
        self.container.dirty = true;
        Ok(())
    }

    /// `root` plus its `.rels` plus every part reachable from it through
    /// relationships that NO part outside that set also references.
    fn parts_only_reached_from(&self, root: &str) -> Result<Vec<String>, XlsxError> {
        // Every (owner part → resolved target) edge in the package.
        let mut edges: Vec<(String, String)> = Vec::new();
        for p in &self.container.parts {
            let name = p.name();
            let Some(dir_end) = name.find("_rels/") else {
                continue;
            };
            if !name.ends_with(".rels") {
                continue;
            }
            let owner_dir = &name[..dir_end];
            let owner_file = &name[dir_end + "_rels/".len()..name.len() - ".rels".len()];
            let owner = format!("{owner_dir}{owner_file}");
            let rels = Relationships::parse(p.bytes())?;
            for r in &rels.rels {
                edges.push((owner.clone(), resolve_target(owner_dir, &r.target)));
            }
        }
        let mut set: BTreeSet<String> = BTreeSet::new();
        set.insert(root.to_string());
        let mut work = vec![root.to_string()];
        while let Some(p) = work.pop() {
            for (owner, target) in &edges {
                if owner != &p || set.contains(target) || self.container.part(target).is_none() {
                    continue;
                }
                // Shared with something outside the set? Keep it.
                let shared = edges
                    .iter()
                    .any(|(o, t)| t == target && !set.contains(o) && o != &p);
                if !shared {
                    set.insert(target.clone());
                    work.push(target.clone());
                }
            }
        }
        let mut out: Vec<String> = Vec::new();
        for p in &set {
            out.push(p.clone());
            let rels = rels_part_for(p);
            if self.container.part(&rels).is_some() {
                out.push(rels);
            }
        }
        Ok(out)
    }

    /// The preserved content that a row/column insert or delete on `sheet_id`
    /// cannot carry along — each entry names one blocker. EMPTY means the
    /// structural edit is safe for the package (the modelled cells, merges,
    /// column widths and row heights shift with the edit; formulas are
    /// re-printed by the consumer).
    pub fn structural_edit_blockers(&self, sheet_id: SheetId, model: &SheetModel) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(b) = self.bindings.iter().find(|b| b.sheet_id == sheet_id) {
            for c in &b.captured.items {
                let local = element_local_name(&c.bytes);
                if ADDRESSED_CHILDREN.contains(&local.as_str()) {
                    out.push(format!("<{local}>"));
                } else if local == "extLst" {
                    let s = String::from_utf8_lossy(&c.bytes);
                    if s.contains("sqref") || s.contains(":f>") {
                        out.push("<extLst> with cell references".into());
                    }
                }
            }
            let rels_part = rels_part_for(&b.part_name);
            if let Some(p) = self.container.part(&rels_part) {
                if let Ok(rels) = Relationships::parse(p.bytes()) {
                    for kind in ["/table", "/drawing", "/comments", "/pivotTable"] {
                        if rels.rels.iter().any(|r| r.is_type(kind)) {
                            out.push(format!("{} part", &kind[1..]));
                        }
                    }
                }
            }
        }
        if self.charts.iter().any(|c| {
            c.host_sheet == sheet_id
                || c.model.series.iter().any(|s| {
                    s.values.start.sheet == sheet_id
                        || s.categories.is_some_and(|r| r.start.sheet == sheet_id)
                })
        }) {
            out.push("chart".into());
        }
        if let Some(ws) = model.sheet(sheet_id) {
            for (_, def) in model.names.iter() {
                if let sheet_core::names::NameTarget::Formula(text) = &def.target {
                    if rewrite_sheet_refs(text, &ws.name, "X").1 {
                        out.push(format!("defined name {}", def.name));
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The `<calcPr>` the workbook part needs for `model`'s calc settings, or
    /// `None` when the knobs are what the file was loaded with (the part then
    /// stays verbatim). `model` is the one being saved (the session lends the
    /// engine's — see [`XlsxDocument::save_model`]).
    pub(crate) fn calc_pr_override(
        &self,
        model: &sheet_core::SheetModel,
    ) -> Result<Option<Vec<u8>>, XlsxError> {
        let now = &model.calc;
        let was = &self.loaded_calc;
        if now.iterative == was.iterative
            && now.max_iter == was.max_iter
            && now.max_change == was.max_change
        {
            return Ok(None);
        }
        let wb = self.workbook_bytes()?;
        Ok(Some(patch_calc_pr(&wb, now)?))
    }
}

/// The local name of the element a captured subtree starts with.
fn element_local_name(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s = s.trim_start();
    let s = s.strip_prefix('<').unwrap_or(s);
    let end = s
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(s.len());
    let q = &s[..end];
    q.rsplit(':').next().unwrap_or(q).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_pr_patched_in_place_or_inserted_in_schema_position() {
        let calc = CalcSettings {
            iterative: true,
            max_iter: 50,
            max_change: 0.0001,
            ..CalcSettings::default()
        };
        let with = br#"<workbook><sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="x">A!$A$1</definedName></definedNames><calcPr calcId="191029"/><extLst/></workbook>"#;
        let out = String::from_utf8(patch_calc_pr(with, &calc).unwrap()).unwrap();
        assert!(out.contains(
            r#"<calcPr calcId="191029" iterate="1" iterateCount="50" iterateDelta="0.0001"/><extLst/>"#
        ));
        let without = br#"<workbook><sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="x">A!$A$1</definedName></definedNames><extLst/></workbook>"#;
        let out = String::from_utf8(patch_calc_pr(without, &calc).unwrap()).unwrap();
        assert!(out.contains(
            r#"</definedNames><calcPr iterate="1" iterateCount="50" iterateDelta="0.0001"/><extLst/>"#
        ));
    }

    #[test]
    fn sheet_refs_rewrite_quoted_unquoted_and_skip_strings() {
        let (t, ch) = rewrite_sheet_refs(
            "Data!A1+'Data'!B2+\"Data!x\"+MyData!C3",
            "data",
            "'New Data'",
        );
        assert!(ch);
        assert_eq!(t, "'New Data'!A1+'New Data'!B2+\"Data!x\"+MyData!C3");
        let (t, _) = rewrite_sheet_refs("SUM('Q1 Sales'!A1:A3)", "Q1 Sales", "#REF");
        assert_eq!(t, "SUM(#REF!A1:A3)");
        assert_eq!(quote_sheet("Plain_1"), "Plain_1");
        assert_eq!(quote_sheet("It's"), "'It''s'");
        assert_eq!(quote_sheet("1st"), "'1st'");
    }

    #[test]
    fn captured_local_name() {
        assert_eq!(
            element_local_name(b"<x:hyperlinks><a/></x:hyperlinks>"),
            "hyperlinks"
        );
        assert_eq!(element_local_name(b"<extLst>"), "extLst");
    }
}
