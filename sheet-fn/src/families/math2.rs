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

//! M1 math/trig (M1 additions) family kernels (spec §7/§11, milestone M1).
//! Pure `fn(&[Arg], &EvalCtx) -> CellValue` kernels named by the registry
//! `rust` field, building against the FROZEN calling convention
//! (`crate::arg`/`crate::ctx`). All type conversion and error propagation go
//! through [`crate::coerce`]; arithmetic accumulation routes through the
//! [`crate::num::Numeric`] seam (D-6: `f64` now, exact-decimal later), while
//! transcendental std calls (`sinh`, `acosh`, …) stay direct — the seam buys
//! nothing there (see `num.rs` doc).
//!
//! ## Excel rulings honored here (each is a registry-tested feature)
//!
//! - **MROUND** rounds to the nearest multiple *half away from zero*
//!   (`MROUND(10, 3) == 9`); a zero multiple yields `0`; when `sign(number)`
//!   differs from `sign(multiple)` the result is `#NUM!` (the classic
//!   sign-mismatch ruling, ECMA-376 §18.17.7).
//! - **EVEN/ODD** round *away from zero* to the nearest even / odd integer
//!   (`EVEN(3) == 4`, `EVEN(-1) == -2`, `ODD(2) == 3`, `ODD(0) == 1`).
//!   `EVEN(0) == 0`.
//! - **FACT/FACTDOUBLE** truncate a non-integer argument toward zero, then
//!   `#NUM!` for a negative argument. `FACT(0) == 1`; `FACTDOUBLE(0) == 1`
//!   and `FACTDOUBLE(-1) == 1` (the empty-product base cases).
//! - **COMBIN/PERMUT** truncate both arguments; `#NUM!` for a negative
//!   argument or when `k > n`.
//! - **GCD/LCM** are variadic and range-aware: every value truncates to a
//!   non-negative integer (`#NUM!` on a negative); non-integer text/blank in
//!   a range is skipped like other aggregations, but a non-numeric *scalar*
//!   coerces (and errors propagate). `GCD(0, 0) == 0`; `LCM(…, 0) == 0`.
//! - **SUMSQ** is the range-aware sum of squares — same range-vs-scalar
//!   asymmetry as `SUM` (text/bool/blank inside a range skip; a scalar
//!   coerces; an error cell propagates).
//! - **SQRTPI** is `sqrt(n * π)`; `#NUM!` for a negative argument.
//! - **SINH/COSH/TANH** are total; **ASINH** is total; **ACOSH** is `#NUM!`
//!   for `x < 1`; **ATANH** is `#NUM!` for `|x| >= 1`.
//! - **BASE(number, radix, [min_length])** renders a non-negative integer in
//!   `radix` (2..=36) as upper-case text, left-padded with `0` to
//!   `min_length`; `#NUM!` for a negative number, an out-of-range radix, or a
//!   non-integer / out-of-range min_length. **DECIMAL(text, radix)** parses
//!   text in `radix` back to a number; `#NUM!` for an out-of-range radix or a
//!   digit not valid in the radix.
//!
//! ## The range-vs-scalar coercion asymmetry (the variadic kernels)
//!
//! `GCD`/`LCM`/`SUMSQ` iterate ranges *skipping non-numeric cells* — text,
//! bools, and blanks inside a RANGE are ignored (Excel aggregation rule). A
//! non-numeric *scalar* argument is run through [`coerce::to_number`] (the
//! bool `TRUE` → `1`, numeric text `"5"` → `5`, un-parseable text → `#VALUE!`).
//! An *error* cell inside a range IS the error (first-error-wins).

use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;
use crate::num::{Numeric, F64};

// ---- small shared helpers --------------------------------------------------

/// Wrap an `f64` result, mapping a non-finite outcome (overflow, etc.) to
/// Excel's `#NUM!`. Real in-domain results are always finite.
#[inline]
fn finite(n: f64) -> CellValue {
    if n.is_finite() {
        CellValue::Number(n)
    } else {
        CellValue::Error(CellError::Num)
    }
}

/// Coerce a single scalar arg to a number, propagating any error or
/// un-parseable-text `#VALUE!`. A range handed to a scalar-arity kernel reads
/// its top-left cell (the 1×1 range `sheet-calc` builds for a single ref).
#[inline]
fn scalar_num(arg: &Arg) -> Result<f64, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_number(v),
        Arg::Range(r) => coerce::to_number(&r.get(0, 0)),
    }
}

/// One numeric scalar argument or its propagated error — the spine of every
/// unary math2 kernel. `f` maps the value to a `CellValue`.
#[inline]
fn unary(args: &[Arg], f: impl FnOnce(f64) -> CellValue) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    match scalar_num(&args[0]) {
        Ok(x) => f(x),
        Err(e) => CellValue::Error(e),
    }
}

/// Two numeric scalar arguments or a propagated error.
#[inline]
fn binary(args: &[Arg], f: impl FnOnce(f64, f64) -> CellValue) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let a = match scalar_num(&args[0]) {
        Ok(x) => x,
        Err(e) => return CellValue::Error(e),
    };
    let b = match scalar_num(&args[1]) {
        Ok(x) => x,
        Err(e) => return CellValue::Error(e),
    };
    f(a, b)
}

// ---- rounding to a multiple ------------------------------------------------

/// `MROUND(number, multiple)` (ECMA-376 §18.17.7) — round `number` to the
/// nearest multiple of `multiple`, ties resolved *half away from zero* (so
/// `MROUND(10, 3) == 9`, `MROUND(-10, -3) == -9`). A zero `multiple` returns
/// `0`; a sign mismatch between `number` and `multiple` (with a non-zero
/// `number`) is `#NUM!`.
pub fn mround(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |n, mult| {
        if mult == 0.0 {
            return CellValue::Number(0.0);
        }
        if n != 0.0 && n.signum() != mult.signum() {
            return CellValue::Error(CellError::Num);
        }
        // f64::round is itself half-away-from-zero — exactly Excel's rule.
        finite((n / mult).round() * mult)
    })
}

/// `EVEN(number)` (ECMA-376 §18.17.7) — round `number` *away from zero* to the
/// nearest even integer. `EVEN(0) == 0`, `EVEN(3) == 4`, `EVEN(-1) == -2`.
pub fn even(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x == 0.0 {
            return CellValue::Number(0.0);
        }
        let s = x.signum();
        let m = x.abs();
        finite((m / 2.0).ceil() * 2.0 * s)
    })
}

/// `ODD(number)` (ECMA-376 §18.17.7) — round `number` *away from zero* to the
/// nearest odd integer. `ODD(0) == 1`, `ODD(2) == 3`, `ODD(-2) == -3`.
pub fn odd(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x == 0.0 {
            return CellValue::Number(1.0);
        }
        let s = x.signum();
        let m = x.abs();
        // Round magnitude up to the next odd integer: ceil((m+1)/2)*2 - 1.
        finite((((m + 1.0) / 2.0).ceil() * 2.0 - 1.0) * s)
    })
}

// ---- factorials & counting -------------------------------------------------

/// `FACT(number)` (ECMA-376 §18.17.7) — factorial of `trunc(number)`. `#NUM!`
/// for a negative argument; `FACT(0) == 1`. Overflow (`number` large) is the
/// non-finite `#NUM!` of [`finite`].
pub fn fact(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        let k = x.trunc();
        if k < 0.0 {
            return CellValue::Error(CellError::Num);
        }
        let mut acc = F64(1.0);
        let mut i = 2.0;
        while i <= k {
            acc = acc.mul(F64::from_f64(i));
            i += 1.0;
        }
        finite(acc.to_f64())
    })
}

/// `FACTDOUBLE(number)` (ECMA-376 §18.17.7) — double factorial of
/// `trunc(number)` (the product `n·(n-2)·(n-4)·…`). Defined for `n >= -1`:
/// `FACTDOUBLE(-1) == 1` and `FACTDOUBLE(0) == 1` are the empty-product base
/// cases; `n < -1` (i.e. `<= -2`) is `#NUM!`.
pub fn factdouble(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        let k = x.trunc();
        if k < -1.0 {
            return CellValue::Error(CellError::Num);
        }
        let mut acc = F64(1.0);
        let mut i = k;
        while i > 1.0 {
            acc = acc.mul(F64::from_f64(i));
            i -= 2.0;
        }
        finite(acc.to_f64())
    })
}

/// `COMBIN(number, number_chosen)` (ECMA-376 §18.17.7) — the count of
/// unordered `k`-combinations of `n` items (`n! / (k!·(n-k)!)`). Both
/// arguments truncate toward zero; `#NUM!` for a negative argument or `k > n`.
/// Computed multiplicatively (no large factorials) and exact for the integer
/// results Excel returns.
pub fn combin(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |n, k| {
        let n = n.trunc();
        let k = k.trunc();
        if n < 0.0 || k < 0.0 || k > n {
            return CellValue::Error(CellError::Num);
        }
        finite(n_choose_k(n, k))
    })
}

/// `PERMUT(number, number_chosen)` (ECMA-376 §18.17.7) — the count of ordered
/// `k`-permutations of `n` items (`n! / (n-k)!`). Both arguments truncate
/// toward zero; `#NUM!` for a negative argument or `k > n`.
pub fn permut(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |n, k| {
        let n = n.trunc();
        let k = k.trunc();
        if n < 0.0 || k < 0.0 || k > n {
            return CellValue::Error(CellError::Num);
        }
        // n·(n-1)·…·(n-k+1), the falling factorial.
        let mut acc = F64(1.0);
        let mut i = 0.0;
        while i < k {
            acc = acc.mul(F64::from_f64(n - i));
            i += 1.0;
        }
        finite(acc.to_f64())
    })
}

/// `C(n, k)` multiplicatively, using the smaller of `k`/`n-k` so the product
/// stays small. Returns an `f64` (Excel's domain); the caller has already
/// validated `0 <= k <= n`.
fn n_choose_k(n: f64, k: f64) -> f64 {
    let k = k.min(n - k);
    let mut acc = F64(1.0);
    let mut i = 1.0;
    while i <= k {
        // acc = acc * (n - k + i) / i, kept balanced to limit rounding.
        acc = acc.mul(F64::from_f64(n - k + i)).div(F64::from_f64(i));
        i += 1.0;
    }
    // Round: the exact result is an integer; balanced division leaves only
    // sub-ULP noise.
    acc.to_f64().round()
}

// ---- GCD / LCM (variadic, range-aware) -------------------------------------

/// Collect the *integer* operands of a variadic number/range kernel, applying
/// the range-vs-scalar asymmetry: a scalar coerces (text/bool participate),
/// a range skips non-numeric cells but propagates an error cell. Each value
/// truncates toward zero; a negative value is `#NUM!`.
fn collect_nonneg_ints(args: &[Arg]) -> Result<Vec<u64>, CellError> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Arg::Scalar(v) => {
                let n = coerce::to_number(v)?;
                let t = n.trunc();
                if t < 0.0 {
                    return Err(CellError::Num);
                }
                out.push(t as u64);
            }
            Arg::Range(r) => {
                for cell in r.iter() {
                    match &cell {
                        CellValue::Error(e) => return Err(*e),
                        CellValue::Number(n) => {
                            let t = n.trunc();
                            if t < 0.0 {
                                return Err(CellError::Num);
                            }
                            out.push(t as u64);
                        }
                        // text/bool/blank inside a range skip (aggregation).
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Binary GCD on non-negative integers (`gcd(0, 0) == 0`, `gcd(0, n) == n`).
fn gcd2(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// `GCD(number1, [number2], …)` (ECMA-376 §18.17.7) — greatest common divisor
/// of the truncated, non-negative integer operands. Variadic and range-aware;
/// `GCD(0, 0) == 0`. A negative operand or an un-parseable scalar errors.
pub fn gcd(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let ints = match collect_nonneg_ints(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let mut g = 0u64;
    for n in ints {
        g = gcd2(g, n);
    }
    CellValue::Number(g as f64)
}

/// `LCM(number1, [number2], …)` (ECMA-376 §18.17.7) — least common multiple of
/// the truncated, non-negative integer operands. Variadic and range-aware;
/// any `0` operand makes the result `0`. A negative operand or an
/// un-parseable scalar errors; an integer overflow surfaces as `#NUM!`.
pub fn lcm(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let ints = match collect_nonneg_ints(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let mut l = 1u64;
    for n in ints {
        if n == 0 {
            return CellValue::Number(0.0);
        }
        let g = gcd2(l, n);
        // l = l / g * n, guarding the multiply against overflow.
        match (l / g).checked_mul(n) {
            Some(v) => l = v,
            None => return CellValue::Error(CellError::Num),
        }
    }
    CellValue::Number(l as f64)
}

// ---- sum of squares (range-aware) ------------------------------------------

/// `SUMSQ(number1, [number2], …)` (ECMA-376 §18.17.7) — the sum of the squares
/// of the numeric operands. Range-aware with the same range-vs-scalar
/// asymmetry as `SUM`: a scalar coerces (text/bool participate, un-parseable
/// text → `#VALUE!`), range cells that are not numbers skip, and an error cell
/// anywhere propagates.
pub fn sumsq(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let mut acc = F64(0.0);
    for arg in args {
        match arg {
            Arg::Scalar(v) => match coerce::to_number(v) {
                Ok(n) => {
                    let sq = F64::from_f64(n);
                    acc = acc.add(sq.mul(sq));
                }
                Err(e) => return CellValue::Error(e),
            },
            Arg::Range(r) => {
                for cell in r.iter() {
                    match &cell {
                        CellValue::Error(e) => return CellValue::Error(*e),
                        CellValue::Number(n) => {
                            let sq = F64::from_f64(*n);
                            acc = acc.add(sq.mul(sq));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    finite(acc.to_f64())
}

// ---- roots & hyperbolics ---------------------------------------------------

/// `SQRTPI(number)` (ECMA-376 §18.17.7) — `sqrt(number · π)`. `#NUM!` for a
/// negative argument (out of domain).
pub fn sqrtpi(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x < 0.0 {
            CellValue::Error(CellError::Num)
        } else {
            finite((x * std::f64::consts::PI).sqrt())
        }
    })
}

/// `SINH(number)` — hyperbolic sine. Overflow surfaces as `#NUM!`.
pub fn sinh(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.sinh()))
}

/// `COSH(number)` — hyperbolic cosine. Overflow surfaces as `#NUM!`.
pub fn cosh(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.cosh()))
}

/// `TANH(number)` — hyperbolic tangent (total, asymptotes at ±1).
pub fn tanh(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.tanh()))
}

/// `ASINH(number)` — inverse hyperbolic sine (total over all reals).
pub fn asinh(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.asinh()))
}

/// `ACOSH(number)` — inverse hyperbolic cosine; `#NUM!` for `x < 1` (out of
/// domain). `ACOSH(1) == 0`.
pub fn acosh(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x < 1.0 {
            CellValue::Error(CellError::Num)
        } else {
            finite(x.acosh())
        }
    })
}

/// `ATANH(number)` — inverse hyperbolic tangent; `#NUM!` for `|x| >= 1` (the
/// asymptotes / out of domain).
pub fn atanh(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x.abs() >= 1.0 {
            CellValue::Error(CellError::Num)
        } else {
            finite(x.atanh())
        }
    })
}

// ---- base conversion (number ↔ positional text) ----------------------------

/// The legal radix range for [`base`] / [`decimal`] (Microsoft docs: 2..=36).
const RADIX_LO: f64 = 2.0;
const RADIX_HI: f64 = 36.0;

/// `BASE(number, radix, [min_length])` (Microsoft docs) — render a
/// non-negative integer in `radix` (2..=36) as upper-case text, left-padded
/// with `0` to at least `min_length` characters. `#NUM!` for a negative
/// `number`, an out-of-range `radix`, or a negative / >255 `min_length`.
/// Non-integer `number`/`radix`/`min_length` truncate toward zero first.
pub fn base(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let number = match scalar_num(&args[0]) {
        Ok(v) => v.trunc(),
        Err(e) => return CellValue::Error(e),
    };
    let radix = match scalar_num(&args[1]) {
        Ok(v) => v.trunc(),
        Err(e) => return CellValue::Error(e),
    };
    let min_len = if args.len() >= 3 {
        match scalar_num(&args[2]) {
            Ok(v) => v.trunc(),
            Err(e) => return CellValue::Error(e),
        }
    } else {
        0.0
    };

    if number < 0.0 || !(RADIX_LO..=RADIX_HI).contains(&radix) {
        return CellValue::Error(CellError::Num);
    }
    if !(0.0..=255.0).contains(&min_len) {
        return CellValue::Error(CellError::Num);
    }

    let radix = radix as u64;
    let min_len = min_len as usize;
    let mut n = number as u64;

    let mut digits = Vec::new();
    if n == 0 {
        digits.push(b'0');
    }
    while n > 0 {
        let d = (n % radix) as u32;
        digits.push(digit_char(d));
        n /= radix;
    }
    while digits.len() < min_len {
        digits.push(b'0');
    }
    digits.reverse();
    // ASCII by construction (`0`-`9`, `A`-`Z`).
    CellValue::from(std::str::from_utf8(&digits).unwrap_or(""))
}

/// `DECIMAL(text, radix)` (Microsoft docs) — parse `text` as a number written
/// in `radix` (2..=36). Case-insensitive; leading/trailing ASCII whitespace
/// is trimmed. `#NUM!` for an out-of-range `radix`, an empty string, or any
/// digit not valid in `radix`.
pub fn decimal(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let text = match &args[0] {
        Arg::Scalar(v) => coerce::to_text(v),
        Arg::Range(r) => coerce::to_text(&r.get(0, 0)),
    };
    let radix = match scalar_num(&args[1]) {
        Ok(v) => v.trunc(),
        Err(e) => return CellValue::Error(e),
    };
    if !(RADIX_LO..=RADIX_HI).contains(&radix) {
        return CellValue::Error(CellError::Num);
    }
    let radix = radix as u32;

    let s = text.trim();
    if s.is_empty() {
        return CellValue::Error(CellError::Num);
    }
    let mut acc = F64(0.0);
    let r = F64::from_f64(radix as f64);
    for ch in s.chars() {
        match ch.to_digit(radix) {
            Some(d) => acc = acc.mul(r).add(F64::from_f64(d as f64)),
            None => return CellValue::Error(CellError::Num),
        }
    }
    finite(acc.to_f64())
}

/// A single base-`radix` digit value (`0`-`35`) → its upper-case ASCII char.
#[inline]
fn digit_char(d: u32) -> u8 {
    if d < 10 {
        b'0' + d as u8
    } else {
        b'A' + (d - 10) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::{CellRef, DateSystem};

    fn cr() -> CellRef {
        CellRef {
            sheet: 0,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        }
    }
    fn ctx() -> EvalCtx {
        EvalCtx::new(DateSystem::Date1900, cr(), 45000.5, 42)
    }
    fn n(x: f64) -> Arg<'static> {
        Arg::Scalar(CellValue::Number(x))
    }

    #[test]
    fn mround_sign_mismatch_is_num() {
        assert_eq!(mround(&[n(10.0), n(3.0)], &ctx()), CellValue::Number(9.0));
        assert_eq!(
            mround(&[n(5.0), n(-2.0)], &ctx()),
            CellValue::Error(CellError::Num)
        );
        assert_eq!(mround(&[n(0.0), n(5.0)], &ctx()), CellValue::Number(0.0));
    }

    #[test]
    fn even_odd_away_from_zero() {
        assert_eq!(even(&[n(3.0)], &ctx()), CellValue::Number(4.0));
        assert_eq!(even(&[n(-1.0)], &ctx()), CellValue::Number(-2.0));
        assert_eq!(odd(&[n(2.0)], &ctx()), CellValue::Number(3.0));
        assert_eq!(odd(&[n(0.0)], &ctx()), CellValue::Number(1.0));
    }

    #[test]
    fn fact_negative_is_num() {
        assert_eq!(fact(&[n(5.0)], &ctx()), CellValue::Number(120.0));
        assert_eq!(fact(&[n(-1.0)], &ctx()), CellValue::Error(CellError::Num));
        assert_eq!(factdouble(&[n(7.0)], &ctx()), CellValue::Number(105.0));
    }
}
