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

//! The date/time family (spec §7, §11 T0). Eleven pure kernels over the
//! workbook serial axis: the volatile clock pair (`NOW`/`TODAY`), the
//! constructors (`DATE`/`TIME`), the calendar decomposers
//! (`YEAR`/`MONTH`/`DAY`/`HOUR`/`MINUTE`/`SECOND`), and `WEEKDAY`.
//!
//! All serial <-> calendar conversion is delegated to
//! [`sheet_format::serial`] (the FMT track owns the 1900/1904 epochs and the
//! adopted 1900 leap-year-bug ruling); these kernels add only the Excel
//! *function* semantics on top — the clock comes from [`EvalCtx::now_serial`]
//! so volatile time is deterministic under test, and the date system from
//! [`EvalCtx::date_system`]. Scalar errors propagate first-error-wins via
//! [`coerce::first_error`]; every numeric coercion routes through
//! [`coerce::to_number`] so the coercion rulings are stated once (spec §7).

use sheet_core::{CellError, CellValue, DateSystem};
use sheet_format::serial::{serial_to_hms, serial_to_ymd, ymd_to_serial};

use crate::arg::Arg;
use crate::coerce;

/// Coerce one scalar argument to a number, mapping an [`Arg::Range`] to the
/// Excel "implicit-intersection has already happened upstream" expectation:
/// a date kernel is never `range_aware`, so a range arriving here is a
/// degenerate caller error — we take its top-left cell (the calc layer
/// normally intersects before dispatch). An error cell propagates.
fn arg_number(a: &Arg) -> Result<f64, CellError> {
    match a {
        Arg::Scalar(v) => coerce::to_number(v),
        Arg::Range(r) => coerce::to_number(&r.get(0, 0)),
    }
}

/// `NOW()` — the injected wall-clock serial (date + fractional time). Volatile
/// (registry `sheet.fn.date.now`); the real serial is computed by the caller
/// and handed in via [`EvalCtx::now_serial`] so the value is deterministic in
/// tests. ECMA-376 §18.17.7.
pub fn now(_args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    CellValue::Number(ctx.now_serial)
}

/// `TODAY()` — the injected clock truncated to midnight (the date serial with
/// no time-of-day). Volatile (registry `sheet.fn.date.today`). ECMA-376
/// §18.17.7.
pub fn today(_args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    CellValue::Number(ctx.now_serial.floor())
}

/// `DATE(year, month, day)` — construct a serial with Excel's normalization
/// (registry `sheet.fn.date.date`; ECMA-376 §18.17.7):
///
/// - **Year semantics:** a year in `0..=1899` is taken as `1900 + year`
///   (Excel's two-/three-digit-year shorthand); `1900..=9999` is literal.
/// - **Rolling normalization:** out-of-range months and days roll in *civil
///   space before* the serial conversion. Months normalize mod 12 (carrying
///   into the year, `DATE(2020,13,1) = 2021-01-01`); the day is then applied
///   as a linear serial offset (`DATE(2020,1,32) = 2020-02-01`,
///   `DATE(2020,1,0) = 2019-12-31`), which is correct across the 1900 phantom
///   day because serials are contiguous.
///
/// Negative/zero day or a year that rolls out of the valid serial domain
/// yields `#NUM!`.
pub fn date(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let y = match arg_number(&args[0]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    let m = match arg_number(&args[1]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    let d = match arg_number(&args[2]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };

    // Excel year shorthand: 0..=1899 means 1900+year.
    let year = if (0..=1899).contains(&y) { y + 1900 } else { y };

    // Normalize the month into 1..=12, carrying whole years (rolling in civil
    // space, BEFORE the serial conversion). `month` is 1-based, so work in
    // 0-based month-of-year arithmetic with Euclidean div/rem (correct for
    // negative months too: DATE(2020,0,1) = 2019-12-01).
    let total_months = year.saturating_mul(12).saturating_add(m - 1);
    let norm_year = total_months.div_euclid(12);
    let norm_month = (total_months.rem_euclid(12) + 1) as u32;

    // i32 is the domain `ymd_to_serial` works in; a year outside it is far
    // out of the 0001..=9999 valid range and resolves to #NUM!.
    let Ok(norm_year_i32) = i32::try_from(norm_year) else {
        return CellValue::Error(CellError::Num);
    };

    // Convert the first of the normalized month to a serial, then apply the
    // day as a linear offset (day-1 days past the 1st). This rolls days of
    // any magnitude and is correct across the phantom 1900-02-29.
    let Some(first) = ymd_to_serial(norm_year_i32, norm_month, 1, ctx.date_system) else {
        return CellValue::Error(CellError::Num);
    };
    let serial = first + (d - 1) as f64;

    // The result must still land in the valid serial domain (a huge negative
    // day can underflow below the epoch). Validate by round-tripping the
    // date part through the serial converter.
    if serial_to_ymd(serial, ctx.date_system).is_none() {
        return CellValue::Error(CellError::Num);
    }
    CellValue::Number(serial)
}

/// Decompose a single serial argument into `(year, month, day)` for the
/// `YEAR`/`MONTH`/`DAY` kernels. `#NUM!` on a negative serial (Excel rejects
/// negative serials for the calendar decomposers) or any out-of-domain value;
/// scalar errors propagate.
fn decompose_ymd(args: &[Arg], sys: DateSystem) -> Result<(i32, u32, u32), CellError> {
    if let Some(e) = coerce::first_error(args) {
        return Err(e);
    }
    let serial = arg_number(&args[0])?;
    if serial < 0.0 {
        return Err(CellError::Num);
    }
    serial_to_ymd(serial, sys).ok_or(CellError::Num)
}

/// `YEAR(serial)` — the calendar year of a serial (registry
/// `sheet.fn.date.year`). ECMA-376 §18.17.7.
pub fn year(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    match decompose_ymd(args, ctx.date_system) {
        Ok((y, _, _)) => CellValue::Number(y as f64),
        Err(e) => CellValue::Error(e),
    }
}

/// `MONTH(serial)` — the calendar month `1..=12` of a serial (registry
/// `sheet.fn.date.month`). ECMA-376 §18.17.7.
pub fn month(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    match decompose_ymd(args, ctx.date_system) {
        Ok((_, m, _)) => CellValue::Number(m as f64),
        Err(e) => CellValue::Error(e),
    }
}

/// `DAY(serial)` — the day-of-month `1..=31` of a serial (registry
/// `sheet.fn.date.day`). ECMA-376 §18.17.7.
pub fn day(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    match decompose_ymd(args, ctx.date_system) {
        Ok((_, _, d)) => CellValue::Number(d as f64),
        Err(e) => CellValue::Error(e),
    }
}

/// `TIME(hour, minute, second)` — a fractional-day serial (registry
/// `sheet.fn.date.time`; ECMA-376 §18.17.7). The three components are summed
/// in seconds and reduced **modulo 24h** (Excel wraps, so
/// `TIME(25,0,0) = TIME(1,0,0) = 1/24`). Negative inputs that drive the total
/// below zero are `#NUM!` (Excel rejects them).
pub fn time(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let h = match arg_number(&args[0]) {
        Ok(n) => n.trunc(),
        Err(e) => return CellValue::Error(e),
    };
    let m = match arg_number(&args[1]) {
        Ok(n) => n.trunc(),
        Err(e) => return CellValue::Error(e),
    };
    let s = match arg_number(&args[2]) {
        Ok(n) => n.trunc(),
        Err(e) => return CellValue::Error(e),
    };

    // Total seconds, then reduce into one day. A negative net total is
    // rejected (#NUM!) — matching Excel, which will not roll a negative time.
    let total_secs = h * 3600.0 + m * 60.0 + s;
    if total_secs < 0.0 {
        return CellValue::Error(CellError::Num);
    }
    let day_secs = 86_400.0;
    let frac = (total_secs % day_secs) / day_secs;
    CellValue::Number(frac)
}

/// Decompose a single serial argument into `(hour, minute, second)` for the
/// `HOUR`/`MINUTE`/`SECOND` kernels. `#NUM!` on a negative serial; scalar
/// errors propagate. The whole-day part is irrelevant — only the fractional
/// time-of-day matters — but a negative serial is still rejected for parity
/// with the calendar decomposers.
fn decompose_hms(args: &[Arg]) -> Result<(u32, u32, u32), CellError> {
    if let Some(e) = coerce::first_error(args) {
        return Err(e);
    }
    let serial = arg_number(&args[0])?;
    if serial < 0.0 {
        return Err(CellError::Num);
    }
    Ok(serial_to_hms(serial))
}

/// `HOUR(serial)` — the hour `0..=23` of a serial's time-of-day (registry
/// `sheet.fn.date.hour`). ECMA-376 §18.17.7.
pub fn hour(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    match decompose_hms(args) {
        Ok((h, _, _)) => CellValue::Number(h as f64),
        Err(e) => CellValue::Error(e),
    }
}

/// `MINUTE(serial)` — the minute `0..=59` of a serial's time-of-day (registry
/// `sheet.fn.date.minute`). ECMA-376 §18.17.7.
pub fn minute(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    match decompose_hms(args) {
        Ok((_, m, _)) => CellValue::Number(m as f64),
        Err(e) => CellValue::Error(e),
    }
}

/// `SECOND(serial)` — the second `0..=59` of a serial's time-of-day (registry
/// `sheet.fn.date.second`). ECMA-376 §18.17.7.
pub fn second(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    match decompose_hms(args) {
        Ok((_, _, s)) => CellValue::Number(s as f64),
        Err(e) => CellValue::Error(e),
    }
}

/// `WEEKDAY(serial[, return_type])` — the day of week of a serial (registry
/// `sheet.fn.date.weekday`; ECMA-376 §18.17.7). T0 supports the three classic
/// return types:
///
/// - **1** (default): `1 = Sunday .. 7 = Saturday`.
/// - **2**: `1 = Monday .. 7 = Sunday`.
/// - **3**: `0 = Monday .. 6 = Sunday`.
///
/// Any other `return_type` is `#NUM!`. A negative serial or out-of-domain
/// value is `#NUM!`; scalar errors propagate.
pub fn weekday(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let serial = match arg_number(&args[0]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    if serial < 0.0 {
        return CellValue::Error(CellError::Num);
    }
    // Validate the serial is in the calendar domain (consistent with the
    // other decomposers) — but the weekday itself is computed from the serial
    // INDEX, not the calendar date, so Excel's phantom-1900-day off-by-one is
    // reproduced exactly (see `day_of_week_sun0`).
    if serial_to_ymd(serial, ctx.date_system).is_none() {
        return CellValue::Error(CellError::Num);
    }

    let return_type = if args.len() >= 2 {
        match arg_number(&args[1]) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return CellValue::Error(e),
        }
    } else {
        1
    };

    let dow_sun0 = day_of_week_sun0(serial, ctx.date_system);

    let v = match return_type {
        1 => dow_sun0 + 1,             // 1=Sun..7=Sat
        2 => ((dow_sun0 + 6) % 7) + 1, // 1=Mon..7=Sun
        3 => (dow_sun0 + 6) % 7,       // 0=Mon..6=Sun
        _ => return CellValue::Error(CellError::Num),
    };
    CellValue::Number(v as f64)
}

/// Day-of-week as `0 = Sunday .. 6 = Saturday`, computed from the **serial
/// index** (not the calendar date) so it matches Excel's WEEKDAY exactly,
/// including the 1900 phantom-day off-by-one. Each system has a known
/// serial→weekday anchor:
///
/// - **1900:** serial `1` (= 1900-01-01) is a Sunday in Excel — so
///   `sun0 = (floor(serial) - 1) mod 7`. The phantom serial 60 lands on a
///   Wednesday (`sun0 = 3`), and serials ≥ 61 line up with the real Gregorian
///   calendar (serial 44197 = real 2021-01-01 = Friday).
/// - **1904:** serial `0` (= 1904-01-01) is a Friday (`sun0 = 5`), with no
///   phantom day — so `sun0 = (floor(serial) + 5) mod 7`, matching the real
///   calendar throughout.
fn day_of_week_sun0(serial: f64, sys: DateSystem) -> i64 {
    let n = serial.floor() as i64;
    let anchored = match sys {
        DateSystem::Date1900 => n - 1,
        DateSystem::Date1904 => n + 5,
    };
    anchored.rem_euclid(7)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::EvalCtx;
    use sheet_core::CellRef;

    fn cr() -> CellRef {
        CellRef {
            sheet: 0,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        }
    }

    fn ctx() -> EvalCtx {
        EvalCtx::new(DateSystem::Date1900, cr(), 45000.5, 42)
    }

    fn n(x: f64) -> Arg<'static> {
        Arg::Scalar(CellValue::Number(x))
    }

    #[test]
    fn now_and_today_use_clock() {
        assert_eq!(now(&[], &ctx()), CellValue::Number(45000.5));
        assert_eq!(today(&[], &ctx()), CellValue::Number(45000.0));
    }

    #[test]
    fn date_basic_and_rolling() {
        // 2021-01-01 = serial 44197 (1900 system).
        assert_eq!(
            date(&[n(2021.0), n(1.0), n(1.0)], &ctx()),
            CellValue::Number(44197.0)
        );
        // Month roll: DATE(2020,13,1) = 2021-01-01.
        let s_2021_01_01 = match date(&[n(2020.0), n(13.0), n(1.0)], &ctx()) {
            CellValue::Number(x) => x,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            serial_to_ymd(s_2021_01_01, DateSystem::Date1900),
            Some((2021, 1, 1))
        );
        // Day roll: DATE(2020,1,32) = 2020-02-01.
        let s = match date(&[n(2020.0), n(1.0), n(32.0)], &ctx()) {
            CellValue::Number(x) => x,
            other => panic!("{other:?}"),
        };
        assert_eq!(serial_to_ymd(s, DateSystem::Date1900), Some((2020, 2, 1)));
        // Year shorthand: DATE(20,1,1) -> 1920-01-01.
        let s = match date(&[n(20.0), n(1.0), n(1.0)], &ctx()) {
            CellValue::Number(x) => x,
            other => panic!("{other:?}"),
        };
        assert_eq!(serial_to_ymd(s, DateSystem::Date1900), Some((1920, 1, 1)));
    }

    #[test]
    fn weekday_modes() {
        // 2021-01-01 (serial 44197) is a Friday.
        assert_eq!(weekday(&[n(44197.0)], &ctx()), CellValue::Number(6.0)); // type1: Fri=6
        assert_eq!(
            weekday(&[n(44197.0), n(2.0)], &ctx()),
            CellValue::Number(5.0)
        ); // type2: Fri=5
        assert_eq!(
            weekday(&[n(44197.0), n(3.0)], &ctx()),
            CellValue::Number(4.0)
        ); // type3: Fri=4
        assert_eq!(
            weekday(&[n(44197.0), n(9.0)], &ctx()),
            CellValue::Error(CellError::Num)
        );
    }

    #[test]
    fn time_components() {
        // TIME(12,0,0) = 0.5.
        assert_eq!(
            time(&[n(12.0), n(0.0), n(0.0)], &ctx()),
            CellValue::Number(0.5)
        );
        // HOUR/MINUTE/SECOND of 45000.5 -> 12:00:00.
        assert_eq!(hour(&[n(45000.5)], &ctx()), CellValue::Number(12.0));
        assert_eq!(minute(&[n(45000.5)], &ctx()), CellValue::Number(0.0));
        assert_eq!(second(&[n(45000.5)], &ctx()), CellValue::Number(0.0));
    }
}
