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

// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
#![allow(non_snake_case)]

//! The bytes of a saved package, pinned whole.
//!
//! The preservation invariant is about the decompressed bytes of each part
//! (ADR 503); `xlsx_roundtrip.rs` checks that. This file pins the zip
//! container around the parts: entry order, local and central headers,
//! timestamps, the "made by" field, external attributes and the deflate
//! streams. A save must not change when the zip library, its defaults or its
//! deflate backend do.
//!
//! The goldens in `tests/golden/zip-container/` are the output of the zip
//! 2.4.2 build (the last one before the upgrade to zip 9). Re-record one only
//! for a change that is meant to alter the bytes, and say so in the commit.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn corpus(rel: &str) -> Vec<u8> {
    let p = root().join("corpus").join(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Compare `actual` with the golden `name`; on a mismatch, write the actual
/// bytes next to the test binary and name the first differing offset.
fn assert_golden(name: &str, actual: &[u8]) {
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/zip-container")
        .join(name);
    let golden =
        std::fs::read(&golden_path).unwrap_or_else(|e| panic!("{}: {e}", golden_path.display()));
    if golden == actual {
        return;
    }
    let first = golden
        .iter()
        .zip(actual)
        .position(|(a, b)| a != b)
        .unwrap_or(golden.len().min(actual.len()));
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::write(&out, actual);
    panic!(
        "{name}: the saved package differs from the golden at byte {first} \
         (golden {} bytes, actual {} bytes); actual written to {}",
        golden.len(),
        actual.len(),
        out.display()
    );
}

fn session(bytes: &[u8]) -> sheet_js::core::SheetSession {
    sheet_js::core::SheetSession::load_xlsx(bytes).expect("load")
}

#[test]
fn a_new_workbook_saves_to_the_pinned_bytes__feat__sheet_xlsx_roundtrip() {
    // The embedded empty workbook (`SheetSession::new`), written and saved.
    let saved = sheet_js::core::SheetSession::new()
        .save_xlsx()
        .expect("save");
    assert_golden("new-session.xlsx", &saved);
}

#[test]
fn a_csv_import_saves_to_the_pinned_bytes__feat__sheet_xlsx_roundtrip() {
    let mut s = sheet_js::core::SheetSession::load_csv(
        "a,b,c\n1,2.5,2026-01-02\nx,=1+1,40%\n",
        None,
        "en-US",
        "Data",
        46000.0,
    )
    .expect("csv");
    assert_golden("csv-import.xlsx", &s.save_xlsx().expect("save"));
}

#[test]
fn a_zero_edit_save_keeps_the_pinned_bytes__feat__sheet_xlsx_roundtrip() {
    // Unknown parts (customXml, a VBA project) ride along; the calc chain
    // is dropped.
    let doc =
        sheet_xlsx::XlsxDocument::open(&corpus("xlsx-corpus/04-unknown-parts.xlsx")).expect("open");
    assert_golden(
        "04-unknown-parts.zero-edit.xlsx",
        &doc.save().expect("save"),
    );

    // An Excel-saved workbook with dynamic arrays: `cm` cell metadata,
    // `xl/metadata.xml` and the rich-data parts.
    let mut s = session(&corpus("xlsx-dynamic/dynamic-arrays.excel.xlsx"));
    assert_golden(
        "dynamic-arrays.excel.zero-edit.xlsx",
        &s.save_xlsx().expect("save"),
    );
}

#[test]
fn an_edited_sheet_saves_to_the_pinned_bytes__feat__sheet_xlsx_roundtrip() {
    // A dirty worksheet is encoded again from the model; the other parts
    // keep their stored bytes.
    let mut s = session(&corpus("xlsx-corpus/02-formulas.xlsx"));
    s.set_cell(0, 300, 5, "=1+2").expect("formula");
    s.set_cell(0, 301, 5, "hello").expect("text");
    assert_golden("02-formulas.edited.xlsx", &s.save_xlsx().expect("save"));
}

/// Every workbook in the three XLSX corpora.
fn corpus_workbooks() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for dir in ["xlsx-corpus", "xlsx-dynamic", "xlsx-recalc"] {
        let d = root().join("corpus").join(dir);
        let mut names: Vec<String> = std::fs::read_dir(&d)
            .unwrap_or_else(|e| panic!("{}: {e}", d.display()))
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".xlsx") || n.ends_with(".xlsm"))
            .collect();
        names.sort();
        for n in names {
            let bytes = corpus(&format!("{dir}/{n}"));
            out.push((format!("{dir}/{n}"), bytes));
        }
    }
    out
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Walk a saved package's central directory (read straight from the bytes,
/// not through the zip crate) and check every entry and its local header
/// against the fields the zip 2.4.2 build wrote for all 1409 entries of the
/// corpus saves. None of them may come from the source package: its
/// attributes, extra fields, data descriptors or timestamps.
fn assert_pinned_headers(label: &str, zip: &[u8]) {
    const EOCD: u32 = 0x0605_4b50;
    const CENTRAL: u32 = 0x0201_4b50;
    const LOCAL: u32 = 0x0403_4b50;
    // No archive comment, so the end record is the last 22 bytes.
    let eocd = zip.len() - 22;
    assert_eq!(u32_at(zip, eocd), EOCD, "{label}: end record");
    assert_eq!(u16_at(zip, eocd + 20), 0, "{label}: archive comment");
    let entries = u16_at(zip, eocd + 10) as usize;
    let mut at = u32_at(zip, eocd + 16) as usize;
    assert!(entries > 0, "{label}: no entries");
    for _ in 0..entries {
        assert_eq!(u32_at(zip, at), CENTRAL, "{label}: central header at {at}");
        let name_len = u16_at(zip, at + 28) as usize;
        let name = String::from_utf8_lossy(&zip[at + 46..at + 46 + name_len]).into_owned();
        let what = format!("{label}: {name}");
        // Version made by: Unix (3) in the high byte, 2.0 in the low byte.
        assert_eq!(u16_at(zip, at + 4), 0x0314, "{what}: version made by");
        assert_eq!(u16_at(zip, at + 6), 20, "{what}: version needed");
        // No data descriptor, no encryption (bit 11, UTF-8, only for a
        // non-ASCII name; the corpus has none).
        assert_eq!(u16_at(zip, at + 8) & !0x0800, 0, "{what}: flags");
        assert_eq!(u16_at(zip, at + 10), 8, "{what}: method (deflate)");
        assert_eq!(u16_at(zip, at + 12), 0, "{what}: DOS time");
        assert_eq!(u16_at(zip, at + 14), 0x0021, "{what}: DOS date 1980-01-01");
        assert_eq!(u16_at(zip, at + 30), 0, "{what}: central extra field");
        assert_eq!(u16_at(zip, at + 32), 0, "{what}: entry comment");
        assert_eq!(u16_at(zip, at + 36), 0, "{what}: internal attributes");
        assert_eq!(
            u32_at(zip, at + 38),
            0o100644 << 16,
            "{what}: external attributes (regular file, rw-r--r--)"
        );
        let local = u32_at(zip, at + 42) as usize;
        assert_eq!(u32_at(zip, local), LOCAL, "{what}: local header");
        assert_eq!(u16_at(zip, local + 4), 20, "{what}: local version needed");
        assert_eq!(
            u16_at(zip, local + 6),
            u16_at(zip, at + 8),
            "{what}: local flags"
        );
        assert_eq!(u16_at(zip, local + 10), 0, "{what}: local DOS time");
        assert_eq!(u16_at(zip, local + 12), 0x0021, "{what}: local DOS date");
        assert_eq!(u16_at(zip, local + 28), 0, "{what}: local extra field");
        at += 46 + name_len;
    }
    assert_eq!(
        at, eocd,
        "{label}: central directory ends at the end record"
    );
}

#[test]
fn every_saved_entry_has_the_pinned_headers__feat__sheet_xlsx_roundtrip() {
    // Untouched parts are written again from their stored bytes, never
    // raw-copied, so nothing of the source's zip metadata can leak into a
    // save. Every save path, over every corpus workbook.
    let workbooks = corpus_workbooks();
    assert!(workbooks.len() >= 47, "corpus shrank: {}", workbooks.len());
    for (name, bytes) in &workbooks {
        let doc = sheet_xlsx::XlsxDocument::open(bytes).expect("open");
        assert_pinned_headers(&format!("{name} (zero-edit)"), &doc.save().expect("save"));

        let mut s = session(bytes);
        assert_pinned_headers(
            &format!("{name} (session, zero-edit)"),
            &s.save_xlsx().expect("save"),
        );
        s.set_cell(0, 300, 5, "=1+2").expect("formula");
        assert_pinned_headers(&format!("{name} (edited)"), &s.save_xlsx().expect("save"));
    }
    let mut s = sheet_js::core::SheetSession::new();
    assert_pinned_headers("new workbook", &s.save_xlsx().expect("save"));
}
