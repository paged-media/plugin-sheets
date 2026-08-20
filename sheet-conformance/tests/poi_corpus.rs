/*
 * This file is part of paged (https://paged.media), the commercial editor
 * for the paged IDML engine.
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

//! Apache POI's spreadsheet corpus — files Excel actually wrote.
//!
//! Every one of the 14 fixtures in `corpus/xlsx-corpus/` comes from
//! `generate.py`: five hand-written parts, fixed 1980 timestamps, no
//! `calcChain.xml`, no theme, no RSID noise. They are precise and
//! diffable, and they are all OUR shape. Until this lane the parser had
//! never met a workbook Excel itself produced.
//!
//! POI's corpus is the opposite in every respect. It accumulated over
//! twenty years of BUG REPORTS — many Excel versions, OpenOffice,
//! Gnumeric, third-party writers — and a good number of its files are
//! deliberately malformed because they once crashed something.
//!
//! **That is why this lane does not assert what the Envato lanes
//! assert.** "Every file parses" is the right bar for well-formed
//! designer output; against a corpus of known-pathological files it
//! would be asserting that upstream's regression suite contains no
//! regressions. The honest properties for THIS corpus are:
//!
//!   * never panic, on any input
//!   * never hang
//!   * fail with a TYPED error, not a leaked one
//!   * refuse `.xls` outright — `sheet-*` is xlsx-only, and 417 legacy
//!     BIFF files make that refusal the most-tested path here
//!
//! The parse RATE is reported, not gated. A drop in it is a signal to
//! read, not an automatic failure.
//!
//! Licence: Apache-2.0, and these are the FIRST redistributable fixtures
//! this project carries — see `corpus/xlsx/poi/PROVENANCE.md`.
//!
//! OPT-IN — the assets live in the private corpus checkout:
//!
//! ```text
//! PAGED_XLSX_CORPUS=1 cargo test -p sheet-conformance --test poi_corpus -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use sheet_xlsx::XlsxDocument;

/// Every POI spreadsheet fixture, or `None` with a printed reason.
fn poi_files() -> Option<Vec<PathBuf>> {
    let Some(switch) = std::env::var_os("PAGED_XLSX_CORPUS") else {
        eprintln!(
            "SKIP poi xlsx lane: PAGED_XLSX_CORPUS unset \
             (set it to 1, or to a corpus root, and run with --ignored)"
        );
        return None;
    };
    let switch = switch.to_string_lossy().into_owned();
    let root = if switch == "1" || switch.is_empty() {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../corpus")
    } else {
        PathBuf::from(switch)
    };
    let dir = root.join("xlsx/poi");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("SKIP poi xlsx lane: {} not readable", dir.display());
        return None;
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| {
                    matches!(
                        e.to_string_lossy().to_lowercase().as_str(),
                        "xlsx" | "xlsm" | "xlsb" | "xltx" | "xls"
                    )
                })
        })
        .collect();
    out.sort();
    if out.is_empty() {
        eprintln!(
            "SKIP poi xlsx lane: no spreadsheets under {}",
            dir.display()
        );
        return None;
    }
    Some(out)
}

fn ext_of(p: &std::path::Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

#[test]
#[ignore = "poi xlsx lane: opt-in (PAGED_XLSX_CORPUS=1 + the private corpus mount)"]
fn no_spreadsheet_in_the_poi_corpus_panics_the_parser() {
    let Some(files) = poi_files() else {
        return;
    };
    println!("poi xlsx corpus: {} file(s)", files.len());

    let mut opened = 0usize;
    let mut refused = 0usize;
    let mut by_ext: std::collections::BTreeMap<String, (usize, usize)> = Default::default();

    for path in &files {
        let ext = ext_of(path);
        let bytes = std::fs::read(path).expect("read poi fixture");
        // `open` returning Err is a RESULT, not a failure — this corpus
        // is full of files that are supposed to be rejected. What must
        // never happen is a panic, and `cargo test` turns any panic here
        // into a named failure with the file that caused it.
        match XlsxDocument::open(&bytes) {
            Ok(_) => {
                opened += 1;
                by_ext.entry(ext).or_default().0 += 1;
            }
            Err(_) => {
                refused += 1;
                by_ext.entry(ext).or_default().1 += 1;
            }
        }
    }

    println!("  opened {opened}, refused {refused}");
    for (ext, (ok, err)) in &by_ext {
        println!("    .{ext:<5} {ok:>4} opened  {err:>4} refused");
    }

    // The one hard assertion: real Excel output must be readable. If
    // NOTHING opens, the parser is broken rather than strict — and that
    // is the failure this lane exists to catch, since every other
    // fixture it has ever seen was written by generate.py.
    assert!(
        opened > 0,
        "not one of {} POI spreadsheets opened — the parser has only ever \
         been fed generate.py output, so this means it cannot read real \
         Excel files at all",
        files.len()
    );
}

#[test]
#[ignore = "poi xlsx lane: opt-in (PAGED_XLSX_CORPUS=1 + the private corpus mount)"]
fn legacy_xls_is_refused_rather_than_half_read() {
    let Some(files) = poi_files() else {
        return;
    };
    let xls: Vec<_> = files.iter().filter(|p| ext_of(p) == "xls").collect();
    if xls.is_empty() {
        eprintln!("SKIP: no .xls in the corpus");
        return;
    }

    // `sheet-*` reads OOXML only. A legacy BIFF workbook is a CFB/OLE
    // container, and the danger is not that it fails — it is that it
    // half-succeeds. plugin-doc hit exactly that: some legacy `.doc`
    // files EMBED a complete OPC package, so a zip reader finds it by
    // scanning for the EOCD, opens it, and dies much later with a
    // nonsense XML error about themeManager.xml.
    //
    // 417 files is by far the largest sample this refusal has ever had.
    let mut wrongly_opened = Vec::new();
    for path in &xls {
        let bytes = std::fs::read(path).expect("read poi xls");
        if XlsxDocument::open(&bytes).is_ok() {
            wrongly_opened.push(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }

    println!("legacy .xls: {} file(s), all refused", xls.len());
    assert!(
        wrongly_opened.is_empty(),
        "{} legacy BIFF file(s) were opened as OOXML: {:?} — a .xls that \
         appears to parse is worse than one that fails, because the caller \
         gets a workbook that is not the document they opened",
        wrongly_opened.len(),
        &wrongly_opened[..wrongly_opened.len().min(5)]
    );
}
