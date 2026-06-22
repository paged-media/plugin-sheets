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

//! The evaluation context (spec §7). [`EvalCtx`] is the read-mostly side
//! channel a kernel needs that is *not* an argument: the workbook date
//! system (date/serial functions), the address of the cell being evaluated
//! (no-arg `ROW()`/`COLUMN()`), an injected wall-clock serial (`NOW`/
//! `TODAY`, so volatile time is deterministic under test), and a
//! deterministic RNG (`RAND`/`RANDBETWEEN`). Kernels stay pure: every
//! observable comes through `&EvalCtx`, never ambient state.
//!
//! # FREEZE NOTICE — this surface is FROZEN (M0 phase 0, Track FN-CONV)
//!
//! Family agents and `sheet-calc` build against [`EvalCtx`] *exactly as
//! written*. See the freeze notice on `arg.rs`.

use std::cell::Cell;

use sheet_core::{CellRef, DateSystem};

/// The non-argument inputs to a function kernel (spec §7). Construct with
/// [`EvalCtx::new`]; the RNG is seeded so volatile functions are
/// reproducible (tests pass a constant `now_serial` and `rng_seed`).
pub struct EvalCtx {
    /// Workbook serial-date epoch (1900 leap-bug vs 1904 Mac).
    pub date_system: DateSystem,
    /// The cell currently being evaluated — the answer to no-arg `ROW()` /
    /// `COLUMN()` and the anchor for relative reference functions.
    pub current: CellRef,
    /// Injected wall-clock serial for `NOW`/`TODAY`. The caller computes the
    /// real serial; tests pass a constant so volatile time is deterministic.
    pub now_serial: f64,
    /// splitmix64 state, advanced by [`EvalCtx::next_f64`]. `Cell` so a
    /// kernel taking `&EvalCtx` can still draw numbers without `&mut`.
    rng_state: Cell<u64>,
}

impl EvalCtx {
    /// Build a context. `rng_seed` seeds the splitmix64 stream so a given
    /// seed reproduces the same `RAND`/`RANDBETWEEN` sequence — the property
    /// the conformance suite relies on for volatile functions.
    pub fn new(date_system: DateSystem, current: CellRef, now_serial: f64, rng_seed: u64) -> Self {
        EvalCtx {
            date_system,
            current,
            now_serial,
            rng_state: Cell::new(rng_seed),
        }
    }

    /// Draw the next pseudo-random `f64` in `[0, 1)` (one splitmix64 step).
    /// Deterministic from the seed: same seed → same sequence. This is the
    /// only randomness a kernel may use (`RAND` returns this directly;
    /// `RANDBETWEEN` scales it) — keeping RNG in the context is what makes
    /// volatile functions testable.
    pub fn next_f64(&self) -> f64 {
        // splitmix64 (Vigna): advance the state, then avalanche.
        let mut z = self.rng_state.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.rng_state.set(z);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Take the top 53 bits → a double in [0, 1) (the standard
        // construction; 53 = f64 mantissa bits, so every value is exact).
        ((z >> 11) as f64) * (1.0 / (1u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr(row: u32, col: u32) -> CellRef {
        CellRef {
            sheet: 0,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn ctx(seed: u64) -> EvalCtx {
        EvalCtx::new(DateSystem::Date1900, cr(0, 0), 0.0, seed)
    }

    #[test]
    fn next_f64_in_unit_interval() {
        let c = ctx(42);
        for _ in 0..10_000 {
            let x = c.next_f64();
            assert!((0.0..1.0).contains(&x), "out of [0,1): {x}");
        }
    }

    #[test]
    fn same_seed_same_sequence() {
        let a = ctx(123);
        let b = ctx(123);
        for _ in 0..100 {
            assert_eq!(a.next_f64(), b.next_f64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let a = ctx(1);
        let b = ctx(2);
        // Overwhelmingly likely to differ on the first draw.
        assert_ne!(a.next_f64(), b.next_f64());
    }

    #[test]
    fn fields_are_carried() {
        let c = EvalCtx::new(DateSystem::Date1904, cr(3, 4), 45000.5, 7);
        assert_eq!(c.date_system, DateSystem::Date1904);
        assert_eq!(c.current, cr(3, 4));
        assert_eq!(c.now_serial, 45000.5);
    }
}
