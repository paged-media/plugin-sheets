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

//! Real spreadsheets — every workbook in the corpus that Excel, or some
//! other third-party writer, actually produced.
//!
//! Every fixture in `corpus/xlsx-corpus/` comes from `generate.py`: five
//! hand-written parts, fixed 1980 timestamps, no `calcChain.xml`, no
//! theme, no RSID noise. They are precise and diffable, and they are all
//! OUR shape. This lane exists because that is not the world.
//!
//! It walks `corpus/xlsx/` whole, so a new set dropped into a new
//! subdirectory is covered the day it lands. Three sources sit there
//! today and they get **deliberately different assertions**, because
//! they are different KINDS of evidence:
//!
//! * **`xlsx/poi/`** — Apache POI's corpus, accumulated over twenty
//!   years of BUG REPORTS. Many Excel versions, OpenOffice, Gnumeric,
//!   third-party writers, and a good number of files that are
//!   deliberately malformed because they once crashed something.
//!   Asserting they all parse would be asserting that upstream's
//!   regression suite contains no regressions. The honest properties
//!   are: never panic, never hang, fail with a TYPED error. The rate is
//!   REPORTED, not gated.
//!
//! * **`xlsx/authored/`** — 17 dashboards written by Excel itself: 14 by
//!   **Excel Online**, 2 by desktop Excel 16, 1 by Excel 2010. These are
//!   well-formed output from the producer we exist to read, so the bar
//!   is the Envato bar — **every one must open**, and a failure is a
//!   real defect rather than a curiosity.
//!
//!   Excel Online matters disproportionately: it writes subtly different
//!   OOXML from desktop Excel and nothing else in the corpus covers it.
//!   The set is also chart-heavy (147 chart parts, 31 tables, slicers,
//!   ActiveX, chartsheets), which is the first time `sheet-chart` faces
//!   a chart Excel wrote rather than one we generated.
//!
//! * **`xlsx/poi-converted/`** — POI's legacy `.xls` re-saved as OOXML
//!   by desktop Excel 16 (`corpus/harness/convert-office.sh`, a
//!   maintainer tool; CI consumes the committed output). 398 of 428
//!   converted. These invert the POI bargain: the INPUT was a
//!   bug-report corpus, but the OUTPUT is Excel's own writer, so the
//!   bar is the authored bar — **every one must open**. What they add
//!   is twenty years of content that was odd enough to file a bug
//!   about, expressed in the OOXML of the producer we exist to read.
//!   Without the conversion all 417 of those files were refused by
//!   name, which exercises one branch and then stops being evidence.
//!
//! The `.xls` refusal is the most-tested path here: 417 legacy BIFF
//! files, against machinery that used to face four.
//!
//! Licence: POI is Apache-2.0 (the first redistributable fixtures this
//! project carries). `xlsx/authored/` is third-party template downloads
//! with UNRESOLVED rights — catalogued `redistributable: null`, which
//! means unknown, not yes. See each `PROVENANCE.md`.
//!
//! OPT-IN — the assets live in the private corpus checkout:
//!
//! ```text
//! PAGED_XLSX_CORPUS=1 cargo test -p sheet-conformance --test real_xlsx_corpus -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use sheet_xlsx::XlsxDocument;

const EXTS: &[&str] = &["xlsx", "xlsm", "xlsb", "xltx", "xltm", "xls"];

/// Recursive walk — `sheet-conformance` has no `walkdir`, and pulling a
/// dependency in for one test lane is not worth it.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_symlink() {
            continue;
        }
        if p.is_dir() {
            // Skip dot-dirs; `fonts/.cache` and friends are machine-local.
            if !p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                walk(&p, out);
            }
        } else if p.is_file()
            && p.extension()
                .is_some_and(|e| EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
        {
            out.push(p);
        }
    }
}

/// The corpus `xlsx/` root, or `None` with a printed reason.
fn xlsx_root() -> Option<PathBuf> {
    let Some(switch) = std::env::var_os("PAGED_XLSX_CORPUS") else {
        eprintln!(
            "SKIP real xlsx lane: PAGED_XLSX_CORPUS unset \
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
    let dir = root.join("xlsx");
    if !dir.is_dir() {
        eprintln!("SKIP real xlsx lane: {} not readable", dir.display());
        return None;
    }
    Some(dir)
}

/// Every spreadsheet under `corpus/xlsx/`, recursively.
fn corpus_files() -> Option<Vec<PathBuf>> {
    let root = xlsx_root()?;
    let mut out = Vec::new();
    walk(&root, &mut out);
    out.sort();
    if out.is_empty() {
        eprintln!(
            "SKIP real xlsx lane: no spreadsheets under {}",
            root.display()
        );
        return None;
    }
    Some(out)
}

/// Files directly under one named source directory, e.g. `authored`.
fn source_files(source: &str) -> Option<Vec<PathBuf>> {
    let root = xlsx_root()?;
    let dir = root.join(source);
    let mut out = Vec::new();
    walk(&dir, &mut out);
    out.sort();
    if out.is_empty() {
        eprintln!("SKIP: no spreadsheets under {}", dir.display());
        return None;
    }
    Some(out)
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The corpus subdirectory a file came from — reporting only, so a rate
/// drop points at a SOURCE rather than at a number.
fn source_of(root: &Path, p: &Path) -> String {
    let Ok(rel) = p.strip_prefix(root) else {
        return "(outside)".to_string();
    };
    let mut parts = rel.components();
    match (parts.next(), parts.next()) {
        // Two or more components: the first is a directory.
        (Some(first), Some(_)) => first.as_os_str().to_string_lossy().into_owned(),
        // One component: the file sits directly in `xlsx/`.
        _ => "(root)".to_string(),
    }
}

#[test]
#[ignore = "real xlsx lane: opt-in (PAGED_XLSX_CORPUS=1 + the private corpus mount)"]
fn no_spreadsheet_in_the_corpus_panics_the_parser() {
    let (Some(root), Some(files)) = (xlsx_root(), corpus_files()) else {
        return;
    };
    println!("real xlsx corpus: {} file(s)", files.len());

    let mut opened = 0usize;
    let mut refused = 0usize;
    let mut by_ext: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut by_src: std::collections::BTreeMap<String, (usize, usize)> = Default::default();

    for path in &files {
        let bytes = std::fs::read(path).expect("read xlsx fixture");
        // `open` returning Err is a RESULT, not a failure — the POI half
        // of this corpus is full of files that are supposed to be
        // rejected. What must never happen is a panic, and `cargo test`
        // turns any panic here into a named failure with the file that
        // caused it.
        let ok = XlsxDocument::open(&bytes).is_ok();
        let (e, s) = (ext_of(path), source_of(&root, path));
        if ok {
            opened += 1;
            by_ext.entry(e).or_default().0 += 1;
            by_src.entry(s).or_default().0 += 1;
        } else {
            refused += 1;
            by_ext.entry(e).or_default().1 += 1;
            by_src.entry(s).or_default().1 += 1;
        }
    }

    println!("  opened {opened}, refused {refused}");
    for (src, (ok, err)) in &by_src {
        println!("    {src:<10} {ok:>4} opened  {err:>4} refused");
    }
    for (ext, (ok, err)) in &by_ext {
        println!("    .{ext:<5} {ok:>4} opened  {err:>4} refused");
    }

    // The one hard assertion at THIS level: real Excel output must be
    // readable. If NOTHING opens, the parser is broken rather than
    // strict — and that is the failure this lane exists to catch, since
    // every other fixture it has ever seen was written by generate.py.
    assert!(
        opened > 0,
        "not one of {} real spreadsheets opened — the parser has only ever \
         been fed generate.py output, so this means it cannot read real \
         Excel files at all",
        files.len()
    );
}

#[test]
#[ignore = "real xlsx lane: opt-in (PAGED_XLSX_CORPUS=1 + the private corpus mount)"]
fn every_authored_workbook_opens() {
    let Some(files) = source_files("authored") else {
        return;
    };

    // Unlike the POI set, these are well-formed output from the producer
    // this engine exists to read — 14 of the 17 from Excel Online, which
    // nothing else in the corpus covers. So the bar here is the Envato
    // bar: every file opens, and a failure is a defect to fix rather
    // than a curiosity to report.
    let mut failed = Vec::new();
    for path in &files {
        let bytes = std::fs::read(path).expect("read authored fixture");
        if let Err(e) = XlsxDocument::open(&bytes) {
            failed.push(format!(
                "{}: {e}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }

    println!(
        "authored: {} file(s), {} opened",
        files.len(),
        files.len() - failed.len()
    );
    assert!(
        failed.is_empty(),
        "{} of {} Excel-authored workbook(s) failed to open — these are \
         well-formed files from Excel itself, so each one is a real \
         parser defect:\n  {}",
        failed.len(),
        files.len(),
        failed.join("\n  ")
    );
}

#[test]
#[ignore = "real xlsx lane: opt-in (PAGED_XLSX_CORPUS=1 + the private corpus mount)"]
fn every_converted_workbook_opens() {
    let Some(files) = source_files("poi-converted") else {
        return;
    };

    // Same bar as `authored`, for the same reason: whatever the input
    // was, Excel 16 wrote these. A refusal here is our parser failing on
    // Excel's own output, which is a defect — not the "upstream ships
    // deliberately-broken fixtures" story that makes the POI rate a
    // report rather than a gate.
    //
    // 397/397 open as of 2026-08-20, so this starts green and stays a
    // gate rather than a ratchet.
    let mut failed = Vec::new();
    for path in &files {
        let bytes = std::fs::read(path).expect("read converted fixture");
        if let Err(e) = XlsxDocument::open(&bytes) {
            failed.push(format!(
                "{}: {e}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }

    println!(
        "poi-converted: {} file(s), {} opened",
        files.len(),
        files.len() - failed.len()
    );
    assert!(
        failed.is_empty(),
        "{} of {} Excel-converted workbook(s) failed to open — Excel 16 \
         wrote every one of these, so each is a real parser defect:\n  {}",
        failed.len(),
        files.len(),
        failed.join("\n  ")
    );
}

#[test]
#[ignore = "real xlsx lane: opt-in (PAGED_XLSX_CORPUS=1 + the private corpus mount)"]
fn legacy_xls_is_refused_rather_than_half_read() {
    let Some(files) = corpus_files() else {
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
        let bytes = std::fs::read(path).expect("read xls");
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
