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

//! Registry-linked conformance for `sheet-parser` (spec §6.1). Each test fn
//! is named exactly per the `tests.rust` pointer in
//! `registry/features/parser.yaml` (the coverage gate greps these prefixes).
//! Self-contained: no helpers imported from the conformance lib — only the
//! public `sheet-parser` API + an in-file `FakeCtx`.

use sheet_parser::{extract_refs, parse, print, rewrite, Edit, ParseCtx, SheetNames};

use sheet_core::ast::{BinOp, Expr, LitValue, NameId, UnOp};
use sheet_core::{CellError, CellRef, SheetId};

/// A minimal resolution context: a fixed sheet list + a fixed name list, home
/// sheet 0. `_NAME<n>` resolves to `NameId(n)` so the printer's T0 name
/// placeholder round-trips (the real spelling round-trip is T1).
struct FakeCtx {
    sheets: &'static [&'static str],
    names: &'static [&'static str],
}

impl FakeCtx {
    fn new() -> Self {
        FakeCtx {
            sheets: &["Sheet1", "Sheet 2", "O'Brien", "2nd", "Data", "Q1 2024"],
            names: &["Tax", "Revenue", "Sheet_Total"],
        }
    }
}

impl ParseCtx for FakeCtx {
    fn sheet_id(&self, name: &str) -> Option<SheetId> {
        self.sheets
            .iter()
            .position(|s| *s == name)
            .map(|i| i as u16)
    }
    fn name_id(&self, name: &str) -> Option<NameId> {
        if let Some(rest) = name.strip_prefix("_NAME") {
            return rest.parse::<u32>().ok().map(NameId);
        }
        self.names
            .iter()
            .position(|s| *s == name)
            .map(|i| NameId(i as u32))
    }
    fn current_sheet(&self) -> SheetId {
        0
    }
}

impl SheetNames for FakeCtx {
    fn sheet_name(&self, id: SheetId) -> Option<&str> {
        self.sheets.get(id as usize).copied()
    }
}

/// Parse `s`, asserting success.
fn parse_ok(s: &str) -> Expr {
    parse(s, &FakeCtx::new())
        .unwrap_or_else(|e| panic!("parse {s:?} failed: {e}"))
        .root
}

/// `parse ∘ print` round-trips to identical canonical text (an AST fixpoint
/// witnessed at the text level; the in-crate proptest proves the AST level).
fn assert_canonical(input: &str, canonical: &str) {
    let ctx = FakeCtx::new();
    let f = parse(input, &ctx).unwrap_or_else(|e| panic!("parse {input:?} failed: {e}"));
    let printed = print(&f, 0, &ctx);
    assert_eq!(printed, canonical, "canonical text for {input:?}");
    // Re-parse the canonical form and re-print: must be stable.
    let f2 = parse(&printed, &ctx).unwrap_or_else(|e| panic!("re-parse {printed:?} failed: {e}"));
    assert_eq!(print(&f2, 0, &ctx), canonical, "fixpoint for {input:?}");
}

// ---- sheet.parser.literals ----

#[test]
fn sheet_parser_literals() {
    // Numbers incl. scientific notation.
    assert_canonical("1.5", "1.5");
    assert_canonical("1.5E-3", "0.0015");
    assert_canonical("2e3", "2000");
    assert_canonical(".25", "0.25");
    assert_canonical("0", "0");
    // Strings with doubled-quote escapes.
    assert_canonical("\"hi\"", "\"hi\"");
    assert_canonical("\"a\"\"b\"", "\"a\"\"b\"");
    assert_eq!(
        parse_ok("\"x\"\"y\""),
        Expr::Lit(LitValue::Text("x\"y".into()))
    );
    // Booleans.
    assert_canonical("TRUE", "TRUE");
    assert_canonical("false", "FALSE");
    // All eight error codes.
    for tok in [
        "#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#N/A", "#NULL!", "#SPILL!",
    ] {
        assert_canonical(tok, tok);
        assert_eq!(
            parse_ok(tok),
            Expr::Lit(LitValue::Error(CellError::parse(tok).unwrap()))
        );
    }
}

// ---- sheet.parser.refs.a1 ----

#[test]
fn sheet_parser_refs_a1() {
    // Plain + absolute-flag combinations.
    for s in ["A1", "$A$1", "$A1", "A$1", "Z99", "XFD1048576"] {
        assert_canonical(s, s);
    }
    // Ranges fold to a single Range node.
    assert!(matches!(parse_ok("A1:B2"), Expr::Range(_)));
    assert_canonical("A1:B2", "A1:B2");
    assert_canonical("$A$1:$C$3", "$A$1:$C$3");
    // Sheet-qualified: home sheet drops the prefix; others keep it.
    assert_canonical("Sheet1!A1", "A1");
    assert_canonical("Data!A1", "Data!A1");
    assert_canonical("Data!A1:B2", "Data!A1:B2");
    // Quoted sheet names (space, apostrophe, leading digit).
    assert_canonical("'Sheet 2'!A1", "'Sheet 2'!A1");
    assert_canonical("'O''Brien'!B2", "'O''Brien'!B2");
    assert_canonical("'2nd'!C3", "'2nd'!C3");
    assert_canonical("'Q1 2024'!A1:B2", "'Q1 2024'!A1:B2");
}

// ---- sheet.parser.operators ----

#[test]
fn sheet_parser_operators() {
    // Precedence: * over +, comparison loosest, & between.
    assert_canonical("1+2*3", "1+2*3");
    assert_canonical("(1+2)*3", "(1+2)*3");
    assert_canonical("1&2=3", "1&2=3");
    assert_canonical("1<=2", "1<=2");
    assert_canonical("1<>2", "1<>2");
    assert_canonical("1>=2", "1>=2");
    // Unary minus binds TIGHTER than ^ (Excel quirk: -2^2 == (-2)^2).
    assert_eq!(
        parse_ok("-2^2"),
        Expr::Binary(
            BinOp::Pow,
            Box::new(Expr::Unary(
                UnOp::Neg,
                Box::new(Expr::Lit(LitValue::Number(
                    sheet_core::ast::OrderedF64::new(2.0)
                )))
            )),
            Box::new(Expr::Lit(LitValue::Number(
                sheet_core::ast::OrderedF64::new(2.0)
            )))
        )
    );
    assert_canonical("-2^2", "-2^2");
    // ^ left-associative: 2^3^2 == (2^3)^2.
    let Expr::Binary(BinOp::Pow, lhs, _) = parse_ok("2^3^2") else {
        panic!("expected pow root")
    };
    assert!(matches!(*lhs, Expr::Binary(BinOp::Pow, _, _)));
    assert_canonical("2^3^2", "2^3^2");
    assert_canonical("2^(3^2)", "2^(3^2)");
    // Postfix percent.
    assert_canonical("50%", "50%");
    assert_canonical("1+50%", "1+50%");
    // Minimal parens (associativity).
    assert_canonical("(1-2)-3", "1-2-3");
    assert_canonical("1-(2-3)", "1-(2-3)");
    assert_canonical("10/(2/5)", "10/(2/5)");
}

// ---- sheet.parser.functions ----

#[test]
fn sheet_parser_functions() {
    assert_canonical("SUM(A1:A10)", "SUM(A1:A10)");
    assert_canonical("sum(a1,b2,c3)", "SUM(A1,B2,C3)");
    assert_canonical("IF(A1>0,SUM(B1:B5),0)", "IF(A1>0,SUM(B1:B5),0)");
    assert_canonical("POWER(2,10)", "POWER(2,10)");
    assert_canonical("PI()", "PI()");
    // TRUE() is the zero-arg function; bare TRUE is the literal.
    assert!(matches!(parse_ok("TRUE()"), Expr::Func(_, a) if a.is_empty()));
    assert_eq!(parse_ok("TRUE"), Expr::Lit(LitValue::Bool(true)));
    // Unknown function name -> parse error (ruling: not a #NAME? literal).
    let err = parse("NOPENOPE(1)", &FakeCtx::new()).unwrap_err();
    assert!(err.message.contains("unknown function"), "{}", err.message);
    // Nested function with an arithmetic argument.
    assert_canonical("ABS(A1-B1)", "ABS(A1-B1)");
}

// ---- sheet.parser.union-intersection ----

#[test]
fn sheet_parser_union_intersection() {
    // Union `,` only inside parentheses (a single SUM argument).
    let Expr::Func(_, args) = parse_ok("SUM((A1,B2:B3))") else {
        panic!("expected SUM call")
    };
    assert_eq!(args.len(), 1, "the inner (,) is a union, one arg");
    assert!(matches!(args[0], Expr::Binary(BinOp::Union, _, _)));
    assert_canonical("SUM((A1,B2:B3))", "SUM((A1,B2:B3))");
    // A bare top-level `,` separates arguments.
    let Expr::Func(_, args2) = parse_ok("SUM(A1,B2:B3)") else {
        panic!()
    };
    assert_eq!(args2.len(), 2);
    // Intersection (space) between two ranges.
    assert!(matches!(
        parse_ok("A1:A10 B1:D1"),
        Expr::Binary(BinOp::Isect, _, _)
    ));
    assert_canonical("A1:A10 B1:D1", "A1:A10 B1:D1");
    // Intersection inside a SUM argument.
    assert_canonical("SUM(A1:A10 A1:D1)", "SUM(A1:A10 A1:D1)");
}

// ---- sheet.parser.array-literals ----

#[test]
fn sheet_parser_array_literals() {
    let Expr::Array(rows) = parse_ok("{1,2;3,4}") else {
        panic!("expected array")
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].len(), 2);
    assert_canonical("{1,2;3,4}", "{1,2;3,4}");
    // Negated number elements.
    assert_canonical("{-1,2;3,-4}", "{-1,2;3,-4}");
    // A single-row array, mixed literal kinds.
    assert_canonical("{\"a\",TRUE,3}", "{\"a\",TRUE,3}");
    // Non-rectangular and empty arrays are rejected.
    assert!(parse("{1,2;3}", &FakeCtx::new()).is_err());
    assert!(parse("{}", &FakeCtx::new()).is_err());
}

// ---- sheet.parser.print.canonical ----

#[test]
fn sheet_parser_print_canonical() {
    // Function names upper-cased; redundant parens dropped; needed parens kept.
    assert_canonical("sum(a1)*2", "SUM(A1)*2");
    assert_canonical("(A1)", "A1");
    assert_canonical("((1+2))", "1+2");
    assert_canonical("1*(2+3)", "1*(2+3)");
    assert_canonical("1+2+3", "1+2+3");
    // f64 literals via shortest round-tripping Display.
    assert_canonical("3.14", "3.14");
    assert_canonical("1000000", "1000000");
    // A ref on a non-home sheet is prefixed; on the home sheet it is not.
    assert_canonical("Sheet1!A1+Data!B2", "A1+Data!B2");
    // print() applied twice is idempotent on a complex formula.
    let ctx = FakeCtx::new();
    let f = parse("IF(Data!A1>0,SUM(B1:B5)*2%,-C1^2)", &ctx).unwrap();
    let once = print(&f, 0, &ctx);
    let twice = print(&parse(&once, &ctx).unwrap(), 0, &ctx);
    assert_eq!(once, twice);
}

// ---- sheet.parser.names ----

#[test]
fn sheet_parser_names() {
    // A defined name resolves to its id.
    assert_eq!(parse_ok("Tax"), Expr::Name(NameId(0)));
    assert_eq!(parse_ok("Revenue"), Expr::Name(NameId(1)));
    // A name used in an expression; extract_refs collects it.
    let f = parse("Revenue*Tax", &FakeCtx::new()).unwrap();
    let set = extract_refs(&f);
    assert_eq!(set.names, vec![NameId(1), NameId(0)]);
    // Unknown name -> parse error.
    let err = parse("Undefined", &FakeCtx::new()).unwrap_err();
    assert!(err.message.contains("unknown name"), "{}", err.message);
    // The T0 name placeholder round-trips through print.
    assert_canonical("_NAME0+_NAME2", "_NAME0+_NAME2");
}

// ---- Extra coverage: extract_refs + rewrite (exercised here so the suite
// witnesses the §6.2/§6.3 API surface alongside the §6.1 rows). ----

#[test]
fn sheet_parser_extract_and_rewrite() {
    // extract_refs: cells, ranges, names, volatility.
    let f = parse("SUM(A1,B2:C3)+Tax+NOW()", &FakeCtx::new()).unwrap();
    let set = extract_refs(&f);
    assert_eq!(set.cells.len(), 1);
    assert_eq!(set.ranges.len(), 1);
    assert_eq!(set.names, vec![NameId(0)]);
    assert!(set.has_volatile, "NOW() is volatile");

    // rewrite: insert rows shifts refs below the insertion point.
    let g = rewrite(
        &parse("A10", &FakeCtx::new()).unwrap(),
        &Edit::InsertRows {
            sheet: 0,
            at: 0,
            n: 2,
        },
    );
    assert_eq!(
        g.root,
        Expr::Ref(CellRef {
            sheet: 0,
            row: 11,
            col: 0,
            row_abs: false,
            col_abs: false
        })
    );

    // rewrite: deleting the row a ref sits on collapses it to #REF!.
    let h = rewrite(
        &parse("A1", &FakeCtx::new()).unwrap(),
        &Edit::DeleteRows {
            sheet: 0,
            at: 0,
            n: 1,
        },
    );
    assert_eq!(h.root, Expr::Lit(LitValue::Error(CellError::Ref)));
}
