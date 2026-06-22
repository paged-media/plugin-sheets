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

//! The numeric seam (spec §5.1 decision **D-6**). v1 is Excel-compatible —
//! `f64` arithmetic — but the precision policy keeps an exact-decimal mode
//! open as a possible v2 differentiator. The [`Numeric`] trait is that
//! boundary: family kernels SHOULD route their nontrivial arithmetic
//! (sums, products, the `+ - * / ^` of accumulation loops) through a
//! `Numeric` so swapping in a decimal backend is a type substitution, not
//! a rewrite. Trivial transcendental std calls (`f64::sin`, `f64::ln`, …)
//! that have no decimal analogue MAY stay direct — the seam buys nothing
//! there and the doc says so.
//!
//! T0 ships exactly one impl, [`F64`] (the IEEE-754 double of D-6). Adding
//! a `Decimal` impl later does not touch any caller written against the
//! trait.

/// A numeric backend for spreadsheet arithmetic (spec §5.1 / D-6). The
/// operations are the closure of `+ - * / ^` over the numeric domain plus
/// the `f64` round-trip used at the coercion / formatting boundary. Kept
/// deliberately small: it is the *seam*, not a math library — transcendental
/// functions live in the family kernels, not here.
pub trait Numeric: Copy {
    /// Lift an `f64` (the wire/coercion form, D-6) into the backend.
    fn from_f64(v: f64) -> Self;
    /// Lower back to `f64` for storage in [`sheet_core::CellValue::Number`]
    /// and for the formatter.
    fn to_f64(self) -> f64;
    fn add(self, rhs: Self) -> Self;
    fn sub(self, rhs: Self) -> Self;
    fn mul(self, rhs: Self) -> Self;
    fn div(self, rhs: Self) -> Self;
    /// `self` raised to `rhs` (the `^` operator / `POWER`).
    fn pow(self, rhs: Self) -> Self;
}

/// The Excel-compatible IEEE-754 backend (D-6, T0's only [`Numeric`]). A
/// transparent `f64` newtype: zero-cost, and the place a future
/// exact-decimal type would slot in beside without disturbing callers.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, Default)]
pub struct F64(pub f64);

impl Numeric for F64 {
    #[inline]
    fn from_f64(v: f64) -> Self {
        F64(v)
    }
    #[inline]
    fn to_f64(self) -> f64 {
        self.0
    }
    #[inline]
    fn add(self, rhs: Self) -> Self {
        F64(self.0 + rhs.0)
    }
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        F64(self.0 - rhs.0)
    }
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        F64(self.0 * rhs.0)
    }
    #[inline]
    fn div(self, rhs: Self) -> Self {
        F64(self.0 / rhs.0)
    }
    #[inline]
    fn pow(self, rhs: Self) -> Self {
        F64(self.0.powf(rhs.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_roundtrip_and_ops() {
        assert_eq!(F64::from_f64(2.5).to_f64(), 2.5);
        assert_eq!(F64(2.0).add(F64(3.0)), F64(5.0));
        assert_eq!(F64(5.0).sub(F64(3.0)), F64(2.0));
        assert_eq!(F64(2.0).mul(F64(3.0)), F64(6.0));
        assert_eq!(F64(6.0).div(F64(3.0)), F64(2.0));
        assert_eq!(F64(2.0).pow(F64(10.0)), F64(1024.0));
    }

    #[test]
    fn f64_div_by_zero_is_inf_not_panic() {
        // The seam does not editorialize: division by zero yields IEEE inf;
        // family kernels decide whether that becomes `#DIV/0!`.
        assert!(F64(1.0).div(F64(0.0)).to_f64().is_infinite());
    }
}
