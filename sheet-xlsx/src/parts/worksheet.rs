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

//! `sheetN.xml` — a worksheet part (ECMA-376 §18.3).
//!
//! T0 reads the cell grid plus the geometry the model holds: `<c r t s>`
//! with `<v>`/`<f>`/`<is>`, `<mergeCells>`, `<cols>` widths, and `<row ht
//! customHeight>` heights. Cell *formulas* are NOT parsed here (the dep rule
//! is xlsx -> core only); their raw text goes to the document's
//! `formula_texts` map for the consumer (sheet-js) to parse.
//!
//! Unknown direct children of `<worksheet>` are captured verbatim
//! (preserve.rs) so a dirty re-encode re-emits them in schema position. The
//! understood children — `sheetData`, `mergeCells`, `cols`, `dimension` —
//! are modeled and regenerated, so they are NOT captured.

use crate::error::XlsxError;
use crate::opc::attr;
use crate::preserve::{Anchor, CapturedSubtrees};
use compact_str::CompactString;
use sheet_core::value::{CellError, CellValue};
use sheet_core::{parse_a1, CellRef, RangeRef};
use std::collections::BTreeMap;

/// A parsed cell, pre-interning: its 0-based address, value, optional raw
/// formula text, and raw `s=` style index (resolved against the styles part
/// by the caller).
#[derive(Debug, Clone)]
pub struct ParsedCell {
    pub row: u32,
    pub col: u32,
    pub value: CellValue,
    /// Raw formula text (without leading `=`), or `None` for a literal cell.
    pub formula: Option<String>,
    /// The `s=` cellXfs index, or 0 (default style).
    pub style_index: u32,
}

/// Everything a worksheet part yields.
#[derive(Default)]
pub struct ParsedWorksheet {
    pub cells: Vec<ParsedCell>,
    pub merges: Vec<RangeRef>,
    /// Column widths in characters, keyed by 0-based column.
    pub col_widths: BTreeMap<u32, f64>,
    /// Row heights in points (only rows with `customHeight="1"`).
    pub row_heights: BTreeMap<u32, f64>,
    /// Unknown `<worksheet>` children, re-emitted on dirty re-encode.
    pub captured: CapturedSubtrees,
}

/// The understood direct children of `<worksheet>`; everything else is
/// captured verbatim (preservation invariant).
const KNOWN_WS_CHILDREN: &[&[u8]] = &[b"sheetData", b"mergeCells", b"cols", b"dimension"];

/// Parse `sheetN.xml`. `shared` is the resolved shared-string table (indexed
/// by `t="s"` cells).
pub fn parse(xml: &[u8], shared: &[CompactString]) -> Result<ParsedWorksheet, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();
    let mut out = ParsedWorksheet::default();

    // Shared-formula masters: si -> (master text, master ref). Members with
    // the same si but no <f> text reuse the master's TEXT verbatim (T0
    // honest limitation, documented below).
    let mut shared_masters: BTreeMap<u32, String> = BTreeMap::new();

    // The depth-1 element name we are currently inside (a direct child of
    // <worksheet>), for unknown-subtree capture.
    let mut depth: i32 = 0;
    let mut capturing: Option<(Anchor, usize, Vec<u8>)> = None; // (anchor, start_pos, name)
    let mut seen_sheet_data = false;

    // Within <sheetData> -> <row> -> <c>:
    let mut cur_row_ht: Option<f64> = None;
    let mut cur_row_idx: u32 = 0;
    let mut cur_cell: Option<CellAccum> = None;
    let mut text_target: Option<TextTarget> = None;

    loop {
        // Byte position BEFORE reading the next event — start of a child
        // element we may need to capture verbatim.
        let pos_before = reader.buffer_position() as usize;
        let ev = reader.read_event_into(&mut buf)?;
        match ev {
            Event::Start(e) => {
                depth += 1;
                let local = e.local_name().as_ref().to_vec();

                // Capture unknown depth-1 children of <worksheet>.
                if depth == 2 && capturing.is_none() && !is_known_child(&local) {
                    let anchor = if seen_sheet_data {
                        Anchor::AfterSheetData
                    } else {
                        Anchor::BeforeSheetData
                    };
                    capturing = Some((anchor, pos_before, local.clone()));
                }

                // Modeled handling only when not inside a captured subtree.
                if capturing.is_none() {
                    match local.as_slice() {
                        b"sheetData" => seen_sheet_data = true,
                        b"row" => {
                            cur_row_idx = attr(&e, b"r")?
                                .and_then(|s| s.parse::<u32>().ok())
                                .map(|r| r.saturating_sub(1))
                                .unwrap_or(0);
                            let custom = attr(&e, b"customHeight")?
                                .map(|v| v == "1" || v == "true")
                                .unwrap_or(false);
                            cur_row_ht = if custom {
                                attr(&e, b"ht")?.and_then(|s| s.parse::<f64>().ok())
                            } else {
                                None
                            };
                        }
                        b"c" => {
                            cur_cell = Some(CellAccum::start(&e)?);
                        }
                        b"v" => text_target = Some(TextTarget::Value),
                        b"t" => text_target = Some(TextTarget::InlineText),
                        b"f" => {
                            // record shared-formula attributes for masters
                            let t = attr(&e, b"t")?;
                            let si = attr(&e, b"si")?.and_then(|s| s.parse::<u32>().ok());
                            if let Some(c) = cur_cell.as_mut() {
                                c.f_shared_si = si;
                                c.f_is_shared_master =
                                    t.as_deref() == Some("shared") && attr(&e, b"ref")?.is_some();
                            }
                            text_target = Some(TextTarget::Formula);
                        }
                        _ => {}
                    }
                }
            }
            Event::Empty(e) => {
                let local = e.local_name().as_ref().to_vec();
                // An empty depth-1 unknown child: capture just this element.
                let is_ws_child = depth == 1;
                if is_ws_child && capturing.is_none() && !is_known_child(&local) {
                    let anchor = if seen_sheet_data {
                        Anchor::AfterSheetData
                    } else {
                        Anchor::BeforeSheetData
                    };
                    let end = reader.buffer_position() as usize;
                    out.captured.push(anchor, xml[pos_before..end].to_vec());
                } else if capturing.is_none() {
                    match local.as_slice() {
                        b"dimension" => { /* recomputed on write */ }
                        b"mergeCell" => {
                            if let Some(rf) = attr(&e, b"ref")? {
                                if let Some(r) = parse_range(&rf) {
                                    out.merges.push(r);
                                }
                            }
                        }
                        b"col" => parse_col(&e, &mut out)?,
                        b"c" => {
                            // empty cell (e.g. style-only <c r=.. s=../>)
                            let acc = CellAccum::start(&e)?;
                            push_cell(&mut out, acc, shared, &mut shared_masters)?;
                        }
                        b"row" => {
                            // empty <row/> with possibly customHeight
                            let idx = attr(&e, b"r")?
                                .and_then(|s| s.parse::<u32>().ok())
                                .map(|r| r.saturating_sub(1));
                            let custom = attr(&e, b"customHeight")?
                                .map(|v| v == "1" || v == "true")
                                .unwrap_or(false);
                            if custom {
                                if let (Some(idx), Some(ht)) =
                                    (idx, attr(&e, b"ht")?.and_then(|s| s.parse::<f64>().ok()))
                                {
                                    out.row_heights.insert(idx, ht);
                                }
                            }
                        }
                        b"f" => {
                            // empty <f/>: a shared-formula member with no text.
                            let si = attr(&e, b"si")?.and_then(|s| s.parse::<u32>().ok());
                            if let Some(c) = cur_cell.as_mut() {
                                c.f_shared_si = si;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::Text(t) => {
                if capturing.is_none() {
                    if let Some(tt) = text_target {
                        let s = t.unescape().map_err(XlsxError::Xml)?;
                        if let Some(c) = cur_cell.as_mut() {
                            match tt {
                                TextTarget::Value => c.v.push_str(&s),
                                TextTarget::Formula => c.f.push_str(&s),
                                TextTarget::InlineText => c.inline_t.push_str(&s),
                            }
                        }
                    }
                }
            }
            Event::End(e) => {
                let local = e.local_name().as_ref().to_vec();

                // Close a captured subtree at depth 1 (its start was depth 2).
                if let Some((anchor, start, ref name)) = capturing {
                    if depth == 2 && &local == name {
                        let end = reader.buffer_position() as usize;
                        out.captured.push(anchor, xml[start..end].to_vec());
                        capturing = None;
                    }
                    depth -= 1;
                } else {
                    match local.as_slice() {
                        b"c" => {
                            if let Some(acc) = cur_cell.take() {
                                push_cell(&mut out, acc, shared, &mut shared_masters)?;
                            }
                        }
                        b"row" => {
                            if let Some(ht) = cur_row_ht.take() {
                                out.row_heights.insert(cur_row_idx, ht);
                            }
                        }
                        b"v" | b"f" | b"t" => text_target = None,
                        _ => {}
                    }
                    depth -= 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

/// True if `local` is a `<worksheet>` child we model (and thus regenerate),
/// rather than capture verbatim.
fn is_known_child(local: &[u8]) -> bool {
    KNOWN_WS_CHILDREN.contains(&local)
}

#[derive(Copy, Clone)]
enum TextTarget {
    Value,
    Formula,
    InlineText,
}

/// Accumulator for the bytes of one `<c>` element.
struct CellAccum {
    row: u32,
    col: u32,
    ty: CompactString, // t= attribute ("n" default, "s","str","b","e","inlineStr")
    style_index: u32,
    v: String,
    f: String,
    inline_t: String,
    f_shared_si: Option<u32>,
    f_is_shared_master: bool,
}

impl CellAccum {
    fn start(e: &quick_xml::events::BytesStart<'_>) -> Result<CellAccum, XlsxError> {
        let r = attr(e, b"r")?.ok_or_else(|| XlsxError::Structure("<c> missing r=".into()))?;
        let (row, col, _, _) =
            parse_a1(&r).ok_or_else(|| XlsxError::Structure(format!("bad cell ref {r}")))?;
        let ty = attr(e, b"t")?.unwrap_or_default();
        let style_index = attr(e, b"s")?
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        Ok(CellAccum {
            row,
            col,
            ty: CompactString::new(&ty),
            style_index,
            v: String::new(),
            f: String::new(),
            inline_t: String::new(),
            f_shared_si: None,
            f_is_shared_master: false,
        })
    }
}

/// Finalize a `<c>` accumulator into a [`ParsedCell`], resolving the value by
/// its `t=` type and handling shared-formula masters/members.
///
/// Shared-formula handling (T0, honest limitation): when a member has no `<f>`
/// text but carries `si=`, we copy the **master's text verbatim** rather than
/// translating the relative refs to the member's position. The text round-
/// trips losslessly; the consumer (sheet-js) re-derives correct refs when it
/// parses. Recorded as the `sheet.xlsx.worksheet.cells` registry note.
fn push_cell(
    out: &mut ParsedWorksheet,
    acc: CellAccum,
    shared: &[CompactString],
    shared_masters: &mut BTreeMap<u32, String>,
) -> Result<(), XlsxError> {
    // Resolve the formula text.
    let formula = if !acc.f.is_empty() {
        if acc.f_is_shared_master {
            if let Some(si) = acc.f_shared_si {
                shared_masters.insert(si, acc.f.clone());
            }
        }
        Some(acc.f.clone())
    } else if let Some(si) = acc.f_shared_si {
        shared_masters.get(&si).cloned()
    } else {
        None
    };

    // Resolve the value by type.
    let value = resolve_value(&acc.ty, &acc.v, &acc.inline_t, shared)?;

    out.cells.push(ParsedCell {
        row: acc.row,
        col: acc.col,
        value,
        formula,
        style_index: acc.style_index,
    });
    Ok(())
}

/// Map (t=, <v>, inline text) to a [`CellValue`] (ECMA-376 §18.18.11 ST_CellType).
fn resolve_value(
    ty: &str,
    v: &str,
    inline_t: &str,
    shared: &[CompactString],
) -> Result<CellValue, XlsxError> {
    Ok(match ty {
        // shared string: <v> is an index into the shared table
        "s" => {
            let idx: usize = v
                .trim()
                .parse()
                .map_err(|_| XlsxError::Structure(format!("bad shared-string index {v:?}")))?;
            shared
                .get(idx)
                .map(|s| CellValue::Text(s.clone()))
                .unwrap_or(CellValue::Empty)
        }
        // inline string
        "inlineStr" => CellValue::Text(CompactString::new(inline_t)),
        // formula string result
        "str" => CellValue::Text(CompactString::new(v)),
        // boolean
        "b" => CellValue::Bool(v.trim() == "1"),
        // error
        "e" => CellValue::Error(CellError::parse(v.trim()).unwrap_or(CellError::Value)),
        // number (default) — empty <v> means a blank cell
        "" | "n" => {
            if v.trim().is_empty() {
                CellValue::Empty
            } else {
                v.trim()
                    .parse::<f64>()
                    .map(CellValue::Number)
                    .unwrap_or(CellValue::Empty)
            }
        }
        // unknown type: keep as text of the raw value (preservation-safe)
        _ => CellValue::Text(CompactString::new(v)),
    })
}

/// Parse a `<col min max width customWidth>` element into per-column widths.
fn parse_col(
    e: &quick_xml::events::BytesStart<'_>,
    out: &mut ParsedWorksheet,
) -> Result<(), XlsxError> {
    let min = attr(e, b"min")?.and_then(|s| s.parse::<u32>().ok());
    let max = attr(e, b"max")?.and_then(|s| s.parse::<u32>().ok());
    let custom = attr(e, b"customWidth")?
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    let width = attr(e, b"width")?.and_then(|s| s.parse::<f64>().ok());
    if let (Some(min), Some(max), Some(width)) = (min, max, width) {
        // <col> uses 1-based inclusive column ranges; store only when an
        // explicit/custom width is present.
        if custom || attr(e, b"width")?.is_some() {
            for c in min..=max {
                out.col_widths.insert(c.saturating_sub(1), width);
            }
        }
    }
    Ok(())
}

/// Parse an `A1:B2` range token into a relative [`RangeRef`] on sheet 0.
fn parse_range(s: &str) -> Option<RangeRef> {
    let (a, b) = s.split_once(':')?;
    let (r0, c0, _, _) = parse_a1(a)?;
    let (r1, c1, _, _) = parse_a1(b)?;
    let mk = |row, col| CellRef {
        sheet: 0,
        row,
        col,
        row_abs: false,
        col_abs: false,
    };
    Some(RangeRef {
        start: mk(r0, c0),
        end: mk(r1, c1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Vec<CompactString> {
        vec![CompactString::new("Alpha"), CompactString::new("Beta")]
    }

    #[test]
    fn cells_values_and_types() {
        let xml = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1:D2"/>
  <sheetData>
    <row r="1">
      <c r="A1" t="n"><v>42</v></c>
      <c r="B1" t="s"><v>1</v></c>
      <c r="C1" t="str"><f>A1+1</f><v>43</v></c>
      <c r="D1" t="b"><v>1</v></c>
    </row>
    <row r="2">
      <c r="A2" t="e"><v>#DIV/0!</v></c>
      <c r="B2" t="inlineStr"><is><t>inline here</t></is></c>
      <c r="C2" s="3"/>
    </row>
  </sheetData>
</worksheet>"#;
        let ws = parse(xml, &shared()).unwrap();
        let by = |r: u32, c: u32| ws.cells.iter().find(|x| x.row == r && x.col == c).unwrap();
        assert_eq!(by(0, 0).value, CellValue::Number(42.0));
        assert_eq!(by(0, 1).value, CellValue::Text(CompactString::new("Beta")));
        assert_eq!(by(0, 2).formula.as_deref(), Some("A1+1"));
        assert_eq!(by(0, 2).value, CellValue::Text(CompactString::new("43")));
        assert_eq!(by(0, 3).value, CellValue::Bool(true));
        assert_eq!(by(1, 0).value, CellValue::Error(CellError::Div0));
        assert_eq!(
            by(1, 1).value,
            CellValue::Text(CompactString::new("inline here"))
        );
        assert_eq!(by(1, 2).style_index, 3);
    }

    #[test]
    fn merges_cols_rows() {
        let xml = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <cols>
    <col min="1" max="1" width="20.5" customWidth="1"/>
    <col min="3" max="4" width="8.0" customWidth="1"/>
  </cols>
  <sheetData>
    <row r="1" ht="30.0" customHeight="1"><c r="A1" t="n"><v>1</v></c></row>
  </sheetData>
  <mergeCells count="1"><mergeCell ref="A1:C1"/></mergeCells>
</worksheet>"#;
        let ws = parse(xml, &shared()).unwrap();
        assert_eq!(ws.col_widths.get(&0), Some(&20.5));
        assert_eq!(ws.col_widths.get(&2), Some(&8.0));
        assert_eq!(ws.col_widths.get(&3), Some(&8.0));
        assert_eq!(ws.row_heights.get(&0), Some(&30.0));
        assert_eq!(ws.merges.len(), 1);
        assert_eq!(ws.merges[0].end.col, 2);
    }

    #[test]
    fn shared_formula_member_reuses_master_text() {
        let xml = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
    <row r="1">
      <c r="A1" t="n"><f t="shared" ref="A1:A2" si="0">B1*2</f><v>10</v></c>
    </row>
    <row r="2">
      <c r="A2" t="n"><f t="shared" si="0"/><v>20</v></c>
    </row>
  </sheetData>
</worksheet>"#;
        let ws = parse(xml, &shared()).unwrap();
        let a1 = ws.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        let a2 = ws.cells.iter().find(|c| c.row == 1 && c.col == 0).unwrap();
        assert_eq!(a1.formula.as_deref(), Some("B1*2"));
        // T0 honest limitation: member reuses master text verbatim.
        assert_eq!(a2.formula.as_deref(), Some("B1*2"));
    }

    #[test]
    fn captures_unknown_worksheet_children() {
        let xml = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetPr><tabColor rgb="FFFF0000"/></sheetPr>
  <sheetData>
    <row r="1"><c r="A1" t="n"><v>1</v></c></row>
  </sheetData>
  <conditionalFormatting sqref="A1"><cfRule type="top10" priority="1"/></conditionalFormatting>
  <pageMargins left="0.7"/>
</worksheet>"#;
        let ws = parse(xml, &shared()).unwrap();
        let before: Vec<String> = ws
            .captured
            .before()
            .map(|c| String::from_utf8_lossy(&c.bytes).into_owned())
            .collect();
        let after: Vec<String> = ws
            .captured
            .after()
            .map(|c| String::from_utf8_lossy(&c.bytes).into_owned())
            .collect();
        assert_eq!(before.len(), 1);
        assert!(before[0].contains("sheetPr"));
        assert!(before[0].contains("FFFF0000"));
        assert_eq!(after.len(), 2);
        assert!(after[0].contains("conditionalFormatting"));
        assert!(after[1].contains("pageMargins"));
    }
}
