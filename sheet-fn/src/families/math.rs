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

//! Math/trig family kernels (spec §7, §11 T0). Pure
//! `fn(&[Arg], &EvalCtx) -> CellValue` per the frozen calling convention
//! (`crate::arg`/`crate::ctx`). All type conversion and error propagation go
//! through [`crate::coerce`]; arithmetic accumulation routes through the
//! [`crate::num::Numeric`] seam (D-6: `f64` now, exact-decimal later), while
//! transcendental std calls (`sin`, `ln`, …) stay direct — the seam buys
//! nothing there (see `num.rs` doc).
//!
//! ## Excel rulings honored here (each is a registry-tested feature)
//!
//! - **MOD** sign follows the *divisor* (`MOD(-3, 2) == 1`), not the
//!   dividend — Excel/OpenFormula `=n - d*INT(n/d)`.
//! - **INT** floors toward −∞ (`INT(-1.5) == -2`), unlike TRUNC.
//! - **ROUND** rounds *half away from zero* (`ROUND(2.5) == 3`,
//!   `ROUND(-2.5) == -3`), not banker's rounding.
//! - **CEILING/FLOOR** are the *classic* 2-arg forms: when `sign(x)` differs
//!   from `sign(significance)` the result is `#NUM!`.
//! - **SQRT/LN/LOG/LOG10/ASIN/ACOS** of an out-of-domain argument → `#NUM!`.
//! - Division by zero (MOD by 0, LOG base issues) → `#DIV/0!`.
//! - **RAND/RANDBETWEEN** draw from `ctx.next_f64` (deterministic under a
//!   fixed seed); RANDBETWEEN's bounds are inclusive integers and `lo > hi`
//!   is `#NUM!`.
//!
//! ## The range-vs-scalar coercion asymmetry (documented once, here)
//!
//! `SUM`/`PRODUCT` iterate ranges *skipping non-numeric cells* — text,
//! bools, and blanks inside a RANGE are ignored (Excel/OpenFormula
//! aggregation rule). A non-numeric *scalar* argument, by contrast, is run
//! through [`coerce::to_number`]: the bool `TRUE` coerces to `1`, numeric
//! text `"5"` coerces to `5`, and un-parseable text is `#VALUE!`. So
//! `=SUM(A1:A3)` with a text cell skips it, but `=SUM("x")` errors — the
//! classic range-vs-scalar asymmetry. An *error* cell inside a range IS the
//! error (aggregation propagates the first error it meets).

use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;
use crate::num::{Numeric, F64};

// ---- small shared helpers --------------------------------------------------

/// Wrap an `f64` result, mapping a non-finite outcome (overflow, `0^neg`,
/// `inf`) to Excel's `#NUM!`. Real in-domain results are always finite.
#[inline]
fn finite(n: f64) -> CellValue {
    if n.is_finite() {
        CellValue::Number(n)
    } else {
        CellValue::Error(CellError::Num)
    }
}

/// Coerce a single scalar arg to a number, propagating any error or
/// un-parseable-text `#VALUE!` (the scalar branch of the asymmetry above).
#[inline]
fn scalar_num(arg: &Arg) -> Result<f64, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_number(v),
        // A range handed to a scalar-arity kernel: take its top-left cell
        // (implicit-intersection fallback is the caller's job in T1; here the
        // 1×1 range that `sheet-calc` builds for a single ref reads cleanly).
        Arg::Range(r) => coerce::to_number(&r.get(0, 0)),
    }
}

/// One numeric scalar argument or its propagated error — the spine of every
/// unary math kernel. `f` maps the value to a `CellValue`.
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

/// Accumulate the numeric contributions of one argument into `(acc, op)`,
/// short-circuiting on the first error cell. Scalars coerce via
/// [`coerce::to_number`] (text/bool participate); range cells skip everything
/// non-numeric but DO propagate an error cell. `start` is the accumulator and
/// `combine` folds each numeric value in.
fn fold_numeric<F>(
    args: &[Arg],
    start: F64,
    combine: impl Fn(F64, F64) -> F64,
    on_scalar: F,
) -> Result<F64, CellError>
where
    F: Fn(&CellValue) -> Result<Option<f64>, CellError>,
{
    let mut acc = start;
    for arg in args {
        match arg {
            Arg::Scalar(v) => {
                if let Some(n) = on_scalar(v)? {
                    acc = combine(acc, F64::from_f64(n));
                }
            }
            Arg::Range(r) => {
                for cell in r.iter() {
                    match &cell {
                        // Error cells in a range propagate (aggregation rule).
                        CellValue::Error(e) => return Err(*e),
                        // Only real numbers contribute; text/bool/blank skip.
                        CellValue::Number(n) => acc = combine(acc, F64::from_f64(*n)),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(acc)
}

// ---- aggregation -----------------------------------------------------------

/// `SUM(number1, [number2], …)` (spec §11). Sums numeric arguments; range
/// cells that are not numbers are skipped (the range-vs-scalar asymmetry —
/// see module doc), an error cell anywhere propagates, and a scalar arg is
/// coerced (text/bool participate, un-parseable text → `#VALUE!`).
pub fn sum(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match fold_numeric(
        args,
        F64(0.0),
        |a, b| a.add(b),
        |v| coerce::to_number(v).map(Some),
    ) {
        Ok(acc) => finite(acc.to_f64()),
        Err(e) => CellValue::Error(e),
    }
}

/// `PRODUCT(number1, [number2], …)` (spec §11). Like [`sum`] but multiplies;
/// the same range-skip / scalar-coerce / error-propagation rules apply. An
/// all-empty PRODUCT is `0` in Excel (no numeric factor seen), so the
/// accumulator tracks whether any factor was multiplied in.
pub fn product(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let mut acc = F64(1.0);
    let mut any = false;
    for arg in args {
        match arg {
            Arg::Scalar(v) => match coerce::to_number(v) {
                Ok(n) => {
                    acc = acc.mul(F64::from_f64(n));
                    any = true;
                }
                Err(e) => return CellValue::Error(e),
            },
            Arg::Range(r) => {
                for cell in r.iter() {
                    match &cell {
                        CellValue::Error(e) => return CellValue::Error(*e),
                        CellValue::Number(n) => {
                            acc = acc.mul(F64::from_f64(*n));
                            any = true;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    finite(if any { acc.to_f64() } else { 0.0 })
}

// ---- elementary ------------------------------------------------------------

/// `ABS(number)` — absolute value.
pub fn abs(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.abs()))
}

/// `SIGN(number)` — −1 / 0 / +1.
pub fn sign(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        CellValue::Number(if x > 0.0 {
            1.0
        } else if x < 0.0 {
            -1.0
        } else {
            0.0
        })
    })
}

/// `SQRT(number)` — `#NUM!` for a negative argument (out of domain).
pub fn sqrt(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x < 0.0 {
            CellValue::Error(CellError::Num)
        } else {
            finite(x.sqrt())
        }
    })
}

/// `EXP(number)` — e^x. Overflow to `inf` surfaces as `#NUM!`.
pub fn exp(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.exp()))
}

/// `LN(number)` — natural log; `#NUM!` for `x <= 0` (out of domain).
pub fn ln(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x <= 0.0 {
            CellValue::Error(CellError::Num)
        } else {
            finite(x.ln())
        }
    })
}

/// `LOG10(number)` — base-10 log; `#NUM!` for `x <= 0`.
pub fn log10(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if x <= 0.0 {
            CellValue::Error(CellError::Num)
        } else {
            finite(x.log10())
        }
    })
}

/// `LOG(number, [base])` — log of `number` to `base` (default 10). `#NUM!`
/// for a non-positive `number` or `base`; `#DIV/0!` when `base == 1`
/// (`ln(1) == 0` in the change-of-base denominator).
pub fn log(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let x = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let base = if args.len() >= 2 {
        match scalar_num(&args[1]) {
            Ok(v) => v,
            Err(e) => return CellValue::Error(e),
        }
    } else {
        10.0
    };
    if x <= 0.0 || base <= 0.0 {
        return CellValue::Error(CellError::Num);
    }
    if base == 1.0 {
        // Change-of-base denominator ln(1) == 0.
        return CellValue::Error(CellError::Div0);
    }
    finite(x.ln() / base.ln())
}

/// `POWER(number, power)` — `number^power`, the `^` operator. A non-finite
/// outcome (e.g. negative base to a fractional power, or `0^negative`)
/// surfaces as `#NUM!`.
pub fn power(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |x, y| finite(F64(x).pow(F64(y)).to_f64()))
}

/// `MOD(number, divisor)` — remainder whose **sign follows the divisor**
/// (`MOD(-3, 2) == 1`), per Excel/OpenFormula `n - d*floor(n/d)`. A zero
/// divisor is `#DIV/0!`.
pub fn mod_fn(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |n, d| {
        if d == 0.0 {
            return CellValue::Error(CellError::Div0);
        }
        // n - d*floor(n/d): the result takes the divisor's sign.
        let r = n - d * (n / d).floor();
        finite(r)
    })
}

// ---- rounding --------------------------------------------------------------

/// Round `x` to `digits` decimal places, *half away from zero* (the Excel
/// ROUND ruling). `digits` may be negative (round to tens/hundreds).
fn round_half_away(x: f64, digits: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let factor = 10f64.powi(digits.trunc() as i32);
    let scaled = x * factor;
    // `f64::round` is itself half-away-from-zero — exactly Excel's rule.
    scaled.round() / factor
}

/// `ROUND(number, num_digits)` — half away from zero.
pub fn round(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |x, d| finite(round_half_away(x, d)))
}

/// `ROUNDUP(number, num_digits)` — round away from zero (ceil of magnitude).
pub fn roundup(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |x, d| {
        let factor = 10f64.powi(d.trunc() as i32);
        let scaled = x * factor;
        // Away from zero: ceil the magnitude, restore the sign.
        let r = scaled.abs().ceil() * scaled.signum() / factor;
        finite(r)
    })
}

/// `ROUNDDOWN(number, num_digits)` — round toward zero (truncate at digits).
pub fn rounddown(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |x, d| {
        let factor = 10f64.powi(d.trunc() as i32);
        let r = (x * factor).trunc() / factor;
        finite(r)
    })
}

/// `TRUNC(number, [num_digits])` — truncate toward zero (default 0 digits).
/// Unlike INT, TRUNC never floors a negative toward −∞.
pub fn trunc(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = coerce::first_error(args) {
        return CellValue::Error(e);
    }
    let x = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let digits = if args.len() >= 2 {
        match scalar_num(&args[1]) {
            Ok(v) => v,
            Err(e) => return CellValue::Error(e),
        }
    } else {
        0.0
    };
    let factor = 10f64.powi(digits.trunc() as i32);
    finite((x * factor).trunc() / factor)
}

/// `INT(number)` — floor toward **negative infinity** (`INT(-1.5) == -2`),
/// the Excel ruling that distinguishes INT from TRUNC.
pub fn int(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.floor()))
}

/// `CEILING(number, significance)` — classic 2-arg form: round `number` up
/// (away from zero) to the nearest multiple of `significance`. A zero
/// significance yields `0`; `#NUM!` when the signs of `number` and
/// `significance` differ (the classic-form domain rule).
pub fn ceiling(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |x, sig| {
        if sig == 0.0 {
            return CellValue::Number(0.0);
        }
        if x.signum() != sig.signum() && x != 0.0 {
            return CellValue::Error(CellError::Num);
        }
        finite((x / sig).ceil() * sig)
    })
}

/// `FLOOR(number, significance)` — classic 2-arg form: round `number` down
/// (toward zero) to the nearest multiple of `significance`. A zero
/// significance is `#DIV/0!` (FLOOR's classic ruling differs from CEILING);
/// `#NUM!` when the signs differ.
pub fn floor(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |x, sig| {
        if sig == 0.0 {
            // Classic FLOOR divides by significance → #DIV/0! at zero.
            return CellValue::Error(CellError::Div0);
        }
        if x.signum() != sig.signum() && x != 0.0 {
            return CellValue::Error(CellError::Num);
        }
        finite((x / sig).floor() * sig)
    })
}

// ---- trig / angle ----------------------------------------------------------

/// `PI()` — the constant π (no arguments).
pub fn pi(_args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Number(std::f64::consts::PI)
}

/// `SIN(number)` — sine of an angle in radians.
pub fn sin(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.sin()))
}

/// `COS(number)` — cosine of an angle in radians.
pub fn cos(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.cos()))
}

/// `TAN(number)` — tangent of an angle in radians.
pub fn tan(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.tan()))
}

/// `ASIN(number)` — arcsine; `#NUM!` for `|x| > 1` (out of domain).
pub fn asin(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if !(-1.0..=1.0).contains(&x) {
            CellValue::Error(CellError::Num)
        } else {
            finite(x.asin())
        }
    })
}

/// `ACOS(number)` — arccosine; `#NUM!` for `|x| > 1`.
pub fn acos(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| {
        if !(-1.0..=1.0).contains(&x) {
            CellValue::Error(CellError::Num)
        } else {
            finite(x.acos())
        }
    })
}

/// `ATAN(number)` — arctangent in `(-π/2, π/2)`.
pub fn atan(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.atan()))
}

/// `ATAN2(x_num, y_num)` — angle of the point `(x, y)` in `(-π, π]`. NOTE
/// the **Excel argument order is `(x, y)`** (the reverse of C's `atan2(y,
/// x)`). `ATAN2(0, 0)` is `#DIV/0!` (undefined direction).
pub fn atan2(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    binary(args, |x, y| {
        if x == 0.0 && y == 0.0 {
            return CellValue::Error(CellError::Div0);
        }
        finite(y.atan2(x))
    })
}

/// `DEGREES(angle)` — radians → degrees.
pub fn degrees(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.to_degrees()))
}

/// `RADIANS(angle)` — degrees → radians.
pub fn radians(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    unary(args, |x| finite(x.to_radians()))
}

// ---- volatile (deterministic under a seeded ctx) ---------------------------

/// `RAND()` — a pseudo-random number in `[0, 1)`. Volatile; draws from
/// `ctx.next_f64`, so a fixed `rng_seed` reproduces the sequence (the
/// property the conformance suite relies on).
pub fn rand(_args: &[Arg], ctx: &EvalCtx) -> CellValue {
    CellValue::Number(ctx.next_f64())
}

/// `RANDBETWEEN(bottom, top)` — a random **integer** in the inclusive range
/// `[bottom, top]`. Bounds are first rounded up (bottom) / down (top) to
/// integers as Excel does for non-integers; `bottom > top` is `#NUM!`.
/// Volatile; uses `ctx.next_f64`.
pub fn randbetween(args: &[Arg], ctx: &EvalCtx) -> CellValue {
    binary(args, |lo, hi| {
        // Excel rounds bottom up and top down to integer bounds.
        let lo = lo.ceil();
        let hi = hi.floor();
        if lo > hi {
            return CellValue::Error(CellError::Num);
        }
        let span = (hi - lo) + 1.0;
        // Map [0,1) onto the integer span [lo, hi]; clamp guards the (never
        // reached for next_f64 < 1.0) top edge.
        let pick = lo + (ctx.next_f64() * span).floor();
        CellValue::Number(pick.min(hi))
    })
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
    fn mod_sign_follows_divisor() {
        assert_eq!(mod_fn(&[n(-3.0), n(2.0)], &ctx()), CellValue::Number(1.0));
        assert_eq!(mod_fn(&[n(3.0), n(-2.0)], &ctx()), CellValue::Number(-1.0));
        assert_eq!(
            mod_fn(&[n(3.0), n(0.0)], &ctx()),
            CellValue::Error(CellError::Div0)
        );
    }

    #[test]
    fn int_floors_toward_neg_inf() {
        assert_eq!(int(&[n(-1.5)], &ctx()), CellValue::Number(-2.0));
        assert_eq!(trunc(&[n(-1.5)], &ctx()), CellValue::Number(-1.0));
    }

    #[test]
    fn round_half_away() {
        assert_eq!(round(&[n(2.5), n(0.0)], &ctx()), CellValue::Number(3.0));
        assert_eq!(round(&[n(-2.5), n(0.0)], &ctx()), CellValue::Number(-3.0));
    }

    #[test]
    fn ceiling_floor_sign_rule() {
        assert_eq!(
            ceiling(&[n(-2.0), n(3.0)], &ctx()),
            CellValue::Error(CellError::Num)
        );
        assert_eq!(
            floor(&[n(2.0), n(0.0)], &ctx()),
            CellValue::Error(CellError::Div0)
        );
    }
}
