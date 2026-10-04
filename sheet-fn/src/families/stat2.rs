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

//! Wave-7 statistics additions (spec §7): the exclusive percentile/quartile
//! pair, the least-squares forecast family (`FORECAST`, `FORECAST.LINEAR`,
//! `TREND`), `FREQUENCY`, and the normal distribution (`NORM.DIST`,
//! `NORM.INV`, `NORM.S.DIST`, `NORM.S.INV`). Provenance: the public Microsoft
//! function documentation for each name. The legacy aliases (`PERCENTILE`,
//! `QUARTILE`, `MODE.SNGL`) reuse the `stat` kernels by registry symbol.
//!
//! ## Rulings
//!
//! - **PERCENTILE.EXC** ranks at `k·(n+1)`; a rank below 1 or above `n` (and
//!   any `k` outside the open interval `(0,1)`) is `#NUM!`.
//! - **FORECAST / FORECAST.LINEAR** fit `y = a + b·x` over the position-wise
//!   numeric pairs (the `stat` pairing rule); the slope uses the centred sums
//!   (`Σ(x−x̄)(y−ȳ) / Σ(x−x̄)²`), zero spread in `x` is `#DIV/0!`, unequal
//!   cardinality `#N/A`.
//! - **TREND** is the single-regressor fit only; a multi-column `known_x`
//!   against a single-column `known_y` (multiple regression) is `#VALUE!` — a
//!   recorded scope limit, not a silent wrong answer. Non-numeric `known_y` →
//!   `#VALUE!`; `known_x` of a different count → `#REF!` (Excel).
//! - **FREQUENCY** returns a vertical `bins+1` block in the bins' own order:
//!   each value lands in the smallest bin `>=` it, values above every bin in
//!   the last slot; non-numeric data cells are ignored.
//! - **The normal family** uses an erf series / erfc continued fraction for the
//!   CDF (full double precision) and Acklam's rational inverse refined by two
//!   Halley steps for the quantile. `sd <= 0` → `#NUM!`; a probability outside
//!   `(0,1)` → `#NUM!`.

use sheet_core::{CellError, CellValue};

use super::stat::{finite, numbers_or_error, paired, scalar_number, sorted};
use crate::arg::Arg;
use crate::coerce;
use crate::ctx::EvalCtx;
use crate::result::FnResult;

// ---- PERCENTILE.EXC / QUARTILE.EXC -----------------------------------------

fn percentile_exc_of(s: &[f64], k: f64) -> Result<f64, CellError> {
    let n = s.len();
    if n == 0 || k <= 0.0 || k >= 1.0 {
        return Err(CellError::Num);
    }
    let rank = k * (n as f64 + 1.0);
    if rank < 1.0 || rank > n as f64 {
        return Err(CellError::Num);
    }
    let lo = rank.floor() as usize; // 1-based
    let frac = rank - lo as f64;
    if lo >= n {
        return Ok(s[n - 1]);
    }
    Ok(s[lo - 1] + frac * (s[lo] - s[lo - 1]))
}

/// `PERCENTILE.EXC(array, k)` (registry `sheet.fn.stat.percentile-exc`).
pub fn percentile_exc(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let k = match scalar_number(args.get(1)) {
        Ok(k) => k,
        Err(e) => return CellValue::Error(e),
    };
    let nums = match numbers_or_error(&args[..1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    match percentile_exc_of(&sorted(&nums), k) {
        Ok(v) => finite(v),
        Err(e) => CellValue::Error(e),
    }
}

/// `QUARTILE.EXC(array, quart)` (registry `sheet.fn.stat.quartile-exc`).
/// `quart` truncates to `1..=3` (else `#NUM!`) and equals
/// `PERCENTILE.EXC(array, quart/4)`.
pub fn quartile_exc(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let q = match scalar_number(args.get(1)) {
        Ok(q) => q.trunc(),
        Err(e) => return CellValue::Error(e),
    };
    let nums = match numbers_or_error(&args[..1]) {
        Ok(v) => v,
        Err(e) => return CellValue::Error(e),
    };
    if !(1.0..=3.0).contains(&q) {
        return CellValue::Error(CellError::Num);
    }
    match percentile_exc_of(&sorted(&nums), q / 4.0) {
        Ok(v) => finite(v),
        Err(e) => CellValue::Error(e),
    }
}

// ---- FORECAST / FORECAST.LINEAR / TREND ------------------------------------

/// Least-squares `(intercept, slope)` over `(y, x)` pairs, centred sums.
fn fit(pairs: &[(f64, f64)], with_const: bool) -> Result<(f64, f64), CellError> {
    if pairs.is_empty() {
        return Err(CellError::Div0);
    }
    let n = pairs.len() as f64;
    if !with_const {
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for &(y, x) in pairs {
            sxy += x * y;
            sxx += x * x;
        }
        if sxx == 0.0 {
            return Err(CellError::Div0);
        }
        return Ok((0.0, sxy / sxx));
    }
    let mx = pairs.iter().map(|p| p.1).sum::<f64>() / n;
    let my = pairs.iter().map(|p| p.0).sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for &(y, x) in pairs {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
    }
    if sxx == 0.0 {
        return Err(CellError::Div0);
    }
    let b = sxy / sxx;
    Ok((my - b * mx, b))
}

/// `FORECAST(x, known_ys, known_xs)` / `FORECAST.LINEAR` (registry
/// `sheet.fn.stat.forecast` / `sheet.fn.stat.forecast-linear`).
pub fn forecast(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let x = match scalar_number(args.first()) {
        Ok(x) => x,
        Err(e) => return CellValue::Error(e),
    };
    let pairs = match paired(&args[1..]) {
        Ok(p) => p,
        Err(e) => return CellValue::Error(e),
    };
    match fit(&pairs, true) {
        Ok((a, b)) => finite(a + b * x),
        Err(e) => CellValue::Error(e),
    }
}

/// A grid of an argument (a scalar is 1×1), row-major.
fn grid(arg: &Arg) -> Vec<Vec<CellValue>> {
    match arg {
        Arg::Scalar(v) => vec![vec![v.clone()]],
        Arg::Range(r) => (0..r.rows())
            .map(|rr| (0..r.cols()).map(|cc| r.get(rr, cc)).collect())
            .collect(),
    }
}

/// Every cell of a grid as a number; an error propagates, anything else
/// non-numeric is `#VALUE!` (TREND's strict numeric inputs).
fn strict_numbers(g: &[Vec<CellValue>]) -> Result<Vec<f64>, CellError> {
    let mut out = Vec::new();
    for row in g {
        for v in row {
            match v {
                CellValue::Number(n) => out.push(*n),
                CellValue::Error(e) => return Err(*e),
                _ => return Err(CellError::Value),
            }
        }
    }
    Ok(out)
}

/// `TREND(known_ys, [known_xs], [new_xs], [const])` (registry
/// `sheet.fn.stat.trend`). Single-regressor least squares; the result has the
/// shape of `new_xs` (default `known_xs`, default `{1..n}` shaped like
/// `known_ys`).
pub fn trend(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let fail = |e: CellError| FnResult::Scalar(CellValue::Error(e));
    let yg = grid(&args[0]);
    let ys = match strict_numbers(&yg) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let (xg, xs) = match args.get(1) {
        Some(a) => {
            let g = grid(a);
            let multi_col = g.first().map_or(0, |r| r.len()) > 1 && g.len() > 1;
            let multi_row = g.len() > 1 && g.first().map_or(0, |r| r.len()) > 1;
            if multi_col || multi_row {
                // Multiple regression — recorded scope limit.
                return fail(CellError::Value);
            }
            match strict_numbers(&g) {
                Ok(v) => (g, v),
                Err(e) => return fail(e),
            }
        }
        None => {
            let g: Vec<Vec<CellValue>> = yg
                .iter()
                .enumerate()
                .map(|(r, row)| {
                    row.iter()
                        .enumerate()
                        .map(|(c, _)| CellValue::Number((r * row.len() + c + 1) as f64))
                        .collect()
                })
                .collect();
            let v = (1..=ys.len()).map(|i| i as f64).collect();
            (g, v)
        }
    };
    if xs.len() != ys.len() {
        return fail(CellError::Ref);
    }
    let with_const = match args.get(3) {
        Some(Arg::Scalar(v)) => match coerce::to_bool(v) {
            Ok(b) => b,
            Err(e) => return fail(e),
        },
        Some(Arg::Range(r)) => match coerce::to_bool(&r.get(0, 0)) {
            Ok(b) => b,
            Err(e) => return fail(e),
        },
        None => true,
    };
    let pairs: Vec<(f64, f64)> = ys.iter().copied().zip(xs.iter().copied()).collect();
    let (a, b) = match fit(&pairs, with_const) {
        Ok(ab) => ab,
        Err(e) => return fail(e),
    };
    let target = match args.get(2) {
        Some(a) => grid(a),
        None => xg,
    };
    let mut out = Vec::with_capacity(target.len());
    for row in &target {
        let mut orow = Vec::with_capacity(row.len());
        for v in row {
            orow.push(match v {
                CellValue::Number(x) => finite(a + b * x),
                CellValue::Error(e) => CellValue::Error(*e),
                _ => CellValue::Error(CellError::Value),
            });
        }
        out.push(orow);
    }
    FnResult::Array(out)
}

// ---- FREQUENCY -------------------------------------------------------------

/// `FREQUENCY(data_array, bins_array)` (registry `sheet.fn.stat.frequency`).
pub fn frequency(args: &[Arg], _ctx: &EvalCtx) -> FnResult {
    let data = match numbers_or_error(&args[..1]) {
        Ok(v) => v,
        Err(e) => return FnResult::Scalar(CellValue::Error(e)),
    };
    let bins = match numbers_or_error(&args[1..2]) {
        Ok(v) => v,
        Err(e) => return FnResult::Scalar(CellValue::Error(e)),
    };
    if bins.is_empty() {
        return FnResult::Array(vec![vec![CellValue::Number(data.len() as f64)]]);
    }
    // Bin order by value (stable, so a duplicated bin's first copy wins).
    let mut order: Vec<usize> = (0..bins.len()).collect();
    order.sort_by(|&a, &b| {
        bins[a]
            .partial_cmp(&bins[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut counts = vec![0u64; bins.len() + 1];
    for &x in &data {
        match order.iter().find(|&&i| x <= bins[i]) {
            Some(&i) => counts[i] += 1,
            None => counts[bins.len()] += 1,
        }
    }
    FnResult::Array(
        counts
            .into_iter()
            .map(|c| vec![CellValue::Number(c as f64)])
            .collect(),
    )
}

// ---- the normal distribution -----------------------------------------------

const SQRT_2: f64 = std::f64::consts::SQRT_2;
const FRAC_2_SQRT_PI: f64 = std::f64::consts::FRAC_2_SQRT_PI;

/// `erf(x)` for `|x| <= 3` by the all-positive series
/// `2/√π · e^{−x²} · Σ 2ⁿ x^{2n+1} / (1·3·…·(2n+1))` (no cancellation).
fn erf_series(x: f64) -> f64 {
    let x2 = x * x;
    let mut term = x;
    let mut sum = x;
    let mut k = 1.0;
    for _ in 0..200 {
        term *= 2.0 * x2 / (2.0 * k + 1.0);
        sum += term;
        if term.abs() <= sum.abs() * 1e-17 {
            break;
        }
        k += 1.0;
    }
    FRAC_2_SQRT_PI * (-x2).exp() * sum
}

/// `erfc(x)` for `x > 0` by the continued fraction (modified Lentz).
fn erfc_cf(x: f64) -> f64 {
    // erfc(x) = e^{-x²}/√π · 1/(x + (1/2)/(x + 1/(x + (3/2)/(x + 2/(x + …)))))
    let tiny = 1e-300;
    let mut f = x;
    if f == 0.0 {
        f = tiny;
    }
    let mut c = f;
    let mut d = 0.0;
    for i in 1..500 {
        let a = i as f64 / 2.0;
        d = x + a * d;
        if d == 0.0 {
            d = tiny;
        }
        c = x + a / c;
        if c == 0.0 {
            c = tiny;
        }
        d = 1.0 / d;
        let delta = c * d;
        f *= delta;
        if (delta - 1.0).abs() < 1e-16 {
            break;
        }
    }
    (-x * x).exp() / std::f64::consts::PI.sqrt() / f
}

/// `erfc(x)` for any real `x`.
fn erfc(x: f64) -> f64 {
    if x < 0.0 {
        2.0 - erfc(-x)
    } else if x <= 3.0 {
        1.0 - erf_series(x)
    } else {
        erfc_cf(x)
    }
}

/// The standard normal CDF `Φ(z)`.
pub(crate) fn std_norm_cdf(z: f64) -> f64 {
    if z.abs() < 0.5 {
        0.5 + 0.5 * erf_series(z / SQRT_2)
    } else {
        0.5 * erfc(-z / SQRT_2)
    }
}

/// The standard normal density `φ(z)`.
fn std_norm_pdf(z: f64) -> f64 {
    (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// The standard normal quantile `Φ⁻¹(p)` for `0 < p < 1`: Acklam's rational
/// approximation, then two Halley refinements against [`std_norm_cdf`].
pub(crate) fn std_norm_inv(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let p_low = 0.02425;
    let mut x = if p < p_low {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - p_low {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    for _ in 0..2 {
        let e = std_norm_cdf(x) - p;
        let u = e * (2.0 * std::f64::consts::PI).sqrt() * (0.5 * x * x).exp();
        x -= u / (1.0 + x * u / 2.0);
    }
    x
}

/// Read arg `i` as a number (a missing arg is `#VALUE!`).
fn num_at(args: &[Arg], i: usize) -> Result<f64, CellError> {
    scalar_number(args.get(i))
}

/// Read arg `i` as a boolean.
fn bool_at(args: &[Arg], i: usize) -> Result<bool, CellError> {
    match args.get(i) {
        Some(Arg::Scalar(v)) => coerce::to_bool(v),
        Some(Arg::Range(r)) => coerce::to_bool(&r.get(0, 0)),
        None => Err(CellError::Value),
    }
}

/// `NORM.DIST(x, mean, standard_dev, cumulative)` (registry
/// `sheet.fn.stat.norm-dist`).
pub fn norm_dist(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let r = (|| {
        let x = num_at(args, 0)?;
        let mean = num_at(args, 1)?;
        let sd = num_at(args, 2)?;
        let cumulative = bool_at(args, 3)?;
        if sd <= 0.0 {
            return Err(CellError::Num);
        }
        let z = (x - mean) / sd;
        Ok(if cumulative {
            std_norm_cdf(z)
        } else {
            std_norm_pdf(z) / sd
        })
    })();
    match r {
        Ok(v) => finite(v),
        Err(e) => CellValue::Error(e),
    }
}

/// `NORM.INV(probability, mean, standard_dev)` (registry
/// `sheet.fn.stat.norm-inv`).
pub fn norm_inv(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let r = (|| {
        let p = num_at(args, 0)?;
        let mean = num_at(args, 1)?;
        let sd = num_at(args, 2)?;
        if p <= 0.0 || p >= 1.0 || sd <= 0.0 {
            return Err(CellError::Num);
        }
        Ok(mean + sd * std_norm_inv(p))
    })();
    match r {
        Ok(v) => finite(v),
        Err(e) => CellValue::Error(e),
    }
}

/// `NORM.S.DIST(z, cumulative)` (registry `sheet.fn.stat.norm-s-dist`).
pub fn norm_s_dist(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    let r = (|| {
        let z = num_at(args, 0)?;
        let cumulative = bool_at(args, 1)?;
        Ok(if cumulative {
            std_norm_cdf(z)
        } else {
            std_norm_pdf(z)
        })
    })();
    match r {
        Ok(v) => finite(v),
        Err(e) => CellValue::Error(e),
    }
}

/// `NORM.S.INV(probability)` (registry `sheet.fn.stat.norm-s-inv`).
pub fn norm_s_inv(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    match num_at(args, 0) {
        Ok(p) if p <= 0.0 || p >= 1.0 => CellValue::Error(CellError::Num),
        Ok(p) => finite(std_norm_inv(p)),
        Err(e) => CellValue::Error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdf_matches_reference_values() {
        // Python statistics.NormalDist (libm erf) reference values; relative
        // agreement to 1e-13 (the erfc = 1 − erf band loses ~1.5 digits near
        // |z| ≈ 2, still far inside Excel's 15-digit display).
        let close = |got: f64, want: f64| ((got - want) / want).abs() < 1e-13;
        assert!(close(std_norm_cdf(1.333333), 0.9087887256040951));
        assert!(close(std_norm_cdf(0.0), 0.5));
        assert!(close(std_norm_cdf(-1.96), 0.02499789514822043));
        assert!(close(std_norm_cdf(5.0), 0.9999997133484281));
        assert!(close(std_norm_cdf(-8.0), 6.22096057427182e-16));
    }

    #[test]
    fn inverse_round_trips() {
        for &p in &[
            1e-10,
            0.001,
            0.02,
            0.3,
            0.5,
            0.7,
            0.908789,
            0.99,
            1.0 - 1e-9,
        ] {
            let z = std_norm_inv(p);
            assert!(
                (std_norm_cdf(z) - p).abs() <= p.min(1.0 - p) * 1e-12,
                "p={p}"
            );
        }
        assert!((std_norm_inv(0.908789) - 1.3333346730441071).abs() < 1e-12);
    }
}
