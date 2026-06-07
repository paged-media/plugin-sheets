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

//! Reference extraction (spec §6.2). Walks the AST and collects every cell
//! ref, range, and defined-name id, plus a `has_volatile` flag set if any
//! called function carries the registry `volatile` bit. The dependency graph
//! (in `sheet-calc`) consumes this to build edges; ranges are kept as
//! intervals (normalized), never exploded per-cell.

use compact_str::CompactString;
use smallvec::SmallVec;

use sheet_core::ast::{Expr, Formula, NameId};
use sheet_core::{CellRef, RangeRef};

/// The references a formula reads from, plus its volatility.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefSet {
    /// Single-cell references.
    pub cells: Vec<CellRef>,
    /// Range references (each normalized so `start <= end`).
    pub ranges: Vec<RangeRef>,
    /// Defined-name ids referenced.
    pub names: Vec<NameId>,
    /// Structured-reference table names (M1 tables track). A structured ref
    /// depends on its table by NAME, not by A1 geometry; the dependency
    /// graph resolves the name to a range when the table model is wired
    /// (M1 Phase B). Usually 0 or 1 entry, so a `SmallVec<[_; 1]>`.
    pub tables: SmallVec<[CompactString; 1]>,
    /// True if any called function is registry-`volatile`.
    pub has_volatile: bool,
}

/// Extract the [`RefSet`] of a formula.
pub fn extract_refs(f: &Formula) -> RefSet {
    let mut set = RefSet::default();
    walk(&f.root, &mut set);
    set
}

fn walk(e: &Expr, set: &mut RefSet) {
    match e {
        Expr::Lit(_) => {}
        Expr::Ref(r) => set.cells.push(*r),
        Expr::Range(r) => set.ranges.push(r.normalized()),
        Expr::Name(n) => set.names.push(*n),
        Expr::Unary(_, inner) => walk(inner, set),
        Expr::Binary(_, a, b) => {
            walk(a, set);
            walk(b, set);
        }
        Expr::Func(fid, args) => {
            if sheet_core::funcs::meta(*fid).volatile {
                set.has_volatile = true;
            }
            for a in args {
                walk(a, set);
            }
        }
        Expr::Array(rows) => {
            for row in rows {
                for el in row {
                    walk(el, set);
                }
            }
        }
        // A structured ref depends on its table by name (the ThisRow
        // `[@Col]` form carries an empty table name — the in-table anchor is
        // resolved from the formula's own cell in Phase B, so an empty name
        // is not recorded as a dependency).
        Expr::StructuredRef(s) => {
            if !s.table.is_empty() {
                set.tables.push(s.table.clone());
            }
        }
        // A spill ref's dependency is its anchor expression.
        Expr::SpillRef(inner) => walk(inner, set),
    }
}
