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

//! Locale-data tables for the number-format engine (spec §9; D-8 v1 set =
//! en/de minimum). The [`Locale`] enum itself lives in `sheet-core` (so
//! [`sheet_core::CalcSettings`] can carry a document locale); this module
//! owns the *data*: decimal/group/list separators, long+short month and day
//! names, AM/PM strings, and the locale's default short-date pattern.
//!
//! ## What localizes, and what does NOT (spec §9)
//!
//! - **Localizes:** the RENDERED separators (`.`/`,` swap between en-US and
//!   de-DE), month/day names (`January` vs `Januar`), and AM/PM strings.
//! - **Does NOT localize:** the format-code token grammar — `yyyy`/`mm`/`dd`,
//!   the digit placeholders `0`/`#`/`?`, the literal `.` decimal token and
//!   `,` grouping token in a *code* are locale-NEUTRAL tokens; the locale
//!   only decides which glyph the renderer emits for them. The formula
//!   DIALECT is always en (only VALUE parsing/formatting localizes).
//!
//! ## Population status (Phase A)
//!
//! en-US is the only fully-exercised entry (it is the default and every M0
//! golden renders through it byte-identically). de-DE data (separators +
//! month/day names + AM/PM + default date pattern) ships here now so the
//! Phase B localization track is mostly tests + corpus; the de RENDERING
//! branches in `number.rs`/`datetime.rs` simply read this table.

pub use sheet_core::Locale;

/// The resolved data for one [`Locale`]: separators, calendar names, AM/PM,
/// and the default short-date pattern. Returned by [`locale_data`] as a
/// `'static` borrow into the const tables below — no allocation.
#[derive(Copy, Clone, Debug)]
pub struct LocaleData {
    /// Decimal separator (en-US `"."`, de-DE `","`).
    pub decimal: &'static str,
    /// Group (thousands) separator (en-US `","`, de-DE `"."`).
    pub group: &'static str,
    /// List/argument separator (en-US `","`, de-DE `";"`). Display-only here;
    /// the formula parser's argument separator is locale-NEUTRAL (`,`).
    pub list: &'static str,
    /// Long month names, January..December (index 0 = January).
    pub months_full: &'static [&'static str; 12],
    /// Short/abbreviated month names, Jan..Dec.
    pub months_abbr: &'static [&'static str; 12],
    /// Long day names, Sunday..Saturday (index 0 = Sunday, matching the
    /// weekday index used by [`crate::datetime`]).
    pub days_full: &'static [&'static str; 7],
    /// Short/abbreviated day names, Sun..Sat.
    pub days_abbr: &'static [&'static str; 7],
    /// AM marker (long form, e.g. `"AM"`).
    pub am: &'static str,
    /// PM marker (long form, e.g. `"PM"`).
    pub pm: &'static str,
    /// AM marker (short form, the single-letter `A`/`P` style).
    pub am_short: &'static str,
    /// PM marker (short form).
    pub pm_short: &'static str,
    /// The locale's default short-date pattern, expressed in the
    /// locale-NEUTRAL token grammar (so the codes stay portable; the locale
    /// only swaps the rendered separators/names). en-US `"m/d/yyyy"`,
    /// de-DE `"dd.mm.yyyy"`.
    pub short_date: &'static str,
}

// ---- en-US (the default; every M0 golden renders through this). ----

const EN_MONTHS_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const EN_MONTHS_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const EN_DAYS_FULL: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const EN_DAYS_ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

const EN_US: LocaleData = LocaleData {
    decimal: ".",
    group: ",",
    list: ",",
    months_full: &EN_MONTHS_FULL,
    months_abbr: &EN_MONTHS_ABBR,
    days_full: &EN_DAYS_FULL,
    days_abbr: &EN_DAYS_ABBR,
    am: "AM",
    pm: "PM",
    am_short: "A",
    pm_short: "P",
    short_date: "m/d/yyyy",
};

// ---- de-DE (Phase A data-fill; the rendering branches read this table). ----

const DE_MONTHS_FULL: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];
// German abbreviations: three-letter forms with a trailing period in
// Excel's de-DE locale; März abbreviates to "Mrz", the rest "Jan".. .
const DE_MONTHS_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mrz", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
];
const DE_DAYS_FULL: [&str; 7] = [
    "Sonntag",
    "Montag",
    "Dienstag",
    "Mittwoch",
    "Donnerstag",
    "Freitag",
    "Samstag",
];
const DE_DAYS_ABBR: [&str; 7] = ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"];

const DE_DE: LocaleData = LocaleData {
    decimal: ",",
    group: ".",
    list: ";",
    months_full: &DE_MONTHS_FULL,
    months_abbr: &DE_MONTHS_ABBR,
    days_full: &DE_DAYS_FULL,
    days_abbr: &DE_DAYS_ABBR,
    am: "AM",
    pm: "PM",
    am_short: "A",
    pm_short: "P",
    short_date: "dd.mm.yyyy",
};

/// Resolve the [`LocaleData`] table for a [`Locale`]. `'static` — a const
/// table lookup, no allocation. en-US is the default and the only entry
/// exercised by every M0 golden (so en stays byte-identical).
pub fn locale_data(locale: Locale) -> &'static LocaleData {
    match locale {
        Locale::EnUs => &EN_US,
        Locale::DeDe => &DE_DE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_format_locale_separators_en_unchanged() {
        // en-US: the existing behavior — "." decimal, "," group, "," list.
        let en = locale_data(Locale::EnUs);
        assert_eq!(en.decimal, ".");
        assert_eq!(en.group, ",");
        assert_eq!(en.list, ",");
    }

    #[test]
    fn sheet_format_locale_separators_de() {
        // de-DE swaps decimal/group and uses ";" as the list separator.
        let de = locale_data(Locale::DeDe);
        assert_eq!(de.decimal, ",");
        assert_eq!(de.group, ".");
        assert_eq!(de.list, ";");
    }

    #[test]
    fn sheet_format_locale_month_day_names_en() {
        let en = locale_data(Locale::EnUs);
        assert_eq!(en.months_full[0], "January");
        assert_eq!(en.months_abbr[0], "Jan");
        assert_eq!(en.days_full[1], "Monday");
        assert_eq!(en.days_abbr[1], "Mon");
    }

    #[test]
    fn sheet_format_locale_month_day_names_de() {
        let de = locale_data(Locale::DeDe);
        assert_eq!(de.months_full[0], "Januar");
        assert_eq!(de.months_full[2], "März");
        assert_eq!(de.days_full[1], "Montag");
        assert_eq!(de.days_abbr[1], "Mo");
    }

    #[test]
    fn sheet_format_locale_default_is_en_us() {
        // Default Locale resolves to the en-US table — keeps every existing
        // FormatCtx::default() path byte-identical.
        let d = locale_data(Locale::default());
        assert_eq!(d.decimal, ".");
        assert_eq!(d.months_full[0], "January");
    }

    #[test]
    fn sheet_format_locale_default_date_patterns() {
        assert_eq!(locale_data(Locale::EnUs).short_date, "m/d/yyyy");
        assert_eq!(locale_data(Locale::DeDe).short_date, "dd.mm.yyyy");
    }
}
