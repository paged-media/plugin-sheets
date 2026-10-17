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
