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

//! M1 date-arithmetic family conformance (spec §7, §11; registry
//! `sheet.fn.date.*` M1 additions — the `date2.rs` kernels). Two layers, both
//! self-contained:
//!
//! 1. **Direct-dispatch tests.** Each case resolves a [`sheet_core::FuncId`]
//!    via `lookup_func`, builds the frozen `&[Arg]` slice, and routes through
//!    the registry-generated [`sheet_fn::dispatch`] — exercising the arity
//!    guard and kernel wiring exactly as `sheet-calc` will. Every function
//!    carries at least one test fn whose name starts with the registry pointer
//!    prefix `sheet_fn_date_<name>` so the §12.2 coverage gate finds it.
//! 2. **Corpus self-verification.** The `*.golden.tsv` cases under
//!    `corpus/fn-corpus/date2/` are replayed through the FROZEN
//!    [`sheet_calc::Engine`] (the same load→enter→recalc→project path
//!    `corpus_runner.rs` drives for the other families) and asserted against
//!    each golden `expected`. This pins the corpus expecteds to the live
//!    implementation in this owned test file (the shared `corpus_runner.rs`
//!    does not yet register a `date2` family directory).
//!
//! The fixed `EvalCtx` (1900 system, `now_serial = 45000.5`, seed 42) mirrors
//! `fn_date.rs`. Serial constants are the well-known 1900-system values the
//! FMT track's `serial_date` corpus pins (44197 = 2021-01-01).

use sheet_calc::{Engine, EngineConfig, SetInput};
use sheet_conformance::{load_corpus, CorpusCase};
use sheet_core::{CellError, CellRef, CellValue, DateSystem, SheetId, SheetModel};
use sheet_fn::{coerce, dispatch, Arg, EvalCtx, RangeView};

// ============================================================ shared plumbing

/// The fixed evaluation context (1900 system, deterministic clock).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cell(), 45000.5, 42)
}

/// A context in the 1904 system (for the date-system-honoring cases).
fn ctx1904() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1904, cell(), 45000.5, 42)
}

fn cell() -> CellRef {
    CellRef {
        sheet: 0,
        row: 0,
        col: 0,
        row_abs: false,
        col_abs: false,
    }
}

/// Dispatch `name(args)` under the given context.
fn call(name: &str, args: &[Arg], ctx: &EvalCtx) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("lookup_func({name}) returned None"));
    dispatch(id, args, ctx)
}

fn n(x: f64) -> Arg<'static> {
    Arg::Scalar(CellValue::Number(x))
}

fn t(s: &str) -> Arg<'static> {
    Arg::Scalar(CellValue::from(s))
}

fn err(e: CellError) -> Arg<'static> {
    Arg::Scalar(CellValue::Error(e))
}

/// Dispatch and require a `Number` result.
fn num(name: &str, args: &[Arg], ctx: &EvalCtx) -> f64 {
    match call(name, args, ctx) {
        CellValue::Number(v) => v,
        other => panic!("{name}: expected Number, got {other:?}"),
    }
}

// ============================================================ EDATE / EOMONTH

#[test]
fn sheet_fn_date_edate_clamps_and_signs() {
    // 2021-01-31 (44227) + 1 month -> 2021-02-28 (clamped), serial 44255.
    assert_eq!(num("EDATE", &[n(44227.0), n(1.0)], &ctx()), 44255.0);
    // Backward two months -> 2020-11-30 (44165).
    assert_eq!(num("EDATE", &[n(44227.0), n(-2.0)], &ctx()), 44165.0);
    // Zero months is a no-op (keeps the day).
    assert_eq!(num("EDATE", &[n(44211.0), n(0.0)], &ctx()), 44211.0);
}

#[test]
fn sheet_fn_date_edate_coercion_and_errors() {
    // Text month coerces through coerce::to_number.
    assert_eq!(
        num("EDATE", &[n(44211.0), t("2")], &ctx()),
        44270.0 // 2021-01-15 + 2m = 2021-03-15.
    );
    // A scalar error in any position propagates first-error-wins.
    assert_eq!(
        call("EDATE", &[err(CellError::Div0), n(1.0)], &ctx()),
        CellValue::Error(CellError::Div0)
    );
    // Negative (sub-epoch) serial -> #NUM!.
    assert_eq!(
        call("EDATE", &[n(-1.0), n(1.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
}

#[test]
fn sheet_fn_date_edate_range_top_left_and_arity() {
    // A range collapses to its top-left cell (non-range-aware kernel).
    let cells = [CellValue::Number(44211.0), CellValue::Number(0.0)];
    let view = RangeView::from_slice(cell(), 1, 2, &cells);
    assert_eq!(
        call("EDATE", &[Arg::Range(view), n(2.0)], &ctx()),
        CellValue::Number(44270.0)
    );
    // EDATE is exactly binary.
    assert_eq!(
        call("EDATE", &[n(44211.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
    assert_eq!(
        call("EDATE", &[n(44211.0), n(1.0), n(1.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_eomonth_last_day() {
    // EOMONTH(2021-01-15, 0) -> 2021-01-31 (44227).
    assert_eq!(num("EOMONTH", &[n(44211.0), n(0.0)], &ctx()), 44227.0);
    // +1 month into non-leap February -> 2021-02-28 (44255).
    assert_eq!(num("EOMONTH", &[n(44211.0), n(1.0)], &ctx()), 44255.0);
    // Leap February: EOMONTH(2020-01-15, 1) -> 2020-02-29 (43890).
    assert_eq!(num("EOMONTH", &[n(43845.0), n(1.0)], &ctx()), 43890.0);
}

#[test]
fn sheet_fn_date_eomonth_errors_and_arity() {
    assert_eq!(
        call("EOMONTH", &[err(CellError::Na), n(0.0)], &ctx()),
        CellValue::Error(CellError::Na)
    );
    assert_eq!(
        call("EOMONTH", &[n(-5.0), n(0.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
    // Arity: binary only.
    assert_eq!(
        call("EOMONTH", &[n(44211.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ===================================================================== DATEDIF

#[test]
fn sheet_fn_date_datedif_units() {
    // 2020-01-15 (43845) .. 2021-03-20 (44275).
    let s = 43845.0;
    let e = 44275.0;
    assert_eq!(num("DATEDIF", &[n(s), n(e), t("Y")], &ctx()), 1.0);
    assert_eq!(num("DATEDIF", &[n(s), n(e), t("M")], &ctx()), 14.0);
    assert_eq!(num("DATEDIF", &[n(s), n(e), t("D")], &ctx()), 430.0);
    assert_eq!(num("DATEDIF", &[n(s), n(e), t("MD")], &ctx()), 5.0);
    assert_eq!(num("DATEDIF", &[n(s), n(e), t("YM")], &ctx()), 2.0);
    assert_eq!(num("DATEDIF", &[n(s), n(e), t("YD")], &ctx()), 64.0);
    // Unit is case-insensitive.
    assert_eq!(num("DATEDIF", &[n(s), n(e), t("d")], &ctx()), 430.0);
}

#[test]
fn sheet_fn_date_datedif_rulings_and_errors() {
    // RULING: end < start -> #NUM! (unique to DATEDIF).
    assert_eq!(
        call("DATEDIF", &[n(44275.0), n(43845.0), t("Y")], &ctx()),
        CellValue::Error(CellError::Num)
    );
    // Unknown unit -> #NUM!.
    assert_eq!(
        call("DATEDIF", &[n(43845.0), n(44275.0), t("Q")], &ctx()),
        CellValue::Error(CellError::Num)
    );
    // Error propagation (first-error-wins over scalar args).
    assert_eq!(
        call(
            "DATEDIF",
            &[err(CellError::Div0), n(44275.0), t("D")],
            &ctx()
        ),
        CellValue::Error(CellError::Div0)
    );
    // Arity: exactly ternary.
    assert_eq!(
        call("DATEDIF", &[n(43845.0), n(44275.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ================================================================ DAYS / DAYS360

#[test]
fn sheet_fn_date_days_basic_and_order() {
    // DAYS(end, start) — end is the FIRST argument.
    assert_eq!(num("DAYS", &[n(44275.0), n(43845.0)], &ctx()), 430.0);
    // Reversed -> negative (signed difference).
    assert_eq!(num("DAYS", &[n(43845.0), n(44275.0)], &ctx()), -430.0);
    // Same day -> 0.
    assert_eq!(num("DAYS", &[n(44197.0), n(44197.0)], &ctx()), 0.0);
}

#[test]
fn sheet_fn_date_days_errors_and_arity() {
    assert_eq!(
        call("DAYS", &[err(CellError::Na), n(44197.0)], &ctx()),
        CellValue::Error(CellError::Na)
    );
    // Negative serial -> #NUM!.
    assert_eq!(
        call("DAYS", &[n(44197.0), n(-1.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
    assert_eq!(
        call("DAYS", &[n(44197.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_days360_us_and_eu() {
    // 2020-01-31 (43861) .. 2020-03-31 (43921): both 31->30 => 2*30 = 60 (US).
    assert_eq!(num("DAYS360", &[n(43861.0), n(43921.0)], &ctx()), 60.0);
    // EU: same here (both 31->30) => 60.
    assert_eq!(
        num(
            "DAYS360",
            &[n(43861.0), n(43921.0), Arg::Scalar(CellValue::Bool(true))],
            &ctx()
        ),
        60.0
    );
    // RULING (US/NASD): a full year 2020-01-01..2020-12-31 = 360; the EU rule
    // collapses the Dec-31 end to 30 (no start-31), giving 359.
    assert_eq!(num("DAYS360", &[n(43831.0), n(44196.0)], &ctx()), 360.0);
    assert_eq!(
        num(
            "DAYS360",
            &[n(43831.0), n(44196.0), Arg::Scalar(CellValue::Bool(true))],
            &ctx()
        ),
        359.0
    );
}

#[test]
fn sheet_fn_date_days360_eom_february_and_errors() {
    // RULING: a US start on EOM-February becomes day 30, then a Mar-31 end
    // collapses to 30 (start adj >= 30): 2021-02-28 (44255)..2021-03-31 (44286)
    // = 1 month * 30 = 30.
    assert_eq!(num("DAYS360", &[n(44255.0), n(44286.0)], &ctx()), 30.0);
    // Error propagation.
    assert_eq!(
        call("DAYS360", &[err(CellError::Div0), n(43921.0)], &ctx()),
        CellValue::Error(CellError::Div0)
    );
    // Arity: 2 or 3 args; 1 is invalid, 4 is invalid.
    assert_eq!(
        call("DAYS360", &[n(43861.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
    assert_eq!(
        call("DAYS360", &[n(43861.0), n(43921.0), n(0.0), n(0.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ======================================================= NETWORKDAYS / WORKDAY

#[test]
fn sheet_fn_date_networkdays_weekends_and_holidays() {
    // 2021-01-04 (Mon, 44200) .. 2021-01-08 (Fri, 44204) => 5 working days.
    assert_eq!(num("NETWORKDAYS", &[n(44200.0), n(44204.0)], &ctx()), 5.0);
    // Spanning a weekend to the next Monday => 6.
    assert_eq!(num("NETWORKDAYS", &[n(44200.0), n(44207.0)], &ctx()), 6.0);
    // A scalar holiday inside the interval drops one.
    assert_eq!(
        num("NETWORKDAYS", &[n(44200.0), n(44204.0), n(44202.0)], &ctx()),
        4.0
    );
    // RULING: reversed interval negates the count.
    assert_eq!(num("NETWORKDAYS", &[n(44204.0), n(44200.0)], &ctx()), -5.0);
}

#[test]
fn sheet_fn_date_networkdays_holiday_range_and_errors() {
    // A holiday RANGE {44201, 44202} (both weekdays in the interval) drops two:
    // 5 - 2 = 3. The range arg is scanned by the range-aware kernel.
    let cells = [CellValue::Number(44201.0), CellValue::Number(44202.0)];
    let view = RangeView::from_slice(cell(), 1, 2, &cells);
    assert_eq!(
        num(
            "NETWORKDAYS",
            &[n(44200.0), n(44204.0), Arg::Range(view)],
            &ctx()
        ),
        3.0
    );
    // An error in a holiday range propagates.
    let bad = [CellValue::Error(CellError::Ref), CellValue::Number(44202.0)];
    let badv = RangeView::from_slice(cell(), 1, 2, &bad);
    assert_eq!(
        call(
            "NETWORKDAYS",
            &[n(44200.0), n(44204.0), Arg::Range(badv)],
            &ctx()
        ),
        CellValue::Error(CellError::Ref)
    );
    // Error in a leading scalar arg propagates.
    assert_eq!(
        call("NETWORKDAYS", &[err(CellError::Div0), n(44204.0)], &ctx()),
        CellValue::Error(CellError::Div0)
    );
    // Arity: 2 or 3 args; 1 is invalid.
    assert_eq!(
        call("NETWORKDAYS", &[n(44200.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_workday_skips_weekends() {
    // WORKDAY(Fri 44204, 1) -> Mon 44207.
    assert_eq!(num("WORKDAY", &[n(44204.0), n(1.0)], &ctx()), 44207.0);
    // WORKDAY(Mon 44200, -1) -> previous Fri 44197.
    assert_eq!(num("WORKDAY", &[n(44200.0), n(-1.0)], &ctx()), 44197.0);
    // 0 days -> the start serial unchanged.
    assert_eq!(num("WORKDAY", &[n(44204.0), n(0.0)], &ctx()), 44204.0);
    // A holiday on the would-be Monday 44207 pushes to Tue 44208.
    assert_eq!(
        num("WORKDAY", &[n(44204.0), n(1.0), n(44207.0)], &ctx()),
        44208.0
    );
}

#[test]
fn sheet_fn_date_workday_range_holiday_and_errors() {
    // A 1x1 holiday range {44207} pushes WORKDAY(44204, 1) to Tue 44208.
    let cells = [CellValue::Number(44207.0)];
    let view = RangeView::from_slice(cell(), 1, 1, &cells);
    assert_eq!(
        num("WORKDAY", &[n(44204.0), n(1.0), Arg::Range(view)], &ctx()),
        44208.0
    );
    // Error in a leading scalar arg propagates.
    assert_eq!(
        call("WORKDAY", &[err(CellError::Na), n(1.0)], &ctx()),
        CellValue::Error(CellError::Na)
    );
    // Arity: 2 or 3 args.
    assert_eq!(
        call("WORKDAY", &[n(44204.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ==================================================================== YEARFRAC

#[test]
fn sheet_fn_date_yearfrac_bases() {
    // 2020-01-01 (43831) .. 2020-07-01 (44013): 182 actual days, leap year.
    let s = 43831.0;
    let e = 44013.0;
    let approx = |got: f64, want: f64| assert!((got - want).abs() < 1e-12, "got {got} want {want}");
    // Basis 0 (US 30/360) and 4 (EU 30/360): 180/360 = 0.5.
    approx(num("YEARFRAC", &[n(s), n(e), n(0.0)], &ctx()), 0.5);
    approx(num("YEARFRAC", &[n(s), n(e), n(4.0)], &ctx()), 0.5);
    // Default basis is 0.
    approx(num("YEARFRAC", &[n(s), n(e)], &ctx()), 0.5);
    // Basis 2 (actual/360) and 3 (actual/365).
    approx(
        num("YEARFRAC", &[n(s), n(e), n(2.0)], &ctx()),
        182.0 / 360.0,
    );
    approx(
        num("YEARFRAC", &[n(s), n(e), n(3.0)], &ctx()),
        182.0 / 365.0,
    );
    // RULING: basis 1 (actual/actual) same-year denominator = year length
    // (366 in leap 2020).
    approx(
        num("YEARFRAC", &[n(s), n(e), n(1.0)], &ctx()),
        182.0 / 366.0,
    );
}

#[test]
fn sheet_fn_date_yearfrac_order_and_errors() {
    // Order-insensitive: a reversed interval gives the same magnitude.
    let approx = |got: f64, want: f64| assert!((got - want).abs() < 1e-12, "got {got} want {want}");
    approx(
        num("YEARFRAC", &[n(44013.0), n(43831.0), n(0.0)], &ctx()),
        0.5,
    );
    // Out-of-range basis -> #NUM!.
    assert_eq!(
        call("YEARFRAC", &[n(43831.0), n(44013.0), n(9.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
    // Error propagation.
    assert_eq!(
        call("YEARFRAC", &[err(CellError::Div0), n(44013.0)], &ctx()),
        CellValue::Error(CellError::Div0)
    );
    // Arity: 2 or 3 args.
    assert_eq!(
        call("YEARFRAC", &[n(43831.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ======================================================= DATEVALUE / TIMEVALUE

#[test]
fn sheet_fn_date_datevalue_formats() {
    // ISO and US 4-digit forms of 2021-01-01 = 44197.
    assert_eq!(num("DATEVALUE", &[t("2021-01-01")], &ctx()), 44197.0);
    assert_eq!(num("DATEVALUE", &[t("1/1/2021")], &ctx()), 44197.0);
    // RULING: two-digit-year window (21 -> 2021).
    assert_eq!(num("DATEVALUE", &[t("1/1/21")], &ctx()), 44197.0);
    // The epoch 1900-01-01 = serial 1.
    assert_eq!(num("DATEVALUE", &[t("1900-01-01")], &ctx()), 1.0);
}

#[test]
fn sheet_fn_date_datevalue_errors_and_arity() {
    // RULING: an unparseable string is #VALUE!.
    assert_eq!(
        call("DATEVALUE", &[t("not a date")], &ctx()),
        CellValue::Error(CellError::Value)
    );
    // An error argument propagates (via arg_text).
    assert_eq!(
        call("DATEVALUE", &[err(CellError::Div0)], &ctx()),
        CellValue::Error(CellError::Div0)
    );
    // Arity: unary.
    assert_eq!(
        call("DATEVALUE", &[t("2021-01-01"), t("x")], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_timevalue_formats() {
    let approx = |got: f64, want: f64| assert!((got - want).abs() < 1e-12, "got {got} want {want}");
    // hh:mm and hh:mm:ss, 24-hour.
    approx(num("TIMEVALUE", &[t("12:00")], &ctx()), 0.5);
    approx(num("TIMEVALUE", &[t("06:00:00")], &ctx()), 0.25);
    approx(
        num("TIMEVALUE", &[t("08:15:30")], &ctx()),
        (8.0 * 3600.0 + 15.0 * 60.0 + 30.0) / 86_400.0,
    );
}

#[test]
fn sheet_fn_date_timevalue_errors_and_arity() {
    // RULING: out-of-range component -> #VALUE!.
    assert_eq!(
        call("TIMEVALUE", &[t("25:00")], &ctx()),
        CellValue::Error(CellError::Value)
    );
    // Non-time text -> #VALUE!.
    assert_eq!(
        call("TIMEVALUE", &[t("noon")], &ctx()),
        CellValue::Error(CellError::Value)
    );
    // Error argument propagates.
    assert_eq!(
        call("TIMEVALUE", &[err(CellError::Na)], &ctx()),
        CellValue::Error(CellError::Na)
    );
    // Arity: unary.
    assert_eq!(
        call("TIMEVALUE", &[t("12:00"), t("x")], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ======================================================= WEEKNUM / ISOWEEKNUM

#[test]
fn sheet_fn_date_weeknum_modes() {
    // 2021-01-01 (44197, a Friday): default Sunday-start system-1 -> week 1.
    assert_eq!(num("WEEKNUM", &[n(44197.0)], &ctx()), 1.0);
    // 2021-01-04 (44200, Monday), return_type 2 (Monday-start) -> week 2.
    assert_eq!(num("WEEKNUM", &[n(44200.0), n(2.0)], &ctx()), 2.0);
    // Mid-year and year-end (Sunday-start system-1).
    assert_eq!(num("WEEKNUM", &[n(44378.0)], &ctx()), 27.0); // 2021-07-01
    assert_eq!(num("WEEKNUM", &[n(44561.0)], &ctx()), 53.0); // 2021-12-31
                                                             // return_type 21 is ISO: 2021-01-01 belongs to ISO 2020-W53.
    assert_eq!(num("WEEKNUM", &[n(44197.0), n(21.0)], &ctx()), 53.0);
}

#[test]
fn sheet_fn_date_weeknum_errors_and_arity() {
    // RULING: an out-of-range return_type -> #NUM!.
    assert_eq!(
        call("WEEKNUM", &[n(44197.0), n(99.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
    // Error propagation.
    assert_eq!(
        call("WEEKNUM", &[err(CellError::Div0)], &ctx()),
        CellValue::Error(CellError::Div0)
    );
    // Negative serial -> #NUM!.
    assert_eq!(
        call("WEEKNUM", &[n(-1.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
    // Arity: 1 or 2 args.
    assert_eq!(
        call("WEEKNUM", &[n(44197.0), n(1.0), n(1.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_isoweeknum_basic() {
    // RULING: 2021-01-01 (Fri) belongs to ISO week 53 of 2020.
    assert_eq!(num("ISOWEEKNUM", &[n(44197.0)], &ctx()), 53.0);
    // 2021-01-04 (Mon) -> ISO week 1.
    assert_eq!(num("ISOWEEKNUM", &[n(44200.0)], &ctx()), 1.0);
    // 2021-12-31 (Fri) -> ISO week 52.
    assert_eq!(num("ISOWEEKNUM", &[n(44561.0)], &ctx()), 52.0);
}

#[test]
fn sheet_fn_date_isoweeknum_errors_and_arity() {
    assert_eq!(
        call("ISOWEEKNUM", &[err(CellError::Na)], &ctx()),
        CellValue::Error(CellError::Na)
    );
    assert_eq!(
        call("ISOWEEKNUM", &[n(-1.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
    // Arity: unary.
    assert_eq!(
        call("ISOWEEKNUM", &[n(44197.0), n(1.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ============================================= date-system honoring (1904)

#[test]
fn sheet_fn_date_edate_honors_date_system() {
    // 1904-system serial for 2021-01-31 = 44227 - 1462 = 42765; + 1 month
    // -> 2021-02-28 = 1900-serial 44255 - 1462 = 42793.
    assert_eq!(num("EDATE", &[n(42765.0), n(1.0)], &ctx1904()), 42793.0);
}

#[test]
fn sheet_fn_date_weeknum_honors_date_system() {
    // The same calendar date is the same week in either epoch. 1904 serial
    // 42735 = 2021-01-01 -> WEEKNUM default 1, ISOWEEKNUM 53.
    assert_eq!(num("WEEKNUM", &[n(42735.0)], &ctx1904()), 1.0);
    assert_eq!(num("ISOWEEKNUM", &[n(42735.0)], &ctx1904()), 53.0);
}

// ==================================================== corpus self-verification

/// The single column-0 sheet id every replayed case runs on.
const SHEET: SheetId = 0;

/// Build a fresh one-sheet engine (mirrors `corpus_runner.rs`).
fn fresh_engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    Engine::new(m, EngineConfig::default())
}

/// 1-based A1 cell address (`B3`) → 0-based `(row, col)`.
fn parse_addr(addr: &str) -> (u32, u32) {
    let upper = addr.trim().to_ascii_uppercase();
    let split = upper
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or_else(|| panic!("bad A1 address {addr:?}"));
    let (col_s, row_s) = upper.split_at(split);
    let col = sheet_core::a1_to_col(col_s).unwrap_or_else(|| panic!("bad column in {addr:?}"));
    let row: u32 = row_s
        .parse()
        .unwrap_or_else(|_| panic!("bad row in {addr:?}"));
    (row - 1, col)
}

/// Apply one setup `(addr, raw)` seed (the `text:` typed tag is honored so a
/// date-string cell stays Text; otherwise Excel-like literal detection).
fn apply_setup(e: &mut Engine, addr: &str, raw: &str, case_id: &str) {
    let (row, col) = parse_addr(addr);
    if let Some(rest) = raw.strip_prefix("text:") {
        e.set_cell(
            SHEET,
            row,
            col,
            SetInput::Value(CellValue::Text(rest.into())),
        );
    } else {
        e.enter(SHEET, row, col, raw)
            .unwrap_or_else(|err| panic!("[{case_id}] setup {addr}={raw:?} parse error: {err:?}"));
    }
}

/// The display projection: General text of a value, or the error token.
fn project(value: &CellValue) -> String {
    match value {
        CellValue::Error(e) => e.as_str().to_string(),
        other => coerce::to_text(other).to_string(),
    }
}

/// Run one corpus case through the engine and compare the projection.
fn run_case(case: &CorpusCase) -> Result<(), String> {
    let mut e = fresh_engine();
    // Default formula host Z99 = col 25, row 98 (outside any setup).
    let (frow, fcol) = (98u32, 25u32);
    for (addr, raw) in &case.setup {
        if addr == "-" {
            continue;
        }
        apply_setup(&mut e, addr, raw, &case.id);
    }
    e.enter(SHEET, frow, fcol, &case.formula).map_err(|err| {
        format!(
            "[{}] formula {:?} parse error: {err:?}",
            case.id, case.formula
        )
    })?;
    let value = e
        .model()
        .sheet(SHEET)
        .and_then(|ws| ws.cell(frow, fcol))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty);
    let got = project(&value);
    if got == case.expected {
        Ok(())
    } else {
        Err(format!(
            "[{}] {} (setup {:?}) -> got {:?}, want {:?}",
            case.id, case.formula, case.setup, got, case.expected
        ))
    }
}

/// Replay every `date2/*.golden.tsv` through the engine and assert each
/// `expected` matches the live implementation's projection — pinning the
/// corpus to the kernels in this owned test file.
#[test]
fn sheet_fn_date_corpus_replay() {
    let files = [
        "edate",
        "eomonth",
        "datedif",
        "days",
        "days360",
        "networkdays",
        "workday",
        "yearfrac",
        "datevalue",
        "timevalue",
        "weeknum",
        "isoweeknum",
    ];
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;
    for name in files {
        let rel = format!("corpus/fn-corpus/date2/{name}.golden.tsv");
        let cases = load_corpus(&rel);
        assert!(!cases.is_empty(), "empty corpus file {rel}");
        for case in &cases {
            total += 1;
            if let Err(msg) = run_case(case) {
                failures.push(format!("{rel}: {msg}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "date2 corpus: {}/{} case(s) failed:\n{}",
        failures.len(),
        total,
        failures.join("\n")
    );
}
