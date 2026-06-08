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

//! `xl/tables/tableN.xml` — an Excel structured table (a `ListObject`,
//! ECMA-376 §18.5 / §18.5.1.2 `table`). The part defines one named, columned
//! rectangular region; a structured reference (`Table1[Col]`, parsed in
//! `sheet-parser`) addresses it symbolically. T0 reads it into the frozen
//! [`sheet_core::Table`] model so structured refs RESOLVE; the part's bytes
//! stay opaque/lazy-verbatim for the preservation invariant (spec §10.2) — we
//! understand it, we don't rewrite it.
//!
//! ## What we read
//!
//! - `name` (falling back to `displayName`) — the workbook-scoped table name;
//! - `ref` — the FULL extent range (header + body + totals rows);
//! - `headerRowCount` — `1` by default, `0` for a headerless table;
//! - `totalsRowCount` — `0` by default, `1` when a totals row is present;
//! - `<tableColumns><tableColumn name="…"/>` — the header labels, left to
//!   right (their order is the column order within `ref`);
//! - the table style id from `<tableStyleInfo name="…"/>` (carried for
//!   round-trip / lowering).
//!
//! Everything else (`autoFilter`, `sortState`, calculated-column formulas,
//! `dxf` references, `extLst`) is ignored at the MODEL layer — it survives
//! because the part re-emits verbatim.

use crate::error::XlsxError;
use crate::opc::attr;
use compact_str::CompactString;
use sheet_core::{parse_a1, CellRef, RangeRef, Table};

/// Parse a `tableN.xml` part into a [`Table`]. `sheet` is the owning worksheet's
/// [`sheet_core::SheetId`]; the `ref` range is anchored to it.
pub fn parse(xml: &[u8], sheet: sheet_core::SheetId) -> Result<Table, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    let mut name: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut range: Option<RangeRef> = None;
    // ECMA-376 defaults: a table has a header row and no totals row.
    let mut header_row_count: u32 = 1;
    let mut totals_row_count: u32 = 0;
    let mut columns: Vec<CompactString> = Vec::new();
    let mut style_name: Option<CompactString> = None;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"table" => {
                    name = attr(&e, b"name")?;
                    display_name = attr(&e, b"displayName")?;
                    if let Some(rf) = attr(&e, b"ref")? {
                        range = parse_ref(&rf, sheet);
                    }
                    if let Some(h) = attr(&e, b"headerRowCount")? {
                        header_row_count = h.trim().parse().unwrap_or(1);
                    }
                    if let Some(t) = attr(&e, b"totalsRowCount")? {
                        totals_row_count = t.trim().parse().unwrap_or(0);
                    }
                }
                b"tableColumn" => {
                    if let Some(n) = attr(&e, b"name")? {
                        columns.push(CompactString::new(&n));
                    }
                }
                b"tableStyleInfo" => {
                    style_name = attr(&e, b"name")?.map(|s| CompactString::new(&s));
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    let name = name
        .or(display_name)
        .ok_or_else(|| XlsxError::Structure("<table> missing name/displayName".into()))?;
    let range =
        range.ok_or_else(|| XlsxError::Structure(format!("table {name:?} missing/invalid ref")))?;

    Ok(Table {
        name: CompactString::new(&name),
        range,
        columns,
        // ECMA-376 permits a header/totals ROW COUNT (0 or 1); the model holds
        // a bool (T0: a table has at most one header and one totals row).
        header_row: header_row_count > 0,
        totals_row: totals_row_count > 0,
        style_name,
    })
}

/// Parse a `ref="A1:C10"` (or single-cell `ref="A1"`) table extent on `sheet`.
fn parse_ref(s: &str, sheet: sheet_core::SheetId) -> Option<RangeRef> {
    let mk = |row, col| CellRef {
        sheet,
        row,
        col,
        row_abs: false,
        col_abs: false,
    };
    match s.split_once(':') {
        Some((a, b)) => {
            let (r0, c0, _, _) = parse_a1(a)?;
            let (r1, c1, _, _) = parse_a1(b)?;
            Some(RangeRef {
                start: mk(r0, c0),
                end: mk(r1, c1),
            })
        }
        None => {
            // A degenerate single-cell table extent.
            let (r, c, _, _) = parse_a1(s)?;
            Some(RangeRef {
                start: mk(r, c),
                end: mk(r, c),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_ref_columns_header_totals() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
       id="1" name="Sales" displayName="Sales" ref="A1:C5"
       totalsRowShown="1" totalsRowCount="1" headerRowCount="1">
  <autoFilter ref="A1:C4"/>
  <tableColumns count="3">
    <tableColumn id="1" name="Region"/>
    <tableColumn id="2" name="Units"/>
    <tableColumn id="3" name="Total" totalsRowFunction="sum"/>
  </tableColumns>
  <tableStyleInfo name="TableStyleMedium2" showRowStripes="1"/>
</table>"#;
        let t = parse(xml, 0).unwrap();
        assert_eq!(t.name.as_str(), "Sales");
        // A1:C5 — header row 0, data 1..3, totals row 4.
        assert_eq!(t.range.start.row, 0);
        assert_eq!(t.range.start.col, 0);
        assert_eq!(t.range.end.row, 4);
        assert_eq!(t.range.end.col, 2);
        assert!(t.header_row);
        assert!(t.totals_row);
        assert_eq!(
            t.columns,
            vec![
                CompactString::new("Region"),
                CompactString::new("Units"),
                CompactString::new("Total")
            ]
        );
        assert_eq!(t.style_name.as_deref(), Some("TableStyleMedium2"));
        // column_index resolves case-insensitively (frozen helper).
        assert_eq!(t.column_index("units"), Some(1));
    }

    #[test]
    fn defaults_header_one_totals_zero() {
        // No headerRowCount/totalsRowCount → header present, no totals row.
        let xml = br#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
       name="T1" ref="B2:B6">
  <tableColumns count="1"><tableColumn id="1" name="X"/></tableColumns>
</table>"#;
        let t = parse(xml, 2).unwrap();
        assert_eq!(t.name.as_str(), "T1");
        assert!(t.header_row);
        assert!(!t.totals_row);
        assert_eq!(t.range.start.sheet, 2);
        assert_eq!(t.style_name, None);
    }

    #[test]
    fn headerless_table() {
        let xml = br#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
       name="NoHdr" ref="A1:A3" headerRowCount="0">
  <tableColumns count="1"><tableColumn id="1" name="Column1"/></tableColumns>
</table>"#;
        let t = parse(xml, 0).unwrap();
        assert!(!t.header_row);
        assert!(!t.totals_row);
    }

    #[test]
    fn falls_back_to_display_name() {
        let xml = br#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
       displayName="OnlyDisplay" ref="A1:A2">
  <tableColumns count="1"><tableColumn id="1" name="C"/></tableColumns>
</table>"#;
        let t = parse(xml, 0).unwrap();
        assert_eq!(t.name.as_str(), "OnlyDisplay");
    }

    #[test]
    fn missing_ref_is_error() {
        let xml = br#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
       name="Bad"><tableColumns count="0"/></table>"#;
        assert!(parse(xml, 0).is_err());
    }
}
