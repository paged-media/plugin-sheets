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
pub use parts::chart::{ParsedChart as XlsxChart, SheetResolver as ChartSheetResolver};
pub use parts::conditional_format::{
    CfBlock, CfOperator, CfRule, CfRuleKind, ColorScale, DataBar, SheetConditionalFormats,
};
pub use parts::external_link::{ExternalBook, ExternalLinks};
pub use parts::freeze::FreezePanes;
pub use parts::styles::{VisualStyle, VisualStyles};

use opc::{ModeledKind, OpcContainer, PartEntry};
use parts::chart::{ParsedChart, SheetResolver};
use rels::{
    part_dir, rels_part_for, resolve_target, Relationships, REL_CHART, REL_DRAWING,
    REL_EXTERNAL_LINK, REL_OFFICE_DOCUMENT, REL_SHARED_STRINGS, REL_STYLES, REL_TABLE,
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

    /// The interpreted visual style model (M1 style-map track, spec §8.3),
    /// keyed by the resolved `StyleId`. Built from `styles.xml`'s
    /// font/fill/border sub-tables, kept SEPARATE from the frozen `CellStyle`
    /// (whose `font`/`fill`/`border` stay opaque `u32` slots). The lowering
    /// reads this to build the IR-v2 styles table; it is read-only derived
    /// state, never written back (round-trip stays verbatim).
    pub visual_styles: parts::styles::VisualStyles,

    /// The differential-format (`<dxfs>`) table from `styles.xml`, indexed by
    /// `dxfId` (M2 conditional-formatting track, spec §10.4). Each entry is the
    /// OVERRIDE a matched `cfRule` applies on top of the cell's base style. The
    /// lowering pairs this with [`conditional_formats`](Self::conditional_formats)
    /// to fold cf matches into the lowered style. Read-only derived state.
    pub dxfs: Vec<parts::styles::VisualStyle>,

    /// Parsed conditional-format blocks per sheet (M2 track, spec §10.4),
    /// derived from each worksheet's captured `<conditionalFormatting>`
    /// subtrees. ADDITIVE: the subtrees STILL round-trip byte-identical via the
    /// verbatim capture (preserve.rs); this is read-only derived state for
    /// lowering, never written back. A sheet with no conditional formatting has
    /// no entry.
    pub conditional_formats: BTreeMap<SheetId, parts::conditional_format::SheetConditionalFormats>,

    /// Parsed DrawingML charts (M2 charts track, spec §8.4), discovered through
    /// each worksheet `.rels`' `/drawing` → drawing `.rels`' `/chart` chain and
    /// parsed into the FROZEN `sheet_chart::ChartModel`. ADDITIVE + READ-ONLY:
    /// the chart + drawing PARTS stay OPAQUE OPC parts (never promoted), so they
    /// re-emit byte-identical on round-trip (preservation invariant, spec
    /// §10.2); this model exists for `sheet-js` to LIST + RENDER charts, never
    /// for re-emit. Document order across sheets.
    pub charts: Vec<ParsedChart>,

    /// The CACHED values of every referenced external workbook (M3
    /// external-link reads, spec §13; the no-network ruling §1.1), indexed by
    /// the workbook's `<externalReferences>` order — the `[n]` external-book
    /// index a formula uses (`=[1]Sheet1!A1`). ADDITIVE + READ-ONLY + CACHED-
    /// ONLY: external links are NEVER followed (no network, no file access).
    /// Each `externalLinkN.xml` part stays OPAQUE (never promoted), so it
    /// re-emits byte-identical on round-trip (preservation invariant, spec
    /// §10.2). This model exists for a consumer to resolve an external
    /// reference to its cached value
    /// (`sheet_calc::external::resolve_cached`), never for re-emit. Empty when
    /// the workbook references no external books (the common case).
    pub external_links: ExternalLinks,

    /// Frozen-pane split per sheet (spec §8.1 — the sheets-mode grid view),
    /// derived from each worksheet's captured `<sheetViews><pane>` subtree.
    /// ADDITIVE + READ-ONLY: the `<sheetViews>` subtree STILL round-trips
    /// byte-identical via the verbatim capture (preserve.rs); this is read-only
    /// derived state for the grid surface, never written back. A sheet with no
    /// frozen pane has no entry (the common case).
    pub freeze_panes: BTreeMap<SheetId, parts::freeze::FreezePanes>,

    /// The OPC container (ordered parts + content types) for re-write.
    container: OpcContainer,
    /// Worksheet bindings: model sheet -> part name + captured subtrees.
    bindings: Vec<SheetBinding>,
}

/// A name→id resolver over the workbook's sheet list, for chart `c:f` refs.
struct ModelSheetResolver<'a> {
    model: &'a SheetModel,
}

impl SheetResolver for ModelSheetResolver<'_> {
    fn resolve(&self, name: &str) -> Option<SheetId> {
        self.model.sheet_id(name)
    }
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

        // styles: build the cellXfs index -> StyleId table, the visual style
        // model (M1 style-map track) keyed by the resolved StyleId, the
        // differential-format `<dxfs>` table (M2 conditional-formatting track),
        // AND the workbook display locale derived from a custom numFmt's
        // `[$…-LCID]` token (M3 localization track, ruling
        // `sheet.format.locale.locale-from-workbook`).
        let (xf_to_style, visual_styles, dxfs, workbook_locale) = match &styles_part {
            Some(name) => match container.part(name) {
                Some(p) => {
                    let parsed = parts::styles::parse(p.bytes(), &mut model.styles)?;
                    (
                        parsed.xf_to_style,
                        parsed.visual,
                        parsed.dxfs,
                        parsed.workbook_locale,
                    )
                }
                None => (
                    Vec::new(),
                    parts::styles::VisualStyles::default(),
                    Vec::new(),
                    None,
                ),
            },
            None => (
                Vec::new(),
                parts::styles::VisualStyles::default(),
                Vec::new(),
                None,
            ),
        };

        // Set the document locale from the derived workbook hint. HONEST
        // FALLBACK: OOXML has no document-locale element, so when no custom
        // numFmt carries a `[$…-LCID]` token we keep the default Locale::EnUs —
        // the locale is then expected to be set via the model/host, NOT
        // auto-detected (the format-code token still localizes per-cell). A
        // non-en hint sets the whole workbook's display locale.
        if let Some(loc) = workbook_locale {
            model.calc.locale = loc;
        }

        // 4. Each worksheet, in workbook (tab) order.
        let mut bindings = Vec::with_capacity(parsed_wb.sheets.len());
        let mut formula_texts: BTreeMap<(SheetId, u32, u32), String> = BTreeMap::new();
        let mut worksheet_parts: Vec<String> = Vec::new();
        let mut conditional_formats: BTreeMap<
            SheetId,
            parts::conditional_format::SheetConditionalFormats,
        > = BTreeMap::new();
        let mut freeze_panes: BTreeMap<SheetId, parts::freeze::FreezePanes> = BTreeMap::new();

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

            // Structured tables (ListObjects, spec §6.4 / ECMA-376 §18.5): a
            // worksheet references its table parts through its OWN `.rels`
            // (`<tableParts>` in the sheet xml lists the r:ids; the relationships
            // resolve to `xl/tables/tableN.xml`). We parse each into the model so
            // structured refs resolve; the table PARTS stay opaque (never
            // promoted), so they re-emit byte-identical (preservation invariant,
            // spec §10.2 — understood now, not rewritten).
            let ws_rels_part = rels_part_for(&ws_part);
            if let Some(ws_rels_bytes) = container.part(&ws_rels_part).map(|p| p.bytes().to_vec()) {
                let ws_rels = Relationships::parse(&ws_rels_bytes)?;
                let ws_base = part_dir(&ws_part);
                let mut tables = Vec::new();
                for rel in ws_rels.all_of_type(REL_TABLE) {
                    let table_part = resolve_target(&ws_base, &rel.target);
                    if let Some(table_bytes) =
                        container.part(&table_part).map(|p| p.bytes().to_vec())
                    {
                        // A malformed table part is skipped (preservation-safe:
                        // its bytes still round-trip); the model just omits it.
                        if let Ok(table) = parts::tables::parse(&table_bytes, sid) {
                            tables.push(table);
                        }
                    }
                }
                model.sheet_mut(sid).expect("just added").tables = tables;
            }

            // Conditional formatting (M2 track, spec §10.4): the
            // `<conditionalFormatting>` children of <worksheet> were captured
            // verbatim (AfterSheetData) for the preservation invariant — we
            // ADDITIONALLY parse them into the read-only cf model for lowering.
            // The captured bytes are untouched (round-trip stays byte-identical).
            let cf = parts::conditional_format::parse_all(
                parsed_ws.captured.after().map(|c| c.bytes.as_slice()),
            )?;
            if !cf.is_empty() {
                conditional_formats.insert(sid, cf);
            }

            // Freeze panes (spec §8.1): the `<sheetViews><pane>` split sits in
            // the worksheet's BEFORE-sheetData captures (an unmodeled
            // `<sheetViews>` child). ADDITIVE parse — the captured bytes are
            // untouched (the view still round-trips byte-identical).
            if let Some(fp) = parts::freeze::parse_all(
                parsed_ws.captured.before().map(|c| c.bytes.as_slice()),
            )? {
                freeze_panes.insert(sid, fp);
            }

            worksheet_parts.push(ws_part.clone());
            bindings.push(SheetBinding {
                sheet_id: sid,
                part_name: ws_part,
                captured: parsed_ws.captured,
            });
        }

        // 4.5. Charts (M2 charts track, spec §8.4). For each worksheet, follow
        // its `.rels`' `/drawing` rel to the drawing part, then the drawing
        // `.rels`' `/chart` rel(s) to each `xl/charts/chartN.xml`, and parse it
        // into the frozen `sheet_chart::ChartModel`. ADDITIVE + READ-ONLY: the
        // chart + drawing parts stay OPAQUE (never promoted), so they re-emit
        // byte-identical on round-trip (the `<drawing>` element on the
        // worksheet is itself a captured unknown child). The resolver maps
        // `c:f` sheet names to model ids; the full model has all sheets now.
        let mut charts: Vec<ParsedChart> = Vec::new();
        {
            let resolver = ModelSheetResolver { model: &model };
            // Snapshot (sheet_id, part_name) so we don't borrow `bindings`
            // while reading the container.
            let ws_for_charts: Vec<(SheetId, String)> = bindings
                .iter()
                .map(|b| (b.sheet_id, b.part_name.clone()))
                .collect();
            for (host_sheet, ws_part) in ws_for_charts {
                let ws_rels_part = rels_part_for(&ws_part);
                let Some(ws_rels_bytes) = container.part(&ws_rels_part).map(|p| p.bytes().to_vec())
                else {
                    continue;
                };
                let ws_rels = Relationships::parse(&ws_rels_bytes)?;
                let ws_base = part_dir(&ws_part);
                for drawing_rel in ws_rels.all_of_type(REL_DRAWING) {
                    let drawing_part = resolve_target(&ws_base, &drawing_rel.target);
                    let drawing_rels_part = rels_part_for(&drawing_part);
                    let Some(dr_bytes) = container
                        .part(&drawing_rels_part)
                        .map(|p| p.bytes().to_vec())
                    else {
                        continue;
                    };
                    let dr_rels = Relationships::parse(&dr_bytes)?;
                    let dr_base = part_dir(&drawing_part);
                    for chart_rel in dr_rels.all_of_type(REL_CHART) {
                        let chart_part = resolve_target(&dr_base, &chart_rel.target);
                        if let Some(chart_bytes) =
                            container.part(&chart_part).map(|p| p.bytes().to_vec())
                        {
                            // A malformed chart part is skipped (preservation-
                            // safe: its bytes still round-trip).
                            if let Ok(parsed) =
                                parts::chart::parse(&chart_bytes, host_sheet, &resolver)
                            {
                                charts.push(parsed);
                            }
                        }
                    }
                }
            }
        }

        // 4.6. External-link CACHED values (M3 external-link reads, spec §13;
        // the no-network ruling §1.1). The workbook's `<externalReferences>`
        // r:ids (in order — the `[n]` external-book index) resolve through the
        // workbook `.rels` to each `xl/externalLinks/externalLinkN.xml` part,
        // whose CACHED last-known cell values we parse into the read-only
        // side-table. CACHED-ONLY by construction: we read ONLY these inline
        // cache bytes — the referenced source workbook (named in the part's
        // `<externalBook>`/`.rels` Target) is NEVER opened; no network, no
        // file access. The externalLink PARTS stay OPAQUE (never promoted), so
        // they re-emit byte-identical on round-trip (preservation invariant,
        // spec §10.2) — the same discipline as the chart/table parts.
        let mut external_links = parts::external_link::ExternalLinks::default();
        for rid in &parsed_wb.external_refs {
            // Resolve the r:id through the workbook rels; only `/externalLink`
            // targets are external-link parts (defensive — the index position
            // is what a formula's `[n]` names, so we push a book per ref).
            let book = wb_rels
                .target_of(rid)
                .map(|t| resolve_target(&wb_base, t))
                .filter(|_| {
                    wb_rels
                        .rels
                        .iter()
                        .any(|r| r.id == *rid && r.is_type(REL_EXTERNAL_LINK))
                })
                .and_then(|part| container.part(&part).map(|p| p.bytes().to_vec()))
                // A malformed external-link part is skipped (preservation-safe:
                // its bytes still round-trip); the slot becomes an empty book so
                // the `[n]` index stays aligned with the workbook order.
                .and_then(|bytes| parts::external_link::parse(&bytes).ok())
                .unwrap_or_default();
            external_links.books.push(book);
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
            visual_styles,
            dxfs,
            conditional_formats,
            charts,
            external_links,
            freeze_panes,
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

    /// The conditional-format model for a sheet, lowered into the
    /// `sheet-lower` mirror the lowering consumes (dxf overrides resolved
    /// against the workbook `<dxfs>` table) — the cf analogue of
    /// [`visual_styles`](Self::visual_styles). An empty
    /// [`sheet_lower::SheetCondFmt`] for a sheet without conditional
    /// formatting, so a caller can always pass the result to
    /// `sheet_lower::lower_range_condfmt` (it reproduces the styled path when
    /// empty). M2 conditional-formatting track, spec §10.4.
    pub fn lowered_conditional_formats(&self, sheet: SheetId) -> sheet_lower::SheetCondFmt {
        match self.conditional_formats.get(&sheet) {
            Some(cf) => cf.to_lower(&self.dxfs),
            None => sheet_lower::SheetCondFmt::default(),
        }
    }

    /// The frozen-pane split of a sheet (spec §8.1), or the default `(0, 0)`
    /// no-freeze when the sheet carries none. Read-only derived state from the
    /// worksheet's captured `<sheetViews><pane>` (the bytes round-trip
    /// byte-identical); the grid surface renders the split.
    pub fn freeze_panes_of(&self, sheet: SheetId) -> parts::freeze::FreezePanes {
        self.freeze_panes.get(&sheet).copied().unwrap_or_default()
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
