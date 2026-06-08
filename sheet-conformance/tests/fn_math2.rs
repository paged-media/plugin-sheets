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

//! M1 math/trig (math2) family conformance (spec §7/§11, milestone M1).
//! SELF-CONTAINED direct-dispatch tests: each case resolves the function
//! name through `sheet_core::funcs::lookup_func` and routes the call through
//! [`sheet_fn::dispatch`] — the same choke point a real evaluation crosses,
//! so arity guards (`#VALUE!` on violation) are exercised end-to-end, not
//! bypassed by calling the kernel directly.
//!
//! Every function gets at least one `fn sheet_fn_math_<name>…` test (the
//! prefix the registry rows in `math2.yaml` point at, which the coverage gate
//! greps for). The cases cover happy path, coercion edge, error propagation,
//! range behavior (where `range_aware`), arity violation, and each named
//! Excel ruling: MROUND nearest-multiple / sign-mismatch `#NUM!`, EVEN/ODD
//! away-from-zero, FACT/FACTDOUBLE negative `#NUM!` + non-integer truncation,
//! COMBIN/PERMUT `k>n` `#NUM!`, GCD/LCM variadic integers, SUMSQ range
//! behavior, SQRTPI, the hyperbolics, ACOSH `x>=1` / ATANH `|x|<1` domains,
//! and BASE/DECIMAL round-trips.

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

/// A deterministic context (fixed now-serial + seed).
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

fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}
fn e(c: CellError) -> CellValue {
    CellValue::Error(c)
}

/// Assert a numeric result within a tight tolerance (transcendentals).
fn approx(got: &CellValue, want: f64) {
    match got {
        CellValue::Number(g) => assert!(
            (g - want).abs() < 1e-9,
            "expected ~{want}, got {g} (Δ={})",
            (g - want).abs()
        ),
        other => panic!("expected Number(~{want}), got {other:?}"),
    }
}

// ---- MROUND ----------------------------------------------------------------

#[test]
fn sheet_fn_math_mround_basic() {
    // Nearest multiple, half away from zero.
    assert_eq!(call("MROUND", &[num(10.0), num(3.0)]), n(9.0));
    assert_eq!(call("MROUND", &[num(-10.0), num(-3.0)]), n(-9.0));
    approx(&call("MROUND", &[num(1.3), num(0.2)]), 1.4);
    // Tie rounds away from zero.
    assert_eq!(call("MROUND", &[num(1.5), num(1.0)]), n(2.0));
    // Zero multiple -> 0; number itself a multiple -> unchanged.
    assert_eq!(call("MROUND", &[num(0.0), num(5.0)]), n(0.0));
    assert_eq!(call("MROUND", &[num(12.0), num(3.0)]), n(12.0));
}

#[test]
fn sheet_fn_math_mround_sign_mismatch_is_num() {
    // sign(number) != sign(multiple) (number non-zero) -> #NUM! (ruling).
    assert_eq!(call("MROUND", &[num(5.0), num(-2.0)]), e(CellError::Num));
    assert_eq!(call("MROUND", &[num(-5.0), num(2.0)]), e(CellError::Num));
}

#[test]
fn sheet_fn_math_mround_coercion_and_errors() {
    // Scalar text/bool coerce.
    assert_eq!(call("MROUND", &[txt("10"), boolean(true)]), n(10.0));
    // Un-parseable scalar text -> #VALUE!.
    assert_eq!(call("MROUND", &[txt("x"), num(3.0)]), e(CellError::Value));
    // Error propagation (first-error-wins).
    assert_eq!(
        call("MROUND", &[err(CellError::Na), num(3.0)]),
        e(CellError::Na)
    );
    // Arity violation -> #VALUE!.
    assert_eq!(call("MROUND", &[num(10.0)]), e(CellError::Value));
    assert_eq!(
        call("MROUND", &[num(10.0), num(3.0), num(1.0)]),
        e(CellError::Value)
    );
}

// ---- EVEN / ODD ------------------------------------------------------------

#[test]
fn sheet_fn_math_even_away_from_zero() {
    assert_eq!(call("EVEN", &[num(3.0)]), n(4.0));
    assert_eq!(call("EVEN", &[num(2.0)]), n(2.0));
    assert_eq!(call("EVEN", &[num(-1.0)]), n(-2.0));
    assert_eq!(call("EVEN", &[num(1.5)]), n(2.0));
    assert_eq!(call("EVEN", &[num(-1.5)]), n(-2.0));
    assert_eq!(call("EVEN", &[num(0.0)]), n(0.0));
    // Coercion + error + arity.
    assert_eq!(call("EVEN", &[boolean(true)]), n(2.0));
    assert_eq!(call("EVEN", &[txt("x")]), e(CellError::Value));
    assert_eq!(call("EVEN", &[err(CellError::Div0)]), e(CellError::Div0));
    assert_eq!(call("EVEN", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_odd_away_from_zero() {
    assert_eq!(call("ODD", &[num(3.0)]), n(3.0));
    assert_eq!(call("ODD", &[num(2.0)]), n(3.0));
    assert_eq!(call("ODD", &[num(-1.0)]), n(-1.0));
    assert_eq!(call("ODD", &[num(-2.0)]), n(-3.0));
    assert_eq!(call("ODD", &[num(1.5)]), n(3.0));
    assert_eq!(call("ODD", &[num(0.0)]), n(1.0));
    assert_eq!(call("ODD", &[]), e(CellError::Value));
    assert_eq!(call("ODD", &[num(1.0), num(2.0)]), e(CellError::Value));
}

// ---- FACT / FACTDOUBLE -----------------------------------------------------

#[test]
fn sheet_fn_math_fact_truncate_and_negative() {
    assert_eq!(call("FACT", &[num(0.0)]), n(1.0));
    assert_eq!(call("FACT", &[num(1.0)]), n(1.0));
    assert_eq!(call("FACT", &[num(5.0)]), n(120.0));
    // Non-integer truncates toward zero (5.9 -> 5! = 120).
    assert_eq!(call("FACT", &[num(5.9)]), n(120.0));
    // Negative -> #NUM!.
    assert_eq!(call("FACT", &[num(-1.0)]), e(CellError::Num));
    // Coercion + error + arity.
    assert_eq!(call("FACT", &[txt("4")]), n(24.0));
    assert_eq!(call("FACT", &[err(CellError::Ref)]), e(CellError::Ref));
    assert_eq!(call("FACT", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_factdouble_basecases() {
    assert_eq!(call("FACTDOUBLE", &[num(0.0)]), n(1.0));
    assert_eq!(call("FACTDOUBLE", &[num(1.0)]), n(1.0));
    // 7!! = 7*5*3*1 = 105; 8!! = 8*6*4*2 = 384.
    assert_eq!(call("FACTDOUBLE", &[num(7.0)]), n(105.0));
    assert_eq!(call("FACTDOUBLE", &[num(8.0)]), n(384.0));
    // Non-integer truncates; negative -> #NUM!.
    assert_eq!(call("FACTDOUBLE", &[num(7.5)]), n(105.0));
    assert_eq!(call("FACTDOUBLE", &[num(-2.0)]), e(CellError::Num));
    assert_eq!(call("FACTDOUBLE", &[]), e(CellError::Value));
}

// ---- COMBIN / PERMUT -------------------------------------------------------

#[test]
fn sheet_fn_math_combin_and_kgtn() {
    assert_eq!(call("COMBIN", &[num(5.0), num(2.0)]), n(10.0));
    assert_eq!(call("COMBIN", &[num(5.0), num(0.0)]), n(1.0));
    assert_eq!(call("COMBIN", &[num(5.0), num(5.0)]), n(1.0));
    assert_eq!(call("COMBIN", &[num(10.0), num(3.0)]), n(120.0));
    // Truncation of both args.
    assert_eq!(call("COMBIN", &[num(5.9), num(2.9)]), n(10.0));
    // k > n -> #NUM!; negative -> #NUM!.
    assert_eq!(call("COMBIN", &[num(2.0), num(5.0)]), e(CellError::Num));
    assert_eq!(call("COMBIN", &[num(-1.0), num(0.0)]), e(CellError::Num));
    // Coercion + error + arity.
    assert_eq!(call("COMBIN", &[txt("x"), num(2.0)]), e(CellError::Value));
    assert_eq!(
        call("COMBIN", &[err(CellError::Na), num(2.0)]),
        e(CellError::Na)
    );
    assert_eq!(call("COMBIN", &[num(5.0)]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_permut_ordered() {
    assert_eq!(call("PERMUT", &[num(5.0), num(2.0)]), n(20.0));
    assert_eq!(call("PERMUT", &[num(5.0), num(0.0)]), n(1.0));
    assert_eq!(call("PERMUT", &[num(4.0), num(4.0)]), n(24.0));
    // k > n / negative -> #NUM!.
    assert_eq!(call("PERMUT", &[num(2.0), num(5.0)]), e(CellError::Num));
    assert_eq!(call("PERMUT", &[num(3.0), num(-1.0)]), e(CellError::Num));
    assert_eq!(
        call("PERMUT", &[num(5.0), num(2.0), num(1.0)]),
        e(CellError::Value)
    );
}

// ---- GCD / LCM -------------------------------------------------------------

#[test]
fn sheet_fn_math_gcd_variadic_and_range() {
    assert_eq!(call("GCD", &[num(24.0), num(36.0)]), n(12.0));
    assert_eq!(call("GCD", &[num(24.0), num(36.0), num(48.0)]), n(12.0));
    // gcd(0, 0) == 0; gcd(0, n) == n.
    assert_eq!(call("GCD", &[num(0.0), num(0.0)]), n(0.0));
    assert_eq!(call("GCD", &[num(0.0), num(7.0)]), n(7.0));
    // Non-integer truncates.
    assert_eq!(call("GCD", &[num(12.9), num(8.0)]), n(4.0));
    // Range-aware: text/blank inside a range skip; numbers contribute.
    let cells = [
        CellValue::Number(12.0),
        CellValue::from("hi"),
        CellValue::Empty,
        CellValue::Number(18.0),
    ];
    let view = RangeView::from_slice(cell(), 1, 4, &cells);
    assert_eq!(call("GCD", &[Arg::Range(view)]), n(6.0));
    // Negative operand -> #NUM!; un-parseable scalar -> #VALUE!.
    assert_eq!(call("GCD", &[num(-4.0), num(6.0)]), e(CellError::Num));
    assert_eq!(call("GCD", &[txt("x")]), e(CellError::Value));
    assert_eq!(call("GCD", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_lcm_variadic_and_zero() {
    assert_eq!(call("LCM", &[num(4.0), num(6.0)]), n(12.0));
    assert_eq!(call("LCM", &[num(4.0), num(6.0), num(8.0)]), n(24.0));
    // A zero operand makes the result 0 (ruling).
    assert_eq!(call("LCM", &[num(0.0), num(5.0)]), n(0.0));
    // Range-aware skip of non-numerics, error cell propagation.
    let cells = [
        CellValue::Number(3.0),
        CellValue::Bool(true),
        CellValue::Number(4.0),
    ];
    let view = RangeView::from_slice(cell(), 1, 3, &cells);
    assert_eq!(call("LCM", &[Arg::Range(view)]), n(12.0));
    let err_cells = [CellValue::Number(3.0), CellValue::Error(CellError::Div0)];
    let err_view = RangeView::from_slice(cell(), 1, 2, &err_cells);
    assert_eq!(call("LCM", &[Arg::Range(err_view)]), e(CellError::Div0));
    // Negative -> #NUM!; arity.
    assert_eq!(call("LCM", &[num(-1.0)]), e(CellError::Num));
    assert_eq!(call("LCM", &[]), e(CellError::Value));
}

// ---- SUMSQ -----------------------------------------------------------------

#[test]
fn sheet_fn_math_sumsq_range_and_scalar() {
    assert_eq!(call("SUMSQ", &[num(3.0), num(4.0)]), n(25.0));
    assert_eq!(call("SUMSQ", &[num(-3.0), num(4.0)]), n(25.0));
    // Scalar coercion (text/bool participate).
    assert_eq!(call("SUMSQ", &[txt("3"), boolean(true)]), n(10.0));
    // Un-parseable scalar text -> #VALUE!.
    assert_eq!(call("SUMSQ", &[num(1.0), txt("x")]), e(CellError::Value));
    // Range: only numbers contribute (text/bool/blank skip).
    let cells = [
        CellValue::Number(3.0),
        CellValue::from("hi"),
        CellValue::Bool(true),
        CellValue::Empty,
        CellValue::Number(4.0),
    ];
    let view = RangeView::from_slice(cell(), 1, 5, &cells);
    assert_eq!(call("SUMSQ", &[Arg::Range(view)]), n(25.0));
    // Error cell inside a range propagates.
    let err_cells = [CellValue::Number(2.0), CellValue::Error(CellError::Na)];
    let err_view = RangeView::from_slice(cell(), 1, 2, &err_cells);
    assert_eq!(call("SUMSQ", &[Arg::Range(err_view)]), e(CellError::Na));
    assert_eq!(call("SUMSQ", &[]), e(CellError::Value));
}

// ---- SQRTPI ----------------------------------------------------------------

#[test]
fn sheet_fn_math_sqrtpi_domain() {
    assert_eq!(call("SQRTPI", &[num(0.0)]), n(0.0));
    approx(&call("SQRTPI", &[num(1.0)]), std::f64::consts::PI.sqrt());
    approx(
        &call("SQRTPI", &[num(4.0)]),
        (4.0 * std::f64::consts::PI).sqrt(),
    );
    // Negative -> #NUM!.
    assert_eq!(call("SQRTPI", &[num(-1.0)]), e(CellError::Num));
    assert_eq!(call("SQRTPI", &[txt("x")]), e(CellError::Value));
    assert_eq!(call("SQRTPI", &[]), e(CellError::Value));
}

// ---- hyperbolics -----------------------------------------------------------

// Each hyperbolic gets its own `fn sheet_fn_math_<name>` so the coverage gate
// resolves the per-row `tests.rust` prefix (SINH/COSH/TANH are separate rows).

#[test]
fn sheet_fn_math_sinh_total() {
    approx(&call("SINH", &[num(0.0)]), 0.0);
    approx(&call("SINH", &[num(1.0)]), 1.0_f64.sinh());
    approx(&call("SINH", &[num(-2.0)]), (-2.0_f64).sinh());
    // Coercion (bool participates) + error propagation + arity.
    approx(&call("SINH", &[boolean(true)]), 1.0_f64.sinh());
    assert_eq!(call("SINH", &[err(CellError::Div0)]), e(CellError::Div0));
    assert_eq!(call("SINH", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_cosh_total() {
    approx(&call("COSH", &[num(0.0)]), 1.0);
    approx(&call("COSH", &[num(1.0)]), 1.0_f64.cosh());
    approx(&call("COSH", &[num(-2.0)]), (-2.0_f64).cosh());
    // Un-parseable scalar text -> #VALUE!; error propagation; arity.
    assert_eq!(call("COSH", &[txt("x")]), e(CellError::Value));
    assert_eq!(call("COSH", &[err(CellError::Na)]), e(CellError::Na));
    assert_eq!(call("COSH", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_tanh_total() {
    approx(&call("TANH", &[num(0.0)]), 0.0);
    approx(&call("TANH", &[num(1.0)]), 1.0_f64.tanh());
    approx(&call("TANH", &[num(-1.0)]), (-1.0_f64).tanh());
    // Coercion (text number participates) + error propagation + arity.
    approx(&call("TANH", &[txt("1")]), 1.0_f64.tanh());
    assert_eq!(call("TANH", &[err(CellError::Na)]), e(CellError::Na));
    assert_eq!(call("TANH", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_asinh_total() {
    approx(&call("ASINH", &[num(0.0)]), 0.0);
    approx(&call("ASINH", &[num(1.0)]), 1.0_f64.asinh());
    approx(&call("ASINH", &[num(-2.0)]), (-2.0_f64).asinh());
    assert_eq!(call("ASINH", &[txt("x")]), e(CellError::Value));
    assert_eq!(call("ASINH", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_acosh_domain() {
    // Domain x >= 1; ACOSH(1) == 0.
    approx(&call("ACOSH", &[num(1.0)]), 0.0);
    approx(&call("ACOSH", &[num(10.0)]), 10.0_f64.acosh());
    // x < 1 -> #NUM!.
    assert_eq!(call("ACOSH", &[num(0.5)]), e(CellError::Num));
    assert_eq!(call("ACOSH", &[num(-1.0)]), e(CellError::Num));
    assert_eq!(call("ACOSH", &[err(CellError::Div0)]), e(CellError::Div0));
    assert_eq!(call("ACOSH", &[]), e(CellError::Value));
}

#[test]
fn sheet_fn_math_atanh_domain() {
    approx(&call("ATANH", &[num(0.0)]), 0.0);
    approx(&call("ATANH", &[num(0.5)]), 0.5_f64.atanh());
    // |x| >= 1 -> #NUM! (asymptotes).
    assert_eq!(call("ATANH", &[num(1.0)]), e(CellError::Num));
    assert_eq!(call("ATANH", &[num(-1.0)]), e(CellError::Num));
    assert_eq!(call("ATANH", &[num(2.0)]), e(CellError::Num));
    assert_eq!(call("ATANH", &[txt("x")]), e(CellError::Value));
    assert_eq!(call("ATANH", &[]), e(CellError::Value));
}

// ---- BASE / DECIMAL --------------------------------------------------------

#[test]
fn sheet_fn_math_base_render() {
    // Number -> positional text in the given radix.
    assert_eq!(call("BASE", &[num(7.0), num(2.0)]), CellValue::from("111"));
    assert_eq!(
        call("BASE", &[num(255.0), num(16.0)]),
        CellValue::from("FF")
    );
    assert_eq!(call("BASE", &[num(0.0), num(2.0)]), CellValue::from("0"));
    // min_length left-pads with '0'.
    assert_eq!(
        call("BASE", &[num(7.0), num(2.0), num(8.0)]),
        CellValue::from("00000111")
    );
    // min_length shorter than the rendering does not truncate.
    assert_eq!(
        call("BASE", &[num(255.0), num(16.0), num(1.0)]),
        CellValue::from("FF")
    );
    // Truncation of number/radix.
    assert_eq!(call("BASE", &[num(7.9), num(2.0)]), CellValue::from("111"));
    // Domain: negative number, out-of-range radix, bad min_length -> #NUM!.
    assert_eq!(call("BASE", &[num(-1.0), num(2.0)]), e(CellError::Num));
    assert_eq!(call("BASE", &[num(7.0), num(1.0)]), e(CellError::Num));
    assert_eq!(call("BASE", &[num(7.0), num(37.0)]), e(CellError::Num));
    assert_eq!(
        call("BASE", &[num(7.0), num(2.0), num(-1.0)]),
        e(CellError::Num)
    );
    // Coercion + error + arity.
    assert_eq!(call("BASE", &[txt("x"), num(2.0)]), e(CellError::Value));
    assert_eq!(
        call("BASE", &[err(CellError::Ref), num(2.0)]),
        e(CellError::Ref)
    );
    assert_eq!(call("BASE", &[num(7.0)]), e(CellError::Value));
    assert_eq!(
        call("BASE", &[num(7.0), num(2.0), num(8.0), num(1.0)]),
        e(CellError::Value)
    );
}

#[test]
fn sheet_fn_math_decimal_parse() {
    // Positional text in radix -> number.
    assert_eq!(call("DECIMAL", &[txt("111"), num(2.0)]), n(7.0));
    assert_eq!(call("DECIMAL", &[txt("FF"), num(16.0)]), n(255.0));
    // Case-insensitive; whitespace trimmed.
    assert_eq!(call("DECIMAL", &[txt("ff"), num(16.0)]), n(255.0));
    assert_eq!(call("DECIMAL", &[txt("  Z "), num(36.0)]), n(35.0));
    // BASE/DECIMAL round-trip.
    assert_eq!(call("DECIMAL", &[txt("00000111"), num(2.0)]), n(7.0));
    // Domain: out-of-range radix, invalid digit, empty -> #NUM!.
    assert_eq!(call("DECIMAL", &[txt("10"), num(1.0)]), e(CellError::Num));
    assert_eq!(call("DECIMAL", &[txt("2"), num(2.0)]), e(CellError::Num));
    assert_eq!(call("DECIMAL", &[txt(""), num(16.0)]), e(CellError::Num));
    // Error propagation + arity.
    assert_eq!(
        call("DECIMAL", &[err(CellError::Na), num(16.0)]),
        e(CellError::Na)
    );
    assert_eq!(call("DECIMAL", &[txt("FF")]), e(CellError::Value));
    assert_eq!(
        call("DECIMAL", &[txt("FF"), num(16.0), num(1.0)]),
        e(CellError::Value)
    );
}
