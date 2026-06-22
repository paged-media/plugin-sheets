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

//! M1 dynamic-array family kernels (spec §6.4 / §13 M1). Every kernel here
//! is a `returns_array` row: its signature is
//! `fn(&[Arg], &EvalCtx) -> FnResult`, routed through
//! [`crate::dispatch::dispatch_rich`] (the scalar dispatch door returns
//! `#VALUE!` for these rows by construction). A kernel that fails returns
//! [`FnResult::Scalar`]`(`[`CellValue::Error`]`(…))`; a successful kernel
//! returns [`FnResult::Array`] — a row-major `Vec<Vec<CellValue>>` (outer =
//! rows, inner = columns) the spill engine (`sheet-calc`) materializes onto
//! the sheet.
//!
//! ## Excel rulings honored here (each is a registry-tested feature)
//!
//! - **SEQUENCE(rows, [cols], [start], [step])** — column-major *fill* order:
//!   the n-th produced value (reading the output row-major) is
//!   `start + n*step`. `rows`/`cols` truncate toward zero; a non-positive
//!   `rows`/`cols` is `#VALUE!` (Excel surfaces `#CALC!`/`#VALUE!`; we ground
//!   it to `#VALUE!` since the wire enum has no `#CALC!`).
//! - **TRANSPOSE(range)** — `out[c][r] = in[r][c]`; empty input → `#VALUE!`.
//! - **UNIQUE(range, [by_col], [exactly_once])** — first-seen order; equality
//!   is the cross-type [`coerce::compare`] order with TEXT compared
//!   case-insensitively (`"a"` and `"A"` are one value — Excel's UNIQUE ruling;
//!   `coerce::compare`'s total-order byte tie-break is collapsed for membership,
//!   the same repair the `=`/`<>` operators apply in `sheet-calc::eval`). With
//!   `exactly_once = TRUE` only values appearing exactly once survive.
//! - **SORT(range, [sort_index], [order], [by_col])** — stable sort by the
//!   1-based `sort_index` key column (or row when `by_col`); `order` is `1`
//!   ascending (default) / `-1` descending; any other → `#VALUE!`.
//! - **SORTBY(range, by_range, [order], …)** — variadic `(by_range, [order])`
//!   key pairs; every `by_range` must match `range`'s sort length or `#VALUE!`.
//! - **FILTER(range, include, [if_empty])** — `include` is a 1-column or
//!   1-row boolean mask matching `range`'s rows (or cols); kept rows are the
//!   truthy ones; an empty result yields `if_empty` (a 1×1 array) or `#CALC!`
//!   ground-truthed to `#N/A` (Excel returns `#CALC!`; no wire variant).
//! - **TEXTSPLIT(text, col_delim, [row_delim], [ignore_empty], [match_mode],
//!   [pad_with])** — split into a 2-D block; `match_mode = 1` is
//!   case-insensitive; ragged rows pad with `pad_with` (default `#N/A`).
//! - **RANDARRAY([rows], [cols], [min], [max], [integer])** — volatile, draws
//!   from [`EvalCtx::next_f64`] (deterministic under a fixed seed).
//!
//! ## The "ground #CALC! to a wire error" ruling (documented once, here)
//!
//! Modern Excel raises `#CALC!` for an empty dynamic-array result (an empty
//! `FILTER`, a zero-length `SEQUENCE`). The frozen [`CellError`] wire enum has
//! no `#CALC!` variant (it is the 8 OOXML codes). We therefore ground every
//! such case to the nearest classic wire error: a degenerate *shape* (rows/cols
//! ≤ 0) is `#VALUE!`; an empty *filter* with no `if_empty` is `#N/A` (the value
//! Excel itself shows in legacy mode). Each grounding is noted on its kernel.

use compact_str::CompactString;
use sheet_core::{CellError, CellValue};

use crate::arg::{Arg, RangeView};
use crate::coerce;
use crate::ctx::EvalCtx;
use crate::result::FnResult;

// ---- small shared helpers --------------------------------------------------

/// A scalar-or-error shortcut: wrap an error as a scalar `FnResult`.
#[inline]
fn err(e: CellError) -> FnResult {
    FnResult::Scalar(CellValue::Error(e))
}

/// Read a scalar argument's value (a range arg collapses to its top-left
/// cell — the implicit-intersection fallback `sheet-calc` relies on for a
/// 1×1 range). Used for the non-array scalar parameters (counts, deltas).
#[inline]
fn scalar_value(arg: &Arg) -> CellValue {
    match arg {
        Arg::Scalar(v) => v.clone(),
        Arg::Range(r) => r.get(0, 0),
    }
}

/// Coerce one scalar argument to a number (propagating errors / `#VALUE!`).
#[inline]
fn scalar_num(arg: &Arg) -> Result<f64, CellError> {
    coerce::to_number(&scalar_value(arg))
}

/// Coerce one scalar argument to a bool (propagating errors / `#VALUE!`).
#[inline]
fn scalar_bool(arg: &Arg) -> Result<bool, CellError> {
    coerce::to_bool(&scalar_value(arg))
}

/// Snapshot a [`RangeView`] (or a scalar arg) into an owned row-major grid of
/// `CellValue` (outer = rows, inner = columns). A scalar becomes a 1×1 grid.
fn grid_of(arg: &Arg) -> Vec<Vec<CellValue>> {
    match arg {
        Arg::Scalar(v) => vec![vec![v.clone()]],
        Arg::Range(r) => grid_of_view(r),
    }
}

/// Snapshot a [`RangeView`] into an owned row-major grid.
fn grid_of_view(r: &RangeView) -> Vec<Vec<CellValue>> {
    let rows = r.rows();
    let cols = r.cols();
    (0..rows)
        .map(|rr| (0..cols).map(|cc| r.get(rr, cc)).collect())
        .collect()
}

/// First [`CellValue::Error`] anywhere in a grid (row-major scan) — array
/// kernels propagate an error cell in their *source* range (unlike the
/// aggregation skip rule, a structural transform carries the error through).
fn first_error_in_grid(grid: &[Vec<CellValue>]) -> Option<CellError> {
    for row in grid {
        for v in row {
            if let CellValue::Error(e) = v {
                return Some(*e);
            }
        }
    }
    None
}

// ---- SEQUENCE --------------------------------------------------------------

/// `SEQUENCE(rows, [columns], [start], [step])` (spec §6.4). Produces a
/// `rows × cols` block filled row-major with `start + n*step`. `rows`/`cols`
/// truncate toward zero; ≤ 0 → `#VALUE!` (the grounded `#CALC!`). `start`
/// defaults to `1`, `step` to `1`.
pub fn sequence(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let rows = match scalar_num(&args[0]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return err(e),
    };
    let cols = if args.len() >= 2 {
        match scalar_num(&args[1]) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return err(e),
        }
    } else {
        1
    };
    let start = if args.len() >= 3 {
        match scalar_num(&args[2]) {
            Ok(n) => n,
            Err(e) => return err(e),
        }
    } else {
        1.0
    };
    let step = if args.len() >= 4 {
        match scalar_num(&args[3]) {
            Ok(n) => n,
            Err(e) => return err(e),
        }
    } else {
        1.0
    };

    if rows <= 0 || cols <= 0 {
        return err(CellError::Value);
    }

    let mut out = Vec::with_capacity(rows as usize);
    let mut n = 0u64;
    for _ in 0..rows {
        let mut row = Vec::with_capacity(cols as usize);
        for _ in 0..cols {
            row.push(CellValue::Number(start + (n as f64) * step));
            n += 1;
        }
        out.push(row);
    }
    FnResult::Array(out)
}

// ---- TRANSPOSE -------------------------------------------------------------

/// `TRANSPOSE(range)` (ECMA-376 §18.17.7). `out[c][r] = in[r][c]`. An empty
/// input is `#VALUE!`.
pub fn transpose(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let grid = grid_of(&args[0]);
    let rows = grid.len();
    let cols = grid.first().map(|r| r.len()).unwrap_or(0);
    if rows == 0 || cols == 0 {
        return err(CellError::Value);
    }
    let mut out = vec![Vec::with_capacity(rows); cols];
    for row in &grid {
        for (c, v) in row.iter().enumerate() {
            out[c].push(v.clone());
        }
    }
    FnResult::Array(out)
}

// ---- UNIQUE ----------------------------------------------------------------

/// Two values are "the same" for UNIQUE membership (Excel ruling): equal under
/// the frozen cross-type [`coerce::compare`] order, with TEXT compared
/// **case-insensitively** (Excel's UNIQUE treats `"a"` and `"A"` as one value).
/// `coerce::compare` is a *total* order, so it tie-breaks two case-fold-equal
/// texts by raw bytes (making `"a" != "A"`) to stay antisymmetric — correct for
/// sorting, wrong for membership. So we collapse that case-only tie-break to
/// `Equal` here, exactly as the operator layer does in `sheet-calc::eval`.
fn values_equal(a: &CellValue, b: &CellValue) -> bool {
    if let (CellValue::Text(x), CellValue::Text(y)) = (a, b) {
        return x.eq_ignore_ascii_case(y);
    }
    coerce::compare(a, b) == std::cmp::Ordering::Equal
}

/// Whether two rows (slices) are equal element-wise under [`values_equal`].
fn rows_equal(a: &[CellValue], b: &[CellValue]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| values_equal(x, y))
}

/// `UNIQUE(range, [by_col], [exactly_once])` (spec §6.4). Distinct entries in
/// first-seen order. `by_col = FALSE` (default) dedups ROWS; `TRUE` dedups
/// COLUMNS. `exactly_once = TRUE` keeps only entries that appear exactly once.
pub fn unique(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let grid = grid_of(&args[0]);
    if let Some(e) = first_error_in_grid(&grid) {
        return err(e);
    }
    let by_col = if args.len() >= 2 {
        match scalar_bool(&args[1]) {
            Ok(b) => b,
            Err(e) => return err(e),
        }
    } else {
        false
    };
    let exactly_once = if args.len() >= 3 {
        match scalar_bool(&args[2]) {
            Ok(b) => b,
            Err(e) => return err(e),
        }
    } else {
        false
    };

    // Normalize to "list of entries" where an entry is a row (default) or a
    // column (by_col); dedup, optionally keep exactly-once; then re-orient.
    let entries: Vec<Vec<CellValue>> = if by_col {
        transpose_grid(&grid)
    } else {
        grid.clone()
    };
    if entries.is_empty() || entries[0].is_empty() {
        return err(CellError::Value);
    }

    // First-seen distinct entries with occurrence counts.
    let mut distinct: Vec<Vec<CellValue>> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for e in &entries {
        match distinct.iter().position(|d| rows_equal(d, e)) {
            Some(i) => counts[i] += 1,
            None => {
                distinct.push(e.clone());
                counts.push(1);
            }
        }
    }

    let kept: Vec<Vec<CellValue>> = distinct
        .into_iter()
        .zip(counts)
        .filter(|(_, c)| !exactly_once || *c == 1)
        .map(|(d, _)| d)
        .collect();

    if kept.is_empty() {
        // No entry survived exactly-once → the grounded #CALC! is #N/A.
        return err(CellError::Na);
    }

    FnResult::Array(if by_col { transpose_grid(&kept) } else { kept })
}

/// Plain grid transpose helper (rows↔cols) for the by-col orientation.
fn transpose_grid(grid: &[Vec<CellValue>]) -> Vec<Vec<CellValue>> {
    let rows = grid.len();
    let cols = grid.first().map(|r| r.len()).unwrap_or(0);
    let mut out = vec![Vec::with_capacity(rows); cols];
    for row in grid {
        for (c, v) in row.iter().enumerate() {
            out[c].push(v.clone());
        }
    }
    out
}

// ---- SORT ------------------------------------------------------------------

/// `SORT(range, [sort_index], [sort_order], [by_col])` (spec §6.4). Stable
/// sort of the rows (or columns when `by_col`) by the 1-based key
/// `sort_index`. `sort_order` is `1` ascending (default) or `-1` descending;
/// any other value → `#VALUE!`.
pub fn sort(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let grid = grid_of(&args[0]);
    if let Some(e) = first_error_in_grid(&grid) {
        return err(e);
    }
    let sort_index = if args.len() >= 2 {
        match scalar_num(&args[1]) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return err(e),
        }
    } else {
        1
    };
    let order = if args.len() >= 3 {
        match scalar_num(&args[2]) {
            Ok(n) => n,
            Err(e) => return err(e),
        }
    } else {
        1.0
    };
    let by_col = if args.len() >= 4 {
        match scalar_bool(&args[3]) {
            Ok(b) => b,
            Err(e) => return err(e),
        }
    } else {
        false
    };
    let descending = match order {
        1.0 => false,
        -1.0 => true,
        _ => return err(CellError::Value),
    };

    // Orient to "list of entries by their sort key".
    let mut entries = if by_col {
        transpose_grid(&grid)
    } else {
        grid.clone()
    };
    if entries.is_empty() || entries[0].is_empty() {
        return err(CellError::Value);
    }
    let key = sort_index - 1;
    if key < 0 || key as usize >= entries[0].len() {
        return err(CellError::Value);
    }
    let key = key as usize;

    // Stable sort by the key column under the cross-type total order.
    entries.sort_by(|a, b| {
        let ord = coerce::compare(&a[key], &b[key]);
        if descending {
            ord.reverse()
        } else {
            ord
        }
    });

    FnResult::Array(if by_col {
        transpose_grid(&entries)
    } else {
        entries
    })
}

// ---- SORTBY ----------------------------------------------------------------

/// `SORTBY(range, by_array1, [sort_order1], [by_array2, sort_order2], …)`
/// (spec §6.4). Sorts the rows of `range` by one or more parallel
/// `by_array` key vectors (each a column or row of the SAME length as
/// `range`'s rows). `sort_order` is `1`/`-1` (default `1`). A length mismatch
/// or bad order → `#VALUE!`. The sort is stable and lexicographic across keys
/// (first key dominates).
pub fn sortby(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let grid = grid_of(&args[0]);
    if let Some(e) = first_error_in_grid(&grid) {
        return err(e);
    }
    let n = grid.len();
    if n == 0 || grid[0].is_empty() {
        return err(CellError::Value);
    }

    // Parse the variadic (by_array, [order]) tail. Each by_array is flattened
    // to a length-n key vector; the trailing order is optional (default +1).
    let mut keys: Vec<(Vec<CellValue>, bool)> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let by = flatten_to_vec(&args[i]);
        if let Some(e) = first_error_in_grid(std::slice::from_ref(&by)) {
            return err(e);
        }
        if by.len() != n {
            return err(CellError::Value);
        }
        let descending = if i + 1 < args.len() {
            // The next arg is this key's sort order.
            match scalar_num(&args[i + 1]) {
                Ok(1.0) => false,
                Ok(-1.0) => true,
                Ok(_) => return err(CellError::Value),
                Err(e) => return err(e),
            }
        } else {
            false
        };
        keys.push((by, descending));
        i += 2;
    }
    if keys.is_empty() {
        return err(CellError::Value);
    }

    // Stable sort the row indices lexicographically across the key vectors.
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        for (vec, desc) in &keys {
            let ord = coerce::compare(&vec[a], &vec[b]);
            let ord = if *desc { ord.reverse() } else { ord };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });

    let out: Vec<Vec<CellValue>> = idx.into_iter().map(|r| grid[r].clone()).collect();
    FnResult::Array(out)
}

/// Flatten an argument (scalar / row / column / block) to a single row-major
/// vector — the form a `by_array` key needs.
fn flatten_to_vec(arg: &Arg) -> Vec<CellValue> {
    match arg {
        Arg::Scalar(v) => vec![v.clone()],
        Arg::Range(r) => r.iter().collect(),
    }
}

// ---- FILTER ----------------------------------------------------------------

/// `FILTER(range, include, [if_empty])` (spec §6.4). Keeps the rows of
/// `range` whose aligned `include` mask entry is truthy. `include` is a
/// 1-column or 1-row boolean vector matching `range`'s row count (or column
/// count for a horizontal mask). An empty result returns `if_empty` (as a
/// 1×1 block) or, if absent, `#N/A` (the grounded `#CALC!`). A mask-length
/// mismatch → `#VALUE!`.
pub fn filter(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let grid = grid_of(&args[0]);
    if let Some(e) = first_error_in_grid(&grid) {
        return err(e);
    }
    let rows = grid.len();
    let cols = grid.first().map(|r| r.len()).unwrap_or(0);
    if rows == 0 || cols == 0 {
        return err(CellError::Value);
    }

    let mask = flatten_to_vec(&args[1]);
    if let Some(e) = first_error_in_grid(std::slice::from_ref(&mask)) {
        return err(e);
    }

    // Decide the filter axis from the mask length: a column mask (len == rows)
    // filters rows; a row mask (len == cols) filters columns. When the grid is
    // square, prefer the row-filter reading (Excel's vertical default).
    let kept: Vec<Vec<CellValue>> = if mask.len() == rows {
        grid.iter()
            .zip(&mask)
            .filter(|(_, m)| is_truthy(m))
            .map(|(r, _)| r.clone())
            .collect()
    } else if mask.len() == cols {
        // Filter columns: keep the c-th column of every row where mask[c].
        let keep_cols: Vec<usize> = (0..cols).filter(|&c| is_truthy(&mask[c])).collect();
        if keep_cols.is_empty() {
            Vec::new()
        } else {
            grid.iter()
                .map(|r| keep_cols.iter().map(|&c| r[c].clone()).collect())
                .collect()
        }
    } else {
        return err(CellError::Value);
    };

    if kept.is_empty() {
        return if args.len() >= 3 {
            FnResult::Array(vec![vec![scalar_value(&args[2])]])
        } else {
            err(CellError::Na)
        };
    }

    FnResult::Array(kept)
}

/// Truthiness of a mask cell: a number ≠ 0, a `TRUE` bool, or numeric/bool
/// text → keep. A blank, `FALSE`, `0`, or un-coercible text → drop. (FILTER's
/// `include` mask is conventionally a `0`/`1` or boolean array; we coerce via
/// [`coerce::to_bool`] but treat a NUMBER directly so a `1`/`0` array works.)
fn is_truthy(v: &CellValue) -> bool {
    match v {
        CellValue::Number(n) => *n != 0.0,
        CellValue::Bool(b) => *b,
        CellValue::Empty => false,
        _ => coerce::to_bool(v).unwrap_or(false),
    }
}

// ---- TEXTSPLIT -------------------------------------------------------------

/// `TEXTSPLIT(text, col_delimiter, [row_delimiter], [ignore_empty],
/// [match_mode], [pad_with])` (spec §6.4). Splits `text` into a 2-D block:
/// `row_delimiter` cuts rows, `col_delimiter` cuts columns within each row.
/// `ignore_empty = TRUE` drops empty produced fields; `match_mode = 1` makes
/// delimiter matching case-insensitive; ragged rows are padded with
/// `pad_with` (default `#N/A`). An empty delimiter is `#VALUE!`.
pub fn textsplit(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let text = match &scalar_value(&args[0]) {
        CellValue::Error(e) => return err(*e),
        v => coerce::to_text(v).to_string(),
    };
    let col_delim = match &scalar_value(&args[1]) {
        CellValue::Error(e) => return err(*e),
        v => coerce::to_text(v).to_string(),
    };
    if col_delim.is_empty() {
        return err(CellError::Value);
    }
    let row_delim = if args.len() >= 3 {
        match &scalar_value(&args[2]) {
            CellValue::Error(e) => return err(*e),
            CellValue::Empty => String::new(),
            v => coerce::to_text(v).to_string(),
        }
    } else {
        String::new()
    };
    let ignore_empty = if args.len() >= 4 {
        match scalar_bool(&args[3]) {
            Ok(b) => b,
            Err(e) => return err(e),
        }
    } else {
        false
    };
    let case_insensitive = if args.len() >= 5 {
        match scalar_num(&args[4]) {
            Ok(n) => n.trunc() as i64 == 1,
            Err(e) => return err(e),
        }
    } else {
        false
    };
    let pad_with = if args.len() >= 6 {
        scalar_value(&args[5])
    } else {
        CellValue::Error(CellError::Na)
    };

    // Cut into rows first (by row_delim, when present), then each row into
    // columns (by col_delim).
    let raw_rows: Vec<&str> = if row_delim.is_empty() {
        vec![text.as_str()]
    } else {
        split_on(&text, &row_delim, case_insensitive)
    };

    let mut grid: Vec<Vec<CellValue>> = Vec::with_capacity(raw_rows.len());
    for rr in raw_rows {
        let fields: Vec<&str> = split_on(rr, &col_delim, case_insensitive);
        let mut row: Vec<CellValue> = Vec::with_capacity(fields.len());
        for f in fields {
            if ignore_empty && f.is_empty() {
                continue;
            }
            row.push(CellValue::Text(CompactString::new(f)));
        }
        // When ignore_empty drops a whole row to nothing, skip it entirely.
        if ignore_empty && row.is_empty() {
            continue;
        }
        grid.push(row);
    }

    if grid.is_empty() {
        return err(CellError::Value);
    }

    // Pad ragged rows to the widest row with `pad_with`.
    let width = grid.iter().map(|r| r.len()).max().unwrap_or(0);
    for row in &mut grid {
        while row.len() < width {
            row.push(pad_with.clone());
        }
    }

    FnResult::Array(grid)
}

/// Split `s` on every occurrence of `delim` (non-empty). Case-insensitive
/// matching folds ASCII when `ci`. Returns the fields including empty ones
/// (the caller applies `ignore_empty`).
fn split_on<'a>(s: &'a str, delim: &str, ci: bool) -> Vec<&'a str> {
    if !ci {
        return s.split(delim).collect();
    }
    // Case-insensitive: scan for ASCII-folded matches, slicing at byte
    // boundaries (the delimiter is matched byte-for-byte after folding, which
    // is correct for ASCII delimiters — the common publishing case).
    let mut out = Vec::new();
    let dl = delim.len();
    let hay = s.as_bytes();
    let needle = delim.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i + dl <= hay.len() {
        if hay[i..i + dl].eq_ignore_ascii_case(needle) {
            out.push(&s[start..i]);
            i += dl;
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&s[start..]);
    out
}

// ---- RANDARRAY -------------------------------------------------------------

/// `RANDARRAY([rows], [columns], [min], [max], [integer])` (spec §6.4).
/// Volatile: every value draws from [`EvalCtx::next_f64`] (deterministic under
/// a fixed seed, the property the conformance suite relies on). Defaults:
/// `rows = 1`, `cols = 1`, `min = 0`, `max = 1`, `integer = FALSE`. With
/// `integer = TRUE` the result is an inclusive integer in `[min, max]`;
/// `min > max` → `#VALUE!`.
pub fn randarray(args: &[Arg], ctx: &EvalCtx) -> FnResult {
    let rows = if !args.is_empty() {
        match scalar_num(&args[0]) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return err(e),
        }
    } else {
        1
    };
    let cols = if args.len() >= 2 {
        match scalar_num(&args[1]) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return err(e),
        }
    } else {
        1
    };
    let min = if args.len() >= 3 {
        match scalar_num(&args[2]) {
            Ok(n) => n,
            Err(e) => return err(e),
        }
    } else {
        0.0
    };
    let max = if args.len() >= 4 {
        match scalar_num(&args[3]) {
            Ok(n) => n,
            Err(e) => return err(e),
        }
    } else {
        1.0
    };
    let integer = if args.len() >= 5 {
        match scalar_bool(&args[4]) {
            Ok(b) => b,
            Err(e) => return err(e),
        }
    } else {
        false
    };

    if rows <= 0 || cols <= 0 || min > max {
        return err(CellError::Value);
    }

    let mut out = Vec::with_capacity(rows as usize);
    for _ in 0..rows {
        let mut row = Vec::with_capacity(cols as usize);
        for _ in 0..cols {
            let u = ctx.next_f64();
            let v = if integer {
                // Inclusive integer in [min, max]: scale the unit draw across
                // the (max-min+1) integer span and floor.
                let span = (max.trunc() - min.trunc() + 1.0).max(1.0);
                (min.trunc() + (u * span).floor()).min(max.trunc())
            } else {
                min + u * (max - min)
            };
            row.push(CellValue::Number(v));
        }
        out.push(row);
    }
    FnResult::Array(out)
}
