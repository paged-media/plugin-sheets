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

//! The batch door (`Engine::set_cells`) leaves the workbook exactly as the
//! same inputs entered one by one (`Engine::enter`) would — values, errors,
//! cycles — with one recalc instead of one per input.

// Test names end in `__feat__<cockpit id>` — the cockpit link (CLAUDE.md).
#![allow(non_snake_case)]

use proptest::prelude::*;
use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellValue, SheetModel};

fn engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    Engine::new(m, EngineConfig::default())
}

fn a1(row: u32, col: u32) -> String {
    format!("{}{}", (b'A' + col as u8) as char, row + 1)
}

/// One input on a 6×4 grid: a number, a blank, a cell reference, a sum over
/// a box, or an IF over a comparison (cycles are possible and intended).
fn input() -> impl Strategy<Value = (u32, u32, String)> {
    let cell = (0u32..6, 0u32..4);
    (
        cell.clone(),
        prop_oneof![
            (-50i32..50).prop_map(|n| n.to_string()),
            Just(String::new()),
            cell.clone().prop_map(|(r, c)| format!("={}+1", a1(r, c))),
            (cell.clone(), cell.clone()).prop_map(|((r0, c0), (r1, c1))| format!(
                "=SUM({}:{})",
                a1(r0, c0),
                a1(r1, c1)
            )),
            (cell.clone(), cell).prop_map(|((r0, c0), (r1, c1))| format!(
                "=IF({}>{},1,2)",
                a1(r0, c0),
                a1(r1, c1)
            )),
        ],
    )
        .prop_map(|((r, c), s)| (r, c, s))
}

fn grid(e: &Engine) -> Vec<CellValue> {
    let ws = e.model().sheet(0).unwrap();
    (0..6)
        .flat_map(|r| (0..4).map(move |c| (r, c)))
        .map(|(r, c)| {
            ws.cell(r, c)
                .map(|x| x.value.clone())
                .unwrap_or(CellValue::Empty)
        })
        .collect()
}

proptest! {
    #[test]
    fn set_cells_equals_sequential_enter__feat__sheet_calc_engine(
        seed in proptest::collection::vec(input(), 0..12),
        batch in proptest::collection::vec(input(), 1..16),
    ) {
        let mut one = engine();
        let mut many = engine();
        for (r, c, s) in &seed {
            one.enter(0, *r, *c, s).unwrap();
            many.enter(0, *r, *c, s).unwrap();
        }
        for (r, c, s) in &batch {
            one.enter(0, *r, *c, s).unwrap();
        }
        let parsed: Vec<_> = batch
            .iter()
            .map(|(r, c, s)| (0u16, *r, *c, many.parse_input(0, s).unwrap()))
            .collect();
        many.set_cells(parsed);
        prop_assert_eq!(grid(&one), grid(&many));
    }
}
