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

//! The formula lexer (spec §6.1). Turns Excel-dialect formula text (without
//! the leading `=`) into a flat token stream. Whitespace is NOT a token: the
//! space-as-intersection operator (ECMA-376 §18.17.2) is reconstructed by
//! the parser from each token's `ws_before` flag — a space is significant
//! only *between* two operand tokens, which only the grammar can decide.
//!
//! Numbers, double-quoted strings (with `""` escapes), `TRUE`/`FALSE`, the
//! eight error literals, A1 references with `$` flags, sheet-qualified and
//! `'quoted''sheet'`-qualified references, and identifiers (a name, or a
//! function iff immediately followed by `(`) are all recognized here.

use std::ops::Range;

use crate::error::ParseError;

/// A lexical token plus its byte span and whether whitespace preceded it.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokKind,
    pub span: Range<usize>,
    /// True if one or more ASCII whitespace chars immediately preceded this
    /// token. The parser uses this to detect the intersection operator.
    pub ws_before: bool,
}

/// The lexical token kinds.
#[derive(Clone, Debug, PartialEq)]
pub enum TokKind {
    /// A numeric literal (already parsed to f64; covers `1.5E-3`).
    Number(f64),
    /// A string literal (with `""` un-escaped to `"`).
    Str(String),
    /// A boolean literal `TRUE`/`FALSE` (case-insensitive).
    Bool(bool),
    /// An error literal, e.g. `#DIV/0!`. Carries the parsed `CellError`.
    Error(sheet_core::CellError),
    /// A bare A1 cell token, e.g. `$B$7` — column/row + `$` flags, no sheet.
    Cell {
        row: u32,
        col: u32,
        row_abs: bool,
        col_abs: bool,
    },
    /// A sheet qualifier `Sheet1!` or `'Quoted''Name'!` (un-escaped name).
    /// Always immediately followed (no whitespace) by a `Cell`.
    SheetQual(String),
    /// An identifier: a defined-name or a function name. The parser decides
    /// which by peeking for a following `(`.
    Ident(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Amp,
    Percent,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Tokenize `input` (formula text without the leading `=`).
pub fn lex(input: &str) -> Result<Vec<Token>, ParseError> {
    Lexer {
        src: input,
        bytes: input.as_bytes(),
        pos: 0,
    }
    .run()
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn run(mut self) -> Result<Vec<Token>, ParseError> {
        let mut out = Vec::new();
        loop {
            let ws_before = self.skip_ws();
            if self.pos >= self.bytes.len() {
                break;
            }
            let start = self.pos;
            let kind = self.next_token()?;
            out.push(Token {
                kind,
                span: start..self.pos,
                ws_before,
            });
        }
        Ok(out)
    }

    /// Advance over ASCII whitespace; return whether any was consumed.
    fn skip_ws(&mut self) -> bool {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        self.pos != start
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next_token(&mut self) -> Result<TokKind, ParseError> {
        let b = self.bytes[self.pos];
        match b {
            b'"' => self.lex_string(),
            b'#' => self.lex_error_literal(),
            b'\'' => self.lex_quoted_sheet(),
            b'0'..=b'9' => Ok(self.lex_number()),
            b'.' if self.bytes.get(self.pos + 1).is_some_and(u8::is_ascii_digit) => {
                Ok(self.lex_number())
            }
            b'(' => self.one(TokKind::LParen),
            b')' => self.one(TokKind::RParen),
            b'{' => self.one(TokKind::LBrace),
            b'}' => self.one(TokKind::RBrace),
            b',' => self.one(TokKind::Comma),
            b';' => self.one(TokKind::Semicolon),
            b':' => self.one(TokKind::Colon),
            b'+' => self.one(TokKind::Plus),
            b'-' => self.one(TokKind::Minus),
            b'*' => self.one(TokKind::Star),
            b'/' => self.one(TokKind::Slash),
            b'^' => self.one(TokKind::Caret),
            b'&' => self.one(TokKind::Amp),
            b'%' => self.one(TokKind::Percent),
            b'=' => self.one(TokKind::Eq),
            b'<' => {
                self.pos += 1;
                match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        Ok(TokKind::Le)
                    }
                    Some(b'>') => {
                        self.pos += 1;
                        Ok(TokKind::Ne)
                    }
                    _ => Ok(TokKind::Lt),
                }
            }
            b'>' => {
                self.pos += 1;
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Ok(TokKind::Ge)
                } else {
                    Ok(TokKind::Gt)
                }
            }
            // `$` or a letter begins a cell ref or an identifier.
            b'$' | b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.lex_ref_or_ident(),
            _ => {
                let start = self.pos;
                // Span the offending UTF-8 char.
                let ch_len = utf8_len(b);
                self.pos = (self.pos + ch_len).min(self.bytes.len());
                Err(ParseError::new(
                    format!("unexpected character {:?}", &self.src[start..self.pos]),
                    start..self.pos,
                ))
            }
        }
    }

    fn one(&mut self, k: TokKind) -> Result<TokKind, ParseError> {
        self.pos += 1;
        Ok(k)
    }

    /// `"..."`, with `""` meaning a literal `"`.
    fn lex_string(&mut self) -> Result<TokKind, ParseError> {
        let start = self.pos;
        self.pos += 1; // opening quote
        let mut s = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::new(
                        "unterminated string literal",
                        start..self.pos,
                    ))
                }
                Some(b'"') => {
                    if self.bytes.get(self.pos + 1) == Some(&b'"') {
                        s.push('"');
                        self.pos += 2;
                    } else {
                        self.pos += 1; // closing quote
                        return Ok(TokKind::Str(s));
                    }
                }
                Some(_) => {
                    // Copy one whole UTF-8 char.
                    let ch_len = utf8_len(self.bytes[self.pos]);
                    let end = (self.pos + ch_len).min(self.bytes.len());
                    s.push_str(&self.src[self.pos..end]);
                    self.pos = end;
                }
            }
        }
    }

    /// `#DIV/0!`, `#N/A`, etc. — reuse [`sheet_core::CellError::parse`].
    fn lex_error_literal(&mut self) -> Result<TokKind, ParseError> {
        let start = self.pos;
        self.pos += 1; // '#'
                       // Error tokens are short; consume the maximal run of allowed bytes
                       // then try the longest matching error code (some are prefixes:
                       // `#NULL!` vs nothing, `#N/A` ends in a letter not `!`).
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || matches!(c, b'/' | b'!' | b'?') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = &self.src[start..self.pos];
        // `#N/A` has no trailing punctuation; the run above already stops at
        // the right place because the following char is not in the set.
        match sheet_core::CellError::parse(raw) {
            Some(e) => Ok(TokKind::Error(e)),
            None => Err(ParseError::new(
                format!("invalid error literal {raw:?}"),
                start..self.pos,
            )),
        }
    }

    /// `'Quoted''Sheet'!` — a sheet qualifier whose name needs quoting.
    fn lex_quoted_sheet(&mut self) -> Result<TokKind, ParseError> {
        let start = self.pos;
        self.pos += 1; // opening '
        let mut name = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::new(
                        "unterminated quoted sheet name",
                        start..self.pos,
                    ))
                }
                Some(b'\'') => {
                    if self.bytes.get(self.pos + 1) == Some(&b'\'') {
                        name.push('\'');
                        self.pos += 2;
                    } else {
                        self.pos += 1; // closing '
                        break;
                    }
                }
                Some(_) => {
                    let ch_len = utf8_len(self.bytes[self.pos]);
                    let end = (self.pos + ch_len).min(self.bytes.len());
                    name.push_str(&self.src[self.pos..end]);
                    self.pos = end;
                }
            }
        }
        // A quoted sheet must be followed by '!'.
        if self.peek() == Some(b'!') {
            self.pos += 1;
            Ok(TokKind::SheetQual(name))
        } else {
            Err(ParseError::new(
                "expected '!' after quoted sheet name",
                start..self.pos,
            ))
        }
    }

    /// A numeric literal, incl. a fractional part and scientific exponent.
    fn lex_number(&mut self) -> TokKind {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            // Only consume the exponent if it is well-formed; otherwise the
            // `e`/`E` belongs to a following token (it cannot, since digits
            // were just seen — but a trailing sign needs a digit).
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            } else {
                self.pos = save; // not an exponent after all
            }
        }
        // The grammar above only emits valid f64 syntax.
        let n: f64 = self.src[start..self.pos].parse().unwrap();
        TokKind::Number(n)
    }

    /// Either a sheet-qualified ref prefix (`Sheet1!`), a bare A1 cell, or a
    /// plain identifier. Disambiguation: read the maximal `$`/letter/digit
    /// run; if it is followed by `!` it is an unquoted sheet qualifier; else
    /// if it parses as A1 it is a `Cell`; else it is an `Ident`.
    fn lex_ref_or_ident(&mut self) -> Result<TokKind, ParseError> {
        let start = self.pos;

        // First: scan an unquoted sheet name candidate (letters, digits,
        // `_`, `.`) NOT containing `$` — only if it is terminated by `!`.
        let mut j = self.pos;
        while j < self.bytes.len() {
            let c = self.bytes[j];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
                j += 1;
            } else {
                break;
            }
        }
        if self.bytes.get(j) == Some(&b'!') && j > self.pos && self.bytes[self.pos] != b'$' {
            let name = self.src[self.pos..j].to_string();
            self.pos = j + 1; // consume the '!'
            return Ok(TokKind::SheetQual(name));
        }

        // Otherwise: consume an A1/identifier run including a leading `$`,
        // letters, digits, `.`, and `_` (a `$` may also appear mid-token for
        // `$B$7`). We then classify.
        let mut k = self.pos;
        while k < self.bytes.len() {
            let c = self.bytes[k];
            if c.is_ascii_alphanumeric() || c == b'$' || c == b'_' || c == b'.' {
                k += 1;
            } else {
                break;
            }
        }
        let tok = &self.src[self.pos..k];
        self.pos = k;

        // TRUE / FALSE keywords (only when not a function call — the parser
        // re-routes `TRUE(` to a function by re-reading the ident; but a bare
        // `TRUE` followed by `(` cannot happen because `(` ends this run).
        if tok.eq_ignore_ascii_case("TRUE") && self.peek() != Some(b'(') {
            return Ok(TokKind::Bool(true));
        }
        if tok.eq_ignore_ascii_case("FALSE") && self.peek() != Some(b'(') {
            return Ok(TokKind::Bool(false));
        }

        // A1 cell? — but a run immediately followed by `(` is a CALL, never a
        // ref: `LOG10` is the one registered name that is also a valid A1
        // address (cols L-O-G, row 10), and Excel itself resolves `LOG10(` as
        // the function. The `(`-peek keeps `LOG10(8)` a call while a bare
        // `LOG10` stays the cell ref (matching Excel's own disambiguation).
        if self.peek() != Some(b'(') {
            if let Some((row, col, row_abs, col_abs)) = sheet_core::parse_a1(tok) {
                return Ok(TokKind::Cell {
                    row,
                    col,
                    row_abs,
                    col_abs,
                });
            }
        }

        // Plain identifier (name or function). Reject a `$` here: `$foo` is
        // neither a valid cell nor a valid name.
        if tok.contains('$') {
            return Err(ParseError::new(
                format!("invalid reference or name {tok:?}"),
                start..self.pos,
            ));
        }
        Ok(TokKind::Ident(tok.to_string()))
    }
}

/// Byte length of the UTF-8 char starting with lead byte `b`.
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}
