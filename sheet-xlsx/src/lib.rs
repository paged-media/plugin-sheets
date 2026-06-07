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

//! # sheet-xlsx — XLSX I/O with the preservation invariant (spec §10)
//!
//! Round-trip-first XLSX: a full OPC/zip + SpreadsheetML parse layer, the
//! preservation invariant ("Paged never destroys a workbook"), and a writer
//! whose zero-edit round-trip is **per-part byte-identical** (the dropped
//! `calcChain.xml` excepted — ruling `sheet.xlsx.calcchain.drop`).
//!
//! ## Architecture
//!
//! - [`opc`]  — the zip container: ordered [`opc::PartEntry`] list +
//!   `[Content_Types].xml`. Understood parts are `Modeled`; everything else
//!   is `Opaque` (preserved byte-identical).
//! - [`rels`] — `.rels` relationship resolution (root → workbook → parts).
//! - [`parts`] — the four understood parts: `workbook`, `worksheet`,
//!   `shared_strings`, `styles`.
//! - [`preserve`] — captured unknown `<worksheet>` subtrees for re-emission
//!   on a dirty re-encode.
//! - [`write`] — the writer (lazy-verbatim + dirty re-encode + calcChain
//!   drop).
//!
//! ## Dependency rule
//!
//! `sheet-xlsx -> sheet-core` only. Formulas are **not** parsed here (that
//! is the consumer `sheet-js`'s job): the raw `<f>` text of every formula
//! cell is exposed in [`XlsxDocument::formula_texts`], keyed by
//! `(sheet, row, col)`. The consumer parses these into model
//! `FormulaId`s on load and writes printed text back before save.

pub mod error;
pub mod opc;
pub mod parts;
pub mod preserve;
pub mod rels;
pub mod sheet_doc;
pub mod write;

pub use error::XlsxError;

use opc::{ModeledKind, OpcContainer, PartEntry};
use rels::{
    part_dir, rels_part_for, resolve_target, Relationships, REL_OFFICE_DOCUMENT,
    REL_SHARED_STRINGS, REL_STYLES,
};
use sheet_core::cell::{Cell, StyleId};
use sheet_core::{SheetId, SheetModel};
use sheet_doc::SheetBinding;
use std::collections::BTreeMap;

/// An open XLSX workbook: the parsed [`SheetModel`], the raw formula text per
/// cell, and (privately) the OPC container + per-sheet bindings that make a
/// faithful re-write possible.
pub struct XlsxDocument {
    /// The parsed workbook model.
    pub model: SheetModel,
    /// Raw formula text per cell `(sheet, row, col)` — sheet-xlsx does NOT
    /// parse formulas (dep rule: xlsx -> core only). The consumer (sheet-js)
    /// parses these into model `FormulaId`s on load and writes printed text
    /// back here before save for edited cells.
    pub formula_texts: BTreeMap<(SheetId, u32, u32), String>,

    /// The OPC container (ordered parts + content types) for re-write.
    container: OpcContainer,
    /// Worksheet bindings: model sheet -> part name + captured subtrees.
    bindings: Vec<SheetBinding>,
}

impl XlsxDocument {
    /// Parse `bytes` into a workbook. Understood parts populate the model;
    /// everything else is preserved opaquely. Errors only on content that is
    /// not a usable SpreadsheetML package at all (never on *unknown* content).
    pub fn open(bytes: &[u8]) -> Result<XlsxDocument, XlsxError> {
        let mut container = OpcContainer::read(bytes)?;

        // 1. Root rels -> the officeDocument (workbook) part.
        let root_rels_bytes = container
            .part("_rels/.rels")
            .ok_or_else(|| XlsxError::Structure("missing _rels/.rels".into()))?
            .bytes()
            .to_vec();
        let root_rels = Relationships::parse(&root_rels_bytes)?;
        let wb_target = root_rels
            .by_type(REL_OFFICE_DOCUMENT)
            .ok_or_else(|| XlsxError::Structure("no officeDocument relationship".into()))?
            .target
            .clone();
        // The root rels target is relative to the package root.
        let workbook_part = resolve_target("", &wb_target);

        // 2. Workbook part + its rels.
        let workbook_bytes = container
            .part(&workbook_part)
            .ok_or_else(|| XlsxError::Structure(format!("missing workbook {workbook_part}")))?
            .bytes()
            .to_vec();
        let parsed_wb = parts::workbook::parse(&workbook_bytes)?;

        let wb_rels_part = rels_part_for(&workbook_part);
        let wb_rels = match container.part(&wb_rels_part) {
            Some(p) => Relationships::parse(p.bytes())?,
            None => Relationships::default(),
        };
        let wb_base = part_dir(&workbook_part);

        // 3. sharedStrings + styles (resolved through the workbook rels).
        let shared_part = wb_rels
            .by_type(REL_SHARED_STRINGS)
            .map(|r| resolve_target(&wb_base, &r.target));
        let styles_part = wb_rels
            .by_type(REL_STYLES)
            .map(|r| resolve_target(&wb_base, &r.target));

        let shared_strings = match &shared_part {
            Some(name) => match container.part(name) {
                Some(p) => parts::shared_strings::parse(p.bytes())?,
                None => Vec::new(),
            },
            None => Vec::new(),
        };

        let mut model = SheetModel::new();
        model.calc.date_system = parsed_wb.date_system;
        model.names = parsed_wb.names;

        // Intern shared strings into the model interner (the cells already
        // carry the resolved text; this keeps the interner populated for the
        // consumer).
        for s in &shared_strings {
            model.strings.intern(s.clone());
        }

        // styles: build the cellXfs index -> StyleId table.
        let xf_to_style = match &styles_part {
            Some(name) => match container.part(name) {
                Some(p) => parts::styles::parse(p.bytes(), &mut model.styles)?.xf_to_style,
                None => Vec::new(),
            },
            None => Vec::new(),
        };

        // 4. Each worksheet, in workbook (tab) order.
        let mut bindings = Vec::with_capacity(parsed_wb.sheets.len());
        let mut formula_texts: BTreeMap<(SheetId, u32, u32), String> = BTreeMap::new();
        let mut worksheet_parts: Vec<String> = Vec::new();

        for sref in &parsed_wb.sheets {
            let target = wb_rels.target_of(&sref.rid).ok_or_else(|| {
                XlsxError::Structure(format!(
                    "sheet {} has dangling r:id {}",
                    sref.name, sref.rid
                ))
            })?;
            let ws_part = resolve_target(&wb_base, target);
            let ws_bytes = container
                .part(&ws_part)
                .ok_or_else(|| XlsxError::Structure(format!("missing worksheet {ws_part}")))?
                .bytes()
                .to_vec();
            let parsed_ws = parts::worksheet::parse(&ws_bytes, &shared_strings)?;

            let sid = model.add_sheet(sref.name.as_str());
            {
                let ws = model.sheet_mut(sid).expect("just added");
                ws.merges = parsed_ws.merges;
                ws.col_widths = parsed_ws.col_widths;
                ws.row_heights = parsed_ws.row_heights;
            }

            // Cells. Resolve style index -> StyleId; stash formula text in
            // formula_texts (NOT a model FormulaId — the consumer parses it).
            let mut cells = BTreeMap::new();
            for pc in parsed_ws.cells {
                let style = xf_to_style
                    .get(pc.style_index as usize)
                    .copied()
                    .unwrap_or(StyleId(0));
                if let Some(ftext) = pc.formula {
                    formula_texts.insert((sid, pc.row, pc.col), ftext);
                }
                cells.insert(
                    (pc.row, pc.col),
                    Cell {
                        value: pc.value,
                        // No FormulaId yet — the consumer interns parsed ASTs.
                        formula: None,
                        style,
                    },
                );
            }
            model.sheet_mut(sid).expect("just added").cells = cells;

            worksheet_parts.push(ws_part.clone());
            bindings.push(SheetBinding {
                sheet_id: sid,
                part_name: ws_part,
                captured: parsed_ws.captured,
            });
        }

        // 5. Promote the understood parts so the writer can distinguish
        // lazy-verbatim from re-encode.
        container.promote(&workbook_part, ModeledKind::Workbook);
        container.promote(&wb_rels_part, ModeledKind::WorkbookRels);
        if let Some(name) = &shared_part {
            container.promote(name, ModeledKind::SharedStrings);
        }
        if let Some(name) = &styles_part {
            container.promote(name, ModeledKind::Styles);
        }
        for name in &worksheet_parts {
            container.promote(name, ModeledKind::Worksheet);
        }

        Ok(XlsxDocument {
            model,
            formula_texts,
            container,
            bindings,
        })
    }

    /// Serialize back to XLSX bytes. Untouched parts re-emit verbatim
    /// (per-part byte identity); `calcChain.xml` is always dropped; dirty
    /// worksheets re-encode from the model.
    pub fn save(&self) -> Result<Vec<u8>, XlsxError> {
        write::save(
            &self.container,
            &self.model,
            &self.bindings,
            &self.formula_texts,
        )
    }

    /// Mark a worksheet's content as modified, so its part re-encodes from
    /// the model on the next [`save`](Self::save) (otherwise lazy-verbatim
    /// re-emits its original bytes).
    pub fn mark_sheet_dirty(&mut self, sheet: SheetId) {
        let Some(binding) = self.bindings.iter().find(|b| b.sheet_id == sheet) else {
            return;
        };
        let part_name = binding.part_name.clone();
        if let Some(PartEntry::Modeled { dirty, .. }) = self.container.part_mut(&part_name) {
            *dirty = true;
        }
        self.container.dirty = true;
    }

    /// True if any sheet has been marked dirty (a re-encode is pending).
    pub fn is_dirty(&self) -> bool {
        self.container.dirty
            || self
                .container
                .parts
                .iter()
                .any(|p| matches!(p, PartEntry::Modeled { dirty: true, .. }))
    }

    /// The worksheet part name bound to a model sheet (test/consumer helper).
    pub fn part_name_of(&self, sheet: SheetId) -> Option<&str> {
        self.bindings
            .iter()
            .find(|b| b.sheet_id == sheet)
            .map(|b| b.part_name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::value::CellValue;

    /// A tiny hand-authored package: minimal but structurally complete.
    fn minimal_xlsx() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let add = |zip: &mut zip::ZipWriter<_>, name: &str, body: &str| {
                use std::io::Write;
                zip.start_file(name, opts).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            };
            add(
                &mut zip,
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            );
            add(
                &mut zip,
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            );
            add(
                &mut zip,
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            );
            add(
                &mut zip,
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            );
            add(
                &mut zip,
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="n"><v>42</v></c><c r="B1" t="inlineStr"><is><t>hi</t></is></c></row></sheetData></worksheet>"#,
            );
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn open_parses_model() {
        let doc = XlsxDocument::open(&minimal_xlsx()).unwrap();
        assert_eq!(doc.model.sheets.len(), 1);
        let ws = doc.model.sheet(0).unwrap();
        assert_eq!(ws.cell(0, 0).unwrap().value, CellValue::Number(42.0));
        assert_eq!(ws.cell(0, 1).unwrap().value, CellValue::Text("hi".into()));
        assert!(!doc.is_dirty());
        assert_eq!(doc.part_name_of(0), Some("xl/worksheets/sheet1.xml"));
    }

    #[test]
    fn save_zero_edit_reparses_equal() {
        let doc = XlsxDocument::open(&minimal_xlsx()).unwrap();
        let out = doc.save().unwrap();
        let doc2 = XlsxDocument::open(&out).unwrap();
        let a = doc.model.sheet(0).unwrap();
        let b = doc2.model.sheet(0).unwrap();
        assert_eq!(a.cell(0, 0).unwrap().value, b.cell(0, 0).unwrap().value);
        assert_eq!(a.cell(0, 1).unwrap().value, b.cell(0, 1).unwrap().value);
    }

    #[test]
    fn mark_dirty_flips_state_and_reencodes() {
        let mut doc = XlsxDocument::open(&minimal_xlsx()).unwrap();
        assert!(!doc.is_dirty());
        doc.mark_sheet_dirty(0);
        assert!(doc.is_dirty());
        // A dirty re-encode still re-parses to the same model.
        let out = doc.save().unwrap();
        let doc2 = XlsxDocument::open(&out).unwrap();
        assert_eq!(
            doc2.model.sheet(0).unwrap().cell(0, 0).unwrap().value,
            CellValue::Number(42.0)
        );
        assert_eq!(
            doc2.model.sheet(0).unwrap().cell(0, 1).unwrap().value,
            CellValue::Text("hi".into())
        );
    }
}
