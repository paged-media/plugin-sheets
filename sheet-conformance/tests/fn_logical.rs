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

//! Logical-family conformance (spec §7, §11 T0): `IF`, `AND`, `OR`, `NOT`,
//! `TRUE`, `FALSE`, `IFERROR`. Self-contained **direct-dispatch** tests — each
//! resolves a function id via [`sheet_core::funcs::lookup_func`] and routes
//! through the frozen [`sheet_fn::dispatch`], so they exercise the SAME path a
//! formula evaluation takes (registry arity guard included). Test-fn names use
//! the prefix the registry rows point at (`sheet_fn_logical_<name>`) so the
//! coverage gate (§12.2) finds them.
//!
//! T0 deviation note (mirrors `sheet_fn::families::logical` module docs): these
//! functions are EAGER here — both `IF` branches and every `AND`/`OR`/`IFERROR`
//! argument arrive already evaluated. Short-circuit (error-masking on the
//! discarded branch) is a `sheet-calc` Phase-2 special form, not a kernel
//! concern, so these tests assert value-selection behavior only.

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::{dispatch, Arg, EvalCtx, RangeView};

// ---- helpers ----

fn cr(row: u32, col: u32) -> CellRef {
    CellRef {
        sheet: 0,
        row,
        col,
        row_abs: false,
        col_abs: false,
    }
}

/// A deterministic context (the §7 convention: fixed clock + seed).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cr(0, 0), 45000.5, 42)
}

/// Dispatch a function by registry NAME through the frozen table.
fn call(name: &str, args: &[Arg], ctx: &EvalCtx) -> CellValue {
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

// ================= IF =================

#[test]
fn sheet_fn_logical_if_basic() {
    let c = ctx();
    // TRUE condition -> value_if_true.
    assert_eq!(
        call("IF", &[s(b(true)), s(num(10.0)), s(num(20.0))], &c),
        num(10.0)
    );
    // FALSE condition -> value_if_false.
    assert_eq!(
        call("IF", &[s(b(false)), s(num(10.0)), s(num(20.0))], &c),
        num(20.0)
    );
}

#[test]
fn sheet_fn_logical_if_coercion() {
    let c = ctx();
    // Numeric condition: 0 is FALSE, non-zero is TRUE.
    assert_eq!(
        call("IF", &[s(num(0.0)), s(txt("yes")), s(txt("no"))], &c),
        txt("no")
    );
    assert_eq!(
        call("IF", &[s(num(5.0)), s(txt("yes")), s(txt("no"))], &c),
        txt("yes")
    );
    // Text "TRUE"/"FALSE" literals coerce.
    assert_eq!(
        call("IF", &[s(txt("FALSE")), s(num(1.0)), s(num(2.0))], &c),
        num(2.0)
    );
}

#[test]
fn sheet_fn_logical_if_omitted_false_branch() {
    let c = ctx();
    // Third arg omitted, condition FALSE -> boolean FALSE (Excel default).
    assert_eq!(call("IF", &[s(b(false)), s(num(99.0))], &c), b(false));
    // Omitted, condition TRUE -> the true value.
    assert_eq!(call("IF", &[s(b(true)), s(num(99.0))], &c), num(99.0));
}

#[test]
fn sheet_fn_logical_if_error_condition_propagates() {
    let c = ctx();
    // An error in the condition propagates; branches are NOT consulted.
    assert_eq!(
        call(
            "IF",
            &[s(err(CellError::Div0)), s(num(1.0)), s(num(2.0))],
            &c
        ),
        err(CellError::Div0)
    );
    // Non-boolean text condition -> #VALUE!.
    assert_eq!(
        call("IF", &[s(txt("maybe")), s(num(1.0)), s(num(2.0))], &c),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_logical_if_arity_violation() {
    let c = ctx();
    // Only one arg (< min 2) -> #VALUE! from the dispatch arity guard.
    assert_eq!(call("IF", &[s(b(true))], &c), err(CellError::Value));
    // Four args (> max 3) -> #VALUE!.
    assert_eq!(
        call(
            "IF",
            &[s(b(true)), s(num(1.0)), s(num(2.0)), s(num(3.0))],
            &c
        ),
        err(CellError::Value)
    );
}

// ================= AND =================

#[test]
fn sheet_fn_logical_and_basic() {
    let c = ctx();
    assert_eq!(call("AND", &[s(b(true)), s(b(true))], &c), b(true));
    assert_eq!(call("AND", &[s(b(true)), s(b(false))], &c), b(false));
    assert_eq!(call("AND", &[s(b(true))], &c), b(true));
}

#[test]
fn sheet_fn_logical_and_coercion() {
    let c = ctx();
    // Numbers coerce: non-zero TRUE, zero FALSE.
    assert_eq!(call("AND", &[s(num(1.0)), s(num(3.0))], &c), b(true));
    assert_eq!(call("AND", &[s(num(1.0)), s(num(0.0))], &c), b(false));
    // Non-boolean scalar text is #VALUE!.
    assert_eq!(
        call("AND", &[s(b(true)), s(txt("nope"))], &c),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_logical_and_range() {
    let c = ctx();
    // A range of TRUEs (mixed bool/number) -> TRUE; text is skipped.
    let cells = [b(true), num(1.0), txt("ignored"), b(true)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 4, &cells);
    assert_eq!(call("AND", &[Arg::Range(rv)], &c), b(true));

    // A FALSE cell in the range -> FALSE.
    let cells2 = [b(true), num(0.0), b(true)];
    let rv2 = RangeView::from_slice(cr(0, 0), 1, 3, &cells2);
    assert_eq!(call("AND", &[Arg::Range(rv2)], &c), b(false));
}

#[test]
fn sheet_fn_logical_and_range_all_text_is_value_error() {
    let c = ctx();
    // No logical values anywhere (range is all text/blank) -> #VALUE!.
    let cells = [txt("a"), txt("b"), CellValue::Empty];
    let rv = RangeView::from_slice(cr(0, 0), 1, 3, &cells);
    assert_eq!(call("AND", &[Arg::Range(rv)], &c), err(CellError::Value));
}

#[test]
fn sheet_fn_logical_and_range_error_propagates() {
    let c = ctx();
    // An error cell INSIDE the range propagates (aggregation poison).
    let cells = [b(true), err(CellError::Na), b(true)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 3, &cells);
    assert_eq!(call("AND", &[Arg::Range(rv)], &c), err(CellError::Na));
}

#[test]
fn sheet_fn_logical_and_arity_violation() {
    let c = ctx();
    // Zero args (< min 1) -> #VALUE!.
    assert_eq!(call("AND", &[], &c), err(CellError::Value));
}

// ================= OR =================

#[test]
fn sheet_fn_logical_or_basic() {
    let c = ctx();
    assert_eq!(call("OR", &[s(b(false)), s(b(false))], &c), b(false));
    assert_eq!(call("OR", &[s(b(false)), s(b(true))], &c), b(true));
    assert_eq!(call("OR", &[s(b(false))], &c), b(false));
}

#[test]
fn sheet_fn_logical_or_coercion() {
    let c = ctx();
    assert_eq!(call("OR", &[s(num(0.0)), s(num(0.0))], &c), b(false));
    assert_eq!(call("OR", &[s(num(0.0)), s(num(2.0))], &c), b(true));
    // Non-boolean scalar text -> #VALUE!.
    assert_eq!(
        call("OR", &[s(b(false)), s(txt("nope"))], &c),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_logical_or_range() {
    let c = ctx();
    // Range with one TRUE among text/blank -> TRUE.
    let cells = [b(false), txt("skip"), num(7.0)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 3, &cells);
    assert_eq!(call("OR", &[Arg::Range(rv)], &c), b(true));

    // Range all FALSE -> FALSE.
    let cells2 = [b(false), num(0.0), CellValue::Empty];
    let rv2 = RangeView::from_slice(cr(0, 0), 1, 3, &cells2);
    assert_eq!(call("OR", &[Arg::Range(rv2)], &c), b(false));
}

#[test]
fn sheet_fn_logical_or_range_error_propagates() {
    let c = ctx();
    let cells = [b(false), err(CellError::Div0)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    assert_eq!(call("OR", &[Arg::Range(rv)], &c), err(CellError::Div0));
}

#[test]
fn sheet_fn_logical_or_no_logical_value_error() {
    let c = ctx();
    let cells = [txt("x"), CellValue::Empty];
    let rv = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    assert_eq!(call("OR", &[Arg::Range(rv)], &c), err(CellError::Value));
}

#[test]
fn sheet_fn_logical_or_arity_violation() {
    let c = ctx();
    assert_eq!(call("OR", &[], &c), err(CellError::Value));
}

// ================= NOT =================

#[test]
fn sheet_fn_logical_not_basic() {
    let c = ctx();
    assert_eq!(call("NOT", &[s(b(true))], &c), b(false));
    assert_eq!(call("NOT", &[s(b(false))], &c), b(true));
}

#[test]
fn sheet_fn_logical_not_coercion() {
    let c = ctx();
    // Numbers coerce (0 -> FALSE -> NOT -> TRUE).
    assert_eq!(call("NOT", &[s(num(0.0))], &c), b(true));
    assert_eq!(call("NOT", &[s(num(5.0))], &c), b(false));
    assert_eq!(call("NOT", &[s(txt("TRUE"))], &c), b(false));
}

#[test]
fn sheet_fn_logical_not_error_propagates() {
    let c = ctx();
    assert_eq!(
        call("NOT", &[s(err(CellError::Ref))], &c),
        err(CellError::Ref)
    );
    // Non-boolean text -> #VALUE!.
    assert_eq!(call("NOT", &[s(txt("x"))], &c), err(CellError::Value));
}

#[test]
fn sheet_fn_logical_not_arity_violation() {
    let c = ctx();
    // Zero args (< min 1) and two args (> max 1) -> #VALUE!.
    assert_eq!(call("NOT", &[], &c), err(CellError::Value));
    assert_eq!(
        call("NOT", &[s(b(true)), s(b(false))], &c),
        err(CellError::Value)
    );
}

// ================= TRUE / FALSE =================

#[test]
fn sheet_fn_logical_true_basic() {
    let c = ctx();
    assert_eq!(call("TRUE", &[], &c), b(true));
}

#[test]
fn sheet_fn_logical_true_arity_violation() {
    let c = ctx();
    // TRUE takes NO args (max 0) -> any arg is #VALUE!.
    assert_eq!(call("TRUE", &[s(num(1.0))], &c), err(CellError::Value));
}

#[test]
fn sheet_fn_logical_false_basic() {
    let c = ctx();
    assert_eq!(call("FALSE", &[], &c), b(false));
}

#[test]
fn sheet_fn_logical_false_arity_violation() {
    let c = ctx();
    assert_eq!(call("FALSE", &[s(num(1.0))], &c), err(CellError::Value));
}

// ================= IFERROR =================

#[test]
fn sheet_fn_logical_iferror_basic() {
    let c = ctx();
    // Primary is not an error -> returned verbatim.
    assert_eq!(
        call("IFERROR", &[s(num(42.0)), s(txt("fallback"))], &c),
        num(42.0)
    );
    // Primary IS an error -> the fallback.
    assert_eq!(
        call(
            "IFERROR",
            &[s(err(CellError::Div0)), s(txt("fallback"))],
            &c
        ),
        txt("fallback")
    );
}

#[test]
fn sheet_fn_logical_iferror_catches_all_codes() {
    let c = ctx();
    for code in [
        CellError::Div0,
        CellError::Value,
        CellError::Ref,
        CellError::Name,
        CellError::Num,
        CellError::Na,
        CellError::Null,
        CellError::Spill,
    ] {
        assert_eq!(
            call("IFERROR", &[s(err(code)), s(num(0.0))], &c),
            num(0.0),
            "IFERROR should catch {}",
            code.as_str()
        );
    }
}

#[test]
fn sheet_fn_logical_iferror_passes_through_non_errors() {
    let c = ctx();
    // Blank, text, bool, and number primaries all pass through unchanged.
    assert_eq!(
        call("IFERROR", &[s(CellValue::Empty), s(num(9.0))], &c),
        CellValue::Empty
    );
    assert_eq!(call("IFERROR", &[s(txt("ok")), s(num(9.0))], &c), txt("ok"));
    assert_eq!(call("IFERROR", &[s(b(true)), s(num(9.0))], &c), b(true));
}

#[test]
fn sheet_fn_logical_iferror_fallback_may_be_error() {
    let c = ctx();
    // When the primary errors, the fallback is returned EVEN if it is itself
    // an error (IFERROR only tests the primary's error-ness).
    assert_eq!(
        call(
            "IFERROR",
            &[s(err(CellError::Na)), s(err(CellError::Ref))],
            &c
        ),
        err(CellError::Ref)
    );
}

#[test]
fn sheet_fn_logical_iferror_arity_violation() {
    let c = ctx();
    // One arg (< min 2) and three args (> max 2) -> #VALUE!.
    assert_eq!(call("IFERROR", &[s(num(1.0))], &c), err(CellError::Value));
    assert_eq!(
        call("IFERROR", &[s(num(1.0)), s(num(2.0)), s(num(3.0))], &c),
        err(CellError::Value)
    );
}

// ================= formula-level golden corpus replay =================
//
// The `corpus/fn-corpus/logical/*.golden.tsv` goldens are FORMULA-level
// fixtures (`=AND(A1:A3)` + `A1=1;A2=2;A3=3` setup) that the Phase-2 end-to-end
// runner replays through `sheet-calc` (still an empty stub today). To keep them
// honest against THIS implementation now — without a dependency on the
// unbuilt evaluator — these tests drive each fixture through the FROZEN
// `sheet_fn::dispatch` with a tiny self-contained decoder for the simple
// `=FUNC(arg;arg;…)` shape the goldens use (a single function call; args are
// scalar literals, A1-style cell refs resolved from `setup`, or a `A1:A3`
// range). General-formatted output reuses `coerce::to_text`, the same contract
// the display path realizes. Same TSV, two consumers (here + Phase 2).

mod corpus {
    use super::*;
    use sheet_core::funcs::lookup_func;
    use sheet_fn::coerce;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// One golden row: `id<TAB>formula<TAB>setup<TAB>expected`.
    struct Case {
        id: String,
        formula: String,
        setup: String,
        expected: String,
    }

    fn corpus_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("corpus/fn-corpus/logical")
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

    /// Classify a bare setup/literal token into a [`CellValue`]: a parseable
    /// finite number is numeric, an `#…!` token is an error, `TRUE`/`FALSE`
    /// (case-insensitive) a bool, an empty token a blank cell, otherwise text.
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

    /// Parse `A1=1;A2=foo;B2=TRUE` into an address→value map.
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

    /// 1-based A1 cell address (`B3`) → 0-based (col, row), uppercase input.
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

    /// One materialized argument from a formula token: a scalar value or an
    /// owned range buffer (so the borrowing `Arg::Range` view stays valid).
    enum Owned {
        Scalar(CellValue),
        Range {
            origin: CellRef,
            rows: u32,
            cols: u32,
            cells: Vec<CellValue>,
        },
    }

    /// Decode one formula argument: a `A1:B2` range, a `A1` cell ref, a
    /// double-quoted string, or a bare literal (number/bool/error/text).
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
        // A lone A1 cell ref (letters then digits) resolves from setup; any
        // other bare token is a literal.
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
            // A ref to a cell absent from setup is a blank cell.
            let (c, r) = parse_addr(&upper);
            // Confirm it is a syntactically valid address before treating it
            // as an empty cell (otherwise fall through to a literal).
            let _ = (c, r);
            return Owned::Scalar(CellValue::Empty);
        }
        Owned::Scalar(classify(tok))
    }

    /// Split a function arg list on top-level `;` (the goldens use `;` as the
    /// argument separator so commas inside are never an issue at T0).
    fn split_args(inner: &str) -> Vec<String> {
        if inner.trim().is_empty() {
            return Vec::new();
        }
        inner.split(';').map(|s| s.trim().to_string()).collect()
    }

    /// Evaluate one `=FUNC(args)` formula through the frozen dispatch and
    /// return the General-format display string of the result.
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
    fn sheet_fn_logical_if_corpus() {
        run("if.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_and_corpus() {
        run("and.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_or_corpus() {
        run("or.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_not_corpus() {
        run("not.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_true_corpus() {
        run("true.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_false_corpus() {
        run("false.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_iferror_corpus() {
        run("iferror.golden.tsv");
    }
}
