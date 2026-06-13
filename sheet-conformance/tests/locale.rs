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

//! Localization conformance — the M3 LOCALIZATION track (spec §9; decision
//! D-8 "minimum en/de"; registry `registry/features/locale.yaml`). Drives the
//! `corpus/format-corpus/locale-de.golden.tsv` golden through [`sheet_format`]
//! under [`Locale::DeDe`] AND pins each locale ruling with targeted asserts.
//! One `#[test]` per `locale.yaml` row (`sheet_format_locale_*`), the coverage
//! gate's pointers.
//!
//! ## The byte-identity regression guard
//!
//! Every assert here ALSO re-runs the matching en-US case to prove en output is
//! unchanged (the de path is purely additive). The en goldens live in
//! `number/datetime.golden.tsv` and stay byte-identical — this file never edits
//! them; it mirrors them under de.
//!
//! ## Rulings documented + tested here (the localization ledger)
//!
//! - **separators** (`sheet.format.locale.separators`): de-DE renders `,`
//!   decimal + `.` group; en-US keeps `.`/`,`. The format CODE is
//!   locale-NEUTRAL — only the rendered glyph swaps.
//! - **month-day-names** (`sheet.format.locale.month-day-names`): de month/day
//!   names (`Januar`/`Freitag`, `Jan`/`Fr`) for `mmmm`/`mmm`/`dddd`/`ddd`.
//! - **ampm** (`sheet.format.locale.ampm`): de honors an explicit AM/PM token as
//!   the literal en `"AM"`/`"PM"` strings (Excel de does NOT substitute
//!   `vorm.`/`nachm.`); the 24h preference lives in the AUTHORING (drop the
//!   token), not the engine.
//! - **locale-from-workbook** (`sheet.format.locale.locale-from-workbook`): a
//!   `[$…-LCID]` token on a cell numFmt carries a per-code locale that overrides
//!   the document locale; a custom numFmt's `[$-407]` token derives the workbook
//!   locale into `CalcSettings.locale`. OOXML has no document-locale element, so
//!   absent any hint the locale defaults en-US (set via the model/host, NOT
//!   auto-detected).
//! - **de-de-rendering** (`sheet.format.locale.de-de-rendering`): the end-to-end
//!   number + date render through the de locale (corpus + the `FormatCtx` path).
//! - **number-parse-locale** (`sheet.format.locale.number-parse-locale`): the
//!   VALUE-string parse localizes (NUMBERVALUE's explicit separators, the
//!   `parse_number_locale` helper) but the formula DIALECT stays en — function
//!   args are `,`-separated, the implicit `coerce::to_number` stays en.

use sheet_core::{CellValue, DateSystem, Locale};
use sheet_fn::arg::Arg;
use sheet_fn::{coerce, dispatch, EvalCtx};
use sheet_format::{
    compile, format_value, locale_from_lcid, parse_number_locale, parse_number_seps, FormatCtx,
};
use std::path::PathBuf;

/// One golden row: `id<TAB>format_code<TAB>value<TAB>expected`. The 4th column
/// may be absent/empty (a hidden value renders to "").
struct Row {
    id: String,
    code: String,
    value: String,
    expected: String,
}

/// Load a format-corpus TSV by repo-relative path. Skips `#` comments and blank
/// lines. Accepts 3 or 4 columns (mirrors `format.rs`/`format2.rs`).
fn load(repo_relative: &str) -> Vec<Row> {
    let path: PathBuf = repo_root().join(repo_relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("locale corpus: cannot read {}: {e}", path.display()));
    let mut rows = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 || cols.len() > 4 {
            panic!(
                "locale corpus: {}:{} has {} columns, expected 3 or 4 \
                 (id<TAB>code<TAB>value[<TAB>expected])",
                path.display(),
                lineno + 1,
                cols.len()
            );
        }
        rows.push(Row {
            id: cols[0].to_string(),
            code: cols[1].to_string(),
            value: cols[2].to_string(),
            expected: cols.get(3).copied().unwrap_or("").to_string(),
        });
    }
    assert!(
        rows.len() >= 10,
        "locale corpus {repo_relative} must carry >= 10 rows (has {})",
        rows.len()
    );
    rows
}

/// Repo root: `CARGO_MANIFEST_DIR/..`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has a parent (the repo root)")
        .to_path_buf()
}

/// Parse a corpus `value` column into a [`CellValue`].
fn parse_value(v: &str) -> CellValue {
    if let Some(t) = v.strip_prefix("text:") {
        CellValue::from(t)
    } else if let Some(b) = v.strip_prefix("bool:") {
        CellValue::Bool(b == "true")
    } else {
        CellValue::Number(v.parse().unwrap_or_else(|_| panic!("bad value {v:?}")))
    }
}

/// Run the de golden corpus through the formatter under [`Locale::DeDe`].
fn run_de_corpus() {
    let ctx = FormatCtx::new(DateSystem::Date1900, Locale::DeDe);
    for row in load("corpus/format-corpus/locale-de.golden.tsv") {
        let fmt = compile(&row.code)
            .unwrap_or_else(|e| panic!("[{}] compile {:?} failed: {e}", row.id, row.code));
        let got = format_value(&parse_value(&row.value), &fmt, &ctx);
        assert_eq!(
            got, row.expected,
            "[{}] format_value({:?}, {:?}) under de-DE = {:?}, want {:?}",
            row.id, row.value, row.code, got, row.expected
        );
    }
}

/// Format a numeric code under a locale (convenience).
fn fmt(code: &str, x: f64, locale: Locale) -> String {
    let f = compile(code).unwrap();
    format_value(
        &CellValue::Number(x),
        &f,
        &FormatCtx::new(DateSystem::Date1900, locale),
    )
}

// ---- Registry-pointer test fns (one per `locale.yaml` row). ----

/// `sheet.format.locale.separators` — de `,`/`.` ; en `.`/`,` byte-identical.
#[test]
fn sheet_format_locale_separators() {
    run_de_corpus();

    // de-DE swaps decimal/group.
    assert_eq!(fmt("#,##0.00", 1234.5, Locale::DeDe), "1.234,50");
    assert_eq!(fmt("0.00", 1.5, Locale::DeDe), "1,50");
    assert_eq!(fmt("#,##0", 1234567.0, Locale::DeDe), "1.234.567");
    // en-US REGRESSION GUARD: the same codes render byte-identical to M0.
    assert_eq!(fmt("#,##0.00", 1234.5, Locale::EnUs), "1,234.50");
    assert_eq!(fmt("0.00", 1.5, Locale::EnUs), "1.50");
    assert_eq!(fmt("#,##0", 1234567.0, Locale::EnUs), "1,234,567");
}

/// `sheet.format.locale.month-day-names` — de calendar names; en unchanged.
#[test]
fn sheet_format_locale_month_day_names() {
    // serial 44197 = 2021-01-01 (Freitag); 44228 = 2021-02-01.
    assert_eq!(fmt("mmmm", 44197.0, Locale::DeDe), "Januar");
    assert_eq!(fmt("mmmm", 44228.0, Locale::DeDe), "Februar");
    assert_eq!(fmt("mmm", 44197.0, Locale::DeDe), "Jan");
    assert_eq!(fmt("dddd", 44197.0, Locale::DeDe), "Freitag");
    assert_eq!(fmt("ddd", 44197.0, Locale::DeDe), "Fr");
    // en-US REGRESSION GUARD.
    assert_eq!(fmt("mmmm", 44197.0, Locale::EnUs), "January");
    assert_eq!(fmt("mmm", 44197.0, Locale::EnUs), "Jan");
    assert_eq!(fmt("dddd", 44197.0, Locale::EnUs), "Friday");
    assert_eq!(fmt("ddd", 44197.0, Locale::EnUs), "Fri");
}

/// `sheet.format.locale.ampm` — de honors the AM/PM token as literal en strings.
#[test]
fn sheet_format_locale_ampm() {
    // RULING: an explicit AM/PM token renders the SAME "AM"/"PM" markers in
    // de-DE as en-US (Excel de does not localize the token to vorm./nachm.).
    assert_eq!(fmt("h:mm AM/PM", 0.5, Locale::DeDe), "12:00 PM");
    assert_eq!(fmt("h:mm AM/PM", 0.25, Locale::DeDe), "6:00 AM");
    assert_eq!(fmt("h:mm AM/PM", 0.5, Locale::EnUs), "12:00 PM");
    assert_eq!(fmt("h:mm AM/PM", 0.25, Locale::EnUs), "6:00 AM");
    // The de 24h convention is honored by DROPPING the token (HH:mm), which
    // renders identically in both locales.
    assert_eq!(fmt("HH:mm", 0.75, Locale::DeDe), "18:00");
    assert_eq!(fmt("HH:mm", 0.75, Locale::EnUs), "18:00");
    // The short A/P form is the en literal too.
    assert_eq!(fmt("h:mm A/P", 0.25, Locale::DeDe), "6:00 A");
}

/// `sheet.format.locale.locale-from-workbook` — the `[$…-LCID]` token + the
/// derived workbook locale.
#[test]
fn sheet_format_locale_from_workbook() {
    // The LCID primary-language bits pick the locale (sublanguage masked off).
    assert_eq!(locale_from_lcid(0x0407), Locale::DeDe);
    assert_eq!(locale_from_lcid(0x0409), Locale::EnUs);

    // A `[$-407]` token on the CODE overrides the ctx locale: under an EN ctx,
    // the code still renders de.
    let f = compile("[$-407]#,##0.00").unwrap();
    let en_ctx = FormatCtx::new(DateSystem::Date1900, Locale::EnUs);
    assert_eq!(
        format_value(&CellValue::Number(1234.5), &f, &en_ctx),
        "1.234,50"
    );
    // A `[$€-407]` token keeps the € symbol AND localizes the separators.
    let fc = compile("[$€-407]#,##0.00").unwrap();
    assert_eq!(
        format_value(&CellValue::Number(1234.5), &fc, &en_ctx),
        "€1.234,50"
    );
    // A code with NO locale token follows the ctx locale (en here → unchanged).
    let plain = compile("#,##0.00").unwrap();
    assert_eq!(
        format_value(&CellValue::Number(1234.5), &plain, &en_ctx),
        "1,234.50"
    );

    // End-to-end: a workbook whose custom numFmt carries `[$-407]` derives the
    // de-DE document locale into CalcSettings.locale.
    let de_wb = sheet_xlsx::XlsxDocument::open(&xlsx_with_numfmt("[$-407]#,##0.00")).unwrap();
    assert_eq!(de_wb.model.calc.locale, Locale::DeDe);
    // HONEST FALLBACK: a workbook with no `[$…-LCID]` hint stays en-US (the
    // locale is set via the model/host, not auto-detected).
    let en_wb = sheet_xlsx::XlsxDocument::open(&xlsx_with_numfmt("#,##0.00")).unwrap();
    assert_eq!(en_wb.model.calc.locale, Locale::EnUs);
}

/// `sheet.format.locale.de-de-rendering` — end-to-end number + date through de.
#[test]
fn sheet_format_locale_de_de_rendering() {
    run_de_corpus();

    let de = FormatCtx::new(DateSystem::Date1900, Locale::DeDe);
    // A number and a date render through ONE de FormatCtx end-to-end.
    let num = compile("#,##0.00").unwrap();
    assert_eq!(
        format_value(&CellValue::Number(-1234.5), &num, &de),
        "-1.234,50"
    );
    let date = compile("dddd\", \"d\". \"mmmm yyyy").unwrap();
    assert_eq!(
        format_value(&CellValue::Number(44197.0), &date, &de),
        "Freitag, 1. Januar 2021"
    );
    // The compiled format is locale-neutral — the SAME CompiledFormat renders
    // en or de purely by the FormatCtx (the cache key need not include locale).
    let en = FormatCtx::new(DateSystem::Date1900, Locale::EnUs);
    assert_eq!(
        format_value(&CellValue::Number(-1234.5), &num, &en),
        "-1,234.50"
    );
}

/// `sheet.format.locale.number-parse-locale` — VALUE parsing localizes; the
/// formula dialect stays en.
#[test]
fn sheet_format_locale_number_parse() {
    // The locale-aware VALUE parse (the inverse of render).
    assert_eq!(parse_number_locale("1.234,50", Locale::DeDe), Some(1234.5));
    assert_eq!(parse_number_locale("1,234.50", Locale::EnUs), Some(1234.5));
    assert_eq!(parse_number_seps("1.234,50", ",", "."), Some(1234.5));

    // NUMBERVALUE is locale-EXPLICIT: its decimal/group args parse a de string.
    let ctx = EvalCtx::new(DateSystem::Date1900, cell(), 45000.5, 0);
    let nv = |args: &[Arg]| {
        let id = sheet_core::funcs::lookup_func("NUMBERVALUE").expect("NUMBERVALUE registered");
        dispatch(id, args, &ctx)
    };
    assert_eq!(
        nv(&[txt("1.234,50"), txt(","), txt(".")]),
        CellValue::Number(1234.5)
    );
    // The en defaults stay "." decimal, "," group.
    assert_eq!(nv(&[txt("1,234.5")]), CellValue::Number(1234.5));

    // SCOPE RULING: the IMPLICIT coercion path (`coerce::to_number`) stays en —
    // a bare de-formatted "1.234,50" is NOT auto-localized (it parses as the en
    // "1.234" with a stray ",50", i.e. NOT 1234.5). Localization is opt-in via
    // an explicit locale, never the bare coercion.
    let bare = coerce::to_number(&CellValue::from("1.234,50"));
    assert_ne!(bare.ok(), Some(1234.5));
    // The plain en number string still coerces normally (dialect unchanged).
    assert_eq!(
        coerce::to_number(&CellValue::from("1234.5")).ok(),
        Some(1234.5)
    );
}

/// `sheet.format.locale.latin-tier` — fr-FR / es-ES / it-IT render through the
/// additive LocaleData rows; en-US AND de-DE stay byte-identical.
#[test]
fn sheet_format_locale_latin_tier() {
    // 1) Drive the 5-column (locale<TAB>id<TAB>code<TAB>value<TAB>expected)
    //    Latin-tier golden corpus through each locale's FormatCtx.
    let path: PathBuf = repo_root().join("corpus/format-corpus/locale-latin.golden.tsv");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("latin corpus: cannot read {}: {e}", path.display()));
    let mut rows = 0usize;
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert!(
            cols.len() == 4 || cols.len() == 5,
            "latin corpus {}:{} has {} columns, expected 4 or 5 \
             (locale<TAB>id<TAB>code<TAB>value[<TAB>expected])",
            path.display(),
            lineno + 1,
            cols.len()
        );
        let locale = match cols[0] {
            "fr" => Locale::FrFr,
            "es" => Locale::EsEs,
            "it" => Locale::ItIt,
            other => panic!("latin corpus {}:{}: unknown locale {other:?}", path.display(), lineno + 1),
        };
        let (id, code, value) = (cols[1], cols[2], cols[3]);
        let expected = cols.get(4).copied().unwrap_or("");
        let f = compile(code).unwrap_or_else(|e| panic!("[{id}] compile {code:?} failed: {e}"));
        let got = fmt_value(code_value(value), &f, locale);
        assert_eq!(
            got, expected,
            "[{id}] format_value({value:?}, {code:?}) under {:?} = {got:?}, want {expected:?}",
            locale
        );
        rows += 1;
    }
    assert!(rows >= 30, "latin corpus must carry >= 30 rows (has {rows})");

    // 2) Targeted separator asserts per locale.
    assert_eq!(fmt("#,##0.00", 1234.5, Locale::FrFr), "1 234,50"); // fr: space group
    assert_eq!(fmt("#,##0.00", 1234.5, Locale::EsEs), "1.234,50"); // es: "." group
    assert_eq!(fmt("#,##0.00", 1234.5, Locale::ItIt), "1.234,50"); // it: "." group

    // 3) Calendar-name asserts (serial 44197 = Fri 2021-01-01).
    assert_eq!(fmt("mmmm", 44197.0, Locale::FrFr), "janvier");
    assert_eq!(fmt("dddd", 44197.0, Locale::FrFr), "vendredi");
    assert_eq!(fmt("mmmm", 44197.0, Locale::EsEs), "enero");
    assert_eq!(fmt("dddd", 44197.0, Locale::EsEs), "viernes");
    assert_eq!(fmt("mmmm", 44197.0, Locale::ItIt), "gennaio");
    assert_eq!(fmt("dddd", 44197.0, Locale::ItIt), "venerdì");

    // 4) The shared AM/PM ruling: an explicit token renders the literal en
    //    markers in fr/es/it too (like de-DE), NOT a localized form.
    assert_eq!(fmt("h:mm AM/PM", 0.5, Locale::FrFr), "12:00 PM");
    assert_eq!(fmt("h:mm AM/PM", 0.25, Locale::EsEs), "6:00 AM");

    // 5) LCID mapping picks up the new locales (per-code [$-LCID] override).
    assert_eq!(locale_from_lcid(0x040c), Locale::FrFr); // fr-FR
    assert_eq!(locale_from_lcid(0x0c0c), Locale::FrFr); // fr-CA (sublang masked)
    assert_eq!(locale_from_lcid(0x040a), Locale::EsEs); // es-ES
    assert_eq!(locale_from_lcid(0x0410), Locale::ItIt); // it-IT
    let fr_code = compile("[$-40c]#,##0.00").unwrap();
    let en_ctx = FormatCtx::new(DateSystem::Date1900, Locale::EnUs);
    assert_eq!(
        format_value(&CellValue::Number(1234.5), &fr_code, &en_ctx),
        "1 234,50",
        "a [$-40c] token renders fr separators even under an en ctx"
    );

    // 6) REGRESSION GUARD: en-US AND de-DE output the SAME codes byte-identically
    //    — the Latin tier is purely additive, never a regression of en/de.
    assert_eq!(fmt("#,##0.00", 1234.5, Locale::EnUs), "1,234.50");
    assert_eq!(fmt("#,##0.00", 1234.5, Locale::DeDe), "1.234,50");
    assert_eq!(fmt("mmmm", 44197.0, Locale::EnUs), "January");
    assert_eq!(fmt("mmmm", 44197.0, Locale::DeDe), "Januar");
}

/// Parse a corpus `value` column (number / `text:` / `bool:`) into a CellValue.
fn code_value(v: &str) -> CellValue {
    parse_value(v)
}

/// Format a CellValue under a locale (the date/text-aware companion to `fmt`).
fn fmt_value(v: CellValue, f: &sheet_format::CompiledFormat, locale: Locale) -> String {
    format_value(&v, f, &FormatCtx::new(DateSystem::Date1900, locale))
}

// ---- helpers ----

fn cell() -> sheet_core::CellRef {
    sheet_core::CellRef {
        sheet: 0,
        row: 0,
        col: 0,
        row_abs: false,
        col_abs: false,
    }
}

fn txt(s: &str) -> Arg<'static> {
    Arg::Scalar(CellValue::from(s))
}

/// Build a minimal but structurally complete xlsx whose single custom numFmt
/// carries `format_code` — so the workbook-locale derivation has a numFmt to
/// scan. One sheet, one styles part, one cell.
fn xlsx_with_numfmt(format_code: &str) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: &str, body: String| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        add(
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#.to_string(),
        );
        add(
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        );
        add(
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
        );
        add(
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_string(),
        );
        // The styles part carries the custom numFmt whose code we vary. XML-
        // escape the code for the attribute (the format codes here contain no
        // `<`/`>`/`&`/`"`, but escape defensively).
        let escaped = format_code.replace('&', "&amp;").replace('"', "&quot;");
        add(
            "xl/styles.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="{escaped}"/></numFmts><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs></styleSheet>"#
            ),
        );
        add(
            "xl/worksheets/sheet1.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="0" t="n"><v>1234.5</v></c></row></sheetData></worksheet>"#.to_string(),
        );
        zip.finish().unwrap();
    }
    buf
}
