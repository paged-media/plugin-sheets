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

//! Information-family conformance (spec §7, §11 T0). Self-contained
//! direct-dispatch tests: each case resolves a `FuncId` via
//! `sheet_core::funcs::lookup_func`, builds an `&[Arg]` slice, and calls
//! `sheet_fn::dispatch` — exercising the exact path `sheet-calc` will, arity
//! guard and all. Test fns are named with the `sheet_fn_info_<name>` prefix
//! the registry rows in `registry/functions/info.yaml` point at, so the
//! §12.2 coverage gate finds them.
//!
//! The information predicates encode three rulings worth restating because
//! they are where engines disagree (ECMA-376 §18.17.7):
//! - `ISBLANK` is `TRUE` **only** for a blank cell — `Text("")` is not blank;
//! - the `IS*` error/type checks apply **no coercion** (`ISNUMBER("1")` is
//!   `FALSE`) and **do not propagate** an error argument (`ISERROR(#DIV/0!)`
//!   is `TRUE`, never `#DIV/0!`);
//! - `ISERROR` ⊇ `ISERR` ⊎ `ISNA`: `ISERROR` is any error, `ISNA` is only
//!   `#N/A`, `ISERR` is any error *except* `#N/A`.

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::{dispatch, Arg, EvalCtx, RangeView};

/// The fixed evaluation context for these (non-volatile) info kernels — a
/// concrete date system, current cell, injected clock serial, and RNG seed.
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cr(), 45000.5, 42)
}

fn cr() -> CellRef {
    CellRef {
        sheet: 0,
        row: 0,
        col: 0,
        row_abs: false,
        col_abs: false,
    }
}

/// Call an info function by Excel name through the generated dispatch (the
/// real call path, including the registry-driven arity guard).
fn call(name: &str, args: &[Arg]) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("lookup_func({name}) returned None — registry row missing?"));
    dispatch(id, args, &ctx())
}

fn boolean(b: bool) -> CellValue {
    CellValue::Bool(b)
}

fn err(e: CellError) -> CellValue {
    CellValue::Error(e)
}

// ---------------------------------------------------------------------------
// ISBLANK — TRUE only for Empty (Text("") is NOT blank).
// ---------------------------------------------------------------------------

#[test]
fn sheet_fn_info_isblank_basic() {
    assert_eq!(
        call("ISBLANK", &[Arg::Scalar(CellValue::Empty)]),
        boolean(true)
    );
    assert_eq!(
        call("ISBLANK", &[Arg::Scalar(CellValue::Number(0.0))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_isblank_empty_string_is_not_blank() {
    // The defining edge: a zero-length string is a value, not a blank cell.
    assert_eq!(
        call("ISBLANK", &[Arg::Scalar(CellValue::from(""))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_isblank_range_top_left() {
    // range_aware:false, but a range arg collapses to its top-left cell.
    let cells = [CellValue::Empty, CellValue::Number(1.0)];
    let v = RangeView::from_slice(cr(), 1, 2, &cells);
    assert_eq!(call("ISBLANK", &[Arg::Range(v)]), boolean(true));
}

#[test]
fn sheet_fn_info_isblank_arity_violation() {
    // Zero args (min is 1) → dispatch arity guard returns #VALUE!.
    assert_eq!(call("ISBLANK", &[]), err(CellError::Value));
    // Two args (max is 1) → #VALUE!.
    assert_eq!(
        call(
            "ISBLANK",
            &[Arg::Scalar(CellValue::Empty), Arg::Scalar(CellValue::Empty)]
        ),
        err(CellError::Value)
    );
}

// ---------------------------------------------------------------------------
// ISNUMBER / ISTEXT / ISLOGICAL — strict variant checks, NO coercion.
// ---------------------------------------------------------------------------

#[test]
fn sheet_fn_info_isnumber_basic() {
    assert_eq!(
        call("ISNUMBER", &[Arg::Scalar(CellValue::Number(2.5))]),
        boolean(true)
    );
    assert_eq!(
        call("ISNUMBER", &[Arg::Scalar(CellValue::Empty)]),
        boolean(false)
    );
    assert_eq!(
        call("ISNUMBER", &[Arg::Scalar(CellValue::Bool(true))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_isnumber_no_coercion() {
    // Numeric TEXT is NOT a number — strict variant check, no coercion.
    assert_eq!(
        call("ISNUMBER", &[Arg::Scalar(CellValue::from("1"))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_isnumber_does_not_propagate_error() {
    // An error argument is the SUBJECT — ISNUMBER(error) is FALSE, not the
    // error (info functions never propagate argument errors).
    assert_eq!(
        call("ISNUMBER", &[Arg::Scalar(err(CellError::Div0))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_istext_basic() {
    assert_eq!(
        call("ISTEXT", &[Arg::Scalar(CellValue::from("hi"))]),
        boolean(true)
    );
    // Empty cell is not text (Empty != Text("")).
    assert_eq!(
        call("ISTEXT", &[Arg::Scalar(CellValue::Empty)]),
        boolean(false)
    );
    assert_eq!(
        call("ISTEXT", &[Arg::Scalar(CellValue::Number(1.0))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_istext_no_coercion() {
    // A number is not text even though it has a text representation.
    assert_eq!(
        call("ISTEXT", &[Arg::Scalar(CellValue::Number(42.0))]),
        boolean(false)
    );
    // Range top-left text → TRUE.
    let cells = [CellValue::from("a"), CellValue::Number(2.0)];
    let v = RangeView::from_slice(cr(), 1, 2, &cells);
    assert_eq!(call("ISTEXT", &[Arg::Range(v)]), boolean(true));
}

#[test]
fn sheet_fn_info_islogical_basic() {
    assert_eq!(
        call("ISLOGICAL", &[Arg::Scalar(CellValue::Bool(false))]),
        boolean(true)
    );
    assert_eq!(
        call("ISLOGICAL", &[Arg::Scalar(CellValue::Bool(true))]),
        boolean(true)
    );
}

#[test]
fn sheet_fn_info_islogical_no_coercion() {
    // 1/0 are numbers, "TRUE"/"FALSE" are text — none are logical.
    assert_eq!(
        call("ISLOGICAL", &[Arg::Scalar(CellValue::Number(1.0))]),
        boolean(false)
    );
    assert_eq!(
        call("ISLOGICAL", &[Arg::Scalar(CellValue::from("TRUE"))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_islogical_arity_violation() {
    assert_eq!(call("ISLOGICAL", &[]), err(CellError::Value));
}

// ---------------------------------------------------------------------------
// ISERROR / ISERR / ISNA — the error-class predicates. ISERROR ⊇ ISERR ⊎ ISNA.
// ---------------------------------------------------------------------------

#[test]
fn sheet_fn_info_iserror_basic() {
    // ANY error → TRUE, #N/A included.
    for e in [
        CellError::Div0,
        CellError::Value,
        CellError::Ref,
        CellError::Name,
        CellError::Num,
        CellError::Na,
        CellError::Null,
        CellError::Spill,
    ] {
        assert_eq!(
            call("ISERROR", &[Arg::Scalar(err(e))]),
            boolean(true),
            "ISERROR({}) should be TRUE",
            e.as_str()
        );
    }
}

#[test]
fn sheet_fn_info_iserror_non_error_is_false() {
    // Does NOT propagate: a clean value → FALSE, not an error.
    assert_eq!(
        call("ISERROR", &[Arg::Scalar(CellValue::Number(1.0))]),
        boolean(false)
    );
    assert_eq!(
        call("ISERROR", &[Arg::Scalar(CellValue::Empty)]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_iserr_excludes_na() {
    // ISERR is any error EXCEPT #N/A.
    assert_eq!(
        call("ISERR", &[Arg::Scalar(err(CellError::Div0))]),
        boolean(true)
    );
    assert_eq!(
        call("ISERR", &[Arg::Scalar(err(CellError::Value))]),
        boolean(true)
    );
    // #N/A is the one error ISERR rejects.
    assert_eq!(
        call("ISERR", &[Arg::Scalar(err(CellError::Na))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_iserr_non_error_is_false() {
    assert_eq!(
        call("ISERR", &[Arg::Scalar(CellValue::from("x"))]),
        boolean(false)
    );
}

#[test]
fn sheet_fn_info_isna_only_na() {
    // ISNA matches ONLY #N/A.
    assert_eq!(
        call("ISNA", &[Arg::Scalar(err(CellError::Na))]),
        boolean(true)
    );
    for e in [
        CellError::Div0,
        CellError::Value,
        CellError::Ref,
        CellError::Name,
        CellError::Num,
        CellError::Null,
        CellError::Spill,
    ] {
        assert_eq!(
            call("ISNA", &[Arg::Scalar(err(e))]),
            boolean(false),
            "ISNA({}) should be FALSE",
            e.as_str()
        );
    }
}

#[test]
fn sheet_fn_info_isna_non_error_is_false() {
    assert_eq!(
        call("ISNA", &[Arg::Scalar(CellValue::Number(1.0))]),
        boolean(false)
    );
}

// ---------------------------------------------------------------------------
// NA — produces the #N/A error. Arity 0.
// ---------------------------------------------------------------------------

#[test]
fn sheet_fn_info_na_basic() {
    assert_eq!(call("NA", &[]), err(CellError::Na));
}

#[test]
fn sheet_fn_info_na_arity_violation() {
    // NA takes zero args (max 0); any arg → #VALUE!.
    assert_eq!(
        call("NA", &[Arg::Scalar(CellValue::Number(1.0))]),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_info_na_feeds_isna() {
    // Composability: NA() is exactly what ISNA recognizes.
    let produced = call("NA", &[]);
    assert_eq!(call("ISNA", &[Arg::Scalar(produced)]), boolean(true));
}
