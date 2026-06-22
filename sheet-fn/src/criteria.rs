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

//! Criteria matching shared by `SUMIF`/`COUNTIF`/`AVERAGEIF` and their `*S`
//! cousins (spec §7). A criterion is a single [`CellValue`] argument that
//! Excel interprets as one of:
//! - a **comparison** with an operator prefix — `>`, `<`, `>=`, `<=`, `<>`,
//!   `=` — against the remainder (number-first, then text);
//! - a **bare value** — equality, with Excel's number↔text equality (the
//!   text `"5"` matches the number `5`); bare text additionally honors `*`/`?`
//!   wildcards;
//! - a **wildcard text pattern** under `=`/no operator: `*` (any run), `?`
//!   (any one char), `~` escapes the next metacharacter. Matching is
//!   case-insensitive.
//!
//! [`parse_criteria`] compiles the argument once into a [`Criteria`];
//! [`matches`] then tests each candidate cell. The family kernels own the
//! iteration; this module owns the *ruling* on what a criterion means.

use compact_str::CompactString;
use sheet_core::CellValue;

use crate::coerce;

/// A relational operator extracted from a criterion prefix.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A compiled criterion (the output of [`parse_criteria`], consumed by
/// [`matches`]). Opaque: its shape is an implementation detail of the
/// SUMIF/COUNTIF ruling, so callers only ever build one and test against it.
#[derive(Clone, Debug)]
pub struct Criteria(Kind);

/// The internal shape of a compiled [`Criteria`].
#[derive(Clone, Debug)]
enum Kind {
    /// Numeric comparison: candidate (coerced to number) `op` `rhs`.
    Number { op: Op, rhs: f64 },
    /// Text comparison with a non-`Eq`/`Ne` operator (lexicographic,
    /// case-insensitive) — e.g. `">m"`.
    TextCmp { op: Op, rhs: CompactString },
    /// Equality/inequality against a wildcard text matcher. `negate` is set
    /// for `<>pattern`.
    TextMatch { matcher: Matcher, negate: bool },
    /// Bare value with no operator: number↔text equality, and (for text)
    /// wildcard matching.
    BareEq {
        number: Option<f64>,
        matcher: Matcher,
    },
    /// A non-text, non-numeric criterion value (bool / blank / error):
    /// strict equality after coercion attempts.
    BareValue(CellValue),
}

/// Parse a criterion value into a compiled [`Criteria`] (spec §7).
pub fn parse_criteria(v: &CellValue) -> Criteria {
    // Non-text criteria: numbers/bools/blank are bare equality; an error
    // criterion can only equal that same error.
    let text = match v {
        CellValue::Text(t) => t.clone(),
        CellValue::Number(n) => {
            return Criteria(Kind::Number {
                op: Op::Eq,
                rhs: *n,
            })
        }
        other => return Criteria(Kind::BareValue(other.clone())),
    };

    let s = text.as_str();
    // Longest operators first so ">=" wins over ">".
    let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
        (Some(Op::Ge), r)
    } else if let Some(r) = s.strip_prefix("<=") {
        (Some(Op::Le), r)
    } else if let Some(r) = s.strip_prefix("<>") {
        (Some(Op::Ne), r)
    } else if let Some(r) = s.strip_prefix('>') {
        (Some(Op::Gt), r)
    } else if let Some(r) = s.strip_prefix('<') {
        (Some(Op::Lt), r)
    } else if let Some(r) = s.strip_prefix('=') {
        (Some(Op::Eq), r)
    } else {
        (None, s)
    };

    match op {
        // No operator: bare equality (number↔text + wildcards).
        None => {
            let number = parse_plain_number(rest);
            Criteria(Kind::BareEq {
                number,
                matcher: Matcher::compile(rest),
            })
        }
        // `=`/`<>` : if the rhs is a clean number, it's a numeric (in)equality;
        // otherwise a wildcard text (in)equality.
        Some(op @ (Op::Eq | Op::Ne)) => {
            if let Some(n) = parse_plain_number(rest) {
                Criteria(Kind::Number { op, rhs: n })
            } else {
                Criteria(Kind::TextMatch {
                    matcher: Matcher::compile(rest),
                    negate: op == Op::Ne,
                })
            }
        }
        // Ordered comparison: numeric if the rhs parses, else lexicographic
        // text.
        Some(op) => {
            if let Some(n) = parse_plain_number(rest) {
                Criteria(Kind::Number { op, rhs: n })
            } else {
                Criteria(Kind::TextCmp {
                    op,
                    rhs: CompactString::new(rest),
                })
            }
        }
    }
}

/// Test a candidate cell against a compiled [`Criteria`] (case-insensitive
/// for text; number↔text equality for bare criteria).
pub fn matches(c: &Criteria, candidate: &CellValue) -> bool {
    match &c.0 {
        Kind::Number { op, rhs } => match coerce::to_number(candidate) {
            Ok(n) => apply_num_op(*op, n, *rhs),
            // A candidate that is not a number never satisfies a numeric
            // comparison (Excel skips it) — except `<>` which a non-number
            // trivially satisfies.
            Err(_) => *op == Op::Ne,
        },
        Kind::TextCmp { op, rhs } => {
            let cand = coerce::to_text(candidate);
            apply_text_op(*op, cand.as_str(), rhs.as_str())
        }
        Kind::TextMatch { matcher, negate } => {
            // A wildcard/text PATTERN matches TEXT cells only — Excel's `*`/`?`
            // never match numbers, bools, or blanks (audit finding 3). A
            // non-text candidate fails the pattern; under `<>` the negation
            // then makes it trivially TRUE (a number is "not like a*").
            let CellValue::Text(t) = candidate else {
                return *negate;
            };
            matcher.is_match(t.as_str()) ^ *negate
        }
        Kind::BareEq { number, matcher } => {
            // Number↔text equality: a numeric criterion matches numeric
            // candidates by value...
            if let Some(rhs) = number {
                if let Ok(n) = coerce::to_number(candidate) {
                    // Only treat the candidate numerically when it really is
                    // a number/bool/blank, not arbitrary text — Excel's bare
                    // "5" matches the number 5 and the text "5".
                    if matches!(
                        candidate,
                        CellValue::Number(_) | CellValue::Bool(_) | CellValue::Empty
                    ) {
                        return n == *rhs;
                    }
                }
            }
            // ...and the wildcard text path covers text candidates (and the
            // textual spelling of the criterion). When the pattern carries an
            // actual unescaped wildcard, it is a TEXT pattern — it matches TEXT
            // cells only, never numbers/bools/blanks (audit finding 3). A bare
            // literal (no wildcard) keeps the case-insensitive text-equality
            // path, which already only matches the textual spelling.
            if matcher.has_wildcard() {
                let CellValue::Text(t) = candidate else {
                    return false;
                };
                return matcher.is_match(t.as_str());
            }
            let cand = coerce::to_text(candidate);
            matcher.is_match(cand.as_str())
        }
        Kind::BareValue(want) => candidate == want,
    }
}

fn apply_num_op(op: Op, a: f64, b: f64) -> bool {
    match op {
        Op::Eq => a == b,
        Op::Ne => a != b,
        Op::Lt => a < b,
        Op::Le => a <= b,
        Op::Gt => a > b,
        Op::Ge => a >= b,
    }
}

fn apply_text_op(op: Op, a: &str, b: &str) -> bool {
    let ord = a
        .bytes()
        .map(|x| x.to_ascii_lowercase())
        .cmp(b.bytes().map(|x| x.to_ascii_lowercase()));
    use std::cmp::Ordering::*;
    match op {
        Op::Eq => ord == Equal,
        Op::Ne => ord != Equal,
        Op::Lt => ord == Less,
        Op::Le => ord != Greater,
        Op::Gt => ord == Greater,
        Op::Ge => ord != Less,
    }
}

/// Parse a criterion's right-hand side as a plain number (no thousands
/// separators), reusing the [`coerce`] ruling. Only non-empty, fully numeric
/// text qualifies.
fn parse_plain_number(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    coerce::to_number(&CellValue::from(s)).ok()
}

/// A compiled wildcard matcher (`*`, `?`, `~`-escape; case-insensitive).
/// Tokens are lowercased literals plus the two wildcard kinds, so matching is
/// an ASCII-folded backtracking scan with no per-call allocation.
#[derive(Clone, Debug)]
pub struct Matcher {
    tokens: Vec<Tok>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    /// A literal char (already ASCII-lowercased for the fold).
    Lit(char),
    /// `?` — exactly one char.
    Any,
    /// `*` — zero or more chars.
    Star,
}

impl Matcher {
    /// Compile a pattern. `~` escapes the next `*`, `?`, or `~` into a
    /// literal; a trailing `~` is a literal `~`.
    pub fn compile(pat: &str) -> Matcher {
        let mut tokens = Vec::new();
        let mut chars = pat.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '~' => {
                    // Escape the next metachar; lone trailing ~ is literal.
                    match chars.next() {
                        Some(next @ ('*' | '?' | '~')) => {
                            tokens.push(Tok::Lit(next.to_ascii_lowercase()))
                        }
                        Some(other) => {
                            tokens.push(Tok::Lit('~'));
                            tokens.push(Tok::Lit(other.to_ascii_lowercase()));
                        }
                        None => tokens.push(Tok::Lit('~')),
                    }
                }
                '*' => tokens.push(Tok::Star),
                '?' => tokens.push(Tok::Any),
                other => tokens.push(Tok::Lit(other.to_ascii_lowercase())),
            }
        }
        Matcher { tokens }
    }

    /// Whether the compiled pattern contains an actual (unescaped) `*` or `?`
    /// wildcard. A `~`-escaped metachar compiles to a [`Tok::Lit`], so it does
    /// NOT count — `"a~*"` is the literal text `"a*"`, not a wildcard pattern.
    /// Drives the "wildcards match text cells only" ruling (audit finding 3).
    pub fn has_wildcard(&self) -> bool {
        self.tokens
            .iter()
            .any(|t| matches!(t, Tok::Star | Tok::Any))
    }

    /// Test the whole candidate string against the pattern (anchored both
    /// ends, as Excel criteria are). Case-insensitive (ASCII fold).
    pub fn is_match(&self, candidate: &str) -> bool {
        let cand: Vec<char> = candidate.chars().map(|c| c.to_ascii_lowercase()).collect();
        glob(&self.tokens, &cand)
    }
}

/// Classic two-pointer glob with `*` backtracking (linear in practice).
fn glob(pat: &[Tok], s: &[char]) -> bool {
    let (mut pi, mut si) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (pat idx after *, s idx)
    while si < s.len() {
        match pat.get(pi) {
            Some(Tok::Lit(c)) if *c == s[si] => {
                pi += 1;
                si += 1;
            }
            Some(Tok::Any) => {
                pi += 1;
                si += 1;
            }
            Some(Tok::Star) => {
                star = Some((pi + 1, si));
                pi += 1;
            }
            _ => {
                // Mismatch (or end of pattern): backtrack to the last `*`.
                if let Some((p, s_at)) = star {
                    pi = p;
                    si = s_at + 1;
                    star = Some((p, s_at + 1));
                } else {
                    return false;
                }
            }
        }
    }
    // Consume trailing `*`s.
    while matches!(pat.get(pi), Some(Tok::Star)) {
        pi += 1;
    }
    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(n: f64) -> CellValue {
        CellValue::Number(n)
    }
    fn txt(s: &str) -> CellValue {
        CellValue::from(s)
    }

    #[test]
    fn numeric_comparisons() {
        let c = parse_criteria(&txt(">5"));
        assert!(matches(&c, &num(6.0)));
        assert!(!matches(&c, &num(5.0)));
        assert!(!matches(&c, &num(4.0)));

        let c = parse_criteria(&txt(">=5"));
        assert!(matches(&c, &num(5.0)));

        let c = parse_criteria(&txt("<>5"));
        assert!(matches(&c, &num(4.0)));
        assert!(!matches(&c, &num(5.0)));
        // A non-number satisfies <>5.
        assert!(matches(&c, &txt("x")));
    }

    #[test]
    fn bare_number_text_equality() {
        let c = parse_criteria(&num(5.0));
        assert!(matches(&c, &num(5.0)));
        assert!(matches(&c, &txt("5"))); // number↔text equality
        assert!(!matches(&c, &num(6.0)));

        let c = parse_criteria(&txt("5"));
        assert!(matches(&c, &num(5.0)));
        assert!(matches(&c, &txt("5")));
    }

    #[test]
    fn bare_text_case_insensitive() {
        let c = parse_criteria(&txt("Apple"));
        assert!(matches(&c, &txt("apple")));
        assert!(matches(&c, &txt("APPLE")));
        assert!(!matches(&c, &txt("apples")));
    }

    #[test]
    fn wildcards() {
        let c = parse_criteria(&txt("a*"));
        assert!(matches(&c, &txt("apple")));
        assert!(matches(&c, &txt("a")));
        assert!(!matches(&c, &txt("banana")));

        let c = parse_criteria(&txt("?at"));
        assert!(matches(&c, &txt("cat")));
        assert!(matches(&c, &txt("bat")));
        assert!(!matches(&c, &txt("at")));
        assert!(!matches(&c, &txt("chat")));

        let c = parse_criteria(&txt("*ee*"));
        assert!(matches(&c, &txt("green")));
        assert!(matches(&c, &txt("ee")));
        assert!(!matches(&c, &txt("red")));
    }

    #[test]
    fn tilde_escape() {
        // "~*" matches a literal asterisk.
        let c = parse_criteria(&txt("a~*b"));
        assert!(matches(&c, &txt("a*b")));
        assert!(!matches(&c, &txt("axb")));

        let c = parse_criteria(&txt("~?"));
        assert!(matches(&c, &txt("?")));
        assert!(!matches(&c, &txt("x")));
    }

    #[test]
    fn ne_wildcard() {
        let c = parse_criteria(&txt("<>a*"));
        assert!(!matches(&c, &txt("apple")));
        assert!(matches(&c, &txt("banana")));
    }

    #[test]
    fn text_ordered_comparison() {
        let c = parse_criteria(&txt(">m"));
        assert!(matches(&c, &txt("n")));
        assert!(!matches(&c, &txt("a")));
    }

    #[test]
    fn wildcard_matches_text_cells_only() {
        // Audit finding 3: `*`/`?` patterns match TEXT cells only — numbers,
        // bools, and blanks NEVER match (Excel semantics). The exact audit
        // scenario is over {100, "hello", 200, "5"(text), empty}.
        let star = parse_criteria(&txt("*"));
        assert!(matches(&star, &txt("hello")), "* matches text");
        assert!(matches(&star, &txt("5")), "* matches numeric-LOOKING text");
        assert!(!matches(&star, &num(100.0)), "* must NOT match a number");
        assert!(!matches(&star, &num(200.0)), "* must NOT match a number");
        assert!(
            !matches(&star, &CellValue::Empty),
            "* must NOT match a blank"
        );
        assert!(
            !matches(&star, &CellValue::Bool(true)),
            "* must NOT match a bool"
        );

        // "?*" (at least one char) also matches only text cells.
        let q = parse_criteria(&txt("?*"));
        assert!(matches(&q, &txt("hello")));
        assert!(matches(&q, &txt("5")));
        assert!(!matches(&q, &num(100.0)));

        // A `<>`-prefixed wildcard over a non-text cell is trivially true (a
        // number is "not like" a text pattern).
        let ne = parse_criteria(&txt("<>a*"));
        assert!(matches(&ne, &num(100.0)), "number is not like a*");
        assert!(matches(&ne, &txt("banana")));
        assert!(!matches(&ne, &txt("apple")));

        // Escaped wildcards stay literal text (no `has_wildcard` trigger): a
        // literal "a*b" still only equals the text "a*b".
        let lit = parse_criteria(&txt("a~*b"));
        assert!(matches(&lit, &txt("a*b")));
        assert!(!matches(&lit, &num(0.0)));
    }
}
