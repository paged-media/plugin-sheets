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

//! Serial-date conformance (spec §9; registry `sheet.format.date.*`). The
//! 1900/1904 epochs and the deliberately-adopted 1900 leap-year bug (serial
//! 60 = the phantom 1900-02-29; ruling `sheet.format.date.leap1900`). The
//! exhaustive round-trip *property* tests live in
//! `sheet-format/tests/serial_props.rs` to keep this crate self-contained.

use sheet_core::DateSystem;
use sheet_format::serial::{serial_to_ymd, ymd_to_serial};

#[test]
fn sheet_format_date_serial_1900() {
    // Anchor + a spread of known Excel serials in the 1900 system.
    let cases = [
        (1.0, (1900, 1, 1)),         // epoch
        (2.0, (1900, 1, 2)),         //
        (59.0, (1900, 2, 28)),       // last serial before the phantom day
        (61.0, (1900, 3, 1)),        // first serial after the phantom day
        (366.0, (1900, 12, 31)),     // shifted +1 by the phantom day
        (367.0, (1901, 1, 1)),       //
        (44197.0, (2021, 1, 1)),     // a well-known modern serial
        (2958465.0, (9999, 12, 31)), // domain max
    ];
    for (serial, ymd) in cases {
        assert_eq!(
            serial_to_ymd(serial, DateSystem::Date1900),
            Some(ymd),
            "serial {serial} (1900)"
        );
        let (y, m, d) = ymd;
        assert_eq!(
            ymd_to_serial(y, m, d, DateSystem::Date1900),
            Some(serial),
            "ymd {ymd:?} (1900)"
        );
    }
    // Above the domain is None; NEGATIVE serials are None. Serial 0 is NOT
    // out of domain under 1900 — it is the day-zero epoch (see
    // `sheet_format_date_serial0_1900`).
    assert_eq!(serial_to_ymd(-1.0, DateSystem::Date1900), None);
    assert_eq!(serial_to_ymd(2958466.0, DateSystem::Date1900), None);
}

#[test]
fn sheet_format_date_serial0_1900() {
    // Audit finding 4: Excel's day-zero epoch. Under the 1900 system serial 0
    // IS 1900-01-00 (a deliberately adopted bug-for-bug ruling), with a
    // symmetric inverse. NEGATIVE serials remain rejected.
    assert_eq!(serial_to_ymd(0.0, DateSystem::Date1900), Some((1900, 1, 0)));
    assert_eq!(ymd_to_serial(1900, 1, 0, DateSystem::Date1900), Some(0.0));
    assert_eq!(serial_to_ymd(-1.0, DateSystem::Date1900), None);

    // The day-zero date does NOT exist in the 1904 system (serial 0 there is
    // 1904-01-01) — 1900-01-00 is rejected as a calendar date.
    assert_eq!(ymd_to_serial(1900, 1, 0, DateSystem::Date1904), None);
    assert_eq!(serial_to_ymd(0.0, DateSystem::Date1904), Some((1904, 1, 1)));
}

#[test]
fn sheet_format_date_leap1900() {
    // The leap-bug ruling (sheet.format.date.leap1900): serial 60 IS the
    // nonexistent 1900-02-29, with serials 59/61 straddling it correctly.
    assert_eq!(
        serial_to_ymd(59.0, DateSystem::Date1900),
        Some((1900, 2, 28))
    );
    assert_eq!(
        serial_to_ymd(60.0, DateSystem::Date1900),
        Some((1900, 2, 29))
    );
    assert_eq!(
        serial_to_ymd(61.0, DateSystem::Date1900),
        Some((1900, 3, 1))
    );

    // ymd_to_serial accepts the phantom date and returns 60.
    assert_eq!(ymd_to_serial(1900, 2, 29, DateSystem::Date1900), Some(60.0));

    // The phantom day does NOT exist in the 1904 system — it is rejected.
    assert_eq!(ymd_to_serial(1900, 2, 29, DateSystem::Date1904), None);
}

#[test]
fn sheet_format_date_serial_1904() {
    // 1904 system: serial 0 = 1904-01-01, NO leap bug, so 1900-02-29 is
    // simply invalid and the late-Feb-1900 dates shift back by the missing
    // phantom day relative to 1900.
    let cases = [
        (0.0, (1904, 1, 1)),         // epoch
        (1.0, (1904, 1, 2)),         //
        (1461.0, (1908, 1, 1)),      // first 1904-system leap cycle boundary
        (42735.0, (2021, 1, 1)),     // 44197 (1900) - 1462 = 42735 (1904)
        (2957003.0, (9999, 12, 31)), // domain max
    ];
    for (serial, ymd) in cases {
        assert_eq!(
            serial_to_ymd(serial, DateSystem::Date1904),
            Some(ymd),
            "serial {serial} (1904)"
        );
        let (y, m, d) = ymd;
        assert_eq!(
            ymd_to_serial(y, m, d, DateSystem::Date1904),
            Some(serial),
            "ymd {ymd:?} (1904)"
        );
    }
    assert_eq!(serial_to_ymd(-1.0, DateSystem::Date1904), None);
    assert_eq!(serial_to_ymd(2957004.0, DateSystem::Date1904), None);
}
