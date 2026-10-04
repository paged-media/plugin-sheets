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
#![allow(non_snake_case)]

//! XLSX round-trip PROPERTY: a generated workbook written by the engine and
//! read back is the same workbook (spec §10.2, "Paged never destroys a
//! workbook"). The hand fixtures in `xlsx_roundtrip.rs` pin byte identity for
//! fourteen files; this lane throws thousands of generated cell populations
//! at the writer — numbers at the edges of f64, text with XML specials,
//! whitespace, non-BMP characters and control characters, booleans, error
//! literals and formulas over them — through the real product path
//! (`SheetSession::set_cell` → `save_xlsx` → `load_xlsx`).
//!
//! Equal means: every cell's INPUT text (formula or literal) and DISPLAY text
//! match, and a second save of the reloaded workbook reloads equal again.

use proptest::prelude::*;
use sheet_js::core::SheetSession;

/// One generated cell input, already in the canonical form the session
/// prints back (`get_cell_input`), so equality is exact.
fn input() -> impl Strategy<Value = String> {
    prop_oneof![
        // Numbers: shortest round-trip text, including tiny/huge magnitudes.
        any::<f64>()
            .prop_filter("finite", |n| n.is_finite())
            // -0 is written as "-0" and read back as 0: Excel has no negative
            // zero either (it shows and stores 0), so that is not a loss.
            .prop_map(|n| if n == 0.0 { 0.0 } else { n })
            .prop_map(|n| n.to_string()),
        (-1_000_000i64..1_000_000).prop_map(|n| n.to_string()),
        // Text with XML specials, whitespace, astral-plane and control chars.
        // A leading letter keeps it from being a number/bool/error literal.
        "[a-zA-Z][ -~<>&\"'\\t\\n\u{e9}\u{4e2d}\u{1f600}\u{0}\u{b}]{0,24}",
        Just("TRUE".to_string()),
        Just("FALSE".to_string()),
        prop_oneof![
            Just("#DIV/0!"),
            Just("#N/A"),
            Just("#VALUE!"),
            Just("#REF!"),
            Just("#NAME?"),
            Just("#NUM!"),
            Just("#NULL!")
        ]
        .prop_map(str::to_string),
        // Formulas over the grid (relative + absolute refs, functions, text).
        prop_oneof![
            Just("=A1+1"),
            Just("=SUM($A$1:B3)"),
            Just("=IF(A1>0,\"pos\",\"<&>\")"),
            Just("=CONCAT(A1,\"x\")"),
            Just("=IFERROR(1/0,\"div\")"),
            Just("=A2&B2"),
            Just("=ROUND(A1*3.5,2)")
        ]
        .prop_map(str::to_string),
    ]
}

/// Cells over a 6x4 window. Formulas are moved to columns C:D and every
/// generated formula reads only A:B, so the population can never contain a
/// reference cycle: how an engine settles a cycle is a CALC question (and the
/// one it raised is pinned in `tests/defects.rs`), not a round-trip one.
fn grid() -> impl Strategy<Value = Vec<(u32, u32, String)>> {
    prop::collection::vec((0u32..6, 0u32..4, input()), 1..20).prop_map(|cells| {
        cells
            .into_iter()
            .map(|(r, c, v)| {
                let c = if v.starts_with('=') { 2 + c % 2 } else { c };
                (r, c, v)
            })
            .collect()
    })
}

/// Every (input, display) of the 6x4 window, row-major.
fn snapshot(s: &SheetSession) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in 0..6 {
        for c in 0..4 {
            out.push((s.get_cell_input(0, r, c), s.get_cell_display(0, r, c)));
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn xlsx_write_read_is_identity__feat__sheet_xlsx_roundtrip(cells in grid()) {
        let mut s = SheetSession::new();
        for (r, c, v) in &cells {
            // A rejected input (e.g. a control char the parser refuses in a
            // formula) is not a round-trip question; skip it.
            let _ = s.set_cell(0, *r, *c, v);
        }
        let before = snapshot(&s);
        let bytes = s.save_xlsx().expect("save");
        let reloaded = SheetSession::load_xlsx(&bytes).expect("reload");
        let after = snapshot(&reloaded);
        for (i, (b, a)) in before.iter().zip(&after).enumerate() {
            prop_assert_eq!(b, a, "cell {} (row {}, col {}) changed across save/load", i, i / 4, i % 4);
        }
        // And a second generation is stable too.
        let mut reloaded = reloaded;
        let bytes2 = reloaded.save_xlsx().expect("save 2");
        let again = SheetSession::load_xlsx(&bytes2).expect("reload 2");
        prop_assert_eq!(after, snapshot(&again));
    }
}

/// The generator's hard cases, pinned once deterministically so the property
/// is known not to pass vacuously (every input here is ACCEPTED by
/// `set_cell` and must survive).
#[test]
fn xlsx_hard_text_survives_roundtrip__feat__sheet_xlsx_roundtrip() {
    let hard = [
        "a\u{0}b",
        "v\u{b}t",
        "x<&>\"'",
        "  lead and trail  ",
        "tab\there\nnl",
        "astral \u{1f600}",
        "_x0041_ literal escape",
    ];
    let mut s = SheetSession::new();
    for (i, t) in hard.iter().enumerate() {
        s.set_cell(0, i as u32, 0, t)
            .expect("set_cell accepts the text");
    }
    let bytes = s.save_xlsx().expect("save");
    let r = SheetSession::load_xlsx(&bytes).expect("reload");
    for (i, t) in hard.iter().enumerate() {
        assert_eq!(&r.get_cell_input(0, i as u32, 0), t, "row {i}");
    }
}
