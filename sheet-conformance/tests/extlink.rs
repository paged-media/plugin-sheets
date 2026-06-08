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

//! External-workbook link reads (M3, spec §13; the no-network ruling §1.1).
//! Test-fn names are the registry pointers for
//! `registry/features/extlink.yaml` (the coverage gate greps these prefixes).
//!
//! HARD SCOPE RULING (§1.1): external links are NEVER followed — paged.sheet
//! reads ONLY the CACHED last values Excel stored in the workbook's
//! `externalLinkN.xml` parts. No network, no file-system access, no live
//! refresh. This is a publishing-first product decision (spec §1, permanent),
//! not a deferral.
//!
//! ## The honest, minimal slice these tests pin
//!
//! - **part-parse:** the `<externalReferences>` order + each
//!   `externalLink1.xml` cached snapshot (sheet names + cell values) parse into
//!   `XlsxDocument::external_links`.
//! - **cached-value-read:** a reference `=[1]Sheet1!A1` resolves to the CACHED
//!   value through the pure `sheet_calc::external::resolve_cached` (the frozen
//!   AST has no external-ref variant, so the local cell already shows the
//!   cached value inline in its own `<v>` — both paths are asserted). An
//!   un-cached cell yields the documented `#REF!` fallback.
//! - **preserve:** the externalLink part + its EXTERNAL `.rels` round-trip
//!   byte-identical (the part stays OPAQUE — preservation invariant §10.2).
//! - **no-network:** asserted BY CONSTRUCTION — the only inputs are the
//!   workbook's own bytes; the resolver reads only the in-memory cache; the
//!   source workbook URI (`TargetMode="External"`) is recorded, never opened.

use std::path::PathBuf;

use sheet_calc::external::{resolve_cached, ExternalCache, ExternalRef};
use sheet_core::value::{CellError, CellValue};
use sheet_xlsx::{ExternalLinks, XlsxDocument};

// ── fixtures ────────────────────────────────────────────────────────────────

/// Path to `corpus/xlsx-corpus/` (sibling of the conformance crate).
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join("xlsx-corpus")
}

fn load(name: &str) -> Vec<u8> {
    let p = corpus_dir().join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read fixture {}: {e}", p.display()))
}

const FIXTURE: &str = "10-extlink.xlsx";

/// Unzip a package to `(name, bytes)` (skips dirs) — for the per-part identity
/// assertion.
fn unzip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_owned();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        out.push((name, data));
    }
    out
}

/// The bridge from the XLSX layer's parsed [`ExternalLinks`] to the
/// dependency-free [`ExternalCache`] trait the resolver consumes. This is the
/// glue a real consumer (`sheet-js`) writes: `sheet-calc` does NOT depend on
/// `sheet-xlsx`, so the cache is supplied through the trait. CACHED-ONLY — it
/// only ever reads the in-memory `ExternalLinks` (no I/O, no network).
struct LinksCache<'a>(&'a ExternalLinks);

impl ExternalCache for LinksCache<'_> {
    fn cached(&self, r: &ExternalRef) -> Option<CellValue> {
        let book = self.0.book(r.book)?;
        let sheet_idx = book.sheet_index(&r.sheet)?;
        // `ExternalBook::get` returns the documented #REF! fallback for an
        // un-cached cell; here we report a genuine MISS as `None` so the
        // resolver applies its OWN fallback uniformly (book/sheet/cell misses
        // all collapse to #REF!). A present cache entry is `Some(value)`.
        book.cells.get(&(sheet_idx, r.row, r.col)).cloned()
    }
}

fn xref(book: u32, sheet: &str, row: u32, col: u32) -> ExternalRef {
    ExternalRef {
        book,
        sheet: sheet.to_string(),
        row,
        col,
    }
}

// ── sheet.xlsx.external-link.part-parse ──────────────────────────────────────

/// The `<externalReferences>` order + the externalLink1.xml cached snapshot
/// (sheet names + cell values per source sheet) parse into
/// `XlsxDocument::external_links`.
#[test]
fn sheet_xlsx_external_link_part_parse() {
    let doc = XlsxDocument::open(&load(FIXTURE)).expect("open 10-extlink");

    // One external book (the single <externalReference>).
    assert_eq!(doc.external_links.books.len(), 1, "one external book");
    let book = doc.external_links.book(1).expect("[1] book present");

    // Cached sheet names, in index order.
    assert_eq!(book.sheet_names.len(), 2);
    assert_eq!(book.sheet_index("Sheet1"), Some(0));
    assert_eq!(book.sheet_index("Costs"), Some(1));

    // Cached cell values on sheet 0 (Sheet1): A1=42, B1="hello", A2=TRUE,
    // B2=#DIV/0!  (the t= types n/str/b/e).
    assert_eq!(book.get(0, 0, 0), CellValue::Number(42.0));
    assert_eq!(book.get(0, 0, 1), CellValue::Text("hello".into()));
    assert_eq!(book.get(0, 1, 0), CellValue::Bool(true));
    assert_eq!(book.get(0, 1, 1), CellValue::Error(CellError::Div0));
    // Cached value on sheet 1 (Costs): C3=3.5.
    assert_eq!(book.get(1, 2, 2), CellValue::Number(3.5));
}

// ── sheet.xlsx.external-link.cached-value-read ───────────────────────────────

/// A reference `=[1]Sheet1!A1` resolves to the CACHED value through the pure
/// resolver, and the LOCAL cell that holds the external-ref formula already
/// displays the same cached value inline (its own `<v>` — the parser never sees
/// the `[1]` prefix). An un-cached cell yields the documented `#REF!` fallback.
#[test]
fn sheet_xlsx_external_link_cached_value_read() {
    let doc = XlsxDocument::open(&load(FIXTURE)).expect("open 10-extlink");
    let cache = LinksCache(&doc.external_links);

    // [1]Sheet1!A1 -> cached 42 ; [1]Sheet1!B1 -> "hello" ; [1]Costs!C3 -> 3.5.
    assert_eq!(
        resolve_cached(&cache, &xref(1, "Sheet1", 0, 0)),
        CellValue::Number(42.0)
    );
    assert_eq!(
        resolve_cached(&cache, &xref(1, "Sheet1", 0, 1)),
        CellValue::Text("hello".into())
    );
    assert_eq!(
        resolve_cached(&cache, &xref(1, "Costs", 2, 2)),
        CellValue::Number(3.5)
    );
    // The cached error value comes through as an error VALUE (=[1]Sheet1!B2).
    assert_eq!(
        resolve_cached(&cache, &xref(1, "Sheet1", 1, 1)),
        CellValue::Error(CellError::Div0)
    );

    // The LOCAL cell A1 holds the formula =[1]Sheet1!A1; its CACHED result is
    // stored inline in the cell's own <v>, so the model DISPLAYS 42 with no AST
    // support (the honest no-parser-change slice). The raw formula text is
    // captured verbatim (round-trips), NOT parsed into the model.
    let ws = doc.model.sheet(0).unwrap();
    assert_eq!(ws.cell(0, 0).unwrap().value, CellValue::Number(42.0));
    assert_eq!(
        doc.formula_texts.get(&(0, 0, 0)).map(String::as_str),
        Some("[1]Sheet1!A1"),
        "the external-ref formula text round-trips (never parsed into the AST)"
    );
    // A2 = [1]Sheet1!B2 displays its cached #DIV/0!.
    assert_eq!(
        ws.cell(1, 0).unwrap().value,
        CellValue::Error(CellError::Div0)
    );

    // Un-cached cell (Sheet1!Z99) -> the documented #REF! fallback (the source
    // workbook is NEVER opened to fill it).
    assert_eq!(
        resolve_cached(&cache, &xref(1, "Sheet1", 98, 25)),
        CellValue::Error(CellError::Ref)
    );
}

// ── sheet.xlsx.external-link.preserve ────────────────────────────────────────

/// The externalLink part + its EXTERNAL `.rels` round-trip byte-identical (the
/// part stays OPAQUE — preservation invariant §10.2 — so "Paged never destroys
/// a workbook" holds for external links exactly as for any understood-but-
/// unmodified part). The model still re-parses from the saved bytes.
#[test]
fn sheet_xlsx_external_link_preserve() {
    let doc = XlsxDocument::open(&load(FIXTURE)).expect("open 10-extlink");
    assert!(!doc.is_dirty(), "open must not dirty the container");

    let out = doc.save().expect("save");
    let orig = unzip(&load(FIXTURE));
    let saved = unzip(&out);

    for part in [
        "xl/externalLinks/externalLink1.xml",
        "xl/externalLinks/_rels/externalLink1.xml.rels",
    ] {
        let a = orig.iter().find(|(n, _)| n == part).map(|(_, b)| b);
        let b = saved.iter().find(|(n, _)| n == part).map(|(_, b)| b);
        assert!(a.is_some(), "{part} present in the fixture");
        assert_eq!(a, b, "{part} must round-trip byte-identical (preservation)");
    }

    // The external links + the local cached cell still resolve from the saved
    // bytes (a zero-edit round-trip preserves the read model).
    let doc2 = XlsxDocument::open(&out).expect("reopen saved 10-extlink");
    assert_eq!(doc2.external_links.books.len(), 1);
    assert_eq!(
        doc2.external_links.book(1).unwrap().get(0, 0, 0),
        CellValue::Number(42.0)
    );
    assert_eq!(
        doc2.model.sheet(0).unwrap().cell(0, 0).unwrap().value,
        CellValue::Number(42.0)
    );
}

// ── sheet.xlsx.external-link.no-network ──────────────────────────────────────

/// The constitutional ruling made executable: parse + resolve happen with NO
/// network and NO file access beyond the workbook's own bytes (BY
/// CONSTRUCTION). We document the guarantee and assert its observable
/// consequence: the source workbook URI is NEVER consulted, so a cache MISS
/// always yields `#REF!` (a live read would instead "succeed") — and a book
/// index past the cache is also `#REF!`, never an attempt to open a file.
#[test]
fn sheet_xlsx_external_link_no_network() {
    // PARSE reads only the in-memory `bytes` slice. There is no code path in
    // `XlsxDocument::open` (nor in the external-link parser) that performs I/O,
    // opens a file, or hits the network: the source workbook named in the
    // externalLink `.rels` (`TargetMode="External"`) is stored but never
    // resolved. This test exercises the whole pipeline from in-memory bytes and
    // asserts the cached-only behaviour; the absence of any network/FS API is a
    // structural property of the crate (no `std::net`, no extra `std::fs` read).
    let bytes = load(FIXTURE);
    let doc = XlsxDocument::open(&bytes).expect("open 10-extlink (in-memory only)");
    let cache = LinksCache(&doc.external_links);

    // A CACHED cell resolves to its stored value (no live read needed).
    assert_eq!(
        resolve_cached(&cache, &xref(1, "Sheet1", 0, 0)),
        CellValue::Number(42.0)
    );

    // An UN-cached cell is #REF! — a live read would have to OPEN the source
    // workbook to know the value; because the link is never followed, the only
    // honest answer is the cached state, i.e. #REF!.
    assert_eq!(
        resolve_cached(&cache, &xref(1, "Sheet1", 500, 500)),
        CellValue::Error(CellError::Ref),
        "an un-cached external cell is #REF! — the link is never followed"
    );

    // A reference to a NON-EXISTENT external book is #REF!, never a file open.
    assert_eq!(
        resolve_cached(&cache, &xref(99, "Sheet1", 0, 0)),
        CellValue::Error(CellError::Ref),
        "an out-of-range external book is #REF! (no file lookup)"
    );

    // A reference to a sheet not in the cache is #REF! (no live resolution).
    assert_eq!(
        resolve_cached(&cache, &xref(1, "NoSuchSheet", 0, 0)),
        CellValue::Error(CellError::Ref),
        "an un-cached external sheet is #REF! (no file lookup)"
    );
}
