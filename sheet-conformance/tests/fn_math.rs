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

//! Math/trig family conformance (spec §7, §11 T0). SELF-CONTAINED
//! direct-dispatch tests: each case resolves the function name through
//! `sheet_core::funcs::lookup_func` and routes the call through
//! [`sheet_fn::dispatch`] — the same choke point a real evaluation crosses,
//! so arity guards (`#VALUE!` on violation) are exercised end-to-end, not
//! bypassed by calling the kernel directly.
//!
//! Every function gets at least one `fn sheet_fn_math_<name>…` test (the
//! prefix the registry rows point at, which the coverage gate greps for).
//! The cases cover happy path, coercion edge, error propagation, range
//! behavior (where `range_aware`), arity violation, and the Excel rulings:
//! MOD sign-follows-divisor, INT floor-toward-−∞, ROUND half-away-from-zero,
//! classic CEILING/FLOOR sign/zero rules, domain `#NUM!`s, `#DIV/0!`, and
//! the deterministic seeded RAND/RANDBETWEEN.

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::arg::{Arg, RangeView};
use sheet_fn::{dispatch, EvalCtx};

// ---- harness ---------------------------------------------------------------

fn cell() -> CellRef {
    CellRef {
        sheet: 0,
        row: 0,
        col: 0,
        row_abs: false,
        col_abs: false,
    }
}

/// A deterministic context (fixed now-serial + seed) per the prompt.
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cell(), 45000.5, 42)
}

/// Dispatch a function by registry name with the given args.
fn call(name: &str, args: &[Arg]) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("function {name} not in registry"));
    dispatch(id, args, &ctx())
}

fn num(x: f64) -> Arg<'static> {
    Arg::Scalar(CellValue::Number(x))
}
fn txt(s: &str) -> Arg<'static> {
    Arg::Scalar(CellValue::from(s))
}
fn boolean(b: bool) -> Arg<'static> {
    Arg::Scalar(CellValue::Bool(b))
}
fn err(e: CellError) -> Arg<'static> {
    Arg::Scalar(CellValue::Error(e))
}

/// Assert a numeric result within a tight tolerance (transcendentals).
fn approx(got: &CellValue, want: f64) {
    match got {
        CellValue::Number(n) => assert!(
            (n - want).abs() < 1e-9,
            "expected ~{want}, got {n} (Δ={})",
            (n - want).abs()
        ),
        other => panic!("expected Number(~{want}), got {other:?}"),
    }
}

fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}
fn e(c: CellError) -> CellValue {
    CellValue::Error(c)
}

// ---- SUM -------------------------------------------------------------------

#[test]
fn sheet_fn_math_sum_basic() {
    assert_eq!(call("SUM", &[num(1.0), num(2.0), num(3.0)]), n(6.0));
    assert_eq!(call("SUM", &[num(-5.0), num(5.0)]), n(0.0));
}

#[test]
fn sheet_fn_math_sum_scalar_coercion() {
    // Scalar text/bool DO coerce (range-vs-scalar asymmetry).
    assert_eq!(call("SUM", &[num(1.0), txt("5"), boolean(true)]), n(7.0));
    // Un-parseable scalar text → #VALUE!.
    assert_eq!(call("SUM", &[num(1.0), txt("x")]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_sum_range_skips_nonnumeric() {
    // A range with text/bool/blank cells: only the numbers contribute.
    let cells = [
        CellValue::Number(1.0),
        CellValue::from("hello"),
        CellValue::Bool(true),
        CellValue::Empty,
        CellValue::Number(4.0),
    ];
    let view = RangeView::from_slice(cell(), 1, 5, &cells);
    assert_eq!(call("SUM", &[Arg::Range(view)]), n(5.0));
}

#[test]
fn sheet_fn_math_sum_range_error_propagates() {
    // An error cell INSIDE a range IS the error.
    let cells = [CellValue::Number(1.0), CellValue::Error(CellError::Div0)];
    let view = RangeView::from_slice(cell(), 1, 2, &cells);
    assert_eq!(call("SUM", &[Arg::Range(view)]), e(CellError::Div0));
}

#[test]
fn sheet_fn_math_sum_scalar_error_propagates() {
    assert_eq!(
        call("SUM", &[num(1.0), err(CellError::Na)]),
        e(CellError::Na)
    );
}

#[test]
fn sheet_fn_math_sum_arity_violation() {
    // min 1 — zero args is #VALUE!.
    assert_eq!(call("SUM", &[]), e(CellError::Value));
}

// ---- PRODUCT ---------------------------------------------------------------

#[test]
fn sheet_fn_math_product_basic() {
    assert_eq!(call("PRODUCT", &[num(2.0), num(3.0), num(4.0)]), n(24.0));
}

#[test]
fn sheet_fn_math_product_range_skips_and_errors() {
    let cells = [
        CellValue::Number(2.0),
        CellValue::from("x"),
        CellValue::Number(5.0),
    ];
    let view = RangeView::from_slice(cell(), 1, 3, &cells);
    assert_eq!(call("PRODUCT", &[Arg::Range(view)]), n(10.0));

    let bad = [CellValue::Number(2.0), CellValue::Error(CellError::Num)];
    let bview = RangeView::from_slice(cell(), 1, 2, &bad);
    assert_eq!(call("PRODUCT", &[Arg::Range(bview)]), e(CellError::Num));
}

#[test]
fn sheet_fn_math_product_scalar_coercion_and_arity() {
    assert_eq!(call("PRODUCT", &[num(3.0), boolean(true)]), n(3.0));
    assert_eq!(call("PRODUCT", &[]), e(CellError::Value));
}

// ---- ABS / SIGN ------------------------------------------------------------

#[test]
fn sheet_fn_math_abs_basic() {
    assert_eq!(call("ABS", &[num(-7.5)]), n(7.5));
    assert_eq!(call("ABS", &[num(3.0)]), n(3.0));
    // Coercion edge: text number.
    assert_eq!(call("ABS", &[txt("-2")]), n(2.0));
}

#[test]
fn sheet_fn_math_abs_error_and_arity() {
    assert_eq!(call("ABS", &[err(CellError::Ref)]), e(CellError::Ref));
    assert_eq!(call("ABS", &[]), e(CellError::Value)); // arity
    assert_eq!(call("ABS", &[num(1.0), num(2.0)]), e(CellError::Value)); // arity max
}

#[test]
fn sheet_fn_math_sign_basic() {
    assert_eq!(call("SIGN", &[num(12.0)]), n(1.0));
    assert_eq!(call("SIGN", &[num(-12.0)]), n(-1.0));
    assert_eq!(call("SIGN", &[num(0.0)]), n(0.0));
}

// ---- SQRT / EXP ------------------------------------------------------------

#[test]
fn sheet_fn_math_sqrt_basic() {
    assert_eq!(call("SQRT", &[num(16.0)]), n(4.0));
    assert_eq!(call("SQRT", &[num(0.0)]), n(0.0));
}

#[test]
fn sheet_fn_math_sqrt_domain_num_error() {
    // Negative argument is out of domain → #NUM!.
    assert_eq!(call("SQRT", &[num(-1.0)]), e(CellError::Num));
}

#[test]
fn sheet_fn_math_exp_basic() {
    approx(&call("EXP", &[num(0.0)]), 1.0);
    approx(&call("EXP", &[num(1.0)]), std::f64::consts::E);
}

// ---- LN / LOG10 / LOG ------------------------------------------------------

#[test]
fn sheet_fn_math_ln_basic() {
    approx(&call("LN", &[num(std::f64::consts::E)]), 1.0);
    approx(&call("LN", &[num(1.0)]), 0.0);
}

#[test]
fn sheet_fn_math_ln_domain_num_error() {
    assert_eq!(call("LN", &[num(0.0)]), e(CellError::Num));
    assert_eq!(call("LN", &[num(-2.0)]), e(CellError::Num));
}

#[test]
fn sheet_fn_math_log10_basic() {
    approx(&call("LOG10", &[num(1000.0)]), 3.0);
    assert_eq!(call("LOG10", &[num(-1.0)]), e(CellError::Num));
}

#[test]
fn sheet_fn_math_log_basic_and_base() {
    approx(&call("LOG", &[num(100.0)]), 2.0); // default base 10
    approx(&call("LOG", &[num(8.0), num(2.0)]), 3.0);
}

#[test]
fn sheet_fn_math_log_edge_errors() {
    assert_eq!(call("LOG", &[num(-1.0)]), e(CellError::Num)); // x<=0
    assert_eq!(call("LOG", &[num(8.0), num(1.0)]), e(CellError::Div0)); // base 1
    assert_eq!(call("LOG", &[]), e(CellError::Value)); // arity min
}

// ---- POWER -----------------------------------------------------------------

#[test]
fn sheet_fn_math_power_basic() {
    assert_eq!(call("POWER", &[num(2.0), num(10.0)]), n(1024.0));
    approx(&call("POWER", &[num(9.0), num(0.5)]), 3.0);
}

#[test]
fn sheet_fn_math_power_edge() {
    // Negative base to a fractional power is non-real → #NUM!.
    assert_eq!(call("POWER", &[num(-1.0), num(0.5)]), e(CellError::Num));
    assert_eq!(call("POWER", &[num(2.0)]), e(CellError::Value)); // arity
}

// ---- MOD -------------------------------------------------------------------

#[test]
fn sheet_fn_math_mod_sign_follows_divisor() {
    // The Excel ruling: result sign follows the DIVISOR.
    assert_eq!(call("MOD", &[num(-3.0), num(2.0)]), n(1.0));
    assert_eq!(call("MOD", &[num(3.0), num(-2.0)]), n(-1.0));
    assert_eq!(call("MOD", &[num(5.0), num(3.0)]), n(2.0));
}

#[test]
fn sheet_fn_math_mod_div0_and_arity() {
    assert_eq!(call("MOD", &[num(3.0), num(0.0)]), e(CellError::Div0));
    assert_eq!(call("MOD", &[num(3.0)]), e(CellError::Value)); // arity
}

// ---- ROUND / ROUNDUP / ROUNDDOWN -------------------------------------------

#[test]
fn sheet_fn_math_round_half_away_from_zero() {
    assert_eq!(call("ROUND", &[num(2.5), num(0.0)]), n(3.0));
    assert_eq!(call("ROUND", &[num(-2.5), num(0.0)]), n(-3.0));
    assert_eq!(call("ROUND", &[num(2.345), num(2.0)]), n(2.35));
    // Negative digits round to tens.
    assert_eq!(call("ROUND", &[num(123.0), num(-1.0)]), n(120.0));
}

#[test]
fn sheet_fn_math_round_error_and_arity() {
    assert_eq!(
        call("ROUND", &[err(CellError::Na), num(0.0)]),
        e(CellError::Na)
    );
    assert_eq!(call("ROUND", &[num(1.0)]), e(CellError::Value)); // arity
}

#[test]
fn sheet_fn_math_roundup_basic() {
    assert_eq!(call("ROUNDUP", &[num(2.1), num(0.0)]), n(3.0));
    assert_eq!(call("ROUNDUP", &[num(-2.1), num(0.0)]), n(-3.0));
    assert_eq!(call("ROUNDUP", &[num(1.2345), num(2.0)]), n(1.24));
}

#[test]
fn sheet_fn_math_rounddown_basic() {
    assert_eq!(call("ROUNDDOWN", &[num(2.9), num(0.0)]), n(2.0));
    assert_eq!(call("ROUNDDOWN", &[num(-2.9), num(0.0)]), n(-2.0));
    assert_eq!(call("ROUNDDOWN", &[num(1.2399), num(2.0)]), n(1.23));
}

// ---- TRUNC / INT -----------------------------------------------------------

#[test]
fn sheet_fn_math_trunc_basic() {
    assert_eq!(call("TRUNC", &[num(8.9)]), n(8.0)); // default 0 digits
    assert_eq!(call("TRUNC", &[num(-8.9)]), n(-8.0)); // toward zero, not −∞
    assert_eq!(call("TRUNC", &[num(1.2345), num(2.0)]), n(1.23));
}

#[test]
fn sheet_fn_math_int_floors_toward_neg_inf() {
    assert_eq!(call("INT", &[num(8.9)]), n(8.0));
    // The ruling: INT floors toward −∞ (unlike TRUNC).
    assert_eq!(call("INT", &[num(-1.5)]), n(-2.0));
    assert_eq!(call("INT", &[num(-8.9)]), n(-9.0));
}

// ---- CEILING / FLOOR -------------------------------------------------------

#[test]
fn sheet_fn_math_ceiling_basic() {
    assert_eq!(call("CEILING", &[num(2.5), num(1.0)]), n(3.0));
    assert_eq!(call("CEILING", &[num(2.5), num(2.0)]), n(4.0));
    // Zero significance → 0.
    assert_eq!(call("CEILING", &[num(5.0), num(0.0)]), n(0.0));
}

#[test]
fn sheet_fn_math_ceiling_sign_mismatch_num_error() {
    // Classic rule: sign(x) != sign(significance) → #NUM!.
    assert_eq!(call("CEILING", &[num(-2.0), num(3.0)]), e(CellError::Num));
}

#[test]
fn sheet_fn_math_floor_basic() {
    assert_eq!(call("FLOOR", &[num(2.5), num(1.0)]), n(2.0));
    assert_eq!(call("FLOOR", &[num(2.5), num(2.0)]), n(2.0));
    // Negative number, negative significance is allowed (same sign).
    assert_eq!(call("FLOOR", &[num(-2.5), num(-1.0)]), n(-2.0));
}

#[test]
fn sheet_fn_math_floor_edge_errors() {
    // Classic FLOOR with zero significance → #DIV/0!.
    assert_eq!(call("FLOOR", &[num(5.0), num(0.0)]), e(CellError::Div0));
    // Sign mismatch → #NUM!.
    assert_eq!(call("FLOOR", &[num(2.0), num(-1.0)]), e(CellError::Num));
}

// ---- PI / trig -------------------------------------------------------------

#[test]
fn sheet_fn_math_pi_basic() {
    approx(&call("PI", &[]), std::f64::consts::PI);
    // PI takes no args (max 0) — an arg is an arity violation.
    assert_eq!(call("PI", &[num(1.0)]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_sin_basic() {
    approx(&call("SIN", &[num(0.0)]), 0.0);
    approx(&call("SIN", &[num(std::f64::consts::FRAC_PI_2)]), 1.0);
}

#[test]
fn sheet_fn_math_cos_basic() {
    approx(&call("COS", &[num(0.0)]), 1.0);
    approx(&call("COS", &[num(std::f64::consts::PI)]), -1.0);
}

#[test]
fn sheet_fn_math_tan_basic() {
    approx(&call("TAN", &[num(0.0)]), 0.0);
    approx(&call("TAN", &[num(std::f64::consts::FRAC_PI_4)]), 1.0);
}

#[test]
fn sheet_fn_math_asin_basic_and_domain() {
    approx(&call("ASIN", &[num(1.0)]), std::f64::consts::FRAC_PI_2);
    assert_eq!(call("ASIN", &[num(2.0)]), e(CellError::Num)); // |x|>1
}

#[test]
fn sheet_fn_math_acos_basic_and_domain() {
    approx(&call("ACOS", &[num(1.0)]), 0.0);
    assert_eq!(call("ACOS", &[num(-2.0)]), e(CellError::Num)); // |x|>1
}

#[test]
fn sheet_fn_math_atan_basic() {
    approx(&call("ATAN", &[num(1.0)]), std::f64::consts::FRAC_PI_4);
    approx(&call("ATAN", &[num(0.0)]), 0.0);
}

#[test]
fn sheet_fn_math_atan2_basic_and_order() {
    // Excel order is (x, y): ATAN2(1, 1) = π/4.
    approx(
        &call("ATAN2", &[num(1.0), num(1.0)]),
        std::f64::consts::FRAC_PI_4,
    );
    // ATAN2(0, 1) = π/2 (point straight up).
    approx(
        &call("ATAN2", &[num(0.0), num(1.0)]),
        std::f64::consts::FRAC_PI_2,
    );
    // Origin is undefined → #DIV/0!.
    assert_eq!(call("ATAN2", &[num(0.0), num(0.0)]), e(CellError::Div0));
}

#[test]
fn sheet_fn_math_degrees_basic() {
    approx(&call("DEGREES", &[num(std::f64::consts::PI)]), 180.0);
}

#[test]
fn sheet_fn_math_radians_basic() {
    approx(&call("RADIANS", &[num(180.0)]), std::f64::consts::PI);
}

// ---- RAND / RANDBETWEEN (deterministic under the seeded ctx) ----------------

#[test]
fn sheet_fn_math_rand_in_unit_interval() {
    for _ in 0..1000 {
        match call("RAND", &[]) {
            CellValue::Number(x) => assert!((0.0..1.0).contains(&x), "RAND out of [0,1): {x}"),
            other => panic!("RAND returned {other:?}"),
        }
    }
    // RAND takes no args.
    assert_eq!(call("RAND", &[num(1.0)]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_rand_deterministic_under_seed() {
    // Same seed → same sequence (one ctx draws repeatably).
    let c1 = EvalCtx::new(DateSystem::Date1900, cell(), 0.0, 7);
    let c2 = EvalCtx::new(DateSystem::Date1900, cell(), 0.0, 7);
    let id = sheet_core::funcs::lookup_func("RAND").unwrap();
    for _ in 0..50 {
        assert_eq!(dispatch(id, &[], &c1), dispatch(id, &[], &c2));
    }
}

#[test]
fn sheet_fn_math_randbetween_inclusive_bounds() {
    for _ in 0..2000 {
        match call("RANDBETWEEN", &[num(1.0), num(6.0)]) {
            CellValue::Number(x) => {
                assert!((1.0..=6.0).contains(&x), "out of [1,6]: {x}");
                assert_eq!(x.fract(), 0.0, "RANDBETWEEN must be integral: {x}");
            }
            other => panic!("RANDBETWEEN returned {other:?}"),
        }
    }
    // Degenerate single-value range.
    assert_eq!(call("RANDBETWEEN", &[num(5.0), num(5.0)]), n(5.0));
}

#[test]
fn sheet_fn_math_randbetween_lo_gt_hi_num_error() {
    assert_eq!(
        call("RANDBETWEEN", &[num(6.0), num(1.0)]),
        e(CellError::Num)
    );
    // Arity violation.
    assert_eq!(call("RANDBETWEEN", &[num(1.0)]), e(CellError::Value));
}
