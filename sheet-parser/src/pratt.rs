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

//! Pratt (precedence-climbing) parser (spec §6.1). Implements the Excel
//! operator precedence with the two documented quirks below.
//!
//! **Precedence (tightest → loosest):** reference ops (`:` range > ` `
//! intersection > `,` union) > unary `-`/`+` > `%` postfix > `^` > `*` `/` >
//! `+` `-` > `&` > comparisons.
//!
//! **Ruling — unary minus binds TIGHTER than `^`** (Excel quirk):
//! `-2^2 == 4` (parsed `(-2)^2`), unlike standard maths where `-2^2 == -4`.
//! We adopt Excel (D-2).
//!
//! **Ruling — `^` is LEFT-associative** in Excel: `2^3^2 == 64`
//! (`(2^3)^2`), not `512`. We adopt Excel.
//!
//! **Ruling — union `,` is a reference operator only inside parentheses.**
//! At function-call argument depth a bare `,` separates arguments, so the
//! parser tracks "are we parsing a parenthesized group vs. an argument list".
//! `;` is accepted only as an array-literal row separator — T0 is the en-US
//! dialect (`,` args), per the registry ruling.

use sheet_core::ast::{BinOp, Expr, LitValue, OrderedF64, UnOp};

use crate::error::ParseError;
use crate::lexer::{TokKind, Token};
use crate::refs;
use crate::ParseCtx;

/// Parse a flat token stream into a root [`Expr`].
pub fn parse_tokens(
    tokens: &[Token],
    ctx: &dyn ParseCtx,
    src_len: usize,
) -> Result<Expr, ParseError> {
    let mut p = Parser {
        tokens,
        pos: 0,
        ctx,
        src_len,
        in_paren: false,
    };
    let e = p.parse_bp(0)?;
    if p.pos != tokens.len() {
        let span = tokens[p.pos].span.clone();
        return Err(ParseError::new("unexpected trailing token", span));
    }
    Ok(e)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    ctx: &'a dyn ParseCtx,
    src_len: usize,
    /// True while parsing inside a parenthesized `(...)` group, where a bare
    /// `,` is the reference UNION operator. False inside a function-argument
    /// list, where `,` separates arguments. Saved/restored on entry to each
    /// `(...)` group and each argument list (ruling, spec §6.1).
    in_paren: bool,
}

// ---- Binding powers. Higher = binds tighter. Pairs are (left, right);
// for left-assoc ops right > left, for right-assoc left > right. ----

// Comparisons (loosest binary).
const BP_CMP: (u8, u8) = (2, 3);
// Concatenation `&`.
const BP_CONCAT: (u8, u8) = (4, 5);
// Additive `+ -`.
const BP_ADD: (u8, u8) = (6, 7);
// Multiplicative `* /`.
const BP_MUL: (u8, u8) = (8, 9);
// Exponent `^` — LEFT-associative. In precedence-climbing, left-assoc means
// the right binding power exceeds the left, so the recursion (which uses
// `rbp` as its `min_bp`) refuses to absorb the *next* same-level `^`:
// `2^3^2` parses `(2^3)^2`.
const BP_POW: (u8, u8) = (10, 11);
// Postfix `%` (a prefix-side binding power; nothing on the right).
const BP_PERCENT: u8 = 13;
// Prefix unary `-`/`+` — binds TIGHTER than `^` (the Excel quirk).
const BP_UNARY: u8 = 15;
// Reference union `,` (inside parens only).
const BP_UNION: (u8, u8) = (17, 18);
// Reference intersection ` ` (space).
const BP_ISECT: (u8, u8) = (19, 20);
// Reference range `:` (tightest).
const BP_RANGE: (u8, u8) = (21, 22);

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> &Token {
        let t = &self.tokens[self.pos];
        self.pos += 1;
        t
    }

    /// Span covering the whole input when no token is available (EOF).
    fn eof_span(&self) -> std::ops::Range<usize> {
        self.src_len..self.src_len
    }

    /// Precedence-climbing core. `min_bp` is the minimum left binding power a
    /// following infix operator must exceed to be absorbed here.
    fn parse_bp(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_prefix()?;

        loop {
            // Postfix `%`.
            if let Some(tok) = self.peek() {
                if matches!(tok.kind, TokKind::Percent) && BP_PERCENT >= min_bp {
                    self.bump();
                    lhs = Expr::Unary(UnOp::Percent, Box::new(lhs));
                    continue;
                }
            }

            // Implicit intersection: a space between two operand expressions.
            // Detected as "next token is an operand-start AND had ws_before".
            if let Some(tok) = self.peek() {
                if tok.ws_before && is_operand_start(&tok.kind) {
                    let (lbp, rbp) = BP_ISECT;
                    if lbp >= min_bp {
                        let rhs = self.parse_bp(rbp)?;
                        lhs = Expr::Binary(BinOp::Isect, Box::new(lhs), Box::new(rhs));
                        continue;
                    } else {
                        break;
                    }
                }
            }

            let Some(tok) = self.peek() else { break };
            let (op, (lbp, rbp)) = match infix_op(&tok.kind) {
                Some(x) => x,
                None => break,
            };
            // Union `,` is an operator ONLY inside a `(...)` group; in an
            // argument list a `,` is a separator, so stop here and let
            // `parse_args` consume it.
            if op == BinOp::Union && !self.in_paren {
                break;
            }
            if lbp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_bp(rbp)?;
            lhs = fold_binary(op, lhs, rhs);
        }

        Ok(lhs)
    }

    /// Parse a prefix position: unary operators or a primary.
    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        let Some(tok) = self.peek() else {
            return Err(ParseError::new(
                "unexpected end of formula",
                self.eof_span(),
            ));
        };
        match &tok.kind {
            TokKind::Minus => {
                self.bump();
                let rhs = self.parse_bp(BP_UNARY)?;
                Ok(Expr::Unary(UnOp::Neg, Box::new(rhs)))
            }
            TokKind::Plus => {
                self.bump();
                let rhs = self.parse_bp(BP_UNARY)?;
                Ok(Expr::Unary(UnOp::Plus, Box::new(rhs)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.bump().clone();
        match tok.kind {
            TokKind::Number(n) => Ok(Expr::Lit(LitValue::Number(OrderedF64::new(n)))),
            TokKind::Str(s) => Ok(Expr::Lit(LitValue::Text(s.into()))),
            TokKind::Bool(b) => Ok(Expr::Lit(LitValue::Bool(b))),
            TokKind::Error(e) => Ok(Expr::Lit(LitValue::Error(e))),
            TokKind::Cell {
                row,
                col,
                row_abs,
                col_abs,
            } => {
                let sheet = self.ctx.current_sheet();
                Ok(Expr::Ref(refs::cell(sheet, row, col, row_abs, col_abs)))
            }
            TokKind::SheetQual(name) => self.parse_qualified(&name, tok.span),
            TokKind::Ident(name) => self.parse_ident(&name, tok.span),
            TokKind::LParen => {
                // Parenthesized group: a `,` here is a reference UNION.
                let saved = self.in_paren;
                self.in_paren = true;
                let inner = self.parse_bp(0)?;
                self.in_paren = saved;
                self.expect(TokKind::RParen, "expected ')'")?;
                Ok(inner)
            }
            TokKind::LBrace => self.parse_array(tok.span),
            other => Err(ParseError::new(
                format!("unexpected token {}", describe(&other)),
                tok.span,
            )),
        }
    }

    /// After `Sheet1!` (or `'Q'!`): expects a cell, optionally a range.
    fn parse_qualified(
        &mut self,
        sheet_name: &str,
        qual_span: std::ops::Range<usize>,
    ) -> Result<Expr, ParseError> {
        let sheet = self.ctx.sheet_id(sheet_name).ok_or_else(|| {
            ParseError::new(format!("unknown sheet {sheet_name:?}"), qual_span.clone())
        })?;
        let Some(tok) = self.peek() else {
            return Err(ParseError::new(
                "expected a cell reference after sheet qualifier",
                self.eof_span(),
            ));
        };
        let TokKind::Cell {
            row,
            col,
            row_abs,
            col_abs,
        } = tok.kind
        else {
            return Err(ParseError::new(
                "expected a cell reference after sheet qualifier",
                tok.span.clone(),
            ));
        };
        self.bump();
        let start = refs::cell(sheet, row, col, row_abs, col_abs);
        // A sheet-qualified range: `Sheet1!A1:B2`. The qualifier applies to
        // the whole range, so the right endpoint must NOT carry its own
        // qualifier (it inherits `sheet`).
        if matches!(self.peek().map(|t| &t.kind), Some(TokKind::Colon)) {
            // Only fold if the next-next token is a plain cell.
            if let Some(next) = self.tokens.get(self.pos + 1) {
                if let TokKind::Cell {
                    row: r2,
                    col: c2,
                    row_abs: ra2,
                    col_abs: ca2,
                } = next.kind
                {
                    self.bump(); // ':'
                    self.bump(); // cell
                    let end = refs::cell(sheet, r2, c2, ra2, ca2);
                    return Ok(Expr::Range(refs::range(start, end)));
                }
            }
        }
        Ok(Expr::Ref(start))
    }

    /// A bare identifier: a function call iff immediately followed by `(`,
    /// else a defined name. Unknown function name → `ParseError` (ruling).
    fn parse_ident(
        &mut self,
        name: &str,
        ident_span: std::ops::Range<usize>,
    ) -> Result<Expr, ParseError> {
        let is_call = matches!(self.peek().map(|t| &t.kind), Some(TokKind::LParen))
            && self.peek().is_some_and(|t| !t.ws_before);
        if is_call {
            let fid = sheet_core::funcs::lookup_func(name).ok_or_else(|| {
                ParseError::new(format!("unknown function {name:?}"), ident_span.clone())
            })?;
            self.bump(); // '('
            let args = self.parse_args()?;
            self.expect(TokKind::RParen, "expected ')' after arguments")?;
            Ok(Expr::Func(fid, args))
        } else {
            let nid = self.ctx.name_id(name).ok_or_else(|| {
                ParseError::new(format!("unknown name {name:?}"), ident_span.clone())
            })?;
            Ok(Expr::Name(nid))
        }
    }

    /// Function-argument list: comma-separated expressions. A `,` here is an
    /// argument separator, NOT a reference union (the union lives inside an
    /// inner `(...)`). An empty list (`SUM()`) yields no args.
    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        // Inside an argument list a bare `,` is a separator, not a union.
        let saved = self.in_paren;
        self.in_paren = false;
        let mut args = Vec::new();
        if matches!(self.peek().map(|t| &t.kind), Some(TokKind::RParen)) {
            self.in_paren = saved;
            return Ok(args);
        }
        loop {
            args.push(self.parse_bp(0)?);
            match self.peek().map(|t| &t.kind) {
                Some(TokKind::Comma) => {
                    self.bump();
                }
                _ => break,
            }
        }
        self.in_paren = saved;
        Ok(args)
    }

    /// `{1,2;3,4}` — rows by `;`, columns by `,`. Elements are literals
    /// (including a negated number). Parse-only in T0.
    fn parse_array(&mut self, open_span: std::ops::Range<usize>) -> Result<Expr, ParseError> {
        let mut rows: Vec<Vec<Expr>> = Vec::new();
        let mut row: Vec<Expr> = Vec::new();
        // Empty array `{}` is invalid in Excel.
        if matches!(self.peek().map(|t| &t.kind), Some(TokKind::RBrace)) {
            return Err(ParseError::new("empty array literal", open_span));
        }
        loop {
            row.push(self.parse_array_element()?);
            match self.peek().map(|t| &t.kind) {
                Some(TokKind::Comma) => {
                    self.bump();
                }
                Some(TokKind::Semicolon) => {
                    self.bump();
                    rows.push(std::mem::take(&mut row));
                }
                Some(TokKind::RBrace) => {
                    self.bump();
                    rows.push(row);
                    break;
                }
                _ => {
                    let span = self
                        .peek()
                        .map(|t| t.span.clone())
                        .unwrap_or_else(|| self.eof_span());
                    return Err(ParseError::new("expected ',', ';' or '}' in array", span));
                }
            }
        }
        // Excel requires rectangular arrays.
        let width = rows[0].len();
        if rows.iter().any(|r| r.len() != width) {
            return Err(ParseError::new(
                "array literal rows have differing lengths",
                open_span,
            ));
        }
        Ok(Expr::Array(rows))
    }

    /// One array element: a literal, optionally negated.
    fn parse_array_element(&mut self) -> Result<Expr, ParseError> {
        let mut neg = false;
        if matches!(self.peek().map(|t| &t.kind), Some(TokKind::Minus)) {
            self.bump();
            neg = true;
        } else if matches!(self.peek().map(|t| &t.kind), Some(TokKind::Plus)) {
            self.bump();
        }
        let Some(tok) = self.peek() else {
            return Err(ParseError::new("expected array element", self.eof_span()));
        };
        let span = tok.span.clone();
        let lit = match &tok.kind {
            TokKind::Number(n) => {
                let v = if neg { -*n } else { *n };
                LitValue::Number(OrderedF64::new(v))
            }
            TokKind::Str(s) if !neg => LitValue::Text(s.clone().into()),
            TokKind::Bool(b) if !neg => LitValue::Bool(*b),
            TokKind::Error(e) if !neg => LitValue::Error(*e),
            _ => return Err(ParseError::new("array elements must be literals", span)),
        };
        self.bump();
        Ok(Expr::Lit(lit))
    }

    fn expect(&mut self, kind: TokKind, msg: &str) -> Result<(), ParseError> {
        match self.peek() {
            Some(t) if t.kind == kind => {
                self.bump();
                Ok(())
            }
            Some(t) => Err(ParseError::new(msg, t.span.clone())),
            None => Err(ParseError::new(msg, self.eof_span())),
        }
    }
}

/// Map an infix token to its `BinOp` and binding powers. `,` is union (only
/// reached inside `(...)`; in argument lists `parse_args` consumes `,`
/// directly before `parse_bp` ever sees it).
fn infix_op(kind: &TokKind) -> Option<(BinOp, (u8, u8))> {
    Some(match kind {
        TokKind::Eq => (BinOp::Eq, BP_CMP),
        TokKind::Ne => (BinOp::Ne, BP_CMP),
        TokKind::Lt => (BinOp::Lt, BP_CMP),
        TokKind::Le => (BinOp::Le, BP_CMP),
        TokKind::Gt => (BinOp::Gt, BP_CMP),
        TokKind::Ge => (BinOp::Ge, BP_CMP),
        TokKind::Amp => (BinOp::Concat, BP_CONCAT),
        TokKind::Plus => (BinOp::Add, BP_ADD),
        TokKind::Minus => (BinOp::Sub, BP_ADD),
        TokKind::Star => (BinOp::Mul, BP_MUL),
        TokKind::Slash => (BinOp::Div, BP_MUL),
        TokKind::Caret => (BinOp::Pow, BP_POW),
        TokKind::Colon => (BinOp::Range, BP_RANGE),
        TokKind::Comma => (BinOp::Union, BP_UNION),
        _ => return None,
    })
}

/// Fold a `Range` BinOp whose operands are both plain cell refs into a single
/// [`Expr::Range`] (constitution: keep `BinOp::Range` only for non-literal
/// operands, e.g. `INDIRECT("A"):B2`). Other ops pass through unchanged.
fn fold_binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    if op == BinOp::Range {
        if let (Expr::Ref(a), Expr::Ref(b)) = (&lhs, &rhs) {
            return Expr::Range(refs::range(*a, *b));
        }
    }
    Expr::Binary(op, Box::new(lhs), Box::new(rhs))
}

/// True if a token can begin an operand (used for space-intersection).
fn is_operand_start(kind: &TokKind) -> bool {
    matches!(
        kind,
        TokKind::Number(_)
            | TokKind::Str(_)
            | TokKind::Bool(_)
            | TokKind::Error(_)
            | TokKind::Cell { .. }
            | TokKind::SheetQual(_)
            | TokKind::Ident(_)
            | TokKind::LParen
            | TokKind::LBrace
    )
}

fn describe(kind: &TokKind) -> &'static str {
    match kind {
        TokKind::RParen => "')'",
        TokKind::RBrace => "'}'",
        TokKind::Comma => "','",
        TokKind::Semicolon => "';'",
        TokKind::Colon => "':'",
        TokKind::Star => "'*'",
        TokKind::Slash => "'/'",
        TokKind::Caret => "'^'",
        TokKind::Amp => "'&'",
        TokKind::Percent => "'%'",
        TokKind::Eq => "'='",
        TokKind::Ne => "'<>'",
        TokKind::Lt => "'<'",
        TokKind::Le => "'<='",
        TokKind::Gt => "'>'",
        TokKind::Ge => "'>='",
        _ => "token",
    }
}
