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

//! Wave-7 function conformance (spec §7): AVERAGEIF, LOOKUP, the legacy and
//! exclusive percentile/quartile pairs, MODE.SNGL, FORECAST(.LINEAR), TREND,
//! FREQUENCY, the normal distribution, the array-shaping family (VSTACK …
//! MMULT) and WORKDAY.INTL / NETWORKDAYS.INTL. Every case runs end to end
//! through the FROZEN [`sheet_calc::Engine`] (`enter` → parse → recalc) — the
//! path `sheet-js` drives — so the nested-array argument door (an array
//! literal or a dynamic-array result handed to another function) is exercised
//! too. Expected values are the worked examples of the public Microsoft
//! documentation for each function (or derived from its definition); the same
//! cases live in the fn-corpus TSVs for the Excel-recorded oracle lane.
//! Test names use the `sheet_fn_<family>_<name>` prefixes the registry rows
//! point at (the coverage gate looks for them).

use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellError, CellValue, SheetModel};

fn engine(setup: &[(&str, &str)]) -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    let mut e = Engine::new(m, EngineConfig::default());
    for (addr, raw) in setup {
        let (r, c) = addr_of(addr);
        e.enter(0, r, c, raw).unwrap();
    }
    e
}

fn addr_of(a: &str) -> (u32, u32) {
    let split = a.find(|ch: char| ch.is_ascii_digit()).unwrap();
    let col = sheet_core::a1_to_col(&a[..split]).unwrap();
    let row: u32 = a[split..].parse().unwrap();
    (row - 1, col)
}

fn val(e: &Engine, a: &str) -> CellValue {
    let (r, c) = addr_of(a);
    e.model()
        .sheet(0)
        .and_then(|ws| ws.cell(r, c))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

/// Enter `formula` at Z1 over `setup` and return Z1's value.
fn calc(setup: &[(&str, &str)], formula: &str) -> CellValue {
    let mut e = engine(setup);
    e.enter(0, 0, 25, formula).unwrap();
    val(&e, "Z1")
}

/// Enter `formula` at J1 and read the `rows × cols` block it spills.
fn spill(setup: &[(&str, &str)], formula: &str, rows: u32, cols: u32) -> Vec<Vec<CellValue>> {
    let mut e = engine(setup);
    e.enter(0, 0, 9, formula).unwrap();
    (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| {
                    e.model()
                        .sheet(0)
                        .and_then(|ws| ws.cell(r, 9 + c))
                        .map(|c| c.value.clone())
                        .unwrap_or(CellValue::Empty)
                })
                .collect()
        })
        .collect()
}

fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}
fn t(s: &str) -> CellValue {
    CellValue::from(s)
}
fn er(e: CellError) -> CellValue {
    CellValue::Error(e)
}
fn rows(g: &[&[f64]]) -> Vec<Vec<CellValue>> {
    g.iter()
        .map(|r| r.iter().map(|x| n(*x)).collect())
        .collect()
}
fn close(v: CellValue, want: f64, tol: f64) {
    match v {
        CellValue::Number(x) => assert!((x - want).abs() <= tol, "got {x}, want {want}"),
        other => panic!("expected a number near {want}, got {other:?}"),
    }
}

const PROPS: &[(&str, &str)] = &[
    ("A1", "100000"),
    ("A2", "200000"),
    ("A3", "300000"),
    ("A4", "400000"),
    ("B1", "7000"),
    ("B2", "14000"),
    ("B3", "21000"),
    ("B4", "28000"),
];

// ---- AVERAGEIF ---------------------------------------------------------------

#[test]
fn sheet_fn_agg_averageif_documented_examples() {
    assert_eq!(calc(PROPS, "=AVERAGEIF(B1:B4,\"<23000\")"), n(14000.0));
    assert_eq!(calc(PROPS, "=AVERAGEIF(A1:A4,\"<250000\")"), n(150000.0));
    assert_eq!(
        calc(PROPS, "=AVERAGEIF(A1:A4,\">250000\",B1:B4)"),
        n(24500.0)
    );
    assert_eq!(
        calc(PROPS, "=AVERAGEIF(A1:A4,\"<95000\")"),
        er(CellError::Div0)
    );
}

#[test]
fn sheet_fn_agg_averageif_ignores_text_targets_and_propagates_errors() {
    let s = &[
        ("A1", "x"),
        ("A2", "x"),
        ("A3", "y"),
        ("B1", "10"),
        ("B2", "abc"),
        ("B3", "#N/A"),
    ];
    // B2 is text: ignored, not averaged as 0.
    assert_eq!(calc(s, "=AVERAGEIF(A1:A3,\"x\",B1:B3)"), n(10.0));
    assert_eq!(calc(s, "=AVERAGEIF(A1:A3,\"y\",B1:B3)"), er(CellError::Na));
}

// ---- LOOKUP ------------------------------------------------------------------

const COLOURS: &[(&str, &str)] = &[
    ("A1", "4.14"),
    ("A2", "4.19"),
    ("A3", "5.17"),
    ("A4", "5.77"),
    ("A5", "6.39"),
    ("B1", "red"),
    ("B2", "orange"),
    ("B3", "yellow"),
    ("B4", "green"),
    ("B5", "blue"),
];

#[test]
fn sheet_fn_lookup_lookup_vector_form() {
    assert_eq!(calc(COLOURS, "=LOOKUP(4.19,A1:A5,B1:B5)"), t("orange"));
    assert_eq!(calc(COLOURS, "=LOOKUP(5.75,A1:A5,B1:B5)"), t("yellow"));
    assert_eq!(calc(COLOURS, "=LOOKUP(7.66,A1:A5,B1:B5)"), t("blue"));
    assert_eq!(calc(COLOURS, "=LOOKUP(0,A1:A5,B1:B5)"), er(CellError::Na));
}

#[test]
fn sheet_fn_lookup_lookup_array_form() {
    assert_eq!(
        calc(&[], "=LOOKUP(\"C\",{\"a\",\"b\",\"c\",\"d\";1,2,3,4})"),
        n(3.0)
    );
    assert_eq!(
        calc(&[], "=LOOKUP(\"bump\",{\"a\",1;\"b\",2;\"c\",3})"),
        n(2.0)
    );
    // A number key never floors onto text.
    assert_eq!(calc(&[], "=LOOKUP(5,{\"a\",\"b\"})"), er(CellError::Na));
}

// ---- percentiles / quartiles / mode ------------------------------------------

#[test]
fn sheet_fn_stat_percentile_legacy_alias() {
    assert_eq!(calc(&[], "=PERCENTILE({1,3,2,4},0.3)"), n(1.9));
    assert_eq!(calc(&[], "=PERCENTILE({1,3,2,4},1.5)"), er(CellError::Num));
}

#[test]
fn sheet_fn_stat_quartile_legacy_alias() {
    assert_eq!(calc(&[], "=QUARTILE({1,2,4,7,8,9,10,12},1)"), n(3.5));
    assert_eq!(
        calc(&[], "=QUARTILE({1,2,4,7,8,9,10,12},5)"),
        er(CellError::Num)
    );
}

#[test]
fn sheet_fn_stat_percentile_exc_documented() {
    assert_eq!(
        calc(&[], "=PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},0.25)"),
        n(2.5)
    );
    assert_eq!(
        calc(&[], "=PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},0.01)"),
        er(CellError::Num)
    );
    assert_eq!(
        calc(&[], "=PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},2)"),
        er(CellError::Num)
    );
}

#[test]
fn sheet_fn_stat_quartile_exc_documented() {
    let arr = "{6,7,15,36,39,40,41,42,43,47,49}";
    assert_eq!(calc(&[], &format!("=QUARTILE.EXC({arr},1)")), n(15.0));
    assert_eq!(calc(&[], &format!("=QUARTILE.EXC({arr},3)")), n(43.0));
    assert_eq!(
        calc(&[], &format!("=QUARTILE.EXC({arr},4)")),
        er(CellError::Num)
    );
}

#[test]
fn sheet_fn_stat_mode_sngl() {
    assert_eq!(calc(&[], "=MODE.SNGL({5.6,4,4,3,2,4})"), n(4.0));
    assert_eq!(calc(&[], "=MODE.SNGL({1,2,3})"), er(CellError::Na));
}

// ---- FORECAST / TREND / FREQUENCY ---------------------------------------------

#[test]
fn sheet_fn_stat_forecast_documented() {
    close(
        calc(&[], "=FORECAST(30,{6,7,9,15,21},{20,28,31,38,40})"),
        10.607253,
        5e-7,
    );
    assert_eq!(calc(&[], "=FORECAST(1,{1,2},{3,3})"), er(CellError::Div0));
    assert_eq!(calc(&[], "=FORECAST(1,{1,2,3},{1,2})"), er(CellError::Na));
}

#[test]
fn sheet_fn_stat_forecast_linear_matches_forecast() {
    close(
        calc(&[], "=FORECAST.LINEAR(30,{6,7,9,15,21},{20,28,31,38,40})"),
        10.607253,
        5e-7,
    );
}

#[test]
fn sheet_fn_stat_trend_single_regressor() {
    let s = &[
        ("A1", "1"),
        ("A2", "3"),
        ("A3", "5"),
        ("A4", "7"),
        ("B1", "1"),
        ("B2", "2"),
        ("B3", "3"),
        ("B4", "4"),
        ("C1", "5"),
        ("C2", "6"),
    ];
    assert_eq!(
        spill(s, "=TREND(A1:A4,B1:B4,C1:C2)", 2, 1),
        rows(&[&[9.0], &[11.0]])
    );
    // Default known_xs = {1..n}, default new_xs = known_xs.
    assert_eq!(
        spill(&[], "=TREND({2;4;6})", 3, 1),
        rows(&[&[2.0], &[4.0], &[6.0]])
    );
    // const = FALSE forces the line through the origin.
    assert_eq!(calc(&[], "=TREND({2;4},{1;2},3,FALSE)"), n(6.0));
    // known_xs of another count → #REF!.
    assert_eq!(calc(&[], "=TREND({1;2;3},{1;2})"), er(CellError::Ref));
}

#[test]
fn sheet_fn_stat_frequency_documented() {
    let s = &[
        ("A1", "79"),
        ("A2", "85"),
        ("A3", "78"),
        ("A4", "85"),
        ("A5", "50"),
        ("A6", "81"),
        ("A7", "95"),
        ("A8", "88"),
        ("A9", "97"),
        ("B1", "70"),
        ("B2", "79"),
        ("B3", "89"),
    ];
    assert_eq!(
        spill(s, "=FREQUENCY(A1:A9,B1:B3)", 4, 1),
        rows(&[&[1.0], &[2.0], &[4.0], &[2.0]])
    );
}

// ---- the normal distribution ----------------------------------------------------

#[test]
fn sheet_fn_stat_norm_dist_documented() {
    close(calc(&[], "=NORM.DIST(42,40,1.5,TRUE)"), 0.9087888, 5e-8);
    close(calc(&[], "=NORM.DIST(42,40,1.5,FALSE)"), 0.10934005, 5e-9);
    assert_eq!(calc(&[], "=NORM.DIST(42,40,0,TRUE)"), er(CellError::Num));
}

#[test]
fn sheet_fn_stat_norm_inv_documented() {
    close(calc(&[], "=NORM.INV(0.908789,40,1.5)"), 42.000002, 5e-7);
    assert_eq!(calc(&[], "=NORM.INV(0,40,1.5)"), er(CellError::Num));
    assert_eq!(calc(&[], "=NORM.INV(0.5,40,-1)"), er(CellError::Num));
}

#[test]
fn sheet_fn_stat_norm_s_dist_documented() {
    close(calc(&[], "=NORM.S.DIST(1.333333,TRUE)"), 0.908788726, 5e-10);
    close(
        calc(&[], "=NORM.S.DIST(1.333333,FALSE)"),
        0.164010148,
        5e-10,
    );
}

#[test]
fn sheet_fn_stat_norm_s_inv_documented() {
    close(calc(&[], "=NORM.S.INV(0.908789)"), 1.3333347, 5e-8);
    assert_eq!(calc(&[], "=NORM.S.INV(1)"), er(CellError::Num));
}

// ---- the array-shaping family ---------------------------------------------------

#[test]
fn sheet_fn_array_vstack_pads_with_na() {
    assert_eq!(
        spill(&[], "=VSTACK({1,2;3,4},{5,6})", 3, 2),
        rows(&[&[1.0, 2.0], &[3.0, 4.0], &[5.0, 6.0]])
    );
    let g = spill(&[], "=VSTACK({1,2},{3})", 2, 2);
    assert_eq!(g[1], vec![n(3.0), er(CellError::Na)]);
    // A nested array as another function's argument.
    assert_eq!(calc(&[], "=INDEX(VSTACK({1,2;3,4},{5,6}),3,2)"), n(6.0));
}

#[test]
fn sheet_fn_array_hstack() {
    assert_eq!(
        spill(&[], "=HSTACK({1;2},{3;4})", 2, 2),
        rows(&[&[1.0, 3.0], &[2.0, 4.0]])
    );
    assert_eq!(calc(&[], "=SUM(HSTACK({1;2},{3;4}))"), n(10.0));
}

const NINE: &str = "{1,2,3;4,5,6;7,8,9}";

#[test]
fn sheet_fn_array_take() {
    assert_eq!(
        spill(&[], &format!("=TAKE({NINE},2)"), 2, 3),
        rows(&[&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]])
    );
    assert_eq!(
        spill(&[], &format!("=TAKE({NINE},-1)"), 1, 3),
        rows(&[&[7.0, 8.0, 9.0]])
    );
    assert_eq!(
        spill(&[], &format!("=TAKE({NINE},2,-1)"), 2, 1),
        rows(&[&[3.0], &[6.0]])
    );
    assert_eq!(calc(&[], &format!("=TAKE({NINE},0)")), er(CellError::Value));
}

#[test]
fn sheet_fn_array_drop() {
    assert_eq!(
        spill(&[], &format!("=DROP({NINE},2)"), 1, 3),
        rows(&[&[7.0, 8.0, 9.0]])
    );
    assert_eq!(
        spill(&[], &format!("=DROP({NINE},1,1)"), 2, 2),
        rows(&[&[5.0, 6.0], &[8.0, 9.0]])
    );
    assert_eq!(calc(&[], &format!("=DROP({NINE},3)")), er(CellError::Value));
}

#[test]
fn sheet_fn_array_choosecols() {
    assert_eq!(
        spill(&[], &format!("=CHOOSECOLS({NINE},1,3)"), 3, 2),
        rows(&[&[1.0, 3.0], &[4.0, 6.0], &[7.0, 9.0]])
    );
    assert_eq!(
        spill(&[], &format!("=CHOOSECOLS({NINE},-1)"), 3, 1),
        rows(&[&[3.0], &[6.0], &[9.0]])
    );
    assert_eq!(
        calc(&[], &format!("=CHOOSECOLS({NINE},4)")),
        er(CellError::Value)
    );
}

#[test]
fn sheet_fn_array_chooserows() {
    assert_eq!(
        spill(&[], &format!("=CHOOSEROWS({NINE},3,1)"), 2, 3),
        rows(&[&[7.0, 8.0, 9.0], &[1.0, 2.0, 3.0]])
    );
    assert_eq!(
        calc(&[], &format!("=CHOOSEROWS({NINE},0)")),
        er(CellError::Value)
    );
}

#[test]
fn sheet_fn_array_tocol() {
    assert_eq!(
        spill(&[], "=TOCOL({1,2;3,4})", 4, 1),
        rows(&[&[1.0], &[2.0], &[3.0], &[4.0]])
    );
    assert_eq!(
        spill(&[], "=TOCOL({1,2;3,4},0,TRUE)", 4, 1),
        rows(&[&[1.0], &[3.0], &[2.0], &[4.0]])
    );
    // ignore = 1 skips the blank B1.
    let s = &[("A1", "1"), ("A2", "2"), ("B2", "4")];
    assert_eq!(
        spill(s, "=TOCOL(A1:B2,1)", 3, 1),
        rows(&[&[1.0], &[2.0], &[4.0]])
    );
}

#[test]
fn sheet_fn_array_torow() {
    assert_eq!(
        spill(&[], "=TOROW({1,2;3,4})", 1, 4),
        rows(&[&[1.0, 2.0, 3.0, 4.0]])
    );
    let s = &[("A1", "1"), ("A2", "#N/A"), ("A3", "3")];
    assert_eq!(spill(s, "=TOROW(A1:A3,2)", 1, 2), rows(&[&[1.0, 3.0]]));
}

#[test]
fn sheet_fn_array_mmult() {
    assert_eq!(
        spill(&[], "=MMULT({1,2;3,4},{5;6})", 2, 1),
        rows(&[&[17.0], &[39.0]])
    );
    assert_eq!(
        spill(&[], "=MMULT({1,3;7,2},{2,0;0,2})", 2, 2),
        rows(&[&[2.0, 6.0], &[14.0, 4.0]])
    );
    assert_eq!(calc(&[], "=MMULT({1,2},{1,2})"), er(CellError::Value));
    assert_eq!(calc(&[], "=MMULT({1,\"a\"},{1;2})"), er(CellError::Value));
}

// ---- WORKDAY.INTL / NETWORKDAYS.INTL -----------------------------------------------

#[test]
fn sheet_fn_date_workday_intl_documented() {
    assert_eq!(
        calc(&[], "=WORKDAY.INTL(DATE(2012,1,1),30,0)"),
        er(CellError::Num)
    );
    assert_eq!(calc(&[], "=WORKDAY.INTL(DATE(2012,1,1),90,11)"), n(41013.0));
    assert_eq!(calc(&[], "=WORKDAY.INTL(DATE(2012,1,1),30,17)"), n(40944.0));
    // An all-weekend mask has no working day.
    assert_eq!(
        calc(&[], "=WORKDAY.INTL(DATE(2012,1,1),1,\"1111111\")"),
        er(CellError::Value)
    );
    // A malformed mask string is #VALUE!.
    assert_eq!(
        calc(&[], "=WORKDAY.INTL(DATE(2012,1,1),1,\"0101\")"),
        er(CellError::Value)
    );
}

#[test]
fn sheet_fn_date_networkdays_intl_documented() {
    assert_eq!(
        calc(&[], "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31))"),
        n(22.0)
    );
    assert_eq!(
        calc(&[], "=NETWORKDAYS.INTL(DATE(2006,2,28),DATE(2006,1,31))"),
        n(-21.0)
    );
    let hol = &[("A1", "38719"), ("A2", "38733")];
    assert_eq!(
        calc(
            hol,
            "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),7,A1:A2)"
        ),
        n(22.0)
    );
    assert_eq!(
        calc(
            hol,
            "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),\"0010001\",A1:A2)"
        ),
        n(20.0)
    );
    assert_eq!(
        calc(
            &[],
            "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31),\"1111111\")"
        ),
        n(0.0)
    );
}
