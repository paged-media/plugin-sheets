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

#![allow(non_snake_case)] // `__feat__<id>` test-name links (cockpit rule)

//! Wave 6 formatting & layout, end to end through `SheetSession` (the logic
//! the wasm shim forwards to): cell styles, merges, column widths / row
//! heights, frozen panes, repeated header rows when paginating, and defined
//! names — each followed through `save_xlsx` and a reload, so the package the
//! writer produces is what the assertions read.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use sheet_js::core::SheetSession;

fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), b);
    }
    out
}

fn part_text(bytes: &[u8], name: &str) -> String {
    String::from_utf8(parts(bytes).remove(name).unwrap_or_default()).unwrap()
}

/// A one-sheet package. `workbook_extra` lands in `<workbook>` after
/// `<sheets>`; `ws_body` is the whole inside of `<worksheet>`; `styles` (when
/// given) is the `xl/styles.xml` part.
fn package(workbook_extra: &str, ws_body: &str, styles: Option<&str>) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        let mut add = |name: &str, body: String| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        let styles_ct = if styles.is_some() {
            r#"<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#
        } else {
            ""
        };
        add("[Content_Types].xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>{styles_ct}</Types>"#));
        add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into());
        add(
            "xl/workbook.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>{workbook_extra}</workbook>"#
            ),
        );
        let styles_rel = if styles.is_some() {
            r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#
        } else {
            ""
        };
        add("xl/_rels/workbook.xml.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>{styles_rel}</Relationships>"#));
        add(
            "xl/worksheets/sheet1.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{ws_body}</worksheet>"#
            ),
        );
        if let Some(st) = styles {
            add("xl/styles.xml", st.to_string());
        }
        zip.finish().unwrap();
    }
    buf
}

/// Byte offset of the first `<name` element (exact local name) in `xml`.
fn pos_of(xml: &str, name: &str) -> usize {
    for (i, _) in xml.match_indices(&format!("<{name}")) {
        let next = xml.as_bytes()[i + name.len() + 1];
        if next == b' ' || next == b'>' || next == b'/' {
            return i;
        }
    }
    panic!("<{name}> not in {xml}")
}

const SHEET1: &str = "xl/worksheets/sheet1.xml";

// ── 0. the dirty re-encode keeps schema order (defects found on the way) ──

/// ECMA-376 fixes `<worksheet>`'s child order. A dirty re-encode used to put
/// `<dimension>` AFTER the captured `<sheetViews>` and `<mergeCells>` BEFORE
/// a captured `<autoFilter>` — both orders Excel repairs as corrupt.
#[test]
fn dirty_reencode_keeps_worksheet_child_order__feat__sheet_xlsx_roundtrip() {
    let body = r#"<sheetPr/><dimension ref="A1:B2"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1"><v>2</v></c></row></sheetData><autoFilter ref="A1:B2"/><mergeCells count="1"><mergeCell ref="A2:B2"/></mergeCells><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>"#;
    let mut s = SheetSession::load_xlsx(&package("", body, None)).unwrap();
    s.set_cell(0, 0, 0, "5").unwrap();
    let out = s.save_xlsx().unwrap();
    let xml = part_text(&out, SHEET1);
    let order = [
        "sheetPr",
        "dimension",
        "sheetViews",
        "sheetFormatPr",
        "sheetData",
        "autoFilter",
        "mergeCells",
        "pageMargins",
    ];
    let at: Vec<usize> = order.iter().map(|n| pos_of(&xml, n)).collect();
    assert!(
        at.windows(2).all(|w| w[0] < w[1]),
        "child order {order:?} at {at:?} in {xml}"
    );
}

/// A row that carries only a custom height (no cells) survived a zero-edit
/// save but was dropped by a dirty re-encode.
#[test]
fn height_only_row_survives_reencode__feat__sheet_xlsx_roundtrip() {
    let body = r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row><row r="5" ht="30" customHeight="1"/></sheetData>"#;
    let mut s = SheetSession::load_xlsx(&package("", body, None)).unwrap();
    s.set_cell(0, 0, 0, "2").unwrap();
    let out = s.save_xlsx().unwrap();
    let xml = part_text(&out, SHEET1);
    assert!(
        xml.contains(r#"<row r="5" ht="30" customHeight="1""#),
        "height-only row lost: {xml}"
    );
}
