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

//! In-crate unit + property tests for the parser (spec §6.1). The
//! cross-crate conformance suite (`sheet-conformance/tests/parser_roundtrip`)
//! re-checks the registry-linked golden set.

use super::*;
use sheet_core::ast::{BinOp, Expr, LitValue, UnOp};
use sheet_core::{CellError, CellValue};

/// A tiny test context: a fixed sheet list + name list. `current_sheet` is
/// configurable. `_NAME<n>` resolves to `NameId(n)` so the printer's T0 name
/// placeholder round-trips.
struct Ctx {
    sheets: Vec<&'static str>,
    names: Vec<&'static str>,
    current: SheetId,
}

impl Ctx {
    fn new() -> Self {
        Ctx {
            sheets: vec!["Sheet1", "Sheet 2", "O'Brien", "2nd", "Data"],
            names: vec!["Tax", "Total", "MyRange"],
            current: 0,
        }
    }
}

impl ParseCtx for Ctx {
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
        self.current
    }
}

impl SheetNames for Ctx {
    fn sheet_name(&self, id: SheetId) -> Option<&str> {
        self.sheets.get(id as usize).copied()
    }
}

fn p(s: &str) -> Formula {
    parse(s, &Ctx::new()).unwrap_or_else(|e| panic!("parse {s:?} failed: {e}"))
}

fn pr(f: &Formula) -> String {
    print(f, 0, &Ctx::new())
}

fn roundtrip(s: &str) -> String {
    pr(&p(s))
}

// ---- Lexer / literals ----

#[test]
fn lex_numbers_scientific() {
    assert_eq!(roundtrip("1.5"), "1.5");
    assert_eq!(roundtrip("1.5E-3"), "0.0015");
    assert_eq!(roundtrip("100"), "100");
    assert_eq!(roundtrip(".5"), "0.5");
    assert_eq!(roundtrip("2e3"), "2000");
}

#[test]
fn lex_strings_escapes() {
    let f = p("\"a\"\"b\"");
    assert_eq!(f.root, Expr::Lit(LitValue::Text("a\"b".into())));
    assert_eq!(roundtrip("\"a\"\"b\""), "\"a\"\"b\"");
    assert_eq!(roundtrip("\"\""), "\"\"");
}

#[test]
fn lex_bool_vs_function() {
    assert_eq!(p("TRUE").root, Expr::Lit(LitValue::Bool(true)));
    assert_eq!(p("false").root, Expr::Lit(LitValue::Bool(false)));
    // TRUE() is the zero-arg function, not the literal.
    let f = p("TRUE()");
    assert!(matches!(f.root, Expr::Func(_, ref a) if a.is_empty()));
}

#[test]
fn lex_cell_vs_function() {
    // LOG10 is the one registered name that is also a valid A1 address
    // (cols L-O-G, row 10). Followed by `(` it is the CALL (Excel's own
    // disambiguation); bare, it stays the cell reference.
    let call = p("LOG10(100)");
    let log10 = sheet_core::funcs::lookup_func("LOG10").unwrap();
    assert!(matches!(call.root, Expr::Func(id, ref a) if id == log10 && a.len() == 1));
    assert_eq!(roundtrip("LOG10(100)"), "LOG10(100)");
    let bare = p("LOG10");
    let log_col = sheet_core::a1_to_col("LOG").unwrap();
    assert!(
        matches!(bare.root, Expr::Ref(r) if r.row == 9 && r.col == log_col),
        "bare LOG10 must stay the A1 ref"
    );
    assert_eq!(roundtrip("LOG10"), "LOG10");
}

#[test]
fn lex_error_literals() {
    for tok in [
        "#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#N/A", "#NULL!", "#SPILL!",
    ] {
        let f = p(tok);
        let want = CellError::parse(tok).unwrap();
        assert_eq!(f.root, Expr::Lit(LitValue::Error(want)), "tok {tok}");
        assert_eq!(roundtrip(tok), tok);
    }
}

// ---- References ----

#[test]
fn refs_a1_flags() {
    for s in ["A1", "$A$1", "$A1", "A$1", "XFD1048576"] {
        assert_eq!(roundtrip(s), s, "ref {s}");
    }
}

#[test]
fn refs_range_folds() {
    let f = p("A1:B2");
    assert!(matches!(f.root, Expr::Range(_)));
    assert_eq!(roundtrip("A1:B2"), "A1:B2");
    assert_eq!(roundtrip("$A$1:$B$2"), "$A$1:$B$2");
}

#[test]
fn refs_sheet_qualified() {
    // Home sheet (Sheet1 == id 0) prints WITHOUT a prefix.
    assert_eq!(roundtrip("Sheet1!A1"), "A1");
    // Other sheet keeps its prefix; quoted when it needs quoting.
    assert_eq!(roundtrip("Data!A1"), "Data!A1");
    assert_eq!(roundtrip("'Sheet 2'!A1"), "'Sheet 2'!A1");
    assert_eq!(roundtrip("'O''Brien'!A1"), "'O''Brien'!A1");
    assert_eq!(roundtrip("'2nd'!A1"), "'2nd'!A1");
    // Range qualifier applies to the whole range.
    assert_eq!(roundtrip("Data!A1:B2"), "Data!A1:B2");
}

// ---- Operators / precedence ----

#[test]
fn ops_precedence_basic() {
    assert_eq!(roundtrip("1+2*3"), "1+2*3");
    assert_eq!(roundtrip("(1+2)*3"), "(1+2)*3");
    assert_eq!(roundtrip("1*2+3"), "1*2+3");
    assert_eq!(roundtrip("1&2=3"), "1&2=3");
    assert_eq!(roundtrip("1=2&3"), "1=2&3");
}

#[test]
fn ops_unary_minus_tighter_than_pow() {
    // Excel quirk: -2^2 == (-2)^2 == 4 (NOT -(2^2)).
    let f = p("-2^2");
    assert_eq!(
        f.root,
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
    assert_eq!(roundtrip("-2^2"), "-2^2");
}

#[test]
fn ops_pow_left_assoc() {
    // 2^3^2 == (2^3)^2 in Excel (left-assoc), printed minimally.
    let f = p("2^3^2");
    let Expr::Binary(BinOp::Pow, lhs, _) = &f.root else {
        panic!("not a pow at root: {:?}", f.root)
    };
    assert!(matches!(**lhs, Expr::Binary(BinOp::Pow, _, _)));
    assert_eq!(roundtrip("2^3^2"), "2^3^2");
    // The right-assoc shape must round-trip with explicit parens.
    assert_eq!(roundtrip("2^(3^2)"), "2^(3^2)");
}

#[test]
fn ops_percent_postfix() {
    assert_eq!(roundtrip("50%"), "50%");
    assert_eq!(roundtrip("A1%"), "A1%");
    assert_eq!(roundtrip("-50%"), "-50%");
    assert_eq!(roundtrip("1+50%"), "1+50%");
}

#[test]
fn ops_minimal_parens_subtraction() {
    assert_eq!(roundtrip("1-(2-3)"), "1-(2-3)");
    assert_eq!(roundtrip("(1-2)-3"), "1-2-3");
    assert_eq!(roundtrip("1-2-3"), "1-2-3");
    assert_eq!(roundtrip("10/(2/5)"), "10/(2/5)");
    assert_eq!(roundtrip("10/2/5"), "10/2/5");
}

// ---- Functions ----

#[test]
fn func_calls_and_nesting() {
    assert_eq!(roundtrip("SUM(A1:A10)"), "SUM(A1:A10)");
    assert_eq!(roundtrip("sum(a1,b2)"), "SUM(A1,B2)");
    assert_eq!(roundtrip("IF(A1>0,SUM(B1:B2),0)"), "IF(A1>0,SUM(B1:B2),0)");
    assert_eq!(roundtrip("POWER(2,10)"), "POWER(2,10)");
    assert_eq!(roundtrip("PI()"), "PI()");
}

#[test]
fn func_unknown_is_parse_error() {
    let err = parse("NOTAFUNC(1)", &Ctx::new()).unwrap_err();
    assert!(err.message.contains("unknown function"), "{}", err.message);
}

#[test]
fn name_unknown_is_parse_error() {
    let err = parse("Bogus", &Ctx::new()).unwrap_err();
    assert!(err.message.contains("unknown name"), "{}", err.message);
}

// ---- Union / intersection ----

#[test]
fn union_only_inside_parens() {
    // Union inside the SUM arg's inner parens.
    let f = p("SUM((A1,B2:B3))");
    let Expr::Func(_, args) = &f.root else {
        panic!()
    };
    assert_eq!(args.len(), 1, "one arg (a union), not two");
    assert!(matches!(args[0], Expr::Binary(BinOp::Union, _, _)));
    assert_eq!(roundtrip("SUM((A1,B2:B3))"), "SUM((A1,B2:B3))");
    // A bare top-level `,` is an arg separator, so SUM(A1,B2) is two args.
    let g = p("SUM(A1,B2)");
    let Expr::Func(_, gargs) = &g.root else {
        panic!()
    };
    assert_eq!(gargs.len(), 2);
}

#[test]
fn intersection_space() {
    let f = p("A1:A10 B1:D1");
    assert!(matches!(f.root, Expr::Binary(BinOp::Isect, _, _)));
    assert_eq!(roundtrip("A1:A10 B1:D1"), "A1:A10 B1:D1");
}

// ---- Array literals ----

#[test]
fn array_literals() {
    let f = p("{1,2;3,4}");
    let Expr::Array(rows) = &f.root else { panic!() };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].len(), 2);
    assert_eq!(roundtrip("{1,2;3,4}"), "{1,2;3,4}");
    assert_eq!(roundtrip("{-1,2;3,-4}"), "{-1,2;3,-4}");
    assert_eq!(roundtrip("{\"a\",\"b\"}"), "{\"a\",\"b\"}");
}

#[test]
fn array_non_rectangular_rejected() {
    assert!(parse("{1,2;3}", &Ctx::new()).is_err());
    assert!(parse("{}", &Ctx::new()).is_err());
}

// ---- Structured (table) references (spec §6.4) ----

#[test]
fn structured_ref_forms_parse_and_print() {
    use sheet_core::ast::{StructuredRef, TableArea};
    // Bare column, default Data area.
    let f = p("Table1[Region]");
    assert_eq!(
        f.root,
        Expr::StructuredRef(StructuredRef {
            table: "Table1".into(),
            area: TableArea::Data,
            col_start: Some("Region".into()),
            col_end: None,
        })
    );
    // Every canonical form is a parse-then-print fixpoint (the string is
    // already in the printer's canonical spelling, so it survives verbatim).
    for s in [
        "Table1[Region]",
        "Table1[[#All]]",
        "Table1[[#Headers]]",
        "Table1[[#Totals]]",
        "Table1[[#Headers],[Region]]",
        "Table1[[Region]:[Total]]",
        "Table1[[#This Row],[Region]]",
        "[@Region]",
    ] {
        assert_eq!(roundtrip(s), s, "structured-ref fixpoint for {s}");
    }

    // Non-canonical (but valid) spellings normalize to the canonical form, and
    // the canonical form is then a fixpoint. The `#Data` area is the DEFAULT,
    // so `Table1[[#Data],[Units]]` prints minimally as `Table1[Units]` (Excel's
    // own collapse); re-parsing yields the same AST (an AST-level fixpoint).
    for (input, canonical) in [
        ("Table1[[#Data],[Units]]", "Table1[Units]"),
        (
            "Table1[[#Data],[Region]:[Total]]",
            "Table1[[Region]:[Total]]",
        ),
        ("Table1[[Region]]", "Table1[Region]"),
        ("Table1[]", "Table1[]"),
    ] {
        let once = roundtrip(input);
        assert_eq!(once, canonical, "canonical print for {input}");
        // AST-level fixpoint: parse(print(parse(input))) == parse(input).
        assert_eq!(p(input).root, p(&once).root, "AST fixpoint for {input}");
        // And the canonical form is a string fixpoint.
        assert_eq!(roundtrip(canonical), canonical, "fixpoint for {canonical}");
    }
}

#[test]
fn structured_ref_column_with_spaces_and_escapes() {
    use sheet_core::ast::{StructuredRef, TableArea};
    // A column name with spaces survives (it must be bracketed).
    let f = p("Sales[[Net Amount]]");
    assert_eq!(
        f.root,
        Expr::StructuredRef(StructuredRef {
            table: "Sales".into(),
            area: TableArea::Data,
            col_start: Some("Net Amount".into()),
            col_end: None,
        })
    );
    // The printer round-trips a spaced single column as the bracketed `[[Col]]`
    // form (a simple `[Col]` would still re-parse, but Excel's canonical form
    // brackets a column that contains a space or special character).
    assert_eq!(roundtrip("Sales[[Net Amount]]"), "Sales[[Net Amount]]");
    // The simple-form input with a space normalizes to the bracketed form, and
    // the two are the SAME AST (an AST-level fixpoint).
    assert_eq!(roundtrip("Sales[Net Amount]"), "Sales[[Net Amount]]");
    assert_eq!(p("Sales[Net Amount]").root, p("Sales[[Net Amount]]").root);
    // An escaped special character (`'#` → literal `#`) un-escapes on parse and
    // re-escapes on print (the `#`-leading column needs bracket+escape).
    let g = p("T[['#Hashed]]");
    let Expr::StructuredRef(s) = g.root else {
        panic!("not a structured ref")
    };
    assert_eq!(s.col_start.as_deref(), Some("#Hashed"));
    assert_eq!(roundtrip("T[['#Hashed]]"), "T[['#Hashed]]");
}

#[test]
fn structured_ref_thisrow_with_table_name() {
    use sheet_core::ast::TableArea;
    // `[@Col]` is the bare ThisRow; `Table1[[#This Row],[Col]]` keeps its name.
    let f = p("[@Units]");
    let Expr::StructuredRef(s) = f.root else {
        panic!()
    };
    assert_eq!(s.area, TableArea::ThisRow);
    assert!(s.table.is_empty());
    assert_eq!(s.col_start.as_deref(), Some("Units"));

    let g = p("Tbl[[#This Row],[Units]]");
    let Expr::StructuredRef(s2) = g.root else {
        panic!()
    };
    assert_eq!(s2.area, TableArea::ThisRow);
    assert_eq!(s2.table.as_str(), "Tbl");
    assert_eq!(s2.col_start.as_deref(), Some("Units"));
}

#[test]
fn structured_ref_in_function_arg() {
    // A structured ref is an operand: it nests inside SUM(...).
    let f = p("SUM(Table1[Amount])");
    let Expr::Func(_, args) = &f.root else {
        panic!("expected a SUM call")
    };
    assert_eq!(args.len(), 1);
    assert!(matches!(args[0], Expr::StructuredRef(_)));
    assert_eq!(roundtrip("SUM(Table1[Amount])"), "SUM(Table1[Amount])");
    // And in an arithmetic context.
    assert_eq!(roundtrip("[@Price]*[@Qty]"), "[@Price]*[@Qty]");
}

#[test]
fn structured_ref_extract_records_table() {
    // extract_refs records the table name (the dep edge resolves it in calc).
    let set = extract_refs(&p("SUM(Sales[Amount])"));
    assert_eq!(set.tables.len(), 1);
    assert_eq!(set.tables[0].as_str(), "Sales");
    assert!(!set.has_self_table_ref, "named ref is not a self-table ref");
    // The bare ThisRow form (empty table) records NO table NAME but DOES flag a
    // self-table dependency (the graph resolves the formula's own table).
    let set2 = extract_refs(&p("[@Amount]"));
    assert!(set2.tables.is_empty());
    assert!(
        set2.has_self_table_ref,
        "bare [@Col] flags a self-table ref"
    );
    // The named `[[#This Row],[Col]]` form records its NAME, not the self flag.
    let set3 = extract_refs(&p("Tbl[[#This Row],[Amount]]"));
    assert_eq!(set3.tables.len(), 1);
    assert!(!set3.has_self_table_ref);
}

#[test]
fn structured_ref_rewrite_passthrough() {
    // Name-anchored: an insert/delete leaves the structured ref unchanged.
    let f = p("SUM(Table1[Amount])");
    let g = rewrite(
        &f,
        &Edit::InsertRows {
            sheet: 0,
            at: 0,
            n: 5,
        },
    );
    assert_eq!(f.root, g.root, "structured ref must survive a row insert");
}

#[test]
fn structured_ref_malformed_is_error() {
    // Unterminated bracket.
    assert!(parse("Table1[Region", &Ctx::new()).is_err());
    // Two columns without a span.
    assert!(parse("Table1[[A],[B],[C]]", &Ctx::new()).is_err());
}

// ---- Names ----

#[test]
fn names_resolve() {
    let f = p("Tax");
    assert_eq!(f.root, Expr::Name(NameId(0)));
    assert_eq!(roundtrip("Tax*100"), "_NAME0*100");
    // The placeholder re-parses to the same id (fixpoint).
    assert_eq!(roundtrip("_NAME0*100"), "_NAME0*100");
}

// ---- extract_refs ----

#[test]
fn extract_cells_ranges_names() {
    let set = extract_refs(&p("SUM(A1,B2:C3)+Tax"));
    assert_eq!(set.cells.len(), 1);
    assert_eq!(set.ranges.len(), 1);
    assert_eq!(set.names, vec![NameId(0)]);
    assert!(!set.has_volatile);
}

#[test]
fn extract_volatile() {
    assert!(extract_refs(&p("NOW()")).has_volatile);
    assert!(extract_refs(&p("RAND()+A1")).has_volatile);
    assert!(!extract_refs(&p("SUM(A1:A2)")).has_volatile);
}

// ---- rewrite ----

fn cell(s: &str) -> sheet_core::CellRef {
    match p(s).root {
        Expr::Ref(r) => r,
        other => panic!("not a cell ref: {other:?}"),
    }
}

#[test]
fn rewrite_insert_rows_shifts_below() {
    // Insert 2 rows at row index 4 (the 5th row): A10 (row 9) -> A12.
    let f = p("A10");
    let g = rewrite(
        &f,
        &Edit::InsertRows {
            sheet: 0,
            at: 4,
            n: 2,
        },
    );
    assert_eq!(g.root, Expr::Ref(cell("A12")));
    // A1 (row 0) is above the insert: unchanged.
    let h = rewrite(
        &p("A1"),
        &Edit::InsertRows {
            sheet: 0,
            at: 4,
            n: 2,
        },
    );
    assert_eq!(h.root, Expr::Ref(cell("A1")));
}

#[test]
fn rewrite_insert_ignores_other_sheet() {
    let f = p("Data!A10");
    let g = rewrite(
        &f,
        &Edit::InsertRows {
            sheet: 0,
            at: 0,
            n: 5,
        },
    );
    // Edit is on sheet 0; the ref is on Data (sheet 4): unchanged.
    assert_eq!(pr(&g), "Data!A10");
}

#[test]
fn rewrite_absolute_still_shifts() {
    // $A$10 still moves on insert (the $ does NOT exempt it).
    let f = p("$A$10");
    let g = rewrite(
        &f,
        &Edit::InsertRows {
            sheet: 0,
            at: 0,
            n: 1,
        },
    );
    assert_eq!(pr(&g), "$A$11");
}

#[test]
fn rewrite_delete_inside_span_is_ref_error() {
    // Delete rows [4,6): A5 (row 4) is inside -> #REF!.
    let f = p("A5");
    let g = rewrite(
        &f,
        &Edit::DeleteRows {
            sheet: 0,
            at: 4,
            n: 2,
        },
    );
    assert_eq!(g.root, Expr::Lit(LitValue::Error(CellError::Ref)));
}

#[test]
fn rewrite_delete_shifts_after() {
    // Delete rows [0,2): A10 (row 9) -> A8 (row 7).
    let f = p("A10");
    let g = rewrite(
        &f,
        &Edit::DeleteRows {
            sheet: 0,
            at: 0,
            n: 2,
        },
    );
    assert_eq!(g.root, Expr::Ref(cell("A8")));
}

#[test]
fn rewrite_delete_range_fully_inside() {
    // Range A5:A6 (rows 4..5) fully inside delete [4,7): -> #REF!.
    let f = p("A5:A6");
    let g = rewrite(
        &f,
        &Edit::DeleteRows {
            sheet: 0,
            at: 4,
            n: 3,
        },
    );
    assert_eq!(g.root, Expr::Lit(LitValue::Error(CellError::Ref)));
}

#[test]
fn rewrite_delete_range_partial_clips() {
    // Range A3:A10 (rows 2..9), delete rows [4,6) (rows 4,5). Start (row 2)
    // is before the span -> unchanged; end (row 9) is after -> shift -2 = 7.
    let f = p("A3:A10");
    let g = rewrite(
        &f,
        &Edit::DeleteRows {
            sheet: 0,
            at: 4,
            n: 2,
        },
    );
    assert_eq!(
        g.root,
        Expr::Range(match p("A3:A8").root {
            Expr::Range(r) => r,
            _ => unreachable!(),
        })
    );
}

#[test]
fn rewrite_insert_cols_shifts_right() {
    // Insert 1 col at col 0: B1 (col 1) -> C1.
    let f = p("B1");
    let g = rewrite(
        &f,
        &Edit::InsertCols {
            sheet: 0,
            at: 0,
            n: 1,
        },
    );
    assert_eq!(g.root, Expr::Ref(cell("C1")));
}

#[test]
fn rewrite_delete_cols_inside_is_ref_error() {
    let f = p("B1");
    let g = rewrite(
        &f,
        &Edit::DeleteCols {
            sheet: 0,
            at: 1,
            n: 1,
        },
    );
    assert_eq!(g.root, Expr::Lit(LitValue::Error(CellError::Ref)));
}

#[test]
fn rewrite_insert_off_grid_is_ref_error() {
    // Push XFD1 (col MAX_COL) one column right -> off the grid -> #REF!.
    let f = p("XFD1");
    let g = rewrite(
        &f,
        &Edit::InsertCols {
            sheet: 0,
            at: 0,
            n: 1,
        },
    );
    assert_eq!(g.root, Expr::Lit(LitValue::Error(CellError::Ref)));
}

// A sanity check on CellValue so the import is exercised (it ties the
// parser's error literal to the stored value the xlsx side caches).
#[test]
fn ref_error_maps_to_value() {
    assert_eq!(
        CellValue::Error(CellError::Ref),
        CellValue::Error(CellError::parse("#REF!").unwrap())
    );
}

// ---- Property: parse ∘ print is an AST fixpoint ----

mod prop {
    use super::*;
    use proptest::prelude::*;
    use sheet_core::ast::OrderedF64;

    fn arb_cellref() -> impl Strategy<Value = sheet_core::CellRef> {
        (0u32..50, 0u32..50, any::<bool>(), any::<bool>()).prop_map(|(r, c, ra, ca)| {
            sheet_core::CellRef {
                sheet: 0,
                row: r,
                col: c,
                row_abs: ra,
                col_abs: ca,
            }
        })
    }

    // A bounded AST generator covering the whole T0 grammar except names
    // (their textual placeholder is exercised by the `names_resolve` unit
    // test). Refs use the home sheet (0) so they print without a prefix.
    fn arb_expr() -> impl Strategy<Value = Expr> {
        let leaf = prop_oneof![
            arb_cellref().prop_map(Expr::Ref),
            // A normalized range (start <= end) so print and a re-parse fold
            // it back to an identical Range node.
            (arb_cellref(), arb_cellref())
                .prop_map(|(a, b)| { Expr::Range(crate::refs::range(a, b).normalized()) }),
            (0i32..1000).prop_map(|n| Expr::Lit(LitValue::Number(OrderedF64::new(n as f64)))),
            any::<bool>().prop_map(|b| Expr::Lit(LitValue::Bool(b))),
            "[a-z ]{0,6}".prop_map(|s| Expr::Lit(LitValue::Text(s.into()))),
        ];
        leaf.prop_recursive(5, 96, 4, |inner| {
            prop_oneof![
                (bin_op(), inner.clone(), inner.clone()).prop_map(|(op, a, b)| Expr::Binary(
                    op,
                    Box::new(a),
                    Box::new(b)
                )),
                // Intersection of two operands (space operator).
                (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::Binary(
                    BinOp::Isect,
                    Box::new(a),
                    Box::new(b)
                )),
                inner
                    .clone()
                    .prop_map(|a| Expr::Unary(UnOp::Neg, Box::new(a))),
                inner
                    .clone()
                    .prop_map(|a| Expr::Unary(UnOp::Plus, Box::new(a))),
                inner
                    .clone()
                    .prop_map(|a| Expr::Unary(UnOp::Percent, Box::new(a))),
                // A function call (SUM is variadic, 1..=3 args here).
                proptest::collection::vec(inner.clone(), 1..=3).prop_map(|args| {
                    Expr::Func(sheet_core::funcs::lookup_func("SUM").unwrap(), args)
                }),
            ]
        })
    }

    fn bin_op() -> impl Strategy<Value = BinOp> {
        prop_oneof![
            Just(BinOp::Add),
            Just(BinOp::Sub),
            Just(BinOp::Mul),
            Just(BinOp::Div),
            Just(BinOp::Pow),
            Just(BinOp::Concat),
            Just(BinOp::Eq),
            Just(BinOp::Lt),
            Just(BinOp::Ge),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]
        #[test]
        fn parse_print_is_fixpoint(e in arb_expr()) {
            let f = Formula { root: e };
            let printed = print(&f, 0, &Ctx::new());
            let reparsed = parse(&printed, &Ctx::new())
                .unwrap_or_else(|err| panic!("re-parse {printed:?} failed: {err}"));
            // The strong property: parse ∘ print recovers the SAME AST.
            prop_assert_eq!(&reparsed.root, &f.root, "printed: {}", printed);
            // ...and print is idempotent on that canonical form.
            let reprinted = print(&reparsed, 0, &Ctx::new());
            prop_assert_eq!(printed, reprinted);
        }
    }

    use sheet_core::ast::{StructuredRef, TableArea};

    /// A column name that exercises both the simple and the bracket-escaped
    /// printer paths: plain names, spaced names, and names with the escapable
    /// specials (`[ ] # ' @`). Empty is excluded (a column is always present
    /// here; the no-column forms are covered by the goldens).
    fn arb_col() -> impl Strategy<Value = compact_str::CompactString> {
        prop_oneof![
            "[A-Za-z][A-Za-z0-9]{0,5}",
            "[A-Za-z][A-Za-z0-9]{0,3} [A-Za-z0-9]{1,3}",
            r"[A-Za-z]['#@:,\[\] ]?[A-Za-z]",
        ]
        .prop_map(compact_str::CompactString::new)
    }

    fn arb_table() -> impl Strategy<Value = compact_str::CompactString> {
        "[A-Za-z_][A-Za-z0-9_]{0,6}".prop_map(compact_str::CompactString::new)
    }

    fn arb_area() -> impl Strategy<Value = TableArea> {
        prop_oneof![
            Just(TableArea::Data),
            Just(TableArea::All),
            Just(TableArea::Headers),
            Just(TableArea::Totals),
            Just(TableArea::ThisRow),
        ]
    }

    /// Generate the structured-ref shapes the printer canonicalizes:
    /// (table, area, single-column) and (table, area, column-span). The bare
    /// `[@Col]` shorthand (empty table + ThisRow) is generated separately.
    fn arb_structured() -> impl Strategy<Value = StructuredRef> {
        prop_oneof![
            // Named table, one column.
            (arb_table(), arb_area(), arb_col()).prop_map(|(t, a, c)| StructuredRef {
                table: t,
                area: a,
                col_start: Some(c),
                col_end: None,
            }),
            // Named table, column span.
            (arb_table(), arb_area(), arb_col(), arb_col()).prop_map(|(t, a, c0, c1)| {
                StructuredRef {
                    table: t,
                    area: a,
                    col_start: Some(c0),
                    col_end: Some(c1),
                }
            }),
            // The bare ThisRow `[@Col]` shorthand (empty table name).
            arb_col().prop_map(|c| StructuredRef {
                table: compact_str::CompactString::default(),
                area: TableArea::ThisRow,
                col_start: Some(c),
                col_end: None,
            }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(3000))]
        #[test]
        fn structured_ref_parse_print_is_fixpoint(s in arb_structured()) {
            let f = Formula { root: Expr::StructuredRef(s) };
            let printed = print(&f, 0, &Ctx::new());
            let reparsed = parse(&printed, &Ctx::new())
                .unwrap_or_else(|err| panic!("re-parse {printed:?} failed: {err}"));
            // AST-level fixpoint: the printer's canonical text re-parses to the
            // SAME structured reference.
            prop_assert_eq!(&reparsed.root, &f.root, "printed: {}", printed);
            // ...and print is idempotent on that canonical form.
            let reprinted = print(&reparsed, 0, &Ctx::new());
            prop_assert_eq!(printed, reprinted);
        }
    }
}
