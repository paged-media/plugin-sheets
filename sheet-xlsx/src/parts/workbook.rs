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

//! `workbook.xml` — the workbook part (ECMA-376 §18.2).
//!
//! T0 reads: the sheet list (`<sheet name r:id sheetId>`, in order), the
//! `date1904` flag from `<workbookPr>`, and `<definedName>` rows (folded
//! into the model's [`NameTable`] as raw `Formula` text — the parser is not
//! a dependency of the XLSX layer, so names resolve in the consumer, T1).

use crate::error::XlsxError;
use crate::opc::attr;
use compact_str::CompactString;
use sheet_core::calc_settings::DateSystem;
use sheet_core::names::{NameDef, NameScope, NameTarget};
use sheet_core::NameTable;

/// One `<sheet>` row: display name + the relationship id that points at its
/// worksheet part.
#[derive(Debug, Clone)]
pub struct SheetRef {
    pub name: String,
    pub rid: String,
}

/// What `workbook.xml` yields.
pub struct ParsedWorkbook {
    /// Sheets in tab order.
    pub sheets: Vec<SheetRef>,
    /// `date1904` -> 1904 epoch; default 1900.
    pub date_system: DateSystem,
    /// Defined names, folded into the model's table (raw formula text).
    pub names: NameTable,
}

/// Parse `workbook.xml`.
pub fn parse(xml: &[u8]) -> Result<ParsedWorkbook, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    let mut sheets = Vec::new();
    let mut date_system = DateSystem::Date1900;
    let mut names = NameTable::default();

    // definedName carries its target as element text; track the in-progress
    // name + accumulate its text.
    let mut cur_name: Option<(String, NameScope)> = None;
    let mut cur_text = String::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Empty(e) => {
                // Self-closing forms: <workbookPr/>, <sheet/>. A self-closing
                // <definedName/> is degenerate (no target text) and ignored.
                let local = e.local_name();
                match local.as_ref() {
                    b"workbookPr" => {
                        if let Some(v) = attr(&e, b"date1904")? {
                            if is_xml_true(&v) {
                                date_system = DateSystem::Date1904;
                            }
                        }
                    }
                    b"sheet" => {
                        sheets.push(parse_sheet(&e)?);
                    }
                    _ => {}
                }
            }
            Event::Start(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"workbookPr" => {
                        if let Some(v) = attr(&e, b"date1904")? {
                            if is_xml_true(&v) {
                                date_system = DateSystem::Date1904;
                            }
                        }
                    }
                    b"sheet" => {
                        sheets.push(parse_sheet(&e)?);
                    }
                    b"definedName" => {
                        let name = attr(&e, b"name")?.unwrap_or_default();
                        let scope =
                            match attr(&e, b"localSheetId")?.and_then(|s| s.parse::<u16>().ok()) {
                                Some(idx) => NameScope::Sheet(idx),
                                None => NameScope::Workbook,
                            };
                        cur_name = Some((name, scope));
                        cur_text.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(t) if cur_name.is_some() => {
                let s = t.unescape().map_err(XlsxError::Xml)?;
                cur_text.push_str(&s);
            }
            Event::End(e) if e.local_name().as_ref() == b"definedName" => {
                if let Some((name, scope)) = cur_name.take() {
                    names.define(NameDef {
                        name: CompactString::new(&name),
                        scope,
                        target: NameTarget::Formula(CompactString::new(cur_text.trim())),
                    });
                }
                cur_text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if sheets.is_empty() {
        return Err(XlsxError::Structure("workbook has no sheets".into()));
    }

    Ok(ParsedWorkbook {
        sheets,
        date_system,
        names,
    })
}

/// Parse one `<sheet name r:id>` row.
fn parse_sheet(e: &quick_xml::events::BytesStart<'_>) -> Result<SheetRef, XlsxError> {
    let name =
        attr(e, b"name")?.ok_or_else(|| XlsxError::Structure("<sheet> missing name".into()))?;
    // r:id — match by local name "id" (the r: prefix is dropped by local_name).
    let rid = attr(e, b"id")?.ok_or_else(|| XlsxError::Structure("<sheet> missing r:id".into()))?;
    Ok(SheetRef { name, rid })
}

/// XML boolean (ECMA-376 §22.9.2.7): `1`/`true` are true, `0`/`false`/absent
/// are false.
fn is_xml_true(s: &str) -> bool {
    matches!(s, "1" | "true" | "True" | "TRUE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheets_in_order_and_date1904() {
        let xml = br#"<?xml version="1.0"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <workbookPr date1904="1"/>
  <sheets>
    <sheet name="Summary" sheetId="1" r:id="rId1"/>
    <sheet name="Data"    sheetId="2" r:id="rId2"/>
    <sheet name="Notes"   sheetId="3" r:id="rId3"/>
  </sheets>
  <definedName name="TaxRate">0.2</definedName>
  <definedName name="Region" localSheetId="1">Data!$A$1:$A$10</definedName>
</workbook>"#;
        let wb = parse(xml).unwrap();
        assert_eq!(wb.date_system, DateSystem::Date1904);
        let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Summary", "Data", "Notes"]);
        assert_eq!(wb.sheets[1].rid, "rId2");
        // names folded
        let defs: Vec<_> = wb.names.iter().collect();
        assert_eq!(defs.len(), 2);
        match &defs[0].1.target {
            NameTarget::Formula(f) => assert_eq!(f.as_str(), "0.2"),
            _ => panic!("expected formula target"),
        }
        assert_eq!(defs[1].1.scope, NameScope::Sheet(1));
    }

    #[test]
    fn default_date_system_is_1900() {
        let xml = br#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#;
        let wb = parse(xml).unwrap();
        assert_eq!(wb.date_system, DateSystem::Date1900);
    }
}
