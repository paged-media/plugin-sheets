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

//! The M1 date-arithmetic family (spec §7/§11/§13 M1; registry
//! `sheet.fn.date.*` M1 additions). Twelve pure
//! `fn(&[Arg], &EvalCtx) -> CellValue` kernels that compute *over* the
//! workbook serial axis the M0 [`date`](super::date) family established:
//! month arithmetic (`EDATE`/`EOMONTH`), span measures
//! (`DATEDIF`/`DAYS`/`DAYS360`/`YEARFRAC`), business-day calendars
//! (`NETWORKDAYS`/`WORKDAY`), string parsing (`DATEVALUE`/`TIMEVALUE`), and
//! week numbering (`WEEKNUM`/`ISOWEEKNUM`).
//!
//! ## Where the rulings live
//!
//! All serial <-> calendar conversion is delegated to
//! [`sheet_format::serial`] (the FMT track owns the 1900/1904 epochs and the
//! adopted 1900 leap-year-bug ruling, [`DateSystem`]); these kernels add only
//! the Excel *function* semantics on top and honor [`EvalCtx::date_system`].
//! Coercion routes through [`crate::coerce`] so the conversion rulings are
//! stated once; scalar errors propagate first-error-wins via
//! [`coerce::first_error`], and a range arriving at a non-range-aware kernel
//! collapses to its top-left cell (implicit intersection already happened in
//! `sheet-calc`).
//!
//! ## Excel-compat rulings adopted here (bug-for-bug; each tested)
//!
//! - **DAYS360 US (NASD) end-of-month rule.** The standard 30/360 US rule
//!   (ECMA-376 §18.17.4): if the start day is 31 (or the last day of
//!   February), set it to 30; then if the end day is 31 *and* the adjusted
//!   start day is ≥ 30, set the end day to 30. February-end collapse applies
//!   to the start date only.
//! - **DATEDIF reversed range.** `DATEDIF` returns `#NUM!` when `end <
//!   start` (Microsoft's documented behavior) — it is the lone span function
//!   that rejects a reversed interval (`DAYS`/`DAYS360`/`YEARFRAC` accept it
//!   and return a signed/absolute result per Excel).
//! - **YEARFRAC basis-1 (actual/actual) denominator.** Excel's heuristic:
//!   when start and end fall in the same year, the denominator is that year's
//!   day count (366 for a leap year); spanning multiple years, it is the
//!   *average* year length over the span. This reproduces Excel's published
//!   actual/actual values.
//! - **Serial-domain rejection.** A negative serial argument (below the
//!   epoch) is `#NUM!`, mirroring the M0 calendar decomposers.
//!
//! ## DATEVALUE / TIMEVALUE accepted formats (T1)
//!
//! `DATEVALUE` accepts ISO `yyyy-mm-dd`, US `mm/dd/yyyy` (and `m/d/yy`, with
//! the same `0..=29 -> 2000+yy`, `30..=99 -> 1900+yy` window Excel uses).
//! `TIMEVALUE` accepts `hh:mm` and `hh:mm:ss` (24-hour). An unparseable
//! string is `#VALUE!` (the documented ruling). A leading date in a
//! TIMEVALUE string and AM/PM markers are *not* accepted in T1 — out of
//! scope, surfaced as `#VALUE!`.

use sheet_core::{CellError, CellValue, DateSystem};
use sheet_format::serial::{serial_to_ymd, ymd_to_serial};

use crate::arg::Arg;
use crate::coerce;

// ---- shared argument plumbing ----------------------------------------------

/// Coerce one scalar argument to a number, collapsing a range to its top-left
/// cell (a date2 scalar kernel is never `range_aware`, so a range arriving
/// here is the degenerate post-intersection case — see [`super::date`]). An
/// error cell propagates.
fn arg_number(a: &Arg) -> Result<f64, CellError> {
    match a {
        Arg::Scalar(v) => coerce::to_number(v),
        Arg::Range(r) => coerce::to_number(&r.get(0, 0)),
    }
}

/// Coerce one scalar argument to its General text form (for the string
/// parsers). A range collapses to its top-left cell; an error cell becomes
/// its display token (the parser then fails it as `#VALUE!`).
fn arg_text(a: &Arg) -> Result<compact_str::CompactString, CellError> {
    match a {
        Arg::Scalar(CellValue::Error(e)) => Err(*e),
        Arg::Scalar(v) => Ok(coerce::to_text(v)),
        Arg::Range(r) => {
            let v = r.get(0, 0);
            if let CellValue::Error(e) = v {
                Err(e)
            } else {
                Ok(coerce::to_text(&v))
            }
        }
    }
}

/// Read a serial argument, rejecting a negative serial (below the epoch) as
/// `#NUM!` — the M0 decomposer convention. Truncates the fractional time.
fn arg_serial(a: &Arg) -> Result<i64, CellError> {
    let n = arg_number(a)?;
    if n < 0.0 {
        return Err(CellError::Num);
    }
    Ok(n.floor() as i64)
}

/// Decompose a serial into `(y, m, d)` for the given system, mapping an
/// out-of-domain serial to `#NUM!`.
fn ymd_of(serial: i64, sys: DateSystem) -> Result<(i32, u32, u32), CellError> {
    serial_to_ymd(serial as f64, sys).ok_or(CellError::Num)
}

/// Real Gregorian day-of-week as `0 = Monday .. 6 = Sunday` for a calendar
/// date — used by the business-day and week-number kernels (which key off the
/// *real* weekday, not Excel's serial-index off-by-one). Howard Hinnant's
/// `days_from_civil` mod 7 with the 1970-01-01 (a Thursday = 3) anchor.
fn weekday_mon0(y: i32, m: u32, d: u32) -> i64 {
    // Days since 1970-01-01 (a Thursday). Local copy of the FMT-track civil
    // algorithm restricted to weekday extraction.
    let yy = if m <= 2 { y - 1 } else { y };
    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
    let yoe = (yy - era * 400) as i64;
    let mi = m as i64;
    let di = d as i64;
    let doy = (153 * (if mi > 2 { mi - 3 } else { mi + 9 }) + 2) / 5 + di - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era as i64 * 146097 + doe - 719468;
    // 1970-01-01 is Thursday => Monday-based index 3.
    (days + 3).rem_euclid(7)
}

/// Whether `(y, m, d)` is a weekend (Saturday or Sunday) in the standard
/// Excel business-week (the only week-mask T1 supports).
fn is_weekend(y: i32, m: u32, d: u32) -> bool {
    weekday_mon0(y, m, d) >= 5
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_in_year(y: i32) -> i64 {
    if is_leap(y) {
        366
    } else {
        365
    }
}

// ---- EDATE / EOMONTH -------------------------------------------------------

/// Add `months` whole months to `(y, m)`, clamping the day to the target
/// month's length (Excel's `EDATE` semantics; ECMA-376 §18.17.7). Returns the
/// serial of the resulting date, or `#NUM!` if it leaves the valid domain.
fn add_months(serial: i64, months: i64, clamp_to_eom: bool, sys: DateSystem) -> CellValue {
    let (y, m, d) = match ymd_of(serial, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    let total = (y as i64) * 12 + (m as i64 - 1) + months;
    let ny = total.div_euclid(12);
    let nm = (total.rem_euclid(12) + 1) as u32;
    let Ok(ny_i32) = i32::try_from(ny) else {
        return CellValue::Error(CellError::Num);
    };
    let last = days_in_month(ny_i32, nm);
    let nd = if clamp_to_eom { last } else { d.min(last) };
    match ymd_to_serial(ny_i32, nm, nd, sys) {
        Some(s) => CellValue::Number(s),
        None => CellValue::Error(CellError::Num),
    }
}

/// `EDATE(start_date, months)` — the serial `months` whole months after
/// `start_date`, keeping the day-of-month (clamped to the target month's
/// length). ECMA-376 §18.17.7. Registry `sheet.fn.date.edate`.
pub fn edate(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let start = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let months = match arg_number(&args[1]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    add_months(start, months, false, ctx.date_system)
}

/// `EOMONTH(start_date, months)` — the serial of the *last day* of the month
/// `months` after `start_date`. ECMA-376 §18.17.7. Registry
/// `sheet.fn.date.eomonth`.
pub fn eomonth(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let start = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let months = match arg_number(&args[1]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    add_months(start, months, true, ctx.date_system)
}

// ---- DATEDIF ---------------------------------------------------------------

/// `DATEDIF(start_date, end_date, unit)` — the elapsed interval between two
/// dates in the requested `unit` (Microsoft public docs):
///
/// - **`"Y"`** complete calendar years; **`"M"`** complete calendar months;
///   **`"D"`** elapsed days (`end - start`).
/// - **`"MD"`** day difference, months and years ignored.
/// - **`"YM"`** month difference, years ignored.
/// - **`"YD"`** day difference, years ignored.
///
/// `end < start` is `#NUM!` (the documented ruling, unique to DATEDIF). An
/// unknown unit is `#NUM!`. Case-insensitive unit. Registry
/// `sheet.fn.date.datedif`.
pub fn datedif(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let start = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let end = match arg_serial(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let unit = match arg_text(&args[2]) {
        Ok(t) => t.trim().to_ascii_uppercase(),
        Err(e) => return CellValue::Error(e),
    };
    if end < start {
        return CellValue::Error(CellError::Num);
    }
    let sys = ctx.date_system;
    let (sy, sm, sd) = match ymd_of(start, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    let (ey, em, ed) = match ymd_of(end, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };

    let result: i64 = match unit.as_str() {
        "D" => end - start,
        "Y" => complete_years(sy, sm, sd, ey, em, ed),
        "M" => complete_months(sy, sm, sd, ey, em, ed),
        "MD" => {
            // Day-of-month difference; borrow from the previous month if the
            // end day is before the start day.
            if ed >= sd {
                (ed - sd) as i64
            } else {
                // Days in the month before the end month (where the borrow
                // comes from).
                let (py, pm) = if em == 1 { (ey - 1, 12) } else { (ey, em - 1) };
                (days_in_month(py, pm) + ed - sd) as i64
            }
        }
        "YM" => {
            // Whole months ignoring years: months between (sm,sd) and (em,ed)
            // within a year, borrowing a day if needed.
            let mut months = em as i64 - sm as i64;
            if ed < sd {
                months -= 1;
            }
            months.rem_euclid(12)
        }
        "YD" => {
            // Day difference ignoring years: place the start in the same year
            // as the end (or the next, if it would be after the end).
            let cand_year = ey;
            let start_in_end_year =
                ymd_to_serial(cand_year, sm, sd.min(days_in_month(cand_year, sm)), sys);
            match start_in_end_year {
                Some(anchor) => {
                    let mut diff = end as f64 - anchor;
                    if diff < 0.0 {
                        // Roll the anchor back one year.
                        let py = cand_year - 1;
                        if let Some(prev) =
                            ymd_to_serial(py, sm, sd.min(days_in_month(py, sm)), sys)
                        {
                            diff = end as f64 - prev;
                        }
                    }
                    diff as i64
                }
                None => return CellValue::Error(CellError::Num),
            }
        }
        _ => return CellValue::Error(CellError::Num),
    };
    CellValue::Number(result as f64)
}

/// Count of complete years between two civil dates (the DATEDIF "Y" rule):
/// the year difference, minus one if the end's (month, day) has not yet
/// reached the start's anniversary.
fn complete_years(sy: i32, sm: u32, sd: u32, ey: i32, em: u32, ed: u32) -> i64 {
    let mut years = ey as i64 - sy as i64;
    if (em, ed) < (sm, sd) {
        years -= 1;
    }
    years
}

/// Count of complete months between two civil dates (the DATEDIF "M" rule):
/// the total month difference, minus one if the end day has not yet reached
/// the start day-of-month.
fn complete_months(sy: i32, sm: u32, sd: u32, ey: i32, em: u32, ed: u32) -> i64 {
    let mut months = (ey as i64 - sy as i64) * 12 + (em as i64 - sm as i64);
    if ed < sd {
        months -= 1;
    }
    months
}

// ---- DAYS / DAYS360 --------------------------------------------------------

/// `DAYS(end_date, start_date)` — the count of days between two serials
/// (`end - start`, signed). Note the argument order: **end first** (Microsoft
/// public docs). Registry `sheet.fn.date.days`.
pub fn days(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let end = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let start = match arg_serial(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    CellValue::Number((end - start) as f64)
}

/// `DAYS360(start_date, end_date, [method])` — the day count on the 30/360
/// basis (ECMA-376 §18.17.4): `method` `FALSE`/omitted = US (NASD),
/// `TRUE` = European. Registry `sheet.fn.date.days360`.
pub fn days360(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let start = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let end = match arg_serial(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let european = if args.len() >= 3 {
        let method = match &args[2] {
            Arg::Scalar(v) => v.clone(),
            Arg::Range(r) => r.get(0, 0),
        };
        match coerce::to_bool(&method) {
            Ok(b) => b,
            Err(e) => return CellValue::Error(e),
        }
    } else {
        false
    };
    let sys = ctx.date_system;
    let (sy, sm, sd) = match ymd_of(start, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    let (ey, em, ed) = match ymd_of(end, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    let n = days360_count(sy, sm, sd, ey, em, ed, european);
    CellValue::Number(n as f64)
}

/// The 30/360 day count between two civil dates. `european` selects the EU
/// rule (any day-31 simply becomes 30); otherwise the US (NASD) rule applies
/// (start-day February-end and 31 collapse to 30, then the end day-31 only
/// collapses when the adjusted start is ≥ 30).
fn days360_count(
    sy: i32,
    sm: u32,
    mut sd: u32,
    ey: i32,
    em: u32,
    mut ed: u32,
    european: bool,
) -> i64 {
    if european {
        if sd == 31 {
            sd = 30;
        }
        if ed == 31 {
            ed = 30;
        }
    } else {
        // US/NASD: a start on the last day of February becomes day 30.
        if sm == 2 && sd == days_in_month(sy, 2) {
            sd = 30;
        }
        if sd == 31 {
            sd = 30;
        }
        // The end day collapses to 30 only when the (adjusted) start is 30/31.
        if ed == 31 && sd >= 30 {
            ed = 30;
        }
    }
    (ey as i64 - sy as i64) * 360 + (em as i64 - sm as i64) * 30 + (ed as i64 - sd as i64)
}

// ---- NETWORKDAYS / WORKDAY -------------------------------------------------

/// Gather the holiday serials from an optional trailing argument (a scalar or
/// a range). Errors propagate; non-numeric cells inside a range are skipped
/// (Excel ignores text/blank holiday cells). A scalar holiday is coerced.
fn collect_holidays(arg: Option<&Arg>, sys: DateSystem) -> Result<Vec<i64>, CellError> {
    let mut out = Vec::new();
    let Some(arg) = arg else {
        return Ok(out);
    };
    match arg {
        Arg::Scalar(v) => {
            let n = coerce::to_number(v)?;
            if n >= 0.0 && serial_to_ymd(n.floor(), sys).is_some() {
                out.push(n.floor() as i64);
            } else {
                return Err(CellError::Num);
            }
        }
        Arg::Range(r) => {
            for cell in r.iter() {
                match cell {
                    CellValue::Error(e) => return Err(e),
                    CellValue::Number(n) => {
                        if n >= 0.0 {
                            out.push(n.floor() as i64);
                        }
                    }
                    // Text / Bool / Empty holiday cells are skipped.
                    _ => {}
                }
            }
        }
    }
    Ok(out)
}

/// Whether `serial` is a working day: not a weekend and not in `holidays`.
fn is_workday(serial: i64, holidays: &[i64], sys: DateSystem) -> Option<bool> {
    let (y, m, d) = serial_to_ymd(serial as f64, sys)?;
    Some(!is_weekend(y, m, d) && !holidays.contains(&serial))
}

/// `NETWORKDAYS(start_date, end_date, [holidays])` — the count of whole
/// working days in `[start, end]` inclusive, excluding weekends and any
/// holidays. Order-insensitive: if `end < start` the count is negated
/// (Excel). ECMA-376 §18.17.7. Registry `sheet.fn.date.networkdays`
/// (`range_aware`: the holidays argument is a range).
pub fn networkdays(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    // Only the first two (scalar) args participate in first-error; the
    // holidays RANGE is scanned by collect_holidays (which propagates).
    if let Some(e) = coerce::first_error(&args[..2.min(args.len())]) {
        return CellValue::Error(e);
    }
    let a = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let b = match arg_serial(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let sys = ctx.date_system;
    let holidays = match collect_holidays(args.get(2), sys) {
        Ok(h) => h,
        Err(e) => return CellValue::Error(e),
    };

    let (lo, hi, sign) = if a <= b { (a, b, 1i64) } else { (b, a, -1i64) };
    let mut count = 0i64;
    for serial in lo..=hi {
        match is_workday(serial, &holidays, sys) {
            Some(true) => count += 1,
            Some(false) => {}
            None => return CellValue::Error(CellError::Num),
        }
    }
    CellValue::Number((count * sign) as f64)
}

/// `WORKDAY(start_date, days, [holidays])` — the serial that is `days` working
/// days from `start_date` (positive = forward, negative = backward), skipping
/// weekends and holidays. The start day itself is never counted. ECMA-376
/// §18.17.7. Registry `sheet.fn.date.workday` (`range_aware`: holidays range).
pub fn workday(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(&args[..2.min(args.len())]) {
        return CellValue::Error(e);
    }
    let start = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let days = match arg_number(&args[1]) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return CellValue::Error(e),
    };
    let sys = ctx.date_system;
    let holidays = match collect_holidays(args.get(2), sys) {
        Ok(h) => h,
        Err(e) => return CellValue::Error(e),
    };

    if days == 0 {
        // WORKDAY(start, 0) returns the start serial unchanged (Excel).
        return CellValue::Number(start as f64);
    }
    let step = if days > 0 { 1 } else { -1 };
    let mut remaining = days.abs();
    let mut serial = start;
    while remaining > 0 {
        serial += step;
        if serial < 0 {
            return CellValue::Error(CellError::Num);
        }
        match is_workday(serial, &holidays, sys) {
            Some(true) => remaining -= 1,
            Some(false) => {}
            None => return CellValue::Error(CellError::Num),
        }
    }
    CellValue::Number(serial as f64)
}

// ---- YEARFRAC --------------------------------------------------------------

/// `YEARFRAC(start_date, end_date, [basis])` — the fraction of a year between
/// two dates on the requested day-count `basis` (ECMA-376 §18.17.7):
///
/// - **0** US (NASD) 30/360 (default).
/// - **1** actual/actual.
/// - **2** actual/360.
/// - **3** actual/365.
/// - **4** European 30/360.
///
/// Order-insensitive (the magnitude of the interval is used). An out-of-range
/// basis is `#NUM!`. Registry `sheet.fn.date.yearfrac`.
pub fn yearfrac(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let mut start = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let mut end = match arg_serial(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let basis = if args.len() >= 3 {
        match arg_number(&args[2]) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return CellValue::Error(e),
        }
    } else {
        0
    };
    if start == end {
        return CellValue::Number(0.0);
    }
    // YEARFRAC is order-insensitive: work with start <= end.
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let sys = ctx.date_system;
    let (sy, sm, sd) = match ymd_of(start, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    let (ey, em, ed) = match ymd_of(end, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };

    let frac = match basis {
        0 => {
            let d = days360_count(sy, sm, sd, ey, em, ed, false);
            d as f64 / 360.0
        }
        4 => {
            let d = days360_count(sy, sm, sd, ey, em, ed, true);
            d as f64 / 360.0
        }
        2 => (end - start) as f64 / 360.0,
        3 => (end - start) as f64 / 365.0,
        1 => {
            let actual = (end - start) as f64;
            let denom = actual_actual_denominator(sy, ey);
            actual / denom
        }
        _ => return CellValue::Error(CellError::Num),
    };
    CellValue::Number(frac)
}

/// The actual/actual (basis 1) year-length denominator (Excel's heuristic):
/// a single-year interval uses that year's length (366 if leap); a
/// multi-year interval uses the average year length over the inclusive span
/// of calendar years it touches.
fn actual_actual_denominator(sy: i32, ey: i32) -> f64 {
    if sy == ey {
        days_in_year(sy) as f64
    } else {
        let mut total = 0i64;
        for y in sy..=ey {
            total += days_in_year(y);
        }
        let years = (ey - sy + 1) as f64;
        total as f64 / years
    }
}

// ---- DATEVALUE / TIMEVALUE -------------------------------------------------

/// `DATEVALUE(date_text)` — parse a date string to its serial (T1 accepts ISO
/// `yyyy-mm-dd` and US `mm/dd/yyyy` / `m/d/yy`). An unparseable string is
/// `#VALUE!`. ECMA-376 §18.17.7. Registry `sheet.fn.date.datevalue`.
pub fn datevalue(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    let text = match arg_text(&args[0]) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    match parse_date(text.trim(), ctx.date_system) {
        Some(serial) => CellValue::Number(serial),
        None => CellValue::Error(CellError::Value),
    }
}

/// `TIMEVALUE(time_text)` — parse a time-of-day string (`hh:mm[:ss]`, 24-hour)
/// to its fractional-day serial in `[0, 1)`. An unparseable string is
/// `#VALUE!`. ECMA-376 §18.17.7. Registry `sheet.fn.date.timevalue`.
pub fn timevalue(args: &[Arg], _ctx: &crate::ctx::EvalCtx) -> CellValue {
    let text = match arg_text(&args[0]) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    match parse_time(text.trim()) {
        Some(frac) => CellValue::Number(frac),
        None => CellValue::Error(CellError::Value),
    }
}

/// Apply Excel's two-digit-year window: `00..=29 -> 2000..=2029`,
/// `30..=99 -> 1930..=1999`.
fn expand_two_digit_year(yy: i32) -> i32 {
    if yy <= 29 {
        2000 + yy
    } else {
        1900 + yy
    }
}

/// Parse a date string per the T1 [`datevalue`] ruling. Accepts ISO
/// `yyyy-mm-dd` (4-digit year, `-` separators) and US `mm/dd/yyyy` (or
/// `m/d/yy`, applying [`expand_two_digit_year`]). Returns the serial for
/// `sys`, or `None` if the string does not match or names an impossible date.
fn parse_date(s: &str, sys: DateSystem) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    // ISO yyyy-mm-dd.
    if let Some((y, m, d)) = split3(s, '-') {
        // ISO requires a 4-digit (>= 1000-style) leading year component.
        if y >= 100 {
            return ymd_to_serial(y, m as u32, d as u32, sys);
        }
        return None;
    }
    // US mm/dd/yyyy or m/d/yy.
    if let Some((m, d, y)) = split3(s, '/') {
        let year = if y < 100 { expand_two_digit_year(y) } else { y };
        return ymd_to_serial(year, m as u32, d as u32, sys);
    }
    None
}

/// Split `s` into exactly three integer components on `sep` (each a
/// non-negative decimal integer). `None` on a different shape.
fn split3(s: &str, sep: char) -> Option<(i32, i32, i32)> {
    let mut it = s.split(sep);
    let a = it.next()?.trim();
    let b = it.next()?.trim();
    let c = it.next()?.trim();
    if it.next().is_some() {
        return None;
    }
    let a: i32 = a.parse().ok()?;
    let b: i32 = b.parse().ok()?;
    let c: i32 = c.parse().ok()?;
    if a < 0 || b < 0 || c < 0 {
        return None;
    }
    Some((a, b, c))
}

/// Parse a 24-hour time string `hh:mm` or `hh:mm:ss` to a fractional day in
/// `[0, 1)`. Components are range-checked (`hh < 24`, `mm < 60`, `ss < 60`);
/// `None` on any other shape. Seconds default to 0.
fn parse_time(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    let mut it = s.split(':');
    let h: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let sec: i64 = match it.next() {
        Some(part) => part.trim().parse().ok()?,
        None => 0,
    };
    if it.next().is_some() {
        return None;
    }
    if !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..60).contains(&sec) {
        return None;
    }
    let total = h * 3600 + m * 60 + sec;
    Some(total as f64 / 86_400.0)
}

// ---- WEEKNUM / ISOWEEKNUM --------------------------------------------------

/// `WEEKNUM(serial_number, [return_type])` — the calendar week of the year
/// (ECMA-376 §18.17.7). T1 supports the two common system-1 modes plus the
/// ISO system:
///
/// - **1** (default) and **17**: weeks start Sunday; week 1 contains Jan 1.
/// - **2** and **11**: weeks start Monday; week 1 contains Jan 1.
/// - **12**..=**16**: weeks start Tue..Sat (the System-1 family).
/// - **21**: ISO-8601 (Monday-start, week 1 = the week with the year's first
///   Thursday) — equivalent to [`isoweeknum`].
///
/// Any other `return_type` is `#NUM!`. Registry `sheet.fn.date.weeknum`.
pub fn weeknum(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let serial = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let return_type = if args.len() >= 2 {
        match arg_number(&args[1]) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return CellValue::Error(e),
        }
    } else {
        1
    };
    let sys = ctx.date_system;
    let (y, m, d) = match ymd_of(serial, sys) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };

    // The weekday (Mon=0..Sun=6) on which the System-1 week begins.
    let week_start_mon0: i64 = match return_type {
        1 | 17 => 6, // Sunday
        2 | 11 => 0, // Monday
        12 => 1,     // Tuesday
        13 => 2,     // Wednesday
        14 => 3,     // Thursday
        15 => 4,     // Friday
        16 => 5,     // Saturday
        21 => return CellValue::Number(iso_week_of(y, m, d) as f64),
        _ => return CellValue::Error(CellError::Num),
    };
    CellValue::Number(system1_week_of(y, m, d, week_start_mon0) as f64)
}

/// `ISOWEEKNUM(serial_number)` — the ISO-8601 week number (weeks start
/// Monday; week 1 is the week containing the year's first Thursday).
/// Microsoft public docs. Registry `sheet.fn.date.isoweeknum`.
pub fn isoweeknum(args: &[Arg], ctx: &crate::ctx::EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let serial = match arg_serial(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let (y, m, d) = match ymd_of(serial, ctx.date_system) {
        Ok(t) => t,
        Err(e) => return CellValue::Error(e),
    };
    CellValue::Number(iso_week_of(y, m, d) as f64)
}

/// Ordinal day-of-year (1-based) for a civil date.
fn day_of_year(y: i32, m: u32, d: u32) -> i64 {
    let mut doy = d as i64;
    for mm in 1..m {
        doy += days_in_month(y, mm) as i64;
    }
    doy
}

/// The System-1 week number: week 1 is the week (starting on
/// `week_start_mon0`) that contains January 1. Each subsequent boundary
/// increments the week. Returns a value in `1..=54`.
fn system1_week_of(y: i32, m: u32, d: u32, week_start_mon0: i64) -> i64 {
    // Day-of-week (Mon=0..Sun=6) of January 1 of this year.
    let jan1_dow = weekday_mon0(y, 1, 1);
    // Offset of Jan 1 within its week (0 = it IS the week start).
    let jan1_offset = (jan1_dow - week_start_mon0).rem_euclid(7);
    let doy = day_of_year(y, m, d);
    // Days since the start of the first (partial) week, +offset to align the
    // first week's start before Jan 1.
    (doy - 1 + jan1_offset) / 7 + 1
}

/// The ISO-8601 week number for a civil date. Uses the standard algorithm:
/// the week containing the date's Thursday determines the ISO year and week.
fn iso_week_of(y: i32, m: u32, d: u32) -> i64 {
    let dow = weekday_mon0(y, m, d); // Mon=0..Sun=6
    let doy = day_of_year(y, m, d);
    // Thursday of this week (ISO weeks belong to the year of their Thursday).
    let week = (doy - (dow + 1) + 10) / 7;
    if week < 1 {
        // Belongs to the last week of the previous year.
        iso_weeks_in_year(y - 1)
    } else if week > iso_weeks_in_year(y) {
        // Belongs to week 1 of the next year.
        1
    } else {
        week
    }
}

/// The number of ISO weeks in a year (52, or 53 for "long" years — those
/// whose Jan 1 is a Thursday, or a leap year whose Jan 1 is a Wednesday).
fn iso_weeks_in_year(y: i32) -> i64 {
    let jan1 = weekday_mon0(y, 1, 1); // Mon=0..Sun=6
    if jan1 == 3 || (is_leap(y) && jan1 == 2) {
        53
    } else {
        52
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arg::RangeView;
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

    fn t(s: &str) -> Arg<'static> {
        Arg::Scalar(CellValue::from(s))
    }

    fn num(v: CellValue) -> f64 {
        match v {
            CellValue::Number(x) => x,
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn edate_clamps_day() {
        // 2021-01-31 + 1 month -> 2021-02-28 (clamped). 2021-01-31 = 44227.
        let s = num(edate(&[n(44227.0), n(1.0)], &ctx()));
        assert_eq!(serial_to_ymd(s, DateSystem::Date1900), Some((2021, 2, 28)));
        // Backward: 2021-01-31 - 2 months -> 2020-11-30.
        let s = num(edate(&[n(44227.0), n(-2.0)], &ctx()));
        assert_eq!(serial_to_ymd(s, DateSystem::Date1900), Some((2020, 11, 30)));
    }

    #[test]
    fn eomonth_last_day() {
        // EOMONTH(2021-01-15, 0) -> 2021-01-31. 2021-01-15 = 44211.
        let s = num(eomonth(&[n(44211.0), n(0.0)], &ctx()));
        assert_eq!(serial_to_ymd(s, DateSystem::Date1900), Some((2021, 1, 31)));
        // +1 month into February (non-leap) -> 2021-02-28.
        let s = num(eomonth(&[n(44211.0), n(1.0)], &ctx()));
        assert_eq!(serial_to_ymd(s, DateSystem::Date1900), Some((2021, 2, 28)));
    }

    #[test]
    fn datedif_units() {
        // 2020-01-15 = 43845, 2021-03-20 = 44275.
        let start = 43845.0;
        let end = 44275.0;
        assert_eq!(num(datedif(&[n(start), n(end), t("Y")], &ctx())), 1.0);
        assert_eq!(num(datedif(&[n(start), n(end), t("M")], &ctx())), 14.0);
        assert_eq!(num(datedif(&[n(start), n(end), t("D")], &ctx())), 430.0);
        assert_eq!(num(datedif(&[n(start), n(end), t("MD")], &ctx())), 5.0);
        assert_eq!(num(datedif(&[n(start), n(end), t("YM")], &ctx())), 2.0);
    }

    #[test]
    fn datedif_reversed_is_num() {
        assert_eq!(
            datedif(&[n(44275.0), n(43845.0), t("Y")], &ctx()),
            CellValue::Error(CellError::Num)
        );
        // Unknown unit -> #NUM!.
        assert_eq!(
            datedif(&[n(43845.0), n(44275.0), t("Q")], &ctx()),
            CellValue::Error(CellError::Num)
        );
    }

    #[test]
    fn days_basic() {
        // DAYS(end, start): 44275 - 43845 = 430.
        assert_eq!(num(days(&[n(44275.0), n(43845.0)], &ctx())), 430.0);
        // Reversed -> negative.
        assert_eq!(num(days(&[n(43845.0), n(44275.0)], &ctx())), -430.0);
    }

    #[test]
    fn days360_us_and_eu() {
        // 2020-01-31 = 43861, 2020-03-31 = 43921.
        // US: start 31->30, end 31->30 (start>=30) => 2 months * 30 = 60.
        assert_eq!(num(days360(&[n(43861.0), n(43921.0)], &ctx())), 60.0);
        // EU: same here (both 31->30) => 60.
        assert_eq!(
            num(days360(
                &[n(43861.0), n(43921.0), Arg::Scalar(CellValue::Bool(true))],
                &ctx()
            )),
            60.0
        );
        // US end-of-Feb start collapse: 2021-02-28 = 44255 -> day 30,
        // 2021-03-31 = 44286 -> end 31 collapses (start adj >=30) -> 30.
        // months diff 1 *30 + (30-30) = 30.
        assert_eq!(num(days360(&[n(44255.0), n(44286.0)], &ctx())), 30.0);
    }

    #[test]
    fn networkdays_excludes_weekends() {
        // 2021-01-04 (Mon) = 44200 .. 2021-01-08 (Fri) = 44204 => 5 days.
        assert_eq!(num(networkdays(&[n(44200.0), n(44204.0)], &ctx())), 5.0);
        // Spanning a weekend: Mon..next Mon = 6 working days.
        assert_eq!(num(networkdays(&[n(44200.0), n(44207.0)], &ctx())), 6.0);
        // With a holiday inside (the Wed 44202) -> 4.
        assert_eq!(
            num(networkdays(&[n(44200.0), n(44204.0), n(44202.0)], &ctx())),
            4.0
        );
    }

    #[test]
    fn networkdays_holiday_range_and_reverse() {
        // Holiday range {44201, 44202}.
        let cells = [CellValue::Number(44201.0), CellValue::Number(44202.0)];
        let view = RangeView::from_slice(cr(), 1, 2, &cells);
        assert_eq!(
            num(networkdays(
                &[n(44200.0), n(44204.0), Arg::Range(view)],
                &ctx()
            )),
            3.0
        );
        // Reversed interval negates.
        assert_eq!(num(networkdays(&[n(44204.0), n(44200.0)], &ctx())), -5.0);
    }

    #[test]
    fn workday_skips_weekends() {
        // WORKDAY(Fri 44204, 1) -> Mon 44207.
        assert_eq!(num(workday(&[n(44204.0), n(1.0)], &ctx())), 44207.0);
        // WORKDAY(Mon 44200, -1) -> previous Fri 44197.
        assert_eq!(num(workday(&[n(44200.0), n(-1.0)], &ctx())), 44197.0);
        // 0 days -> start unchanged.
        assert_eq!(num(workday(&[n(44204.0), n(0.0)], &ctx())), 44204.0);
        // With a holiday on the would-be Monday 44207 -> next Tue 44208.
        assert_eq!(
            num(workday(&[n(44204.0), n(1.0), n(44207.0)], &ctx())),
            44208.0
        );
    }

    #[test]
    fn yearfrac_bases() {
        // 2020-01-01 = 43831, 2020-07-01 = 44013 (leap year).
        let start = 43831.0;
        let end = 44013.0;
        // Basis 0 (US 30/360): 180/360 = 0.5.
        assert!((num(yearfrac(&[n(start), n(end), n(0.0)], &ctx())) - 0.5).abs() < 1e-12);
        // Basis 4 (EU 30/360): 0.5.
        assert!((num(yearfrac(&[n(start), n(end), n(4.0)], &ctx())) - 0.5).abs() < 1e-12);
        // Basis 2 (actual/360): 182/360.
        assert!((num(yearfrac(&[n(start), n(end), n(2.0)], &ctx())) - 182.0 / 360.0).abs() < 1e-12);
        // Basis 3 (actual/365): 182/365.
        assert!((num(yearfrac(&[n(start), n(end), n(3.0)], &ctx())) - 182.0 / 365.0).abs() < 1e-12);
        // Basis 1 (actual/actual, same leap year): 182/366.
        assert!((num(yearfrac(&[n(start), n(end), n(1.0)], &ctx())) - 182.0 / 366.0).abs() < 1e-12);
    }

    #[test]
    fn yearfrac_bad_basis_is_num() {
        assert_eq!(
            yearfrac(&[n(43831.0), n(44013.0), n(9.0)], &ctx()),
            CellValue::Error(CellError::Num)
        );
    }

    #[test]
    fn datevalue_formats() {
        // ISO and US forms of 2021-01-01 = 44197.
        assert_eq!(num(datevalue(&[t("2021-01-01")], &ctx())), 44197.0);
        assert_eq!(num(datevalue(&[t("1/1/2021")], &ctx())), 44197.0);
        // Two-digit year window: 21 -> 2021.
        assert_eq!(num(datevalue(&[t("1/1/21")], &ctx())), 44197.0);
        // Unparseable -> #VALUE!.
        assert_eq!(
            datevalue(&[t("not a date")], &ctx()),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn timevalue_formats() {
        // 12:00 -> 0.5.
        assert!((num(timevalue(&[t("12:00")], &ctx())) - 0.5).abs() < 1e-12);
        // 06:00:00 -> 0.25.
        assert!((num(timevalue(&[t("06:00:00")], &ctx())) - 0.25).abs() < 1e-12);
        // Out-of-range -> #VALUE!.
        assert_eq!(
            timevalue(&[t("25:00")], &ctx()),
            CellValue::Error(CellError::Value)
        );
    }

    #[test]
    fn weeknum_and_iso() {
        // 2021-01-01 = 44197 (a Friday).
        // System-1 default (Sunday start): Jan 1 is in week 1.
        assert_eq!(num(weeknum(&[n(44197.0)], &ctx())), 1.0);
        // ISO: 2021-01-01 (Fri) belongs to ISO week 53 of 2020.
        assert_eq!(num(isoweeknum(&[n(44197.0)], &ctx())), 53.0);
        // WEEKNUM return_type 21 == ISO.
        assert_eq!(num(weeknum(&[n(44197.0), n(21.0)], &ctx())), 53.0);
        // 2021-01-04 (Mon) = 44200 -> ISO week 1.
        assert_eq!(num(isoweeknum(&[n(44200.0)], &ctx())), 1.0);
    }

    #[test]
    fn weeknum_bad_type_is_num() {
        assert_eq!(
            weeknum(&[n(44197.0), n(99.0)], &ctx()),
            CellValue::Error(CellError::Num)
        );
    }

    #[test]
    fn error_propagation() {
        // A scalar error in any leading position propagates.
        assert_eq!(
            edate(
                &[Arg::Scalar(CellValue::Error(CellError::Div0)), n(1.0)],
                &ctx()
            ),
            CellValue::Error(CellError::Div0)
        );
        // Negative serial -> #NUM!.
        assert_eq!(
            eomonth(&[n(-1.0), n(0.0)], &ctx()),
            CellValue::Error(CellError::Num)
        );
    }

    #[test]
    fn honors_date_system_1904() {
        let c = EvalCtx::new(DateSystem::Date1904, cr(), 45000.5, 42);
        // 1904-system serial for 2021-01-31 = 44227 - 1462 = 42765.
        let s = num(edate(&[n(42765.0), n(1.0)], &c));
        assert_eq!(serial_to_ymd(s, DateSystem::Date1904), Some((2021, 2, 28)));
    }
}
