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

//! Lookup & reference family (spec §7, §11 T0). The seven T0 lookups:
//! `VLOOKUP`/`HLOOKUP` (vertical/horizontal table lookup), `INDEX`,
//! `MATCH`, `CHOOSE`, and the reference-arg `ROW`/`COLUMN`.
//!
//! Each kernel is the frozen pure signature
//! `fn(&[Arg], &EvalCtx) -> CellValue` — no I/O, no statics. The generated
//! [`crate::dispatch`] already arity-range-checks every call (a row's
//! `arity: {min, max}` becomes a `#VALUE!` guard *before* the kernel runs),
//! so a kernel sees an argument count inside its declared band; it stays
//! defensive at the edges anyway. All type conversion and error propagation
//! routes through [`crate::coerce`], so the cross-engine coercion rulings are
//! stated once (repo constitution, CLAUDE.md §"Pure kernels").
//!
//! ## Lookup-mode rulings (ECMA-376 §18.17.7)
//!
//! - **Approximate match** (`VLOOKUP`/`HLOOKUP` `range_lookup` default TRUE,
//!   `MATCH` type `1`/`-1`): the data is *assumed sorted*; the kernel walks
//!   it and keeps the last element on the correct side of the key (largest
//!   `<=` key ascending, smallest `>=` key descending). A key below the
//!   first ascending element (or above the first descending element) is
//!   `#N/A`. The cross-type total order is [`crate::coerce::compare`].
//! - **Exact match** (`range_lookup` FALSE, `MATCH` type `0`): equality with
//!   `*`/`?` wildcards for *text* keys (via [`crate::criteria`]); a missing
//!   key is `#N/A`.
//! - **Index bounds** are 1-based; out of the table's rows/cols is `#REF!`.
//!   `INDEX` row/col `0` (Excel's "whole row/column") is **not** in T0 and
//!   returns `#VALUE!` (documented degrade — the whole-vector form needs the
//!   spill machinery of T1).

use sheet_core::{CellError, CellRef, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::criteria::Matcher;
use crate::ctx::EvalCtx;

// ---------------------------------------------------------------------------
// VLOOKUP / HLOOKUP
// ---------------------------------------------------------------------------

/// `VLOOKUP(key, table, col_index, [range_lookup])` (ECMA-376 §18.17.7).
///
/// Searches the **first column** of `table` for `key`, then returns the cell
/// in column `col_index` (1-based) of the matching row. `range_lookup`
/// defaults TRUE (approximate: assumes the first column is sorted ascending
/// and takes the largest value `<= key`; `#N/A` when `key` precedes the first
/// value). FALSE is exact match with wildcard support for text keys.
/// `col_index < 1` is `#VALUE!`; past the table width is `#REF!`; no match is
/// `#N/A`.
pub fn vlookup(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    table_lookup(args, Axis::Vertical)
}

/// `HLOOKUP(key, table, row_index, [range_lookup])` (ECMA-376 §18.17.7) — the
/// horizontal transpose of [`vlookup`]: searches the **first row** and
/// returns the cell in row `row_index` (1-based) of the matching column.
pub fn hlookup(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    table_lookup(args, Axis::Horizontal)
}

/// Which axis a table lookup scans: `Vertical` searches the first column and
/// indexes by column (VLOOKUP); `Horizontal` searches the first row and
/// indexes by row (HLOOKUP).
#[derive(Copy, Clone, PartialEq, Eq)]
enum Axis {
    Vertical,
    Horizontal,
}

/// Shared VLOOKUP/HLOOKUP body (they differ only by axis).
fn table_lookup(args: &[Arg], axis: Axis) -> CellValue {
    // Scalar-arg errors propagate first (key + index + range_lookup are
    // scalars; the table is a range and is handled per Excel below).
    let key = match &args[0] {
        Arg::Scalar(v) => {
            if let CellValue::Error(e) = v {
                return CellValue::Error(*e);
            }
            v.clone()
        }
        // A range key implicit-intersects to its top-left in a scalar slot.
        Arg::Range(rv) => rv.get(0, 0),
    };

    let table = match &args[1] {
        Arg::Range(rv) => rv,
        // A scalar table is a 1×1 lookup — degenerate but total.
        Arg::Scalar(v) => {
            if let CellValue::Error(e) = v {
                return CellValue::Error(*e);
            }
            // Search a 1×1 table by re-dispatching against a synthesized view.
            return single_cell_table(&key, v, args, axis);
        }
    };

    let index = match coerce_scalar_number(&args[2]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    // Truncate toward zero (Excel reads the integer part of the index).
    let index = index.trunc();
    if index < 1.0 {
        return CellValue::Error(CellError::Value);
    }

    let approximate = match args.get(3) {
        Some(a) => match coerce_scalar_bool(a) {
            Ok(b) => b,
            Err(e) => return CellValue::Error(e),
        },
        None => true, // default TRUE = approximate
    };

    // Length of the search vector and the offset axis bound.
    let (search_len, index_max) = match axis {
        Axis::Vertical => (table.rows(), table.cols()),
        Axis::Horizontal => (table.cols(), table.rows()),
    };
    let idx0 = index as u32; // 1-based index, validated >= 1
    if idx0 > index_max {
        return CellValue::Error(CellError::Ref);
    }

    let cell_at = |pos: u32| -> CellValue {
        match axis {
            Axis::Vertical => table.get(pos, 0),
            Axis::Horizontal => table.get(0, pos),
        }
    };
    let result_at = |pos: u32| -> CellValue {
        match axis {
            Axis::Vertical => table.get(pos, idx0 - 1),
            Axis::Horizontal => table.get(idx0 - 1, pos),
        }
    };

    let found = if approximate {
        approximate_pos_asc(&key, search_len, &cell_at)
    } else {
        exact_pos(&key, search_len, &cell_at)
    };

    match found {
        Some(pos) => result_at(pos),
        None => CellValue::Error(CellError::Na),
    }
}

/// Degenerate 1×1 table (a scalar in the table slot): treat the lone cell as
/// the whole search vector and reuse the vector machinery.
fn single_cell_table(key: &CellValue, cell: &CellValue, args: &[Arg], axis: Axis) -> CellValue {
    let index = match coerce_scalar_number(&args[2]) {
        Ok(n) => n.trunc(),
        Err(e) => return CellValue::Error(e),
    };
    if index < 1.0 {
        return CellValue::Error(CellError::Value);
    }
    if index as u32 > 1 {
        return CellValue::Error(CellError::Ref);
    }
    let approximate = match args.get(3) {
        Some(a) => match coerce_scalar_bool(a) {
            Ok(b) => b,
            Err(e) => return CellValue::Error(e),
        },
        None => true,
    };
    let _ = axis;
    let cell_at = |_pos: u32| cell.clone();
    let found = if approximate {
        approximate_pos_asc(key, 1, &cell_at)
    } else {
        exact_pos(key, 1, &cell_at)
    };
    match found {
        Some(_) => cell.clone(),
        None => CellValue::Error(CellError::Na),
    }
}

// ---------------------------------------------------------------------------
// INDEX
// ---------------------------------------------------------------------------

/// `INDEX(range, row, [col])` (ECMA-376 §18.17.7). Returns the cell at the
/// 1-based `(row, col)` of `range`. For a single-row or single-column range
/// the second argument may address along the only axis and `col` is omitted.
///
/// **T0 ruling:** `row`/`col` `0` (Excel's "entire row/column" array form)
/// returns `#VALUE!` — the whole-vector result needs the T1 spill machinery
/// and is out of T0 scope (documented degrade, module header). Out-of-bounds
/// indices are `#REF!`; an error in any scalar argument propagates.
pub fn index(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = first_scalar_error(&args[1..]) {
        return CellValue::Error(e);
    }

    let rv = match &args[0] {
        Arg::Range(rv) => rv,
        Arg::Scalar(v) => {
            if let CellValue::Error(e) = v {
                return CellValue::Error(*e);
            }
            // A scalar is a 1×1 range; INDEX(x, 1[, 1]) == x.
            return index_scalar(v, args);
        }
    };

    let row_arg = match coerce_scalar_number(&args[1]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    let col_arg = match args.get(2) {
        Some(a) => match coerce_scalar_number(a) {
            Ok(n) => Some(n.trunc() as i64),
            Err(e) => return CellValue::Error(e),
        },
        None => None,
    };

    // T0 degrade: the whole-row/whole-column (index 0) array forms are not
    // implemented (need spill). Treat a 0 in an addressing slot as #VALUE!.
    if row_arg == 0 || matches!(col_arg, Some(0)) {
        return CellValue::Error(CellError::Value);
    }

    let (r, c) = if rv.rows() == 1 && col_arg.is_none() {
        // Single-row range, one selector: it addresses the column.
        (1i64, row_arg)
    } else if rv.cols() == 1 && col_arg.is_none() {
        // Single-column range, one selector: it addresses the row.
        (row_arg, 1i64)
    } else {
        (row_arg, col_arg.unwrap_or(1))
    };

    if r < 1 || c < 1 || r as u32 > rv.rows() || c as u32 > rv.cols() {
        return CellValue::Error(CellError::Ref);
    }
    rv.get(r as u32 - 1, c as u32 - 1)
}

/// `INDEX` over a scalar (1×1) range. Only `(1, [1])` is in bounds.
fn index_scalar(cell: &CellValue, args: &[Arg]) -> CellValue {
    let row = match coerce_scalar_number(&args[1]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    let col = match args.get(2) {
        Some(a) => match coerce_scalar_number(a) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return CellValue::Error(e),
        },
        None => 1,
    };
    if row == 0 || col == 0 {
        return CellValue::Error(CellError::Value);
    }
    if row == 1 && col == 1 {
        cell.clone()
    } else {
        CellValue::Error(CellError::Ref)
    }
}

// ---------------------------------------------------------------------------
// MATCH
// ---------------------------------------------------------------------------

/// `MATCH(key, range, [match_type])` (ECMA-376 §18.17.7). Returns the 1-based
/// position of `key` within the one-dimensional `range`.
///
/// `match_type` (default `1`):
/// - `1` — largest value `<= key`, `range` assumed **sorted ascending**;
/// - `0` — exact match, with `*`/`?` wildcards for a text key;
/// - `-1` — smallest value `>= key`, `range` assumed **sorted descending**.
///
/// No match (or a `1`/`-1` key out of range on the wrong side) is `#N/A`. An
/// error in `key` or `match_type` propagates.
pub fn match_fn(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let key = match &args[0] {
        Arg::Scalar(v) => {
            if let CellValue::Error(e) = v {
                return CellValue::Error(*e);
            }
            v.clone()
        }
        Arg::Range(rv) => rv.get(0, 0),
    };

    let rv = match &args[1] {
        Arg::Range(rv) => rv,
        Arg::Scalar(v) => {
            if let CellValue::Error(e) = v {
                return CellValue::Error(*e);
            }
            // 1×1 search vector.
            return match_scalar(&key, v, args);
        }
    };

    let match_type = match args.get(2) {
        Some(a) => match coerce_scalar_number(a) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return CellValue::Error(e),
        },
        None => 1,
    };

    // The search vector is the linear scan of the range (row-major), so a
    // 1×N or N×1 range both work; a rectangular range scans row-major (Excel
    // expects a vector, but we stay total).
    let len = rv.rows() * rv.cols();
    let at = |i: u32| -> CellValue {
        let c = i % rv.cols();
        let r = i / rv.cols();
        rv.get(r, c)
    };

    let pos = match match_type {
        0 => exact_pos(&key, len, &at),
        m if m > 0 => approximate_pos_asc(&key, len, &at),
        _ => approximate_pos_desc(&key, len, &at),
    };

    match pos {
        Some(p) => CellValue::Number((p + 1) as f64),
        None => CellValue::Error(CellError::Na),
    }
}

/// `MATCH` over a 1×1 range (scalar in the range slot).
fn match_scalar(key: &CellValue, cell: &CellValue, args: &[Arg]) -> CellValue {
    let match_type = match args.get(2) {
        Some(a) => match coerce_scalar_number(a) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return CellValue::Error(e),
        },
        None => 1,
    };
    let at = |_i: u32| cell.clone();
    let pos = match match_type {
        0 => exact_pos(key, 1, &at),
        m if m > 0 => approximate_pos_asc(key, 1, &at),
        _ => approximate_pos_desc(key, 1, &at),
    };
    match pos {
        Some(p) => CellValue::Number((p + 1) as f64),
        None => CellValue::Error(CellError::Na),
    }
}

// ---------------------------------------------------------------------------
// CHOOSE
// ---------------------------------------------------------------------------

/// `CHOOSE(index, value1, value2, …)` (ECMA-376 §18.17.7). Returns the
/// `index`-th value argument (1-based). `index` truncates toward zero; an
/// index `< 1` or past the value count is `#VALUE!`. Not range-aware: a value
/// argument that is a range implicit-intersects to its top-left (its scalar
/// projection) in T0. An error in `index` propagates; an error reaching the
/// chosen value passes through.
pub fn choose(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let index = match coerce_scalar_number(&args[0]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    if index < 1 {
        return CellValue::Error(CellError::Value);
    }
    // args[0] is the index; values are args[1..].
    let values = &args[1..];
    let pick = index as usize - 1;
    match values.get(pick) {
        Some(Arg::Scalar(v)) => v.clone(),
        Some(Arg::Range(rv)) => rv.get(0, 0),
        None => CellValue::Error(CellError::Value),
    }
}

// ---------------------------------------------------------------------------
// ROW / COLUMN
// ---------------------------------------------------------------------------

/// `ROW([reference])` (ECMA-376 §18.17.7). Reference-arg function: with an
/// argument it returns the 1-based row of the reference's top-left
/// ([`crate::arg::RangeView::origin`]); with no argument it returns the
/// 1-based row of the
/// cell being evaluated ([`EvalCtx::current`]). T0 returns a single number
/// (the array form over a multi-row reference is a T1 spill concern).
pub fn row(args: &[Arg], ctx: &EvalCtx) -> CellValue {
    let origin = ref_origin(args, ctx);
    CellValue::Number((origin.row + 1) as f64)
}

/// `COLUMN([reference])` (ECMA-376 §18.17.7) — the column transpose of
/// [`row`]: the 1-based column of the reference's top-left, or of the current
/// cell when omitted.
pub fn column(args: &[Arg], ctx: &EvalCtx) -> CellValue {
    let origin = ref_origin(args, ctx);
    CellValue::Number((origin.col + 1) as f64)
}

/// Resolve the reference cell for `ROW`/`COLUMN`: the origin of a range arg,
/// or the current cell when there is no argument. A scalar argument (no
/// recoverable address) falls back to the current cell — the conservative
/// total answer for the degenerate non-reference call.
fn ref_origin(args: &[Arg], ctx: &EvalCtx) -> CellRef {
    match args.first() {
        Some(Arg::Range(rv)) => rv.origin(),
        _ => ctx.current,
    }
}

// ---------------------------------------------------------------------------
// Shared search primitives
// ---------------------------------------------------------------------------

/// Exact-match scan over a `len`-element vector (`at(i)` reads element `i`).
///
/// For a **text** key, matching honors `*`/`?` wildcards against **text**
/// candidates (Excel exact lookup is wildcard-aware) — case-insensitive,
/// anchored both ends; non-text candidates never satisfy a text key (the
/// cross-type order keeps Number/Bool distinct from Text). For a non-text
/// key, equality is the cross-type total order [`coerce::compare`] (`5`
/// matches the number `5`, never the text `"5"`). Returns the 0-based
/// position of the first match.
fn exact_pos(key: &CellValue, len: u32, at: &dyn Fn(u32) -> CellValue) -> Option<u32> {
    if let CellValue::Text(_) = key {
        // Wildcard-aware text exact match: compile the key as a pattern and
        // test each *text* candidate's value. A plain key compiles to a
        // literal matcher, so this is ordinary case-insensitive equality
        // unless the key carries `*`/`?`.
        let matcher = Matcher::compile(coerce::to_text(key).as_str());
        for i in 0..len {
            if let CellValue::Text(t) = at(i) {
                if matcher.is_match(t.as_str()) {
                    return Some(i);
                }
            }
        }
        None
    } else {
        for i in 0..len {
            let cand = at(i);
            if coerce::compare(key, &cand) == std::cmp::Ordering::Equal {
                return Some(i);
            }
        }
        None
    }
}

/// Approximate ascending scan (`VLOOKUP`/`HLOOKUP` default, `MATCH` type `1`):
/// the data is assumed sorted ascending; return the 0-based position of the
/// largest element `<= key`. A key below the first element is `None` (`#N/A`).
/// An exact hit short-circuits.
fn approximate_pos_asc(key: &CellValue, len: u32, at: &dyn Fn(u32) -> CellValue) -> Option<u32> {
    use std::cmp::Ordering::*;
    let mut best: Option<u32> = None;
    for i in 0..len {
        let cand = at(i);
        match coerce::compare(&cand, key) {
            Equal => return Some(i), // exact hit wins immediately
            Less => best = Some(i),  // candidate <= key: a better floor
            Greater => { /* candidate > key: skip (assumed-sorted) */ }
        }
    }
    best
}

/// Approximate descending scan (`MATCH` type `-1`): the data is assumed sorted
/// descending; return the 0-based position of the smallest element `>= key`. A
/// key above the first element is `None` (`#N/A`). An exact hit short-circuits.
fn approximate_pos_desc(key: &CellValue, len: u32, at: &dyn Fn(u32) -> CellValue) -> Option<u32> {
    use std::cmp::Ordering::*;
    let mut best: Option<u32> = None;
    for i in 0..len {
        let cand = at(i);
        match coerce::compare(&cand, key) {
            Equal => return Some(i),   // exact hit wins immediately
            Greater => best = Some(i), // candidate >= key: a better ceiling
            Less => { /* candidate < key: skip (assumed-sorted) */ }
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Scalar-argument coercion helpers (route through `coerce`)
// ---------------------------------------------------------------------------

/// Coerce a scalar-slot argument to a number. A range in a scalar slot
/// implicit-intersects to its top-left, then coerces. Errors propagate.
fn coerce_scalar_number(arg: &Arg) -> Result<f64, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_number(v),
        Arg::Range(rv) => coerce::to_number(&rv.get(0, 0)),
    }
}

/// Coerce a scalar-slot argument to a boolean (the `range_lookup` flag).
fn coerce_scalar_bool(arg: &Arg) -> Result<bool, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_bool(v),
        Arg::Range(rv) => coerce::to_bool(&rv.get(0, 0)),
    }
}

/// First-error-wins over the *scalar* arguments of a slice (errors inside a
/// range are not scanned here, matching [`coerce::first_error`]'s ruling).
fn first_scalar_error(args: &[Arg]) -> Option<CellError> {
    coerce::first_error(args)
}
