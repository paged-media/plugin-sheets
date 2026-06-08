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

//! M1 financial (fin) family conformance (spec §7/§11, milestone M1).
//! SELF-CONTAINED direct-dispatch tests: each case resolves the function name
//! through `sheet_core::funcs::lookup_func` and routes the call through the
//! FROZEN [`sheet_fn::dispatch`] — the same choke point a real evaluation
//! crosses, so the generated arity guard (`#VALUE!` on violation) is
//! exercised end-to-end rather than bypassed by calling the kernel directly.
//!
//! Every function gets at least one `fn sheet_fn_fin_<name>…` test (the
//! prefix the `fin.yaml` rows point at, which the coverage gate greps for).
//! The cases cover happy path, coercion edge, error propagation, range
//! behavior (where `range_aware`), arity violation, the Excel sign convention
//! (outflows negative), and each named ruling:
//!
//! - the annuity sign convention + the closed forms cross-checked against the
//!   Microsoft function-doc values (PMT/FV/PV/IPMT/PPMT/RATE/NPER);
//! - **NPV discounts the FIRST value one full period** (end-of-period) and
//!   scans ranges skipping non-numerics while propagating an error cell;
//! - IRR/XIRR Newton+bisection convergence and the no-root `#NUM!`;
//! - **XNPV/XIRR Actual/365** dated flows, date-before-first `#NUM!`,
//!   value/date count mismatch `#NUM!`;
//! - MIRR needs an inflow AND an outflow (else `#DIV/0!`);
//! - the depreciation domain guards (SLN `#DIV/0!`; SYD/DDB/DB `#NUM!`),
//!   **DB's 3-decimal-rounded rate** quirk and its partial first/last
//!   periods, and **DDB's factor-2 default** that never dips below salvage.

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

/// A deterministic context (fixed now-serial + seed; fin kernels ignore it).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cell(), 45000.5, 42)
}

/// Dispatch a function by registry name with the given args (the scalar door).
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

fn e(c: CellError) -> CellValue {
    CellValue::Error(c)
}

/// Build a single-row range arg over an owned cell buffer.
fn range_row(cells: &[CellValue]) -> RangeView<'_> {
    RangeView::from_slice(cell(), 1, cells.len() as u32, cells)
}

/// Assert a numeric result within a tolerance.
fn approx(got: &CellValue, want: f64, tol: f64) {
    match got {
        CellValue::Number(g) => assert!(
            (g - want).abs() <= tol,
            "expected ~{want} (±{tol}), got {g} (Δ={})",
            (g - want).abs()
        ),
        other => panic!("expected Number(~{want}), got {other:?}"),
    }
}

/// Tight tolerance for closed-form results; loose for iterative roots.
const TOL: f64 = 1e-7;
const ITER_TOL: f64 = 1e-6;

// ---- PMT -------------------------------------------------------------------

#[test]
fn sheet_fn_fin_pmt_loan_and_savings() {
    // =PMT(0.08/12,10,10000) -> -1037.03 (Microsoft docs): a LOAN of +10000
    // amortizes to a NEGATIVE payment (you pay it out).
    approx(
        &call("PMT", &[num(0.08 / 12.0), num(10.0), num(10000.0)]),
        -1037.0320893591636,
        TOL,
    );
    // Savings goal: deposit toward +1000 fv over 5 periods -> negative pmt.
    approx(
        &call("PMT", &[num(0.1), num(5.0), num(0.0), num(1000.0)]),
        -163.79748079474477,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_pmt_zero_rate() {
    // r=0 is the linear -(pv+fv)/nper.
    approx(
        &call("PMT", &[num(0.0), num(10.0), num(1000.0)]),
        -100.0,
        TOL,
    );
    approx(
        &call("PMT", &[num(0.0), num(5.0), num(0.0), num(1000.0)]),
        -200.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_pmt_type_flag() {
    // Annuity-due (type=1) payment is smaller in magnitude than ordinary.
    let ord = call(
        "PMT",
        &[num(0.1), num(5.0), num(10000.0), num(0.0), num(0.0)],
    );
    let due = call(
        "PMT",
        &[num(0.1), num(5.0), num(10000.0), num(0.0), num(1.0)],
    );
    match (ord, due) {
        (CellValue::Number(o), CellValue::Number(d)) => {
            // due = ord / (1+r): begin-of-period payments accrue less interest.
            assert!(
                (d - o / 1.1).abs() < 1e-6,
                "type=1 should equal type=0 / (1+r); o={o} d={d}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn sheet_fn_fin_pmt_coercion_and_errors() {
    // Scalar text/bool coerce.
    approx(
        &call("PMT", &[txt("0"), num(10.0), num(1000.0)]),
        -100.0,
        TOL,
    );
    // Un-parseable scalar text -> #VALUE!.
    assert_eq!(
        call("PMT", &[txt("x"), num(10.0), num(1000.0)]),
        e(CellError::Value)
    );
    // First-error-wins.
    assert_eq!(
        call("PMT", &[num(0.1), err(CellError::Div0), num(1000.0)]),
        e(CellError::Div0)
    );
}

#[test]
fn sheet_fn_fin_pmt_arity() {
    // min 3, max 5 — too few / too many -> #VALUE! (generated arity guard).
    assert_eq!(call("PMT", &[num(0.1), num(5.0)]), e(CellError::Value));
    assert_eq!(
        call(
            "PMT",
            &[num(0.1), num(5.0), num(1.0), num(0.0), num(0.0), num(9.0)]
        ),
        e(CellError::Value)
    );
}

// ---- IPMT / PPMT -----------------------------------------------------------

#[test]
fn sheet_fn_fin_ipmt_interest_portion() {
    // =IPMT(0.1,1,3,8000) -> -800 (first-period interest = 8000*0.1, paid out).
    approx(
        &call("IPMT", &[num(0.1), num(1.0), num(3.0), num(8000.0)]),
        -800.0,
        TOL,
    );
    // =IPMT(0.1,3,3,8000) -> -292.4471 (Microsoft docs).
    approx(
        &call("IPMT", &[num(0.1), num(3.0), num(3.0), num(8000.0)]),
        -292.4471299093658,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_ipmt_type1_first_period_zero_interest() {
    // Begin-of-period: the FIRST payment accrues no interest (made before any
    // accrues) -> IPMT(.,1,.,.,.,1) = 0.
    approx(
        &call(
            "IPMT",
            &[
                num(0.1),
                num(1.0),
                num(3.0),
                num(8000.0),
                num(0.0),
                num(1.0),
            ],
        ),
        0.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_ipmt_per_out_of_range_is_num() {
    // per < 1 or per > nper -> #NUM!.
    assert_eq!(
        call("IPMT", &[num(0.1), num(0.0), num(3.0), num(8000.0)]),
        e(CellError::Num)
    );
    assert_eq!(
        call("IPMT", &[num(0.1), num(4.0), num(3.0), num(8000.0)]),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_ipmt_arity_and_error_prop() {
    // min 4 — too few -> #VALUE!.
    assert_eq!(
        call("IPMT", &[num(0.1), num(1.0), num(3.0)]),
        e(CellError::Value)
    );
    assert_eq!(
        call("IPMT", &[num(0.1), num(1.0), num(3.0), err(CellError::Na)]),
        e(CellError::Na)
    );
}

#[test]
fn sheet_fn_fin_ppmt_principal_portion() {
    // PPMT = PMT - IPMT. =PPMT(0.1,1,3,8000) -> -2416.9184 (Microsoft docs).
    approx(
        &call("PPMT", &[num(0.1), num(1.0), num(3.0), num(8000.0)]),
        -2416.9184290030184,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_ppmt_identity_pmt_minus_ipmt() {
    // The amortization identity: for every period, IPMT + PPMT == PMT.
    let (r, n, pv) = (num(0.08 / 12.0), num(36.0), num(20000.0));
    let pmt = call("PMT", &[r_clone(&r), r_clone(&n), r_clone(&pv)]);
    for per in 1..=3u32 {
        let ip = call(
            "IPMT",
            &[r_clone(&r), num(per as f64), r_clone(&n), r_clone(&pv)],
        );
        let pp = call(
            "PPMT",
            &[r_clone(&r), num(per as f64), r_clone(&n), r_clone(&pv)],
        );
        if let (CellValue::Number(p), CellValue::Number(i), CellValue::Number(pr)) =
            (&pmt, &ip, &pp)
        {
            assert!((i + pr - p).abs() < 1e-7, "period {per}: IPMT+PPMT != PMT");
        } else {
            panic!("non-numeric pmt/ipmt/ppmt");
        }
    }
}

#[test]
fn sheet_fn_fin_ppmt_per_out_of_range_is_num() {
    assert_eq!(
        call("PPMT", &[num(0.1), num(4.0), num(3.0), num(8000.0)]),
        e(CellError::Num)
    );
}

// A tiny helper to re-make a scalar Arg by value (Args aren't Copy).
fn r_clone(a: &Arg) -> Arg<'static> {
    match a {
        Arg::Scalar(v) => Arg::Scalar(v.clone()),
        Arg::Range(_) => panic!("r_clone only clones scalars"),
    }
}

// ---- FV / PV ---------------------------------------------------------------

#[test]
fn sheet_fn_fin_fv_annuity_due() {
    // =FV(0.06/12,10,-200,-500,1) -> 2581.40 (Microsoft docs): negative
    // deposits accrue to a POSITIVE future value.
    approx(
        &call(
            "FV",
            &[
                num(0.06 / 12.0),
                num(10.0),
                num(-200.0),
                num(-500.0),
                num(1.0),
            ],
        ),
        2581.4033740601362,
        TOL,
    );
    // Single-period compounding: FV(0.1,1,0,-100) -> 110.
    approx(
        &call("FV", &[num(0.1), num(1.0), num(0.0), num(-100.0)]),
        110.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_fv_zero_rate_and_coercion() {
    approx(&call("FV", &[num(0.0), num(5.0), num(-100.0)]), 500.0, TOL);
    // bool fv coerces (FALSE -> 0).
    approx(
        &call("FV", &[num(0.0), num(5.0), num(-100.0), boolean(false)]),
        500.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_fv_arity_and_error_prop() {
    assert_eq!(call("FV", &[num(0.1), num(5.0)]), e(CellError::Value));
    assert_eq!(
        call("FV", &[err(CellError::Div0), num(5.0), num(-100.0)]),
        e(CellError::Div0)
    );
}

#[test]
fn sheet_fn_fin_pv_annuity() {
    // =PV(0.08/12,240,500) -> -59777.15 (Microsoft docs): the PV of positive
    // receipts is NEGATIVE (what you would pay to acquire them).
    approx(
        &call("PV", &[num(0.08 / 12.0), num(240.0), num(500.0)]),
        -59777.14585118777,
        TOL,
    );
    // Single period: PV(0.1,1,0,-110) -> 100.
    approx(
        &call("PV", &[num(0.1), num(1.0), num(0.0), num(-110.0)]),
        100.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_pv_zero_rate_and_arity() {
    approx(&call("PV", &[num(0.0), num(5.0), num(-100.0)]), 500.0, TOL);
    assert_eq!(call("PV", &[num(0.1), num(5.0)]), e(CellError::Value));
}

#[test]
fn sheet_fn_fin_pv_coercion_and_error() {
    approx(&call("PV", &[txt("0"), num(5.0), num(-100.0)]), 500.0, TOL);
    assert_eq!(
        call("PV", &[num(0.1), num(5.0), txt("nope")]),
        e(CellError::Value)
    );
}

// ---- RATE ------------------------------------------------------------------

#[test]
fn sheet_fn_fin_rate_newton_converges() {
    // One period: -100 grows to 110 -> 10% (a clean root the sign-corrected
    // residual must find; the pre-fix `fv_of+fv` form found the WRONG root).
    approx(
        &call("RATE", &[num(1.0), num(0.0), num(-100.0), num(110.0)]),
        0.1,
        ITER_TOL,
    );
    // Two periods: -100 -> 121 -> 10%.
    approx(
        &call("RATE", &[num(2.0), num(0.0), num(-100.0), num(121.0)]),
        0.1,
        ITER_TOL,
    );
    // Loan rate (Microsoft docs RATE(48,-200,8000) -> ~0.0077 monthly).
    approx(
        &call("RATE", &[num(48.0), num(-200.0), num(8000.0)]),
        0.0077014724882,
        ITER_TOL,
    );
}

#[test]
fn sheet_fn_fin_rate_guess_argument() {
    // An explicit guess reaches the same root.
    approx(
        &call(
            "RATE",
            &[
                num(1.0),
                num(0.0),
                num(-100.0),
                num(110.0),
                num(0.0),
                num(0.5),
            ],
        ),
        0.1,
        ITER_TOL,
    );
}

#[test]
fn sheet_fn_fin_rate_non_convergence_is_num() {
    // Newton wanders without ever satisfying |f| < 1e-7 within 40 steps for
    // these pathological annuity parameters -> #NUM! (Excel's behavior on a
    // RATE that does not converge).
    assert_eq!(
        call("RATE", &[num(10.0), num(100.0), num(100.0), num(0.0)]),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_rate_arity_and_error_prop() {
    assert_eq!(call("RATE", &[num(1.0), num(0.0)]), e(CellError::Value));
    assert_eq!(
        call(
            "RATE",
            &[num(1.0), err(CellError::Value), num(-100.0), num(110.0)]
        ),
        e(CellError::Value)
    );
}

// ---- NPER ------------------------------------------------------------------

#[test]
fn sheet_fn_fin_nper_zero_rate_linear() {
    // r=0: -(pv+fv)/pmt. -(-1000+2000)/-100 = 10.
    approx(
        &call("NPER", &[num(0.0), num(-100.0), num(-1000.0), num(2000.0)]),
        10.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_nper_logarithmic() {
    // =NPER(0.12/12,-100,-1000,10000,1) -> 59.6738657 (Microsoft docs).
    approx(
        &call(
            "NPER",
            &[
                num(0.12 / 12.0),
                num(-100.0),
                num(-1000.0),
                num(10000.0),
                num(1.0),
            ],
        ),
        59.67386567429457,
        1e-6,
    );
}

#[test]
fn sheet_fn_fin_nper_impossible_is_num() {
    // r=0, pmt=0 -> division by zero in the linear branch -> #NUM!.
    assert_eq!(
        call("NPER", &[num(0.0), num(0.0), num(-1000.0), num(2000.0)]),
        e(CellError::Num)
    );
    // A log of a non-positive ratio -> #NUM! (via `finite`): a positive debt
    // (pv) with a negative "payment" (you keep borrowing) never amortizes.
    assert_eq!(
        call("NPER", &[num(0.1), num(-50.0), num(1000.0)]),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_nper_arity_and_coercion() {
    assert_eq!(call("NPER", &[num(0.1), num(-100.0)]), e(CellError::Value));
    approx(
        &call("NPER", &[num(0.0), txt("-100"), num(-1000.0), num(2000.0)]),
        10.0,
        TOL,
    );
}

// ---- NPV -------------------------------------------------------------------

#[test]
fn sheet_fn_fin_npv_first_flow_discounted_one_period() {
    // RULING: NPV discounts the FIRST value one full period. =NPV(0.1,110)
    // -> 110/1.1 = 100 (not 110).
    approx(&call("NPV", &[num(0.1), num(110.0)]), 100.0, TOL);
    // The Microsoft idiom v0 + NPV(rate, v1..): here flat as a 4-flow stream.
    approx(
        &call(
            "NPV",
            &[
                num(0.1),
                num(-10000.0),
                num(3000.0),
                num(4200.0),
                num(6800.0),
            ],
        ),
        1188.4434123352207,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_npv_range_skips_nonnumeric() {
    // RULING: range cells skip non-numerics, but an error cell propagates.
    let cells = [
        CellValue::Number(1.0),
        CellValue::from("x"),
        CellValue::Number(3.0),
    ];
    // rate=0 -> plain sum of the numeric cells = 4.
    approx(
        &call("NPV", &[num(0.0), Arg::Range(range_row(&cells))]),
        4.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_npv_range_error_propagates() {
    let cells = [CellValue::Number(1.0), CellValue::Error(CellError::Na)];
    assert_eq!(
        call("NPV", &[num(0.1), Arg::Range(range_row(&cells))]),
        e(CellError::Na)
    );
}

#[test]
fn sheet_fn_fin_npv_rate_negative_one_and_arity() {
    assert_eq!(call("NPV", &[num(-1.0), num(100.0)]), e(CellError::Num));
    // min 2 -> a lone rate is #VALUE!.
    assert_eq!(call("NPV", &[num(0.1)]), e(CellError::Value));
}

// ---- IRR -------------------------------------------------------------------

#[test]
fn sheet_fn_fin_irr_simple_and_doc() {
    // -100 then 110 -> 10%.
    let c1 = [CellValue::Number(-100.0), CellValue::Number(110.0)];
    approx(&call("IRR", &[Arg::Range(range_row(&c1))]), 0.1, ITER_TOL);
    // Microsoft docs: {-70000,12000,15000,18000,21000} -> -2.12%.
    let c2 = [-70000.0, 12000.0, 15000.0, 18000.0, 21000.0].map(CellValue::Number);
    approx(
        &call("IRR", &[Arg::Range(range_row(&c2))]),
        -0.02124484827341,
        ITER_TOL,
    );
    // with 26000 -> 8.66%.
    let c3 = [-70000.0, 12000.0, 15000.0, 18000.0, 21000.0, 26000.0].map(CellValue::Number);
    approx(
        &call("IRR", &[Arg::Range(range_row(&c3))]),
        0.08663094803625,
        ITER_TOL,
    );
}

#[test]
fn sheet_fn_fin_irr_guess_argument() {
    let c = [CellValue::Number(-100.0), CellValue::Number(110.0)];
    approx(
        &call("IRR", &[Arg::Range(range_row(&c)), num(0.5)]),
        0.1,
        ITER_TOL,
    );
}

#[test]
fn sheet_fn_fin_irr_no_root_is_num() {
    // All-positive flows have no IRR -> #NUM!.
    let c = [CellValue::Number(100.0), CellValue::Number(110.0)];
    assert_eq!(call("IRR", &[Arg::Range(range_row(&c))]), e(CellError::Num));
    // Fewer than two flows -> #NUM!.
    let single = [CellValue::Number(-100.0)];
    assert_eq!(
        call("IRR", &[Arg::Range(range_row(&single))]),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_irr_range_error_propagates() {
    let c = [CellValue::Number(-100.0), CellValue::Error(CellError::Div0)];
    assert_eq!(
        call("IRR", &[Arg::Range(range_row(&c))]),
        e(CellError::Div0)
    );
}

// ---- XNPV ------------------------------------------------------------------

#[test]
fn sheet_fn_fin_xnpv_actual_365() {
    // RULING: Actual/365 from the first date. Flows exactly one year apart at
    // rate 0 -> plain sum.
    let v = [-100.0, 50.0, 60.0].map(CellValue::Number);
    let d = [1.0, 366.0, 731.0].map(CellValue::Number);
    approx(
        &call(
            "XNPV",
            &[
                num(0.0),
                Arg::Range(range_row(&v)),
                Arg::Range(range_row(&d)),
            ],
        ),
        10.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_xnpv_date_before_first_is_num() {
    let v = [-100.0, 110.0].map(CellValue::Number);
    let d = [10.0, 5.0].map(CellValue::Number); // second date precedes first
    assert_eq!(
        call(
            "XNPV",
            &[
                num(0.1),
                Arg::Range(range_row(&v)),
                Arg::Range(range_row(&d))
            ]
        ),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_xnpv_count_mismatch_and_rate_guard() {
    // value/date count mismatch -> #NUM!.
    let v = [-100.0, 50.0, 60.0].map(CellValue::Number);
    let d = [1.0, 366.0].map(CellValue::Number);
    assert_eq!(
        call(
            "XNPV",
            &[
                num(0.1),
                Arg::Range(range_row(&v)),
                Arg::Range(range_row(&d))
            ]
        ),
        e(CellError::Num)
    );
    // rate <= -1 -> #NUM!.
    let v2 = [-100.0, 110.0].map(CellValue::Number);
    let d2 = [1.0, 366.0].map(CellValue::Number);
    assert_eq!(
        call(
            "XNPV",
            &[
                num(-1.0),
                Arg::Range(range_row(&v2)),
                Arg::Range(range_row(&d2))
            ]
        ),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_xnpv_arity() {
    // Exactly 3 args.
    assert_eq!(call("XNPV", &[num(0.1), num(1.0)]), e(CellError::Value));
}

// ---- XIRR ------------------------------------------------------------------

#[test]
fn sheet_fn_fin_xirr_actual_365() {
    // One year apart -> XIRR equals the simple annual rate, 10%.
    let v = [-100.0, 110.0].map(CellValue::Number);
    let d = [1.0, 366.0].map(CellValue::Number);
    approx(
        &call(
            "XIRR",
            &[Arg::Range(range_row(&v)), Arg::Range(range_row(&d))],
        ),
        0.1,
        ITER_TOL,
    );
    // Two years apart, -100 -> 121 -> 10%.
    let v2 = [-100.0, 121.0].map(CellValue::Number);
    let d2 = [1.0, 731.0].map(CellValue::Number);
    approx(
        &call(
            "XIRR",
            &[Arg::Range(range_row(&v2)), Arg::Range(range_row(&d2))],
        ),
        0.1,
        ITER_TOL,
    );
}

#[test]
fn sheet_fn_fin_xirr_guess_and_errors() {
    let v = [-100.0, 110.0].map(CellValue::Number);
    let d = [1.0, 366.0].map(CellValue::Number);
    approx(
        &call(
            "XIRR",
            &[
                Arg::Range(range_row(&v)),
                Arg::Range(range_row(&d)),
                num(0.2),
            ],
        ),
        0.1,
        ITER_TOL,
    );
    // date before first -> #NUM!.
    let d2 = [10.0, 5.0].map(CellValue::Number);
    assert_eq!(
        call(
            "XIRR",
            &[Arg::Range(range_row(&v)), Arg::Range(range_row(&d2))]
        ),
        e(CellError::Num)
    );
    // min 2 args.
    assert_eq!(
        call("XIRR", &[Arg::Range(range_row(&v))]),
        e(CellError::Value)
    );
}

// ---- MIRR ------------------------------------------------------------------

#[test]
fn sheet_fn_fin_mirr_basic_and_doc() {
    // Clean: -100 then 121 two periods at equal rates -> 10%.
    let c1 = [-100.0, 0.0, 121.0].map(CellValue::Number);
    approx(
        &call("MIRR", &[Arg::Range(range_row(&c1)), num(0.1), num(0.1)]),
        0.1,
        TOL,
    );
    // Microsoft docs: 0.126094.
    let c2 = [-120000.0, 39000.0, 30000.0, 21000.0, 37000.0, 46000.0].map(CellValue::Number);
    approx(
        &call("MIRR", &[Arg::Range(range_row(&c2)), num(0.1), num(0.12)]),
        0.12609413036591,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_mirr_needs_inflow_and_outflow() {
    // All-positive (no outflow) -> #DIV/0!.
    let pos = [100.0, 50.0, 121.0].map(CellValue::Number);
    assert_eq!(
        call("MIRR", &[Arg::Range(range_row(&pos)), num(0.1), num(0.1)]),
        e(CellError::Div0)
    );
    // Single flow -> #DIV/0!.
    let single = [CellValue::Number(-100.0)];
    assert_eq!(
        call(
            "MIRR",
            &[Arg::Range(range_row(&single)), num(0.1), num(0.1)]
        ),
        e(CellError::Div0)
    );
}

#[test]
fn sheet_fn_fin_mirr_range_error_and_arity() {
    let c = [CellValue::Number(-100.0), CellValue::Error(CellError::Na)];
    assert_eq!(
        call("MIRR", &[Arg::Range(range_row(&c)), num(0.1), num(0.1)]),
        e(CellError::Na)
    );
    // Exactly 3 args.
    assert_eq!(
        call("MIRR", &[Arg::Range(range_row(&c)), num(0.1)]),
        e(CellError::Value)
    );
}

// ---- SLN -------------------------------------------------------------------

#[test]
fn sheet_fn_fin_sln_straight_line() {
    // =SLN(30000,7500,10) -> 2250 (Microsoft docs).
    approx(
        &call("SLN", &[num(30000.0), num(7500.0), num(10.0)]),
        2250.0,
        TOL,
    );
    approx(&call("SLN", &[num(1000.0), num(0.0), num(4.0)]), 250.0, TOL);
}

#[test]
fn sheet_fn_fin_sln_zero_life_is_div0() {
    assert_eq!(
        call("SLN", &[num(1000.0), num(100.0), num(0.0)]),
        e(CellError::Div0)
    );
}

#[test]
fn sheet_fn_fin_sln_coercion_arity_error() {
    approx(
        &call("SLN", &[txt("30000"), num(7500.0), num(10.0)]),
        2250.0,
        TOL,
    );
    assert_eq!(call("SLN", &[num(1000.0), num(100.0)]), e(CellError::Value));
    assert_eq!(
        call("SLN", &[err(CellError::Value), num(100.0), num(5.0)]),
        e(CellError::Value)
    );
}

// ---- SYD -------------------------------------------------------------------

#[test]
fn sheet_fn_fin_syd_sum_of_years_digits() {
    // =SYD(30000,7500,10,1) -> 4090.91; period 10 -> 409.09 (Microsoft docs).
    approx(
        &call("SYD", &[num(30000.0), num(7500.0), num(10.0), num(1.0)]),
        4090.909090909091,
        TOL,
    );
    approx(
        &call("SYD", &[num(30000.0), num(7500.0), num(10.0), num(10.0)]),
        409.0909090909091,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_syd_domain_guards_num() {
    // per outside 1..=life -> #NUM!.
    assert_eq!(
        call("SYD", &[num(30000.0), num(7500.0), num(10.0), num(11.0)]),
        e(CellError::Num)
    );
    assert_eq!(
        call("SYD", &[num(30000.0), num(7500.0), num(10.0), num(0.0)]),
        e(CellError::Num)
    );
    // life <= 0 -> #NUM!.
    assert_eq!(
        call("SYD", &[num(30000.0), num(7500.0), num(0.0), num(1.0)]),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_syd_arity_and_error() {
    assert_eq!(
        call("SYD", &[num(30000.0), num(7500.0), num(10.0)]),
        e(CellError::Value)
    );
    assert_eq!(
        call(
            "SYD",
            &[num(30000.0), num(7500.0), err(CellError::Div0), num(1.0)]
        ),
        e(CellError::Div0)
    );
}

// ---- DDB -------------------------------------------------------------------

#[test]
fn sheet_fn_fin_ddb_factor_two_default() {
    // RULING: factor defaults to 2 (double-declining). =DDB(2400,300,10,1)
    // -> 480 (= 2400 * 2/10); period 2 -> 384 (Microsoft docs).
    approx(
        &call("DDB", &[num(2400.0), num(300.0), num(10.0), num(1.0)]),
        480.0,
        TOL,
    );
    approx(
        &call("DDB", &[num(2400.0), num(300.0), num(10.0), num(2.0)]),
        384.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_ddb_never_below_salvage() {
    // Across the full life the running book never dips below salvage; the last
    // period's charge is small and positive.
    match call("DDB", &[num(2400.0), num(300.0), num(10.0), num(10.0)]) {
        CellValue::Number(d) => {
            assert!((0.0..50.0).contains(&d), "late DDB charge {d} out of band")
        }
        other => panic!("unexpected {other:?}"),
    }
    // Explicit factor 1.5: 2400 * 1.5/10 = 360.
    approx(
        &call(
            "DDB",
            &[num(2400.0), num(300.0), num(10.0), num(1.0), num(1.5)],
        ),
        360.0,
        TOL,
    );
}

#[test]
fn sheet_fn_fin_ddb_domain_guards_num() {
    // period > life -> #NUM!.
    assert_eq!(
        call("DDB", &[num(2400.0), num(300.0), num(10.0), num(11.0)]),
        e(CellError::Num)
    );
    // negative cost -> #NUM!.
    assert_eq!(
        call("DDB", &[num(-2400.0), num(300.0), num(10.0), num(1.0)]),
        e(CellError::Num)
    );
    // factor <= 0 -> #NUM!.
    assert_eq!(
        call(
            "DDB",
            &[num(2400.0), num(300.0), num(10.0), num(1.0), num(0.0)]
        ),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_ddb_arity_and_error() {
    assert_eq!(
        call("DDB", &[num(2400.0), num(300.0), num(10.0)]),
        e(CellError::Value)
    );
    assert_eq!(
        call(
            "DDB",
            &[num(2400.0), err(CellError::Na), num(10.0), num(1.0)]
        ),
        e(CellError::Na)
    );
}

// ---- DB --------------------------------------------------------------------

#[test]
fn sheet_fn_fin_db_three_decimal_rounded_rate() {
    // RULING: DB rounds the rate to 3 decimals. Microsoft docs
    // DB(1000000,100000,6,1,7) -> 186083.33; period 2 -> 259639.42; period 7
    // (the trailing partial year) -> 15845.10.
    approx(
        &call("DB", &[num(1e6), num(1e5), num(6.0), num(1.0), num(7.0)]),
        186083.33333333334,
        1e-4,
    );
    approx(
        &call("DB", &[num(1e6), num(1e5), num(6.0), num(2.0), num(7.0)]),
        259639.41666666666,
        1e-4,
    );
    approx(
        &call("DB", &[num(1e6), num(1e5), num(6.0), num(7.0), num(7.0)]),
        15845.098473848071,
        1e-4,
    );
}

#[test]
fn sheet_fn_fin_db_default_month_twelve() {
    // month defaults to 12: first-period charge = cost * rate (full year).
    // rate = ROUND(1 - (1e5/1e6)^(1/6), 3) = 0.319 -> 1e6 * 0.319 = 319000.
    approx(
        &call("DB", &[num(1e6), num(1e5), num(6.0), num(1.0)]),
        319000.0,
        1e-6,
    );
}

#[test]
fn sheet_fn_fin_db_domain_guards_num() {
    // period > life + 1 -> #NUM!.
    assert_eq!(
        call("DB", &[num(1e6), num(1e5), num(6.0), num(8.0), num(7.0)]),
        e(CellError::Num)
    );
    // month outside 1..=12 -> #NUM!.
    assert_eq!(
        call("DB", &[num(1e6), num(1e5), num(6.0), num(1.0), num(13.0)]),
        e(CellError::Num)
    );
    // cost <= 0 -> #NUM!.
    assert_eq!(
        call("DB", &[num(0.0), num(1e5), num(6.0), num(1.0)]),
        e(CellError::Num)
    );
}

#[test]
fn sheet_fn_fin_db_arity_and_error() {
    assert_eq!(
        call("DB", &[num(1e6), num(1e5), num(6.0)]),
        e(CellError::Value)
    );
    assert_eq!(
        call("DB", &[err(CellError::Div0), num(1e5), num(6.0), num(1.0)]),
        e(CellError::Div0)
    );
}
