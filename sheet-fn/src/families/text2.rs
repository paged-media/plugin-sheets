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

//! The M1 text function family (spec §7, §11 T1). Pure
//! `fn(&[Arg], &EvalCtx) -> CellValue` kernels for the 15 string functions the
//! "publishing product" tier adds on top of the T0 [`crate::families::text`]
//! spine: `TEXTJOIN`, `EXACT`, `PROPER`, `CHAR`/`CODE`, `UNICHAR`/`UNICODE`,
//! `CLEAN`, `T`, `NUMBERVALUE`, `FIXED`, `DOLLAR`, `REPLACE`, and the T1 subset
//! of `TEXTBEFORE`/`TEXTAFTER`.
//!
//! ## Conventions shared across the family
//!
//! - **1-based, char-counted.** Every position/length argument is 1-based and
//!   counts Unicode scalar values (`char`s) — the same T0 reading of Excel's
//!   character positions used in [`crate::families::text`] (Excel counts
//!   UTF-16 code units; a BMP-only corpus cannot tell the two apart).
//! - **Coercion through [`crate::coerce`].** All type conversion routes through
//!   `coerce::to_text` / `coerce::to_number` so the §7 coercion rulings are
//!   stated once. A scalar error argument propagates via
//!   [`crate::coerce::first_error`] before any work (range cells propagate
//!   per-function — `TEXTJOIN` walks its ranges and propagates a contained
//!   error, matching Excel).
//! - **`sheet_format` owns formatting.** `FIXED` and `DOLLAR` build an
//!   ECMA-376 number-format code and render it through `sheet_format` rather
//!   than re-deriving grouping/rounding — §9 stays the single source.
//!
//! ## Excel rulings adopted in this family (bug-for-bug; see registry rows)
//!
//! - `CHAR(n)` accepts the latin-1 range `1..=255` only (`0` or `>255` →
//!   `#VALUE!`); `CODE` returns the first character's code point under the
//!   same latin-1 reading (`sheet.fn.text.char` / `…code`).
//! - `UNICHAR`/`UNICODE` are the full-Unicode counterparts: `UNICHAR(0)` is
//!   `#VALUE!`, a surrogate or out-of-range code point is `#VALUE!`
//!   (`sheet.fn.text.unichar` / `…unicode`).
//! - `NUMBERVALUE` strips the group separator and re-points the decimal
//!   separator before the shared numeric parse; a trailing `%` divides by 100
//!   (one `%` per suffix char) (`sheet.fn.text.numbervalue`).
//! - `FIXED` with negative `decimals` rounds left of the decimal point and
//!   formats with zero decimals; `no_commas` TRUE drops grouping
//!   (`sheet.fn.text.fixed`).
//! - `REPLACE` is 1-based, char-counted; `start < 1` → `#VALUE!`; a `start`
//!   past the end appends `new_text` (`sheet.fn.text.replace`).
//! - `TEXTBEFORE`/`TEXTAFTER` T1 subset: `text`, `delimiter`, optional 1-based
//!   `instance` (negative counts from the end), case-insensitive matching by
//!   default; a not-found instance is `#N/A` (`sheet.fn.text.textbefore` /
//!   `…textafter`).

use compact_str::CompactString;
use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;

// ---- Small shared helpers (private to the family). ----

/// Resolve an [`Arg`] to a single [`CellValue`]: a scalar as-is, a range to its
/// top-left cell (implicit intersection / first-cell degrade for the
/// non-`range_aware` kernels). Cloned so callers own the value.
fn arg_scalar(a: &Arg) -> CellValue {
    match a {
        Arg::Scalar(v) => v.clone(),
        Arg::Range(rv) => rv.get(0, 0),
    }
}

/// Text-coerce one scalar [`Arg`], propagating an error argument as the error
/// (so a kernel can `?`-style early-return).
fn arg_text(a: &Arg) -> Result<CompactString, CellError> {
    let v = arg_scalar(a);
    if let CellValue::Error(e) = v {
        return Err(e);
    }
    Ok(coerce::to_text(&v))
}

/// Number-coerce one scalar [`Arg`] through the shared [`coerce::to_number`]
/// ruling (un-parseable text → `#VALUE!`, error → that error).
fn arg_number(a: &Arg) -> Result<f64, CellError> {
    coerce::to_number(&arg_scalar(a))
}

/// Wrap a `Result<CellValue, CellError>`-style outcome.
fn text_or_err(r: Result<String, CellError>) -> CellValue {
    match r {
        Ok(s) => CellValue::Text(CompactString::new(s)),
        Err(e) => CellValue::Error(e),
    }
}

// ============================ TEXTJOIN ============================

/// `TEXTJOIN(delimiter, ignore_empty, text1, [text2], …)` → every text argument
/// joined by `delimiter` (spec §11; Microsoft `TEXTJOIN`). **Range-aware**: a
/// range contributes every cell in row-major order. When `ignore_empty` is
/// TRUE (the common case), empty/blank cells and empty strings are skipped (no
/// stray delimiter); when FALSE, an empty contributes an empty field so the
/// delimiter still appears. An error anywhere — the `delimiter`/`ignore_empty`
/// scalars or any joined cell — propagates (Excel).
pub fn textjoin(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    // delimiter and ignore_empty are scalar; propagate their errors first.
    let delim = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let ignore_empty = match coerce::to_bool(&arg_scalar(&args[1])) {
        Ok(b) => b,
        Err(e) => return CellValue::Error(e),
    };

    // Collect the joined fields, walking ranges row-major. An empty CELL is
    // skipped under ignore_empty; an empty STRING field is too (Excel treats a
    // blank-result text the same as a blank cell for the skip rule).
    let mut fields: Vec<String> = Vec::new();
    for a in &args[2..] {
        match a {
            Arg::Scalar(v) => {
                if let CellValue::Error(e) = v {
                    return CellValue::Error(*e);
                }
                let s = coerce::to_text(v);
                push_field(&mut fields, s.as_str(), v.is_blank(), ignore_empty);
            }
            Arg::Range(rv) => {
                for cell in rv.iter() {
                    if let CellValue::Error(e) = cell {
                        return CellValue::Error(e);
                    }
                    let s = coerce::to_text(&cell);
                    push_field(&mut fields, s.as_str(), cell.is_blank(), ignore_empty);
                }
            }
        }
    }

    CellValue::Text(CompactString::new(fields.join(delim.as_str())))
}

/// Append one field to `TEXTJOIN`'s buffer, honoring `ignore_empty`. An empty
/// field (blank cell or empty string) is dropped when `ignore_empty` is TRUE.
fn push_field(fields: &mut Vec<String>, s: &str, is_blank_cell: bool, ignore_empty: bool) {
    if ignore_empty && (is_blank_cell || s.is_empty()) {
        return;
    }
    fields.push(s.to_string());
}

// ============================ EXACT ============================

/// `EXACT(text1, text2)` → TRUE iff the two text forms are **byte-for-byte**
/// equal (case-sensitive, no fold), else FALSE (spec §11; ECMA-376 §18.17.7).
/// Unlike the `=` operator (which case-folds text), `EXACT` distinguishes
/// `"a"` from `"A"`. An error argument propagates.
pub fn exact(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let a = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let b = match arg_text(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    CellValue::Bool(a == b)
}

// ============================ PROPER ============================

/// `PROPER(text)` → title case: the first letter of each word capitalized, the
/// rest lowercased (spec §11; ECMA-376 §18.17.7). A "word" starts after any
/// non-letter (space, digit, punctuation). Apostrophes count as non-letters, so
/// `PROPER("o'brien")` → `"O'Brien"` (matching Excel). An error propagates.
pub fn proper(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let mut out = String::with_capacity(s.len());
    let mut prev_is_letter = false;
    for ch in s.chars() {
        if ch.is_alphabetic() {
            if prev_is_letter {
                out.extend(ch.to_lowercase());
            } else {
                out.extend(ch.to_uppercase());
            }
            prev_is_letter = true;
        } else {
            out.push(ch);
            prev_is_letter = false;
        }
    }
    CellValue::Text(CompactString::new(out))
}

// ============================ CHAR / CODE (latin-1) ============================

/// `CHAR(number)` → the single character whose **latin-1** code point is
/// `number` (spec §11; ECMA-376 §18.17.7). The argument is truncated to an
/// integer and MUST be in `1..=255`; `0`, a negative, or `>255` is `#VALUE!`.
/// (T0 reads the legacy "ANSI character set" as latin-1; `UNICHAR` is the
/// full-Unicode door.)
pub fn char(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let n = match arg_number(&args[0]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    if !n.is_finite() {
        return CellValue::Error(CellError::Value);
    }
    let code = n.trunc();
    if !(1.0..=255.0).contains(&code) {
        return CellValue::Error(CellError::Value);
    }
    // latin-1: code point 1..=255 maps to the same Unicode scalar.
    let ch = char::from_u32(code as u32).expect("1..=255 is a valid latin-1 scalar");
    CellValue::Text(CompactString::new(ch.to_string()))
}

/// `CODE(text)` → the **latin-1** code point of the first character of `text`
/// (spec §11; ECMA-376 §18.17.7). Empty text is `#VALUE!`. A first character
/// outside latin-1 (`> 255`) still returns its Unicode code point under the T0
/// reading (Excel substitutes `63` / `'?'`, a code-page artifact we do not
/// reproduce — the corpus is BMP/latin-1 so the two never diverge there).
pub fn code(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    match s.chars().next() {
        Some(ch) => CellValue::Number(ch as u32 as f64),
        None => CellValue::Error(CellError::Value),
    }
}

// ============================ UNICHAR / UNICODE (full Unicode) ============================

/// `UNICHAR(number)` → the character at the Unicode code point `number` (spec
/// §11; Microsoft `UNICHAR`). The argument truncates to an integer and MUST be
/// a valid, non-zero Unicode scalar value: `0`, a negative, a surrogate
/// (`0xD800..=0xDFFF`), or a value past `0x10FFFF` is `#VALUE!`.
pub fn unichar(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let n = match arg_number(&args[0]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    if !n.is_finite() {
        return CellValue::Error(CellError::Value);
    }
    let code = n.trunc();
    if code < 1.0 || code > u32::MAX as f64 {
        return CellValue::Error(CellError::Value);
    }
    match char::from_u32(code as u32) {
        Some(ch) => CellValue::Text(CompactString::new(ch.to_string())),
        // 0 is excluded above; from_u32 also rejects surrogates / > 0x10FFFF.
        None => CellValue::Error(CellError::Value),
    }
}

/// `UNICODE(text)` → the Unicode code point of the first character of `text`
/// (spec §11; Microsoft `UNICODE`). Empty text is `#VALUE!`. The full-Unicode
/// counterpart of `CODE` — `UNICODE("€")` is `8364`, not the latin-1 truncation.
pub fn unicode(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    match s.chars().next() {
        Some(ch) => CellValue::Number(ch as u32 as f64),
        None => CellValue::Error(CellError::Value),
    }
}

// ============================ CLEAN ============================

/// `CLEAN(text)` → `text` with every ASCII control character (`0x00..=0x1F`)
/// removed (spec §11; ECMA-376 §18.17.7). The classic use is stripping
/// stray line breaks / tabs from imported data. Only the low control block is
/// stripped — printable characters (including non-ASCII) pass through. An error
/// propagates.
pub fn clean(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let out: String = s.chars().filter(|&c| (c as u32) > 0x1F).collect();
    CellValue::Text(CompactString::new(out))
}

// ============================ T ============================

/// `T(value)` → `value` if it is text, else the empty string (spec §11;
/// ECMA-376 §18.17.7). A number, bool, or blank yields `""`; text passes
/// through unchanged. An error argument propagates (Excel returns the error,
/// not `""`).
pub fn t(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match arg_scalar(&args[0]) {
        CellValue::Text(s) => CellValue::Text(s),
        CellValue::Error(e) => CellValue::Error(e),
        _ => CellValue::Text(CompactString::default()),
    }
}

// ============================ NUMBERVALUE ============================

/// `NUMBERVALUE(text, [decimal_separator], [group_separator])` → the numeric
/// value of `text` read with the given separators (spec §11; Microsoft
/// `NUMBERVALUE`). Defaults: decimal `"."`, group `","`. The group separator is
/// stripped, the decimal separator re-pointed to `"."`, then the result goes
/// through the shared [`coerce::to_number`] parse. A trailing run of `%` divides
/// by 100 per character. A blank `text` is `0`. Un-parseable text is `#VALUE!`;
/// the separators must each be a non-empty string (Excel `#VALUE!` otherwise),
/// and only their first character is significant.
pub fn numbervalue(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let text = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    // A blank input is 0 (Excel).
    if text.trim().is_empty() {
        return CellValue::Number(0.0);
    }
    let decimal = match sep_char(args.get(1), '.') {
        Ok(c) => c,
        Err(e) => return CellValue::Error(e),
    };
    let group = match sep_char(args.get(2), ',') {
        Ok(c) => c,
        Err(e) => return CellValue::Error(e),
    };

    // Strip ASCII whitespace anywhere (Excel ignores spaces in NUMBERVALUE),
    // remove the group separator, re-point the decimal separator to '.'.
    let mut buf = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_whitespace() || ch == group {
            continue;
        }
        if ch == decimal {
            buf.push('.');
        } else {
            buf.push(ch);
        }
    }

    // A trailing run of '%' scales by 1/100 each.
    let mut percent_div = 1.0_f64;
    while buf.ends_with('%') {
        buf.pop();
        percent_div *= 100.0;
    }

    match coerce::to_number(&CellValue::from(buf.as_str())) {
        Ok(n) => CellValue::Number(n / percent_div),
        Err(e) => CellValue::Error(e),
    }
}

/// The first character of an optional separator [`Arg`] (default `fallback`).
/// An explicitly empty separator string is `#VALUE!` (Excel).
fn sep_char(a: Option<&Arg>, fallback: char) -> Result<char, CellError> {
    match a {
        None => Ok(fallback),
        Some(arg) => {
            let s = arg_text(arg)?;
            s.chars().next().ok_or(CellError::Value)
        }
    }
}

// ============================ FIXED ============================

/// `FIXED(number, [decimals], [no_commas])` → `number` rounded to `decimals`
/// places and formatted as fixed-point text, with thousands grouping unless
/// `no_commas` is TRUE (spec §11; ECMA-376 §18.17.7). `decimals` defaults to
/// `2`; a **negative** `decimals` rounds left of the decimal point and formats
/// with zero decimals (`FIXED(1234.567, -1)` → `"1,230"`). The rounding and
/// grouping are `sheet_format`'s, so the §9 number-format engine stays the
/// single source. An error propagates.
pub fn fixed(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let number = match arg_number(&args[0]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let decimals = match opt_int_arg(args.get(1), 2) {
        Ok(d) => d,
        Err(e) => return CellValue::Error(e),
    };
    let no_commas = match opt_bool_arg(args.get(2), false) {
        Ok(b) => b,
        Err(e) => return CellValue::Error(e),
    };
    text_or_err(format_currency(number, decimals, no_commas, ""))
}

/// The general fixed/currency renderer (powers both `FIXED` and `DOLLAR`).
/// `prefix` is a literal string put inside the format code before the number
/// pattern (DOLLAR passes `"$"`).
fn format_currency(
    value: f64,
    decimals: i32,
    no_commas: bool,
    prefix: &str,
) -> Result<String, CellError> {
    // Negative `decimals`: round left of the point, then format at 0 decimals.
    let (rounded, eff_decimals) = if decimals < 0 {
        let pow = 10f64.powi(-decimals);
        (round_half_away(value / pow) * pow, 0usize)
    } else {
        (value, decimals as usize)
    };

    // Build the ECMA-376 pattern. The literal currency prefix is quoted so the
    // format parser treats it verbatim, then the integer/grouping pattern, then
    // the optional fractional places.
    let group = if no_commas { "0" } else { "#,##0" };
    let frac = if eff_decimals > 0 {
        format!(".{}", "0".repeat(eff_decimals))
    } else {
        String::new()
    };
    let quoted_prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("\"{prefix}\"")
    };
    // Negative section repeats the pattern with a leading minus before the
    // currency prefix. T0 RULING (registry `…fixed`/`…dollar` rows): the
    // negative form is the simple leading-minus "-$1,234.50", NOT Excel's
    // parenthesized "($1,234.50)" — a documented bug-for-bug deviation.
    let pos = format!("{quoted_prefix}{group}{frac}");
    let neg = format!("-{quoted_prefix}{group}{frac}");
    let code = format!("{pos};{neg}");

    let compiled = sheet_format::compile(&code).map_err(|_| CellError::Value)?;
    let fctx = sheet_format::FormatCtx::default();
    Ok(sheet_format::format_value(
        &CellValue::Number(rounded),
        &compiled,
        &fctx,
    ))
}

/// Round half away from zero (Excel display/`FIXED` rounding) to an integer.
fn round_half_away(v: f64) -> f64 {
    if v >= 0.0 {
        (v + 0.5).floor()
    } else {
        (v - 0.5).ceil()
    }
}

// ============================ DOLLAR ============================

/// `DOLLAR(number, [decimals])` → `number` formatted as currency text with a
/// leading `$`, thousands grouping, and `decimals` places (default `2`) (spec
/// §11; ECMA-376 §18.17.7). A negative `decimals` rounds left of the point as
/// in `FIXED`. The currency style comes from `sheet_format` (§9). The locale
/// currency symbol is the T0 `$` literal (D-8: no locale tailoring yet). An
/// error propagates.
pub fn dollar(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let number = match arg_number(&args[0]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let decimals = match opt_int_arg(args.get(1), 2) {
        Ok(d) => d,
        Err(e) => return CellValue::Error(e),
    };
    // DOLLAR always groups thousands (no `no_commas` argument).
    text_or_err(format_currency(number, decimals, false, "$"))
}

// ============================ REPLACE ============================

/// `REPLACE(old_text, start_num, num_chars, new_text)` → `old_text` with the
/// `num_chars` characters beginning at the 1-based `start_num` replaced by
/// `new_text` (spec §11; ECMA-376 §18.17.7). Char-counted. `start_num < 1` or a
/// negative `num_chars` is `#VALUE!`. A `start_num` past the end appends
/// `new_text`; a `num_chars` running past the end clamps to the remainder. An
/// error propagates.
pub fn replace(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let old = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let start_f = match arg_number(&args[1]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let len_f = match arg_number(&args[2]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let new = match arg_text(&args[3]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    if !start_f.is_finite() || start_f < 1.0 {
        return CellValue::Error(CellError::Value);
    }
    if !len_f.is_finite() || len_f < 0.0 {
        return CellValue::Error(CellError::Value);
    }
    let start = start_f.trunc() as usize; // 1-based
    let count = len_f.trunc() as usize;

    let chars: Vec<char> = old.chars().collect();
    let total = chars.len();
    // 0-based start, clamped to the end (a start past the end appends).
    let begin = (start - 1).min(total);
    let end = begin.saturating_add(count).min(total);

    let mut out = String::with_capacity(old.len() + new.len());
    out.extend(chars[..begin].iter());
    out.push_str(new.as_str());
    out.extend(chars[end..].iter());
    CellValue::Text(CompactString::new(out))
}

// ============================ TEXTBEFORE / TEXTAFTER (T1 subset) ============================

/// `TEXTBEFORE(text, delimiter, [instance])` → the substring before the
/// `instance`-th occurrence of `delimiter` (spec §11; Microsoft `TEXTBEFORE`,
/// T1 subset). `instance` defaults to `1`; a **negative** instance counts
/// occurrences from the end (`-1` = last). Matching is **case-insensitive** by
/// default (the T1 reading; `match_mode`/`match_end`/`if_not_found` are deferred
/// to the full M2 form). A not-found instance (or `0`) is `#N/A`. An empty
/// `delimiter` returns `""` (the whole text is "after" an empty delimiter at
/// position 0). An error propagates.
pub fn textbefore(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    textsplit(args, Side::Before)
}

/// `TEXTAFTER(text, delimiter, [instance])` → the substring after the
/// `instance`-th occurrence of `delimiter` (spec §11; Microsoft `TEXTAFTER`,
/// T1 subset). Same `instance`/case-insensitive rules as [`textbefore`]. A
/// not-found instance is `#N/A`. An empty `delimiter` returns the whole `text`.
/// An error propagates.
pub fn textafter(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    textsplit(args, Side::After)
}

/// Which side of the matched delimiter `TEXTBEFORE`/`TEXTAFTER` returns.
enum Side {
    Before,
    After,
}

/// Shared `TEXTBEFORE`/`TEXTAFTER` body. Finds the `instance`-th delimiter
/// occurrence (case-insensitive, char-aligned, negative counts from the end)
/// and returns the text on the requested side.
fn textsplit(args: &[Arg], side: Side) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let text = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let delim = match arg_text(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let instance = match args.get(2) {
        None => 1i64,
        Some(arg) => {
            let n = match arg_number(arg) {
                Ok(n) => n,
                Err(e) => return CellValue::Error(e),
            };
            if !n.is_finite() {
                return CellValue::Error(CellError::Value);
            }
            n.trunc() as i64
        }
    };

    // Excel: empty delimiter -> instance 1 splits at position 0
    // (TEXTBEFORE -> "", TEXTAFTER -> whole text). A zero instance is #N/A.
    if instance == 0 {
        return CellValue::Error(CellError::Na);
    }
    let tchars: Vec<char> = text.chars().collect();
    if delim.is_empty() {
        return match side {
            Side::Before => CellValue::Text(CompactString::default()),
            Side::After => CellValue::Text(text),
        };
    }
    let dchars: Vec<char> = delim.chars().collect();

    // All occurrence start indices (0-based, char positions), case-insensitive.
    let occ = find_all_ci(&tchars, &dchars);
    if occ.is_empty() {
        return CellValue::Error(CellError::Na);
    }
    let idx = if instance > 0 {
        let k = instance as usize;
        if k > occ.len() {
            return CellValue::Error(CellError::Na);
        }
        occ[k - 1]
    } else {
        let k = (-instance) as usize;
        if k > occ.len() {
            return CellValue::Error(CellError::Na);
        }
        occ[occ.len() - k]
    };

    let out: String = match side {
        Side::Before => tchars[..idx].iter().collect(),
        Side::After => tchars[idx + dchars.len()..].iter().collect(),
    };
    CellValue::Text(CompactString::new(out))
}

/// Every 0-based char start index where `needle` occurs in `hay`,
/// case-insensitively (ASCII fold). Overlapping matches advance by one (Excel's
/// occurrence counting is left-to-right, non-skipping past a found start).
fn find_all_ci(hay: &[char], needle: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    let last = hay.len() - needle.len();
    let mut i = 0;
    while i <= last {
        if (0..needle.len()).all(|j| eq_ci(hay[i + j], needle[j])) {
            out.push(i);
            // Advance past this occurrence so counting is non-overlapping
            // (Excel counts non-overlapping delimiter occurrences).
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Case-insensitive char equality (ASCII fold; non-ASCII compares as-is — T0
/// has no Unicode case folding, D-8). [`char::eq_ignore_ascii_case`] folds
/// only the ASCII range, so a non-ASCII pair falls back to plain equality.
fn eq_ci(a: char, b: char) -> bool {
    a.eq_ignore_ascii_case(&b)
}

// ---- Shared optional-argument coercion for FIXED/DOLLAR. ----

/// An optional integer argument (truncated toward zero), defaulting to
/// `fallback` when absent. An error or un-parseable value propagates.
fn opt_int_arg(a: Option<&Arg>, fallback: i32) -> Result<i32, CellError> {
    match a {
        None => Ok(fallback),
        Some(arg) => {
            let n = arg_number(arg)?;
            if !n.is_finite() {
                return Err(CellError::Value);
            }
            Ok(n.trunc() as i32)
        }
    }
}

/// An optional boolean argument, defaulting to `fallback` when absent. Routes
/// through the shared [`coerce::to_bool`] ruling.
fn opt_bool_arg(a: Option<&Arg>, fallback: bool) -> Result<bool, CellError> {
    match a {
        None => Ok(fallback),
        Some(arg) => coerce::to_bool(&arg_scalar(arg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // FIXED/DOLLAR/NUMBERVALUE rounding & format-code construction are exercised
    // end-to-end in sheet-conformance/tests/fn_text2.rs; these unit tests pin
    // the private helpers' rulings directly.

    #[test]
    fn round_half_away_pins() {
        assert_eq!(round_half_away(2.5), 3.0);
        assert_eq!(round_half_away(-2.5), -3.0);
        assert_eq!(round_half_away(2.4), 2.0);
        assert_eq!(round_half_away(0.0), 0.0);
    }

    #[test]
    fn find_all_ci_non_overlapping() {
        let hay: Vec<char> = "a-b-c".chars().collect();
        let needle: Vec<char> = "-".chars().collect();
        assert_eq!(find_all_ci(&hay, &needle), vec![1, 3]);
        // Case-insensitive, non-overlapping.
        let hay: Vec<char> = "aAaA".chars().collect();
        let needle: Vec<char> = "aa".chars().collect();
        assert_eq!(find_all_ci(&hay, &needle), vec![0, 2]);
    }

    #[test]
    fn sep_char_defaults_and_empty() {
        assert_eq!(sep_char(None, '.'), Ok('.'));
        let arg = Arg::Scalar(CellValue::from(","));
        assert_eq!(sep_char(Some(&arg), '.'), Ok(','));
        let empty = Arg::Scalar(CellValue::from(""));
        assert_eq!(sep_char(Some(&empty), '.'), Err(CellError::Value));
    }
}
