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

//! Information-family conformance, M1 additions (spec §7, §11 T1): `N`, `TYPE`,
//! `ERROR.TYPE`, `SHEET`, `SHEETS`, `ISEVEN`, `ISODD`. Self-contained
//! **direct-dispatch** tests: each case resolves a `FuncId` via
//! [`sheet_core::funcs::lookup_func`], builds an `&[Arg]` slice, and calls
//! [`sheet_fn::dispatch`] — exercising the exact path `sheet-calc` will, arity
//! guard and all. Test fns are named with the `sheet_fn_info_<name>` prefix the
//! `registry/functions/info2.yaml` rows point at, so the §12.2 coverage gate
//! finds them.
//!
//! The M1 additions split from the classic `IS*` predicates (`tests/fn_info.rs`)
//! in their error behavior (ECMA-376 §18.17.7):
//! - `N`/`ISEVEN`/`ISODD` are *coercions* — they route through
//!   `coerce::to_number` and therefore PROPAGATE an error argument.
//! - `TYPE`/`ERROR.TYPE` *classify* — they INSPECT the variant (an error
//!   argument is the subject, never returned).
//! - `SHEET`/`SHEETS` are reference functions resolved from the argument's
//!   range origin or the current cell; their T1 limitations are documented in
//!   the kernel and the registry rows.
//! - `ISFORMULA` is now an EVALUATOR SPECIAL FORM (M2 Phase A,
//!   `special_form: true`): it reads `cell.formula.is_some()` from the model in
//!   `sheet-calc/eval.rs`, so its real behavior is tested through the evaluator
//!   in `tests/special_forms.rs`, not as a pure kernel here.

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::{dispatch, Arg, EvalCtx, RangeView};

// ---- helpers ----

fn cr_at(sheet: u16, row: u32, col: u32) -> CellRef {
    CellRef {
        sheet,
        row,
        col,
        row_abs: false,
        col_abs: false,
    }
}

/// A deterministic context anchored at sheet 0, cell A1.
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cr_at(0, 0, 0), 45000.5, 42)
}

/// Dispatch a function by registry NAME through the frozen table at the default
/// context.
fn call(name: &str, args: &[Arg]) -> CellValue {
    call_in(name, args, &ctx())
}

/// Dispatch a function by registry NAME through the frozen table at a given
/// context (so `SHEET()` can be anchored at a non-zero sheet).
fn call_in(name: &str, args: &[Arg], ctx: &EvalCtx) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("lookup_func({name}) returned None — registry row missing"));
    dispatch(id, args, ctx)
}

fn num(n: f64) -> CellValue {
    CellValue::Number(n)
}
fn txt(s: &str) -> CellValue {
    CellValue::from(s)
}
fn b(v: bool) -> CellValue {
    CellValue::Bool(v)
}
fn err(e: CellError) -> CellValue {
    CellValue::Error(e)
}
fn s(v: CellValue) -> Arg<'static> {
    Arg::Scalar(v)
}

// ================= N =================

#[test]
fn sheet_fn_info_n_coercions() {
    // Number is itself; TRUE -> 1, FALSE -> 0; blank -> 0.
    assert_eq!(call("N", &[s(num(7.0))]), num(7.0));
    assert_eq!(call("N", &[s(b(true))]), num(1.0));
    assert_eq!(call("N", &[s(b(false))]), num(0.0));
    assert_eq!(call("N", &[s(CellValue::Empty)]), num(0.0));
}

#[test]
fn sheet_fn_info_n_text_is_zero() {
    // The defining ruling: N does NOT parse numeric text — any text is 0.
    assert_eq!(call("N", &[s(txt("7"))]), num(0.0));
    assert_eq!(call("N", &[s(txt("hello"))]), num(0.0));
}

#[test]
fn sheet_fn_info_n_propagates_error() {
    // N is a coercion → an error argument propagates.
    assert_eq!(call("N", &[s(err(CellError::Div0))]), err(CellError::Div0));
}

#[test]
fn sheet_fn_info_n_arity_violation() {
    // Zero args (< min 1) and two args (> max 1) → #VALUE!.
    assert_eq!(call("N", &[]), err(CellError::Value));
    assert_eq!(
        call("N", &[s(num(1.0)), s(num(2.0))]),
        err(CellError::Value)
    );
}

// ================= TYPE =================

#[test]
fn sheet_fn_info_type_codes() {
    assert_eq!(call("TYPE", &[s(num(1.0))]), num(1.0));
    assert_eq!(call("TYPE", &[s(txt("x"))]), num(2.0));
    assert_eq!(call("TYPE", &[s(b(true))]), num(4.0));
    // A blank cell classifies as a number.
    assert_eq!(call("TYPE", &[s(CellValue::Empty)]), num(1.0));
}

#[test]
fn sheet_fn_info_type_inspects_error_as_16() {
    // TYPE inspects — an error subject is 16, never propagated.
    assert_eq!(call("TYPE", &[s(err(CellError::Na))]), num(16.0));
    assert_eq!(call("TYPE", &[s(err(CellError::Div0))]), num(16.0));
}

#[test]
fn sheet_fn_info_type_range_top_left() {
    // range_aware:false — a range arg collapses to its top-left cell.
    let cells = [txt("a"), num(2.0)];
    let rv = RangeView::from_slice(cr_at(0, 0, 0), 1, 2, &cells);
    assert_eq!(call("TYPE", &[Arg::Range(rv)]), num(2.0));
}

#[test]
fn sheet_fn_info_type_arity_violation() {
    assert_eq!(call("TYPE", &[]), err(CellError::Value));
}

// ================= ERROR.TYPE =================

#[test]
fn sheet_fn_info_error_type_indices() {
    assert_eq!(call("ERROR.TYPE", &[s(err(CellError::Null))]), num(1.0));
    assert_eq!(call("ERROR.TYPE", &[s(err(CellError::Div0))]), num(2.0));
    assert_eq!(call("ERROR.TYPE", &[s(err(CellError::Value))]), num(3.0));
    assert_eq!(call("ERROR.TYPE", &[s(err(CellError::Ref))]), num(4.0));
    assert_eq!(call("ERROR.TYPE", &[s(err(CellError::Name))]), num(5.0));
    assert_eq!(call("ERROR.TYPE", &[s(err(CellError::Num))]), num(6.0));
    assert_eq!(call("ERROR.TYPE", &[s(err(CellError::Na))]), num(7.0));
}

#[test]
fn sheet_fn_info_error_type_non_error_is_na() {
    // A non-error subject has no error code → #N/A.
    assert_eq!(call("ERROR.TYPE", &[s(num(5.0))]), err(CellError::Na));
    assert_eq!(call("ERROR.TYPE", &[s(txt("x"))]), err(CellError::Na));
    // The ruled #SPILL! → #N/A (no classic 1..7 index).
    assert_eq!(
        call("ERROR.TYPE", &[s(err(CellError::Spill))]),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_info_error_type_arity_violation() {
    assert_eq!(call("ERROR.TYPE", &[]), err(CellError::Value));
}

// ================= SHEET =================

#[test]
fn sheet_fn_info_sheet_no_arg_is_current_plus_one() {
    // current.sheet == 2 → SHEET() == 3 (1-based).
    let c = EvalCtx::new(DateSystem::Date1900, cr_at(2, 0, 0), 0.0, 1);
    assert_eq!(call_in("SHEET", &[], &c), num(3.0));
}

#[test]
fn sheet_fn_info_sheet_range_arg_uses_origin_sheet() {
    // Origin on sheet id 4 → SHEET(ref) == 5.
    let cells = [num(1.0)];
    let rv = RangeView::from_slice(cr_at(4, 0, 0), 1, 1, &cells);
    assert_eq!(call("SHEET", &[Arg::Range(rv)]), num(5.0));
}

#[test]
fn sheet_fn_info_sheet_scalar_arg_falls_back_to_current() {
    // T1: a scalar arg has no recoverable reference → current cell's sheet.
    let c = EvalCtx::new(DateSystem::Date1900, cr_at(1, 0, 0), 0.0, 1);
    assert_eq!(call_in("SHEET", &[s(num(99.0))], &c), num(2.0));
    // A sheet-NAME text arg likewise falls back at T1.
    assert_eq!(call_in("SHEET", &[s(txt("Sheet1"))], &c), num(2.0));
}

#[test]
fn sheet_fn_info_sheet_arity_violation() {
    // Two args (> max 1) → #VALUE!.
    assert_eq!(
        call("SHEET", &[s(num(1.0)), s(num(2.0))]),
        err(CellError::Value)
    );
}

// ================= SHEETS =================

#[test]
fn sheet_fn_info_sheets_is_one_t1() {
    // T1 limitation: always 1 (no 3-D range Arg shape, no workbook sheet count).
    assert_eq!(call("SHEETS", &[]), num(1.0));
    let cells = [num(1.0), num(2.0)];
    let rv = RangeView::from_slice(cr_at(0, 0, 0), 1, 2, &cells);
    assert_eq!(call("SHEETS", &[Arg::Range(rv)]), num(1.0));
    // Even a range whose origin is on a non-zero sheet spans one sheet at T1.
    let cells2 = [num(1.0)];
    let rv2 = RangeView::from_slice(cr_at(3, 0, 0), 1, 1, &cells2);
    assert_eq!(call("SHEETS", &[Arg::Range(rv2)]), num(1.0));
}

#[test]
fn sheet_fn_info_sheets_arity_violation() {
    // Two args (> max 1) → #VALUE!.
    assert_eq!(
        call("SHEETS", &[s(num(1.0)), s(num(2.0))]),
        err(CellError::Value)
    );
}

// ================= ISEVEN / ISODD =================

#[test]
fn sheet_fn_info_iseven_basic_and_truncation() {
    assert_eq!(call("ISEVEN", &[s(num(4.0))]), b(true));
    assert_eq!(call("ISEVEN", &[s(num(3.0))]), b(false));
    // Zero is even.
    assert_eq!(call("ISEVEN", &[s(num(0.0))]), b(true));
    // Truncation toward zero: -2.5 → -2 (even).
    assert_eq!(call("ISEVEN", &[s(num(-2.5))]), b(true));
}

#[test]
fn sheet_fn_info_iseven_coercion_and_errors() {
    // Numeric text coerces (the general to_number, unlike N).
    assert_eq!(call("ISEVEN", &[s(txt("8"))]), b(true));
    // Non-numeric text → #VALUE!.
    assert_eq!(call("ISEVEN", &[s(txt("x"))]), err(CellError::Value));
    // An error argument propagates.
    assert_eq!(
        call("ISEVEN", &[s(err(CellError::Ref))]),
        err(CellError::Ref)
    );
}

#[test]
fn sheet_fn_info_iseven_arity_violation() {
    assert_eq!(call("ISEVEN", &[]), err(CellError::Value));
}

#[test]
fn sheet_fn_info_isodd_basic_and_truncation() {
    assert_eq!(call("ISODD", &[s(num(3.0))]), b(true));
    assert_eq!(call("ISODD", &[s(num(4.0))]), b(false));
    // Zero is not odd.
    assert_eq!(call("ISODD", &[s(num(0.0))]), b(false));
    // Truncation toward zero: 3.9 → 3 (odd).
    assert_eq!(call("ISODD", &[s(num(3.9))]), b(true));
}

#[test]
fn sheet_fn_info_isodd_coercion_and_errors() {
    // TRUE → 1 → odd.
    assert_eq!(call("ISODD", &[s(b(true))]), b(true));
    // Non-numeric text → #VALUE!.
    assert_eq!(call("ISODD", &[s(txt("x"))]), err(CellError::Value));
    // An error argument propagates.
    assert_eq!(
        call("ISODD", &[s(err(CellError::Num))]),
        err(CellError::Num)
    );
}

#[test]
fn sheet_fn_info_isodd_arity_violation() {
    assert_eq!(call("ISODD", &[]), err(CellError::Value));
}

// ================= formula-level golden corpus replay =================
//
// The `corpus/fn-corpus/info2/*.golden.tsv` goldens are FORMULA-level fixtures
// that the Phase-2 end-to-end runner will replay through `sheet-calc`. To keep
// them honest against THIS implementation now, these tests drive each fixture
// through the FROZEN `sheet_fn::dispatch` with a tiny self-contained decoder for
// the simple `=FUNC(arg;arg;…)` shape the goldens use. General-formatted output
// reuses `coerce::to_text`. The decoder mirrors `tests/fn_logical.rs::corpus`.
// SHEET/SHEETS goldens are anchored at the decoder's sheet-0 context (current
// cell and every range origin on sheet 0), so they resolve to 1 — the honest T1
// projection.

mod corpus {
    use super::*;
    use sheet_core::funcs::lookup_func;
    use sheet_fn::coerce;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    struct Case {
        id: String,
        formula: String,
        setup: String,
        expected: String,
    }

    fn corpus_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("corpus/fn-corpus/info2")
            .join(name)
    }

    fn load(name: &str) -> Vec<Case> {
        let path = corpus_path(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read corpus {}: {e}", path.display()));
        let mut out = Vec::new();
        for raw in text.lines() {
            let line = raw.trim_end_matches(['\r', '\n']);
            if line.trim_start().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(
                cols.len(),
                4,
                "{}: row has {} cols, want 4 (id<TAB>formula<TAB>setup<TAB>expected): {line:?}",
                path.display(),
                cols.len()
            );
            out.push(Case {
                id: cols[0].to_string(),
                formula: cols[1].to_string(),
                setup: cols[2].to_string(),
                expected: cols[3].to_string(),
            });
        }
        assert!(!out.is_empty(), "corpus {name} has no cases");
        out
    }

    fn classify(tok: &str) -> CellValue {
        let t = tok.trim();
        if t.is_empty() {
            return CellValue::Empty;
        }
        if let Some(e) = CellError::parse(t) {
            return CellValue::Error(e);
        }
        if t.eq_ignore_ascii_case("TRUE") {
            return CellValue::Bool(true);
        }
        if t.eq_ignore_ascii_case("FALSE") {
            return CellValue::Bool(false);
        }
        if let Ok(n) = t.parse::<f64>() {
            if n.is_finite() {
                return CellValue::Number(n);
            }
        }
        CellValue::from(t)
    }

    fn parse_setup(setup: &str) -> BTreeMap<String, CellValue> {
        let mut map = BTreeMap::new();
        for pair in setup.split(';') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (addr, val) = pair
                .split_once('=')
                .unwrap_or_else(|| panic!("bad setup pair {pair:?} (want ADDR=VALUE)"));
            map.insert(addr.trim().to_ascii_uppercase(), classify(val));
        }
        map
    }

    fn parse_addr(addr: &str) -> (u32, u32) {
        let split = addr
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or_else(|| panic!("bad A1 address {addr:?}"));
        let (col_s, row_s) = addr.split_at(split);
        let col = sheet_core::a1_to_col(col_s).unwrap_or_else(|| panic!("bad column in {addr:?}"));
        let row: u32 = row_s
            .parse::<u32>()
            .unwrap_or_else(|_| panic!("bad row in {addr:?}"));
        (col, row - 1)
    }

    enum Owned {
        Scalar(CellValue),
        Range {
            origin: CellRef,
            rows: u32,
            cols: u32,
            cells: Vec<CellValue>,
        },
    }

    fn decode_arg(tok: &str, cells: &BTreeMap<String, CellValue>) -> Owned {
        let tok = tok.trim();
        if let Some(inner) = tok.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            return Owned::Scalar(CellValue::from(inner));
        }
        if let Some((a, b)) = tok.split_once(':') {
            let (c0, r0) = parse_addr(&a.to_ascii_uppercase());
            let (c1, r1) = parse_addr(&b.to_ascii_uppercase());
            let (rmin, rmax) = (r0.min(r1), r0.max(r1));
            let (cmin, cmax) = (c0.min(c1), c0.max(c1));
            let rows = rmax - rmin + 1;
            let cols = cmax - cmin + 1;
            let mut buf = Vec::with_capacity((rows * cols) as usize);
            for r in rmin..=rmax {
                for c in cmin..=cmax {
                    let label = format!("{}{}", sheet_core::col_to_a1(c), r + 1);
                    buf.push(cells.get(&label).cloned().unwrap_or(CellValue::Empty));
                }
            }
            return Owned::Range {
                origin: CellRef {
                    sheet: 0,
                    row: rmin,
                    col: cmin,
                    row_abs: false,
                    col_abs: false,
                },
                rows,
                cols,
                cells: buf,
            };
        }
        let upper = tok.to_ascii_uppercase();
        let looks_like_ref = !tok.is_empty()
            && tok.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && tok.chars().any(|c| c.is_ascii_digit())
            && tok.chars().all(|c| c.is_ascii_alphanumeric())
            && CellError::parse(tok).is_none()
            && !tok.eq_ignore_ascii_case("TRUE")
            && !tok.eq_ignore_ascii_case("FALSE");
        if looks_like_ref && cells.contains_key(&upper) {
            return Owned::Scalar(cells[&upper].clone());
        }
        if looks_like_ref {
            let (c, r) = parse_addr(&upper);
            let _ = (c, r);
            return Owned::Scalar(CellValue::Empty);
        }
        Owned::Scalar(classify(tok))
    }

    /// Split a function arg list on top-level `;`. A `;` inside a quoted string
    /// is kept intact.
    fn split_args(inner: &str) -> Vec<String> {
        if inner.trim().is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_quote = false;
        for ch in inner.chars() {
            match ch {
                '"' => {
                    in_quote = !in_quote;
                    cur.push(ch);
                }
                ';' if !in_quote => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(ch),
            }
        }
        out.push(cur.trim().to_string());
        out
    }

    fn eval(formula: &str, setup: &BTreeMap<String, CellValue>) -> String {
        let body = formula
            .strip_prefix('=')
            .unwrap_or_else(|| panic!("formula must start with '=': {formula:?}"));
        let open = body
            .find('(')
            .unwrap_or_else(|| panic!("no '(' in {formula:?}"));
        let close = body
            .rfind(')')
            .unwrap_or_else(|| panic!("no ')' in {formula:?}"));
        let name = body[..open].trim();
        let inner = &body[open + 1..close];

        let owned: Vec<Owned> = split_args(inner)
            .iter()
            .map(|t| decode_arg(t, setup))
            .collect();
        let args: Vec<Arg> = owned
            .iter()
            .map(|o| match o {
                Owned::Scalar(v) => Arg::Scalar(v.clone()),
                Owned::Range {
                    origin,
                    rows,
                    cols,
                    cells,
                } => Arg::Range(RangeView::from_slice(*origin, *rows, *cols, cells)),
            })
            .collect();

        let id = lookup_func(name)
            .unwrap_or_else(|| panic!("lookup_func({name}) None — registry row missing"));
        let result = dispatch(id, &args, &ctx());
        match result {
            CellValue::Error(e) => e.as_str().to_string(),
            other => coerce::to_text(&other).to_string(),
        }
    }

    fn run(name: &str) {
        for case in load(name) {
            let setup = parse_setup(&case.setup);
            let got = eval(&case.formula, &setup);
            assert_eq!(
                got, case.expected,
                "[{}] {} (setup {:?}) -> got {:?}, want {:?}",
                case.id, case.formula, case.setup, got, case.expected
            );
        }
    }

    #[test]
    fn sheet_fn_info_n_corpus() {
        run("n.golden.tsv");
    }
    #[test]
    fn sheet_fn_info_type_corpus() {
        run("type.golden.tsv");
    }
    #[test]
    fn sheet_fn_info_error_type_corpus() {
        run("error_type.golden.tsv");
    }
    #[test]
    fn sheet_fn_info_sheet_corpus() {
        run("sheet.golden.tsv");
    }
    #[test]
    fn sheet_fn_info_sheets_corpus() {
        run("sheets.golden.tsv");
    }
    #[test]
    fn sheet_fn_info_iseven_corpus() {
        run("iseven.golden.tsv");
    }
    #[test]
    fn sheet_fn_info_isodd_corpus() {
        run("isodd.golden.tsv");
    }
}
