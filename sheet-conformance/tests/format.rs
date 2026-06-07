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

//! Number-format conformance (spec §9; registry `sheet.format.*`). Drives the
//! `corpus/format-corpus/*.golden.tsv` goldens through [`sheet_format`] and
//! adds targeted unit asserts. Self-contained: a tiny in-file TSV loader (no
//! `sheet-conformance` lib import) so the format corpus — whose 4th `expected`
//! column is sometimes empty (hidden values) — parses without the generic
//! 4-column-strict loader. The same TSVs are re-consumed by the Phase-2
//! LibreOffice oracle.

use sheet_core::{CellValue, DateSystem};
use sheet_format::{compile, format_value, FormatCtx};
use std::path::PathBuf;

/// One golden row: `id<TAB>format_code<TAB>value<TAB>expected`. The 4th
/// column may be absent/empty (a hidden value renders to "").
struct Row {
    id: String,
    code: String,
    value: String,
    expected: String,
}

/// Load a format-corpus TSV by repo-relative path. Skips `#` comments and
/// blank lines. Accepts 3 or 4 columns (a 3-column row means `expected` is
/// the empty string — a value the format hides).
fn load(repo_relative: &str) -> Vec<Row> {
    let path: PathBuf = repo_root().join(repo_relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("format corpus: cannot read {}: {e}", path.display()));
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
                "format corpus: {}:{} has {} columns, expected 3 or 4 \
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
        "format corpus {repo_relative} must carry >= 10 rows (has {})",
        rows.len()
    );
    rows
}

/// Repo root: `CARGO_MANIFEST_DIR/..` (the crate sits one level under the
/// workspace root, per the §4 top-level-crates layout).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has a parent (the repo root)")
        .to_path_buf()
}

/// Parse a corpus `value` column into a [`CellValue`]: `text:...` ⇒ text,
/// `bool:true`/`bool:false` ⇒ bool, otherwise a bare f64.
fn parse_value(v: &str) -> CellValue {
    if let Some(t) = v.strip_prefix("text:") {
        CellValue::from(t)
    } else if let Some(b) = v.strip_prefix("bool:") {
        CellValue::Bool(b == "true")
    } else {
        CellValue::Number(v.parse().unwrap_or_else(|_| panic!("bad value {v:?}")))
    }
}

/// Run every row of a corpus through the formatter and assert byte-equality.
fn run_corpus(repo_relative: &str) {
    let ctx = FormatCtx {
        date_system: DateSystem::Date1900,
    };
    for row in load(repo_relative) {
        let fmt = compile(&row.code)
            .unwrap_or_else(|e| panic!("[{}] compile {:?} failed: {e}", row.id, row.code));
        let got = format_value(&parse_value(&row.value), &fmt, &ctx);
        assert_eq!(
            got, row.expected,
            "[{}] format_value({:?}, {:?}) = {:?}, want {:?}",
            row.id, row.value, row.code, got, row.expected
        );
    }
}

// ---- Registry-pointer test fns (one per `sheet.format.*` row). ----

#[test]
fn sheet_format_general() {
    run_corpus("corpus/format-corpus/general.golden.tsv");

    // Targeted asserts: the 15-sig cap and the scientific switch boundary.
    let ctx = FormatCtx::default();
    let f = compile("General").unwrap();
    let g = |x: f64| format_value(&CellValue::Number(x), &f, &ctx);
    assert_eq!(g(1e21), "1E+21"); // e == 21 -> scientific
    assert_eq!(g(1e20), "100000000000000000000"); // e == 20 -> plain
    assert_eq!(g(1e-7), "0.0000001"); // e == -7 -> plain
    assert_eq!(g(1e-8), "1E-08"); // e == -8 -> scientific
                                  // Bools / errors via General.
    assert_eq!(format_value(&CellValue::Bool(true), &f, &ctx), "TRUE");
    assert_eq!(
        format_value(&CellValue::Error(sheet_core::CellError::Div0), &f, &ctx),
        "#DIV/0!"
    );
}

#[test]
fn sheet_format_number_placeholders() {
    run_corpus("corpus/format-corpus/number.golden.tsv");

    let ctx = FormatCtx::default();
    let fmt = |c: &str, x: f64| {
        let f = compile(c).unwrap();
        format_value(&CellValue::Number(x), &f, &ctx)
    };
    // Half-away-from-zero (Excel display rounding), not bankers'.
    assert_eq!(fmt("0", 0.5), "1");
    assert_eq!(fmt("0", 1.5), "2");
    assert_eq!(fmt("0", 2.5), "3");
    // ? space-padding leaves a blank slot.
    assert_eq!(fmt("???", 5.0), "  5");
}

#[test]
fn sheet_format_percent() {
    run_corpus("corpus/format-corpus/percent.golden.tsv");

    let ctx = FormatCtx::default();
    let f = compile("0%").unwrap();
    assert_eq!(format_value(&CellValue::Number(0.5), &f, &ctx), "50%");
}

#[test]
fn sheet_format_scientific() {
    run_corpus("corpus/format-corpus/scientific.golden.tsv");

    let ctx = FormatCtx::default();
    let f = compile("0.00E+00").unwrap();
    let s = |x: f64| format_value(&CellValue::Number(x), &f, &ctx);
    assert_eq!(s(12345.0), "1.23E+04");
    assert_eq!(s(0.0001234), "1.23E-04");
}

#[test]
fn sheet_format_sections() {
    run_corpus("corpus/format-corpus/sections.golden.tsv");

    let ctx = FormatCtx::default();
    // 2-section: negatives use section 2 WITHOUT an auto minus (implicit).
    let f = compile("0.00;(0.00)").unwrap();
    assert_eq!(format_value(&CellValue::Number(-5.0), &f, &ctx), "(5.00)");
    // 1-section: negatives get the auto minus.
    let f1 = compile("0.00").unwrap();
    assert_eq!(format_value(&CellValue::Number(-5.0), &f1, &ctx), "-5.00");
}

#[test]
fn sheet_format_datetime_tokens() {
    run_corpus("corpus/format-corpus/datetime.golden.tsv");

    let ctx = FormatCtx {
        date_system: DateSystem::Date1900,
    };
    let f = compile("h:mm AM/PM").unwrap();
    // 12-hour clock under AM/PM.
    assert_eq!(format_value(&CellValue::Number(0.5), &f, &ctx), "12:00 PM");
    // m/mm minutes-vs-month adjacency: `hh:mm` is minutes; `mm/dd` is month.
    let fm = compile("hh:mm").unwrap();
    assert_eq!(format_value(&CellValue::Number(0.75), &fm, &ctx), "18:00");
}

#[test]
fn sheet_format_text_section() {
    run_corpus("corpus/format-corpus/text.golden.tsv");

    let ctx = FormatCtx::default();
    // 4th section @ mask.
    let f = compile("0;0;0;\"<\"@\">\"").unwrap();
    assert_eq!(format_value(&CellValue::from("hi"), &f, &ctx), "<hi>");
    // Single-section text code applies to text values.
    let f2 = compile("\"Note: \"@").unwrap();
    assert_eq!(format_value(&CellValue::from("x"), &f2, &ctx), "Note: x");
}
