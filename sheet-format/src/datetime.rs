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

//! Date/time section rendering (spec §9; ECMA-376 §18.8.31 date-time
//! codes). A cell value is treated as a serial (see [`crate::serial`]) when
//! its section [`classifies`](crate::sections::SectionKind::DateTime) as
//! date/time. Month/day/AM-PM names come from the locale-data table
//! ([`crate::locale`]); en-US is the default and keeps existing output
//! byte-identical, de-DE is data-filled for the Phase B localization track.

use crate::locale::LocaleData;
use crate::sections::{ElapsedUnit, Section, Token};
use crate::serial;
use sheet_core::DateSystem;
use std::fmt::Write as _;

/// Render a serial value through a date/time `section`. Returns `None` when
/// the serial is outside the valid calendar domain AND the section needs a
/// date (the caller then falls back to General). A time-only serial (integer
/// part `0`, the Excel pseudo-day 1900-01-00) still renders its time tokens.
/// Rounds the time-of-day to the nearest second; a carry that rolls past
/// midnight re-derives the date from the bumped serial. `loc` supplies the
/// localized month/day/AM-PM names (en-US default = unchanged output).
pub fn render_datetime(
    serial_val: f64,
    section: &Section,
    sys: DateSystem,
    loc: &LocaleData,
) -> Option<String> {
    if !serial_val.is_finite() || serial_val < 0.0 {
        return None;
    }
    // Round to the nearest second so a 23:59:59.7 does not under-render.
    let (h, mi, s) = serial::serial_to_hms(serial_val);
    // If the rounding carried into the next day, hms is 00:00:00 of the next
    // day; bump the integer serial accordingly for date derivation.
    let frac = serial_val - serial_val.floor();
    let carried = (frac * 86400.0).round() as i64 >= 86400;
    let date_serial = if carried {
        serial_val.floor() + 1.0
    } else {
        serial_val.floor()
    };

    let needs_date = section.tokens.iter().any(|t| {
        matches!(
            t,
            Token::Year4
                | Token::Year2
                | Token::Month { .. }
                | Token::MonthName { .. }
                | Token::Day { .. }
                | Token::DayName { .. }
        )
    });

    // serial::serial_to_ymd rejects NEGATIVE serials (both systems); under
    // 1900 serial 0 is the day-zero epoch 1900-01-00 (audit finding 4), so a
    // date-needing section over serial 0 renders "1900-01-00" rather than
    // falling back. We only bail when the section needs a date AND the serial
    // is genuinely out of domain.
    let date = serial::serial_to_ymd(date_serial, sys);
    if needs_date && date.is_none() {
        return None;
    }
    let (y, mon, day) = date.unwrap_or((1900, 1, 0));
    let weekday = weekday_of(date_serial, sys);

    // 12-hour clock when an AM/PM token is present.
    let has_ampm = section
        .tokens
        .iter()
        .any(|t| matches!(t, Token::AmPm { .. }));
    let (h12, pm) = to_12h(h);

    // Elapsed-time totals (spec §9, ruling `sheet.format.elapsed-brackets`).
    // Total accumulators over the WHOLE serial — not the modular wall clock.
    let elapsed = serial::serial_to_elapsed(serial_val).unwrap_or((0, 0, 0));

    let mut out = String::new();
    for t in &section.tokens {
        match t {
            Token::Literal(lit) => out.push_str(lit),
            Token::Year4 => {
                let _ = write!(out, "{y:04}");
            }
            Token::Year2 => {
                let _ = write!(out, "{:02}", (y % 100 + 100) % 100);
            }
            Token::Month { count } => {
                if *count >= 2 {
                    let _ = write!(out, "{mon:02}");
                } else {
                    let _ = write!(out, "{mon}");
                }
            }
            Token::MonthName { full } => {
                let idx = (mon as usize).saturating_sub(1).min(11);
                out.push_str(if *full {
                    loc.months_full[idx]
                } else {
                    loc.months_abbr[idx]
                });
            }
            Token::Day { pad } => {
                if *pad {
                    let _ = write!(out, "{day:02}");
                } else {
                    let _ = write!(out, "{day}");
                }
            }
            Token::DayName { full } => {
                out.push_str(if *full {
                    loc.days_full[weekday]
                } else {
                    loc.days_abbr[weekday]
                });
            }
            Token::Hour { pad } => {
                let hv = if has_ampm { h12 } else { h };
                if *pad {
                    let _ = write!(out, "{hv:02}");
                } else {
                    let _ = write!(out, "{hv}");
                }
            }
            Token::Minute { pad } => {
                if *pad {
                    let _ = write!(out, "{mi:02}");
                } else {
                    let _ = write!(out, "{mi}");
                }
            }
            Token::Second { pad } => {
                if *pad {
                    let _ = write!(out, "{s:02}");
                } else {
                    let _ = write!(out, "{s}");
                }
            }
            Token::AmPm { long } => {
                if *long {
                    out.push_str(if pm { loc.pm } else { loc.am });
                } else {
                    out.push_str(if pm { loc.pm_short } else { loc.am_short });
                }
            }
            Token::Elapsed { unit, pad } => {
                let total = match unit {
                    ElapsedUnit::Hours => elapsed.0,
                    ElapsedUnit::Minutes => elapsed.1,
                    ElapsedUnit::Seconds => elapsed.2,
                };
                let _ = write!(out, "{total:0pad$}");
            }
            Token::Fill(c) => out.push(*c),
            // Numeric/fraction tokens are not expected inside a date section; skip.
            _ => {}
        }
    }
    Some(out)
}

/// Day-of-week index (0 = Sunday) for a date serial. Excel serial 1
/// (1900-01-01) is a Sunday; we anchor on the civil date to stay correct
/// across the phantom-leap boundary.
fn weekday_of(date_serial: f64, sys: DateSystem) -> usize {
    if let Some((y, m, d)) = serial::serial_to_ymd(date_serial, sys) {
        // Sakamoto's algorithm (0 = Sunday).
        const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let yy = if m < 3 { y - 1 } else { y };
        let w = (yy + yy / 4 - yy / 100 + yy / 400 + T[(m - 1) as usize] + d as i32) % 7;
        (((w % 7) + 7) % 7) as usize
    } else {
        0
    }
}

/// 24-hour -> (12-hour clock value, is_pm). Hour 0 and 12 map to 12.
fn to_12h(h: u32) -> (u32, bool) {
    let pm = h >= 12;
    let h12 = match h % 12 {
        0 => 12,
        n => n,
    };
    (h12, pm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::compile;

    fn dt(code: &str, serial_val: f64) -> String {
        let f = compile(code).unwrap();
        render_datetime(
            serial_val,
            &f.pos,
            DateSystem::Date1900,
            crate::locale::locale_data(crate::Locale::EnUs),
        )
        .unwrap()
    }

    fn dt_de(code: &str, serial_val: f64) -> String {
        let f = compile(code).unwrap();
        render_datetime(
            serial_val,
            &f.pos,
            DateSystem::Date1900,
            crate::locale::locale_data(crate::Locale::DeDe),
        )
        .unwrap()
    }

    #[test]
    fn sheet_format_locale_de_month_day_names() {
        // serial 44197 = 2021-01-01 (a Friday).
        assert_eq!(dt_de("mmmm d, yyyy", 44197.0), "Januar 1, 2021");
        assert_eq!(dt_de("mmm", 44228.0), "Feb"); // 2021-02-01
        assert_eq!(dt_de("dddd", 44197.0), "Freitag");
        assert_eq!(dt_de("ddd", 44197.0), "Fr");
    }

    #[test]
    fn ymd() {
        // serial 1 = 1900-01-01
        assert_eq!(dt("yyyy-mm-dd", 1.0), "1900-01-01");
        // serial 44197 = 2021-01-01
        assert_eq!(dt("yyyy-mm-dd", 44197.0), "2021-01-01");
    }

    #[test]
    fn day_zero_serial0() {
        // Audit finding 4: serial 0 is Excel's day-zero epoch 1900-01-00 — the
        // date tokens render day 0 (zero-padded "00"), not General/#NUM!.
        assert_eq!(dt("yyyy-mm-dd", 0.0), "1900-01-00");
    }

    #[test]
    fn month_names() {
        assert_eq!(dt("mmm d, yyyy", 44197.0), "Jan 1, 2021");
        assert_eq!(dt("mmmm", 44228.0), "February");
    }

    #[test]
    fn day_names() {
        // 2021-01-01 was a Friday.
        assert_eq!(dt("dddd", 44197.0), "Friday");
        assert_eq!(dt("ddd", 44197.0), "Fri");
    }

    #[test]
    fn time_tokens() {
        // 0.5 = noon
        assert_eq!(dt("hh:mm:ss", 0.5), "12:00:00");
        // 0.75 = 18:00
        assert_eq!(dt("hh:mm", 0.75), "18:00");
    }

    #[test]
    fn ampm_12h() {
        assert_eq!(dt("h:mm AM/PM", 0.5), "12:00 PM");
        assert_eq!(dt("h:mm AM/PM", 0.0), "12:00 AM");
        assert_eq!(dt("h:mm AM/PM", 0.25), "6:00 AM");
    }
}
