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
// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
#![allow(non_snake_case)]

//! Regression tests for defects the Excel oracle lanes found and Wave 3b
//! fixed (the oracle lanes themselves pin the agreement counts; these pin
//! the mechanism in a form that names it).

use sheet_js::core::SheetSession;

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../corpus/xlsx-recalc")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// POI's SharedFormulaTest: `DY2` is the master `DZ2*2` of `DY2:DY8`, the
/// members carry only `si="1"`. Each member must evaluate ITS row, and a
/// dirty re-encode (any edit on the sheet) must write each member's own
/// formula — not the master text, which would point every row at row 2.
#[test]
fn shared_formula_members_shift_and_survive_a_dirty_save__feat__sheet_xlsx_roundtrip() {
    let mut s = SheetSession::load_xlsx(&fixture("SharedFormulaTest.xls.xlsx")).unwrap();
    for r in 2..=8u32 {
        assert_eq!(
            s.get_cell_input(0, r - 1, 128),
            format!("=DZ{r}*2"),
            "DY{r} after load"
        );
    }
    let before: Vec<String> = (1..8).map(|r| s.get_cell_display(0, r, 128)).collect();
    s.set_cell(0, 20, 0, "1").unwrap(); // dirties sheet 1
    let bytes = s.save_xlsx().unwrap();
    let r = SheetSession::load_xlsx(&bytes).unwrap();
    for row in 2..=8u32 {
        assert_eq!(
            r.get_cell_input(0, row - 1, 128),
            format!("=DZ{row}*2"),
            "DY{row} after save + reload"
        );
    }
    let after: Vec<String> = (1..8).map(|row| r.get_cell_display(0, row, 128)).collect();
    assert_eq!(before, after);
}
