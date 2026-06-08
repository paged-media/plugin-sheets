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

//! Structured (table) reference lexing (spec §6.4 / ECMA-376 §18.17.2.4,
//! Microsoft structured-reference syntax). A structured reference names an
//! Excel table region symbolically — `Table1[Col]`, `Table1[[#Headers],[Col]]`,
//! `Table1[[Col1]:[Col2]]`, the in-table `[@Col]`/`Table1[[#This Row],[Col]]`
//! forms — rather than by A1 geometry, so it is immune to structural rewrites.
//!
//! The bracket grammar is self-contained (it is not affected by operator
//! precedence), so the **lexer** consumes a whole structured reference into one
//! [`crate::lexer::TokKind::Structured`] token via [`lex_structured`]; the
//! parser then just lifts it into [`sheet_core::ast::Expr::StructuredRef`].
//! Printing (the inverse) lives in `print.rs` (frozen in M1 Phase A) — the
//! `parse ∘ print` fixpoint over these forms is property/golden tested.
//!
//! ## Grammar (the subset Excel emits, ECMA-376 §18.17.2.4)
//!
//! A structured ref is an optional table name immediately followed by a
//! bracketed *spec*:
//!
//! ```text
//! structured := table-name? "[" spec "]"
//! spec       := simple-column            ; Table1[Col]
//!             | area-keyword             ; Table1[#Totals]
//!             | "@" inner-col            ; Table1[@Col]   (ThisRow shorthand)
//!             | item ("," item)*         ; Table1[[#Headers],[Col]] etc.
//! item       := "[" area-keyword "]"     ; [#All] [#Data] [#Headers] [#Totals]
//!             | "[" "#This Row" "]"      ; ThisRow
//!             | "@"                      ; ThisRow marker inside a list
//!             | "[" col-name "]"         ; a column, possibly a span endpoint
//!             | "[" col "]" ":" "[" col "]"  ; a column span
//! ```
//!
//! ## Rulings (documented Excel-compat decisions, constitution §3)
//!
//! - **Column names may contain spaces and most punctuation.** Inside a `[ ]`
//!   column token, the special characters `[ ] # ' @` are escaped with a
//!   leading `'` (ECMA-376 §18.17.2.4); we un-escape on lex. A *simple* column
//!   (no inner brackets, e.g. `Table1[Col]`) runs to the closing `]` and may
//!   NOT itself contain `[`/`]`.
//! - **`#This Row` and the `@` marker both mean [`TableArea::ThisRow`].** The
//!   bare-name `[@Col]` form (empty table name) is the in-table shorthand the
//!   printer round-trips; with a table name and `@` we keep the table name.
//! - **At most one column span.** Excel permits exactly one `[Col1]:[Col2]`
//!   span (plus an optional area keyword). More than two columns, or a span
//!   mixed with a third column, is a parse error.
//! - **At most one area keyword.** Two `#`-specifiers in one ref is an error.

use compact_str::CompactString;
use sheet_core::ast::{StructuredRef, TableArea};

use crate::error::ParseError;
use crate::lexer::TokKind;

/// Try to lex a structured reference starting at `start` in `src`. `table` is
/// the (already-consumed) table name preceding the `[` — empty for the bare
/// `[@Col]` / `[[#Headers],[Col]]` forms. `bracket_pos` is the byte offset of
/// the opening `[`. On success returns the token plus the byte offset just
/// past the closing `]`.
///
/// `start` is only used to anchor error spans at the reference's beginning.
pub(crate) fn lex_structured(
    src: &str,
    start: usize,
    table: &str,
    bracket_pos: usize,
) -> Result<(TokKind, usize), ParseError> {
    let bytes = src.as_bytes();
    debug_assert_eq!(bytes.get(bracket_pos), Some(&b'['));

    // Find the matching closing `]` for the OUTER bracket, honoring `'`-escapes
    // and nested `[ ]` (a bracketed item list). The outer span is everything
    // between the outer `[` and its matching `]`.
    let body_start = bracket_pos + 1;
    let mut i = body_start;
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                // Escape: the next byte is literal, skip both.
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 {
        return Err(ParseError::new(
            "unterminated structured reference (missing ']')",
            start..src.len(),
        ));
    }
    let body = &src[body_start..i];
    let end = i + 1; // past the closing ']'

    let sref = parse_spec(body, table, start..end)?;
    Ok((TokKind::Structured(sref), end))
}

/// Parse the inner *spec* (the text between the outer `[ ]`) into a
/// [`StructuredRef`]. `span` is the whole reference's span (for errors).
fn parse_spec(
    body: &str,
    table: &str,
    span: std::ops::Range<usize>,
) -> Result<StructuredRef, ParseError> {
    let trimmed = body.trim();

    // Empty body `Table1[]` → the whole Data body (no column span).
    if trimmed.is_empty() {
        return Ok(StructuredRef {
            table: table.into(),
            area: TableArea::Data,
            col_start: None,
            col_end: None,
        });
    }

    // The `@` ThisRow shorthand: `[@Col]`, `[@[Col Name]]`, or `[@]`.
    if let Some(rest) = trimmed.strip_prefix('@') {
        return parse_thisrow_shorthand(rest, table, span);
    }

    // A bracketed item list `[..],[..]` (or a single bracketed item). The body
    // begins with `[` exactly when items are individually bracketed.
    if trimmed.starts_with('[') {
        return parse_item_list(trimmed, table, span);
    }

    // Otherwise the body is a SIMPLE token: either a bare `#`-area keyword
    // (`Table1[#Totals]`) or a simple column name (`Table1[Col]`).
    if let Some(area) = area_keyword(trimmed) {
        return Ok(StructuredRef {
            table: table.into(),
            area,
            col_start: None,
            col_end: None,
        });
    }
    // A simple column: un-escape and store as the Data-area single column.
    Ok(StructuredRef {
        table: table.into(),
        area: TableArea::Data,
        col_start: Some(unescape_col(trimmed)),
        col_end: None,
    })
}

/// Parse the `@…` ThisRow shorthand body (text after the leading `@`). `rest`
/// may be a bare column (`@Col`), a bracketed column (`@[Col Name]`), or empty
/// (`@`).
fn parse_thisrow_shorthand(
    rest: &str,
    table: &str,
    span: std::ops::Range<usize>,
) -> Result<StructuredRef, ParseError> {
    let rest = rest.trim();
    let col = if rest.is_empty() {
        None
    } else if rest.starts_with('[') {
        // `@[Col Name]` — exactly one bracketed column.
        let items = split_items(rest, &span)?;
        if items.len() != 1 {
            return Err(ParseError::new(
                "structured reference `@` accepts a single column",
                span,
            ));
        }
        match &items[0] {
            Item::Column(c) => Some(c.clone()),
            _ => {
                return Err(ParseError::new(
                    "structured reference `@` must name a column",
                    span,
                ))
            }
        }
    } else {
        // `@Col` — a bare simple column.
        Some(unescape_col(rest))
    };
    Ok(StructuredRef {
        table: table.into(),
        area: TableArea::ThisRow,
        col_start: col,
        col_end: None,
    })
}

/// One parsed bracketed item.
enum Item {
    /// `[#All]`, `[#Data]`, `[#Headers]`, `[#Totals]`, `[#This Row]`.
    Area(TableArea),
    /// A column name (un-escaped).
    Column(CompactString),
    /// The bare `@` ThisRow marker inside a list (`[@],[Col]` is rare but
    /// Excel does emit `[#This Row]` for it on print; we accept `@` too).
    ThisRow,
}

/// Parse a bracketed item list `[..],[..]` (optionally with one `:` span) into
/// a [`StructuredRef`].
fn parse_item_list(
    body: &str,
    table: &str,
    span: std::ops::Range<usize>,
) -> Result<StructuredRef, ParseError> {
    // A column span uses `:` BETWEEN two bracketed columns: `[A]:[B]`. Detect a
    // top-level `:` separating two bracketed items (not inside a `[ ]`).
    if let Some((lhs, rhs)) = split_span(body) {
        let left = split_items(lhs, &span)?;
        let right = split_items(rhs, &span)?;
        // The left side may carry an optional leading area keyword; the right
        // side must be exactly one column.
        let (area, c0) = take_area_and_one_col(left, &span)?;
        let c1 = take_one_col(right, &span)?;
        return Ok(StructuredRef {
            table: table.into(),
            area,
            col_start: Some(c0),
            col_end: Some(c1),
        });
    }

    // No span: a comma-separated list of items. Fold an optional area keyword
    // plus at most one column.
    let items = split_items(body, &span)?;
    fold_items(items, table, span)
}

/// Fold a flat (no-span) item list into a [`StructuredRef`]: at most one area
/// keyword and at most one column.
fn fold_items(
    items: Vec<Item>,
    table: &str,
    span: std::ops::Range<usize>,
) -> Result<StructuredRef, ParseError> {
    let mut area: Option<TableArea> = None;
    let mut col: Option<CompactString> = None;
    for it in items {
        match it {
            Item::Area(a) => set_area(&mut area, a, &span)?,
            Item::ThisRow => set_area(&mut area, TableArea::ThisRow, &span)?,
            Item::Column(c) => {
                if col.is_some() {
                    return Err(ParseError::new(
                        "structured reference has more than one column (use a span `[A]:[B]`)",
                        span,
                    ));
                }
                col = Some(c);
            }
        }
    }
    Ok(StructuredRef {
        table: table.into(),
        area: area.unwrap_or(TableArea::Data),
        col_start: col,
        col_end: None,
    })
}

/// Set the (single) area keyword, erroring on a second one.
fn set_area(
    slot: &mut Option<TableArea>,
    a: TableArea,
    span: &std::ops::Range<usize>,
) -> Result<(), ParseError> {
    if slot.is_some() {
        return Err(ParseError::new(
            "structured reference has more than one #-area specifier",
            span.clone(),
        ));
    }
    *slot = Some(a);
    Ok(())
}

/// From the LEFT side of a span: an optional area keyword plus exactly one
/// column endpoint.
fn take_area_and_one_col(
    items: Vec<Item>,
    span: &std::ops::Range<usize>,
) -> Result<(TableArea, CompactString), ParseError> {
    let mut area = TableArea::Data;
    let mut area_set = false;
    let mut col: Option<CompactString> = None;
    for it in items {
        match it {
            Item::Area(a) => {
                if area_set {
                    return Err(ParseError::new(
                        "structured reference has more than one #-area specifier",
                        span.clone(),
                    ));
                }
                area = a;
                area_set = true;
            }
            Item::ThisRow => {
                if area_set {
                    return Err(ParseError::new(
                        "structured reference has more than one #-area specifier",
                        span.clone(),
                    ));
                }
                area = TableArea::ThisRow;
                area_set = true;
            }
            Item::Column(c) => {
                if col.is_some() {
                    return Err(ParseError::new(
                        "structured reference span left side has two columns",
                        span.clone(),
                    ));
                }
                col = Some(c);
            }
        }
    }
    match col {
        Some(c) => Ok((area, c)),
        None => Err(ParseError::new(
            "structured reference span is missing its first column",
            span.clone(),
        )),
    }
}

/// From the RIGHT side of a span: exactly one column, no area keyword.
fn take_one_col(
    items: Vec<Item>,
    span: &std::ops::Range<usize>,
) -> Result<CompactString, ParseError> {
    if items.len() != 1 {
        return Err(ParseError::new(
            "structured reference span right side must be a single column",
            span.clone(),
        ));
    }
    match items.into_iter().next() {
        Some(Item::Column(c)) => Ok(c),
        _ => Err(ParseError::new(
            "structured reference span right side must be a column",
            span.clone(),
        )),
    }
}

/// Split a bracketed-item body into individual [`Item`]s on top-level `,`
/// separators. Each item is a `[ ... ]` group (or a bare `@` marker).
fn split_items(body: &str, span: &std::ops::Range<usize>) -> Result<Vec<Item>, ParseError> {
    let bytes = body.as_bytes();
    let mut items = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Skip separators / whitespace between items.
        while i < bytes.len() && (bytes[i] == b',' || bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'@' {
            items.push(Item::ThisRow);
            i += 1;
            continue;
        }
        if bytes[i] != b'[' {
            return Err(ParseError::new(
                "expected `[` to start a structured-reference item",
                span.clone(),
            ));
        }
        // Consume one `[ ... ]` group, honoring `'`-escapes.
        let group_start = i;
        i += 1;
        let inner_start = i;
        while i < bytes.len() {
            match bytes[i] {
                b'\'' => {
                    i += 2;
                    continue;
                }
                b']' => break,
                _ => {}
            }
            i += 1;
        }
        if i >= bytes.len() {
            return Err(ParseError::new(
                "unterminated structured-reference item (missing ']')",
                span.clone(),
            ));
        }
        let inner = &body[inner_start..i];
        i += 1; // past ']'
        let _ = group_start;
        items.push(parse_item(inner)?);
    }
    Ok(items)
}

/// Parse one bracketed item's inner text (already without its `[ ]`).
fn parse_item(inner: &str) -> Result<Item, ParseError> {
    let t = inner.trim();
    if let Some(area) = area_keyword(t) {
        // `#This Row` is its own ThisRow area; the others map directly.
        return Ok(Item::Area(area));
    }
    Ok(Item::Column(unescape_col(t)))
}

/// Find a TOP-LEVEL `:` span separator (between two bracketed groups), if any.
/// Returns the `(left, right)` substrings. A `:` inside a `[ ]` group (a column
/// name may contain `:`) is ignored.
fn split_span(body: &str) -> Option<(&str, &str)> {
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut in_group = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if in_group => {
                i += 2;
                continue;
            }
            b'[' => in_group = true,
            b']' => in_group = false,
            b':' if !in_group => {
                return Some((&body[..i], &body[i + 1..]));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Map a `#`-area keyword spelling to its [`TableArea`] (case-insensitive;
/// `#This Row` tolerates collapsed internal whitespace). `None` if the text is
/// not a `#`-keyword (so it must be a column name).
fn area_keyword(s: &str) -> Option<TableArea> {
    let s = s.trim();
    if !s.starts_with('#') {
        return None;
    }
    let body = &s[1..];
    if body.eq_ignore_ascii_case("All") {
        Some(TableArea::All)
    } else if body.eq_ignore_ascii_case("Data") {
        Some(TableArea::Data)
    } else if body.eq_ignore_ascii_case("Headers") {
        Some(TableArea::Headers)
    } else if body.eq_ignore_ascii_case("Totals") {
        Some(TableArea::Totals)
    } else if is_this_row(body) {
        Some(TableArea::ThisRow)
    } else {
        // An unknown `#`-keyword: still not a column (column names cannot start
        // with `#` unescaped). Treat as ThisRow only for the exact spelling;
        // otherwise None lets the caller surface a column-name path, which for
        // a leading `#` is itself invalid — but Excel only emits the five.
        None
    }
}

/// True for `This Row` ignoring case and internal whitespace runs.
fn is_this_row(body: &str) -> bool {
    let collapsed: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.eq_ignore_ascii_case("This Row")
}

/// Un-escape a structured-ref column token: a `'` escapes the next character
/// (`'[`, `']`, `'#`, `'\''`, `'@`). Surrounding whitespace is trimmed; INNER
/// spaces are preserved (column names may contain spaces).
fn unescape_col(s: &str) -> CompactString {
    let s = s.trim();
    if !s.contains('\'') {
        return CompactString::new(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    CompactString::new(out.trim())
}
