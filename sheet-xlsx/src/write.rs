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

//! The XLSX writer (spec §10.2/§10.4).
//!
//! Strategy, per the preservation invariant:
//!
//! - **Untouched parts** re-emit their stored decompressed bytes verbatim
//!   (lazy-verbatim). Re-deflating identical bytes yields decompressed
//!   per-part identity — the zero-edit assertion. (Whole-file/zip-metadata
//!   identity is explicitly not claimed.)
//! - **`calcChain.xml` is always dropped** (ruling `sheet.xlsx.calcchain.drop`):
//!   the part entry, its `[Content_Types]` override, and its workbook
//!   relationship are removed. Consumers (Excel) regenerate the chain.
//! - **`[Content_Types].xml` and the workbook `.rels`** re-emit verbatim
//!   UNLESS calcChain removal changed them, in which case they are rebuilt
//!   minimally (only the calcChain line removed).
//! - **Dirty worksheets** re-encode from the model (`r/t/s/v/f` cells,
//!   `mergeCells`, `cols`, custom row heights) and re-emit captured unknown
//!   subtrees in schema position (preserve.rs).
//! - **sharedStrings** rebuilds only when a sheet is dirty AND new strings
//!   were added; otherwise verbatim.

use crate::error::XlsxError;
use crate::opc::{ContentTypes, ModeledKind, OpcContainer, PartEntry};
use crate::sheet_doc::SheetBinding;
use sheet_core::value::CellValue;
use sheet_core::{col_to_a1, SheetId, SheetModel};
use std::collections::BTreeMap;
use std::io::Write;

/// The `calcChain.xml` part name (always dropped).
const CALC_CHAIN_PART: &str = "xl/calcChain.xml";

/// Serialize the document to XLSX bytes.
///
/// `bindings` maps each worksheet's part name + SheetId so dirty sheets can
/// be re-encoded from the model. `formula_texts` supplies the raw `<f>` text
/// for each edited formula cell.
pub fn save(
    container: &OpcContainer,
    model: &SheetModel,
    bindings: &[SheetBinding],
    formula_texts: &BTreeMap<(SheetId, u32, u32), String>,
) -> Result<Vec<u8>, XlsxError> {
    let drop_calc_chain = container.part(CALC_CHAIN_PART).is_some();

    // sharedStrings is NOT rebuilt here in T0: a dirty re-encode keeps the
    // original interner (the consumer sheet-js owns string additions before
    // save), so re-emitting it verbatim stays correct even when sheets are
    // dirty. The rebuild hook lands when sheet-js writes new strings back.

    let buf = Vec::new();
    let cursor = std::io::Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for part in &container.parts {
        let name = part.name();

        // calcChain is dropped entirely.
        if drop_calc_chain && name == CALC_CHAIN_PART {
            continue;
        }

        let bytes: Vec<u8> = match part {
            PartEntry::Opaque { bytes, .. } => bytes.clone(),
            PartEntry::Modeled {
                kind,
                raw,
                dirty,
                name,
            } => {
                match kind {
                    ModeledKind::ContentTypes => {
                        if drop_calc_chain {
                            rebuild_content_types(&container.content_types)
                        } else {
                            raw.clone()
                        }
                    }
                    ModeledKind::WorkbookRels => {
                        if drop_calc_chain {
                            strip_calc_chain_rel(raw)?
                        } else {
                            raw.clone()
                        }
                    }
                    ModeledKind::Worksheet => {
                        if *dirty {
                            let binding = bindings
                                .iter()
                                .find(|b| &b.part_name == name)
                                .ok_or_else(|| {
                                    XlsxError::Structure(format!(
                                        "dirty worksheet {name} has no binding"
                                    ))
                                })?;
                            encode_worksheet(model, binding, formula_texts)?
                        } else {
                            raw.clone()
                        }
                    }
                    // Workbook / SharedStrings / Styles: verbatim in T0
                    // (the model never mutates them through this writer yet).
                    _ => raw.clone(),
                }
            }
        };

        zip.start_file(name, opts)?;
        zip.write_all(&bytes).map_err(zip_io)?;
    }

    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}

fn zip_io(e: std::io::Error) -> XlsxError {
    XlsxError::Zip(zip::result::ZipError::Io(e))
}

/// Rebuild `[Content_Types].xml` from the model, omitting the calcChain
/// override. Deterministic byte output (defaults then overrides, in order).
fn rebuild_content_types(ct: &ContentTypes) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push('\n');
    s.push_str(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#);
    for (ext, ty) in &ct.defaults {
        s.push_str(&format!(
            r#"<Default Extension="{}" ContentType="{}"/>"#,
            xml_attr(ext),
            xml_attr(ty)
        ));
    }
    for (pn, ty) in &ct.overrides {
        if pn == "/xl/calcChain.xml" {
            continue;
        }
        s.push_str(&format!(
            r#"<Override PartName="{}" ContentType="{}"/>"#,
            xml_attr(pn),
            xml_attr(ty)
        ));
    }
    s.push_str("</Types>");
    s.into_bytes()
}

/// Remove the `<Relationship ... /calcChain ... />` line from the workbook
/// `.rels`, leaving the rest byte-for-byte. We parse + re-serialize only the
/// relationship rows we recognize, which is enough for T0 because workbook
/// `.rels` is a flat list of `<Relationship>` elements.
fn strip_calc_chain_rel(raw: &[u8]) -> Result<Vec<u8>, XlsxError> {
    use crate::rels::{Relationships, REL_CALC_CHAIN};
    let rels = Relationships::parse(raw)?;
    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push('\n');
    s.push_str(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for r in &rels.rels {
        if r.is_type(REL_CALC_CHAIN) {
            continue;
        }
        s.push_str(&format!(
            r#"<Relationship Id="{}" Type="{}" Target="{}"/>"#,
            xml_attr(&r.id),
            xml_attr(&r.rel_type),
            xml_attr(&r.target)
        ));
    }
    s.push_str("</Relationships>");
    Ok(s.into_bytes())
}

/// Re-encode a dirty worksheet from the model.
///
/// Emits, in ECMA-376 §18.3.1.99 child order: captured `BeforeSheetData`
/// subtrees, `<dimension>`, `<cols>`, `<sheetData>` (rows/cells from the
/// model + `formula_texts`), `<mergeCells>`, then captured `AfterSheetData`
/// subtrees. Unknown per-cell/row attributes are NOT preserved on a dirty
/// re-encode (T0 granularity, recorded in the registry note for
/// `sheet.xlsx.preserve.unknown-subtrees`).
fn encode_worksheet(
    model: &SheetModel,
    binding: &SheetBinding,
    formula_texts: &BTreeMap<(SheetId, u32, u32), String>,
) -> Result<Vec<u8>, XlsxError> {
    let ws = model
        .sheet(binding.sheet_id)
        .ok_or_else(|| XlsxError::Structure("binding points at missing sheet".into()))?;
    let sid = binding.sheet_id;

    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push('\n');
    s.push_str(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
    );

    // Captured children that sit before <sheetData>.
    for c in binding.captured.before() {
        s.push_str(&String::from_utf8_lossy(&c.bytes));
    }

    // <dimension>
    if let Some(ur) = ws.used_range() {
        let a = format!("{}{}", col_to_a1(ur.col0), ur.row0 + 1);
        let b = format!("{}{}", col_to_a1(ur.col1), ur.row1 + 1);
        if a == b {
            s.push_str(&format!(r#"<dimension ref="{a}"/>"#));
        } else {
            s.push_str(&format!(r#"<dimension ref="{a}:{b}"/>"#));
        }
    }

    // <cols>
    if !ws.col_widths.is_empty() {
        s.push_str("<cols>");
        for (&col, &width) in &ws.col_widths {
            let c1 = col + 1;
            s.push_str(&format!(
                r#"<col min="{c1}" max="{c1}" width="{}" customWidth="1"/>"#,
                fmt_f64(width)
            ));
        }
        s.push_str("</cols>");
    }

    // <sheetData> — rows in order, cells in column order within a row.
    s.push_str("<sheetData>");
    let mut rows: BTreeMap<u32, Vec<(u32, &sheet_core::Cell)>> = BTreeMap::new();
    for (&(r, c), cell) in ws.iter_cells() {
        rows.entry(r).or_default().push((c, cell));
    }
    for (&r, cells) in &rows {
        let r1 = r + 1;
        let ht = ws.row_heights.get(&r);
        match ht {
            Some(h) => s.push_str(&format!(
                r#"<row r="{r1}" ht="{}" customHeight="1">"#,
                fmt_f64(*h)
            )),
            None => s.push_str(&format!(r#"<row r="{r1}">"#)),
        }
        for (c, cell) in cells {
            encode_cell(&mut s, sid, r, *c, cell, formula_texts);
        }
        s.push_str("</row>");
    }
    // Any row that has only a custom height but no cells.
    s.push_str("</sheetData>");

    // <mergeCells>
    if !ws.merges.is_empty() {
        s.push_str(&format!(r#"<mergeCells count="{}">"#, ws.merges.len()));
        for m in &ws.merges {
            let n = m.normalized();
            let a = format!("{}{}", col_to_a1(n.start.col), n.start.row + 1);
            let b = format!("{}{}", col_to_a1(n.end.col), n.end.row + 1);
            s.push_str(&format!(r#"<mergeCell ref="{a}:{b}"/>"#));
        }
        s.push_str("</mergeCells>");
    }

    // Captured children that sit after <sheetData>.
    for c in binding.captured.after() {
        s.push_str(&String::from_utf8_lossy(&c.bytes));
    }

    s.push_str("</worksheet>");
    Ok(s.into_bytes())
}

/// Encode one `<c>` cell: address, type, style index, value, and formula.
fn encode_cell(
    s: &mut String,
    sid: SheetId,
    row: u32,
    col: u32,
    cell: &sheet_core::Cell,
    formula_texts: &BTreeMap<(SheetId, u32, u32), String>,
) {
    let r = format!("{}{}", col_to_a1(col), row + 1);
    // Style index: StyleId(0) is default — omit s= for it.
    let style_attr = if cell.style.0 != 0 {
        format!(r#" s="{}""#, cell.style.0)
    } else {
        String::new()
    };

    let formula = formula_texts.get(&(sid, row, col));

    // Determine the t= attribute + inner content from the value.
    let (type_attr, inner) = match &cell.value {
        CellValue::Empty => (String::new(), String::new()),
        CellValue::Number(n) => (String::new(), format!("<v>{}</v>", fmt_f64(*n))),
        CellValue::Bool(b) => (
            r#" t="b""#.to_owned(),
            format!("<v>{}</v>", if *b { 1 } else { 0 }),
        ),
        CellValue::Error(e) => (
            r#" t="e""#.to_owned(),
            format!("<v>{}</v>", xml_text(e.as_str())),
        ),
        CellValue::Text(t) => {
            // Formula cells with text results use t="str"; literal text uses
            // an inline string (T0 does not write back into sharedStrings).
            if formula.is_some() {
                (r#" t="str""#.to_owned(), format!("<v>{}</v>", xml_text(t)))
            } else {
                (
                    r#" t="inlineStr""#.to_owned(),
                    format!("<is><t>{}</t></is>", xml_text(t)),
                )
            }
        }
    };

    let formula_xml = match formula {
        Some(f) => format!("<f>{}</f>", xml_text(f)),
        None => String::new(),
    };

    if inner.is_empty() && formula_xml.is_empty() {
        // style-only cell
        s.push_str(&format!(r#"<c r="{r}"{style_attr}{type_attr}/>"#));
    } else {
        s.push_str(&format!(
            r#"<c r="{r}"{style_attr}{type_attr}>{formula_xml}{inner}</c>"#
        ));
    }
}

/// Format an f64 for `<v>` / width / height. Integers print without a
/// decimal point (Excel convention); other values use Rust's shortest
/// round-trip representation (the std `{}` formatter is shortest-round-trip).
fn fmt_f64(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Escape text content for an XML element body.
fn xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape an attribute value.
fn xml_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_f64_integers_and_decimals() {
        assert_eq!(fmt_f64(42.0), "42");
        assert_eq!(fmt_f64(0.0), "0");
        assert_eq!(fmt_f64(-7.0), "-7");
        assert_eq!(fmt_f64(3.5), "3.5");
        assert_eq!(fmt_f64(20.5), "20.5");
    }

    #[test]
    fn xml_escapes() {
        assert_eq!(xml_text("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(xml_attr(r#"x"y&z"#), "x&quot;y&amp;z");
    }
}
