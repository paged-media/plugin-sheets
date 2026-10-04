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

//! Defined names whose target is a FORMULA (a constant `0.5`, an expression
//! `SUM(Sheet1!$A$1:$A$3)`, a reference-valued `OFFSET(…)`, an array
//! constant `{1,2,3}`). Plain-reference names are `NameTarget::Range`; these
//! keep their text in the model and are compiled here when the engine is
//! built, then evaluated in the REFERRING cell's context (Excel's rule).
//!
//! A RELATIVE reference inside a stored name is relative to A1 and re-based
//! on the referring cell, wrapping around the grid: `Calc!$I1048576` read from
//! row 10 is `Calc!$I9` (the row above) — Excel's own encoding.

use std::borrow::Cow;

use sheet_core::ast::{Expr, NameId};
use sheet_core::names::NameTarget;
use sheet_core::{CellRef, NameScope, RangeRef, SheetModel, MAX_COL, MAX_ROW};
use sheet_parser::{parse, strip_storage_prefixes, RefSet};

/// How deep a name may refer to other names before evaluation gives up
/// (`#NAME?`) — guards a name defined in terms of itself.
pub(crate) const MAX_NAME_DEPTH: u32 = 32;

/// Parse every `Formula` target into the name table's compiled slot.
pub fn compile_names(model: &mut SheetModel) {
    let mut out = Vec::new();
    for (id, def) in model.names.iter() {
        let NameTarget::Formula(text) = &def.target else {
            continue;
        };
        let current = match def.scope {
            NameScope::Sheet(s) => s,
            NameScope::Workbook => 0,
        };
        let ctx = crate::ModelParseCtx {
            model,
            current,
        };
        let text = text.trim_start_matches('=');
        out.push((id, parse(&strip_storage_prefixes(text), &ctx).ok()));
    }
    for (id, f) in out {
        model.names.set_compiled(id, f);
    }
}

/// The compiled expression of a formula-target name.
pub(crate) fn name_expr(model: &SheetModel, id: NameId) -> Option<&Expr> {
    match &model.names.get(id)?.target {
        NameTarget::Formula(_) => model.names.compiled(id).map(|f| &f.root),
        NameTarget::Range(_) => None,
    }
}

/// Whether an expression holds a relative row or column anywhere.
fn has_relative(e: &Expr) -> bool {
    let rel = |c: &CellRef| !c.row_abs || !c.col_abs;
    match e {
        Expr::Ref(c) => rel(c),
        Expr::Range(r) => rel(&r.start) || rel(&r.end),
        Expr::Unary(_, a) | Expr::SpillRef(a) => has_relative(a),
        Expr::Binary(_, a, b) => has_relative(a) || has_relative(b),
        Expr::Func(_, args) => args.iter().any(has_relative),
        Expr::Call(c, args) => has_relative(c) || args.iter().any(has_relative),
        Expr::Array(rows) => rows.iter().flatten().any(has_relative),
        Expr::Lit(_) | Expr::Name(_) | Expr::StructuredRef(_) | Expr::Local(_) => false,
    }
}

fn rebase_cell(c: CellRef, at: CellRef) -> CellRef {
    let wrap = |v: u32, d: u32, max: u32| ((v as u64 + d as u64) % (max as u64 + 1)) as u32;
    CellRef {
        row: if c.row_abs { c.row } else { wrap(c.row, at.row, MAX_ROW) },
        col: if c.col_abs { c.col } else { wrap(c.col, at.col, MAX_COL) },
        ..c
    }
}

fn rebase(e: &Expr, at: CellRef) -> Expr {
    let f = |x: &Expr| rebase(x, at);
    match e {
        Expr::Ref(c) => Expr::Ref(rebase_cell(*c, at)),
        Expr::Range(r) => Expr::Range(RangeRef {
            start: rebase_cell(r.start, at),
            end: rebase_cell(r.end, at),
        }),
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(f(a))),
        Expr::Binary(op, a, b) => Expr::Binary(*op, Box::new(f(a)), Box::new(f(b))),
        Expr::Func(fid, args) => Expr::Func(*fid, args.iter().map(f).collect()),
        Expr::Array(rows) => Expr::Array(rows.iter().map(|r| r.iter().map(f).collect()).collect()),
        Expr::SpillRef(a) => Expr::SpillRef(Box::new(f(a))),
        Expr::Call(c, args) => Expr::Call(Box::new(f(c)), args.iter().map(f).collect()),
        Expr::Lit(_) | Expr::Name(_) | Expr::StructuredRef(_) | Expr::Local(_) => e.clone(),
    }
}

/// A formula-target name's expression as seen from the referring cell `at`
/// (relative parts re-based), or `None` for a range / uncompiled name.
pub(crate) fn name_expr_at(model: &SheetModel, id: NameId, at: CellRef) -> Option<Cow<'_, Expr>> {
    let e = name_expr(model, id)?;
    Some(if has_relative(e) {
        Cow::Owned(rebase(e, at))
    } else {
        Cow::Borrowed(e)
    })
}

/// `extract_refs` of a formula PLUS the references of every formula-target
/// name it uses (transitively, re-based on `cell`), so the dependency graph
/// and the volatile set see through names.
pub fn refs_with_names(model: &SheetModel, f: &sheet_core::ast::Formula, cell: CellRef) -> RefSet {
    let mut refs = sheet_parser::extract_refs(f);
    let mut seen: Vec<NameId> = Vec::new();
    let mut todo: Vec<NameId> = refs.names.clone();
    while let Some(id) = todo.pop() {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let Some(inner) = name_expr_at(model, id, cell) else {
            continue;
        };
        let r = sheet_parser::extract_refs(&sheet_core::ast::Formula {
            root: inner.into_owned(),
        });
        refs.cells.extend(r.cells);
        refs.ranges.extend(r.ranges);
        refs.tables.extend(r.tables);
        refs.structured.extend(r.structured);
        refs.has_self_table_ref |= r.has_self_table_ref;
        refs.has_volatile |= r.has_volatile;
        for n in r.names {
            if !refs.names.contains(&n) {
                refs.names.push(n);
            }
            todo.push(n);
        }
    }
    refs
}
