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

//! Dependents of a volatile cell recompute with it. A volatile cell (RAND,
//! NOW, …) is reseeded dirty on every recalc; its readers must follow, or
//! `=A1*2` over `A1=RAND()` shows a stale value after the next edit.

// Test names end in `__feat__<cockpit id>` — the cockpit link (CLAUDE.md).
#![allow(non_snake_case)]

use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellValue, SheetModel};

fn num(e: &Engine, row: u32, col: u32) -> f64 {
    match e
        .model()
        .sheet(0)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
    {
        Some(CellValue::Number(n)) => n,
        other => panic!("({row},{col}) is not a number: {other:?}"),
    }
}

#[test]
fn volatile_dependents_recompute__feat__sheet_calc_engine() {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    let mut e = Engine::new(m, EngineConfig::default());
    e.enter(0, 0, 0, "=RAND()").unwrap();
    e.enter(0, 0, 1, "=A1*2").unwrap();
    e.enter(0, 0, 2, "=B1+1").unwrap(); // transitively volatile
    assert_eq!(num(&e, 0, 1), num(&e, 0, 0) * 2.0);
    for i in 0..5 {
        let before = num(&e, 0, 0);
        // An edit no formula reads still recalcs the volatile cell...
        e.enter(0, 5, 5, &i.to_string()).unwrap();
        assert_ne!(num(&e, 0, 0), before, "RAND moves every recalc");
        // ...and its readers follow it.
        assert_eq!(
            num(&e, 0, 1),
            num(&e, 0, 0) * 2.0,
            "B1 = A1*2 after edit {i}"
        );
        assert_eq!(
            num(&e, 0, 2),
            num(&e, 0, 1) + 1.0,
            "C1 = B1+1 after edit {i}"
        );
    }
}
