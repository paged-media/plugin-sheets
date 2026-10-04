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

// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
// Test names end in `__feat__<cockpit id>` or carry the registry fn name.
#![allow(non_snake_case)]

//! Functions added from the recalc corpus's unregistered list, every
//! expected value recorded on Excel 16 by the oracle lane
//! (`oracle/excel/recorded/stat.tsv`; the goldens replay the same cases).

use sheet_js::core::SheetSession;

fn session(cells: &[(u32, u32, &str)]) -> SheetSession {
    let mut s = SheetSession::new();
    for (r, c, v) in cells {
        s.set_cell(0, *r, *c, v).unwrap();
    }
    s
}

fn eval(s: &mut SheetSession, f: &str) -> String {
    s.set_cell(0, 50, 10, f).unwrap();
    s.get_cell_display(0, 50, 10)
}

#[test]
fn sheet_fn_stat_percentrank__feat__sheet_fn_library() {
    let data = [13, 12, 11, 8, 4, 3, 2, 1, 1, 1];
    let cells: Vec<(u32, u32, String)> = data
        .iter()
        .enumerate()
        .map(|(i, v)| (i as u32, 0, v.to_string()))
        .collect();
    let refs: Vec<(u32, u32, &str)> = cells.iter().map(|(r, c, v)| (*r, *c, v.as_str())).collect();
    let mut s = session(&refs);
    // Excel 16: exact 0.555, interpolated 0.583, 2 digits 0.66, outside #N/A.
    assert_eq!(eval(&mut s, "=PERCENTRANK(A1:A10,4)"), "0.555");
    assert_eq!(eval(&mut s, "=PERCENTRANK(A1:A10,5)"), "0.583");
    assert_eq!(eval(&mut s, "=PERCENTRANK.INC(A1:A10,8,2)"), "0.66");
    assert_eq!(eval(&mut s, "=PERCENTRANK(A1:A10,0)"), "#N/A");
    let mut s = session(
        &(1..=10)
            .map(|i| {
                (
                    i - 1,
                    1u32,
                    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"][i as usize - 1],
                )
            })
            .collect::<Vec<_>>(),
    );
    // Excel 16: PERCENTRANK.EXC(1..10, 7) is 0.636, (…, 5.5) is 0.5.
    assert_eq!(eval(&mut s, "=PERCENTRANK.EXC(B1:B10,7)"), "0.636");
    assert_eq!(eval(&mut s, "=PERCENTRANK.EXC(B1:B10,5.5)"), "0.5");
}

#[test]
fn sheet_fn_stat_linest__feat__sheet_fn_library() {
    let mut s = session(&[
        (0, 0, "1"),
        (1, 0, "2"),
        (2, 0, "3"),
        (3, 0, "4"),
        (4, 0, "5"),
        (0, 1, "1"),
        (1, 1, "0"),
        (2, 1, "1"),
        (3, 1, "0"),
        (4, 1, "2"),
        (0, 3, "2.1"),
        (1, 3, "3.9"),
        (2, 3, "6.2"),
        (3, 3, "7.8"),
        (4, 3, "10.1"),
    ]);
    // Excel 16 (recorded): slope 1.99, df 3, F 1110.308…, multi m2 0.18333….
    assert_eq!(eval(&mut s, "=INDEX(LINEST(D1:D5,A1:A5),1)"), "1.99");
    assert_eq!(
        eval(&mut s, "=INDEX(LINEST(D1:D5,A1:A5,TRUE,TRUE),4,2)"),
        "3"
    );
    assert_eq!(
        eval(&mut s, "=ROUND(INDEX(LINEST(D1:D5,A1:A5,TRUE,TRUE),4,1),6)"),
        "1110.308411"
    );
    assert_eq!(
        eval(&mut s, "=ROUND(INDEX(LINEST(D1:D5,A1:B5),1),9)"),
        "0.183333333"
    );
    assert_eq!(
        eval(&mut s, "=INDEX(LINEST(D1:D5,A1:B5,TRUE,TRUE),3,3)"),
        "#N/A"
    );
}
