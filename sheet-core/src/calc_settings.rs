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

//! Workbook calculation settings (spec §5.1). Mirrors the OOXML
//! `<calcPr>` knobs the engine honors in T0: the date epoch, the document
//! locale, and the iterative-calculation parameters.

/// Per-workbook calc configuration. Defaults match Excel's out-of-box
/// values (1900 date system, en-US locale, iteration off, 100 iterations,
/// 0.001 delta).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CalcSettings {
    pub date_system: DateSystem,
    /// Document display locale (spec §9 / D-8). Drives the separators and
    /// month/day names the formatter renders; the formula DIALECT stays en
    /// regardless. The xlsx external-link/localization track wires parsing
    /// of the workbook's declared locale (`sheet.xlsx.external-link`/
    /// `sheet.format.locale.locale-from-workbook`); the default keeps
    /// existing behavior byte-identical.
    pub locale: Locale,
    pub iterative: bool,
    pub max_iter: u32,
    pub max_change: f64,
}

/// Display locale for the number-format engine (spec §9; D-8 v1 set =
/// en/de minimum, extended to the Western-European Latin set fr/es/it).
/// Affects the RENDERED separators + month/day/AM-PM names only — the
/// format-code token grammar (`yyyy`/`mm`/`dd`, `0`, `#`, `.`) stays
/// locale-neutral, and the formula dialect is always en. The locale-data
/// table lives in `sheet-format/src/locale.rs`.
///
/// SCOPE NOTE: this set is the Latin-script European tier (decimal/group
/// swap + Gregorian month/day names + en AM/PM token). CJK locales (ja/zh,
/// era calendars, `午前`/`午後` AM/PM) are a scoped v2 follow-up — see the
/// `sheet.format.locale.cjk-followup` ruling in `registry/features/locale.yaml`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Locale {
    /// English (United States): `.` decimal, `,` group, `January`..,
    /// `Sunday`.., `AM`/`PM`.
    #[default]
    EnUs,
    /// German (Germany): `,` decimal, `.` group, `Januar`.., `Montag`..
    DeDe,
    /// French (France): `,` decimal, ` ` (space) group, `janvier`..,
    /// `dimanche`.., en `AM`/`PM` token.
    FrFr,
    /// Spanish (Spain): `,` decimal, `.` group, `enero`.., `domingo`..,
    /// en `AM`/`PM` token.
    EsEs,
    /// Italian (Italy): `,` decimal, `.` group, `gennaio`.., `domenica`..,
    /// en `AM`/`PM` token.
    ItIt,
}

/// The serial-date epoch (ECMA-376 §18.17.4). `Date1900` carries the 1900
/// leap-year bug (serial 60); `Date1904` is the Mac epoch.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum DateSystem {
    #[default]
    Date1900,
    Date1904,
}

impl Default for CalcSettings {
    fn default() -> Self {
        CalcSettings {
            date_system: DateSystem::Date1900,
            locale: Locale::EnUs,
            iterative: false,
            max_iter: 100,
            max_change: 0.001,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_defaults() {
        let c = CalcSettings::default();
        assert_eq!(c.date_system, DateSystem::Date1900);
        assert_eq!(c.locale, Locale::EnUs);
        assert!(!c.iterative);
        assert_eq!(c.max_iter, 100);
        assert_eq!(c.max_change, 0.001);
    }

    #[test]
    fn locale_default_is_en_us() {
        assert_eq!(Locale::default(), Locale::EnUs);
    }
}
