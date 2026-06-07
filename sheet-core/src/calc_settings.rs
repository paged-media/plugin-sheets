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
//! `<calcPr>` knobs the engine honors in T0: the date epoch and the
//! iterative-calculation parameters.

/// Per-workbook calc configuration. Defaults match Excel's out-of-box
/// values (1900 date system, iteration off, 100 iterations, 0.001 delta).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CalcSettings {
    pub date_system: DateSystem,
    pub iterative: bool,
    pub max_iter: u32,
    pub max_change: f64,
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
        assert!(!c.iterative);
        assert_eq!(c.max_iter, 100);
        assert_eq!(c.max_change, 0.001);
    }
}
