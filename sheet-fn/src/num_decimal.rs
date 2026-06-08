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

//! The D-6 exact-decimal SPIKE backend (spec §3, §5.1, M3) — gated behind the
//! `exact-decimal` cargo feature, OFF by default.
//!
//! v1 is and stays Excel-compatible `f64` ([`crate::num::F64`]); this module is
//! the *spike* deliverable that proves the [`crate::num::Numeric`] seam (D-6)
//! can carry an exact base-10 backend without rewriting any kernel. The
//! adopt/defer recommendation lives in `DECIMAL-SPIKE.md` at the repo root; the
//! registry ruling is `sheet.calc.decimal.*` (registry/features/decimal.yaml).
//!
//! ## Carrier
//!
//! The carrier is [`rust_decimal::Decimal`] (crate `rust_decimal`, **MIT**,
//! pure Rust, wasm-compatible, no native/C dependencies — its only required
//! deps are `arrayvec` + `num-traits`, both `MIT OR Apache-2.0`). It is a
//! 96-bit integer significand with a base-10 scale (0..=28), giving **28-29
//! significant decimal digits**. This is the classic "money/fixed-precision"
//! representation: `0.1 + 0.2` is exactly `0.3`, with no binary rounding error.
//!
//! Carrier alternatives considered and rejected for the spike: a hand-rolled
//! fixed-point `i128` (would re-implement rounding, scale alignment, and
//! `Display` from scratch — strictly more code for a worse-tested result);
//! `bigdecimal` (arbitrary precision, heavier, pulls `num-bigint`). 28-29
//! digits is comfortably wider than Excel's 15-significant-digit *display*
//! budget (D-6), so the carrier never limits a publishing workload.
//!
//! ## `from_f64` semantics (the divergence hinge)
//!
//! [`Decimal::from_f64`] is used with rust_decimal's default
//! *remove-excess-bits* rounding: `from_f64(0.1)` yields exactly `0.1`, not
//! `0.1000000000000000055…`. THIS is what makes the spike interesting —
//! lifting the *literal the user typed* (`0.1`) into an exact decimal, then
//! summing in base 10, avoids the IEEE-754 representation error that f64
//! carries through the same arithmetic. (The alternative,
//! `Decimal::from_f64_retain`, would preserve the binary error and defeat the
//! purpose — it is deliberately not used here.)
//!
//! Non-finite inputs (`NaN`, `±∞`) have no decimal analogue;
//! [`Decimal::from_f64`] returns `None` there. The seam never editorializes
//! (mirroring [`crate::num::F64`], which lets IEEE inf flow), so we map a
//! non-representable `f64` to [`Decimal::ZERO`] as a SPIKE-ONLY fallback. A
//! production decimal mode would route division-by-zero / overflow to
//! `#DIV/0!` / `#NUM!` in the family kernel, exactly as the f64 path does —
//! out of scope for the trait-level spike. See `DECIMAL-SPIKE.md`.
//!
//! ## `pow`
//!
//! `pow` follows rust_decimal's [`MathematicalOps::powd`]: **integer exponents
//! are exact** (repeated multiplication, `powi`/`powu`); a **fractional
//! exponent** falls back to the `e^(y·ln x)` Taylor approximation — i.e. it is
//! NOT exact and carries the documented `~1e-7` tolerance of rust_decimal's
//! `exp`/`ln`. This matches the spike-track ruling: decimal buys exactness for
//! `+ - * /` and integer powers (the accumulation that money cares about), not
//! for transcendental `^`. Fractional `POWER`/`^` would stay an f64-domain
//! operation in any real adoption. Documented here and in the registry row.

use crate::num::Numeric;
use rust_decimal::prelude::{Decimal as RawDecimal, FromPrimitive, MathematicalOps, ToPrimitive};

/// The exact base-10 SPIKE backend (D-6, behind `exact-decimal`). A transparent
/// newtype over [`rust_decimal::Decimal`] so it slots in beside
/// [`crate::num::F64`] as a drop-in [`Numeric`] — callers written against the
/// trait do not change. Exploratory: f64 remains the v1 default.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, Default)]
pub struct Decimal(pub RawDecimal);

impl Numeric for Decimal {
    /// Lift an `f64` literal into an exact decimal using rust_decimal's
    /// *remove-excess-bits* rounding (so `0.1_f64` becomes exactly `0.1`, the
    /// divergence hinge documented at the module top). Non-finite inputs have
    /// no decimal analogue and map to `0` as a SPIKE-ONLY fallback.
    #[inline]
    fn from_f64(v: f64) -> Self {
        Decimal(RawDecimal::from_f64(v).unwrap_or(RawDecimal::ZERO))
    }

    /// Lower back to `f64` for [`sheet_core::CellValue::Number`] storage and the
    /// formatter (the D-6 wire form). `to_f64` on a `Decimal` never returns
    /// `None` for an in-range value; the spike clamps the theoretical `None` to
    /// `0.0`.
    #[inline]
    fn to_f64(self) -> f64 {
        self.0.to_f64().unwrap_or(0.0)
    }

    #[inline]
    fn add(self, rhs: Self) -> Self {
        Decimal(self.0 + rhs.0)
    }

    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Decimal(self.0 - rhs.0)
    }

    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Decimal(self.0 * rhs.0)
    }

    #[inline]
    fn div(self, rhs: Self) -> Self {
        Decimal(self.0 / rhs.0)
    }

    /// `self` raised to `rhs`. Integer exponents are EXACT (rust_decimal's
    /// `powd` dispatches to `powi`/`powu`); a fractional exponent uses the
    /// `e^(y·ln x)` approximation and is NOT exact (documented caveat).
    #[inline]
    fn pow(self, rhs: Self) -> Self {
        Decimal(self.0.powd(rhs.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_roundtrip_and_ops() {
        assert_eq!(Decimal::from_f64(2.5).to_f64(), 2.5);
        assert_eq!(
            Decimal::from_f64(2.0).add(Decimal::from_f64(3.0)),
            Decimal::from_f64(5.0)
        );
        assert_eq!(
            Decimal::from_f64(5.0).sub(Decimal::from_f64(3.0)),
            Decimal::from_f64(2.0)
        );
        assert_eq!(
            Decimal::from_f64(2.0).mul(Decimal::from_f64(3.0)),
            Decimal::from_f64(6.0)
        );
        assert_eq!(
            Decimal::from_f64(6.0).div(Decimal::from_f64(3.0)),
            Decimal::from_f64(2.0)
        );
        assert_eq!(
            Decimal::from_f64(2.0).pow(Decimal::from_f64(10.0)),
            Decimal::from_f64(1024.0)
        );
    }

    #[test]
    fn decimal_is_exact_where_f64_is_not() {
        // The D-6 hinge: 0.1 + 0.2 is EXACTLY 0.3 in base-10 decimal, but
        // 0.30000000000000004 in IEEE-754 f64.
        let sum = Decimal::from_f64(0.1).add(Decimal::from_f64(0.2));
        assert_eq!(sum, Decimal::from_f64(0.3));
        // ...and the f64 round-trip of the *decimal* result is the clean 0.3,
        // not the binary-drifted value.
        assert_eq!(sum.to_f64(), 0.3);
        // Sanity: native f64 does NOT have this property.
        assert_ne!(0.1_f64 + 0.2_f64, 0.3_f64);
    }

    #[test]
    fn decimal_integer_pow_is_exact() {
        // Integer exponents route through exact repeated multiplication.
        assert_eq!(
            Decimal::from_f64(1.1).pow(Decimal::from_f64(2.0)).to_f64(),
            1.21
        );
    }

    #[test]
    fn decimal_nonfinite_from_f64_is_spike_fallback_zero() {
        // Documented SPIKE-ONLY behavior: NaN/inf have no decimal analogue.
        assert_eq!(Decimal::from_f64(f64::NAN).to_f64(), 0.0);
        assert_eq!(Decimal::from_f64(f64::INFINITY).to_f64(), 0.0);
    }
}
