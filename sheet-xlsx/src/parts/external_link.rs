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

//! `xl/externalLinks/externalLinkN.xml` — the CACHED last-known values of an
//! external (referenced-but-not-embedded) workbook (ECMA-376 §18.14
//! `externalLink`; M3 external-link reads, spec §13; the no-network ruling,
//! spec §1.1).
//!
//! ## What this is — and the hard scope ruling (§1.1)
//!
//! Excel stores, inside the workbook, a snapshot of the cells a formula reads
//! from ANOTHER workbook (`=[1]Sheet1!A1`), so the file can display those
//! values without the source workbook open. paged.sheet reads ONLY that cache:
//! **external links are NEVER followed.** No network, no file-system traversal,
//! no live refresh — the referenced workbook (named in the part's `.rels`
//! `Target`/`externalBook`) is treated as an opaque label we never resolve.
//! This is a publishing-first product decision (spec §1, permanent), not a
//! deferral. A reference to an external workbook yields the value frozen at the
//! authoring application's last save (or [`CellError::Ref`] when the cache has
//! no entry for that cell — the documented fallback, see [`ExternalBook::get`]).
//!
//! ## What we read (the honest T3 subset)
//!
//! Each `externalLinkN.xml` holds one `<externalBook>` with:
//!
//! - `<sheetNames><sheetName val="…"/>` — the cached sheet names of the source
//!   book, in index order (the `[n]Sheet1!` `Sheet1` resolves through this);
//! - `<sheetDataSet><sheetData sheetId="i"><row r="…"><cell r="A1" t="…"><v>…`
//!   — the cached cell VALUES, keyed by `(sheetId, row, col)`. The `t=` types
//!   mirror ST_CellType's external-cache subset (`n`/`str`/`b`/`e`; default
//!   number). `<sheetData>` and `<row>` carry no shared-string indirection
//!   (external caches inline their text), so no shared table is consulted.
//!
//! `<definedNames>` (cached external defined names) are NOT modeled — a formula
//! that references an external NAME is out of the cached-cell-read slice; the
//! part still round-trips verbatim. Everything we don't model survives because
//! the part stays OPAQUE in the OPC container (never promoted), so it re-emits
//! byte-identical on save (preservation invariant, spec §10.2) — the same
//! discipline as the chart/table parts.
//!
//! ## Why no AST / parser change
//!
//! The frozen formula AST ([`sheet_core::ast::Expr`]) has no external-reference
//! variant, and `sheet-parser` does not parse the `[n]` external-book prefix.
//! That is intentional and sufficient: a cell whose formula is an external ref
//! already carries its cached result in the worksheet's own `<v>` (the
//! worksheet parser reads it as the cell value), so the cell DISPLAYS the
//! cached value with no AST support. This table is the parser-independent
//! resolution surface — for a consumer that wants to resolve an external
//! reference `(book, sheet, cell)` to its cached value directly (e.g.
//! `sheet_calc::external::resolve_cached`) — never a re-implementation of
//! external evaluation.

use crate::error::XlsxError;
use crate::opc::attr;
use compact_str::CompactString;
use sheet_core::parse_a1;
use sheet_core::value::{CellError, CellValue};
use std::collections::BTreeMap;

/// One referenced external workbook's CACHED snapshot (one `externalLinkN.xml`
/// part). Keyed in the parent [`ExternalLinks`] by the external-reference
/// index a formula's `[n]` prefix uses.
#[derive(Debug, Clone, Default)]
pub struct ExternalBook {
    /// Cached sheet names of the source book, in index order (the `sheetId`
    /// attribute of `<sheetData>` indexes into this). A `[n]Sheet1!` resolves
    /// `Sheet1` against this list.
    pub sheet_names: Vec<CompactString>,
    /// Cached cell values, keyed by `(source_sheet_index, row, col)` — both
    /// 0-based. The authoritative cached snapshot; absence means "no cache",
    /// which resolves to `#REF!` ([`ExternalBook::get`]).
    pub cells: BTreeMap<(u32, u32, u32), CellValue>,
}

impl ExternalBook {
    /// The 0-based source-sheet index for a cached sheet `name`
    /// (case-insensitive, matching Excel's sheet-name resolution). `None` if
    /// the cache has no such sheet.
    pub fn sheet_index(&self, name: &str) -> Option<u32> {
        self.sheet_names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }

    /// The CACHED value at `(sheet_index, row, col)` (all 0-based), or the
    /// documented fallback [`CellError::Ref`] when the cache holds no entry.
    ///
    /// Ruling (`sheet.xlsx.external-link.cached-value-read`): a missing cached
    /// cell yields `#REF!` — the source workbook is NEVER opened to fill it,
    /// so an un-cached cell is, from paged.sheet's read-only stance,
    /// unresolvable. (Excel itself shows `#REF!` for an external reference
    /// whose link target cannot be resolved and whose cache lacks the cell.)
    pub fn get(&self, sheet_index: u32, row: u32, col: u32) -> CellValue {
        self.cells
            .get(&(sheet_index, row, col))
            .cloned()
            .unwrap_or(CellValue::Error(CellError::Ref))
    }
}

/// Every referenced external workbook's cached snapshot, indexed by the
/// external-reference index (the workbook's `<externalReferences>` order,
/// which is what a formula's `[n]` prefix names). The `[n]` index is 1-BASED
/// in formulas; [`ExternalLinks::book`] converts.
#[derive(Debug, Clone, Default)]
pub struct ExternalLinks {
    /// One [`ExternalBook`] per `<externalReference>`, in workbook order. Index
    /// `i` here is the `[i+1]` a formula writes (see [`ExternalLinks::book`]).
    pub books: Vec<ExternalBook>,
}

impl ExternalLinks {
    /// True when no external links are present (the common case — most
    /// workbooks reference no external books).
    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
    }

    /// The cached book for a formula's `[n]` prefix (1-based, as written in
    /// `=[1]Sheet1!A1`). `None` if `n` is out of range.
    pub fn book(&self, n: u32) -> Option<&ExternalBook> {
        if n == 0 {
            return None;
        }
        self.books.get((n - 1) as usize)
    }
}

/// Parse one `externalLinkN.xml` part into an [`ExternalBook`]. Cached-only:
/// the source workbook named in the part's `<externalBook>`/`.rels` is NEVER
/// opened — only the inline `<sheetNames>` + `<sheetDataSet>` cache is read.
pub fn parse(xml: &[u8]) -> Result<ExternalBook, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    let mut book = ExternalBook::default();

    // The current `<sheetData sheetId="i">` index, and the in-progress
    // `<cell r="A1" t="…">` accumulator (its `<v>` text arrives as the next
    // Text event).
    let mut cur_sheet: u32 = 0;
    let mut cur_cell: Option<(u32, u32, String)> = None; // (row, col, t)
    let mut cur_v = String::new();
    // Are we inside a `<v>` whose text we should capture?
    let mut in_v = false;

    loop {
        match reader.read_event_into(&mut buf)? {
            // `<sheetName>`/`<sheetData>` carry their data in attributes, so
            // both the Start and the self-closing Empty form are equivalent.
            Event::Start(e) | Event::Empty(e)
                if matches!(e.local_name().as_ref(), b"sheetName" | b"sheetData") =>
            {
                match e.local_name().as_ref() {
                    b"sheetName" => {
                        if let Some(v) = attr(&e, b"val")? {
                            book.sheet_names.push(CompactString::new(&v));
                        }
                    }
                    b"sheetData" => {
                        cur_sheet = attr(&e, b"sheetId")?
                            .and_then(|s| s.trim().parse::<u32>().ok())
                            .unwrap_or(0);
                    }
                    _ => unreachable!(),
                }
            }
            // A `<cell>` with a value is a Start (its `<v>` child holds the
            // cached value). A self-closing `<cell/>` (Empty) carries no value
            // and is ignored — there is nothing to cache.
            Event::Start(e) if e.local_name().as_ref() == b"cell" => {
                let r = attr(&e, b"r")?;
                let ty = attr(&e, b"t")?.unwrap_or_default();
                cur_v.clear();
                cur_cell = r.as_deref().and_then(parse_a1).map(|rf| (rf.0, rf.1, ty));
            }
            Event::Start(e) if e.local_name().as_ref() == b"v" => {
                in_v = cur_cell.is_some();
                cur_v.clear();
            }
            Event::Text(t) if in_v => {
                let s = t.unescape().map_err(XlsxError::Xml)?;
                cur_v.push_str(&s);
            }
            Event::End(e) if e.local_name().as_ref() == b"v" => {
                in_v = false;
            }
            Event::End(e) if e.local_name().as_ref() == b"cell" => {
                if let Some((row, col, ty)) = cur_cell.take() {
                    let value = resolve_cached_value(&ty, cur_v.trim());
                    book.cells.insert((cur_sheet, row, col), value);
                }
                cur_v.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(book)
}

/// Map an external-cache `(t=, <v>)` to a [`CellValue`] (ECMA-376 §18.14.6
/// `externalCell` — the cache uses the inline ST_CellType subset; there is no
/// shared-string indirection in an external cache, so `str`/inline text is the
/// `<v>` text verbatim).
fn resolve_cached_value(ty: &str, v: &str) -> CellValue {
    match ty {
        // formula-string / text result (external caches inline their text).
        "str" | "s" | "inlineStr" => CellValue::Text(CompactString::new(v)),
        // boolean
        "b" => CellValue::Bool(v == "1"),
        // error
        "e" => CellValue::Error(CellError::parse(v).unwrap_or(CellError::Value)),
        // number (default) — empty `<v>` is a blank cached cell.
        "" | "n" => {
            if v.is_empty() {
                CellValue::Empty
            } else {
                v.parse::<f64>()
                    .map(CellValue::Number)
                    .unwrap_or(CellValue::Empty)
            }
        }
        // unknown type: keep the raw text (preservation-safe at the model layer;
        // the part still round-trips verbatim regardless).
        _ => CellValue::Text(CompactString::new(v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
              xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <externalBook r:id="rId1">
    <sheetNames>
      <sheetName val="Sheet1"/>
      <sheetName val="Costs"/>
    </sheetNames>
    <sheetDataSet>
      <sheetData sheetId="0">
        <row r="1">
          <cell r="A1"><v>42</v></cell>
          <cell r="B1" t="str"><v>hello</v></cell>
        </row>
        <row r="2">
          <cell r="A2" t="b"><v>1</v></cell>
          <cell r="B2" t="e"><v>#DIV/0!</v></cell>
        </row>
      </sheetData>
      <sheetData sheetId="1">
        <row r="3">
          <cell r="C3" t="n"><v>3.5</v></cell>
        </row>
      </sheetData>
    </sheetDataSet>
  </externalBook>
</externalLink>"#;

    #[test]
    fn parses_sheet_names_and_cached_values() {
        let b = parse(SAMPLE).unwrap();
        assert_eq!(
            b.sheet_names,
            vec![CompactString::new("Sheet1"), CompactString::new("Costs")]
        );
        // Sheet 0 (Sheet1).
        assert_eq!(b.get(0, 0, 0), CellValue::Number(42.0)); // A1
        assert_eq!(b.get(0, 0, 1), CellValue::Text("hello".into())); // B1
        assert_eq!(b.get(0, 1, 0), CellValue::Bool(true)); // A2
        assert_eq!(b.get(0, 1, 1), CellValue::Error(CellError::Div0)); // B2
                                                                       // Sheet 1 (Costs), C3.
        assert_eq!(b.get(1, 2, 2), CellValue::Number(3.5));
    }

    #[test]
    fn sheet_index_is_case_insensitive() {
        let b = parse(SAMPLE).unwrap();
        assert_eq!(b.sheet_index("Sheet1"), Some(0));
        assert_eq!(b.sheet_index("costs"), Some(1));
        assert_eq!(b.sheet_index("Nope"), None);
    }

    #[test]
    fn missing_cached_cell_is_ref_error() {
        let b = parse(SAMPLE).unwrap();
        // Z99 was never cached → the documented #REF! fallback (no source
        // workbook is ever opened to fill it).
        assert_eq!(b.get(0, 98, 25), CellValue::Error(CellError::Ref));
    }
}
