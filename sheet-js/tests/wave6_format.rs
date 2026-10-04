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

use sheet_js::core::{EdgeArg, LowerOptions, SheetSession, StylePatchArg};

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
        add(
            "[Content_Types].xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>{styles_ct}</Types>"#
            ),
        );
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
        add(
            "xl/_rels/workbook.xml.rels",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>{styles_rel}</Relationships>"#
            ),
        );
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

// ── 1. cell styles ───────────────────────────────────────────────────────

/// A style sheet with two `<xf>` records the frozen `CellStyle` cannot tell
/// apart (they differ only in vertical alignment and wrap, which the model
/// does not carry) plus a bold font.
const TWIN_XF_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><color theme="1"/><name val="Calibri"/><scheme val="minor"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1" applyAlignment="1"><alignment vertical="top" wrapText="1"/></xf></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;

/// A cell's `s=` index is its `StyleId`. Two `<xf>` records that differ only
/// in an attribute the model did not read used to intern to ONE id, so a
/// dirty re-encode wrote the wrapped header back as the unwrapped one.
#[test]
fn style_ids_stay_positional_through_a_dirty_save__feat__sheet_xlsx_roundtrip() {
    let body = r#"<sheetData><row r="1"><c r="A1" s="2" t="inlineStr"><is><t>Head</t></is></c><c r="B1" s="1"><v>1</v></c></row></sheetData>"#;
    let mut s = SheetSession::load_xlsx(&package("", body, Some(TWIN_XF_STYLES))).unwrap();
    s.set_cell(0, 1, 1, "7").unwrap();
    let out = s.save_xlsx().unwrap();
    let xml = part_text(&out, SHEET1);
    assert!(
        xml.contains(r#"<c r="A1" s="2""#),
        "wrapped header kept: {xml}"
    );
    assert!(xml.contains(r#"<c r="B1" s="1""#), "{xml}");
}

fn full_patch() -> StylePatchArg {
    StylePatchArg {
        num_fmt: Some("0.00".into()),
        font_name: Some("Arial".into()),
        font_size: Some(14.0),
        bold: Some(true),
        italic: Some(true),
        underline: Some(true),
        font_color: Some("#FF0000".into()),
        fill: Some("#FFFF00".into()),
        border_top: Some(EdgeArg {
            style: "thin".into(),
            color: Some("#00FF00".into()),
        }),
        border_bottom: Some(EdgeArg {
            style: "double".into(),
            color: None,
        }),
        h_align: Some("center".into()),
        v_align: Some("top".into()),
        wrap: Some(true),
        ..Default::default()
    }
}

/// Every facet of a patch lands in `styles.xml`, survives save + reload, and
/// reaches the page lowering — on a workbook that had NO styles part (one is
/// created with its relationship and content-type override).
#[test]
fn set_style_round_trips_every_facet__feat__sheet_format_cell_style() {
    let body = r#"<sheetData><row r="1"><c r="A1"><v>1.5</v></c></row></sheetData>"#;
    let mut s = SheetSession::load_xlsx(&package("", body, None)).unwrap();
    let r = s.set_style(0, "A1:B2", full_patch()).unwrap();
    assert_eq!((r.cells, r.styles), (4, 1));
    assert_eq!(s.get_cell_display(0, 0, 0), "1.50", "number format applies");

    let page = s
        .get_range_page(0, "A1:B2", LowerOptions::default())
        .unwrap();
    let st = &page.styles[page.rows[0].cells[0].style_key as usize];
    assert!(st.bold && st.italic && st.underline && st.wrap);
    assert_eq!(st.font_name.as_deref(), Some("Arial"));
    assert_eq!(st.font_size_pt, Some(14.0));
    assert_eq!(st.text_rgb.as_deref(), Some("#FF0000"));
    assert_eq!(st.fill_rgb.as_deref(), Some("#FFFF00"));
    assert_eq!(st.v_align.as_deref(), Some("top"));
    let top = st.border_lines.top.as_ref().unwrap();
    assert_eq!(
        (top.style.as_str(), top.rgb.as_deref()),
        ("thin", Some("#00FF00"))
    );
    assert_eq!(st.border_lines.bottom.as_ref().unwrap().style, "double");
    assert!(st.border_top && st.border_bottom && !st.border_left);
    assert_eq!(
        format!("{:?}", page.rows[0].cells[0].align),
        "Center",
        "horizontal alignment rides the cell"
    );
    // The blank cells were styled too (B2 has no value).
    assert_eq!(
        page.rows[1].cells[1].style_key,
        page.rows[0].cells[0].style_key
    );

    let out = s.save_xlsx().unwrap();
    let p = parts(&out);
    assert!(p.contains_key("xl/styles.xml"), "a styles part is created");
    let ct = part_text(&out, "[Content_Types].xml");
    assert!(ct.contains("/xl/styles.xml"), "{ct}");
    let rels = part_text(&out, "xl/_rels/workbook.xml.rels");
    assert!(rels.contains("relationships/styles"), "{rels}");

    let s2 = SheetSession::load_xlsx(&out).unwrap();
    for (r, c) in [(0, 0), (1, 1)] {
        let got = s2.get_style(0, r, c).unwrap();
        let want = full_patch();
        assert_eq!(got.num_fmt, want.num_fmt);
        assert_eq!(got.font_name, want.font_name);
        assert_eq!(got.font_size, want.font_size);
        assert_eq!(
            (got.bold, got.italic, got.underline),
            (Some(true), Some(true), Some(true))
        );
        assert_eq!(got.font_color, want.font_color);
        assert_eq!(got.fill, want.fill);
        assert_eq!(got.border_top, want.border_top);
        assert_eq!(
            got.border_bottom.as_ref().map(|e| e.style.as_str()),
            Some("double")
        );
        assert_eq!(
            got.border_left.as_ref().map(|e| e.style.as_str()),
            Some("none")
        );
        assert_eq!(got.h_align.as_deref(), Some("center"));
        assert_eq!(got.v_align.as_deref(), Some("top"));
        assert_eq!(got.wrap, Some(true));
    }
    assert_eq!(s2.get_cell_display(0, 0, 0), "1.50");
}

/// A patch changes only what it names: filling a bold, theme-coloured cell
/// keeps its font record; the same patch twice adds no records; existing
/// records keep their indices.
#[test]
fn set_style_patches_and_dedups__feat__sheet_format_cell_style() {
    let body = r#"<sheetData><row r="1"><c r="A1" s="1"><v>1</v></c><c r="B1" s="1"><v>2</v></c><c r="C1" s="2"><v>3</v></c></row></sheetData>"#;
    let mut s = SheetSession::load_xlsx(&package("", body, Some(TWIN_XF_STYLES))).unwrap();
    let fill = StylePatchArg {
        fill: Some("#DDEEFF".into()),
        ..Default::default()
    };
    s.set_style(0, "A1", fill.clone()).unwrap();
    let a1 = s.get_style(0, 0, 0).unwrap();
    assert_eq!(a1.bold, Some(true), "the font is untouched");
    assert_eq!(a1.fill.as_deref(), Some("#DDEEFF"));
    s.set_style(0, "B1", fill.clone()).unwrap();
    let out = s.save_xlsx().unwrap();
    let styles = part_text(&out, "xl/styles.xml");
    assert!(
        styles.contains(r#"<cellXfs count="4">"#),
        "one new xf, shared: {styles}"
    );
    assert!(
        styles.contains(r#"<fills count="3">"#),
        "one new fill: {styles}"
    );
    assert!(
        styles.contains(r#"<fonts count="2">"#),
        "no new font: {styles}"
    );
    let xml = part_text(&out, SHEET1);
    assert!(
        xml.contains(r#"<c r="A1" s="3""#) && xml.contains(r#"<c r="B1" s="3""#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<c r="C1" s="2""#),
        "an untouched cell keeps its index: {xml}"
    );
    // The original records are byte-identical (appended, never rewritten).
    let orig_xfs = r#"<xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1" applyAlignment="1"><alignment vertical="top" wrapText="1"/></xf>"#;
    assert!(styles.contains(orig_xfs), "{styles}");
}

/// Changing a theme font's name drops its `<scheme>` (Excel ignores `name`
/// on a scheme font); a bad value is refused and changes nothing.
#[test]
fn set_style_font_name_and_refusals__feat__sheet_format_cell_style() {
    let body = r#"<sheetData><row r="1"><c r="A1" s="1"><v>1</v></c></row></sheetData>"#;
    let mut s = SheetSession::load_xlsx(&package("", body, Some(TWIN_XF_STYLES))).unwrap();
    for bad in [
        StylePatchArg {
            fill: Some("yellow".into()),
            ..Default::default()
        },
        StylePatchArg {
            h_align: Some("middle".into()),
            ..Default::default()
        },
        StylePatchArg {
            border_left: Some(EdgeArg {
                style: "wavy".into(),
                color: None,
            }),
            ..Default::default()
        },
    ] {
        assert!(s.set_style(0, "A1", bad).is_err());
    }
    assert!(!s.metadata().dirty, "a refused patch changes nothing");
    s.set_style(
        0,
        "A1",
        StylePatchArg {
            font_name: Some("Georgia".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let out = s.save_xlsx().unwrap();
    let styles = part_text(&out, "xl/styles.xml");
    assert!(
        styles
            .contains(r#"<font><b/><sz val="11"/><color theme="1"/><name val="Georgia"/></font>"#),
        "{styles}"
    );
}

// ── 2. merges ────────────────────────────────────────────────────────────

/// Merging keeps the top-left content and clears the rest (Excel's rule;
/// the cleared inputs come back for the undo journal), the page lowering
/// spans it, the writer persists `<mergeCells>`, and unmerge removes it.
#[test]
fn merge_and_unmerge_round_trip__feat__sheet_layout_merge() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "Title").unwrap();
    s.set_cell(0, 0, 1, "x").unwrap();
    s.set_cell(0, 1, 0, "=B1&\"!\"").unwrap();
    let r = s.merge(0, "A1:C1").unwrap();
    assert_eq!(r.edits.len(), 1);
    assert_eq!((r.edits[0].col, r.edits[0].prev_input.as_str()), (1, "x"));
    assert_eq!(s.get_cell_display(0, 0, 1), "");
    assert_eq!(s.get_cell_display(0, 1, 0), "!", "dependents recalc");
    assert!(
        s.merge(0, "B1:D2").is_err(),
        "an overlapping merge is refused"
    );
    assert!(s.merge(0, "E5").is_err(), "a single cell is not a merge");

    let page = s
        .get_range_page(0, "A1:C2", LowerOptions::default())
        .unwrap();
    assert_eq!(page.merges.len(), 1);
    let m = &page.merges[0];
    assert_eq!((m.row, m.col, m.row_span, m.col_span), (0, 0, 1, 3));

    let out = s.save_xlsx().unwrap();
    assert!(part_text(&out, SHEET1).contains(r#"<mergeCell ref="A1:C1"/>"#));
    let mut s2 = SheetSession::load_xlsx(&out).unwrap();
    assert_eq!(s2.get_layout(0).unwrap().merges, vec!["A1:C1".to_string()]);
    assert_eq!(
        s2.unmerge(0, "B1").unwrap(),
        1,
        "any cell of the merge unmerges it"
    );
    assert!(s2.get_layout(0).unwrap().merges.is_empty());
    let out2 = s2.save_xlsx().unwrap();
    assert!(!part_text(&out2, SHEET1).contains("mergeCell"));
}

// ── 3. column widths / row heights ───────────────────────────────────────

/// Widths (characters) and heights (points) persist to `<cols>` and
/// `<row ht customHeight>`, drive the lowering geometry, and clear back to
/// the default; out-of-range values are refused.
#[test]
fn column_width_and_row_height_round_trip__feat__sheet_layout_sizes() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "a").unwrap();
    s.set_col_width(0, 1, 2, Some(20.0)).unwrap();
    s.set_row_height(0, 0, 0, Some(30.0)).unwrap();
    assert!(s.set_col_width(0, 0, 0, Some(300.0)).is_err());
    assert!(s.set_row_height(0, 0, 0, Some(-1.0)).is_err());

    let page = s
        .get_range_page(0, "A1:C2", LowerOptions::default())
        .unwrap();
    assert!(
        (page.cols[1].width_pt - 105.0).abs() < 1e-9,
        "20 ch = 105 pt"
    );
    assert!((page.cols[2].width_pt - 105.0).abs() < 1e-9);
    assert!((page.rows[0].height_pt - 30.0).abs() < 1e-9);

    let out = s.save_xlsx().unwrap();
    let xml = part_text(&out, SHEET1);
    assert!(
        xml.contains(r#"<col min="2" max="2" width="20" customWidth="1"/>"#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<row r="1" ht="30" customHeight="1""#),
        "{xml}"
    );
    let mut s2 = SheetSession::load_xlsx(&out).unwrap();
    let l = s2.get_layout(0).unwrap();
    assert_eq!(l.col_widths, vec![(1, 20.0), (2, 20.0)]);
    assert_eq!(l.row_heights, vec![(0, 30.0)]);
    s2.set_col_width(0, 1, 2, None).unwrap();
    s2.set_row_height(0, 0, 0, None).unwrap();
    let l = s2.get_layout(0).unwrap();
    assert!(l.col_widths.is_empty() && l.row_heights.is_empty());
}

// ── 4. frozen panes ──────────────────────────────────────────────────────

/// Freezing writes a frozen `<pane>` into the sheet's first `<sheetView>`
/// (created in schema position when the sheet has none), the grid reads it,
/// it survives save + reload, and clearing removes it.
#[test]
fn freeze_set_and_clear_round_trip__feat__sheet_layout_freeze() {
    let body = r#"<sheetPr/><sheetViews><sheetView tabSelected="1" workbookViewId="0"><selection activeCell="B2" sqref="B2"/></sheetView></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>"#;
    let mut s = SheetSession::load_xlsx(&package("", body, None)).unwrap();
    s.set_freeze(0, 1, 2).unwrap();
    let l = s.get_layout(0).unwrap();
    assert_eq!((l.freeze_rows, l.freeze_cols), (1, 2));
    assert_eq!(s.list_freeze_panes().len(), 1);
    let out = s.save_xlsx().unwrap();
    let xml = part_text(&out, SHEET1);
    assert!(
        xml.contains(r#"<sheetView tabSelected="1" workbookViewId="0"><pane xSplit="2" ySplit="1" topLeftCell="C2" activePane="bottomRight" state="frozen"/><selection activeCell="B2" sqref="B2"/></sheetView>"#),
        "{xml}"
    );
    assert!(pos_of(&xml, "sheetViews") < pos_of(&xml, "sheetFormatPr"));
    let mut s2 = SheetSession::load_xlsx(&out).unwrap();
    let l = s2.get_layout(0).unwrap();
    assert_eq!((l.freeze_rows, l.freeze_cols), (1, 2));

    s2.set_freeze(0, 0, 0).unwrap();
    let out2 = s2.save_xlsx().unwrap();
    assert!(!part_text(&out2, SHEET1).contains("<pane"));
    let s3 = SheetSession::load_xlsx(&out2).unwrap();
    assert!(s3.list_freeze_panes().is_empty());
}

/// A sheet with no `<sheetViews>` gets one, after `<sheetPr>` and before
/// `<dimension>`'s followers; rows-only freezes use the bottom-left pane.
#[test]
fn freeze_creates_sheet_views__feat__sheet_layout_freeze() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "h").unwrap();
    s.set_freeze(0, 1, 0).unwrap();
    let out = s.save_xlsx().unwrap();
    let xml = part_text(&out, SHEET1);
    assert!(
        xml.contains(r#"<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>"#),
        "{xml}"
    );
    assert!(pos_of(&xml, "dimension") < pos_of(&xml, "sheetViews"));
    assert!(pos_of(&xml, "sheetViews") < pos_of(&xml, "sheetData"));
}
