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

//! `metadata.xml` — the workbook's cell metadata (ECMA-376 Part 1), read for
//! one thing: which `<c cm="N">` values mark a DYNAMIC-ARRAY formula.
//!
//! Excel stores a spilled formula as an array formula over its spill range
//! (`<f t="array" ref="A8:A31">`) whose cell carries `cm="N"`: the N-th
//! (1-based) `<cellMetadata>` block, which names (`<rc t v>`) the `XLDAPR`
//! metadata type and its `futureMetadata` record — an
//! `xda:dynamicArrayProperties fDynamic="1"` extension
//! ([MS-XLSX] `CT_DynamicArrayProperties`). An array formula without
//! that marker is a legacy CSE formula.
//!
//! The part stays an OPAQUE package part (it may also carry rich-value
//! metadata Paged does not interpret). When a save needs a dynamic-array
//! marker the part lacks, [`with_dynamic_array_record`] splices one in —
//! appended, so every existing index (`t=`, `v=`, `cm=`, `vm=`) keeps its
//! meaning; [`NEW_PART`] is the whole part for a package that has none.

use std::collections::{BTreeMap, BTreeSet};

use quick_xml::events::Event;

use crate::error::XlsxError;
use crate::splice::{attr_local, attrs_of, children_of, render_tag, root_children, splice, Span};

/// The `[Content_Types]` content type of the part.
pub const CT_SHEET_METADATA: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml";
/// The workbook → metadata relationship type.
pub const REL_TYPE_SHEET_METADATA: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata";
/// The relationship type suffix (matched with `ends_with`).
pub const REL_SHEET_METADATA: &str = "/sheetMetadata";

/// The metadata type name of dynamic-array properties.
const XLDAPR: &str = "XLDAPR";
/// The extension namespace of `dynamicArrayProperties`.
const XDA_NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray";
/// The `<ext uri>` that carries `dynamicArrayProperties`.
const XDA_EXT_URI: &str = "{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}";
/// The `<metadataType>` attributes Excel writes for `XLDAPR`.
const XLDAPR_TYPE_ATTRS: &str = r#"minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1""#;

/// A whole `metadata.xml` holding one dynamic-array record, `cm="1"` — the
/// part Excel writes for a workbook whose only cell metadata is dynamic
/// arrays.
pub const NEW_PART: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray">"#,
    r#"<metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/></metadataTypes>"#,
    r#"<futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata>"#,
    r#"<cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata>"#,
    "</metadata>"
);

/// What the part says about dynamic arrays.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DynamicArrayMetadata {
    /// Every `cm` value that marks a dynamic-array formula.
    pub dynamic: BTreeSet<u32>,
    /// The `cm` value a writer should use for a new dynamic-array anchor: the
    /// smallest dynamic one that is not collapsed (`fCollapsed="1"`).
    pub writable: Option<u32>,
}

/// One `dynamicArrayProperties` record: (fDynamic, fCollapsed).
type DaProps = (bool, bool);

/// Parse the part. Content it does not understand is ignored.
pub fn parse(xml: &[u8]) -> Result<DynamicArrayMetadata, XlsxError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut types: Vec<String> = Vec::new();
    // futureMetadata name → its records, in order (None: no dynamic-array
    // properties in that record).
    let mut future: BTreeMap<String, Vec<Option<DaProps>>> = BTreeMap::new();
    // cellMetadata blocks: each a list of (t, v).
    let mut cells: Vec<Vec<(u32, u32)>> = Vec::new();

    let mut section: Option<String> = None; // root child we are inside
    let mut future_name: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf)? {
            ev @ (Event::Start(_) | Event::Empty(_)) => {
                let (e, empty) = match &ev {
                    Event::Start(e) => (e, false),
                    Event::Empty(e) => (e, true),
                    _ => unreachable!(),
                };
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                match (section.as_deref(), local.as_str()) {
                    (None, "metadataTypes" | "futureMetadata" | "cellMetadata") => {
                        if local == "futureMetadata" {
                            let name = crate::opc::attr(e, b"name")?.unwrap_or_default();
                            future.entry(name.clone()).or_default();
                            future_name = Some(name);
                        }
                        if !empty {
                            section = Some(local);
                        }
                    }
                    (Some("metadataTypes"), "metadataType") => {
                        types.push(crate::opc::attr(e, b"name")?.unwrap_or_default());
                    }
                    (Some("futureMetadata"), "bk") => {
                        if let Some(n) = &future_name {
                            future.entry(n.clone()).or_default().push(None);
                        }
                    }
                    (Some("futureMetadata"), "dynamicArrayProperties") => {
                        let flag = |v: Option<String>| v.is_some_and(|v| v == "1" || v == "true");
                        let props = (
                            flag(crate::opc::attr(e, b"fDynamic")?),
                            flag(crate::opc::attr(e, b"fCollapsed")?),
                        );
                        if let Some(last) = future_name
                            .as_ref()
                            .and_then(|n| future.get_mut(n))
                            .and_then(|v| v.last_mut())
                        {
                            *last = Some(props);
                        }
                    }
                    (Some("cellMetadata"), "bk") => cells.push(Vec::new()),
                    (Some("cellMetadata"), "rc") => {
                        let num = |v: Option<String>| v.and_then(|v| v.parse::<u32>().ok());
                        if let (Some(t), Some(v), Some(block)) = (
                            num(crate::opc::attr(e, b"t")?),
                            num(crate::opc::attr(e, b"v")?),
                            cells.last_mut(),
                        ) {
                            block.push((t, v));
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                let local = e.local_name();
                if section.as_deref().map(str::as_bytes) == Some(local.as_ref()) {
                    section = None;
                    future_name = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    let mut out = DynamicArrayMetadata::default();
    for (i, block) in cells.iter().enumerate() {
        let cm = i as u32 + 1;
        for &(t, v) in block {
            let Some(name) = t.checked_sub(1).and_then(|t| types.get(t as usize)) else {
                continue;
            };
            if name != XLDAPR {
                continue;
            }
            let props = future
                .get(XLDAPR)
                .and_then(|recs| recs.get(v as usize))
                .copied()
                .flatten();
            if let Some((true, collapsed)) = props {
                out.dynamic.insert(cm);
                if !collapsed && out.writable.is_none() {
                    out.writable = Some(cm);
                }
            }
        }
    }
    Ok(out)
}

/// `xml` with a non-collapsed dynamic-array record added, and that record's
/// `cm` value. Existing types, records and blocks keep their positions — the
/// new ones are appended — so every index already in use keeps its meaning.
pub fn with_dynamic_array_record(xml: &[u8]) -> Result<(Vec<u8>, u32), XlsxError> {
    if let Some(cm) = parse(xml)?.writable {
        return Ok((xml.to_vec(), cm));
    }
    let mut xml = xml.to_vec();

    // 1. The XLDAPR metadata type (its 1-based index is `t`).
    let (kids, close) = root_children(&xml)?;
    if close >= xml.len() {
        // A self-closing (or unterminated) root has nowhere to append to.
        return Err(XlsxError::Structure(
            "metadata part has no root element to extend".into(),
        ));
    }
    let p = kids
        .first()
        .map(|k| crate::splice::prefix_of(&k.qname).to_string())
        .unwrap_or_default();
    let type_tag = format!(r#"<{p}metadataType name="{XLDAPR}" {XLDAPR_TYPE_ATTRS}/>"#);
    let t = match kids.iter().find(|k| k.local == "metadataTypes") {
        Some(mt) => {
            let (items, at) = children_of(&xml, mt)?;
            let types: Vec<&Span> = items.iter().filter(|s| s.local == "metadataType").collect();
            let mut found = None;
            for (i, s) in types.iter().enumerate() {
                if attr_local(&attrs_of(&xml, s)?, "name").as_deref() == Some(XLDAPR) {
                    found = Some(i as u32 + 1);
                    break;
                }
            }
            match found {
                Some(t) => t,
                None => {
                    let n = types.len() as u32 + 1;
                    xml = append_child(&xml, mt, at, &type_tag, n)?;
                    n
                }
            }
        }
        None => {
            let at = kids.first().map(|k| k.start).unwrap_or(close);
            let block = format!(r#"<{p}metadataTypes count="1">{type_tag}</{p}metadataTypes>"#);
            xml = splice(&xml, at, at, block.as_bytes());
            1
        }
    };

    // 2. A non-collapsed record in futureMetadata name="XLDAPR" (`v`, 0-based).
    let record = format!(
        r#"<{p}bk><{p}extLst><{p}ext uri="{XDA_EXT_URI}" xmlns:xda="{XDA_NS}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></{p}ext></{p}extLst></{p}bk>"#
    );
    let (kids, close) = root_children(&xml)?;
    let mut fm_xldapr = None;
    for k in kids.iter().filter(|k| k.local == "futureMetadata") {
        if attr_local(&attrs_of(&xml, k)?, "name").as_deref() == Some(XLDAPR) {
            fm_xldapr = Some(k.clone());
            break;
        }
    }
    let v = match fm_xldapr {
        Some(fm) => {
            let (items, at) = children_of(&xml, &fm)?;
            let bks: Vec<&Span> = items.iter().filter(|s| s.local == "bk").collect();
            let mut found = None;
            for (i, bk) in bks.iter().enumerate() {
                if record_is_writable(&xml[bk.start..bk.end])? {
                    found = Some(i as u32);
                    break;
                }
            }
            match found {
                Some(v) => v,
                None => {
                    let v = bks.len() as u32;
                    xml = append_child(&xml, &fm, at, &record, v + 1)?;
                    v
                }
            }
        }
        None => {
            // After the last of metadataTypes / metadataStrings / mdxMetadata
            // / futureMetadata (CT_Metadata child order).
            let at = kids
                .iter()
                .filter(|k| {
                    matches!(
                        k.local.as_str(),
                        "metadataTypes" | "metadataStrings" | "mdxMetadata" | "futureMetadata"
                    )
                })
                .map(|k| k.end)
                .next_back()
                .unwrap_or(close);
            let block = format!(
                r#"<{p}futureMetadata name="{XLDAPR}" count="1">{record}</{p}futureMetadata>"#
            );
            xml = splice(&xml, at, at, block.as_bytes());
            0
        }
    };

    // 3. A cellMetadata block pointing at it (`cm`, 1-based).
    let block = format!(r#"<{p}bk><{p}rc t="{t}" v="{v}"/></{p}bk>"#);
    let (kids, close) = root_children(&xml)?;
    let cm = match kids.iter().find(|k| k.local == "cellMetadata") {
        Some(cmd) => {
            let (items, at) = children_of(&xml, cmd)?;
            let n = items.iter().filter(|s| s.local == "bk").count() as u32 + 1;
            xml = append_child(&xml, cmd, at, &block, n)?;
            n
        }
        None => {
            let at = kids
                .iter()
                .find(|k| matches!(k.local.as_str(), "valueMetadata" | "extLst"))
                .map(|k| k.start)
                .unwrap_or(close);
            let whole = format!(r#"<{p}cellMetadata count="1">{block}</{p}cellMetadata>"#);
            xml = splice(&xml, at, at, whole.as_bytes());
            1
        }
    };

    debug_assert_eq!(parse(&xml).ok().and_then(|m| m.writable), Some(cm));
    Ok((xml, cm))
}

/// Append `child` inside `parent` (whose children end at `at`) and set its
/// `count` to `count`. A self-closing parent is opened up.
fn append_child(
    xml: &[u8],
    parent: &Span,
    at: usize,
    child: &str,
    count: u32,
) -> Result<Vec<u8>, XlsxError> {
    let mut attrs = attrs_of(xml, parent)?;
    crate::splice::set_attr(&mut attrs, "count", Some(count.to_string()));
    if parent.empty {
        let whole = format!(
            "{}{child}</{}>",
            render_tag(&parent.qname, &attrs, false),
            parent.qname
        );
        return Ok(splice(xml, parent.start, parent.end, whole.as_bytes()));
    }
    // The later offset first, so the earlier one stays valid.
    let out = splice(xml, at, at, child.as_bytes());
    Ok(splice(
        &out,
        parent.start,
        parent.tag_end,
        render_tag(&parent.qname, &attrs, false).as_bytes(),
    ))
}

/// Whether one futureMetadata `<bk>` is a dynamic, non-collapsed record.
fn record_is_writable(bk: &[u8]) -> Result<bool, XlsxError> {
    let mut reader = quick_xml::Reader::from_reader(bk);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e)
                if e.local_name().as_ref() == b"dynamicArrayProperties" =>
            {
                let flag = |v: Option<String>| v.is_some_and(|v| v == "1" || v == "true");
                return Ok(flag(crate::opc::attr(&e, b"fDynamic")?)
                    && !flag(crate::opc::attr(&e, b"fCollapsed")?));
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buf.clear();
    }
}

#[cfg(test)]
#[allow(non_snake_case)] // `__feat__<id>` test-name links (cockpit rule)
mod tests {
    use super::*;

    /// The part Excel 16 wrote for a workbook with dynamic arrays and one
    /// error carrying rich-value details (the types in Excel's order).
    const EXCEL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xlrd="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="2"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/><metadataType name="XLRICHVALUE" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><futureMetadata name="XLRICHVALUE" count="1"><bk><extLst><ext uri="{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}"><xlrd:rvb i="0"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata><valueMetadata count="1"><bk><rc t="2" v="0"/></bk></valueMetadata></metadata>"#;

    /// Rich values only (no dynamic arrays) — Excel's form.
    const RICH_ONLY: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xlrd="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata"><metadataTypes count="1"><metadataType name="XLRICHVALUE" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1"/></metadataTypes><futureMetadata name="XLRICHVALUE" count="1"><bk><extLst><ext uri="{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}"><xlrd:rvb i="0"/></ext></extLst></bk></futureMetadata><valueMetadata count="1"><bk><rc t="1" v="0"/></bk></valueMetadata></metadata>"#;

    #[test]
    fn excels_part_names_cm_1_dynamic__feat__sheet_xlsx_roundtrip() {
        let m = parse(EXCEL.as_bytes()).unwrap();
        assert_eq!(m.dynamic, BTreeSet::from([1]));
        assert_eq!(m.writable, Some(1));
        let (same, cm) = with_dynamic_array_record(EXCEL.as_bytes()).unwrap();
        assert_eq!((same.as_slice(), cm), (EXCEL.as_bytes(), 1), "reused as is");
    }

    #[test]
    fn the_new_part_parses_as_one_dynamic_record__feat__sheet_xlsx_roundtrip() {
        let m = parse(NEW_PART.as_bytes()).unwrap();
        assert_eq!(m.dynamic, BTreeSet::from([1]));
        assert_eq!(m.writable, Some(1));
    }

    #[test]
    fn rich_values_only_mean_no_dynamic_arrays__feat__sheet_xlsx_roundtrip() {
        assert_eq!(
            parse(RICH_ONLY.as_bytes()).unwrap(),
            DynamicArrayMetadata::default()
        );
    }

    #[test]
    fn a_record_is_appended_and_every_index_in_use_keeps_its_meaning__feat__sheet_xlsx_roundtrip() {
        let (out, cm) = with_dynamic_array_record(RICH_ONLY.as_bytes()).unwrap();
        let s = String::from_utf8(out.clone()).unwrap();
        assert_eq!(cm, 1);
        assert!(
            s.contains(r#"<metadataTypes count="2"><metadataType name="XLRICHVALUE""#),
            "{s}"
        );
        assert!(
            s.contains(r#"coerce="1"/><metadataType name="XLDAPR""#),
            "{s}"
        );
        assert!(
            s.contains(r#"</futureMetadata><futureMetadata name="XLDAPR" count="1"><bk>"#),
            "{s}"
        );
        assert!(
            s.contains(r#"<cellMetadata count="1"><bk><rc t="2" v="0"/></bk></cellMetadata><valueMetadata count="1"><bk><rc t="1" v="0"/></bk></valueMetadata>"#),
            "{s}"
        );
        assert_eq!(parse(&out).unwrap().writable, Some(1));
    }

    #[test]
    fn an_empty_root_is_refused_not_corrupted__feat__sheet_xlsx_roundtrip() {
        let empty = r#"<?xml version="1.0"?><metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>"#;
        assert!(with_dynamic_array_record(empty.as_bytes()).is_err());
    }

    #[test]
    fn a_collapsed_only_record_gains_a_writable_one__feat__sheet_xlsx_roundtrip() {
        let collapsed = NEW_PART.replace(r#"fCollapsed="0""#, r#"fCollapsed="1""#);
        let m = parse(collapsed.as_bytes()).unwrap();
        assert_eq!((m.dynamic, m.writable), (BTreeSet::from([1]), None));
        let (out, cm) = with_dynamic_array_record(collapsed.as_bytes()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(cm, 2);
        assert!(
            s.contains(r#"<metadataTypes count="1">"#),
            "the type is reused: {s}"
        );
        assert!(
            s.contains(r#"<futureMetadata name="XLDAPR" count="2">"#),
            "{s}"
        );
        assert!(
            s.contains(r#"<cellMetadata count="2"><bk><rc t="1" v="0"/></bk><bk><rc t="1" v="1"/></bk></cellMetadata>"#),
            "{s}"
        );
    }
}
