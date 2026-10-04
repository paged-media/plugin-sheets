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

//! Number-format PROPERTIES (spec §9) — what must hold for EVERY value, where
//! the golden corpora (and the Excel oracle over them) pin chosen points.
//!
//! - totality: any code built from the format grammar's tokens either fails
//!   to compile with a typed error or renders any value without panicking;
//! - `0`, `0.00`, `#,##0`, `0%`, `0.00E+00` agree with an independent
//!   arithmetic rendering (Excel rounds half away from zero);
//! - section selection: a three-section code routes positive / negative /
//!   zero to the right section, and the negative section never shows a
//!   second minus;
//! - grouping only inserts separators: stripping them gives the `0` result.

use proptest::prelude::*;
use sheet_core::{CellValue, DateSystem, Locale};
use sheet_format::{compile, format_value, FormatCtx};

fn ctx() -> FormatCtx {
    FormatCtx::new(DateSystem::Date1900, Locale::EnUs)
}

fn render(code: &str, v: f64) -> String {
    let f = compile(code).unwrap_or_else(|e| panic!("compile {code:?}: {e}"));
    format_value(&CellValue::Number(v), &f, &ctx())
}

/// Round half away from zero to `d` decimals, as Excel's display does: the
/// value is first taken at Excel's 15 significant digits (the registry's
/// 15-digit ruling — 663825561.895 is stored as ...894999 but DISPLAYS
/// .90), then that decimal text is rounded, so binary noise cannot flip a tie.
fn excel_round(v: f64, d: usize) -> String {
    let s = format!("{:.*e}", 14, v.abs());
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    // value = 0.digits * 10^(exp+1)
    let point = exp + 1; // digits before the decimal point
    let mut all: Vec<u8> = digits.bytes().map(|b| b - b'0').collect();
    // Pad so we have `point + d` digits available.
    let keep = point + d as i32;
    if keep < 0 {
        return format_fixed(0, &[], d, false);
    }
    let keep = keep as usize;
    while all.len() < keep + 1 {
        all.push(0);
    }
    let round_up = all[keep] >= 5;
    let mut kept: Vec<u8> = all[..keep].to_vec();
    if round_up {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, 1);
                break;
            }
            i -= 1;
            if kept[i] == 9 {
                kept[i] = 0;
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let int_len = kept.len() as i32 - d as i32;
    let neg = v < 0.0 && kept.iter().any(|&x| x != 0);
    format_fixed(int_len.max(0) as usize, &kept, d, neg)
}

fn format_fixed(int_len: usize, kept: &[u8], d: usize, neg: bool) -> String {
    let mut digits: Vec<u8> = kept.to_vec();
    while digits.len() < int_len + d {
        digits.insert(0, 0);
    }
    let (i, f) = digits.split_at(digits.len() - d);
    let mut int: String = i.iter().map(|x| (b'0' + x) as char).collect();
    let int_trim = int.trim_start_matches('0');
    int = if int_trim.is_empty() {
        "0".into()
    } else {
        int_trim.into()
    };
    let frac: String = f.iter().map(|x| (b'0' + x) as char).collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(&int);
    if d > 0 {
        out.push('.');
        out.push_str(&frac);
    }
    out
}

/// Values in the range a published table shows (Excel displays at most 15
/// significant digits, so stay inside ±1e12 with ≤ 2 decimals of interest).
fn value() -> impl Strategy<Value = f64> {
    prop_oneof![
        (-1_000_000_000_000i64..1_000_000_000_000).prop_map(|n| n as f64 / 1000.0),
        (-100_000i64..100_000).prop_map(|n| n as f64 / 8.0), // exact binary ties
        -1e6f64..1e6,
    ]
}

/// Codes from the grammar's token alphabet — valid or not.
fn any_code() -> impl Strategy<Value = String> {
    let tok = prop_oneof![
        Just("0"),
        Just("#"),
        Just("?"),
        Just("."),
        Just(","),
        Just("%"),
        Just("E+"),
        Just("E-"),
        Just(";"),
        Just("\"x\""),
        Just("@"),
        Just("_)"),
        Just("*-"),
        Just("[Red]"),
        Just("[>=100]"),
        Just("[$€-407]"),
        Just("yyyy"),
        Just("mm"),
        Just("dd"),
        Just("hh"),
        Just("ss"),
        Just("AM/PM"),
        Just("[h]"),
        Just("/"),
        Just("General"),
        Just("\\"),
        Just(" "),
        Just("00"),
        Just(".0"),
    ];
    prop::collection::vec(tok, 0..10).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn format_is_total_over_the_token_grammar__feat__sheet_format_engine(
        code in any_code(),
        v in prop_oneof![any::<f64>(), value()],
    ) {
        if let Ok(f) = compile(&code) {
            let _ = format_value(&CellValue::Number(v), &f, &ctx());
            let _ = format_value(&CellValue::Text("t".into()), &f, &ctx());
            let _ = format_value(&CellValue::Bool(true), &f, &ctx());
        }
    }

    // Over values whose binary form IS their decimal form (k/8) — so the
    // property is independent of the 15-digit pre-rounding Excel applies and
    // the engine does not yet (`tests/defects.rs::defect_fixed_decimals_round_the_binary_value`).
    #[test]
    fn fixed_decimal_codes_round_half_away_from_zero__feat__sheet_format_engine(
        v in (-8_000_000_000i64..8_000_000_000).prop_map(|n| n as f64 / 8.0)
    ) {
        prop_assert_eq!(render("0", v), excel_round(v, 0));
        prop_assert_eq!(render("0.00", v), excel_round(v, 2));
    }

    #[test]
    fn grouping_only_inserts_separators__feat__sheet_format_engine(v in value()) {
        let grouped = render("#,##0", v);
        prop_assert_eq!(grouped.replace(',', ""), render("0", v));
        // Separators sit every three digits from the right.
        let digits = grouped.trim_start_matches('-');
        for (i, part) in digits.split(',').enumerate() {
            if i > 0 {
                prop_assert_eq!(part.len(), 3, "group {:?} in {:?}", part, grouped);
            } else {
                prop_assert!((1..=3).contains(&part.len()), "lead group in {:?}", grouped);
            }
        }
    }

    #[test]
    fn percent_scales_by_one_hundred__feat__sheet_format_engine(n in -100_000i64..100_000) {
        let v = n as f64 / 10_000.0; // exact to 4 decimals -> percent integral to 2
        prop_assert_eq!(render("0.00%", v), format!("{}%", excel_round(v * 100.0, 2)));
    }

    #[test]
    fn three_sections_route_by_sign__feat__sheet_format_engine(v in value()) {
        let out = render("0.0;(0.0);\"zero\"", v);
        if v > 0.0 {
            prop_assert!(!out.starts_with('(') && out != "zero", "{} -> {:?}", v, out);
        } else if v < 0.0 {
            prop_assert!(out.starts_with('(') && out.ends_with(')'), "{} -> {:?}", v, out);
            prop_assert!(!out.contains('-'), "negative section shows its own minus: {:?}", out);
        } else {
            prop_assert_eq!(out, "zero");
        }
    }

    #[test]
    fn scientific_mantissa_is_normalised__feat__sheet_format_engine(v in value()) {
        prop_assume!(v != 0.0);
        let out = render("0.00E+00", v);
        let (mant, exp) = out.split_once('E').expect("an E");
        let m: f64 = mant.parse().expect("mantissa");
        let e: i32 = exp.parse().expect("exponent");
        prop_assert!((1.0..10.0).contains(&m.abs()) || m.abs() == 10.0, "{} -> {:?}", v, out);
        prop_assert!(exp.starts_with('+') || exp.starts_with('-'), "{:?}", out);
        let back = m * 10f64.powi(e);
        prop_assert!((back - v).abs() <= v.abs() * 0.006, "{} -> {:?}", v, out);
    }
}
