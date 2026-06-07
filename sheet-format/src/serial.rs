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

//! Serial-date <-> calendar conversion (spec §9; ECMA-376 §18.17.4.1).
//!
//! Two date systems (see [`DateSystem`]):
//!
//! - **1900** (Excel/Windows default). Serial `1` = 1900-01-01, and serial
//!   `60` is the *phantom* 1900-02-29 — a deliberately adopted Excel-compat
//!   defect (registry ruling `sheet.format.date.leap1900`, KB214326): Excel
//!   inherited Lotus 1-2-3's belief that 1900 was a leap year. Serials
//!   `1..=59` map to 1900-01-01..1900-02-28, serial `60` renders as
//!   1900-02-29 (which never existed), and serial `61` is 1900-03-01.
//!   Serial `0` is Excel's day-zero epoch `1900-01-00` (a second adopted
//!   bug-for-bug ruling; audit finding 4): `YEAR/MONTH/DAY(0)` yield
//!   `1900/1/0`, `TEXT(0,"yyyy-mm-dd")` renders `"1900-01-00"`, and
//!   `DATE(1900,1,0)` produces serial `0`. Only NEGATIVE serials are rejected
//!   under 1900.
//! - **1904** (legacy Mac epoch). Serial `0` = 1904-01-01. No leap bug.
//!
//! Conversions use Howard Hinnant's branchless `days_from_civil` /
//! `civil_from_days` algorithm, which is valid across the whole proleptic
//! Gregorian calendar. The valid serial domain is the date range
//! `[epoch .. 9999-12-31]`; anything outside maps to `None`.

use sheet_core::DateSystem;

/// Days from the Unix-style civil epoch (0000-03-01 reckoning) used as a
/// stable integer axis. Hinnant's algorithm; correct for any proleptic
/// Gregorian (y, m, d) with `1 <= m <= 12` and a valid day. Returns days
/// relative to 1970-01-01.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as i64; // [0, 399]
    let m = m as i64;
    let d = d as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era as i64 * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`]: days-since-1970-01-01 -> (y, m, d).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

// The civil-day index of the two epochs (relative to 1970-01-01).
//
// 1900 system: Excel treats serial 1 as 1900-01-01 BUT counts a phantom
// 1900-02-29. The clean way to model this is: for serials >= 61 the offset
// is shifted by one day to absorb the nonexistent day. We anchor on
// 1899-12-31 = "serial 0" of the bug-aware axis, then special-case 60.
const EPOCH_1900_DAYS: i64 = -25568; // 1899-12-31 (serial 0 anchor, pre-bug)
const EPOCH_1904_DAYS: i64 = -24107; // 1904-01-01 (serial 0)

// Civil-day index of 1900-03-01 — the first day after the phantom leap day.
// Serial 61 must map here under the 1900 system.
const MAR1_1900_DAYS: i64 = -25508;

// The civil-day index of 9999-12-31 — the last in-domain date.
fn max_civil_days() -> i64 {
    days_from_civil(9999, 12, 31)
}

/// Calendar date -> serial number for `sys`. `None` when (y, m, d) is not a
/// real date in the valid domain, with ONE deliberate exception: under the
/// 1900 system `ymd_to_serial(1900, 2, 29, _)` returns `Some(60.0)` (the
/// phantom leap day). Time-of-day is not represented here (callers add the
/// fractional part via [`time_fraction`]).
pub fn ymd_to_serial(y: i32, m: u32, d: u32, sys: DateSystem) -> Option<f64> {
    if m == 0 || m > 12 || d == 0 || d > 31 {
        // Excel's day-zero epoch: 1900-01-00 IS serial 0 under the 1900
        // system (the symmetric inverse of `serial_to_ymd(0.0)`; audit
        // finding 4).
        if sys == DateSystem::Date1900 && y == 1900 && m == 1 && d == 0 {
            return Some(0.0);
        }
        // Phantom 1900-02-29 under the 1900 system is the lone valid
        // "impossible" date.
        if sys == DateSystem::Date1900 && y == 1900 && m == 2 && d == 29 {
            return Some(60.0);
        }
        return None;
    }
    // Reject (y, m, d) where d exceeds the real month length, again with the
    // phantom-day exception.
    if !is_valid_civil(y, m, d) {
        if sys == DateSystem::Date1900 && y == 1900 && m == 2 && d == 29 {
            return Some(60.0);
        }
        return None;
    }
    let days = days_from_civil(y, m, d);
    if days > max_civil_days() {
        return None;
    }
    match sys {
        DateSystem::Date1904 => {
            if days < EPOCH_1904_DAYS {
                return None;
            }
            Some((days - EPOCH_1904_DAYS) as f64)
        }
        DateSystem::Date1900 => {
            // Pre-bug serial relative to the 1899-12-31 anchor.
            let base = days - EPOCH_1900_DAYS;
            if base < 1 {
                return None;
            }
            // Dates on/after 1900-03-01 are shifted +1 by the phantom day.
            if days >= MAR1_1900_DAYS {
                Some((base + 1) as f64)
            } else {
                // 1900-01-01..1900-02-28 -> serials 1..=59, unshifted.
                Some(base as f64)
            }
        }
    }
}

/// Serial number -> (year, month, day) for `sys`. Truncates any fractional
/// time-of-day. `None` outside the valid domain. Under the 1900 system,
/// serial `60` returns `(1900, 2, 29)` (the phantom day; ruling
/// `sheet.format.date.leap1900`).
pub fn serial_to_ymd(serial: f64, sys: DateSystem) -> Option<(i32, u32, u32)> {
    if !serial.is_finite() {
        return None;
    }
    let n = serial.floor() as i64;
    match sys {
        DateSystem::Date1904 => {
            if n < 0 {
                return None;
            }
            let days = EPOCH_1904_DAYS + n;
            if days > max_civil_days() {
                return None;
            }
            Some(civil_from_days(days))
        }
        DateSystem::Date1900 => {
            if n < 0 {
                return None;
            }
            if n == 0 {
                // Excel's day-zero epoch: serial 0 IS 1900-01-00 (the literal
                // "1900-01-00" Excel renders, and the value DATE(1900,1,0)
                // produces). A deliberately adopted bug-for-bug ruling
                // (`sheet.format.date.serial-1900` day-zero note, audit
                // finding 4). The 1904 system keeps serial 0 = 1904-01-01.
                return Some((1900, 1, 0));
            }
            if n == 60 {
                // The phantom 1900-02-29.
                return Some((1900, 2, 29));
            }
            // Serials 1..=59 are unshifted; 61.. absorb the phantom day.
            let days = if n <= 59 {
                EPOCH_1900_DAYS + n
            } else {
                EPOCH_1900_DAYS + n - 1
            };
            if days > max_civil_days() {
                return None;
            }
            Some(civil_from_days(days))
        }
    }
}

/// True when (y, m, d) is a real Gregorian date (right day-of-month).
fn is_valid_civil(y: i32, m: u32, d: u32) -> bool {
    if m == 0 || m > 12 || d == 0 {
        return false;
    }
    d <= days_in_month(y, m)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Fractional day for a time-of-day. `0.5` == noon. Seconds may be
/// fractional. Does not range-check (a normalized 24h time gives `[0, 1)`).
pub fn time_fraction(h: u32, m: u32, s: f64) -> f64 {
    (h as f64 * 3600.0 + m as f64 * 60.0 + s) / 86400.0
}

/// Fractional part of a serial -> (h, m, s), rounded to the nearest second.
/// Rounding can carry: e.g. a fraction of 23:59:59.7 rounds to 24:00:00,
/// which this returns as `(0, 0, 0)` (the day rollover is the caller's
/// concern — see [`datetime`](crate::datetime), which re-derives the date
/// from the rounded serial).
pub fn serial_to_hms(serial: f64) -> (u32, u32, u32) {
    let frac = serial - serial.floor();
    let mut total = (frac * 86400.0).round() as i64; // nearest second
    if total >= 86400 {
        total -= 86400; // rolled into the next day
    }
    let h = (total / 3600) as u32;
    let m = ((total % 3600) / 60) as u32;
    let s = (total % 60) as u32;
    (h, m, s)
}
