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

//! CSV / TSV import (Wave 4). The delimited text becomes an ordinary XLSX
//! package — one sheet, a `styles.xml` carrying the date and percent formats
//! the typed cells need — which then loads through the normal
//! [`SheetSession::load_xlsx`] path. So an imported CSV saves, edits, lowers
//! and round-trips exactly like a workbook, and nothing downstream needs a
//! CSV special case.
//!
//! Typing is locale-aware (`sheet-format`): numbers parse with the locale's
//! decimal/group separators, dates in ISO `yyyy-mm-dd` or the locale's short
//! date order (en `m/d/yyyy`, de `dd.mm.yyyy`, fr/es/it `dd/mm/yyyy`) and
//! render in that pattern; `12%` is a percent; `TRUE`/`FALSE` booleans;
//! everything else text. A leading `=` stays TEXT (a CSV is data, not
//! formulas — no formula injection through an import).

use std::io::Write as _;

use sheet_core::{DateSystem, Locale};
use sheet_format::locale::locale_data;
use sheet_format::number::parse_number_locale;
use sheet_format::serial::ymd_to_serial;

use super::{SessionError, SheetSession};

/// The style index of each typed cell kind in the generated `cellXfs`.
const XF_DATE: u32 = 1;
const XF_PERCENT: u32 = 2;
const XF_PERCENT_2: u32 = 3;

/// Map a BCP-47-ish host language tag to the display locale (`de-AT` → de).
pub fn locale_from_tag(tag: &str) -> Locale {
    let lang = tag
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match lang.as_str() {
        "de" => Locale::DeDe,
        "fr" => Locale::FrFr,
        "es" => Locale::EsEs,
        "it" => Locale::ItIt,
        _ => Locale::EnUs,
    }
}

/// Split delimited text into records (RFC 4180: `"…"` fields, `""` escapes a
/// quote, newlines inside quotes are data; CRLF or LF row ends).
pub fn parse_records(text: &str, delim: char) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            c if c == delim => row.push(std::mem::take(&mut field)),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    if any && (!field.is_empty() || !row.is_empty()) {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Pick the delimiter: the explicit one, else whichever of tab / `;` / `,`
/// occurs most (outside quotes) in the first line — so a German Excel CSV
/// (`;`, because `,` is its decimal) imports without being told.
pub fn sniff_delimiter(text: &str) -> char {
    let first = text.lines().next().unwrap_or("");
    let mut counts = [('\t', 0usize), (';', 0), (',', 0)];
    let mut quoted = false;
    for c in first.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted {
            for (d, n) in counts.iter_mut() {
                if c == *d {
                    *n += 1;
                }
            }
        }
    }
    counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .map(|(d, _)| *d)
        .unwrap_or(',')
}

/// One typed field.
#[derive(Debug, Clone, PartialEq)]
pub enum Typed {
    Empty,
    Number(f64),
    Date(f64),
    Percent(f64, bool),
    Bool(bool),
    Text(String),
}

/// A date in ISO order or the locale's short-date order → serial.
fn parse_date(s: &str, locale: Locale, sys: DateSystem) -> Option<f64> {
    let s = s.trim();
    let nums = |sep: char| -> Option<Vec<u32>> {
        let parts: Vec<&str> = s.split(sep).collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || p.len() > 4) {
            return None;
        }
        parts.iter().map(|p| p.parse::<u32>().ok()).collect()
    };
    if let Some(p) = nums('-') {
        if s.split('-').next()?.len() == 4 {
            return ymd_to_serial(p[0] as i32, p[1], p[2], sys);
        }
    }
    let pattern = locale_data(locale).short_date;
    let sep = if pattern.contains('.') { '.' } else { '/' };
    let p = nums(sep)?;
    let year = |y: u32| -> i32 {
        if y < 100 {
            // Excel's two-digit-year window: 00–29 → 20xx, 30–99 → 19xx.
            if y < 30 {
                2000 + y as i32
            } else {
                1900 + y as i32
            }
        } else {
            y as i32
        }
    };
    if pattern.starts_with('m') {
        ymd_to_serial(year(p[2]), p[0], p[1], sys)
    } else {
        ymd_to_serial(year(p[2]), p[1], p[0], sys)
    }
}

/// Type one field for `locale`.
pub fn type_field(s: &str, locale: Locale, sys: DateSystem) -> Typed {
    let t = s.trim();
    if t.is_empty() {
        return Typed::Empty;
    }
    if t.starts_with('=') {
        return Typed::Text(s.to_string());
    }
    if t.eq_ignore_ascii_case("TRUE") {
        return Typed::Bool(true);
    }
    if t.eq_ignore_ascii_case("FALSE") {
        return Typed::Bool(false);
    }
    if let Some(body) = t.strip_suffix('%') {
        if let Some(n) = parse_number_locale(body, locale) {
            return Typed::Percent(n / 100.0, n.fract() != 0.0);
        }
    }
    // Dates before numbers: `04.10.2026` is a valid de group-separated
    // number shape too, and the date reading is the one a person meant.
    if let Some(serial) = parse_date(t, locale, sys) {
        return Typed::Date(serial);
    }
    let numeric_shape = t.chars().all(|c| {
        c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | 'e' | 'E' | ' ' | '\u{a0}')
    });
    if numeric_shape && t.chars().any(|c| c.is_ascii_digit()) {
        if let Some(n) = parse_number_locale(t, locale) {
            return Typed::Number(n);
        }
    }
    Typed::Text(s.to_string())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Column letters for a 0-based index.
fn col_name(mut c: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).expect("ascii")
}

/// Build the XLSX package for delimited `text`.
pub fn csv_to_xlsx(text: &str, delim: Option<char>, locale: Locale, sheet_name: &str) -> Vec<u8> {
    let delim = delim.unwrap_or_else(|| sniff_delimiter(text));
    let sys = DateSystem::Date1900;
    let records = parse_records(text, delim);

    let mut sheet = String::new();
    for (r, rec) in records.iter().enumerate() {
        let mut cells = String::new();
        for (c, field) in rec.iter().enumerate() {
            let a1 = format!("{}{}", col_name(c), r + 1);
            match type_field(field, locale, sys) {
                Typed::Empty => {}
                Typed::Number(n) => cells.push_str(&format!(r#"<c r="{a1}"><v>{n}</v></c>"#)),
                Typed::Date(n) => {
                    cells.push_str(&format!(r#"<c r="{a1}" s="{XF_DATE}"><v>{n}</v></c>"#))
                }
                Typed::Percent(n, decimals) => {
                    let xf = if decimals { XF_PERCENT_2 } else { XF_PERCENT };
                    cells.push_str(&format!(r#"<c r="{a1}" s="{xf}"><v>{n}</v></c>"#));
                }
                Typed::Bool(b) => {
                    cells.push_str(&format!(r#"<c r="{a1}" t="b"><v>{}</v></c>"#, u8::from(b)))
                }
                Typed::Text(t) => cells.push_str(&format!(
                    r#"<c r="{a1}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                    esc(&t)
                )),
            }
        }
        if !cells.is_empty() {
            sheet.push_str(&format!(r#"<row r="{}">{cells}</row>"#, r + 1));
        }
    }

    // The locale's short-date pattern, its `.` separators escaped as literals,
    // tagged with the locale's LCID so the workbook's display locale follows
    // the import (sheet-xlsx reads `[$-LCID]`, ruling
    // `sheet.format.locale.locale-from-workbook`).
    let lcid = match locale {
        Locale::EnUs => "",
        Locale::DeDe => "[$-407]",
        Locale::FrFr => "[$-40C]",
        Locale::EsEs => "[$-C0A]",
        Locale::ItIt => "[$-410]",
    };
    let date_code = esc(&format!(
        "{lcid}{}",
        locale_data(locale).short_date.replace('.', "\\.")
    ));
    let files: [(&str, String); 7] = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
        ("xl/workbook.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="{}" sheetId="1" r:id="rId1"/></sheets></workbook>"#, esc(sheet_name))),
        ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_string()),
        ("xl/styles.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="{date_code}"/></numFmts><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="4"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="9" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="10" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#)),
        ("xl/worksheets/sheet1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{sheet}</sheetData></worksheet>"#)),
        ("docProps/app.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>paged.sheet</Application></Properties>"#.to_string()),
    ];
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in files.iter() {
            zip.start_file(*name, opts).expect("in-memory zip");
            zip.write_all(body.as_bytes()).expect("in-memory zip");
        }
        zip.finish().expect("in-memory zip");
    }
    buf
}

impl SheetSession {
    /// Import delimited text as a fresh one-sheet workbook. `delimiter` is
    /// `None` to sniff (tab / `;` / `,`); `locale_tag` is the host language
    /// (`"de-DE"`) the numbers and dates are read in; `sheet_name` names the
    /// sheet (the file's base name; empty → `Sheet1`).
    pub fn load_csv(
        text: &str,
        delimiter: Option<char>,
        locale_tag: &str,
        sheet_name: &str,
        now_serial: f64,
    ) -> Result<SheetSession, SessionError> {
        let locale = locale_from_tag(locale_tag);
        let name: String = sheet_name
            .chars()
            .filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
            .take(31)
            .collect();
        let name = name.trim_matches('\'');
        let name = if name.is_empty() { "Sheet1" } else { name };
        let bytes = csv_to_xlsx(text, delimiter, locale, name);
        let mut s = SheetSession::load_xlsx_at(&bytes, now_serial)?;
        // The typed values are the user's import: the workbook is unsaved.
        s.structure_changed = true;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_quotes_newlines_and_bom() {
        let r = parse_records("\u{feff}a,\"b,\"\"c\"\"\",d\r\n1,\"x\ny\",\n", ',');
        assert_eq!(r, vec![vec!["a", "b,\"c\"", "d"], vec!["1", "x\ny", ""]]);
    }

    #[test]
    fn sniffs_semicolon_and_tab() {
        assert_eq!(sniff_delimiter("a;b;\"c,d\"\n1;2;3"), ';');
        assert_eq!(sniff_delimiter("a\tb\n"), '\t');
        assert_eq!(sniff_delimiter("plain"), ',');
    }

    #[test]
    fn types_by_locale() {
        let de = Locale::DeDe;
        let sys = DateSystem::Date1900;
        assert_eq!(type_field("1.234,5", de, sys), Typed::Number(1234.5));
        assert_eq!(
            type_field("1,234.5", Locale::EnUs, sys),
            Typed::Number(1234.5)
        );
        assert_eq!(type_field("04.10.2026", de, sys), Typed::Date(46299.0));
        assert_eq!(
            type_field("10/4/2026", Locale::EnUs, sys),
            Typed::Date(46299.0)
        );
        assert_eq!(type_field("2026-10-04", de, sys), Typed::Date(46299.0));
        assert_eq!(type_field("12,5%", de, sys), Typed::Percent(0.125, true));
        assert_eq!(type_field("=1+1", de, sys), Typed::Text("=1+1".into()));
        assert_eq!(type_field("true", de, sys), Typed::Bool(true));
        assert_eq!(
            type_field("Q1 2026", de, sys),
            Typed::Text("Q1 2026".into())
        );
        assert_eq!(col_name(27), "AB");
    }
}
