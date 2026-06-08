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

//! Logical-family conformance, M1 additions (spec §7, §11 T1): `IFS`, `SWITCH`,
//! `XOR`, `IFNA`. Self-contained **direct-dispatch** tests — each resolves a
//! function id via [`sheet_core::funcs::lookup_func`] and routes through the
//! frozen [`sheet_fn::dispatch`], so they exercise the SAME path a formula
//! evaluation takes (registry arity guard included). Test-fn names use the
//! prefix the `registry/functions/logical2.yaml` rows point at
//! (`sheet_fn_logical_<name>`) so the §12.2 coverage gate finds them.
//!
//! T1 eager note (mirrors `sheet_fn::families::logical2` module docs): these
//! selectors are EAGER here — `IFS`/`SWITCH`/`IFNA` candidate values arrive
//! already evaluated. Short-circuit (error-masking on the discarded branch) is
//! a `sheet-calc` Phase-2 special form, not a kernel concern, so these tests
//! assert value-selection behavior only.

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

// ================= IFS =================

#[test]
fn sheet_fn_logical_ifs_first_true_wins() {
    let c = ctx();
    // The first TRUE condition (the second pair) returns its paired value.
    assert_eq!(
        call(
            "IFS",
            &[s(b(false)), s(txt("a")), s(b(true)), s(txt("b"))],
            &c
        ),
        txt("b")
    );
    // When the FIRST pair is true it wins over later true pairs.
    assert_eq!(
        call(
            "IFS",
            &[s(b(true)), s(txt("a")), s(b(true)), s(txt("b"))],
            &c
        ),
        txt("a")
    );
}

#[test]
fn sheet_fn_logical_ifs_number_coercion() {
    let c = ctx();
    // Numeric conditions coerce: 0 is FALSE, non-zero is TRUE.
    assert_eq!(
        call(
            "IFS",
            &[s(num(0.0)), s(txt("x")), s(num(5.0)), s(txt("y"))],
            &c
        ),
        txt("y")
    );
}

#[test]
fn sheet_fn_logical_ifs_no_match_is_na() {
    let c = ctx();
    // No condition is TRUE — IFS has no default slot, so #N/A (Excel ruling).
    assert_eq!(
        call(
            "IFS",
            &[s(b(false)), s(num(1.0)), s(num(0.0)), s(num(2.0))],
            &c
        ),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_logical_ifs_odd_arity_is_na() {
    let c = ctx();
    // A dangling final condition (odd argument count) with no value → #N/A.
    assert_eq!(
        call("IFS", &[s(b(false)), s(num(1.0)), s(b(true))], &c),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_logical_ifs_error_condition_propagates() {
    let c = ctx();
    // An error in a condition propagates (first-condition-error-wins).
    assert_eq!(
        call("IFS", &[s(err(CellError::Div0)), s(num(1.0))], &c),
        err(CellError::Div0)
    );
    // Non-boolean text in a condition → #VALUE! (via coerce::to_bool).
    assert_eq!(
        call("IFS", &[s(txt("maybe")), s(num(1.0))], &c),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_logical_ifs_range_condition_top_left() {
    let c = ctx();
    // range_aware:false — a range condition collapses to its top-left cell.
    let cells = [b(true), b(false)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    assert_eq!(
        call("IFS", &[Arg::Range(rv), s(txt("hit"))], &c),
        txt("hit")
    );
}

#[test]
fn sheet_fn_logical_ifs_arity_violation() {
    let c = ctx();
    // One arg (< min 2) → #VALUE! from the dispatch arity guard.
    assert_eq!(call("IFS", &[s(b(true))], &c), err(CellError::Value));
}

// ================= SWITCH =================

#[test]
fn sheet_fn_logical_switch_matches_and_defaults() {
    let c = ctx();
    // 2 matches the second value → "two".
    assert_eq!(
        call(
            "SWITCH",
            &[
                s(num(2.0)),
                s(num(1.0)),
                s(txt("one")),
                s(num(2.0)),
                s(txt("two")),
            ],
            &c
        ),
        txt("two")
    );
    // No match, lone trailing default returned.
    assert_eq!(
        call(
            "SWITCH",
            &[s(num(9.0)), s(num(1.0)), s(txt("one")), s(txt("default"))],
            &c
        ),
        txt("default")
    );
}

#[test]
fn sheet_fn_logical_switch_no_match_no_default_is_na() {
    let c = ctx();
    // Even trailing shape (no default) and no match → #N/A.
    assert_eq!(
        call("SWITCH", &[s(num(9.0)), s(num(1.0)), s(txt("one"))], &c),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_logical_switch_cross_type_and_case_fold() {
    let c = ctx();
    // A number expression never matches the text "5" (cross-type total order).
    assert_eq!(
        call(
            "SWITCH",
            &[s(num(5.0)), s(txt("5")), s(txt("text")), s(txt("def"))],
            &c
        ),
        txt("def")
    );
    // Text matches case-insensitively (the compare fold).
    assert_eq!(
        call(
            "SWITCH",
            &[s(txt("HELLO")), s(txt("hello")), s(txt("hit"))],
            &c
        ),
        txt("hit")
    );
}

#[test]
fn sheet_fn_logical_switch_error_expression_propagates() {
    let c = ctx();
    // An error expression cannot be compared — it propagates.
    assert_eq!(
        call(
            "SWITCH",
            &[s(err(CellError::Ref)), s(num(1.0)), s(txt("one"))],
            &c
        ),
        err(CellError::Ref)
    );
}

#[test]
fn sheet_fn_logical_switch_range_expression_top_left() {
    let c = ctx();
    // range_aware:false — a range expression collapses to its top-left cell,
    // which then matches a value pair.
    let cells = [num(2.0), num(9.0)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    assert_eq!(
        call(
            "SWITCH",
            &[
                Arg::Range(rv),
                s(num(1.0)),
                s(txt("one")),
                s(num(2.0)),
                s(txt("two"))
            ],
            &c
        ),
        txt("two")
    );
}

#[test]
fn sheet_fn_logical_switch_arity_violation() {
    let c = ctx();
    // Two args (< min 3) → #VALUE! from the dispatch arity guard.
    assert_eq!(
        call("SWITCH", &[s(num(1.0)), s(num(1.0))], &c),
        err(CellError::Value)
    );
}

// ================= XOR =================

#[test]
fn sheet_fn_logical_xor_parity() {
    let c = ctx();
    // One TRUE → TRUE; two TRUEs → FALSE; three → TRUE (odd-count parity).
    assert_eq!(call("XOR", &[s(b(true)), s(b(false))], &c), b(true));
    assert_eq!(call("XOR", &[s(b(true)), s(b(true))], &c), b(false));
    assert_eq!(
        call("XOR", &[s(b(true)), s(b(true)), s(b(true))], &c),
        b(true)
    );
}

#[test]
fn sheet_fn_logical_xor_coercion() {
    let c = ctx();
    // Numbers coerce (non-zero TRUE, zero FALSE).
    assert_eq!(call("XOR", &[s(num(1.0)), s(num(0.0))], &c), b(true));
    assert_eq!(call("XOR", &[s(num(2.0)), s(num(3.0))], &c), b(false));
    // Non-boolean scalar text → #VALUE!.
    assert_eq!(
        call("XOR", &[s(b(true)), s(txt("nope"))], &c),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_logical_xor_range_skips_text_and_blank() {
    let c = ctx();
    // Range with two TRUEs (mixed bool/number); text and blank are ignored →
    // even count → FALSE.
    let cells = [b(true), num(1.0), txt("skip"), CellValue::Empty];
    let rv = RangeView::from_slice(cr(0, 0), 1, 4, &cells);
    assert_eq!(call("XOR", &[Arg::Range(rv)], &c), b(false));
}

#[test]
fn sheet_fn_logical_xor_range_error_propagates() {
    let c = ctx();
    // An error cell inside the range poisons the whole call.
    let cells = [b(true), err(CellError::Na)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    assert_eq!(call("XOR", &[Arg::Range(rv)], &c), err(CellError::Na));
}

#[test]
fn sheet_fn_logical_xor_no_logical_value_is_value_error() {
    let c = ctx();
    // No logical value found anywhere → #VALUE! (empty-population rule).
    let cells = [txt("a"), CellValue::Empty];
    let rv = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    assert_eq!(call("XOR", &[Arg::Range(rv)], &c), err(CellError::Value));
}

#[test]
fn sheet_fn_logical_xor_arity_violation() {
    let c = ctx();
    // Zero args (< min 1) → #VALUE! from the dispatch arity guard.
    assert_eq!(call("XOR", &[], &c), err(CellError::Value));
}

// ================= IFNA =================

#[test]
fn sheet_fn_logical_ifna_catches_only_na() {
    let c = ctx();
    // #N/A primary → the fallback.
    assert_eq!(
        call("IFNA", &[s(err(CellError::Na)), s(txt("ok"))], &c),
        txt("ok")
    );
    // A different error passes straight through (IFNA catches ONLY #N/A).
    assert_eq!(
        call("IFNA", &[s(err(CellError::Div0)), s(txt("ok"))], &c),
        err(CellError::Div0)
    );
    // A clean value passes through verbatim.
    assert_eq!(call("IFNA", &[s(num(42.0)), s(txt("ok"))], &c), num(42.0));
}

#[test]
fn sheet_fn_logical_ifna_fallback_returned_even_if_error() {
    let c = ctx();
    // When the primary is #N/A, the fallback is returned even if itself an
    // error (only the primary's #N/A-ness is tested).
    assert_eq!(
        call("IFNA", &[s(err(CellError::Na)), s(err(CellError::Ref))], &c),
        err(CellError::Ref)
    );
}

#[test]
fn sheet_fn_logical_ifna_arity_violation() {
    let c = ctx();
    // One arg (< min 2) and three args (> max 2) → #VALUE!.
    assert_eq!(call("IFNA", &[s(num(1.0))], &c), err(CellError::Value));
    assert_eq!(
        call("IFNA", &[s(num(1.0)), s(num(2.0)), s(num(3.0))], &c),
        err(CellError::Value)
    );
}

// ================= formula-level golden corpus replay =================
//
// The `corpus/fn-corpus/logical2/*.golden.tsv` goldens are FORMULA-level
// fixtures that the Phase-2 end-to-end runner will replay through `sheet-calc`.
// To keep them honest against THIS implementation now — without a dependency on
// the evaluator — these tests drive each fixture through the FROZEN
// `sheet_fn::dispatch` with a tiny self-contained decoder for the simple
// `=FUNC(arg;arg;…)` shape the goldens use (a single function call; args are
// scalar literals, A1-style cell refs resolved from `setup`, or an `A1:A3`
// range). General-formatted output reuses `coerce::to_text`, the same contract
// the display path realizes. Same TSV, two consumers (here + Phase 2). The
// decoder mirrors `tests/fn_logical.rs::corpus`.

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
            .join("corpus/fn-corpus/logical2")
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

    /// Classify a bare setup/literal token into a [`CellValue`].
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

    /// One materialized argument from a formula token.
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

    /// Split a function arg list on top-level `;` (the goldens use `;` as the
    /// argument separator). A `;` inside a quoted string is kept intact.
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
    fn sheet_fn_logical_ifs_corpus() {
        run("ifs.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_switch_corpus() {
        run("switch.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_xor_corpus() {
        run("xor.golden.tsv");
    }
    #[test]
    fn sheet_fn_logical_ifna_corpus() {
        run("ifna.golden.tsv");
    }
}
