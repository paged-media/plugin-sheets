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

//! Volatile-pass support (spec §6.2). Volatile functions (`RAND`,
//! `RANDBETWEEN`, `NOW`, `TODAY`) must produce *fresh* output on every recalc
//! pass while staying deterministic under a fixed configuration seed. The
//! engine derives a per-cell RNG seed by mixing the config seed, a monotonic
//! **pass counter**, and the cell's address — so:
//!
//! - `RAND` varies across recalcs (the pass counter changes the seed each
//!   pass) but is reproducible given the same seed and pass count;
//! - two volatile cells in one pass get different streams (the cell address is
//!   mixed in), so `RAND()` in A1 and B1 differ.
//!
//! `NOW`/`TODAY` read `EvalCtx::now_serial`, which the engine carries from
//! [`crate::EngineConfig::now_serial`] / `set_now`; this module only owns the
//! RNG-seed derivation.

use sheet_core::CellRef;

/// Derive the deterministic per-cell RNG seed for one evaluation. Mixes the
/// configuration `base_seed`, the monotonic `pass` counter (so output changes
/// each recalc), and the cell's address (so co-evaluated volatile cells
/// diverge). Uses the splitmix64 finalizer so adjacent inputs avalanche.
pub fn cell_seed(base_seed: u64, pass: u64, cell: CellRef) -> u64 {
    let mut z = base_seed;
    z = mix(z ^ pass.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = mix(z ^ ((cell.sheet as u64) << 48 | (cell.row as u64) << 16 | cell.col as u64));
    z
}

/// splitmix64 finalizer.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
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

    #[test]
    fn same_inputs_same_seed() {
        assert_eq!(cell_seed(7, 3, cr(0, 0)), cell_seed(7, 3, cr(0, 0)));
    }

    #[test]
    fn pass_changes_seed() {
        assert_ne!(cell_seed(7, 1, cr(0, 0)), cell_seed(7, 2, cr(0, 0)));
    }

    #[test]
    fn cell_changes_seed() {
        assert_ne!(cell_seed(7, 1, cr(0, 0)), cell_seed(7, 1, cr(0, 1)));
    }

    #[test]
    fn base_seed_changes_seed() {
        assert_ne!(cell_seed(7, 1, cr(0, 0)), cell_seed(8, 1, cr(0, 0)));
    }
}
