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

//! Format-code parser: text -> [`CompiledFormat`] (spec §9; ECMA-376
//! §18.8.31).
//!
//! Splits on top-level `;` into up to four sections, lexes each into a
//! [`Token`] stream, classifies it ([`SectionKind`]), then resolves the
//! `m`/`mm` month-vs-minute ambiguity by the standard adjacency rule: an
//! `m`/`mm` token is **minutes** when the nearest non-literal date/time
//! neighbour (looking outward in either direction) is an hour (`h`) or
//! second (`s`) token; otherwise it is a **month**.

use crate::sections::{CompiledFormat, Section, SectionKind, Token};
use compact_str::CompactString;

/// Compile-time error from [`compile`] (public API; `thiserror`).
#[derive(Debug, thiserror::Error)]
#[error("format code error: {message}")]
pub struct FormatError {
    /// Human-readable diagnostic.
    pub message: String,
}

impl FormatError {
    fn new(message: impl Into<String>) -> Self {
        FormatError {
            message: message.into(),
        }
    }
}

/// Parse an ECMA-376 number-format code into a [`CompiledFormat`] (spec §9).
/// Empty input is treated as `General` (a single passthrough number
/// section). More than four sections is an error.
pub fn compile(code: &str) -> Result<CompiledFormat, FormatError> {
    // An empty whole code is General.
    if code.is_empty() {
        return Ok(CompiledFormat {
            pos: Section {
                tokens: vec![],
                kind: SectionKind::Number,
                general: true,
            },
            neg: None,
            zero: None,
            text: None,
        });
    }
    let raw_sections = split_sections(code);
    if raw_sections.len() > 4 {
        return Err(FormatError::new(format!(
            "format code has {} sections (max 4)",
            raw_sections.len()
        )));
    }

    let mut sections: Vec<Section> = Vec::with_capacity(raw_sections.len());
    for (i, raw) in raw_sections.iter().enumerate() {
        // The 4th section is always a text section by position.
        let force_text = i == 3;
        sections.push(parse_section(raw, force_text)?);
    }

    let mut iter = sections.into_iter();
    let pos = iter.next().unwrap_or_else(|| Section {
        tokens: vec![],
        kind: SectionKind::Number,
        general: true,
    });
    let neg = iter.next();
    let zero = iter.next();
    let text = iter.next();

    Ok(CompiledFormat {
        pos,
        neg,
        zero,
        text,
    })
}

/// Split on top-level `;`, honouring quotes and backslash escapes. Bracketed
/// `[...]` runs (color/condition/locale) are skipped over for the split but
/// otherwise dropped at lex time in T0.
fn split_sections(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = code.chars().peekable();
    let mut in_quote = false;
    let mut depth = 0u32; // bracket nesting
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            '\\' if !in_quote => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '[' if !in_quote => {
                depth += 1;
                cur.push(c);
            }
            ']' if !in_quote => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ';' if !in_quote && depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Lex one section into tokens, classify it, and resolve month/minute.
fn parse_section(raw: &str, force_text: bool) -> Result<Section, FormatError> {
    // `General` (case-insensitive) compiles to an empty, general-flagged
    // number section, which the renderer treats as General.
    if raw.trim().eq_ignore_ascii_case("general") {
        return Ok(Section {
            tokens: vec![],
            kind: SectionKind::Number,
            general: true,
        });
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut lit = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;

    macro_rules! flush_lit {
        () => {
            if !lit.is_empty() {
                tokens.push(Token::Literal(CompactString::new(&lit)));
                lit.clear();
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                // Quoted literal run.
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    lit.push(chars[i]);
                    i += 1;
                }
                i += 1; // closing quote (or end)
            }
            '\\' => {
                // Backslash escapes the next char as a literal.
                i += 1;
                if i < chars.len() {
                    lit.push(chars[i]);
                    i += 1;
                }
            }
            '[' => {
                // Color / condition / locale bracket — dropped in T0.
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                i += 1; // closing ']'
            }
            '_' => {
                // `_x` = a space the width of x. T0 approximates as a space.
                i += 1;
                if i < chars.len() {
                    i += 1;
                }
                lit.push(' ');
            }
            '*' => {
                // `*x` = fill repeat. T0 drops the fill (no column width).
                i += 1;
                if i < chars.len() {
                    i += 1;
                }
            }
            '@' => {
                flush_lit!();
                tokens.push(Token::TextPlaceholder);
                i += 1;
            }
            '0' => {
                flush_lit!();
                tokens.push(Token::DigitZero);
                i += 1;
            }
            '#' => {
                flush_lit!();
                tokens.push(Token::DigitHash);
                i += 1;
            }
            '?' => {
                flush_lit!();
                tokens.push(Token::DigitSpace);
                i += 1;
            }
            '.' => {
                flush_lit!();
                tokens.push(Token::DecimalPoint);
                i += 1;
            }
            ',' => {
                flush_lit!();
                tokens.push(Token::ThousandsSep);
                i += 1;
            }
            '%' => {
                flush_lit!();
                tokens.push(Token::Percent);
                i += 1;
            }
            'E' | 'e' => {
                // Scientific: E+ / E- (next char is the sign).
                if i + 1 < chars.len() && (chars[i + 1] == '+' || chars[i + 1] == '-') {
                    flush_lit!();
                    tokens.push(Token::Exponent {
                        plus: chars[i + 1] == '+',
                    });
                    i += 2;
                } else {
                    lit.push(c);
                    i += 1;
                }
            }
            // Date/time letters (case-insensitive run of the same letter).
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => {
                flush_lit!();
                let lower = c.to_ascii_lowercase();
                let mut count = 0usize;
                while i < chars.len() && chars[i].to_ascii_lowercase() == lower {
                    count += 1;
                    i += 1;
                }
                tokens.push(lex_datetime(lower, count));
            }
            'a' | 'A' => {
                // AM/PM handling: "AM/PM" or "A/P".
                if let Some((tok, consumed)) = lex_ampm(&chars, i) {
                    flush_lit!();
                    tokens.push(tok);
                    i += consumed;
                } else {
                    lit.push(c);
                    i += 1;
                }
            }
            // Pass-through literal punctuation Excel allows unquoted.
            '$' | '+' | '-' | '/' | '(' | ')' | ' ' | ':' => {
                lit.push(c);
                i += 1;
            }
            _ => {
                lit.push(c);
                i += 1;
            }
        }
    }
    flush_lit!();

    resolve_month_minute(&mut tokens);
    let kind = classify(&tokens, force_text);
    if kind == SectionKind::DateTime {
        // In a date/time section, `,` is a literal comma, not a thousands
        // separator (e.g. `mmm d, yyyy`). Numeric grouping has no meaning here.
        for t in &mut tokens {
            if matches!(t, Token::ThousandsSep) {
                *t = Token::Literal(CompactString::new(","));
            }
        }
    }
    Ok(Section {
        tokens,
        kind,
        general: false,
    })
}

/// Build the date/time token for a letter run (month/minute is provisional —
/// stored as [`Token::Month`] and resolved later by adjacency).
fn lex_datetime(letter: char, count: usize) -> Token {
    match letter {
        'y' => {
            if count <= 2 {
                Token::Year2
            } else {
                Token::Year4
            }
        }
        'm' => match count {
            1 => Token::Month { count: 1 },
            2 => Token::Month { count: 2 },
            3 => Token::MonthName { full: false },
            _ => Token::MonthName { full: true }, // mmmm / mmmmm -> full (T0)
        },
        'd' => match count {
            1 => Token::Day { pad: false },
            2 => Token::Day { pad: true },
            3 => Token::DayName { full: false },
            _ => Token::DayName { full: true },
        },
        'h' => Token::Hour { pad: count >= 2 },
        's' => Token::Second { pad: count >= 2 },
        _ => Token::Literal(CompactString::new("")),
    }
}

/// Recognize an AM/PM marker starting at `i`. Returns the token and the
/// number of chars consumed.
fn lex_ampm(chars: &[char], i: usize) -> Option<(Token, usize)> {
    let rest: String = chars[i..].iter().collect();
    let up = rest.to_ascii_uppercase();
    if up.starts_with("AM/PM") {
        Some((Token::AmPm { long: true }, 5))
    } else if up.starts_with("A/P") {
        Some((Token::AmPm { long: false }, 3))
    } else {
        None
    }
}

/// Resolve every provisional [`Token::Month`] to either a real month or a
/// [`Token::Minute`] using the adjacency rule (spec §9): minutes when the
/// nearest date/time neighbour (skipping literals) on either side is an hour
/// or second token.
fn resolve_month_minute(tokens: &mut [Token]) {
    // Indices that are date/time-relevant for neighbour scanning.
    fn is_hms(t: &Token) -> bool {
        matches!(t, Token::Hour { .. } | Token::Second { .. })
    }
    let n = tokens.len();
    for idx in 0..n {
        let count = match &tokens[idx] {
            Token::Month { count } => *count,
            _ => continue,
        };
        // Scan left for the nearest non-literal date/time token.
        let mut minute = false;
        // left
        let mut j = idx;
        while j > 0 {
            j -= 1;
            if matches!(tokens[j], Token::Literal(_) | Token::AmPm { .. }) {
                continue;
            }
            if is_hms(&tokens[j]) {
                minute = true;
            }
            break;
        }
        // right (only if not already decided)
        if !minute {
            let mut k = idx + 1;
            while k < n {
                if matches!(tokens[k], Token::Literal(_) | Token::AmPm { .. }) {
                    k += 1;
                    continue;
                }
                if is_hms(&tokens[k]) {
                    minute = true;
                }
                break;
            }
        }
        if minute {
            tokens[idx] = Token::Minute { pad: count >= 2 };
        }
    }
}

/// Classify a token stream into a [`SectionKind`].
fn classify(tokens: &[Token], force_text: bool) -> SectionKind {
    let has_text = tokens.iter().any(|t| matches!(t, Token::TextPlaceholder));
    if force_text || has_text {
        // A 4th section, or any section with @, is text — UNLESS it also
        // carries numeric/date structure (rare in T0); text wins for @.
        let has_datetime = tokens.iter().any(is_datetime);
        let has_number = tokens.iter().any(is_number_struct);
        if has_text || (!has_datetime && !has_number) {
            return SectionKind::Text;
        }
    }
    if tokens.iter().any(is_datetime) {
        return SectionKind::DateTime;
    }
    SectionKind::Number
}

fn is_datetime(t: &Token) -> bool {
    matches!(
        t,
        Token::Year4
            | Token::Year2
            | Token::Month { .. }
            | Token::MonthName { .. }
            | Token::Minute { .. }
            | Token::Day { .. }
            | Token::DayName { .. }
            | Token::Hour { .. }
            | Token::Second { .. }
            | Token::AmPm { .. }
    )
}

fn is_number_struct(t: &Token) -> bool {
    matches!(
        t,
        Token::DigitZero
            | Token::DigitHash
            | Token::DigitSpace
            | Token::DecimalPoint
            | Token::Percent
            | Token::Exponent { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sections::SectionKind;

    #[test]
    fn single_section_is_number() {
        let f = compile("0.00").unwrap();
        assert_eq!(f.pos.kind, SectionKind::Number);
        assert!(f.neg.is_none());
        assert_eq!(f.pos.decimals(), 2);
    }

    #[test]
    fn four_sections() {
        let f = compile("0;(0);\"-\";@").unwrap();
        assert!(f.neg.is_some());
        assert!(f.zero.is_some());
        assert_eq!(f.text.as_ref().unwrap().kind, SectionKind::Text);
    }

    #[test]
    fn too_many_sections_errors() {
        assert!(compile("0;0;0;0;0").is_err());
    }

    #[test]
    fn month_vs_minute_adjacency() {
        // m after h -> minute; m before / standalone -> month.
        let f = compile("h:mm").unwrap();
        assert!(matches!(f.pos.tokens.last().unwrap(), Token::Minute { .. }));

        let f = compile("mm/dd/yyyy").unwrap();
        assert!(f
            .pos
            .tokens
            .iter()
            .any(|t| matches!(t, Token::Month { .. })));

        // m before s -> minute.
        let f = compile("mm:ss").unwrap();
        assert!(matches!(f.pos.tokens[0], Token::Minute { .. }));
    }

    #[test]
    fn datetime_classified() {
        assert_eq!(
            compile("yyyy-mm-dd").unwrap().pos.kind,
            SectionKind::DateTime
        );
        assert_eq!(compile("hh:mm:ss").unwrap().pos.kind, SectionKind::DateTime);
    }

    #[test]
    fn quoted_and_escaped_literals() {
        let f = compile("\"USD \"0").unwrap();
        assert!(matches!(&f.pos.tokens[0], Token::Literal(s) if s == "USD "));
        let f = compile("\\$0").unwrap();
        assert!(matches!(&f.pos.tokens[0], Token::Literal(s) if s == "$"));
    }

    #[test]
    fn general_compiles_empty_number() {
        let f = compile("General").unwrap();
        assert_eq!(f.pos.kind, SectionKind::Number);
        assert!(f.pos.tokens.is_empty());
    }
}
