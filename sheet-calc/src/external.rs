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

//! External-workbook reference resolution — CACHED VALUES ONLY (spec §13 M3
//! external-link reads; the no-network ruling §1.1).
//!
//! ## The hard scope ruling (§1.1)
//!
//! A formula may reference another workbook: `=[1]Sheet1!A1`. paged.sheet
//! resolves such a reference to the CACHED last-known value Excel stored in the
//! workbook's `externalLinkN.xml` parts (parsed in `sheet-xlsx`). **External
//! links are NEVER followed** — no network, no file-system access, no live
//! refresh. The referenced source workbook is treated as an opaque label this
//! crate never resolves. This is a publishing-first product decision (spec §1,
//! permanent), not a deferral.
//!
//! ## Why this is a thin, pure surface (no AST, no calc-graph)
//!
//! The frozen formula AST ([`sheet_core::ast::Expr`]) has no external-reference
//! variant and `sheet-parser` does not parse the `[n]` external-book prefix, so
//! the tree-walk evaluator ([`crate::eval`]) never SEES an external reference.
//! It does not need to: a cell whose formula is an external ref already carries
//! its cached result in the worksheet's own `<v>` (the worksheet parser stores
//! it as the cell value), so the cell DISPLAYS the cached value with no
//! evaluation. This module is the parser-INDEPENDENT resolution surface for a
//! consumer (e.g. `sheet-js`, or a future tier wiring an external-ref AST
//! variant via a versioned amendment) that holds an [`ExternalRef`] and a cache
//! and wants the cached [`CellValue`] directly.
//!
//! ## Dependency hygiene
//!
//! `sheet-calc` does NOT depend on `sheet-xlsx` (where the cache is parsed), so
//! the cache is supplied through the [`ExternalCache`] trait. `sheet-xlsx`'s
//! `ExternalLinks` (or any other source) implements it; this crate stays a pure
//! consumer. The fallback for a cache MISS is the documented ruling
//! ([`resolve_cached`]).

use sheet_core::value::{CellError, CellValue};

/// A reference to a single cell in an external (referenced-but-not-embedded)
/// workbook, as a formula writes it: `=[book]sheet!row,col`. `book` is the
/// 1-based external-reference index (the `[1]` in `=[1]Sheet1!A1`); `sheet` is
/// the source sheet NAME (resolved against the cache's cached sheet names);
/// `row`/`col` are 0-based.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRef {
    /// 1-based external-book index (the `[n]` prefix).
    pub book: u32,
    /// Source sheet name (the `Sheet1` in `[1]Sheet1!A1`).
    pub sheet: String,
    /// 0-based row in the source sheet.
    pub row: u32,
    /// 0-based column in the source sheet.
    pub col: u32,
}

/// A read-only source of CACHED external-workbook values. Implemented by the
/// XLSX layer's `ExternalLinks` (and by test doubles). It returns `Some(value)`
/// only when the cache holds an entry for `(book, sheet, row, col)` — it NEVER
/// opens the source workbook, performs I/O, or hits the network. A cache miss
/// is `None` (the caller maps it to the [`resolve_cached`] fallback).
pub trait ExternalCache {
    /// The cached value for an external reference, or `None` when the cache has
    /// no such entry (a missing book, a missing sheet, or an un-cached cell).
    /// CACHED-ONLY: never follows the link.
    fn cached(&self, r: &ExternalRef) -> Option<CellValue>;
}

/// Resolve an external reference to its CACHED value (spec §13;
/// `sheet.xlsx.external-link.cached-value-read`).
///
/// Returns the cached [`CellValue`] when `cache` holds it; otherwise the
/// documented fallback [`CellError::Ref`] (`#REF!`). RULING: a missing external
/// cache yields `#REF!` — the source workbook is NEVER opened to fill it, so an
/// un-cached external reference is, from paged.sheet's read-only stance,
/// unresolvable. (Excel shows `#REF!` for an external reference whose link
/// cannot be resolved and whose cache lacks the cell; a stale value persists
/// only while the cache holds it.) This function performs NO I/O and NO network
/// access by construction — it only reads `cache`.
pub fn resolve_cached(cache: &dyn ExternalCache, r: &ExternalRef) -> CellValue {
    cache.cached(r).unwrap_or(CellValue::Error(CellError::Ref))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A tiny in-memory cache double — proves the resolver is pure (no I/O).
    #[derive(Default)]
    struct MapCache {
        // (book, sheet-lowercased, row, col) -> value
        cells: BTreeMap<(u32, String, u32, u32), CellValue>,
    }

    impl MapCache {
        fn put(&mut self, book: u32, sheet: &str, row: u32, col: u32, v: CellValue) {
            self.cells
                .insert((book, sheet.to_ascii_lowercase(), row, col), v);
        }
    }

    impl ExternalCache for MapCache {
        fn cached(&self, r: &ExternalRef) -> Option<CellValue> {
            self.cells
                .get(&(r.book, r.sheet.to_ascii_lowercase(), r.row, r.col))
                .cloned()
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

    #[test]
    fn sheet_xlsx_external_link_cached_value_read_hit() {
        let mut c = MapCache::default();
        c.put(1, "Sheet1", 0, 0, CellValue::Number(42.0));
        c.put(1, "Sheet1", 0, 1, CellValue::Text("hi".into()));
        // [1]Sheet1!A1 -> cached 42 ; [1]Sheet1!B1 -> cached "hi".
        assert_eq!(
            resolve_cached(&c, &xref(1, "Sheet1", 0, 0)),
            CellValue::Number(42.0)
        );
        assert_eq!(
            resolve_cached(&c, &xref(1, "sheet1", 0, 1)),
            CellValue::Text("hi".into())
        );
    }

    #[test]
    fn sheet_xlsx_external_link_cached_value_read_miss_is_ref() {
        let c = MapCache::default();
        // Nothing cached -> the documented #REF! fallback (link never followed).
        assert_eq!(
            resolve_cached(&c, &xref(1, "Sheet1", 99, 99)),
            CellValue::Error(CellError::Ref)
        );
        // An out-of-range book index is also a miss -> #REF!.
        assert_eq!(
            resolve_cached(&c, &xref(7, "Ghost", 0, 0)),
            CellValue::Error(CellError::Ref)
        );
    }
}
