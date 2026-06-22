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
//! ## The de-DE AM/PM ruling (resolved — `sheet.format.locale.ampm`)
//!
//! German convention is the 24-hour clock, so a German format code normally
//! omits the AM/PM token entirely (`HH:mm`). The honest question is what to
//! emit when a de-DE code *does* carry an explicit `AM/PM` token. **Ruling:**
//! the AM/PM token is honored LITERALLY as the en `"AM"`/`"PM"` strings under
//! de-DE — Excel's German locale does not substitute `vorm.`/`nachm.` for the
//! AM/PM format token (it emits `"AM"`/`"PM"`), so the de table mirrors en for
//! `am`/`pm`/`am_short`/`pm_short`. The 24-hour preference lives in the
//! AUTHORING (drop the token), not in the engine swapping in German strings.
//! This keeps a localized round-trip stable: a code with AM/PM renders the same
//! marker glyphs in either locale, and only the separators/calendar names move.
//!
//! ## Population status
//!
//! en-US is the only fully-exercised entry (it is the default and every M0
//! golden renders through it byte-identically). de-DE data (separators +
//! month/day names + AM/PM + default date pattern) ships here, and the de
//! RENDERING branches in `number.rs`/`datetime.rs` read this table; the
//! `locale-de.golden.tsv` corpus + `tests/locale.rs` exercise the de path
//! end-to-end. A `[$<symbol>-<LCID>]` token in a format code carries a
//! per-code locale ([`locale_from_lcid`]) that overrides the document locale.
//!
//! The Western-European Latin tier (**fr-FR / es-ES / it-IT**) is a pure
//! data-table expansion of this same machinery: each is a [`LocaleData`] const
//! row plus a [`locale_data`]/[`locale_from_lcid`] arm — no rendering-code
//! change, since `number.rs`/`datetime.rs` already read every separator/name
//! from the table. They share the de-DE AM/PM ruling (an explicit AM/PM token
//! renders the literal en `"AM"`/`"PM"`; the 24-hour convention lives in the
//! AUTHORING, not the engine). CJK locales (ja/zh — era calendars and
//! `午前`/`午後` AM/PM) are a scoped v2 follow-up (registry
//! `sheet.format.locale.cjk-followup`), NOT a table row here, because their
//! calendar/AM-PM semantics need oracle verification beyond a separator swap.

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

// ---- fr-FR (Western-European Latin tier; data-fill, table-read only). ----

const FR_MONTHS_FULL: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
// French abbreviations (Excel fr-FR / CLDR): three-letter lowercase forms.
const FR_MONTHS_ABBR: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
    "déc.",
];
const FR_DAYS_FULL: [&str; 7] = [
    "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
];
const FR_DAYS_ABBR: [&str; 7] = ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."];

const FR_FR: LocaleData = LocaleData {
    decimal: ",",
    // Excel fr-FR groups with a (regular) space, e.g. `1 234 567`.
    group: " ",
    list: ";",
    months_full: &FR_MONTHS_FULL,
    months_abbr: &FR_MONTHS_ABBR,
    days_full: &FR_DAYS_FULL,
    days_abbr: &FR_DAYS_ABBR,
    // Shares the de-DE ruling: an explicit AM/PM token renders the literal en
    // markers (the 24-hour convention lives in the authoring, not the engine).
    am: "AM",
    pm: "PM",
    am_short: "A",
    pm_short: "P",
    short_date: "dd/mm/yyyy",
};

// ---- es-ES (Western-European Latin tier; data-fill, table-read only). ----

const ES_MONTHS_FULL: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];
const ES_MONTHS_ABBR: [&str; 12] = [
    "ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sep.", "oct.", "nov.", "dic.",
];
const ES_DAYS_FULL: [&str; 7] = [
    "domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado",
];
const ES_DAYS_ABBR: [&str; 7] = ["dom.", "lun.", "mar.", "mié.", "jue.", "vie.", "sáb."];

const ES_ES: LocaleData = LocaleData {
    decimal: ",",
    group: ".",
    list: ";",
    months_full: &ES_MONTHS_FULL,
    months_abbr: &ES_MONTHS_ABBR,
    days_full: &ES_DAYS_FULL,
    days_abbr: &ES_DAYS_ABBR,
    am: "AM",
    pm: "PM",
    am_short: "A",
    pm_short: "P",
    short_date: "dd/mm/yyyy",
};

// ---- it-IT (Western-European Latin tier; data-fill, table-read only). ----

const IT_MONTHS_FULL: [&str; 12] = [
    "gennaio",
    "febbraio",
    "marzo",
    "aprile",
    "maggio",
    "giugno",
    "luglio",
    "agosto",
    "settembre",
    "ottobre",
    "novembre",
    "dicembre",
];
const IT_MONTHS_ABBR: [&str; 12] = [
    "gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
];
const IT_DAYS_FULL: [&str; 7] = [
    "domenica",
    "lunedì",
    "martedì",
    "mercoledì",
    "giovedì",
    "venerdì",
    "sabato",
];
const IT_DAYS_ABBR: [&str; 7] = ["dom", "lun", "mar", "mer", "gio", "ven", "sab"];

const IT_IT: LocaleData = LocaleData {
    decimal: ",",
    group: ".",
    list: ";",
    months_full: &IT_MONTHS_FULL,
    months_abbr: &IT_MONTHS_ABBR,
    days_full: &IT_DAYS_FULL,
    days_abbr: &IT_DAYS_ABBR,
    am: "AM",
    pm: "PM",
    am_short: "A",
    pm_short: "P",
    short_date: "dd/mm/yyyy",
};

/// Resolve the [`LocaleData`] table for a [`Locale`]. `'static` — a const
/// table lookup, no allocation. en-US is the default and the only entry
/// exercised by every M0 golden (so en stays byte-identical).
pub fn locale_data(locale: Locale) -> &'static LocaleData {
    match locale {
        Locale::EnUs => &EN_US,
        Locale::DeDe => &DE_DE,
        Locale::FrFr => &FR_FR,
        Locale::EsEs => &ES_ES,
        Locale::ItIt => &IT_IT,
    }
}

/// Map an OOXML LCID (locale id) to a [`Locale`] (spec §9; ECMA-376 §18.8.30,
/// the `[$<symbol>-<LCID>]` currency/locale modifier). LCIDs are 16-bit; the
/// LOW 10 bits are the *primary* language id (the rest are the sublanguage /
/// sort id), so we mask to the primary language: `0x07` = German,
/// `0x09` = English, `0x0c` = French, `0x0a` = Spanish, `0x10` = Italian.
/// Only the modelled set (en/de/fr/es/it) is mapped; ANY other LCID resolves
/// to [`Locale::EnUs`] (the default — keeps an unmodelled locale's
/// separators/names the en defaults rather than failing).
///
/// Examples: `0x0407` (de-DE) and `0x0807` (de-CH) → [`Locale::DeDe`];
/// `0x040c` (fr-FR) and `0x0c0c` (fr-CA) → [`Locale::FrFr`]; `0x040a` (es-ES)
/// → [`Locale::EsEs`]; `0x0410` (it-IT) → [`Locale::ItIt`]; `0x0409` (en-US),
/// `0x0809` (en-GB), and the absent/zero LCID → [`Locale::EnUs`].
pub fn locale_from_lcid(lcid: u32) -> Locale {
    match lcid & 0x03ff {
        0x07 => Locale::DeDe,
        0x0c => Locale::FrFr,
        0x0a => Locale::EsEs,
        0x10 => Locale::ItIt,
        _ => Locale::EnUs,
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

    #[test]
    fn sheet_format_locale_ampm_de_mirrors_en() {
        // RULING (sheet.format.locale.ampm): de-DE honors the AM/PM token as the
        // literal en "AM"/"PM" strings (Excel de does not localize the token).
        let de = locale_data(Locale::DeDe);
        let en = locale_data(Locale::EnUs);
        assert_eq!(de.am, en.am);
        assert_eq!(de.pm, en.pm);
        assert_eq!(de.am, "AM");
        assert_eq!(de.pm, "PM");
        assert_eq!(de.am_short, "A");
        assert_eq!(de.pm_short, "P");
    }

    #[test]
    fn sheet_format_locale_from_workbook_lcid() {
        // The [$-LCID] token's primary-language bits pick the locale; the
        // sublanguage bits are masked off (de-CH 0x0807 → de, en-GB 0x0809 → en).
        assert_eq!(locale_from_lcid(0x0407), Locale::DeDe); // de-DE
        assert_eq!(locale_from_lcid(0x0807), Locale::DeDe); // de-CH
        assert_eq!(locale_from_lcid(0x0409), Locale::EnUs); // en-US
        assert_eq!(locale_from_lcid(0x0809), Locale::EnUs); // en-GB
                                                            // The Western-European Latin tier maps by primary language too
                                                            // (sublang masked: fr-CA 0x0c0c → fr).
        assert_eq!(locale_from_lcid(0x040c), Locale::FrFr); // fr-FR
        assert_eq!(locale_from_lcid(0x0c0c), Locale::FrFr); // fr-CA
        assert_eq!(locale_from_lcid(0x040a), Locale::EsEs); // es-ES
        assert_eq!(locale_from_lcid(0x0410), Locale::ItIt); // it-IT
                                                            // A still-unmodelled LCID (e.g. ja-JP 0x0411 — the CJK v2 follow-up)
                                                            // falls back to en-US.
        assert_eq!(locale_from_lcid(0x0411), Locale::EnUs);
        // The bare/zero LCID is en-US (default).
        assert_eq!(locale_from_lcid(0), Locale::EnUs);
    }
}
