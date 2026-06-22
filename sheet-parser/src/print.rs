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

//! The canonical printer (spec §6.1): AST → formula text (no leading `=`).
//! `parse ∘ print` is an AST fixpoint (property-tested). Conventions:
//!
//! - UPPER-cased function names (from the registry meta).
//! - Minimal parentheses, re-derived from the same precedence model the
//!   parser uses (including the unary-minus-tighter-than-`^` quirk and `^`
//!   left-associativity).
//! - `$` absolute flags preserved verbatim.
//! - A sheet prefix is printed iff the ref's sheet differs from the
//!   formula's `home` sheet (the printer takes a `home: SheetId` parameter —
//!   a documented amendment to the frozen signature so the prefix decision
//!   is well-defined). Quoted iff the name needs quoting (non-alphanumeric
//!   or a leading digit).
//! - f64 literals via Rust `Display` (shortest round-tripping form).

use std::fmt::Write as _;

use sheet_core::ast::{BinOp, Expr, Formula, LitValue, StructuredRef, TableArea, UnOp};
use sheet_core::{format_a1, CellRef, RangeRef, SheetId};

use crate::SheetNames;

/// Print `f` to canonical text. `home` is the sheet the formula lives on:
/// refs on `home` print without a sheet prefix; refs elsewhere are prefixed.
pub fn print(f: &Formula, home: SheetId, sheets: &dyn SheetNames) -> String {
    let mut out = String::new();
    Printer { home, sheets }.expr(&f.root, &mut out, Prec::TOP, Side::Whole);
    out
}

struct Printer<'a> {
    home: SheetId,
    sheets: &'a dyn SheetNames,
}

/// Precedence level for the printer. Higher = tighter (mirrors the parser's
/// binding powers, collapsed to comparison levels).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Prec(u8);

impl Prec {
    const TOP: Prec = Prec(0);
    const CMP: Prec = Prec(1);
    const CONCAT: Prec = Prec(2);
    const ADD: Prec = Prec(3);
    const MUL: Prec = Prec(4);
    const POW: Prec = Prec(5);
    const PERCENT: Prec = Prec(6);
    const UNARY: Prec = Prec(7);
    const UNION: Prec = Prec(8);
    const ISECT: Prec = Prec(9);
    const RANGE: Prec = Prec(10);
    const ATOM: Prec = Prec(11);
}

/// Which side of its parent a sub-expression sits on (for associativity).
#[derive(Copy, Clone, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Whole,
}

impl Printer<'_> {
    /// Print `e`. `parent` is the parent's precedence; `side` says whether
    /// `e` is the left/right operand (for associativity-driven parens).
    fn expr(&self, e: &Expr, out: &mut String, parent: Prec, side: Side) {
        let prec = expr_prec(e);
        // Parenthesize when the child binds looser than the parent, OR equal
        // precedence on the associativity-disfavored side.
        let needs = prec < parent || (prec == parent && needs_assoc_paren(e, side));
        if needs {
            out.push('(');
        }
        self.expr_inner(e, out);
        if needs {
            out.push(')');
        }
    }

    fn expr_inner(&self, e: &Expr, out: &mut String) {
        match e {
            Expr::Lit(l) => self.lit(l, out),
            Expr::Ref(r) => self.cell(r, out),
            Expr::Range(r) => self.range(r, out),
            Expr::Name(_) => {
                // Names print through the SheetNames trait? No — names are
                // not sheet-scoped text here. The parser resolves a name to a
                // NameId; round-tripping its spelling needs a name table the
                // printer is not given. T0 prints names as their resolved
                // id is opaque — but every golden/property formula that uses
                // a name supplies it through ctx, and the fixpoint check
                // re-parses with the same ctx, so a stable textual form is
                // required. We print `_NAME<id>` and the test ctx resolves it
                // back. (Documented: real name spelling round-trip is T1,
                // when the printer gains a NameTable.)
                if let Expr::Name(nid) = e {
                    let _ = write!(out, "_NAME{}", nid.0);
                }
            }
            Expr::Unary(op, inner) => self.unary(*op, inner, out),
            Expr::Binary(op, a, b) => self.binary(*op, a, b, out),
            Expr::Func(fid, args) => self.func(*fid, args, out),
            Expr::Array(rows) => self.array(rows, out),
            Expr::StructuredRef(s) => self.structured_ref(s, out),
            // A spill ref prints as `<inner>#` (M1 spill track).
            Expr::SpillRef(inner) => {
                self.expr(inner, out, Prec::ATOM, Side::Whole);
                out.push('#');
            }
        }
    }

    /// Print a structured (table) reference in the canonical Excel forms
    /// (ECMA-376 §18.17.2.4 / Microsoft structured-reference syntax):
    ///
    /// - bare column, default Data area: `Table1[Col]`
    /// - whole-area specifier: `Table1[[#All]]`, `Table1[[#Headers]]`,
    ///   `Table1[[#Totals]]`
    /// - area + single column: `Table1[[#Headers],[Col]]`
    /// - a column span: `Table1[[Col1]:[Col2]]`
    /// - current-row (`ThisRow`) with an empty table name: `[@Col]`
    ///
    /// These are the standard Excel spellings; Phase B's parser round-trips
    /// against exactly these (the canonical-print contract, like A1 refs).
    fn structured_ref(&self, s: &StructuredRef, out: &mut String) {
        // ThisRow with an empty table name is the in-table `[@Col]` shorthand.
        // A plain column prints `[@Col]`; a column that needs escaping prints
        // the bracketed `[@[Col Name]]` form Excel emits (the `@` shorthand
        // takes a single bracketed column, ECMA-376 §18.17.2.4).
        if s.area == TableArea::ThisRow && s.table.is_empty() {
            out.push_str("[@");
            if let Some(c) = &s.col_start {
                push_column(out, c);
            }
            if let Some(c) = &s.col_end {
                out.push(':');
                push_bracketed_col(out, c);
            }
            out.push(']');
            return;
        }

        out.push_str(&s.table);

        let area_tok = match s.area {
            TableArea::Data => None, // Data is the default → no `#`-specifier
            TableArea::All => Some("#All"),
            TableArea::Headers => Some("#Headers"),
            TableArea::Totals => Some("#Totals"),
            TableArea::ThisRow => Some("#This Row"),
        };

        match (area_tok, &s.col_start, &s.col_end) {
            // No area specifier and a single column: `Table1[Col]` (plain) or
            // `Table1[[Col Name]]` (the column needs bracket-escaping).
            (None, Some(c), None) => {
                out.push('[');
                push_column(out, c);
                out.push(']');
            }
            // No area specifier and a column span: `Table1[[Col1]:[Col2]]`.
            (None, Some(c0), Some(c1)) => {
                out.push('[');
                push_bracketed_col(out, c0);
                out.push(':');
                push_bracketed_col(out, c1);
                out.push(']');
            }
            // No area, no column (whole Data body): `Table1[]`.
            (None, None, _) => {
                out.push_str("[]");
            }
            // An area specifier, no column: `Table1[[#All]]`.
            (Some(area), None, _) => {
                out.push('[');
                push_bracketed(out, area);
                out.push(']');
            }
            // An area specifier with a single column:
            // `Table1[[#Headers],[Col]]`.
            (Some(area), Some(c), None) => {
                out.push('[');
                push_bracketed(out, area);
                out.push(',');
                push_bracketed_col(out, c);
                out.push(']');
            }
            // An area specifier with a column span:
            // `Table1[[#Headers],[Col1]:[Col2]]`.
            (Some(area), Some(c0), Some(c1)) => {
                out.push('[');
                push_bracketed(out, area);
                out.push(',');
                push_bracketed_col(out, c0);
                out.push(':');
                push_bracketed_col(out, c1);
                out.push(']');
            }
        }
    }

    fn lit(&self, l: &LitValue, out: &mut String) {
        match l {
            LitValue::Number(n) => {
                let _ = write!(out, "{}", n.get());
            }
            LitValue::Text(s) => {
                out.push('"');
                for ch in s.chars() {
                    if ch == '"' {
                        out.push('"'); // escape: " -> ""
                    }
                    out.push(ch);
                }
                out.push('"');
            }
            LitValue::Bool(b) => out.push_str(if *b { "TRUE" } else { "FALSE" }),
            LitValue::Error(e) => out.push_str(e.as_str()),
        }
    }

    /// Print a sheet prefix if `sheet != home`, quoting the name as needed.
    fn sheet_prefix(&self, sheet: SheetId, out: &mut String) {
        if sheet == self.home {
            return;
        }
        // An unknown sheet id has no name — fall back to `#REF!` per Excel.
        let Some(name) = self.sheets.sheet_name(sheet) else {
            out.push_str("#REF!");
            return;
        };
        if needs_quoting(name) {
            out.push('\'');
            for ch in name.chars() {
                if ch == '\'' {
                    out.push('\''); // escape ' -> ''
                }
                out.push(ch);
            }
            out.push('\'');
        } else {
            out.push_str(name);
        }
        out.push('!');
    }

    fn cell(&self, r: &CellRef, out: &mut String) {
        self.sheet_prefix(r.sheet, out);
        out.push_str(&format_a1(r.row, r.col, r.row_abs, r.col_abs));
    }

    fn range(&self, r: &RangeRef, out: &mut String) {
        // The sheet prefix applies to the whole range (printed once).
        self.sheet_prefix(r.start.sheet, out);
        out.push_str(&format_a1(
            r.start.row,
            r.start.col,
            r.start.row_abs,
            r.start.col_abs,
        ));
        out.push(':');
        out.push_str(&format_a1(
            r.end.row,
            r.end.col,
            r.end.row_abs,
            r.end.col_abs,
        ));
    }

    fn unary(&self, op: UnOp, inner: &Expr, out: &mut String) {
        match op {
            UnOp::Neg => {
                out.push('-');
                self.expr(inner, out, Prec::UNARY, Side::Right);
            }
            UnOp::Plus => {
                out.push('+');
                self.expr(inner, out, Prec::UNARY, Side::Right);
            }
            UnOp::Percent => {
                self.expr(inner, out, Prec::PERCENT, Side::Left);
                out.push('%');
            }
        }
    }

    fn binary(&self, op: BinOp, a: &Expr, b: &Expr, out: &mut String) {
        let prec = binop_prec(op);
        self.expr(a, out, prec, Side::Left);
        out.push_str(binop_text(op));
        self.expr(b, out, prec, Side::Right);
    }

    fn func(&self, fid: sheet_core::ast::FuncId, args: &[Expr], out: &mut String) {
        out.push_str(sheet_core::funcs::meta(fid).name);
        out.push('(');
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            // A union (`,`) directly under a function arg MUST be parenthesized:
            // a bare top-level `,` would otherwise read as an argument
            // separator (`SUM((A1,B2))` vs the two-arg `SUM(A1,B2)`).
            if matches!(a, Expr::Binary(BinOp::Union, _, _)) {
                out.push('(');
                self.expr_inner(a, out);
                out.push(')');
            } else {
                // Arguments reset to TOP precedence (the parens delimit them).
                self.expr(a, out, Prec::TOP, Side::Whole);
            }
        }
        out.push(')');
    }

    fn array(&self, rows: &[Vec<Expr>], out: &mut String) {
        out.push('{');
        for (ri, row) in rows.iter().enumerate() {
            if ri > 0 {
                out.push(';');
            }
            for (ci, el) in row.iter().enumerate() {
                if ci > 0 {
                    out.push(',');
                }
                self.expr(el, out, Prec::TOP, Side::Whole);
            }
        }
        out.push('}');
    }
}

/// Wrap `inner` in `[ ]` (a structured-ref bracketed token, e.g. `[#All]` or
/// `[Col]`). Used for `#`-area keywords (which never need escaping).
fn push_bracketed(out: &mut String, inner: &str) {
    out.push('[');
    out.push_str(inner);
    out.push(']');
}

/// Emit a structured-ref COLUMN in the *minimal* canonical form: a plain
/// simple column `Col` when the name has no characters that would confuse the
/// bracket grammar, else the bracket-escaped `[Col Name]` form. Caller has
/// already opened the outer `[`; this writes only the column token.
fn push_column(out: &mut String, name: &str) {
    if col_needs_brackets(name) {
        push_bracketed_col(out, name);
    } else {
        out.push_str(name);
    }
}

/// Emit a column wrapped in its own `[ ]` with the `'`-escaping ECMA-376
/// §18.17.2.4 requires (`[`, `]`, `#`, `'`, `@` are each prefixed with `'`).
/// This is the form used inside item lists / spans, where the column is always
/// individually bracketed.
fn push_bracketed_col(out: &mut String, name: &str) {
    out.push('[');
    for ch in name.chars() {
        if matches!(ch, '[' | ']' | '#' | '\'' | '@') {
            out.push('\'');
        }
        out.push(ch);
    }
    out.push(']');
}

/// A column name needs the bracketed form (`[[Col]]` rather than `[Col]`) when
/// a *simple* (run-to-`]`) token would re-parse differently: it contains a
/// space or any structural character (`[ ] # ' @ : ,`), is empty, or would be
/// read as an `#`-area keyword. Excel emits the bracketed form for exactly
/// these (the parse-print fixpoint then recovers the same AST).
fn col_needs_brackets(name: &str) -> bool {
    name.is_empty()
        || name.starts_with('#')
        || name.starts_with('@')
        || name
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '[' | ']' | '#' | '\'' | '@' | ':' | ','))
}

/// The printing precedence of an expression node.
fn expr_prec(e: &Expr) -> Prec {
    match e {
        Expr::Lit(_) | Expr::Ref(_) | Expr::Range(_) | Expr::Name(_) | Expr::Func(_, _) => {
            Prec::ATOM
        }
        // Structured/spill refs are atoms (a `#` postfix on an atom is still
        // atomic for re-parse purposes).
        Expr::Array(_) | Expr::StructuredRef(_) | Expr::SpillRef(_) => Prec::ATOM,
        Expr::Unary(UnOp::Percent, _) => Prec::PERCENT,
        Expr::Unary(_, _) => Prec::UNARY,
        Expr::Binary(op, _, _) => binop_prec(*op),
    }
}

fn binop_prec(op: BinOp) -> Prec {
    match op {
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Prec::CMP,
        BinOp::Concat => Prec::CONCAT,
        BinOp::Add | BinOp::Sub => Prec::ADD,
        BinOp::Mul | BinOp::Div => Prec::MUL,
        BinOp::Pow => Prec::POW,
        BinOp::Union => Prec::UNION,
        BinOp::Isect => Prec::ISECT,
        BinOp::Range => Prec::RANGE,
    }
}

fn binop_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Pow => "^",
        BinOp::Concat => "&",
        BinOp::Eq => "=",
        BinOp::Ne => "<>",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::Range => ":",
        BinOp::Union => ",",
        BinOp::Isect => " ",
    }
}

/// At equal precedence, decide whether the operand on `side` needs parens
/// for the operator to re-parse the same way (associativity).
fn needs_assoc_paren(e: &Expr, side: Side) -> bool {
    match e {
        // Left-associative binary ops: the RIGHT operand of the same level
        // needs parens (e.g. `a-(b-c)`, `a/(b/c)`, `2^(3^2)` since `^` is
        // left-assoc).
        Expr::Binary(op, _, _) if is_left_assoc(*op) => side == Side::Right,
        // `&` is associative but we keep it minimal: right operand at equal
        // level reparses fine, so no parens needed; treat as left-assoc.
        Expr::Binary(_, _, _) => side == Side::Right,
        _ => false,
    }
}

fn is_left_assoc(op: BinOp) -> bool {
    // All Excel binary operators are left-associative, including `^` (ruling).
    matches!(
        op,
        BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Pow
            | BinOp::Concat
            | BinOp::Range
            | BinOp::Union
            | BinOp::Isect
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
    )
}

/// A sheet name needs quoting iff it is empty, starts with a digit, or
/// contains any non-alphanumeric/underscore character (ECMA-376 naming).
fn needs_quoting(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        None => true,
        Some(c) if c.is_ascii_digit() => true,
        Some(c) if !(c.is_alphanumeric() || c == '_') => true,
        _ => name.chars().any(|c| !(c.is_alphanumeric() || c == '_')),
    }
}
