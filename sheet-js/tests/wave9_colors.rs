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

#![allow(non_snake_case)] // `__feat__<id>` test-name links (cockpit rule)

//! Wave 9 — `get_style` reports theme and indexed colours resolved: through
//! the workbook's theme part (`/theme` relationship), its own
//! `<indexedColors>` palette, and each reference's `tint`.

use std::io::Write;

use sheet_js::core::SheetSession;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<fonts count="3"><font><sz val="11"/><name val="Calibri"/></font><font><color theme="6"/><sz val="11"/><name val="Calibri"/></font><font><color indexed="10"/><sz val="11"/><name val="Calibri"/></font></fonts>
<fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor theme="4" tint="0.79998168889431442"/></patternFill></fill></fills>
<borders count="2"><border><left/><right/><top/><bottom/></border><border><left/><right/><top style="thin"><color indexed="2"/></top><bottom style="thin"><color theme="1" tint="0.5"/></bottom></border></borders>
<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
<cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="0" fontId="1" fillId="2" borderId="1"/><xf numFmtId="0" fontId="2" fillId="0" borderId="0"/></cellXfs>
<colors><indexedColors><rgbColor rgb="FF000000"/><rgbColor rgb="FFFFFFFF"/><rgbColor rgb="FFFF0000"/><rgbColor rgb="FF00FF00"/><rgbColor rgb="FF0000FF"/><rgbColor rgb="FFFFFF00"/><rgbColor rgb="FFFF00FF"/><rgbColor rgb="FF00FFFF"/><rgbColor rgb="FF000000"/><rgbColor rgb="FFFFFFFF"/><rgbColor rgb="FF123456"/></indexedColors></colors>
</styleSheet>"#;

const THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Custom"><a:themeElements><a:clrScheme name="Custom">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2>
<a:accent3><a:srgbClr val="9BBB59"/></a:accent3><a:accent4><a:srgbClr val="8064A2"/></a:accent4>
<a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6>
<a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink>
</a:clrScheme></a:themeElements></a:theme>"#;

fn package(with_theme: bool) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        let mut add = |name: &str, body: &str| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        add(
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/></Types>"#,
        );
        add(
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        );
        add(
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        );
        let theme_rel = if with_theme {
            r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>"#
        } else {
            ""
        };
        add(
            "xl/_rels/workbook.xml.rels",
            &format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>{theme_rel}</Relationships>"#
            ),
        );
        add(
            "xl/worksheets/sheet1.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>1</v></c><c r="B1" s="2"><v>2</v></c></row></sheetData></worksheet>"#,
        );
        add("xl/styles.xml", STYLES);
        if with_theme {
            add("xl/theme/theme1.xml", THEME);
        }
        zip.finish().unwrap();
    }
    buf
}

#[test]
fn get_style_resolves_theme_indexed_and_tint__feat__sheet_format_cell_style() {
    let s = SheetSession::load_xlsx(&package(true)).unwrap();
    let a1 = s.get_style(0, 0, 0).unwrap();
    // font theme 6 = accent3 of THIS theme (not the Office default).
    assert_eq!(a1.font_color.as_deref(), Some("#9BBB59"));
    // fill theme 4 (accent1 #4F81BD) lightened 80 % — Excel's own swatch.
    assert_eq!(a1.fill.as_deref(), Some("#DCE6F2"));
    // border: indexed 2 from the workbook palette; theme 1 (dk1) tinted 50 %.
    assert_eq!(a1.border_top.unwrap().color.as_deref(), Some("#FF0000"));
    assert_eq!(a1.border_bottom.unwrap().color.as_deref(), Some("#808080"));
    // indexed 10: the workbook's OWN palette entry, not the legacy table.
    assert_eq!(
        s.get_style(0, 0, 1).unwrap().font_color.as_deref(),
        Some("#123456")
    );
}

#[test]
fn without_a_theme_part_the_default_scheme_still_answers__feat__sheet_format_cell_style() {
    let s = SheetSession::load_xlsx(&package(false)).unwrap();
    let a1 = s.get_style(0, 0, 0).unwrap();
    // accent3 is beyond the best-effort default slots: unknown, not guessed.
    assert_eq!(a1.font_color.as_deref(), Some(""));
    // accent1 is known to the default; its tint still applies.
    assert_eq!(a1.fill.as_deref(), Some("#DAE3F3"));
}
