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

//! LET / LAMBDA / lambda-helper conformance (Wave 7) and the ISREF special
//! form. These are evaluator special forms (`sheet-calc/src/eval/lambda.rs`)
//! over the Wave-7 AST amendment (`Expr::Local`, `Expr::Call`), so every case
//! runs end to end through the FROZEN [`sheet_calc::Engine`] — parse (name
//! scoping), print round-trip, evaluation, spill. Expected values follow the
//! public Microsoft documentation of each function. Test names carry the
//! `sheet_fn_lambda_<name>` / `sheet_fn_info_isref` prefixes the registry rows
//! point at.

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

fn cell(e: &Engine, r: u32, c: u32) -> CellValue {
    e.model()
        .sheet(0)
        .and_then(|ws| ws.cell(r, c))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

fn calc(setup: &[(&str, &str)], formula: &str) -> CellValue {
    let mut e = engine(setup);
    e.enter(0, 0, 25, formula).unwrap();
    cell(&e, 0, 25)
}

fn spill(setup: &[(&str, &str)], formula: &str, rows: u32, cols: u32) -> Vec<Vec<CellValue>> {
    let mut e = engine(setup);
    e.enter(0, 0, 9, formula).unwrap();
    (0..rows)
        .map(|r| (0..cols).map(|c| cell(&e, r, 9 + c)).collect())
        .collect()
}

fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}
fn er(e: CellError) -> CellValue {
    CellValue::Error(e)
}
fn rows(g: &[&[f64]]) -> Vec<Vec<CellValue>> {
    g.iter()
        .map(|r| r.iter().map(|x| n(*x)).collect())
        .collect()
}

/// Parse `text` and print it back (display dialect and OOXML storage form).
fn round_trip(text: &str) -> (String, String) {
    let e = engine(&[]);
    struct Ctx<'a>(&'a SheetModel);
    impl sheet_parser::ParseCtx for Ctx<'_> {
        fn sheet_id(&self, name: &str) -> Option<u16> {
            self.0.sheet_id(name)
        }
        fn name_id(&self, _: &str) -> Option<sheet_core::ast::NameId> {
            None
        }
        fn current_sheet(&self) -> u16 {
            0
        }
    }
    impl sheet_parser::SheetNames for Ctx<'_> {
        fn sheet_name(&self, id: u16) -> Option<&str> {
            self.0.sheet(id).map(|s| s.name.as_str())
        }
    }
    let ctx = Ctx(e.model());
    let f = sheet_parser::parse(text, &ctx).unwrap();
    (
        sheet_parser::print(&f, 0, &ctx),
        sheet_parser::print_ooxml(&f, 0, &ctx),
    )
}

// ---- LET ---------------------------------------------------------------------

#[test]
fn sheet_fn_lambda_let_binds_and_scopes() {
    assert_eq!(calc(&[], "=LET(x,1,x+1)"), n(2.0));
    assert_eq!(calc(&[], "=LET(x,2,y,x*3,x+y)"), n(8.0));
    // Names are case-insensitive.
    assert_eq!(calc(&[], "=LET(Total,5,total*2)"), n(10.0));
    // An inner LET shadows the outer name for its own body only.
    assert_eq!(calc(&[], "=LET(x,1,LET(x,2,x)+x)"), n(3.0));
}

#[test]
fn sheet_fn_lambda_let_keeps_references_and_evaluates_once() {
    let s = &[("A1", "1"), ("A2", "2"), ("A3", "3")];
    // A range binding is still a range for SUM.
    assert_eq!(calc(s, "=LET(r,A1:A3,SUM(r))"), n(6.0));
    // A volatile value is computed ONCE: x - x is exactly 0.
    assert_eq!(calc(&[], "=LET(x,RAND(),x-x)"), n(0.0));
    // A LET rooted formula spills a bound block.
    assert_eq!(
        spill(&[], "=LET(s,SEQUENCE(3),s)", 3, 1),
        rows(&[&[1.0], &[2.0], &[3.0]])
    );
    // Editing a cell the binding reads recalculates the LET cell.
    let mut e = engine(s);
    e.enter(0, 0, 25, "=LET(r,A1:A3,SUM(r)*2)").unwrap();
    assert_eq!(cell(&e, 0, 25), n(12.0));
    e.enter(0, 0, 0, "10").unwrap();
    assert_eq!(cell(&e, 0, 25), n(30.0));
}

#[test]
fn sheet_fn_lambda_let_parse_and_storage_round_trip() {
    let (display, storage) = round_trip("LET(x,1,x+1)");
    assert_eq!(display, "LET(x,1,x+1)");
    assert_eq!(storage, "_xlfn.LET(_xlpm.x,1,_xlpm.x+1)");
    // An unknown name outside a LET is still a parse error (the ruling).
    let e = engine(&[]);
    let mut e = e;
    assert!(e.enter(0, 0, 0, "=x+1").is_err());
}

// ---- LAMBDA ------------------------------------------------------------------

#[test]
fn sheet_fn_lambda_lambda_calls_and_closures() {
    assert_eq!(calc(&[], "=LAMBDA(x,x+1)(2)"), n(3.0));
    assert_eq!(calc(&[], "=LET(f,LAMBDA(x,y,x*y),f(3,4))"), n(12.0));
    // A closure over an outer LET name.
    assert_eq!(calc(&[], "=LET(a,10,f,LAMBDA(x,x+a),f(1))"), n(11.0));
    // A parameter shadows the outer name of the same spelling.
    assert_eq!(calc(&[], "=LET(a,1,f,LAMBDA(a,a*2),f(5)+a)"), n(11.0));
    // Curried call.
    assert_eq!(calc(&[], "=LAMBDA(x,LAMBDA(y,x+y))(1)(2)"), n(3.0));
}

#[test]
fn sheet_fn_lambda_lambda_error_rulings() {
    // A lambda as a cell value: #CALC! grounded to #VALUE!.
    assert_eq!(calc(&[], "=LAMBDA(x,x)"), er(CellError::Value));
    // Wrong argument count.
    assert_eq!(calc(&[], "=LAMBDA(x,x)(1,2)"), er(CellError::Value));
    // Calling a value.
    assert_eq!(calc(&[], "=LET(f,5,f(1))"), er(CellError::Value));
    let (display, storage) = round_trip("LAMBDA(x,x+1)(2)");
    assert_eq!(display, "LAMBDA(x,x+1)(2)");
    assert_eq!(storage, "_xlfn.LAMBDA(_xlpm.x,_xlpm.x+1)(2)");
}

// ---- helpers -----------------------------------------------------------------

#[test]
fn sheet_fn_lambda_map() {
    let s = &[("A1", "1"), ("A2", "2"), ("A3", "3")];
    assert_eq!(
        spill(s, "=MAP(A1:A3,LAMBDA(x,x*2))", 3, 1),
        rows(&[&[2.0], &[4.0], &[6.0]])
    );
    assert_eq!(
        spill(&[], "=MAP({1,2},{3,4},LAMBDA(a,b,a+b))", 1, 2),
        rows(&[&[4.0, 6.0]])
    );
    // MAP over a range hands each element as a reference.
    let g = spill(s, "=MAP(A1:A2,LAMBDA(c,ISREF(c)))", 2, 1);
    assert_eq!(
        g,
        vec![vec![CellValue::Bool(true)], vec![CellValue::Bool(true)]]
    );
    // A non-lambda last argument.
    assert_eq!(calc(&[], "=MAP({1,2},5)"), er(CellError::Value));
}

#[test]
fn sheet_fn_lambda_reduce() {
    let s = &[("A1", "1"), ("A2", "2"), ("A3", "3")];
    assert_eq!(calc(s, "=REDUCE(0,A1:A3,LAMBDA(a,v,a+v))"), n(6.0));
    assert_eq!(calc(&[], "=REDUCE(1,{1,2,3,4},LAMBDA(a,v,a*v))"), n(24.0));
}

#[test]
fn sheet_fn_lambda_scan() {
    assert_eq!(
        spill(&[], "=SCAN(0,{1,2,3},LAMBDA(a,v,a+v))", 1, 3),
        rows(&[&[1.0, 3.0, 6.0]])
    );
}

#[test]
fn sheet_fn_lambda_byrow() {
    assert_eq!(
        spill(&[], "=BYROW({1,2;3,4},LAMBDA(r,SUM(r)))", 2, 1),
        rows(&[&[3.0], &[7.0]])
    );
    let s = &[("A1", "1"), ("B1", "2"), ("A2", "3"), ("B2", "4")];
    assert_eq!(
        spill(s, "=BYROW(A1:B2,LAMBDA(r,MAX(r)))", 2, 1),
        rows(&[&[2.0], &[4.0]])
    );
}

#[test]
fn sheet_fn_lambda_bycol() {
    assert_eq!(
        spill(&[], "=BYCOL({1,2;3,4},LAMBDA(c,SUM(c)))", 1, 2),
        rows(&[&[4.0, 6.0]])
    );
}

#[test]
fn sheet_fn_lambda_makearray() {
    assert_eq!(
        spill(&[], "=MAKEARRAY(2,3,LAMBDA(r,c,r*c))", 2, 3),
        rows(&[&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]])
    );
    assert_eq!(
        calc(&[], "=MAKEARRAY(0,3,LAMBDA(r,c,r))"),
        er(CellError::Value)
    );
    assert_eq!(
        calc(&[], "=MAKEARRAY(2000,2000,LAMBDA(r,c,r))"),
        er(CellError::Num)
    );
}

// ---- ISREF ---------------------------------------------------------------------

#[test]
fn sheet_fn_info_isref() {
    let s = &[("A1", "1")];
    assert_eq!(calc(s, "=ISREF(A1)"), CellValue::Bool(true));
    assert_eq!(calc(s, "=ISREF(A1:B2)"), CellValue::Bool(true));
    assert_eq!(calc(s, "=ISREF(OFFSET(A1,1,1))"), CellValue::Bool(true));
    assert_eq!(calc(s, "=ISREF(INDIRECT(\"A1\"))"), CellValue::Bool(true));
    assert_eq!(calc(s, "=ISREF(5)"), CellValue::Bool(false));
    assert_eq!(calc(s, "=ISREF({1,2})"), CellValue::Bool(false));
    assert_eq!(calc(s, "=ISREF(SEQUENCE(2))"), CellValue::Bool(false));
    assert_eq!(
        calc(s, "=ISREF(INDIRECT(\"no such\"))"),
        CellValue::Bool(false)
    );
}
