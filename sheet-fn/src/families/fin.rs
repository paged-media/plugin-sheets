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

//! Financial (`fin`) family kernels (spec §7/§11 T1, milestone M1). Pure
//! `fn(&[Arg], &EvalCtx) -> CellValue` per the frozen calling convention
//! (`crate::arg`/`crate::ctx`); all coercion routes through [`crate::coerce`]
//! and accumulation through the [`crate::num::Numeric`] seam where it buys
//! anything (D-6). Sixteen functions: the annuity/time-value family
//! (`PMT`/`IPMT`/`PPMT`/`FV`/`PV`/`RATE`/`NPER`), the cash-flow family
//! (`NPV`/`IRR`/`XNPV`/`XIRR`/`MIRR`), and the depreciation family
//! (`SLN`/`SYD`/`DB`/`DDB`).
//!
//! ## The annuity equation and its sign convention (ECMA-376 §18.17.7)
//!
//! Every time-value function is a rearrangement of the ONE cash-flow
//! identity Excel uses:
//!
//! ```text
//! pv·(1+r)^n  +  pmt·(1 + r·type)·((1+r)^n − 1)/r  +  fv  =  0      (r ≠ 0)
//! pv  +  pmt·n  +  fv  =  0                                          (r = 0)
//! ```
//!
//! where `r` = periodic rate, `n` = number of periods, `type` ∈ {0,1}
//! (0 = end-of-period / ordinary annuity, 1 = begin-of-period / annuity
//! due). **Sign convention:** money you *pay out* is negative, money you
//! *receive* is positive — so a loan `PV` is positive (you receive it) and
//! `PMT` comes back negative (you pay it), while a savings goal `FV` is
//! positive against negative `PMT` deposits. Getting the signs right is the
//! whole game; the formulas below preserve them by construction.
//!
//! ## Iterative functions and their tolerances
//!
//! - **RATE** — Newton–Raphson on the annuity residual, Excel's default
//!   `guess = 0.1`, up to 40 iterations, convergence `|f| < 1e-7` (Excel's
//!   own published tolerance). Non-convergence → `#NUM!`.
//! - **IRR** — Newton–Raphson on `NPV(r) = 0` with Excel's default
//!   `guess = 0.1`; a bisection fallback brackets a sign change in
//!   `(−0.999…, 1e7)` when Newton wanders out of domain. ≤ 50 Newton
//!   steps, `|f| < 1e-7`. No root → `#NUM!`.
//! - **XIRR** — same as IRR but on the Actual/365 dated `XNPV(r) = 0`.
//!
//! ## Excel rulings honored here (each is a registry-tested feature row)
//!
//! - **NPV discounts the FIRST value one full period** (`v₁/(1+r)¹`), so it
//!   assumes flows occur at the *end* of each period. The classic idiom for
//!   an initial outlay at t₀ is `=v₀ + NPV(r, v₁…)` (the t₀ flow added
//!   outside) — NPV itself never leaves a flow undiscounted.
//! - **NPV scans ranges, skipping non-numeric cells**, but a non-numeric
//!   *scalar* argument coerces (the range-vs-scalar asymmetry, shared with
//!   the agg family). An error cell anywhere propagates.
//! - **XNPV/XIRR use Actual/365** day counting from the first date; dates
//!   are truncated to integers (Excel ignores the fractional/time part).
//!   A date strictly before the first date → `#NUM!`; mismatched
//!   value/date counts → `#NUM!`.
//! - **DB rounds the depreciation rate to 3 decimals** (`ROUND(1 −
//!   (salvage/cost)^(1/life), 3)`) — a documented Excel quirk that makes DB
//!   disagree with a "pure" fixed-declining computation. The first and the
//!   `life+1`-th periods are pro-rated by `month`/`(12 − month)`.
//! - **DDB factor defaults to 2** (double-declining) and never depreciates
//!   below salvage: each period takes `min(book·factor/life,
//!   book − salvage)`.
//! - **Depreciation domain guards** (Excel): cost/salvage negative, life
//!   ≤ 0, period ≤ 0 or > life (SYD/DDB/DB), `month` out of 1..=12 (DB)
//!   → `#NUM!`; `RATE`/`NPER` impossible parameters → `#NUM!`.

use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;

// ---- small shared helpers --------------------------------------------------

/// Wrap an `f64`, mapping a non-finite outcome (overflow, log of a
/// non-positive ratio, division blow-up) to Excel's `#NUM!`. Every
/// in-domain financial result is finite.
#[inline]
fn finite(n: f64) -> CellValue {
    if n.is_finite() {
        CellValue::Number(n)
    } else {
        CellValue::Error(CellError::Num)
    }
}

/// Coerce one scalar-position argument to a number, propagating an error or
/// un-parseable-text `#VALUE!`. A range handed to a scalar slot reads its
/// top-left cell (the 1×1 range `sheet-calc` builds for a single ref).
#[inline]
fn scalar_num(arg: &Arg) -> Result<f64, CellError> {
    match arg {
        Arg::Scalar(v) => coerce::to_number(v),
        Arg::Range(r) => coerce::to_number(&r.get(0, 0)),
    }
}

/// Read the optional argument at `idx`, defaulting to `default` when it is
/// absent. Present args coerce via [`scalar_num`] (so an error/`#VALUE!`
/// propagates).
#[inline]
fn opt_num(args: &[Arg], idx: usize, default: f64) -> Result<f64, CellError> {
    match args.get(idx) {
        Some(a) => scalar_num(a),
        None => Ok(default),
    }
}

/// Excel's `type` flag: any non-zero value means begin-of-period (1); zero
/// (or absent) means end-of-period (0). Returns the normalized `0.0`/`1.0`.
#[inline]
fn pay_type(args: &[Arg], idx: usize) -> Result<f64, CellError> {
    let t = opt_num(args, idx, 0.0)?;
    Ok(if t != 0.0 { 1.0 } else { 0.0 })
}

/// First-error pre-pass over the scalar arguments (the shared coercion
/// ruling). Range args are not scanned here — the cash-flow kernels iterate
/// their own ranges and propagate per cell.
#[inline]
fn scalar_first_error(args: &[Arg]) -> Option<CellError> {
    coerce::first_error(args)
}

/// Pull every numeric cell out of an `Arg`, in order, for the cash-flow
/// kernels (`NPV`/`IRR`/`MIRR`). Scalars coerce (text/bool participate);
/// range cells skip non-numeric values but propagate an error cell — the
/// same range-vs-scalar asymmetry the agg/math families document.
fn collect_flows(args: &[Arg], out: &mut Vec<f64>) -> Result<(), CellError> {
    for arg in args {
        match arg {
            Arg::Scalar(v) => out.push(coerce::to_number(v)?),
            Arg::Range(r) => {
                for cell in r.iter() {
                    match &cell {
                        CellValue::Error(e) => return Err(*e),
                        CellValue::Number(n) => out.push(*n),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- the annuity equation --------------------------------------------------

/// Future value of the annuity at the END of `n` periods (the left side of
/// the cash-flow identity solved for `fv`, negated). Used directly by `FV`
/// and as the running balance for `IPMT`/`PPMT`. `r = 0` is the linear case.
#[inline]
fn fv_of(r: f64, n: f64, pmt: f64, pv: f64, t: f64) -> f64 {
    if r == 0.0 {
        -(pv + pmt * n)
    } else {
        let g = (1.0 + r).powf(n);
        -(pv * g + pmt * (1.0 + r * t) * (g - 1.0) / r)
    }
}

// ---- PMT / IPMT / PPMT -----------------------------------------------------

/// `PMT(rate, nper, pv, [fv], [type])` — the level periodic payment that
/// amortizes `pv` to `fv` over `nper` periods. Outflows come back negative.
pub fn pmt(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    let (r, n, pv, fv, t) = match annuity5(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    finite(payment(r, n, pv, fv, t))
}

/// The PMT closed form. `nper = 0` is `#NUM!`-territory (no payments); the
/// `finite` wrapper at the call site turns the resulting `inf` into `#NUM!`.
#[inline]
fn payment(r: f64, n: f64, pv: f64, fv: f64, t: f64) -> f64 {
    if r == 0.0 {
        -(pv + fv) / n
    } else {
        let g = (1.0 + r).powf(n);
        -(pv * g + fv) * r / ((1.0 + r * t) * (g - 1.0))
    }
}

/// Parse the shared `(rate, nper, pv, [fv], [type])` tail used by
/// PMT/FV/PV-shaped kernels into a 5-tuple (defaults `fv = 0`, `type = 0`).
#[inline]
fn annuity5(args: &[Arg]) -> Result<(f64, f64, f64, f64, f64), CellError> {
    let r = scalar_num(&args[0])?;
    let n = scalar_num(&args[1])?;
    let third = scalar_num(&args[2])?;
    let fourth = opt_num(args, 3, 0.0)?;
    let t = pay_type(args, 4)?;
    Ok((r, n, third, fourth, t))
}

/// `IPMT(rate, per, nper, pv, [fv], [type])` — the interest portion of the
/// `per`-th payment. `per` outside `1..=nper` is `#NUM!`.
pub fn ipmt(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    match ipmt_ppmt(args) {
        Ok((ip, _)) => finite(ip),
        Err(e) => CellValue::Error(e),
    }
}

/// `PPMT(rate, per, nper, pv, [fv], [type])` — the principal portion of the
/// `per`-th payment (`payment − interest`).
pub fn ppmt(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    match ipmt_ppmt(args) {
        Ok((_, pp)) => finite(pp),
        Err(e) => CellValue::Error(e),
    }
}

/// Shared core for IPMT/PPMT, returning `(interest, principal)`. The
/// interest of period `per` is the rate times the outstanding balance at the
/// START of the period (= the FV of the annuity after `per − 1` payments);
/// the begin-of-period (`type = 1`) case discounts the interest one period,
/// with the first period carrying zero interest (the payment is made before
/// any interest accrues).
fn ipmt_ppmt(args: &[Arg]) -> Result<(f64, f64), CellError> {
    let r = scalar_num(&args[0])?;
    let per = scalar_num(&args[1])?;
    let n = scalar_num(&args[2])?;
    let pv = scalar_num(&args[3])?;
    let fv = opt_num(args, 4, 0.0)?;
    let t = pay_type(args, 5)?;

    if per < 1.0 || per > n {
        return Err(CellError::Num);
    }

    let pay = payment(r, n, pv, fv, t);
    let interest = if t == 1.0 {
        // Begin-of-period: the first payment accrues no interest; later
        // periods use the balance after (per − 2) payments, discounted once.
        if per == 1.0 {
            0.0
        } else {
            (fv_of(r, per - 2.0, pay, pv, 1.0) * r) / (1.0 + r)
        }
    } else {
        // End-of-period: interest on the balance after (per − 1) payments.
        fv_of(r, per - 1.0, pay, pv, 0.0) * r
    };
    let principal = pay - interest;
    Ok((interest, principal))
}

// ---- FV / PV ---------------------------------------------------------------

/// `FV(rate, nper, pmt, [pv], [type])` — the future value of an investment.
pub fn fv(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    // Argument order here is (rate, nper, pmt, pv, type), so reuse annuity5
    // with the third arg meaning `pmt`.
    let (r, n, pmt, pv, t) = match annuity5(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    finite(fv_of(r, n, pmt, pv, t))
}

/// `PV(rate, nper, pmt, [fv], [type])` — the present value of an annuity.
pub fn pv(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    // (rate, nper, pmt, fv, type); solve the identity for pv.
    let (r, n, pmt, fv, t) = match annuity5(args) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let pv = if r == 0.0 {
        -(fv + pmt * n)
    } else {
        let g = (1.0 + r).powf(n);
        -(fv + pmt * (1.0 + r * t) * (g - 1.0) / r) / g
    };
    finite(pv)
}

// ---- NPER ------------------------------------------------------------------

/// `NPER(rate, pmt, pv, [fv], [type])` — the number of periods. `r = 0` is
/// the linear `−(pv + fv)/pmt`; otherwise solve the identity's exponent with
/// a logarithm (non-positive argument → `#NUM!` via `finite`).
pub fn nper(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    let r = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let pmt = match scalar_num(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let pv = match scalar_num(&args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let fv = match opt_num(args, 3, 0.0) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let t = match pay_type(args, 4) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };

    let result = if r == 0.0 {
        if pmt == 0.0 {
            return CellValue::Error(CellError::Num);
        }
        -(pv + fv) / pmt
    } else {
        let adj = pmt * (1.0 + r * t) / r;
        let numerator = adj - fv;
        let denominator = pv + adj;
        // log of a non-positive ratio is NaN → #NUM! through `finite`.
        (numerator / denominator).ln() / (1.0 + r).ln()
    };
    finite(result)
}

// ---- RATE ------------------------------------------------------------------

/// `RATE(nper, pmt, pv, [fv], [type], [guess])` — the periodic rate, found
/// by Newton–Raphson on the annuity residual (Excel default `guess = 0.1`,
/// ≤ 40 iterations, `|f| < 1e-7`). Non-convergence → `#NUM!`.
pub fn rate(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    let n = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let pmt = match scalar_num(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let pv = match scalar_num(&args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let fv = match opt_num(args, 3, 0.0) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let t = match pay_type(args, 4) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let guess = match opt_num(args, 5, 0.1) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };

    // Residual: the annuity identity `pv·g + pmt·(1+r·t)·(g−1)/r + fv = 0`.
    // Since `fv_of` returns the NEGATION of that sum (the balance at the end
    // of `n` periods), the residual is `fv − fv_of(r, …)` — zero at the root.
    // (When `fv = 0` either sign shares the root; the explicit form keeps the
    // general `fv ≠ 0` case correct.) A finite-difference derivative keeps the
    // r=0 branch honest.
    let f = |r: f64| fv - fv_of(r, n, pmt, pv, t);

    let mut r = guess;
    for _ in 0..40 {
        let y = f(r);
        if y.abs() < 1e-7 {
            return finite(r);
        }
        // Central difference; small step relative to |r| but never zero.
        let h = 1e-6 * (1.0 + r.abs());
        let dy = (f(r + h) - f(r - h)) / (2.0 * h);
        if dy == 0.0 || !dy.is_finite() {
            break;
        }
        let next = r - y / dy;
        if !next.is_finite() {
            break;
        }
        if (next - r).abs() < 1e-9 {
            r = next;
            if f(r).abs() < 1e-7 {
                return finite(r);
            }
            break;
        }
        r = next;
    }
    // Final convergence check (Newton may have stepped onto the root last).
    if f(r).abs() < 1e-7 {
        finite(r)
    } else {
        CellValue::Error(CellError::Num)
    }
}

// ---- NPV / IRR -------------------------------------------------------------

/// `NPV(rate, value1, [value2], …)` — net present value of a series of
/// future cash flows, the FIRST discounted one full period (Excel's
/// end-of-period assumption). `rate = −1` → `#NUM!`.
pub fn npv(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    // Only the rate (a scalar) participates in the scalar error pre-pass;
    // the value args may legitimately be ranges scanned below.
    let rate = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if rate == -1.0 {
        return CellValue::Error(CellError::Num);
    }
    let mut flows = Vec::new();
    if let Err(e) = collect_flows(&args[1..], &mut flows) {
        return CellValue::Error(e);
    }
    finite(npv_at(rate, &flows))
}

/// NPV of `flows` (first flow discounted one period) at `rate`.
#[inline]
fn npv_at(rate: f64, flows: &[f64]) -> f64 {
    let base = 1.0 + rate;
    let mut acc = 0.0;
    let mut disc = base;
    for &c in flows {
        acc += c / disc;
        disc *= base;
    }
    acc
}

/// The cash-flow valuation `IRR`/`MIRR` share: NPV with the FIRST flow
/// UNDISCOUNTED (period 0), the convention IRR roots against.
#[inline]
fn npv0(rate: f64, flows: &[f64]) -> f64 {
    let base = 1.0 + rate;
    let mut acc = 0.0;
    let mut disc = 1.0;
    for &c in flows {
        acc += c / disc;
        disc *= base;
    }
    acc
}

/// `IRR(values, [guess])` — the rate making `npv0(values) = 0`. Newton from
/// `guess` (default 0.1), then a bisection fallback over `(−0.999999, 1e7)`
/// when Newton fails to find a sign-bracketed root. No root → `#NUM!`.
pub fn irr(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let mut flows = Vec::new();
    // values is args[0]; an optional guess is the LAST scalar arg.
    if let Err(e) = collect_flows(&args[..1], &mut flows) {
        return CellValue::Error(e);
    }
    if flows.len() < 2 {
        return CellValue::Error(CellError::Num);
    }
    let guess = match args.get(1) {
        Some(a) => match scalar_num(a) {
            Ok(v) => v,
            Err(e) => return CellValue::Error(e),
        },
        None => 0.1,
    };
    match solve_irr(|r| npv0(r, &flows), guess) {
        Some(r) => finite(r),
        None => CellValue::Error(CellError::Num),
    }
}

/// Shared root-finder for IRR/XIRR: Newton from `guess` (≤ 50 steps,
/// `|f| < 1e-7`), then bisection over a wide bracket if Newton leaves the
/// `(−1, ∞)` domain or stalls. `f` is the NPV-at-rate functional.
fn solve_irr(f: impl Fn(f64) -> f64, guess: f64) -> Option<f64> {
    // --- Newton phase ---
    let mut r = if guess <= -1.0 { 0.1 } else { guess };
    for _ in 0..50 {
        let y = f(r);
        if y.abs() < 1e-7 {
            return Some(r);
        }
        let h = 1e-6 * (1.0 + r.abs());
        let dy = (f(r + h) - f(r - h)) / (2.0 * h);
        if dy == 0.0 || !dy.is_finite() {
            break;
        }
        let next = r - y / dy;
        if !next.is_finite() || next <= -1.0 {
            break;
        }
        if (next - r).abs() < 1e-10 {
            return Some(next);
        }
        r = next;
    }
    if f(r).abs() < 1e-7 {
        return Some(r);
    }

    // --- Bisection fallback: scan for a sign change over a wide bracket. ---
    let mut lo = -0.999_999_f64;
    let mut flo = f(lo);
    let steps = 2000;
    let hi_max = 1e7_f64;
    // Geometric-ish scan from just above −1 up to a very large rate.
    let mut prev = lo;
    let mut fprev = flo;
    for i in 1..=steps {
        let frac = i as f64 / steps as f64;
        // Map [0,1] onto (−1, 1e7] with denser sampling near −1.
        let cur = -0.999_999 + (hi_max + 0.999_999) * frac.powi(3);
        let fcur = f(cur);
        if fprev == 0.0 {
            return Some(prev);
        }
        if (fprev < 0.0) != (fcur < 0.0) && fcur.is_finite() && fprev.is_finite() {
            lo = prev;
            flo = fprev;
            let mut hi = cur;
            // Bisect the bracket.
            for _ in 0..200 {
                let mid = 0.5 * (lo + hi);
                let fmid = f(mid);
                if fmid.abs() < 1e-9 || (hi - lo).abs() < 1e-12 {
                    return Some(mid);
                }
                if (flo < 0.0) != (fmid < 0.0) {
                    hi = mid;
                } else {
                    lo = mid;
                    flo = fmid;
                }
            }
            return Some(0.5 * (lo + hi));
        }
        prev = cur;
        fprev = fcur;
    }
    let _ = flo;
    None
}

// ---- XNPV / XIRR (Actual/365 dated flows) ----------------------------------

/// Extract aligned `(values, dates)` from two range/scalar args. Excel
/// truncates each date to an integer serial; the counts must match and at
/// least two flows are required.
fn dated_flows(values: &Arg, dates: &Arg) -> Result<(Vec<f64>, Vec<f64>), CellError> {
    let mut v = Vec::new();
    let mut d = Vec::new();
    collect_flows(std::slice::from_ref(values), &mut v)?;
    // Dates: collect every numeric cell, truncated to an integer serial.
    let mut raw_dates = Vec::new();
    collect_flows(std::slice::from_ref(dates), &mut raw_dates)?;
    for x in raw_dates {
        d.push(x.trunc());
    }
    if v.len() != d.len() || v.len() < 2 {
        return Err(CellError::Num);
    }
    Ok((v, d))
}

/// XNPV residual at `rate`: Σ vᵢ / (1+rate)^((dᵢ − d₀)/365), Actual/365.
/// A date strictly before `d₀` is `#NUM!` (caller checks the ordering once).
#[inline]
fn xnpv_at(rate: f64, values: &[f64], dates: &[f64]) -> f64 {
    let d0 = dates[0];
    let base = 1.0 + rate;
    let mut acc = 0.0;
    for (&v, &d) in values.iter().zip(dates) {
        let exp = (d - d0) / 365.0;
        acc += v / base.powf(exp);
    }
    acc
}

/// `XNPV(rate, values, dates)` — NPV of dated cash flows on Actual/365. The
/// first date anchors the discounting; an earlier date → `#NUM!`.
pub fn xnpv(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let rate = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if rate <= -1.0 {
        return CellValue::Error(CellError::Num);
    }
    let (values, dates) = match dated_flows(&args[1], &args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let d0 = dates[0];
    if dates.iter().any(|&d| d < d0) {
        return CellValue::Error(CellError::Num);
    }
    finite(xnpv_at(rate, &values, &dates))
}

/// `XIRR(values, dates, [guess])` — the rate making `XNPV = 0` on
/// Actual/365. Newton/bisection via [`solve_irr`], default `guess = 0.1`.
pub fn xirr(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let (values, dates) = match dated_flows(&args[0], &args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let d0 = dates[0];
    if dates.iter().any(|&d| d < d0) {
        return CellValue::Error(CellError::Num);
    }
    let guess = match args.get(2) {
        Some(a) => match scalar_num(a) {
            Ok(v) => v,
            Err(e) => return CellValue::Error(e),
        },
        None => 0.1,
    };
    match solve_irr(|r| xnpv_at(r, &values, &dates), guess) {
        Some(r) => finite(r),
        None => CellValue::Error(CellError::Num),
    }
}

// ---- MIRR ------------------------------------------------------------------

/// `MIRR(values, finance_rate, reinvest_rate)` — the modified IRR: the
/// positive flows are compounded forward at `reinvest_rate`, the negative
/// flows discounted back at `finance_rate`, and the period-root of their
/// ratio less one is the answer. Requires at least one inflow AND one
/// outflow (else `#DIV/0!`).
pub fn mirr(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let mut flows = Vec::new();
    if let Err(e) = collect_flows(&args[..1], &mut flows) {
        return CellValue::Error(e);
    }
    let finance = match scalar_num(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let reinvest = match scalar_num(&args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let n = flows.len();
    if n < 2 {
        return CellValue::Error(CellError::Div0);
    }

    // PV of the negatives at the finance rate (period 0 weighting):
    let mut neg = 0.0;
    let mut has_neg = false;
    let fbase = 1.0 + finance;
    for (i, &c) in flows.iter().enumerate() {
        if c < 0.0 {
            has_neg = true;
            neg += c / fbase.powi(i as i32);
        }
    }
    // FV of the positives at the reinvest rate (compounded to period n−1):
    let mut pos = 0.0;
    let mut has_pos = false;
    let rbase = 1.0 + reinvest;
    for (i, &c) in flows.iter().enumerate() {
        if c > 0.0 {
            has_pos = true;
            pos += c * rbase.powi((n - 1 - i) as i32);
        }
    }
    if !has_neg || !has_pos || neg == 0.0 {
        return CellValue::Error(CellError::Div0);
    }

    let ratio = -pos / neg;
    let result = ratio.powf(1.0 / (n as f64 - 1.0)) - 1.0;
    finite(result)
}

// ---- depreciation: SLN / SYD / DB / DDB ------------------------------------

/// `SLN(cost, salvage, life)` — straight-line depreciation per period:
/// `(cost − salvage) / life`. `life = 0` → `#DIV/0!`.
pub fn sln(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    let cost = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let salvage = match scalar_num(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let life = match scalar_num(&args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if life == 0.0 {
        return CellValue::Error(CellError::Div0);
    }
    finite((cost - salvage) / life)
}

/// `SYD(cost, salvage, life, per)` — sum-of-years'-digits depreciation:
/// `(cost − salvage)·(life − per + 1)·2 / (life·(life + 1))`. `life ≤ 0` or
/// `per` outside `1..=life` → `#NUM!`.
pub fn syd(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    let cost = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let salvage = match scalar_num(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let life = match scalar_num(&args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let per = match scalar_num(&args[3]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if life <= 0.0 || per < 1.0 || per > life {
        return CellValue::Error(CellError::Num);
    }
    finite((cost - salvage) * (life - per + 1.0) * 2.0 / (life * (life + 1.0)))
}

/// `DDB(cost, salvage, life, period, [factor])` — declining-balance
/// depreciation, `factor` defaulting to 2 (double-declining). Each period
/// takes `min(book·factor/life, book − salvage)`, never dipping below
/// salvage; the cumulative sweep up to `period` gives the period's charge.
/// Domain: cost/salvage ≥ 0, `life > 0`, `1 ≤ period ≤ life`, `factor > 0`.
pub fn ddb(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    let cost = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let salvage = match scalar_num(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let life = match scalar_num(&args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let period = match scalar_num(&args[3]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let factor = match opt_num(args, 4, 2.0) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period < 1.0 || period > life || factor <= 0.0
    {
        return CellValue::Error(CellError::Num);
    }

    let rate = factor / life;
    let last = period.floor() as u64;
    let mut book = cost;
    let mut dep = 0.0;
    for _ in 0..last {
        dep = (book * rate).min((book - salvage).max(0.0));
        book -= dep;
    }
    finite(dep)
}

/// `DB(cost, salvage, life, period, [month])` — fixed-declining-balance
/// depreciation. **Excel ruling:** the rate is `ROUND(1 −
/// (salvage/cost)^(1/life), 3)` — rounded to three decimals, the documented
/// quirk that makes DB diverge from a pure declining computation. The first
/// period is pro-rated by `month/12`; the `(life+1)`-th (partial) period by
/// `(12 − month)/12`. `month` defaults to 12. Domain: cost > 0, salvage ≥ 0,
/// `life > 0`, `1 ≤ period ≤ life + 1`, `1 ≤ month ≤ 12`.
pub fn db(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    if let Some(e) = scalar_first_error(args) {
        return CellValue::Error(e);
    }
    let cost = match scalar_num(&args[0]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let salvage = match scalar_num(&args[1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let life = match scalar_num(&args[2]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let period = match scalar_num(&args[3]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let month = match opt_num(args, 4, 12.0) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    let month = month.trunc();
    if cost <= 0.0
        || salvage < 0.0
        || life <= 0.0
        || period < 1.0
        || period > life + 1.0
        || !(1.0..=12.0).contains(&month)
    {
        return CellValue::Error(CellError::Num);
    }

    // The Excel three-decimal-rounded rate (the documented DB quirk):
    // ROUND(1 − (salvage/cost)^(1/life), 3).
    let rate = {
        let r = 1.0 - (salvage / cost).powf(1.0 / life);
        (r * 1000.0).round() / 1000.0
    };

    let life_i = life.floor() as u64;
    let per_i = period.floor() as u64;

    // Period 1 (partial-year start).
    let first = cost * rate * month / 12.0;
    if per_i == 1 {
        return finite(first);
    }

    // Sweep periods 2..=per_i, tracking accumulated depreciation.
    let mut total = first;
    let mut dep = first;
    for p in 2..=per_i {
        if p == life_i + 1 {
            // The trailing partial period uses the remaining months.
            dep = (cost - total) * rate * (12.0 - month) / 12.0;
        } else {
            dep = (cost - total) * rate;
        }
        total += dep;
    }
    finite(dep)
}
