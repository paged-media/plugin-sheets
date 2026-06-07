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

//! Conformance for the text function family (spec §7, §11 T0). Self-contained
//! direct-dispatch tests: each case resolves a `FuncId` via
//! `sheet_core::funcs::lookup_func`, builds an `&[Arg]` slice, and calls the
//! registry-generated `sheet_fn::dispatch` — exactly the path `sheet-calc`
//! takes. Every registry `tests.rust` pointer is a `fn sheet_fn_text_<name>…`
//! here. Coverage per the track brief: happy path, coercion edge, error
//! propagation, range behavior (range-aware rows), arity violation → `#VALUE!`,
//! and the named Excel rulings (negative counts, FIND vs SEARCH, SUBSTITUTE
//! instance, TEXT via sheet-format, VALUE percent suffix).

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::{dispatch, Arg, EvalCtx, RangeView};

// ---- Tiny harness shared by every case. ----

fn cr(row: u32, col: u32) -> CellRef {
    CellRef {
        sheet: 0,
        row,
        col,
        row_abs: false,
        col_abs: false,
    }
}

/// A deterministic context (Date1900, current B2, fixed clock, fixed seed).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cr(1, 1), 45000.5, 42)
}

/// Resolve a function name to its `FuncId` and dispatch the given args.
fn call(name: &str, args: &[Arg]) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("function {name} not registered"));
    dispatch(id, args, &ctx())
}

fn num(n: f64) -> Arg<'static> {
    Arg::Scalar(CellValue::Number(n))
}
fn txt(s: &str) -> Arg<'static> {
    Arg::Scalar(CellValue::from(s))
}
fn err(e: CellError) -> Arg<'static> {
    Arg::Scalar(CellValue::Error(e))
}
fn bln(b: bool) -> Arg<'static> {
    Arg::Scalar(CellValue::Bool(b))
}
fn empty() -> Arg<'static> {
    Arg::Scalar(CellValue::Empty)
}

/// Assert a call yields the given text.
fn want_text(name: &str, args: &[Arg], expected: &str) {
    match call(name, args) {
        CellValue::Text(t) => assert_eq!(t.as_str(), expected, "{name} text mismatch"),
        other => panic!("{name}: expected Text({expected:?}), got {other:?}"),
    }
}

/// Assert a call yields the given number.
fn want_num(name: &str, args: &[Arg], expected: f64) {
    match call(name, args) {
        CellValue::Number(n) => assert_eq!(n, expected, "{name} number mismatch"),
        other => panic!("{name}: expected Number({expected}), got {other:?}"),
    }
}

/// Assert a call yields the given error.
fn want_err(name: &str, args: &[Arg], expected: CellError) {
    match call(name, args) {
        CellValue::Error(e) => assert_eq!(e, expected, "{name} error mismatch"),
        other => panic!("{name}: expected Error({expected:?}), got {other:?}"),
    }
}

// ============================ LEN ============================

#[test]
fn sheet_fn_text_len_basic() {
    want_num("LEN", &[txt("hello")], 5.0);
    want_num("LEN", &[txt("")], 0.0);
    // Multi-byte char counts as one character (not bytes).
    want_num("LEN", &[txt("héllo")], 5.0);
}

#[test]
fn sheet_fn_text_len_coercion() {
    // Number coerced through its General text form.
    want_num("LEN", &[num(12.5)], 4.0);
    want_num("LEN", &[bln(true)], 4.0); // "TRUE"
    want_num("LEN", &[empty()], 0.0);
}

#[test]
fn sheet_fn_text_len_error_and_arity() {
    want_err("LEN", &[err(CellError::Div0)], CellError::Div0);
    // Arity violation: zero args -> #VALUE!.
    want_err("LEN", &[], CellError::Value);
    want_err("LEN", &[txt("a"), txt("b")], CellError::Value);
}

// ============================ LEFT ============================

#[test]
fn sheet_fn_text_left_basic() {
    want_text("LEFT", &[txt("hello"), num(2.0)], "he");
    // Default count is 1.
    want_text("LEFT", &[txt("hello")], "h");
    // Count past the end clamps.
    want_text("LEFT", &[txt("hi"), num(10.0)], "hi");
}

#[test]
fn sheet_fn_text_left_negative_is_value_error() {
    // Named ruling: negative count -> #VALUE!.
    want_err("LEFT", &[txt("hello"), num(-1.0)], CellError::Value);
}

#[test]
fn sheet_fn_text_left_error_and_arity() {
    want_err("LEFT", &[err(CellError::Na), num(2.0)], CellError::Na);
    want_err("LEFT", &[], CellError::Value); // arity
}

// ============================ RIGHT ============================

#[test]
fn sheet_fn_text_right_basic() {
    want_text("RIGHT", &[txt("hello"), num(2.0)], "lo");
    want_text("RIGHT", &[txt("hello")], "o"); // default 1
    want_text("RIGHT", &[txt("hi"), num(10.0)], "hi"); // clamp
    want_text("RIGHT", &[txt("hello"), num(0.0)], ""); // zero count
}

#[test]
fn sheet_fn_text_right_negative_is_value_error() {
    want_err("RIGHT", &[txt("hello"), num(-3.0)], CellError::Value);
}

#[test]
fn sheet_fn_text_right_error_propagation() {
    want_err("RIGHT", &[err(CellError::Ref)], CellError::Ref);
}

// ============================ MID ============================

#[test]
fn sheet_fn_text_mid_basic() {
    want_text("MID", &[txt("hello"), num(2.0), num(3.0)], "ell");
    // start past end -> "".
    want_text("MID", &[txt("hi"), num(5.0), num(2.0)], "");
    // count past end clamps.
    want_text("MID", &[txt("hello"), num(3.0), num(99.0)], "llo");
}

#[test]
fn sheet_fn_text_mid_negative_is_value_error() {
    // start < 1 -> #VALUE!.
    want_err("MID", &[txt("hello"), num(0.0), num(2.0)], CellError::Value);
    // negative count -> #VALUE!.
    want_err(
        "MID",
        &[txt("hello"), num(1.0), num(-1.0)],
        CellError::Value,
    );
}

#[test]
fn sheet_fn_text_mid_error_and_arity() {
    want_err(
        "MID",
        &[err(CellError::Num), num(1.0), num(1.0)],
        CellError::Num,
    );
    // MID requires exactly 3 args.
    want_err("MID", &[txt("a"), num(1.0)], CellError::Value);
}

// ============================ CONCAT (range-aware) ============================

#[test]
fn sheet_fn_text_concat_basic() {
    want_text("CONCAT", &[txt("a"), txt("b"), txt("c")], "abc");
    // Mixed types coerced to General text.
    want_text("CONCAT", &[txt("x"), num(1.0), bln(true)], "x1TRUE");
}

#[test]
fn sheet_fn_text_concat_range_aware() {
    // A range contributes every cell in row-major order.
    let cells = [
        CellValue::from("a"),
        CellValue::from("b"),
        CellValue::Number(3.0),
        CellValue::from("d"),
    ];
    let rv = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    want_text("CONCAT", &[Arg::Range(rv), txt("!")], "ab3d!");
}

#[test]
fn sheet_fn_text_concat_error_in_range_propagates() {
    // An error cell INSIDE a range propagates (CONCAT walks the range).
    let cells = [CellValue::from("a"), CellValue::Error(CellError::Div0)];
    let rv = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    want_err("CONCAT", &[Arg::Range(rv)], CellError::Div0);
    // Scalar error propagates too.
    want_err("CONCAT", &[txt("a"), err(CellError::Na)], CellError::Na);
    // Arity: at least one arg.
    want_err("CONCAT", &[], CellError::Value);
}

// ============================ CONCATENATE (scalar-only) ============================

#[test]
fn sheet_fn_text_concatenate_basic() {
    want_text("CONCATENATE", &[txt("foo"), txt("bar")], "foobar");
    want_text("CONCATENATE", &[txt("n="), num(42.0)], "n=42");
}

#[test]
fn sheet_fn_text_concatenate_not_range_aware() {
    // NOT range-aware: a range degrades to its top-left cell only.
    let cells = [
        CellValue::from("a"),
        CellValue::from("b"),
        CellValue::from("c"),
        CellValue::from("d"),
    ];
    let rv = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    want_text("CONCATENATE", &[Arg::Range(rv), txt("!")], "a!");
}

#[test]
fn sheet_fn_text_concatenate_error_and_arity() {
    want_err(
        "CONCATENATE",
        &[txt("a"), err(CellError::Value)],
        CellError::Value,
    );
    want_err("CONCATENATE", &[], CellError::Value);
}

// ============================ UPPER ============================

#[test]
fn sheet_fn_text_upper_basic() {
    want_text("UPPER", &[txt("Hello World")], "HELLO WORLD");
    want_text("UPPER", &[txt("abc123")], "ABC123");
    want_text("UPPER", &[num(12.0)], "12"); // number -> text -> upper
}

#[test]
fn sheet_fn_text_upper_error_and_arity() {
    want_err("UPPER", &[err(CellError::Null)], CellError::Null);
    want_err("UPPER", &[], CellError::Value);
}

// ============================ LOWER ============================

#[test]
fn sheet_fn_text_lower_basic() {
    want_text("LOWER", &[txt("Hello World")], "hello world");
    want_text("LOWER", &[bln(true)], "true"); // "TRUE" -> "true"
}

#[test]
fn sheet_fn_text_lower_error_and_arity() {
    want_err("LOWER", &[err(CellError::Spill)], CellError::Spill);
    want_err("LOWER", &[txt("a"), txt("b")], CellError::Value);
}

// ============================ TRIM ============================

#[test]
fn sheet_fn_text_trim_basic() {
    // Leading/trailing spaces removed; internal runs collapsed to one.
    want_text("TRIM", &[txt("  hello   world  ")], "hello world");
    want_text("TRIM", &[txt("nochange")], "nochange");
    want_text("TRIM", &[txt("   ")], "");
}

#[test]
fn sheet_fn_text_trim_error_and_arity() {
    want_err("TRIM", &[err(CellError::Ref)], CellError::Ref);
    want_err("TRIM", &[], CellError::Value);
}

// ============================ TEXT (via sheet-format) ============================

#[test]
fn sheet_fn_text_text_basic() {
    // Fixed-decimal format.
    want_text("TEXT", &[num(1234.5), txt("0.00")], "1234.50");
    // Percent format.
    want_text("TEXT", &[num(0.5), txt("0%")], "50%");
    // General passthrough.
    want_text("TEXT", &[num(42.0), txt("General")], "42");
}

#[test]
fn sheet_fn_text_text_date_system() {
    // Serial 0.5 under a date/time code uses ctx.date_system (Date1900).
    want_text("TEXT", &[num(0.5), txt("h:mm AM/PM")], "12:00 PM");
}

#[test]
fn sheet_fn_text_text_error_and_arity() {
    // Error value propagates before formatting.
    want_err(
        "TEXT",
        &[err(CellError::Div0), txt("0.00")],
        CellError::Div0,
    );
    // TEXT requires exactly 2 args.
    want_err("TEXT", &[num(1.0)], CellError::Value);
}

// ============================ FIND (case-sensitive, literal) ============================

#[test]
fn sheet_fn_text_find_basic() {
    want_num("FIND", &[txt("l"), txt("hello")], 3.0);
    // start_num skips earlier matches.
    want_num("FIND", &[txt("l"), txt("hello"), num(4.0)], 4.0);
    // Empty needle matches at start.
    want_num("FIND", &[txt(""), txt("hello")], 1.0);
}

#[test]
fn sheet_fn_text_find_case_sensitive_no_wildcard() {
    // Case-sensitive: 'H' not found in "hello".
    want_err("FIND", &[txt("H"), txt("hello")], CellError::Value);
    // Wildcards are literal in FIND: "h*" is not a glob.
    want_err("FIND", &[txt("h*"), txt("hello")], CellError::Value);
    want_num("FIND", &[txt("*"), txt("a*b")], 2.0); // literal asterisk
}

#[test]
fn sheet_fn_text_find_not_found_and_error() {
    want_err("FIND", &[txt("z"), txt("hello")], CellError::Value);
    // start_num past the end -> #VALUE!.
    want_err("FIND", &[txt("h"), txt("hi"), num(9.0)], CellError::Value);
    // start_num < 1 -> #VALUE!.
    want_err("FIND", &[txt("h"), txt("hi"), num(0.0)], CellError::Value);
    // Error arg propagates.
    want_err("FIND", &[err(CellError::Na), txt("x")], CellError::Na);
    // Arity: at least 2 args.
    want_err("FIND", &[txt("x")], CellError::Value);
}

// ============================ SEARCH (case-insensitive, wildcard) ============================

#[test]
fn sheet_fn_text_search_basic() {
    // Case-insensitive.
    want_num("SEARCH", &[txt("L"), txt("hello")], 3.0);
    want_num("SEARCH", &[txt("WORLD"), txt("hello world")], 7.0);
    // start_num.
    want_num("SEARCH", &[txt("l"), txt("hello"), num(4.0)], 4.0);
}

#[test]
fn sheet_fn_text_search_wildcards() {
    // `?` matches one char, `*` matches a run.
    want_num("SEARCH", &[txt("h?llo"), txt("xhello")], 2.0);
    want_num("SEARCH", &[txt("e*o"), txt("hello")], 2.0);
    // `~` escapes a wildcard into a literal.
    want_num("SEARCH", &[txt("~*"), txt("a*b")], 2.0);
}

#[test]
fn sheet_fn_text_search_not_found_and_error() {
    want_err("SEARCH", &[txt("z"), txt("hello")], CellError::Value);
    want_err("SEARCH", &[txt("h"), txt("hi"), num(9.0)], CellError::Value);
    want_err("SEARCH", &[err(CellError::Div0), txt("x")], CellError::Div0);
    // Arity: at least 2 args.
    want_err("SEARCH", &[txt("x")], CellError::Value);
}

// ============================ SUBSTITUTE ============================

#[test]
fn sheet_fn_text_substitute_basic() {
    // All occurrences when instance_num absent.
    want_text("SUBSTITUTE", &[txt("a-b-c"), txt("-"), txt("_")], "a_b_c");
    // new_text longer than old.
    want_text("SUBSTITUTE", &[txt("cat"), txt("c"), txt("ch")], "chat");
}

#[test]
fn sheet_fn_text_substitute_instance_num() {
    // Only the k-th (1-based) occurrence replaced.
    want_text(
        "SUBSTITUTE",
        &[txt("a-b-c-d"), txt("-"), txt("_"), num(2.0)],
        "a-b_c-d",
    );
    // instance past the count -> unchanged.
    want_text(
        "SUBSTITUTE",
        &[txt("a-b"), txt("-"), txt("_"), num(5.0)],
        "a-b",
    );
}

#[test]
fn sheet_fn_text_substitute_edge_and_error() {
    // Empty old_text -> unchanged (no infinite match).
    want_text("SUBSTITUTE", &[txt("abc"), txt(""), txt("X")], "abc");
    // Case-sensitive: "A" not replaced in "abc".
    want_text("SUBSTITUTE", &[txt("abc"), txt("A"), txt("X")], "abc");
    // instance_num < 1 -> #VALUE!.
    want_err(
        "SUBSTITUTE",
        &[txt("a-b"), txt("-"), txt("_"), num(0.0)],
        CellError::Value,
    );
    // Error arg propagates.
    want_err(
        "SUBSTITUTE",
        &[err(CellError::Ref), txt("-"), txt("_")],
        CellError::Ref,
    );
    // Arity: at least 3 args.
    want_err("SUBSTITUTE", &[txt("a"), txt("b")], CellError::Value);
}

// ============================ REPT ============================

#[test]
fn sheet_fn_text_rept_basic() {
    want_text("REPT", &[txt("ab"), num(3.0)], "ababab");
    want_text("REPT", &[txt("x"), num(0.0)], ""); // zero -> empty
    want_text("REPT", &[txt("-"), num(5.0)], "-----");
}

#[test]
fn sheet_fn_text_rept_negative_and_error() {
    // Negative count -> #VALUE!.
    want_err("REPT", &[txt("x"), num(-1.0)], CellError::Value);
    want_err("REPT", &[err(CellError::Na), num(2.0)], CellError::Na);
    // Arity: exactly 2 args.
    want_err("REPT", &[txt("x")], CellError::Value);
}

// ============================ VALUE ============================

#[test]
fn sheet_fn_text_value_basic() {
    want_num("VALUE", &[txt("123")], 123.0);
    want_num("VALUE", &[txt("  2.54  ")], 2.54);
    want_num("VALUE", &[txt("1e3")], 1000.0);
    // A number passes through.
    want_num("VALUE", &[num(7.0)], 7.0);
}

#[test]
fn sheet_fn_text_value_percent_suffix() {
    // Documented extension over plain coercion: a trailing `%` -> /100.
    want_num("VALUE", &[txt("50%")], 0.5);
    want_num("VALUE", &[txt("100%")], 1.0);
    want_num("VALUE", &[txt(" 12.5% ")], 0.125);
}

#[test]
fn sheet_fn_text_value_error_and_arity() {
    // Un-parseable text -> #VALUE!.
    want_err("VALUE", &[txt("abc")], CellError::Value);
    want_err("VALUE", &[txt("")], CellError::Value);
    // Error arg propagates.
    want_err("VALUE", &[err(CellError::Div0)], CellError::Div0);
    // Arity: exactly 1 arg.
    want_err("VALUE", &[txt("1"), txt("2")], CellError::Value);
}
