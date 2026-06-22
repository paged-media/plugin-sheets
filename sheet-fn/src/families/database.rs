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

//! The database (D-) function family (spec §7, §11 T2; ECMA-376 §18.17.7).
//! Twelve range-aware aggregators that distil a *list* (Excel's term for a
//! header-topped table) down to one value over the rows matching a criteria
//! table: `DSUM`, `DCOUNT`, `DCOUNTA`, `DGET`, `DMAX`, `DMIN`, `DAVERAGE`,
//! `DPRODUCT`, `DSTDEV`, `DSTDEVP`, `DVAR`, `DVARP`.
//!
//! Every kernel is the frozen pure signature
//! `fn(&[Arg], &EvalCtx) -> CellValue`; arity (exactly 3) is range-checked by
//! the generated [`crate::dispatch`] before these are called. All three
//! arguments are *ranges* — the registry marks the family `range_aware: true`,
//! so the evaluator materializes each into a [`RangeView`]. A scalar in any
//! slot is read as a one-cell window (defensively, so a 1×1 range or a bare
//! literal both work).
//!
//! ## The shared signature `D-fn(database, field, criteria)`
//!
//! - **`database`** — a rectangular range whose **first row is the column
//!   headers** and whose remaining rows are the records. An empty database
//!   (header row only) contributes no records.
//! - **`field`** — selects which database column to aggregate. Excel accepts
//!   three spellings, all supported here:
//!   1. a **1-based column number** (`1` = the first database column) — a
//!      number `< 1` or `>` the column count is `#VALUE!`;
//!   2. a **header label** (text) matched **case-insensitively** against the
//!      database header row — no match is `#VALUE!`;
//!   3. a **cell reference to a header** — which arrives as a 1×1 range and is
//!      read through (1) or (2) on its single value.
//!
//!   A blank/error field is `#VALUE!`.
//! - **`criteria`** — a range whose **first row is headers** (a subset of the
//!   database headers) and whose following rows are condition rows. The
//!   database-criteria grammar (ECMA-376 §18.17.7):
//!   - cells **within one criteria row are AND-ed** (a record must satisfy
//!     every non-blank cell in that row);
//!   - **criteria rows are OR-ed** (a record matches if it satisfies *any*
//!     row);
//!   - a **blank criteria cell imposes no constraint** (always satisfied),
//!     and a criteria row that is **entirely blank matches every record** —
//!     the Excel idiom for "no filter";
//!   - each non-blank criteria cell is matched against the record's value in
//!     the column named by that cell's **criteria header** using the shared
//!     [`crate::criteria`] ruling (operator prefixes `>`,`<`,`>=`,`<=`,`<>`,
//!     `=`; bare number↔text equality; case-insensitive `*`/`?` wildcards);
//!   - a criteria header that names **no database column** is ignored (its
//!     cells impose no constraint) — Excel treats an unrecognised label as a
//!     non-list (computed) field; T0 does not evaluate computed criteria, so
//!     we conservatively drop it rather than fail the whole call.
//!
//! A criteria range that is only its header row (no condition rows) matches
//! **every** record (Excel: an empty criteria table is "all rows").
//!
//! ## Aggregation rulings (mirror the agg/stat families)
//!
//! Over the selected field of the matching records:
//! - `DSUM` sums numeric cells; `DPRODUCT` multiplies them.
//! - `DCOUNT` counts **numeric** field cells; `DCOUNTA` counts **non-blank**
//!   field cells (the COUNT/COUNTA split of the agg family).
//! - `DMAX`/`DMIN` take the extreme numeric field cell — **of no numbers, `0`**
//!   (Excel, like `MAX`/`MIN`).
//! - `DAVERAGE` is the mean — **of no numbers, `#DIV/0!`** (Excel).
//! - `DSTDEV`/`DVAR` are **sample** (`n−1`); `DSTDEVP`/`DVARP` are
//!   **population** (`n`). Not enough numbers (`n ≤ 1` sample, `n = 0`
//!   population) is `#DIV/0!` — the stat-family ruling.
//! - `DGET` returns the single matching field value: **no match → `#VALUE!`,
//!   more than one match → `#NUM!`** (ECMA-376 §18.17.7).
//!
//! Non-numeric field cells (text/bool/blank) are **skipped** by the numeric
//! aggregators (the agg-family range-skip ruling); an [`CellValue::Error`] in
//! a contributing field cell **propagates** (first error wins, scan order
//! top-to-bottom). A malformed shape (empty database, field out of range)
//! short-circuits to the documented error before any scan.

use sheet_core::{CellError, CellValue};

use crate::arg::{Arg, RangeView};
use crate::coerce;
use crate::criteria;
use crate::num::{Numeric, F64};

// ---- shape extraction -------------------------------------------------------

/// A materialized view of one of the three range arguments, normalized so a
/// scalar arrives as a 1×1 grid. Reads go through [`Grid::get`] with
/// **relative** `(row, col)`; out-of-bounds yields [`CellValue::Empty`].
enum Grid<'a> {
    /// A real range argument.
    Range(&'a RangeView<'a>),
    /// A scalar argument, surfaced as a single cell.
    Scalar(&'a CellValue),
}

impl Grid<'_> {
    fn rows(&self) -> u32 {
        match self {
            Grid::Range(v) => v.rows(),
            Grid::Scalar(_) => 1,
        }
    }

    fn cols(&self) -> u32 {
        match self {
            Grid::Range(v) => v.cols(),
            Grid::Scalar(_) => 1,
        }
    }

    fn get(&self, r: u32, c: u32) -> CellValue {
        match self {
            Grid::Range(v) => v.get(r, c),
            Grid::Scalar(v) => {
                if r == 0 && c == 0 {
                    (*v).clone()
                } else {
                    CellValue::Empty
                }
            }
        }
    }
}

/// Borrow an [`Arg`] as a [`Grid`] (range → grid, scalar → 1×1 cell).
fn grid<'a>(arg: &'a Arg<'a>) -> Grid<'a> {
    match arg {
        Arg::Range(v) => Grid::Range(v),
        Arg::Scalar(v) => Grid::Scalar(v),
    }
}

/// Case-insensitive header equality (the field/criteria-header match ruling).
/// Reuses the [`coerce::to_text`] General projection so a numeric header
/// (`2024`) still matches the textual criteria header `"2024"`.
fn header_eq(a: &CellValue, b: &CellValue) -> bool {
    coerce::to_text(a).eq_ignore_ascii_case(coerce::to_text(b).as_str())
}

/// Resolve the `field` argument to a **0-based database column index**.
///
/// `Err` carries the Excel error for an unresolvable field (`#VALUE!`).
/// `db_cols` is the database width; `headers(c)` reads database header `c`.
fn resolve_field(
    field: &Grid,
    db_cols: u32,
    headers: impl Fn(u32) -> CellValue,
) -> Result<u32, CellError> {
    let v = field.get(0, 0);
    match &v {
        // A blank or error field never resolves.
        CellValue::Empty => Err(CellError::Value),
        CellValue::Error(e) => Err(*e),
        // A numeric field is a 1-based column index.
        CellValue::Number(n) => index_from_number(*n, db_cols),
        // A boolean is not a valid field selector in Excel.
        CellValue::Bool(_) => Err(CellError::Value),
        // Text: first try a header-label match, then a numeric spelling.
        CellValue::Text(_) => {
            for c in 0..db_cols {
                if header_eq(&headers(c), &v) {
                    return Ok(c);
                }
            }
            // Excel also accepts a numeric STRING ("2") as a 1-based index.
            match coerce::to_number(&v) {
                Ok(n) => index_from_number(n, db_cols),
                Err(_) => Err(CellError::Value),
            }
        }
    }
}

/// Convert a 1-based column number to a 0-based index, bounds-checking against
/// `db_cols`. Truncates toward zero (Excel reads `2.9` as column 2).
fn index_from_number(n: f64, db_cols: u32) -> Result<u32, CellError> {
    let idx = n.trunc();
    if idx < 1.0 || idx > db_cols as f64 {
        return Err(CellError::Value);
    }
    Ok(idx as u32 - 1)
}

/// For each database **record** (rows `1..db.rows()`), test it against the
/// criteria table and, if it matches, hand the selected field's value to `f`.
///
/// Returns `Some(error)` as soon as a contributing field cell is an error
/// (first error wins). The empty-database (`rows <= 1`) and degenerate-grid
/// cases simply produce no callbacks.
fn for_each_matching(
    db: &Grid,
    field_col: u32,
    crit: &Grid,
    mut f: impl FnMut(CellValue) -> Option<CellError>,
) -> Option<CellError> {
    let db_rows = db.rows();
    let db_cols = db.cols();
    if db_rows <= 1 || db_cols == 0 {
        return None; // header-only / empty database: no records.
    }

    // Compile the criteria table once: for each criteria column, the database
    // column it targets (by header match) and the per-row compiled criterion.
    let crit_rows = crit.rows();
    let crit_cols = crit.cols();

    // Map each criteria column -> Some(db column index) by header match, or
    // None when the criteria header names no database column (ignored).
    let mut crit_to_db: Vec<Option<u32>> = Vec::with_capacity(crit_cols as usize);
    for cc in 0..crit_cols {
        let ch = crit.get(0, cc);
        // A blank criteria header column carries no constraint.
        if ch.is_blank() {
            crit_to_db.push(None);
            continue;
        }
        let mut found = None;
        for dc in 0..db_cols {
            if header_eq(&db.get(0, dc), &ch) {
                found = Some(dc);
                break;
            }
        }
        crit_to_db.push(found);
    }

    for dr in 1..db_rows {
        if record_matches(db, dr, crit, crit_rows, crit_cols, &crit_to_db) {
            let cell = db.get(dr, field_col);
            if let Some(e) = f(cell) {
                return Some(e);
            }
        }
    }
    None
}

/// Does database record `dr` satisfy the criteria table?
///
/// AND within a criteria row, OR across criteria rows; a fully-blank criteria
/// row (or a header-only criteria table) matches everything.
fn record_matches(
    db: &Grid,
    dr: u32,
    crit: &Grid,
    crit_rows: u32,
    crit_cols: u32,
    crit_to_db: &[Option<u32>],
) -> bool {
    // Header-only criteria table (no condition rows) matches every record.
    if crit_rows <= 1 {
        return true;
    }
    // OR across the condition rows.
    for cr in 1..crit_rows {
        if row_matches(db, dr, crit, cr, crit_cols, crit_to_db) {
            return true;
        }
    }
    false
}

/// Does record `dr` satisfy criteria row `cr` (AND across its non-blank
/// cells)? An entirely-blank criteria row vacuously matches (no constraints).
fn row_matches(
    db: &Grid,
    dr: u32,
    crit: &Grid,
    cr: u32,
    crit_cols: u32,
    crit_to_db: &[Option<u32>],
) -> bool {
    for cc in 0..crit_cols {
        let cell = crit.get(cr, cc);
        // A blank criteria cell imposes no constraint.
        if cell.is_blank() {
            continue;
        }
        // A constraint whose header names no database column is ignored.
        let Some(Some(db_col)) = crit_to_db.get(cc as usize).copied() else {
            continue;
        };
        let compiled = criteria::parse_criteria(&cell);
        let candidate = db.get(dr, db_col);
        if !criteria::matches(&compiled, &candidate) {
            return false; // AND: one failed cell fails the row.
        }
    }
    true
}

// ---- the numeric / counting collectors -------------------------------------

/// Collect the **numeric** field values of the matching records into `acc`
/// (text/bool/blank field cells skipped). An error in a matching field cell
/// propagates (returned as `Some(e)`).
fn collect_numbers(
    db: &Grid,
    field_col: u32,
    crit: &Grid,
    acc: &mut Vec<f64>,
) -> Option<CellError> {
    for_each_matching(db, field_col, crit, |cell| match cell {
        CellValue::Number(n) => {
            acc.push(n);
            None
        }
        CellValue::Error(e) => Some(e),
        // Text / Bool / Empty field cells are skipped.
        _ => None,
    })
}

/// The common entry point: normalize the three args to grids, resolve the
/// field column, then run `aggregate` over the collected numeric field values.
/// `aggregate` decides the empty-set behavior (e.g. `MAX`→`0`, `AVERAGE`→
/// `#DIV/0!`), so it receives the full `Vec<f64>`.
fn numeric_aggregate(args: &[Arg], aggregate: impl Fn(&[f64]) -> CellValue) -> CellValue {
    let (Some(db_arg), Some(field_arg), Some(crit_arg)) = (args.first(), args.get(1), args.get(2))
    else {
        // The generated arity guard enforces exactly 3; this stays total.
        return CellValue::Error(CellError::Value);
    };
    let db = grid(db_arg);
    let field = grid(field_arg);
    let crit = grid(crit_arg);

    let field_col = match resolve_field(&field, db.cols(), |c| db.get(0, c)) {
        Ok(c) => c,
        Err(e) => return CellValue::Error(e),
    };

    let mut nums = Vec::new();
    if let Some(e) = collect_numbers(&db, field_col, &crit, &mut nums) {
        return CellValue::Error(e);
    }
    aggregate(&nums)
}

// ---- the twelve kernels -----------------------------------------------------

/// `DSUM(database, field, criteria)` — sum of the field over matching records.
/// Empty matching set sums to `0` (Excel).
pub fn dsum(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    numeric_aggregate(args, |nums| {
        let mut sum = F64::from_f64(0.0);
        for &n in nums {
            sum = sum.add(F64::from_f64(n));
        }
        CellValue::Number(sum.to_f64())
    })
}

/// `DPRODUCT(database, field, criteria)` — product of the field over matching
/// records. Empty matching set yields `0` (Excel: DPRODUCT of no values).
pub fn dproduct(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    numeric_aggregate(args, |nums| {
        if nums.is_empty() {
            return CellValue::Number(0.0);
        }
        let mut prod = F64::from_f64(1.0);
        for &n in nums {
            prod = prod.mul(F64::from_f64(n));
        }
        CellValue::Number(prod.to_f64())
    })
}

/// `DMAX(database, field, criteria)` — largest field value over matching
/// records; **of no numbers, `0`** (Excel, like `MAX`).
pub fn dmax(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    numeric_aggregate(args, |nums| {
        let m = nums.iter().copied().fold(None, |acc: Option<f64>, n| {
            Some(acc.map_or(n, |a| a.max(n)))
        });
        CellValue::Number(m.unwrap_or(0.0))
    })
}

/// `DMIN(database, field, criteria)` — smallest field value over matching
/// records; **of no numbers, `0`** (Excel, like `MIN`).
pub fn dmin(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    numeric_aggregate(args, |nums| {
        let m = nums.iter().copied().fold(None, |acc: Option<f64>, n| {
            Some(acc.map_or(n, |a| a.min(n)))
        });
        CellValue::Number(m.unwrap_or(0.0))
    })
}

/// `DAVERAGE(database, field, criteria)` — mean of the field over matching
/// records; **of no numbers, `#DIV/0!`** (Excel).
pub fn daverage(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    numeric_aggregate(args, |nums| {
        if nums.is_empty() {
            return CellValue::Error(CellError::Div0);
        }
        let mut sum = F64::from_f64(0.0);
        for &n in nums {
            sum = sum.add(F64::from_f64(n));
        }
        let mean = sum.div(F64::from_f64(nums.len() as f64));
        CellValue::Number(mean.to_f64())
    })
}

/// `DCOUNT(database, field, criteria)` — count of **numeric** field cells over
/// matching records. Total; never errors (an error field cell simply is not a
/// number, so it is not counted — DCOUNT mirrors `COUNT`, which does not
/// propagate range errors). Counts the matching numeric cells via the same
/// scan, ignoring an error cell rather than propagating it.
pub fn dcount(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let (Some(db_arg), Some(field_arg), Some(crit_arg)) = (args.first(), args.get(1), args.get(2))
    else {
        return CellValue::Error(CellError::Value);
    };
    let db = grid(db_arg);
    let field = grid(field_arg);
    let crit = grid(crit_arg);

    let field_col = match resolve_field(&field, db.cols(), |c| db.get(0, c)) {
        Ok(c) => c,
        Err(e) => return CellValue::Error(e),
    };

    let mut n = 0u64;
    // DCOUNT never propagates a field error: count numeric cells, ignore the
    // rest (mirrors COUNT's no-propagation ruling). The collector's error
    // short-circuit is bypassed by always returning None.
    for_each_matching(&db, field_col, &crit, |cell| {
        if matches!(cell, CellValue::Number(_)) {
            n += 1;
        }
        None
    });
    CellValue::Number(n as f64)
}

/// `DCOUNTA(database, field, criteria)` — count of **non-blank** field cells
/// over matching records (text/number/bool/error all count; only a truly
/// blank field cell is skipped). Total; never errors.
pub fn dcounta(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let (Some(db_arg), Some(field_arg), Some(crit_arg)) = (args.first(), args.get(1), args.get(2))
    else {
        return CellValue::Error(CellError::Value);
    };
    let db = grid(db_arg);
    let field = grid(field_arg);
    let crit = grid(crit_arg);

    let field_col = match resolve_field(&field, db.cols(), |c| db.get(0, c)) {
        Ok(c) => c,
        Err(e) => return CellValue::Error(e),
    };

    let mut n = 0u64;
    for_each_matching(&db, field_col, &crit, |cell| {
        if !cell.is_blank() {
            n += 1;
        }
        None
    });
    CellValue::Number(n as f64)
}

/// `DGET(database, field, criteria)` — the single field value of the one
/// matching record. **No match → `#VALUE!`; more than one match → `#NUM!`**
/// (ECMA-376 §18.17.7). Returns the matching field cell verbatim (its own type
/// — text, number, bool, or even an error cell — is passed through).
pub fn dget(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let (Some(db_arg), Some(field_arg), Some(crit_arg)) = (args.first(), args.get(1), args.get(2))
    else {
        return CellValue::Error(CellError::Value);
    };
    let db = grid(db_arg);
    let field = grid(field_arg);
    let crit = grid(crit_arg);

    let field_col = match resolve_field(&field, db.cols(), |c| db.get(0, c)) {
        Ok(c) => c,
        Err(e) => return CellValue::Error(e),
    };

    let mut found: Option<CellValue> = None;
    let mut count = 0u64;
    for_each_matching(&db, field_col, &crit, |cell| {
        count += 1;
        if found.is_none() {
            found = Some(cell);
        }
        None
    });

    match count {
        0 => CellValue::Error(CellError::Value),
        1 => found.unwrap_or(CellValue::Empty),
        _ => CellValue::Error(CellError::Num),
    }
}

/// Sum of squared deviations from the mean (`Σ(xᵢ − x̄)²`). `values`
/// non-empty. Shared by [`d_variance`].
fn sum_sq_dev(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    let mut sum = F64::from_f64(0.0);
    for &v in values {
        sum = sum.add(F64::from_f64(v));
    }
    let mean = sum.to_f64() / n;
    let mut acc = F64::from_f64(0.0);
    for &v in values {
        let d = F64::from_f64(v - mean);
        acc = acc.add(d.mul(d));
    }
    acc.to_f64()
}

/// The shared variance over the matching field numbers. `population` selects
/// the `n` (population) vs `n−1` (sample) divisor. Not enough numbers
/// (`n = 0` population, `n ≤ 1` sample) is `#DIV/0!` (the stat-family ruling).
fn d_variance(args: &[Arg], population: bool) -> CellValue {
    numeric_aggregate(args, move |nums| {
        let n = nums.len();
        let denom = if population { n } else { n.saturating_sub(1) };
        if denom == 0 {
            return CellValue::Error(CellError::Div0);
        }
        CellValue::Number(sum_sq_dev(nums) / denom as f64)
    })
}

/// `DSTDEV(database, field, criteria)` — **sample** standard deviation
/// (`n−1`) of the field over matching records.
pub fn dstdev(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    match d_variance(args, false) {
        CellValue::Number(v) => CellValue::Number(v.sqrt()),
        other => other,
    }
}

/// `DSTDEVP(database, field, criteria)` — **population** standard deviation
/// (`n`) of the field over matching records.
pub fn dstdevp(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    match d_variance(args, true) {
        CellValue::Number(v) => CellValue::Number(v.sqrt()),
        other => other,
    }
}

/// `DVAR(database, field, criteria)` — **sample** variance (`n−1`) of the
/// field over matching records.
pub fn dvar(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    d_variance(args, false)
}

/// `DVARP(database, field, criteria)` — **population** variance (`n`) of the
/// field over matching records.
pub fn dvarp(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    d_variance(args, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::CellRef;

    fn cr(row: u32, col: u32) -> CellRef {
        CellRef {
            sheet: 0,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn ctx() -> crate::ctx::EvalCtx {
        crate::ctx::EvalCtx::new(sheet_core::DateSystem::Date1900, cr(0, 0), 0.0, 1)
    }

    fn num(n: f64) -> CellValue {
        CellValue::Number(n)
    }
    fn txt(s: &str) -> CellValue {
        CellValue::from(s)
    }

    /// Build a RangeView over a row-major buffer (origin A1).
    fn view(rows: u32, cols: u32, cells: &[CellValue]) -> RangeView<'_> {
        RangeView::from_slice(cr(0, 0), rows, cols, cells)
    }

    /// A 2-column database: header (Name, Age) + 3 records.
    fn sample_db() -> Vec<CellValue> {
        vec![
            txt("Name"),
            txt("Age"),
            txt("ann"),
            num(30.0),
            txt("bob"),
            num(40.0),
            txt("cy"),
            num(50.0),
        ]
    }

    #[test]
    fn dsum_by_header_and_number_agree() {
        let db = sample_db();
        let crit = vec![txt("Age"), txt(">35")];
        let db_v = view(4, 2, &db);
        let crit_v = view(2, 1, &crit);

        // field by header label "Age" -> 40 + 50 = 90.
        let by_label = dsum(
            &[
                Arg::Range(view(4, 2, &db)),
                Arg::Scalar(txt("Age")),
                Arg::Range(view(2, 1, &crit)),
            ],
            &ctx(),
        );
        assert_eq!(by_label, num(90.0));

        // field by 1-based number 2 -> same answer.
        let by_number = dsum(
            &[Arg::Range(db_v), Arg::Scalar(num(2.0)), Arg::Range(crit_v)],
            &ctx(),
        );
        assert_eq!(by_number, num(90.0));
    }

    #[test]
    fn field_out_of_range_is_value() {
        let db = sample_db();
        let crit = vec![txt("Age"), CellValue::Empty];
        let out = dsum(
            &[
                Arg::Range(view(4, 2, &db)),
                Arg::Scalar(num(5.0)),
                Arg::Range(view(2, 1, &crit)),
            ],
            &ctx(),
        );
        assert_eq!(out, CellValue::Error(CellError::Value));
    }
}
