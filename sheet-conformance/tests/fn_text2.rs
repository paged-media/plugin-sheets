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

//! M1 text (text2) family conformance (spec §7/§11, milestone M1).
//! SELF-CONTAINED direct-dispatch tests: each case resolves the function
//! name through `sheet_core::funcs::lookup_func` and routes the call through
//! [`sheet_fn::dispatch`] — the same choke point a real evaluation crosses,
//! so the generated arity guards (`#VALUE!` on violation) are exercised
//! end-to-end, not bypassed by calling the kernel directly.
//!
//! Every function gets at least one `fn sheet_fn_text2_<name>…` test (the
//! prefix the registry rows in `text2.yaml` point at, which the coverage gate
//! greps for). The cases cover happy path, coercion edge, error propagation,
//! range behavior (`TEXTJOIN`), arity violation, and each named Excel ruling:
//! TEXTJOIN ignore_empty / range-walk / contained-error; EXACT case-sensitive;
//! PROPER title-case + apostrophe; CHAR/CODE latin-1 1..=255; UNICHAR/UNICODE
//! full-Unicode + surrogate `#VALUE!`; CLEAN 0x00..=0x1F strip; T text-or-empty;
//! NUMBERVALUE separators + percent; FIXED negative-decimals + no_commas;
//! DOLLAR `$` currency; REPLACE 1-based + append; TEXTBEFORE/TEXTAFTER instance
//! (negative-from-end) + not-found `#N/A`.

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::arg::{Arg, RangeView};
use sheet_fn::{dispatch, EvalCtx};

// ---- harness ---------------------------------------------------------------

fn cell() -> CellRef {
    CellRef {
        sheet: 0,
        row: 0,
        col: 0,
        row_abs: false,
        col_abs: false,
    }
}

/// A deterministic context (fixed now-serial + seed).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cell(), 45000.5, 42)
}

/// Dispatch a function by registry name with the given args (the real choke
/// point — proves the registry row is wired and the arity guard fires).
fn call(name: &str, args: &[Arg]) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("function {name} not in registry"));
    dispatch(id, args, &ctx())
}

fn num(x: f64) -> Arg<'static> {
    Arg::Scalar(CellValue::Number(x))
}
fn txt(s: &str) -> Arg<'static> {
    Arg::Scalar(CellValue::from(s))
}
fn boolean(b: bool) -> Arg<'static> {
    Arg::Scalar(CellValue::Bool(b))
}
fn empty() -> Arg<'static> {
    Arg::Scalar(CellValue::Empty)
}
fn err(e: CellError) -> Arg<'static> {
    Arg::Scalar(CellValue::Error(e))
}

fn t(s: &str) -> CellValue {
    CellValue::from(s)
}
fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}
fn b(x: bool) -> CellValue {
    CellValue::Bool(x)
}
fn e(c: CellError) -> CellValue {
    CellValue::Error(c)
}

/// A 1-row range argument from owned cells (lives only as long as the slice).
fn range1<'a>(cells: &'a [CellValue]) -> Arg<'a> {
    let cols = cells.len() as u32;
    Arg::Range(RangeView::from_slice(cell(), 1, cols, cells))
}

// ============================ TEXTJOIN ============================

#[test]
fn sheet_fn_text2_textjoin_basic() {
    // delimiter, ignore_empty, then variadic text.
    assert_eq!(
        call(
            "TEXTJOIN",
            &[txt("-"), boolean(true), txt("a"), txt("b"), txt("c")]
        ),
        t("a-b-c")
    );
    // A numeric / bool field coerces to its General text.
    assert_eq!(
        call("TEXTJOIN", &[txt(", "), boolean(true), num(1.0), num(2.0)]),
        t("1, 2")
    );
}

#[test]
fn sheet_fn_text2_textjoin_ignore_empty() {
    let cells = [t("a"), CellValue::Empty, t("b"), t("")];
    // ignore_empty TRUE: blank cell AND empty string are skipped, no stray "-".
    assert_eq!(
        call("TEXTJOIN", &[txt("-"), boolean(true), range1(&cells)]),
        t("a-b")
    );
    // ignore_empty FALSE: every field contributes, so empties keep the gaps.
    assert_eq!(
        call("TEXTJOIN", &[txt("-"), boolean(false), range1(&cells)]),
        t("a--b-")
    );
}

#[test]
fn sheet_fn_text2_textjoin_range_walk_and_error() {
    // Range contributes every cell row-major.
    let cells = [t("x"), t("y"), t("z")];
    assert_eq!(
        call("TEXTJOIN", &[txt("|"), boolean(true), range1(&cells)]),
        t("x|y|z")
    );
    // An error INSIDE a joined range propagates (Excel).
    let with_err = [t("x"), CellValue::Error(CellError::Div0), t("z")];
    assert_eq!(
        call("TEXTJOIN", &[txt("|"), boolean(true), range1(&with_err)]),
        e(CellError::Div0)
    );
    // A scalar error in delimiter/ignore_empty propagates first.
    assert_eq!(
        call("TEXTJOIN", &[err(CellError::Na), boolean(true), txt("a")]),
        e(CellError::Na)
    );
}

#[test]
fn sheet_fn_text2_textjoin_arity() {
    // min 3 (delimiter, ignore_empty, at least one text). Two args -> #VALUE!.
    assert_eq!(
        call("TEXTJOIN", &[txt("-"), boolean(true)]),
        e(CellError::Value)
    );
}

// ============================ EXACT ============================

#[test]
fn sheet_fn_text2_exact_basic() {
    assert_eq!(call("EXACT", &[txt("abc"), txt("abc")]), b(true));
    // Case-sensitive: distinguishes case where `=` would fold.
    assert_eq!(call("EXACT", &[txt("a"), txt("A")]), b(false));
    // Coerced forms compare on their text (number vs its General text).
    assert_eq!(call("EXACT", &[num(12.0), txt("12")]), b(true));
}

#[test]
fn sheet_fn_text2_exact_error_and_arity() {
    assert_eq!(
        call("EXACT", &[err(CellError::Value), txt("a")]),
        e(CellError::Value)
    );
    // arity min/max 2.
    assert_eq!(call("EXACT", &[txt("a")]), e(CellError::Value));
    assert_eq!(
        call("EXACT", &[txt("a"), txt("b"), txt("c")]),
        e(CellError::Value)
    );
}

// ============================ PROPER ============================

#[test]
fn sheet_fn_text2_proper_basic() {
    assert_eq!(call("PROPER", &[txt("hello world")]), t("Hello World"));
    // Apostrophe is a non-letter, so it begins a new word: O'Brien.
    assert_eq!(call("PROPER", &[txt("o'brien")]), t("O'Brien"));
    // Digits/punct delimit words too; mixed case is normalized.
    assert_eq!(
        call("PROPER", &[txt("2-cent's worth")]),
        t("2-Cent'S Worth")
    );
}

#[test]
fn sheet_fn_text2_proper_error_and_arity() {
    assert_eq!(call("PROPER", &[err(CellError::Na)]), e(CellError::Na));
    assert_eq!(call("PROPER", &[]), e(CellError::Value));
}

// ============================ CHAR / CODE ============================

#[test]
fn sheet_fn_text2_char_basic() {
    assert_eq!(call("CHAR", &[num(65.0)]), t("A"));
    assert_eq!(call("CHAR", &[num(97.0)]), t("a"));
    // Truncates to integer.
    assert_eq!(call("CHAR", &[num(66.9)]), t("B"));
}

#[test]
fn sheet_fn_text2_char_range_ruling() {
    // latin-1 1..=255 only; 0 and >255 are #VALUE!.
    assert_eq!(call("CHAR", &[num(0.0)]), e(CellError::Value));
    assert_eq!(call("CHAR", &[num(256.0)]), e(CellError::Value));
    assert_eq!(call("CHAR", &[num(-1.0)]), e(CellError::Value));
    // 255 is the top of latin-1 (ÿ).
    assert_eq!(call("CHAR", &[num(255.0)]), t("\u{00FF}"));
}

#[test]
fn sheet_fn_text2_code_basic() {
    assert_eq!(call("CODE", &[txt("A")]), n(65.0));
    // First character only.
    assert_eq!(call("CODE", &[txt("Apple")]), n(65.0));
    // Empty text -> #VALUE!.
    assert_eq!(call("CODE", &[txt("")]), e(CellError::Value));
}

#[test]
fn sheet_fn_text2_code_error_and_arity() {
    assert_eq!(call("CODE", &[err(CellError::Div0)]), e(CellError::Div0));
    assert_eq!(call("CODE", &[]), e(CellError::Value));
}

// ============================ UNICHAR / UNICODE ============================

#[test]
fn sheet_fn_text2_unichar_basic() {
    assert_eq!(call("UNICHAR", &[num(65.0)]), t("A"));
    // Full Unicode: the Euro sign (U+20AC).
    assert_eq!(call("UNICHAR", &[num(8364.0)]), t("\u{20AC}"));
}

#[test]
fn sheet_fn_text2_unichar_invalid_is_value() {
    // 0, negative, surrogate (0xD800), and > 0x10FFFF are #VALUE!.
    assert_eq!(call("UNICHAR", &[num(0.0)]), e(CellError::Value));
    assert_eq!(call("UNICHAR", &[num(-3.0)]), e(CellError::Value));
    assert_eq!(call("UNICHAR", &[num(0xD800 as f64)]), e(CellError::Value));
    assert_eq!(
        call("UNICHAR", &[num(0x110000 as f64)]),
        e(CellError::Value)
    );
}

#[test]
fn sheet_fn_text2_unicode_basic() {
    assert_eq!(call("UNICODE", &[txt("A")]), n(65.0));
    // The full-Unicode counterpart of CODE: Euro -> 8364, not a latin-1 truncation.
    assert_eq!(call("UNICODE", &[txt("\u{20AC}")]), n(8364.0));
    assert_eq!(call("UNICODE", &[txt("")]), e(CellError::Value));
}

#[test]
fn sheet_fn_text2_unicode_arity() {
    assert_eq!(call("UNICODE", &[]), e(CellError::Value));
    assert_eq!(call("UNICHAR", &[]), e(CellError::Value));
}

// ============================ CLEAN ============================

#[test]
fn sheet_fn_text2_clean_basic() {
    // Strip ASCII control chars (0x00..=0x1F): tab + newline removed.
    assert_eq!(call("CLEAN", &[txt("a\tb\nc")]), t("abc"));
    // Printable (incl. non-ASCII) passes through.
    assert_eq!(call("CLEAN", &[txt("h\u{00E9}llo")]), t("h\u{00E9}llo"));
    // 0x1F removed, 0x20 (space) kept.
    assert_eq!(call("CLEAN", &[txt("x\u{001F} y")]), t("x y"));
}

#[test]
fn sheet_fn_text2_clean_error_and_arity() {
    assert_eq!(call("CLEAN", &[err(CellError::Value)]), e(CellError::Value));
    assert_eq!(call("CLEAN", &[]), e(CellError::Value));
}

// ============================ T ============================

#[test]
fn sheet_fn_text2_t_basic() {
    // Text passes through; non-text -> empty string.
    assert_eq!(call("T", &[txt("hello")]), t("hello"));
    assert_eq!(call("T", &[num(42.0)]), t(""));
    assert_eq!(call("T", &[boolean(true)]), t(""));
    assert_eq!(call("T", &[empty()]), t(""));
}

#[test]
fn sheet_fn_text2_t_error_and_arity() {
    // An error argument propagates (Excel returns the error, not "").
    assert_eq!(call("T", &[err(CellError::Na)]), e(CellError::Na));
    assert_eq!(call("T", &[]), e(CellError::Value));
}

// ============================ NUMBERVALUE ============================

#[test]
fn sheet_fn_text2_numbervalue_basic() {
    // Default separators: "." decimal, "," group.
    assert_eq!(call("NUMBERVALUE", &[txt("1,234.5")]), n(1234.5));
    assert_eq!(call("NUMBERVALUE", &[txt("2.5")]), n(2.5));
    // Blank text -> 0.
    assert_eq!(call("NUMBERVALUE", &[txt("   ")]), n(0.0));
}

#[test]
fn sheet_fn_text2_numbervalue_custom_separators() {
    // European separators: "," decimal, "." group -> 3.5.
    assert_eq!(
        call("NUMBERVALUE", &[txt("3,5"), txt(","), txt(".")]),
        n(3.5)
    );
    assert_eq!(
        call("NUMBERVALUE", &[txt("1.234,5"), txt(","), txt(".")]),
        n(1234.5)
    );
    // An explicitly empty separator is #VALUE!.
    assert_eq!(
        call("NUMBERVALUE", &[txt("1"), txt("")]),
        e(CellError::Value)
    );
}

#[test]
fn sheet_fn_text2_numbervalue_percent_and_errors() {
    // A trailing run of '%' divides by 100 each.
    assert_eq!(call("NUMBERVALUE", &[txt("50%")]), n(0.5));
    assert_eq!(call("NUMBERVALUE", &[txt("100%%")]), n(0.01));
    // Un-parseable -> #VALUE!.
    assert_eq!(call("NUMBERVALUE", &[txt("abc")]), e(CellError::Value));
    // Error propagation + arity (min 1).
    assert_eq!(
        call("NUMBERVALUE", &[err(CellError::Div0)]),
        e(CellError::Div0)
    );
    assert_eq!(call("NUMBERVALUE", &[]), e(CellError::Value));
}

// ============================ FIXED ============================

#[test]
fn sheet_fn_text2_fixed_basic() {
    // Default 2 decimals, thousands grouping.
    assert_eq!(call("FIXED", &[num(1234.567)]), t("1,234.57"));
    assert_eq!(call("FIXED", &[num(1234.567), num(1.0)]), t("1,234.6"));
    // Zero decimals -> no fractional part.
    assert_eq!(call("FIXED", &[num(1234.567), num(0.0)]), t("1,235"));
}

#[test]
fn sheet_fn_text2_fixed_no_commas_and_negative_decimals() {
    // no_commas TRUE drops grouping.
    assert_eq!(
        call("FIXED", &[num(1234.567), num(2.0), boolean(true)]),
        t("1234.57")
    );
    // Negative decimals round left of the point, format at 0 decimals.
    assert_eq!(call("FIXED", &[num(1234.567), num(-1.0)]), t("1,230"));
    assert_eq!(call("FIXED", &[num(1234.567), num(-2.0)]), t("1,200"));
    // Negative number keeps a leading minus.
    assert_eq!(call("FIXED", &[num(-1234.5), num(0.0)]), t("-1,235"));
}

#[test]
fn sheet_fn_text2_fixed_error_and_arity() {
    assert_eq!(call("FIXED", &[err(CellError::Na)]), e(CellError::Na));
    // Un-parseable scalar number -> #VALUE!.
    assert_eq!(call("FIXED", &[txt("x")]), e(CellError::Value));
    assert_eq!(call("FIXED", &[]), e(CellError::Value));
}

// ============================ DOLLAR ============================

#[test]
fn sheet_fn_text2_dollar_basic() {
    // Default 2 decimals, leading $, thousands grouping.
    assert_eq!(call("DOLLAR", &[num(1234.567)]), t("$1,234.57"));
    assert_eq!(call("DOLLAR", &[num(1234.5), num(0.0)]), t("$1,235"));
    // Negative -> the $ sits with the leading minus (T0 simple form).
    assert_eq!(call("DOLLAR", &[num(-1234.5), num(0.0)]), t("-$1,235"));
}

#[test]
fn sheet_fn_text2_dollar_negative_decimals_and_errors() {
    // Negative decimals round left of the point (like FIXED), still grouped.
    assert_eq!(call("DOLLAR", &[num(1234.567), num(-2.0)]), t("$1,200"));
    assert_eq!(
        call("DOLLAR", &[err(CellError::Value)]),
        e(CellError::Value)
    );
    assert_eq!(call("DOLLAR", &[]), e(CellError::Value));
}

// ============================ REPLACE ============================

#[test]
fn sheet_fn_text2_replace_basic() {
    // 1-based: replace 5 chars from position 1.
    assert_eq!(
        call("REPLACE", &[txt("abcdefg"), num(1.0), num(3.0), txt("XY")]),
        t("XYdefg")
    );
    // Zero-length replacement = insertion at start_num.
    assert_eq!(
        call("REPLACE", &[txt("abc"), num(2.0), num(0.0), txt("--")]),
        t("a--bc")
    );
}

#[test]
fn sheet_fn_text2_replace_edges() {
    // start past the end appends new_text.
    assert_eq!(
        call("REPLACE", &[txt("abc"), num(10.0), num(2.0), txt("XY")]),
        t("abcXY")
    );
    // num_chars running past the end clamps to the remainder.
    assert_eq!(
        call("REPLACE", &[txt("abcdef"), num(3.0), num(99.0), txt("Z")]),
        t("abZ")
    );
}

#[test]
fn sheet_fn_text2_replace_errors_and_arity() {
    // start_num < 1 -> #VALUE!.
    assert_eq!(
        call("REPLACE", &[txt("abc"), num(0.0), num(1.0), txt("X")]),
        e(CellError::Value)
    );
    // negative num_chars -> #VALUE!.
    assert_eq!(
        call("REPLACE", &[txt("abc"), num(1.0), num(-1.0), txt("X")]),
        e(CellError::Value)
    );
    // error propagation + arity (exactly 4).
    assert_eq!(
        call(
            "REPLACE",
            &[err(CellError::Na), num(1.0), num(1.0), txt("X")]
        ),
        e(CellError::Na)
    );
    assert_eq!(
        call("REPLACE", &[txt("abc"), num(1.0), num(1.0)]),
        e(CellError::Value)
    );
}

// ============================ TEXTBEFORE / TEXTAFTER ============================

#[test]
fn sheet_fn_text2_textbefore_basic() {
    assert_eq!(call("TEXTBEFORE", &[txt("a-b-c"), txt("-")]), t("a"));
    // instance 2: before the 2nd "-".
    assert_eq!(
        call("TEXTBEFORE", &[txt("a-b-c"), txt("-"), num(2.0)]),
        t("a-b")
    );
    // negative instance counts from the end: -1 = before the last "-".
    assert_eq!(
        call("TEXTBEFORE", &[txt("a-b-c"), txt("-"), num(-1.0)]),
        t("a-b")
    );
}

#[test]
fn sheet_fn_text2_textafter_basic() {
    assert_eq!(call("TEXTAFTER", &[txt("a-b-c"), txt("-")]), t("b-c"));
    assert_eq!(
        call("TEXTAFTER", &[txt("a-b-c"), txt("-"), num(2.0)]),
        t("c")
    );
    // negative instance: -1 = after the last "-".
    assert_eq!(
        call("TEXTAFTER", &[txt("a-b-c"), txt("-"), num(-1.0)]),
        t("c")
    );
}

#[test]
fn sheet_fn_text2_textbefore_case_insensitive_and_empty_delim() {
    // Case-insensitive default matching.
    assert_eq!(call("TEXTBEFORE", &[txt("aXbXc"), txt("x")]), t("a"));
    // Empty delimiter: split at position 0 -> "" before, whole text after.
    assert_eq!(call("TEXTBEFORE", &[txt("abc"), txt("")]), t(""));
    assert_eq!(call("TEXTAFTER", &[txt("abc"), txt("")]), t("abc"));
}

#[test]
fn sheet_fn_text2_textsplit_not_found_and_errors() {
    // A not-found delimiter / instance is #N/A.
    assert_eq!(
        call("TEXTBEFORE", &[txt("abc"), txt("-")]),
        e(CellError::Na)
    );
    assert_eq!(call("TEXTAFTER", &[txt("abc"), txt("-")]), e(CellError::Na));
    // instance past the count is #N/A.
    assert_eq!(
        call("TEXTBEFORE", &[txt("a-b"), txt("-"), num(5.0)]),
        e(CellError::Na)
    );
    // instance 0 is #N/A.
    assert_eq!(
        call("TEXTAFTER", &[txt("a-b"), txt("-"), num(0.0)]),
        e(CellError::Na)
    );
    // error propagation + arity (min 2).
    assert_eq!(
        call("TEXTBEFORE", &[err(CellError::Div0), txt("-")]),
        e(CellError::Div0)
    );
    assert_eq!(call("TEXTAFTER", &[txt("a")]), e(CellError::Value));
}

// ============================ corpus replay (end-to-end) ============================
//
// The shared `tests/corpus_runner.rs` has a fixed list of `sheet_calc_corpus_<family>`
// tests (the seven T0 families) and no `text2` arm — so the text2 goldens under
// `corpus/fn-corpus/text2/` would otherwise never be replayed through the real
// engine. This test owns that gate: it loads every text2 golden and drives it
// through `sheet_calc::Engine` exactly as `corpus_runner` does (seed setup, enter
// the formula, recalc, compare the General projection / error literal), so the
// registry `tests.corpus` pointers are genuinely end-to-end verified, not merely
// existence-checked by the coverage gate.

mod corpus_replay {
    use sheet_calc::{Engine, EngineConfig, SetInput};
    use sheet_conformance::load_corpus;
    use sheet_core::{CellValue, SheetId, SheetModel};
    use sheet_fn::coerce;

    const SHEET: SheetId = 0;

    /// The 15 text2 golden files (one per registry row), repo-relative.
    const FILES: &[&str] = &[
        "textjoin",
        "exact",
        "proper",
        "char",
        "code",
        "unichar",
        "unicode",
        "clean",
        "t",
        "numbervalue",
        "fixed",
        "dollar",
        "replace",
        "textbefore",
        "textafter",
    ];

    fn fresh_engine() -> Engine {
        let mut m = SheetModel::new();
        m.add_sheet("Sheet1");
        Engine::new(m, EngineConfig::default())
    }

    /// 1-based A1 (`B3`) -> 0-based `(row, col)`.
    fn parse_addr(addr: &str) -> (u32, u32) {
        let upper = addr.trim().to_ascii_uppercase();
        let split = upper
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or_else(|| panic!("bad A1 address {addr:?}"));
        let (col_s, row_s) = upper.split_at(split);
        let col = sheet_core::a1_to_col(col_s).unwrap_or_else(|| panic!("bad column in {addr:?}"));
        let row: u32 = row_s
            .parse()
            .unwrap_or_else(|_| panic!("bad row in {addr:?}"));
        (row - 1, col)
    }

    /// Apply one setup seed (mirrors corpus_runner: `empty`/`text:`/`bool:` tags,
    /// else Excel-like literal detection through `enter`).
    fn apply_setup(e: &mut Engine, addr: &str, raw: &str, id: &str) {
        let (row, col) = parse_addr(addr);
        if raw == "empty" {
            e.set_cell(SHEET, row, col, SetInput::Empty);
        } else if let Some(rest) = raw.strip_prefix("text:") {
            e.set_cell(
                SHEET,
                row,
                col,
                SetInput::Value(CellValue::Text(rest.into())),
            );
        } else if let Some(rest) = raw.strip_prefix("bool:") {
            let b = match rest.trim().to_ascii_uppercase().as_str() {
                "TRUE" => true,
                "FALSE" => false,
                other => panic!("[{id}] bad bool: setup {other:?}"),
            };
            e.set_cell(SHEET, row, col, SetInput::Value(CellValue::Bool(b)));
        } else {
            e.enter(SHEET, row, col, raw)
                .unwrap_or_else(|err| panic!("[{id}] setup {addr}={raw:?} parse error: {err:?}"));
        }
    }

    fn project(v: &CellValue) -> String {
        match v {
            CellValue::Error(err) => err.as_str().to_string(),
            other => coerce::to_text(other).to_string(),
        }
    }

    #[test]
    fn sheet_fn_text2_corpus_replay() {
        let mut failures: Vec<String> = Vec::new();
        let mut total = 0usize;
        for name in FILES {
            let rel = format!("corpus/fn-corpus/text2/{name}.golden.tsv");
            for case in load_corpus(&rel) {
                total += 1;
                let mut e = fresh_engine();
                // Host the formula at Z99 (outside every text2 setup, max D1).
                let (frow, fcol) = (98u32, 25u32);
                for (addr, raw) in &case.setup {
                    if addr == "-" {
                        continue;
                    }
                    apply_setup(&mut e, addr, raw, &case.id);
                }
                if let Err(err) = e.enter(SHEET, frow, fcol, &case.formula) {
                    failures.push(format!(
                        "{rel} [{}] {:?} parse error: {err:?}",
                        case.id, case.formula
                    ));
                    continue;
                }
                let value = e
                    .model()
                    .sheet(SHEET)
                    .and_then(|ws| ws.cell(frow, fcol))
                    .map(|c| c.value.clone())
                    .unwrap_or(CellValue::Empty);
                let got = project(&value);
                if got != case.expected {
                    failures.push(format!(
                        "{rel} [{}] {} (setup {:?}) -> got {:?}, want {:?}",
                        case.id, case.formula, case.setup, got, case.expected
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "text2 corpus: {}/{} case(s) failed:\n{}",
            failures.len(),
            total,
            failures.join("\n")
        );
    }
}
