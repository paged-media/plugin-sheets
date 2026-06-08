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

use crate::sections::{
    CompareOp, CompiledFormat, Condition, ElapsedUnit, FormatColor, FractionSpec, Section,
    SectionKind, Token,
};
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
            pos: Section::general_section(),
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
    let pos = iter.next().unwrap_or_else(Section::general_section);
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
    // `General` (case-insensitive), possibly preceded by a color/condition
    // bracket, compiles to a general-flagged number section. We must still
    // scan the brackets to carry the color/condition (e.g. `[Red]General`).
    let mut color: Option<FormatColor> = None;
    let mut condition: Option<Condition> = None;

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
                // A bracket run. Capture its body and classify it (M1, spec
                // §9): elapsed-time accumulators emit a token; color and
                // condition brackets become section attributes; currency/locale
                // tokens lower to a literal currency symbol; anything else is
                // dropped (e.g. an unmodelled `[ColorN]` palette index).
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                let body: String = chars[start..j].iter().collect();
                i = j + 1; // past the closing ']'
                match classify_bracket(&body) {
                    Bracket::Elapsed(tok) => {
                        flush_lit!();
                        tokens.push(tok);
                    }
                    Bracket::Color(c) => color = Some(c),
                    Bracket::Condition(cond) => condition = Some(cond),
                    Bracket::Currency(sym) => lit.push_str(&sym),
                    Bracket::Drop => {}
                }
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
                // `*x` = repeat-fill char (ruling `sheet.format.padding`). The
                // engine has no column width, so T0 emits the fill char ONCE
                // (documented in the ruling).
                i += 1;
                if i < chars.len() {
                    flush_lit!();
                    tokens.push(Token::Fill(chars[i]));
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
            '/' => {
                // A `/` is a slash literal, BUT between digit placeholders it
                // is the bar of a fraction (`# ?/?`). Keep it as its own
                // single-char literal so the fraction post-pass can recognize
                // the `digits / digits` pattern (ruling `sheet.format.fractions`).
                flush_lit!();
                tokens.push(Token::Literal(CompactString::new("/")));
                i += 1;
            }
            // Pass-through literal punctuation Excel allows unquoted.
            '$' | '+' | '-' | '(' | ')' | ' ' | ':' => {
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

    // `General` (case-insensitive) as the sole content — possibly wrapped in a
    // color/condition bracket (e.g. `[Red]General`, `[>0]General`) — compiles
    // to a general-flagged section that still carries its color/condition.
    if tokens.len() == 1 {
        if let Token::Literal(s) = &tokens[0] {
            if s.trim().eq_ignore_ascii_case("general") {
                return Ok(Section {
                    tokens: vec![],
                    kind: SectionKind::Number,
                    general: true,
                    color,
                    condition,
                });
            }
        }
    }

    resolve_month_minute(&mut tokens);
    let kind = classify(&tokens, force_text);
    if kind == SectionKind::DateTime {
        // In a date/time section, `,` is a literal comma, not a thousands
        // separator (e.g. `mmm d, yyyy`). Numeric grouping has no meaning here.
        // A `/` is a real date separator here, not a fraction bar.
        for t in &mut tokens {
            if matches!(t, Token::ThousandsSep) {
                *t = Token::Literal(CompactString::new(","));
            }
        }
    } else if kind == SectionKind::Number {
        // Number sections may carry a `# ?/?` fraction. Recognize and rewrite
        // the `digits / digits` pattern into a single Token::Fraction.
        resolve_fraction(&mut tokens);
    }
    Ok(Section {
        tokens,
        kind,
        general: false,
        color,
        condition,
    })
}

/// Result of classifying a `[...]` bracket body.
enum Bracket {
    /// An elapsed-time accumulator (`[h]`, `[mm]`, `[ss]`).
    Elapsed(Token),
    /// A named color (`[Red]`).
    Color(FormatColor),
    /// A comparison condition (`[<100]`).
    Condition(Condition),
    /// A currency/locale token (`[$$-409]`, `[$€-407]`) — lowers to a literal
    /// currency symbol.
    Currency(String),
    /// Anything unmodelled (an indexed `[Color12]`, a bare locale id) — dropped.
    Drop,
}

/// Classify a `[...]` bracket body (without the brackets) per spec §9.
/// Order matters: elapsed-time first, then color, condition, currency.
fn classify_bracket(body: &str) -> Bracket {
    let trimmed = body.trim();
    // Elapsed-time accumulator: a run of one elapsed letter (h/m/s),
    // case-insensitive, nothing else.
    if let Some(tok) = parse_elapsed(trimmed) {
        return Bracket::Elapsed(tok);
    }
    if let Some(c) = FormatColor::parse(trimmed) {
        return Bracket::Color(c);
    }
    if let Some(cond) = parse_condition(trimmed) {
        return Bracket::Condition(cond);
    }
    if let Some(sym) = parse_currency(trimmed) {
        return Bracket::Currency(sym);
    }
    Bracket::Drop
}

/// `[h]`/`[hh]`/`[m]`/`[mm]`/`[s]`/`[ss]` -> an [`Token::Elapsed`]. The body
/// must be a non-empty run of a SINGLE elapsed letter (case-insensitive).
fn parse_elapsed(body: &str) -> Option<Token> {
    let mut chars = body.chars();
    let first = chars.next()?.to_ascii_lowercase();
    let unit = match first {
        'h' => ElapsedUnit::Hours,
        'm' => ElapsedUnit::Minutes,
        's' => ElapsedUnit::Seconds,
        _ => return None,
    };
    let mut count = 1usize;
    for c in chars {
        if c.to_ascii_lowercase() != first {
            return None;
        }
        count += 1;
    }
    Some(Token::Elapsed { unit, pad: count })
}

/// `[<100]`, `[>=0]`, `[=5]`, `[<>0]` -> a [`Condition`]. Returns `None` for a
/// body that does not start with a comparison operator.
fn parse_condition(body: &str) -> Option<Condition> {
    let b = body.trim();
    // Longest operators first so `<=`/`>=`/`<>` beat `<`/`>`/`=`.
    let (op, rest) = if let Some(r) = b.strip_prefix("<=") {
        (CompareOp::Le, r)
    } else if let Some(r) = b.strip_prefix(">=") {
        (CompareOp::Ge, r)
    } else if let Some(r) = b.strip_prefix("<>") {
        (CompareOp::Ne, r)
    } else if let Some(r) = b.strip_prefix('<') {
        (CompareOp::Lt, r)
    } else if let Some(r) = b.strip_prefix('>') {
        (CompareOp::Gt, r)
    } else if let Some(r) = b.strip_prefix('=') {
        (CompareOp::Eq, r)
    } else {
        return None;
    };
    let rhs: f64 = rest.trim().parse().ok()?;
    Some(Condition { op, rhs })
}

/// `[$$-409]`, `[$€-407]`, `[$USD]` -> the currency symbol (everything between
/// the `$` and the `-locale` suffix). Returns `None` for a body not starting
/// with `$` (spec §9; ruling `sheet.format.locale-currency-token`).
///
/// Excel's grammar is `[$<symbol>-<locale-hex>]`; the symbol may be empty
/// (`[$-409]`, a pure locale tag, which contributes no literal) or multi-char
/// (`[$USD]`). We emit exactly the symbol portion.
fn parse_currency(body: &str) -> Option<String> {
    let rest = body.strip_prefix('$')?;
    // The symbol runs up to the FIRST `-` (the locale separator), if any.
    let symbol = match rest.find('-') {
        Some(idx) => &rest[..idx],
        None => rest,
    };
    Some(symbol.to_string())
}

/// Rewrite a `digits Literal("/") digits` run into a single [`Token::Fraction`]
/// (ruling `sheet.format.fractions`). Recognizes both the fitted form
/// (`# ?/?`, `# ??/??`) and the fixed-denominator form (`# ?/16`). The integer
/// part (digit placeholders BEFORE the numerator group, e.g. the `#` in
/// `# ?/?`) is left in place; the renderer reads the [`FractionSpec`] to fit.
fn resolve_fraction(tokens: &mut Vec<Token>) {
    // Find the slash literal.
    let slash = tokens
        .iter()
        .position(|t| matches!(t, Token::Literal(s) if s.as_str() == "/"));
    let Some(slash) = slash else { return };

    fn is_digit(t: &Token) -> bool {
        matches!(t, Token::DigitZero | Token::DigitHash | Token::DigitSpace)
    }

    // Numerator placeholders: the contiguous digit run immediately left of `/`.
    let mut num_start = slash;
    while num_start > 0 && is_digit(&tokens[num_start - 1]) {
        num_start -= 1;
    }
    let num_digits = slash - num_start;

    // Denominator: either a contiguous digit-placeholder run, or a literal
    // integer (the fixed-denominator form). Examine the tokens right of `/`.
    let mut den_end = slash + 1;
    while den_end < tokens.len() && is_digit(&tokens[den_end]) {
        den_end += 1;
    }
    let den_digits = den_end - (slash + 1);

    // A real fraction needs at least one numerator placeholder and a
    // denominator (placeholders OR a literal number).
    let mut fixed: Option<u32> = None;
    if num_digits == 0 {
        return; // not a fraction (e.g. a stray slash literal)
    }
    if den_digits == 0 {
        // Fixed denominator: the token right of `/` must be a literal integer.
        match tokens.get(slash + 1) {
            Some(Token::Literal(s)) => match s.parse::<u32>() {
                Ok(d) if d > 0 => {
                    fixed = Some(d);
                    den_end = slash + 2;
                }
                _ => return,
            },
            _ => return,
        }
    }

    let spec = FractionSpec {
        num_digits,
        den_digits,
        fixed,
    };

    // Replace [num_start .. den_end) with a single Fraction token.
    let replacement = Token::Fraction(spec);
    tokens.splice(num_start..den_end, std::iter::once(replacement));
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
    // Indices that are date/time-relevant for neighbour scanning. An elapsed
    // `[h]`/`[s]` accumulator also makes an adjacent `m`/`mm` a minute (e.g.
    // `[h]:mm` is elapsed-hours:minutes).
    fn is_hms(t: &Token) -> bool {
        matches!(
            t,
            Token::Hour { .. }
                | Token::Second { .. }
                | Token::Elapsed {
                    unit: ElapsedUnit::Hours | ElapsedUnit::Seconds,
                    ..
                }
        )
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
            | Token::Elapsed { .. }
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
            | Token::Fraction(_)
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
