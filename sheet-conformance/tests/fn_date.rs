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

//! Date/time family conformance (spec §7, §11 T0; registry
//! `sheet.fn.date.*`). Self-contained direct-dispatch tests: each case
//! resolves a [`sheet_core::FuncId`] via `lookup_func`, builds the frozen
//! `&[Arg]` slice, and routes through the registry-generated
//! [`sheet_fn::dispatch`] — exercising the arity guard and kernel wiring as
//! `sheet-calc` will. Every function carries at least one test fn whose name
//! starts with the registry pointer prefix `sheet_fn_date_<name>` so the
//! §12.2 coverage gate finds it.
//!
//! The `EvalCtx` is fixed (1900 system, a constant `now_serial = 45000.5`,
//! seed 42) so the volatile clock is deterministic. The serial constants are
//! the same well-known 1900-system values the FMT track's `serial_date`
//! corpus pins (44197 = 2021-01-01).

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::{dispatch, Arg, EvalCtx, RangeView};

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

fn num(name: &str, args: &[Arg], ctx: &EvalCtx) -> f64 {
    match call(name, args, ctx) {
        CellValue::Number(v) => v,
        other => panic!("{name}: expected Number, got {other:?}"),
    }
}

// ---------------------------------------------------------------- NOW / TODAY

#[test]
fn sheet_fn_date_now_basic() {
    // NOW() returns the injected wall-clock serial verbatim (volatile but
    // deterministic under the fixed ctx).
    assert_eq!(call("NOW", &[], &ctx()), CellValue::Number(45000.5));
}

#[test]
fn sheet_fn_date_now_arity() {
    // NOW takes no args: a surplus arg is a #VALUE! arity violation.
    assert_eq!(
        call("NOW", &[n(1.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_today_basic() {
    // TODAY() floors the clock to midnight.
    assert_eq!(call("TODAY", &[], &ctx()), CellValue::Number(45000.0));
}

#[test]
fn sheet_fn_date_today_arity() {
    assert_eq!(
        call("TODAY", &[n(1.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ---------------------------------------------------------------------- DATE

#[test]
fn sheet_fn_date_date_basic() {
    // 2021-01-01 = serial 44197 (1900 system).
    assert_eq!(num("DATE", &[n(2021.0), n(1.0), n(1.0)], &ctx()), 44197.0);
    // 1900-01-01 = serial 1 (the epoch).
    assert_eq!(num("DATE", &[n(1900.0), n(1.0), n(1.0)], &ctx()), 1.0);
}

#[test]
fn sheet_fn_date_date_rolling() {
    // Month roll: DATE(2020,13,1) = 2021-01-01.
    let s = num("DATE", &[n(2020.0), n(13.0), n(1.0)], &ctx());
    assert_eq!(
        sheet_format::serial::serial_to_ymd(s, DateSystem::Date1900),
        Some((2021, 1, 1))
    );
    // Day roll forward: DATE(2020,1,32) = 2020-02-01.
    let s = num("DATE", &[n(2020.0), n(1.0), n(32.0)], &ctx());
    assert_eq!(
        sheet_format::serial::serial_to_ymd(s, DateSystem::Date1900),
        Some((2020, 2, 1))
    );
    // Day roll back: DATE(2020,1,0) = 2019-12-31.
    let s = num("DATE", &[n(2020.0), n(1.0), n(0.0)], &ctx());
    assert_eq!(
        sheet_format::serial::serial_to_ymd(s, DateSystem::Date1900),
        Some((2019, 12, 31))
    );
    // Month roll back: DATE(2020,0,1) = 2019-12-01.
    let s = num("DATE", &[n(2020.0), n(0.0), n(1.0)], &ctx());
    assert_eq!(
        sheet_format::serial::serial_to_ymd(s, DateSystem::Date1900),
        Some((2019, 12, 1))
    );
}

#[test]
fn sheet_fn_date_date_year_shorthand() {
    // Years 0..=1899 are read as 1900+year.
    let s = num("DATE", &[n(20.0), n(1.0), n(1.0)], &ctx());
    assert_eq!(
        sheet_format::serial::serial_to_ymd(s, DateSystem::Date1900),
        Some((1920, 1, 1))
    );
    let s = num("DATE", &[n(0.0), n(1.0), n(1.0)], &ctx());
    assert_eq!(
        sheet_format::serial::serial_to_ymd(s, DateSystem::Date1900),
        Some((1900, 1, 1))
    );
}

#[test]
fn sheet_fn_date_date_coercion() {
    // Text and bool args coerce through coerce::to_number.
    let s = num(
        "DATE",
        &[Arg::Scalar(CellValue::from("2021")), n(1.0), n(1.0)],
        &ctx(),
    );
    assert_eq!(s, 44197.0);
    // Bad text -> #VALUE!.
    assert_eq!(
        call(
            "DATE",
            &[Arg::Scalar(CellValue::from("nope")), n(1.0), n(1.0)],
            &ctx()
        ),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_date_error_propagation() {
    assert_eq!(
        call(
            "DATE",
            &[
                n(2021.0),
                Arg::Scalar(CellValue::Error(CellError::Div0)),
                n(1.0)
            ],
            &ctx()
        ),
        CellValue::Error(CellError::Div0)
    );
}

#[test]
fn sheet_fn_date_date_out_of_domain_is_num() {
    // A huge negative day underflows below the 1900 epoch -> #NUM!.
    assert_eq!(
        call("DATE", &[n(1900.0), n(1.0), n(-1000000.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
}

#[test]
fn sheet_fn_date_date_arity() {
    // DATE is exactly ternary.
    assert_eq!(
        call("DATE", &[n(2021.0), n(1.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
    assert_eq!(
        call("DATE", &[n(2021.0), n(1.0), n(1.0), n(1.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// --------------------------------------------------------- YEAR / MONTH / DAY

#[test]
fn sheet_fn_date_year_basic() {
    assert_eq!(num("YEAR", &[n(44197.0)], &ctx()), 2021.0);
}

#[test]
fn sheet_fn_date_year_negative_is_num() {
    assert_eq!(
        call("YEAR", &[n(-1.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
}

#[test]
fn sheet_fn_date_year_error_propagation() {
    assert_eq!(
        call(
            "YEAR",
            &[Arg::Scalar(CellValue::Error(CellError::Na))],
            &ctx()
        ),
        CellValue::Error(CellError::Na)
    );
}

#[test]
fn sheet_fn_date_year_range_takes_top_left() {
    // A range arriving at a non-range-aware kernel collapses to its top-left
    // cell (implicit intersection happens upstream; the kernel is total).
    let cells = [CellValue::Number(44197.0), CellValue::Number(0.0)];
    let view = RangeView::from_slice(cell(), 1, 2, &cells);
    assert_eq!(
        call("YEAR", &[Arg::Range(view)], &ctx()),
        CellValue::Number(2021.0)
    );
}

#[test]
fn sheet_fn_date_month_basic() {
    assert_eq!(num("MONTH", &[n(44197.0)], &ctx()), 1.0);
    // 2021-12-31 = serial 44561.
    assert_eq!(num("MONTH", &[n(44561.0)], &ctx()), 12.0);
}

#[test]
fn sheet_fn_date_month_arity() {
    assert_eq!(
        call("MONTH", &[n(1.0), n(2.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_day_basic() {
    assert_eq!(num("DAY", &[n(44197.0)], &ctx()), 1.0);
    // 2021-01-15.
    assert_eq!(num("DAY", &[n(44211.0)], &ctx()), 15.0);
}

#[test]
fn sheet_fn_date_day_coercion_bool() {
    // TRUE -> 1.0 -> 1900-01-01 -> DAY 1.
    assert_eq!(
        num("DAY", &[Arg::Scalar(CellValue::Bool(true))], &ctx()),
        1.0
    );
}

#[test]
fn sheet_fn_date_day_leap1900() {
    // The phantom serial 60 decodes to 1900-02-29 -> DAY 29 (ruling
    // sheet.format.date.leap1900, honored through serial_to_ymd).
    assert_eq!(num("DAY", &[n(60.0)], &ctx()), 29.0);
    assert_eq!(num("MONTH", &[n(60.0)], &ctx()), 2.0);
}

#[test]
fn sheet_fn_date_day_zero_serial0() {
    // Audit finding 4: Excel accepts serial 0 (the day-zero epoch 1900-01-00)
    // where it previously yielded #NUM!. YEAR(0)=1900, MONTH(0)=1, DAY(0)=0.
    assert_eq!(num("YEAR", &[n(0.0)], &ctx()), 1900.0);
    assert_eq!(num("MONTH", &[n(0.0)], &ctx()), 1.0);
    assert_eq!(num("DAY", &[n(0.0)], &ctx()), 0.0);
    // Still rejects NEGATIVE serials.
    assert_eq!(
        call("YEAR", &[n(-1.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
}

#[test]
fn sheet_fn_date_date_day_zero() {
    // DATE(1900,1,0) is Excel's day-zero -> serial 0 (audit finding 4), via the
    // rolling path (day-1 offset from 1900-01-01 = serial 1).
    assert_eq!(num("DATE", &[n(1900.0), n(1.0), n(0.0)], &ctx()), 0.0);
}

#[test]
fn sheet_fn_date_weekday_serial0() {
    // WEEKDAY(0) under 1900: serial index 0 -> Saturday. Type 1 = 7 (audit
    // finding 4); the serial-index anchors with rem_euclid: (0-1).rem_euclid(7)
    // = 6 (Sat, sun0) -> type 1 = 7.
    assert_eq!(num("WEEKDAY", &[n(0.0)], &ctx()), 7.0);
}

// ---------------------------------------------------------------------- TIME

#[test]
fn sheet_fn_date_time_basic() {
    // TIME(12,0,0) = 0.5 (noon).
    assert_eq!(num("TIME", &[n(12.0), n(0.0), n(0.0)], &ctx()), 0.5);
    // TIME(6,0,0) = 0.25.
    assert_eq!(num("TIME", &[n(6.0), n(0.0), n(0.0)], &ctx()), 0.25);
}

#[test]
fn sheet_fn_date_time_wraps_mod_24h() {
    // TIME(25,0,0) wraps to 1:00 = 1/24.
    let got = num("TIME", &[n(25.0), n(0.0), n(0.0)], &ctx());
    assert!((got - 1.0 / 24.0).abs() < 1e-12, "got {got}");
    // TIME(0,90,0) = 1h30m = 0.0625.
    let got = num("TIME", &[n(0.0), n(90.0), n(0.0)], &ctx());
    assert!((got - 0.0625).abs() < 1e-12, "got {got}");
}

#[test]
fn sheet_fn_date_time_negative_is_num() {
    assert_eq!(
        call("TIME", &[n(-1.0), n(0.0), n(0.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
}

#[test]
fn sheet_fn_date_time_error_propagation() {
    assert_eq!(
        call(
            "TIME",
            &[
                n(1.0),
                Arg::Scalar(CellValue::Error(CellError::Ref)),
                n(0.0)
            ],
            &ctx()
        ),
        CellValue::Error(CellError::Ref)
    );
}

#[test]
fn sheet_fn_date_time_arity() {
    assert_eq!(
        call("TIME", &[n(1.0), n(0.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// --------------------------------------------------- HOUR / MINUTE / SECOND

#[test]
fn sheet_fn_date_hour_basic() {
    // 45000.5 -> 12:00:00.
    assert_eq!(num("HOUR", &[n(45000.5)], &ctx()), 12.0);
    // 0.25 -> 06:00:00.
    assert_eq!(num("HOUR", &[n(0.25)], &ctx()), 6.0);
}

#[test]
fn sheet_fn_date_hour_negative_is_num() {
    assert_eq!(
        call("HOUR", &[n(-0.5)], &ctx()),
        CellValue::Error(CellError::Num)
    );
}

#[test]
fn sheet_fn_date_minute_basic() {
    // 0.0625 = 01:30:00 -> MINUTE 30.
    assert_eq!(num("MINUTE", &[n(0.0625)], &ctx()), 30.0);
    assert_eq!(num("MINUTE", &[n(45000.5)], &ctx()), 0.0);
}

#[test]
fn sheet_fn_date_minute_error_propagation() {
    assert_eq!(
        call(
            "MINUTE",
            &[Arg::Scalar(CellValue::Error(CellError::Value))],
            &ctx()
        ),
        CellValue::Error(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_second_basic() {
    // 1 second = 1/86400; HMS rounds to nearest second.
    let one_sec = 1.0 / 86400.0;
    assert_eq!(num("SECOND", &[n(one_sec)], &ctx()), 1.0);
    assert_eq!(num("SECOND", &[n(45000.5)], &ctx()), 0.0);
}

#[test]
fn sheet_fn_date_second_arity() {
    assert_eq!(
        call("SECOND", &[n(1.0), n(2.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ------------------------------------------------------------------- WEEKDAY

#[test]
fn sheet_fn_date_weekday_basic() {
    // 2021-01-01 (serial 44197) is a Friday.
    // Type 1 (default): 1=Sun..7=Sat -> Friday = 6.
    assert_eq!(num("WEEKDAY", &[n(44197.0)], &ctx()), 6.0);
}

#[test]
fn sheet_fn_date_weekday_return_types() {
    // Friday under each return-type mode.
    assert_eq!(num("WEEKDAY", &[n(44197.0), n(1.0)], &ctx()), 6.0); // 1=Sun..7
    assert_eq!(num("WEEKDAY", &[n(44197.0), n(2.0)], &ctx()), 5.0); // 1=Mon..7
    assert_eq!(num("WEEKDAY", &[n(44197.0), n(3.0)], &ctx()), 4.0); // 0=Mon..6
}

#[test]
fn sheet_fn_date_weekday_serial_epoch() {
    // Excel's 1900 serial 1 is a Sunday (the phantom-day off-by-one): type 1
    // -> 1, computed from the serial INDEX, not the real 1900-01-01 (Monday).
    assert_eq!(num("WEEKDAY", &[n(1.0)], &ctx()), 1.0);
    // The phantom serial 60 is a Wednesday under Excel -> type 1 = 4.
    assert_eq!(num("WEEKDAY", &[n(60.0)], &ctx()), 4.0);
}

#[test]
fn sheet_fn_date_weekday_bad_return_type_is_num() {
    assert_eq!(
        call("WEEKDAY", &[n(44197.0), n(9.0)], &ctx()),
        CellValue::Error(CellError::Num)
    );
}

#[test]
fn sheet_fn_date_weekday_error_propagation() {
    assert_eq!(
        call(
            "WEEKDAY",
            &[Arg::Scalar(CellValue::Error(CellError::Div0))],
            &ctx()
        ),
        CellValue::Error(CellError::Div0)
    );
}

#[test]
fn sheet_fn_date_weekday_arity() {
    assert_eq!(
        call("WEEKDAY", &[n(1.0), n(2.0), n(3.0)], &ctx()),
        CellValue::Error(CellError::Value)
    );
}

// ----------------------------------------------- date-system honoring (1904)

#[test]
fn sheet_fn_date_year_honors_date_system() {
    // 1904-system serial 42735 = 2021-01-01 (= 1900-serial 44197 - 1462).
    assert_eq!(num("YEAR", &[n(42735.0)], &ctx1904()), 2021.0);
    assert_eq!(num("MONTH", &[n(42735.0)], &ctx1904()), 1.0);
    assert_eq!(num("DAY", &[n(42735.0)], &ctx1904()), 1.0);
}

#[test]
fn sheet_fn_date_weekday_honors_date_system() {
    // The same calendar date is the same weekday in either system: 2021-01-01
    // is Friday -> type 1 = 6 under the 1904 epoch too.
    assert_eq!(num("WEEKDAY", &[n(42735.0)], &ctx1904()), 6.0);
    // 1904-system serial 0 = 1904-01-01 = Friday -> type 1 = 6.
    assert_eq!(num("WEEKDAY", &[n(0.0)], &ctx1904()), 6.0);
}

#[test]
fn sheet_fn_date_date_honors_date_system() {
    // DATE builds in the active system: 1904 DATE(2021,1,1) = serial 42735.
    assert_eq!(
        num("DATE", &[n(2021.0), n(1.0), n(1.0)], &ctx1904()),
        42735.0
    );
}
