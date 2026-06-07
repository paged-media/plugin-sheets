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

//! Property tests for serial<->calendar round-tripping (spec §9). These live
//! in `sheet-format`'s own test target (not `sheet-conformance`) so the
//! conformance crate stays self-contained and `proptest` never leaks into a
//! shipping dependency graph.

use proptest::prelude::*;
use sheet_core::DateSystem;
use sheet_format::serial::{serial_to_ymd, ymd_to_serial};

// 1900 valid integer serial domain: 1 ..= 2958465 (9999-12-31). Includes the
// phantom serial 60 — it MUST round-trip.
const MAX_1900: i64 = 2958465;
// 1904 valid integer serial domain: 0 ..= 2957003 (9999-12-31).
const MAX_1904: i64 = 2957003;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    /// serial -> ymd -> serial is the identity across the whole 1900 domain,
    /// phantom serial 60 included.
    #[test]
    fn serial_ymd_serial_1900(n in 1i64..=MAX_1900) {
        let s = n as f64;
        let (y, m, d) = serial_to_ymd(s, DateSystem::Date1900)
            .expect("in-domain serial must convert");
        let back = ymd_to_serial(y, m, d, DateSystem::Date1900)
            .expect("derived ymd must convert back");
        prop_assert_eq!(back, s, "serial {} -> {:?} -> {}", s, (y, m, d), back);
    }

    /// serial -> ymd -> serial identity across the whole 1904 domain.
    #[test]
    fn serial_ymd_serial_1904(n in 0i64..=MAX_1904) {
        let s = n as f64;
        let (y, m, d) = serial_to_ymd(s, DateSystem::Date1904)
            .expect("in-domain serial must convert");
        let back = ymd_to_serial(y, m, d, DateSystem::Date1904)
            .expect("derived ymd must convert back");
        prop_assert_eq!(back, s, "serial {} -> {:?} -> {}", s, (y, m, d), back);
    }

    /// ymd -> serial -> ymd identity for valid civil dates in [1900, 9999]
    /// (the in-domain calendar) under the 1900 system. The phantom day is
    /// excluded here (it is not a civil date) and covered explicitly below.
    #[test]
    fn ymd_serial_ymd_1900(
        y in 1900i32..=9999,
        m in 1u32..=12,
        d in 1u32..=28, // <=28 is valid in every month, every year
    ) {
        let s = ymd_to_serial(y, m, d, DateSystem::Date1900)
            .expect("valid civil date must convert");
        let (yy, mm, dd) = serial_to_ymd(s, DateSystem::Date1900)
            .expect("serial must convert back");
        prop_assert_eq!((yy, mm, dd), (y, m, d));
    }

    /// ymd -> serial -> ymd identity under the 1904 system.
    #[test]
    fn ymd_serial_ymd_1904(
        y in 1904i32..=9999,
        m in 1u32..=12,
        d in 1u32..=28,
    ) {
        let s = ymd_to_serial(y, m, d, DateSystem::Date1904)
            .expect("valid civil date must convert");
        let (yy, mm, dd) = serial_to_ymd(s, DateSystem::Date1904)
            .expect("serial must convert back");
        prop_assert_eq!((yy, mm, dd), (y, m, d));
    }
}

#[test]
fn phantom_day_round_trips() {
    // serial 60 == 1900-02-29 (the leap bug); ymd_to_serial accepts the
    // phantom date and returns 60.
    assert_eq!(
        serial_to_ymd(60.0, DateSystem::Date1900),
        Some((1900, 2, 29))
    );
    assert_eq!(ymd_to_serial(1900, 2, 29, DateSystem::Date1900), Some(60.0));
    // The phantom day does NOT exist in the 1904 system.
    assert_eq!(ymd_to_serial(1900, 2, 29, DateSystem::Date1904), None);
}

#[test]
fn out_of_domain_is_none() {
    assert_eq!(serial_to_ymd(0.0, DateSystem::Date1900), None); // < serial 1
    assert_eq!(serial_to_ymd(-1.0, DateSystem::Date1904), None);
    assert_eq!(
        serial_to_ymd((MAX_1900 + 1) as f64, DateSystem::Date1900),
        None
    );
    assert_eq!(ymd_to_serial(10000, 1, 1, DateSystem::Date1900), None);
    assert_eq!(ymd_to_serial(2021, 2, 30, DateSystem::Date1900), None);
}
