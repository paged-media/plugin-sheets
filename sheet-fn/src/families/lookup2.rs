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

//! M1 lookup/reference (T1) family kernels (spec §7, §11 T1 — "full lookup
//! incl. XLOOKUP/INDEX/MATCH"). Pure `fn(&[Arg], &EvalCtx) -> CellValue`
//! kernels named by the registry `rust` field. Reuses the T0 cross-type total
//! order ([`crate::coerce::compare`]) and the wildcard matcher
//! ([`crate::criteria::Matcher`]) so the lookup rulings are stated once.
//!
//! ## Implemented (no model access — fully pure kernels)
//!
//! - [`xlookup`] / [`xmatch`] — the modern lookup pair. `match_mode` 0 exact
//!   (default), -1 exact-or-next-smaller, 1 exact-or-next-larger, 2 wildcard
//!   (reuses [`crate::criteria::Matcher`]). `search_mode` 1 first-to-last
//!   (default), -1 last-to-first, 2 binary-ascending, -2 binary-descending.
//!   `lookup_array` and `return_array` must align (same length, single row or
//!   single column) else `#VALUE!`; a not-found is `if_not_found`
//!   (`XLOOKUP`) or `#N/A` (`XMATCH`).
//! - [`address`] — builds the A1 (or R1C1, or sheet-qualified) address TEXT.
//!   `abs_num` 1 `$A$1` (default), 2 `A$1`, 3 `$A1`, 4 `A1`; `a1=FALSE`
//!   selects R1C1 text. Entirely textual — needs no model.
//! - [`rows`] / [`columns`] — the row/column count of a range argument; a
//!   scalar argument is `1`.
//!
//! ## Deferred (status `planned`) — need a model cell/formula reader the
//! ## FROZEN `EvalCtx` cannot carry
//!
//! `OFFSET`, `INDIRECT`, and `FORMULATEXT` each require reading **arbitrary**
//! cells (or a cell's formula) that are NOT among the kernel's `Arg` values:
//!
//! - `OFFSET(reference, rows, cols, …)` must read the cell `(rows, cols)`
//!   away from the base reference's origin — a cell *outside* the
//!   materialized base [`crate::arg::RangeView`]. A `RangeView` clamps
//!   out-of-window reads to [`sheet_core::CellValue::Empty`], so the kernel
//!   physically cannot reach the offset target through its arguments.
//! - `INDIRECT(ref_text, …)` parses an A1 string at runtime and reads that
//!   address — again a cell the kernel never received as an `Arg`.
//! - `FORMULATEXT(reference)` needs the referenced cell's *formula source*,
//!   which is not a `CellValue` and never crosses the `Arg` boundary at all.
//!
//! The pure-kernel signature (`fn(&[Arg], &EvalCtx) -> CellValue`) and
//! [`crate::ctx::EvalCtx`] are **FROZEN** (CLAUDE.md §"Interface freeze"); the
//! field that would unblock these — `cell_reader: Option<&dyn Fn(CellRef) ->
//! CellValue>` (plus a `formula_reader` for `FORMULATEXT`) — requires giving
//! `EvalCtx` a lifetime, which ripples through every family kernel signature
//! and `sheet-calc`. That is a versioned amendment for the orchestrator, NOT a
//! drive-by edit, so these three rows stay `status: planned` (uncallable →
//! `#NAME?`) and are documented here and in the implementation report. Nothing
//! is faked: a `planned` row never reaches a kernel.

use compact_str::CompactString;
use sheet_core::{col_to_a1, CellError, CellValue};

use crate::arg::{Arg, RangeView};
use crate::coerce;
use crate::criteria::Matcher;
use crate::ctx::EvalCtx;

// ===========================================================================
// XLOOKUP / XMATCH — shared match/search machinery
// ===========================================================================

/// `match_mode` (Microsoft XLOOKUP/XMATCH): exact, or the directional
/// approximate fallbacks, or wildcard.
#[derive(Copy, Clone, PartialEq, Eq)]
enum MatchMode {
    /// 0 — exact match (default). Not found → fallback.
    Exact,
    /// -1 — exact match or the next *smaller* item.
    ExactOrSmaller,
    /// 1 — exact match or the next *larger* item.
    ExactOrLarger,
    /// 2 — wildcard text match (`*`/`?`/`~`, via [`Matcher`]).
    Wildcard,
}

impl MatchMode {
    fn from_i64(n: i64) -> Option<MatchMode> {
        match n {
            0 => Some(MatchMode::Exact),
            -1 => Some(MatchMode::ExactOrSmaller),
            1 => Some(MatchMode::ExactOrLarger),
            2 => Some(MatchMode::Wildcard),
            _ => None,
        }
    }
}

/// `search_mode` (Microsoft XLOOKUP/XMATCH): scan direction or binary search.
#[derive(Copy, Clone, PartialEq, Eq)]
enum SearchMode {
    /// 1 — first to last (default).
    FirstToLast,
    /// -1 — last to first.
    LastToFirst,
    /// 2 — binary search, data assumed ascending.
    BinaryAsc,
    /// -2 — binary search, data assumed descending.
    BinaryDesc,
}

impl SearchMode {
    fn from_i64(n: i64) -> Option<SearchMode> {
        match n {
            1 => Some(SearchMode::FirstToLast),
            -1 => Some(SearchMode::LastToFirst),
            2 => Some(SearchMode::BinaryAsc),
            -2 => Some(SearchMode::BinaryDesc),
            _ => None,
        }
    }
}

/// A 1-D vector view over a single-row or single-column range (the shape
/// XLOOKUP/XMATCH require of `lookup_array`/`return_array`). `len` is the
/// element count; `at(i)` reads element `i` row-major.
struct Vector<'a> {
    rv: &'a RangeView<'a>,
    len: u32,
    /// True when the source range is a single column (`rows×1`); false for a
    /// single row (`1×cols`). Drives the row-major index → `(r, c)` map.
    column: bool,
}

impl Vector<'_> {
    /// Build a 1-D vector view, requiring a single-row or single-column shape.
    /// A rectangular (multi-row AND multi-column) range is `None` → `#VALUE!`.
    fn new<'b>(rv: &'b RangeView<'b>) -> Option<Vector<'b>> {
        let (rows, cols) = (rv.rows(), rv.cols());
        if rows == 0 || cols == 0 {
            return None;
        }
        if cols == 1 {
            Some(Vector {
                rv,
                len: rows,
                column: true,
            })
        } else if rows == 1 {
            Some(Vector {
                rv,
                len: cols,
                column: false,
            })
        } else {
            None
        }
    }

    fn get(&self, i: u32) -> CellValue {
        if self.column {
            self.rv.get(i, 0)
        } else {
            self.rv.get(0, i)
        }
    }
}

/// `XLOOKUP(lookup_value, lookup_array, return_array, [if_not_found],
/// [match_mode], [search_mode])` (Microsoft public docs).
///
/// Finds `lookup_value` in `lookup_array` (a single row or column) and returns
/// the aligned cell of `return_array` (which MUST be the same length, else
/// `#VALUE!`). `match_mode` and `search_mode` are as on the module docs. A
/// not-found returns `if_not_found` when supplied, otherwise `#N/A`. An error
/// in a scalar argument (the key or the two mode flags) propagates.
pub fn xlookup(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let key = match scalar_value(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };

    // lookup_array and return_array are ranges (a scalar is a 1×1 vector).
    let lookup_rv = arg_as_range(&args[1]);
    let return_rv = arg_as_range(&args[2]);
    let (lookup_rv, return_rv) = match (lookup_rv, return_rv) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return CellValue::Error(e),
    };

    let lookup_vec = match Vector::new(lookup_rv) {
        Some(v) => v,
        None => return CellValue::Error(CellError::Value),
    };
    let return_vec = match Vector::new(return_rv) {
        Some(v) => v,
        None => return CellValue::Error(CellError::Value),
    };
    // The two vectors must align (XLOOKUP requires matching lengths).
    if lookup_vec.len != return_vec.len {
        return CellValue::Error(CellError::Value);
    }

    let match_mode = match args.get(4) {
        Some(a) => match scalar_i64(a) {
            Ok(n) => match MatchMode::from_i64(n) {
                Some(m) => m,
                None => return CellValue::Error(CellError::Value),
            },
            Err(e) => return CellValue::Error(e),
        },
        None => MatchMode::Exact,
    };
    let search_mode = match args.get(5) {
        Some(a) => match scalar_i64(a) {
            Ok(n) => match SearchMode::from_i64(n) {
                Some(m) => m,
                None => return CellValue::Error(CellError::Value),
            },
            Err(e) => return CellValue::Error(e),
        },
        None => SearchMode::FirstToLast,
    };

    match find_pos(&key, &lookup_vec, match_mode, search_mode) {
        Some(pos) => return_vec.get(pos),
        None => match args.get(3) {
            // if_not_found supplied: return it (an error there passes through).
            Some(a) => scalar_value(a).unwrap_or_else(CellValue::Error),
            None => CellValue::Error(CellError::Na),
        },
    }
}

/// `XMATCH(lookup_value, lookup_array, [match_mode], [search_mode])` (Microsoft
/// public docs). Returns the 1-based position of `lookup_value` within the
/// single-row/single-column `lookup_array`; modes are as [`xlookup`]. A
/// not-found is `#N/A`; an error in a scalar argument propagates.
pub fn xmatch(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let key = match scalar_value(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };

    let lookup_rv = match arg_as_range(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let lookup_vec = match Vector::new(lookup_rv) {
        Some(v) => v,
        None => return CellValue::Error(CellError::Value),
    };

    let match_mode = match args.get(2) {
        Some(a) => match scalar_i64(a) {
            Ok(n) => match MatchMode::from_i64(n) {
                Some(m) => m,
                None => return CellValue::Error(CellError::Value),
            },
            Err(e) => return CellValue::Error(e),
        },
        None => MatchMode::Exact,
    };
    let search_mode = match args.get(3) {
        Some(a) => match scalar_i64(a) {
            Ok(n) => match SearchMode::from_i64(n) {
                Some(m) => m,
                None => return CellValue::Error(CellError::Value),
            },
            Err(e) => return CellValue::Error(e),
        },
        None => SearchMode::FirstToLast,
    };

    match find_pos(&key, &lookup_vec, match_mode, search_mode) {
        Some(pos) => CellValue::Number((pos + 1) as f64),
        None => CellValue::Error(CellError::Na),
    }
}

/// The shared XLOOKUP/XMATCH position finder. Returns the 0-based index of the
/// match, honoring `match_mode` (exact / directional approximate / wildcard)
/// and `search_mode` (linear direction / binary). Binary modes fall back to a
/// linear scan for the wildcard mode (wildcards have no sorted order).
fn find_pos(
    key: &CellValue,
    vec: &Vector,
    match_mode: MatchMode,
    search_mode: SearchMode,
) -> Option<u32> {
    match match_mode {
        MatchMode::Wildcard => wildcard_pos(key, vec, search_mode),
        MatchMode::Exact => exact_pos(key, vec, search_mode),
        MatchMode::ExactOrSmaller => approx_pos(key, vec, search_mode, Direction::Smaller),
        MatchMode::ExactOrLarger => approx_pos(key, vec, search_mode, Direction::Larger),
    }
}

/// Direction of an XLOOKUP approximate fallback when no exact hit exists.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Direction {
    /// `match_mode -1`: the largest item that is `<= key`.
    Smaller,
    /// `match_mode 1`: the smallest item that is `>= key`.
    Larger,
}

/// Exact match (`match_mode 0`). Linear modes scan in the requested direction;
/// the binary modes use a true binary search over the assumed-sorted data.
fn exact_pos(key: &CellValue, vec: &Vector, search_mode: SearchMode) -> Option<u32> {
    match search_mode {
        SearchMode::FirstToLast => (0..vec.len).find(|&i| values_equal(&vec.get(i), key)),
        SearchMode::LastToFirst => (0..vec.len).rev().find(|&i| values_equal(&vec.get(i), key)),
        SearchMode::BinaryAsc => binary_search_exact(key, vec, true),
        SearchMode::BinaryDesc => binary_search_exact(key, vec, false),
    }
}

/// Lookup equality (XLOOKUP/XMATCH exact, non-wildcard). Excel lookup equality
/// is **case-insensitive** for text — but [`coerce::compare`] keeps a raw-byte
/// tie-break to stay a total order, so `"cherry"` vs `"CHERRY"` compares
/// `Greater`/`Less`, never `Equal`. This helper restores the case-insensitive
/// text equality (matching the T0 `lookup::exact_pos` ruling) while delegating
/// to the cross-type order for every non-text/non-text pair.
fn values_equal(a: &CellValue, b: &CellValue) -> bool {
    match (a, b) {
        (CellValue::Text(x), CellValue::Text(y)) => x.eq_ignore_ascii_case(y),
        // Mixed types (e.g. number vs text) never compare equal in lookup.
        (CellValue::Text(_), _) | (_, CellValue::Text(_)) => false,
        _ => coerce::compare(a, b) == std::cmp::Ordering::Equal,
    }
}

/// Wildcard match (`match_mode 2`). The key is compiled as a text pattern and
/// tested against TEXT candidates only (Excel wildcards never match
/// numbers/bools/blanks). Binary search is meaningless for wildcards, so the
/// binary modes degrade to the corresponding linear scan.
fn wildcard_pos(key: &CellValue, vec: &Vector, search_mode: SearchMode) -> Option<u32> {
    let matcher = Matcher::compile(coerce::to_text(key).as_str());
    let test = |i: u32| -> bool {
        match vec.get(i) {
            CellValue::Text(t) => matcher.is_match(t.as_str()),
            _ => false,
        }
    };
    match search_mode {
        SearchMode::LastToFirst | SearchMode::BinaryDesc => (0..vec.len).rev().find(|&i| test(i)),
        _ => (0..vec.len).find(|&i| test(i)),
    }
}

/// Approximate match (`match_mode -1`/`1`). An exact hit always wins; otherwise
/// the nearest item on the requested side. Linear modes scan the whole vector
/// tracking the best candidate; binary modes use a sorted binary search.
fn approx_pos(
    key: &CellValue,
    vec: &Vector,
    search_mode: SearchMode,
    dir: Direction,
) -> Option<u32> {
    use std::cmp::Ordering::*;
    match search_mode {
        SearchMode::BinaryAsc => binary_search_approx(key, vec, true, dir),
        SearchMode::BinaryDesc => binary_search_approx(key, vec, false, dir),
        _ => {
            // Linear scan. Track the best item on the requested side; an exact
            // hit short-circuits. Direction of the scan does not change the
            // mathematical answer (nearest-on-side), but for ties XLOOKUP
            // returns the first encountered, so honor the scan order.
            let order: Vec<u32> = match search_mode {
                SearchMode::LastToFirst => (0..vec.len).rev().collect(),
                _ => (0..vec.len).collect(),
            };
            let mut best: Option<(u32, CellValue)> = None;
            for i in order {
                let cand = vec.get(i);
                match coerce::compare(&cand, key) {
                    Equal => return Some(i),
                    Less if dir == Direction::Smaller => {
                        // candidate < key, want the LARGEST such → keep the max.
                        best = match best {
                            Some((_, ref b)) if coerce::compare(&cand, b) != Greater => best,
                            _ => Some((i, cand)),
                        };
                    }
                    Greater if dir == Direction::Larger => {
                        // candidate > key, want the SMALLEST such → keep the min.
                        best = match best {
                            Some((_, ref b)) if coerce::compare(&cand, b) != Less => best,
                            _ => Some((i, cand)),
                        };
                    }
                    _ => {}
                }
            }
            best.map(|(i, _)| i)
        }
    }
}

/// Binary search for an exact match over assumed-sorted data (`asc` true for
/// ascending, false for descending). Returns any matching index (the data is
/// assumed to have a single key).
fn binary_search_exact(key: &CellValue, vec: &Vector, asc: bool) -> Option<u32> {
    use std::cmp::Ordering::*;
    let (mut lo, mut hi) = (0i64, vec.len as i64 - 1);
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let cand = vec.get(mid as u32);
        // Case-insensitive equality wins (lookup equality); the cross-type
        // order only steers navigation over the assumed-sorted data.
        if values_equal(&cand, key) {
            return Some(mid as u32);
        }
        match coerce::compare(&cand, key) {
            Less => {
                if asc {
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            // Greater (Equal already handled above): move the other half.
            _ => {
                if asc {
                    hi = mid - 1;
                } else {
                    lo = mid + 1;
                }
            }
        }
    }
    None
}

/// Binary search for an approximate match over assumed-sorted data. On no exact
/// hit, returns the nearest index on the `dir` side (largest `<= key` for
/// `Smaller`, smallest `>= key` for `Larger`). `asc` is the assumed sort order.
fn binary_search_approx(key: &CellValue, vec: &Vector, asc: bool, dir: Direction) -> Option<u32> {
    use std::cmp::Ordering::*;
    let (mut lo, mut hi) = (0i64, vec.len as i64 - 1);
    let mut best: Option<u32> = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let cand = vec.get(mid as u32);
        match coerce::compare(&cand, key) {
            Equal => return Some(mid as u32),
            ord => {
                // `ord` is candidate vs key. Decide which half to keep and
                // whether `mid` is a valid fallback for `dir`.
                let cand_less = ord == Less; // candidate < key
                if (dir == Direction::Smaller && cand_less)
                    || (dir == Direction::Larger && !cand_less)
                {
                    best = Some(mid as u32);
                }
                // Move toward the key in the assumed sort order.
                let go_right = if asc { cand_less } else { !cand_less };
                if go_right {
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
        }
    }
    best
}

// ===========================================================================
// ADDRESS
// ===========================================================================

/// `ADDRESS(row_num, col_num, [abs_num], [a1], [sheet_text])` (ECMA-376
/// §18.17.7). Builds the address TEXT of the cell at 1-based `(row_num,
/// col_num)`.
///
/// - `abs_num`: 1 `$A$1` (default), 2 `A$1`, 3 `$A1`, 4 `A1`; out of 1..=4 →
///   `#VALUE!`.
/// - `a1`: TRUE/omitted → A1 style; FALSE → R1C1 style (`R1C1`, `R1`, `C1`,
///   etc. with the same abs/rel rule via `[..]` brackets).
/// - `sheet_text`: when present and non-empty, prefixes `Sheet!` (quoting the
///   name when it contains a space or other token-breaking character).
///
/// `row_num`/`col_num < 1` (or past the grid) → `#REF!`. An error in any
/// scalar argument propagates. Pure: needs no model access.
pub fn address(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let row = match scalar_i64(&args[0]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let col = match scalar_i64(&args[1]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };

    let abs_num = match args.get(2) {
        Some(a) => match scalar_i64(a) {
            Ok(n) => n,
            Err(e) => return CellValue::Error(e),
        },
        None => 1,
    };
    if !(1..=4).contains(&abs_num) {
        return CellValue::Error(CellError::Value);
    }
    // abs_num: row absolute when 1|2, col absolute when 1|3.
    let row_abs = abs_num == 1 || abs_num == 2;
    let col_abs = abs_num == 1 || abs_num == 3;

    let a1_style = match args.get(3) {
        Some(a) => match scalar_bool(a) {
            Ok(b) => b,
            Err(e) => return CellValue::Error(e),
        },
        None => true,
    };

    // Grid bounds: 1-based row/col must be within Excel's 1..=MAX+1 range.
    if row < 1
        || col < 1
        || row > (sheet_core::MAX_ROW as i64 + 1)
        || col > (sheet_core::MAX_COL as i64 + 1)
    {
        return CellValue::Error(CellError::Ref);
    }
    let col0 = (col - 1) as u32;

    let body = if a1_style {
        let mut s = CompactString::default();
        if col_abs {
            s.push('$');
        }
        s.push_str(&col_to_a1(col0));
        if row_abs {
            s.push('$');
        }
        s.push_str(&CompactString::new(row.to_string()));
        s
    } else {
        // R1C1: absolute coordinate is bare (`R1`), relative is bracketed
        // (`R[1]`). ADDRESS produces absolute coordinates for an absolute flag
        // and a 0-offset relative coordinate otherwise (`R[0]`-style is what
        // Excel emits for a relative ADDRESS, but the common ADDRESS contract
        // is absolute numbers — Excel's ADDRESS with a1=FALSE emits `R1C1` for
        // abs and `R[1]C[1]` for relative). We follow that: bare for absolute.
        let r = if row_abs {
            format!("R{row}")
        } else {
            format!("R[{row}]")
        };
        let c = if col_abs {
            format!("C{col}")
        } else {
            format!("C[{col}]")
        };
        CompactString::new(format!("{r}{c}"))
    };

    // Optional sheet prefix.
    match args.get(4) {
        Some(a) => match scalar_value(a) {
            Ok(v) => {
                let name = coerce::to_text(&v);
                if name.is_empty() {
                    CellValue::Text(body)
                } else {
                    CellValue::Text(prefix_sheet(name.as_str(), body.as_str()))
                }
            }
            Err(e) => CellValue::Error(e),
        },
        None => CellValue::Text(body),
    }
}

/// Prefix `Sheet!` to an address body, quoting the sheet name with single
/// quotes when it contains a character that would break the `Sheet!A1` token
/// (a space or any non-alphanumeric/underscore), per Excel's address syntax.
fn prefix_sheet(sheet: &str, body: &str) -> CompactString {
    let needs_quote = sheet
        .chars()
        .any(|c| !(c.is_alphanumeric() || c == '_' || c == '.'))
        || sheet.chars().next().is_some_and(|c| c.is_ascii_digit());
    if needs_quote {
        // Embedded single quotes are doubled inside the quoted name.
        let escaped = sheet.replace('\'', "''");
        CompactString::new(format!("'{escaped}'!{body}"))
    } else {
        CompactString::new(format!("{sheet}!{body}"))
    }
}

// ===========================================================================
// ROWS / COLUMNS
// ===========================================================================

/// `ROWS(range)` (ECMA-376 §18.17.7). The row count of the reference/array
/// argument. A scalar (a single value) is `1`. An error scalar propagates.
pub fn rows(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match &args[0] {
        Arg::Range(rv) => CellValue::Number(rv.rows() as f64),
        Arg::Scalar(CellValue::Error(e)) => CellValue::Error(*e),
        Arg::Scalar(_) => CellValue::Number(1.0),
    }
}

/// `COLUMNS(range)` (ECMA-376 §18.17.7) — the column transpose of [`rows`]: the
/// column count of the reference/array argument; a scalar is `1`.
pub fn columns(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match &args[0] {
        Arg::Range(rv) => CellValue::Number(rv.cols() as f64),
        Arg::Scalar(CellValue::Error(e)) => CellValue::Error(*e),
        Arg::Scalar(_) => CellValue::Number(1.0),
    }
}

// ===========================================================================
// Scalar-argument helpers (route through `coerce`)
// ===========================================================================

/// Read a scalar-slot argument as a [`CellValue`], propagating an error value.
/// A range in a scalar slot implicit-intersects to its top-left.
fn scalar_value(arg: &Arg) -> Result<CellValue, CellError> {
    match arg {
        Arg::Scalar(CellValue::Error(e)) => Err(*e),
        Arg::Scalar(v) => Ok(v.clone()),
        Arg::Range(rv) => match rv.get(0, 0) {
            CellValue::Error(e) => Err(e),
            v => Ok(v),
        },
    }
}

/// Coerce a scalar-slot argument to a truncated `i64` (a mode flag / index).
fn scalar_i64(arg: &Arg) -> Result<i64, CellError> {
    let n = match arg {
        Arg::Scalar(v) => coerce::to_number(v)?,
        Arg::Range(rv) => coerce::to_number(&rv.get(0, 0))?,
    };
    Ok(n.trunc() as i64)
}

/// Coerce a scalar-slot argument to a boolean (the `a1` flag).
fn scalar_bool(arg: &Arg) -> Result<bool, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_bool(v),
        Arg::Range(rv) => coerce::to_bool(&rv.get(0, 0)),
    }
}

/// Borrow a range argument's [`RangeView`], propagating an error scalar. A
/// non-error scalar in a range slot is a degenerate 1×1 vector — but the
/// XLOOKUP/XMATCH path needs a real `RangeView`, so a scalar there is reported
/// as `#VALUE!` (the convention hands ranges for `range_aware` args; a bare
/// scalar lookup_array is a malformed call).
fn arg_as_range<'a>(arg: &'a Arg<'a>) -> Result<&'a RangeView<'a>, CellError> {
    match arg {
        Arg::Range(rv) => Ok(rv),
        Arg::Scalar(CellValue::Error(e)) => Err(*e),
        Arg::Scalar(_) => Err(CellError::Value),
    }
}
