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

//! The text function family (spec §7, §11 T0). Pure
//! `fn(&[Arg], &EvalCtx) -> CellValue` kernels for the 15 string functions in
//! the T0 spine: `LEN`/`LEFT`/`RIGHT`/`MID`, `CONCAT`/`CONCATENATE`,
//! `UPPER`/`LOWER`/`TRIM`/`REPT`, `TEXT`, `FIND`/`SEARCH`/`SUBSTITUTE`, and
//! `VALUE`.
//!
//! ## Conventions shared across the family
//!
//! - **1-based, char-counted.** Every position/length argument is 1-based and
//!   counts Unicode scalar values (`char`s), the T0 reading of Excel's
//!   character positions — `LEFT("héllo", 2)` is `"hé"`, `MID(s, 3, 2)`
//!   starts at the 3rd `char`. (Excel counts UTF-16 code units; a BMP-only
//!   corpus cannot tell the two apart, and the spine ships char-counting.)
//! - **Coercion through [`crate::coerce`].** All type conversion routes
//!   through `coerce::to_text` / `coerce::to_number`, so the cross-engine
//!   rulings (§7 coercion hot zone) are stated once. A scalar error argument
//!   propagates via [`crate::coerce::first_error`] before any work.
//! - **Negative counts → `#VALUE!`.** `LEFT`/`RIGHT`/`MID` with a negative
//!   count or (for `MID`) a start < 1 yield `#VALUE!`, matching Excel.

use compact_str::CompactString;
use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::criteria::Matcher;
use crate::ctx::EvalCtx;

// ---- Small shared helpers (private to the family). ----

/// Text-coerce one scalar [`Arg`], propagating an error argument as the error
/// (so a kernel can `?`-style early-return). Ranges are not a text-scalar
/// shape here — a range argument is coerced from its top-left cell, which is
/// `sheet-calc`'s implicit-intersection contract; for T0 the kernels that are
/// not `range_aware` only ever receive scalars, so a range degrades to its
/// first cell defensively.
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

/// Resolve an [`Arg`] to a single [`CellValue`]: a scalar as-is, a range to
/// its top-left cell (implicit intersection / first-cell degrade for the
/// non-`range_aware` kernels). Cloned so callers own the value.
fn arg_scalar(a: &Arg) -> CellValue {
    match a {
        Arg::Scalar(v) => v.clone(),
        Arg::Range(rv) => rv.get(0, 0),
    }
}

/// A finite, non-negative count argument rounded toward zero (Excel truncates
/// the count). A negative count is the named `#VALUE!` ruling; a non-finite
/// or out-of-`usize` value is also `#VALUE!`.
fn count_arg(n: f64) -> Result<usize, CellError> {
    if !n.is_finite() || n < 0.0 {
        return Err(CellError::Value);
    }
    let t = n.trunc();
    if t > usize::MAX as f64 {
        return Err(CellError::Value);
    }
    Ok(t as usize)
}

// ---- LEN — character count (spec §11, ECMA-376 §18.17.7). ----

/// `LEN(text)` → the number of characters (Unicode scalars) in the text form
/// of the argument. Empty/blank is `0`; a number is counted in its General
/// text form (`LEN(12.5)` = 4).
pub fn len(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    match arg_text(&args[0]) {
        Ok(s) => CellValue::Number(s.chars().count() as f64),
        Err(e) => CellValue::Error(e),
    }
}

// ---- LEFT / RIGHT / MID — substrings (1-based, char-counted). ----

/// `LEFT(text, [num_chars])` → the first `num_chars` characters (default 1).
/// A negative `num_chars` is `#VALUE!`; a count past the end clamps to the
/// whole string.
pub fn left(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let n = match num_chars_default_1(args.get(1)) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    CellValue::Text(CompactString::new(take_chars(&s, n)))
}

/// `RIGHT(text, [num_chars])` → the last `num_chars` characters (default 1).
/// A negative `num_chars` is `#VALUE!`; a count past the end clamps to the
/// whole string.
pub fn right(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let n = match num_chars_default_1(args.get(1)) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let total = s.chars().count();
    let skip = total.saturating_sub(n);
    let tail: String = s.chars().skip(skip).collect();
    CellValue::Text(CompactString::new(tail))
}

/// `MID(text, start_num, num_chars)` → `num_chars` characters beginning at the
/// 1-based `start_num`. `start_num < 1` or `num_chars < 0` is `#VALUE!`; a
/// `start_num` past the end yields `""`; a count past the end clamps.
pub fn mid(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let start_f = match arg_number(&args[1]) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    // start_num is 1-based; < 1 is a #VALUE! ruling.
    if !start_f.is_finite() || start_f < 1.0 {
        return CellValue::Error(CellError::Value);
    }
    let start = start_f.trunc() as usize; // 1-based
    let n = match arg_number(&args[2]).and_then(count_arg) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let out: String = s.chars().skip(start - 1).take(n).collect();
    CellValue::Text(CompactString::new(out))
}

/// The optional `num_chars` of `LEFT`/`RIGHT`: absent → 1, else a coerced,
/// truncated, non-negative count (negative → `#VALUE!`).
fn num_chars_default_1(a: Option<&Arg>) -> Result<usize, CellError> {
    match a {
        None => Ok(1),
        Some(arg) => arg_number(arg).and_then(count_arg),
    }
}

/// Take the first `n` characters of `s` (clamped to the string length).
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---- CONCAT (range-aware) / CONCATENATE (scalar-only). ----

/// `CONCAT(value1, …)` → every argument's text joined left-to-right. Unlike
/// `CONCATENATE`, `CONCAT` is **range-aware**: a range argument contributes
/// every cell in row-major order (spec §11; the registry `range_aware: true`).
/// An error anywhere — scalar arg or range cell — propagates.
pub fn concat(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let mut out = String::new();
    for a in args {
        match a {
            Arg::Scalar(v) => {
                if let CellValue::Error(e) = v {
                    return CellValue::Error(*e);
                }
                out.push_str(coerce::to_text(v).as_str());
            }
            Arg::Range(rv) => {
                for cell in rv.iter() {
                    if let CellValue::Error(e) = cell {
                        return CellValue::Error(e);
                    }
                    out.push_str(coerce::to_text(&cell).as_str());
                }
            }
        }
    }
    CellValue::Text(CompactString::new(out))
}

/// `CONCATENATE(text1, …)` → the scalar text of each argument joined
/// left-to-right. **Not** range-aware (registry `range_aware: false`): a range
/// argument degrades to its top-left cell (implicit intersection). An error
/// argument propagates.
pub fn concatenate(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let mut out = String::new();
    for a in args {
        match arg_text(a) {
            Ok(s) => out.push_str(s.as_str()),
            Err(e) => return CellValue::Error(e),
        }
    }
    CellValue::Text(CompactString::new(out))
}

// ---- UPPER / LOWER / TRIM — case + whitespace. ----

/// `UPPER(text)` → ASCII-and-Unicode uppercase (Rust `to_uppercase`). T0 has
/// no locale tailoring (D-8); the Turkish-i and similar locale folds wait for
/// the locale set.
pub fn upper(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    map_text(args, |s| s.to_uppercase())
}

/// `LOWER(text)` → lowercase, mirror of [`upper`].
pub fn lower(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    map_text(args, |s| s.to_lowercase())
}

/// `TRIM(text)` → leading/trailing ASCII spaces removed and every internal run
/// of spaces collapsed to a single space (Excel's space-only `TRIM`; it does
/// **not** touch tabs/newlines, only `U+0020`).
pub fn trim(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    map_text(args, |s| {
        let mut out = String::with_capacity(s.len());
        let mut prev_space = false;
        for ch in s.chars() {
            if ch == ' ' {
                prev_space = true;
            } else {
                if prev_space && !out.is_empty() {
                    out.push(' ');
                }
                prev_space = false;
                out.push(ch);
            }
        }
        out
    })
}

/// `REPT(text, number_times)` → `text` repeated `number_times` times. A
/// negative count is `#VALUE!`; zero yields `""`.
pub fn rept(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let s = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let times = match arg_number(&args[1]).and_then(count_arg) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    CellValue::Text(CompactString::new(s.as_str().repeat(times)))
}

/// Shared shape for the single-arg `UPPER`/`LOWER`/`TRIM`: propagate an error,
/// else text-coerce and apply `f`.
fn map_text(args: &[Arg], f: impl Fn(&str) -> String) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    match arg_text(&args[0]) {
        Ok(s) => CellValue::Text(CompactString::new(f(s.as_str()))),
        Err(e) => CellValue::Error(e),
    }
}

// ---- TEXT — apply a number-format code (via sheet-format). ----

/// `TEXT(value, format_code)` → `value` rendered through the ECMA-376
/// number-format `format_code`, using the context's date system (spec §9). A
/// format code that fails to compile is `#VALUE!` (Excel surfaces a bad code
/// as an error). The numeric/text/bool routing is `sheet_format::format_value`
/// — `TEXT` does not re-derive formatting, keeping §9 the single source.
pub fn text(args: &[Arg], ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let value = arg_scalar(&args[0]);
    let code = match arg_text(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let fmt = match sheet_format::compile(code.as_str()) {
        Ok(f) => f,
        Err(_) => return CellValue::Error(CellError::Value),
    };
    // TEXT renders en-US (D-8): the function dialect is always en; value
    // localization rides on the document locale (CalcSettings.locale),
    // which the EvalCtx does not yet carry. Default EnUs keeps existing
    // output byte-identical until the localization track threads it here.
    let fctx = sheet_format::FormatCtx::new(ctx.date_system, sheet_core::Locale::EnUs);
    CellValue::Text(CompactString::new(sheet_format::format_value(
        &value, &fmt, &fctx,
    )))
}

// ---- FIND (case-sensitive, literal) / SEARCH (case-insensitive, wildcard). ----

/// `FIND(find_text, within_text, [start_num])` → the 1-based **character**
/// position of the first case-sensitive, literal (no-wildcard) occurrence of
/// `find_text` in `within_text` at or after `start_num` (default 1). Not found
/// is `#VALUE!`; `start_num < 1` or past the end is `#VALUE!`. An empty
/// `find_text` matches at `start_num`.
pub fn find(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let needle = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let hay = match arg_text(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let start = match start_num_default_1(args.get(2)) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    match find_literal(needle.as_str(), hay.as_str(), start) {
        Some(pos) => CellValue::Number(pos as f64),
        None => CellValue::Error(CellError::Value),
    }
}

/// `SEARCH(find_text, within_text, [start_num])` → the 1-based character
/// position of the first **case-insensitive** match of `find_text` (which may
/// contain `*`/`?` wildcards, `~`-escaped) in `within_text` at or after
/// `start_num` (default 1). Not found is `#VALUE!`. Reuses the shared
/// [`crate::criteria::Matcher`] glob so `SEARCH` and `SUMIF`/`COUNTIF`
/// wildcards stay one ruling.
pub fn search(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let pat = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let hay = match arg_text(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let start = match start_num_default_1(args.get(2)) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    match search_wildcard(pat.as_str(), hay.as_str(), start) {
        Some(pos) => CellValue::Number(pos as f64),
        None => CellValue::Error(CellError::Value),
    }
}

/// The optional `start_num` of `FIND`/`SEARCH`: absent → 1, else a coerced,
/// truncated value that MUST be ≥ 1 (else `#VALUE!`). Returned 1-based.
fn start_num_default_1(a: Option<&Arg>) -> Result<usize, CellError> {
    match a {
        None => Ok(1),
        Some(arg) => {
            let n = arg_number(arg)?;
            if !n.is_finite() || n < 1.0 {
                return Err(CellError::Value);
            }
            Ok(n.trunc() as usize)
        }
    }
}

/// Case-sensitive literal substring search from a 1-based char `start`. The
/// scan is char-aligned: it walks candidate start positions `start-1 ..=` and
/// compares `char` prefixes, so the returned index is a character position
/// (not a byte offset). `start` past the string length means "no match".
fn find_literal(needle: &str, hay: &str, start: usize) -> Option<usize> {
    let hchars: Vec<char> = hay.chars().collect();
    let nchars: Vec<char> = needle.chars().collect();
    // start is 1-based; the first candidate index is start-1.
    let first = start - 1;
    if first > hchars.len() {
        return None;
    }
    // An empty needle matches at `start` (Excel returns start_num) as long as
    // start is within [1, len+1].
    if nchars.is_empty() {
        return Some(start);
    }
    let last_start = hchars.len().checked_sub(nchars.len())?;
    (first..=last_start).find_map(|i| {
        if hchars[i..i + nchars.len()] == nchars[..] {
            Some(i + 1) // 1-based
        } else {
            None
        }
    })
}

/// Case-insensitive wildcard search from a 1-based char `start`. The pattern
/// is treated as anchored-anywhere: for each candidate end length it tests
/// whether some prefix of the remaining string matches the (anchored) glob.
/// Excel's `SEARCH` wildcard semantics are "find the shortest match starting
/// at the earliest position", so we scan candidate start positions and, for
/// each, the shortest sub-slice the matcher accepts.
fn search_wildcard(pat: &str, hay: &str, start: usize) -> Option<usize> {
    let hchars: Vec<char> = hay.chars().collect();
    let first = start - 1;
    if first > hchars.len() {
        return None;
    }
    let matcher = Matcher::compile(pat);
    // An empty pattern matches at `start` (mirrors empty-needle FIND).
    if pat.is_empty() {
        return Some(start);
    }
    for i in first..=hchars.len() {
        // Try the shortest-to-longest sub-slice anchored at i: the matcher is
        // anchored both ends, so we test every end position.
        for j in i..=hchars.len() {
            let candidate: String = hchars[i..j].iter().collect();
            if matcher.is_match(&candidate) {
                return Some(i + 1); // 1-based start position
            }
        }
    }
    None
}

// ---- SUBSTITUTE — replace occurrences (optional 1-based instance). ----

/// `SUBSTITUTE(text, old_text, new_text, [instance_num])` → `text` with
/// occurrences of `old_text` replaced by `new_text`. With `instance_num`
/// (1-based) only that occurrence is replaced; absent, all occurrences are.
/// An empty `old_text` returns `text` unchanged (no replacement). An
/// `instance_num < 1` is `#VALUE!`; an `instance_num` past the occurrence
/// count leaves `text` unchanged (Excel). Case-sensitive, literal (no
/// wildcards) — matching Excel's `SUBSTITUTE`.
pub fn substitute(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let text = match arg_text(&args[0]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let old = match arg_text(&args[1]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    let new = match arg_text(&args[2]) {
        Ok(s) => s,
        Err(e) => return CellValue::Error(e),
    };
    // Empty old_text: no-op (avoids an infinite-match loop, matches Excel).
    if old.is_empty() {
        return CellValue::Text(text);
    }
    let instance = match args.get(3) {
        None => None,
        Some(arg) => {
            let n = match arg_number(arg) {
                Ok(n) => n,
                Err(e) => return CellValue::Error(e),
            };
            if !n.is_finite() || n < 1.0 {
                return CellValue::Error(CellError::Value);
            }
            Some(n.trunc() as usize)
        }
    };
    CellValue::Text(CompactString::new(substitute_str(
        text.as_str(),
        old.as_str(),
        new.as_str(),
        instance,
    )))
}

/// Literal, case-sensitive replacement of `old` by `new` in `text`. `instance`
/// is `None` for all occurrences, `Some(k)` for the k-th (1-based) only. A
/// `Some(k)` larger than the occurrence count returns `text` unchanged.
fn substitute_str(text: &str, old: &str, new: &str, instance: Option<usize>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut count = 0usize;
    while let Some(pos) = rest.find(old) {
        count += 1;
        let replace_here = match instance {
            None => true,
            Some(k) => count == k,
        };
        out.push_str(&rest[..pos]);
        if replace_here {
            out.push_str(new);
        } else {
            out.push_str(&rest[pos..pos + old.len()]);
        }
        rest = &rest[pos + old.len()..];
        // If we only wanted one instance and we've passed it, append the rest.
        if matches!(instance, Some(k) if count >= k) {
            out.push_str(rest);
            return out;
        }
    }
    out.push_str(rest);
    out
}

// ---- VALUE — text → number (accepts a percent suffix). ----

/// `VALUE(text)` → the numeric value of a number-spelling text. Routes through
/// the shared [`coerce::to_number`] ruling, then additionally accepts a
/// trailing `%` percent suffix — `VALUE("50%")` → `0.5` — which plain
/// coercion rejects (a percent sign is a number-format concern, but `VALUE`
/// is documented to read it). A number argument passes through; un-parseable
/// text is `#VALUE!`.
pub fn value(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let v = arg_scalar(&args[0]);
    // Non-text passes through the shared numeric coercion unchanged.
    let s = match &v {
        CellValue::Text(t) => t.clone(),
        other => return num_or_err(coerce::to_number(other)),
    };
    // Percent suffix: strip one trailing `%`, parse the head, divide by 100.
    let trimmed = s.trim();
    if let Some(head) = trimmed.strip_suffix('%') {
        return match coerce::to_number(&CellValue::from(head)) {
            Ok(n) => CellValue::Number(n / 100.0),
            Err(e) => CellValue::Error(e),
        };
    }
    num_or_err(coerce::to_number(&v))
}

/// Wrap a `Result<f64, CellError>` as a [`CellValue`].
fn num_or_err(r: Result<f64, CellError>) -> CellValue {
    match r {
        Ok(n) => CellValue::Number(n),
        Err(e) => CellValue::Error(e),
    }
}
