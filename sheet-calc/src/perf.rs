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

//! Work counters (the `perf-counters` feature) — the numbers the paged.sheet
//! perf budgets stand on (`tests/perf_budgets.rs`).
//!
//! A recalc's cost is not its wall clock (that moves with the machine); it
//! is the WORK it does: how many registered range boxes a dirty walk probes,
//! how many cells a range argument copies, how many formulas it evaluates.
//! Those are the same on a laptop and a CI runner, and a fix that halves one
//! halves it everywhere.
//!
//! Thread-local, so parallel test threads never see each other's work.
//!
//! With the feature OFF (every shipped build — the release wasm, the native
//! crates) this module does not exist and the `perf_count!` sites expand to
//! NOTHING: no thread-local, no branch, no argument evaluation.

use std::cell::Cell;

/// One snapshot of every counter. Each field names the site that bumps it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PerfCounters {
    /// `DepGraph::dependents_of` calls — one per cell a dirty walk visits
    /// (`graph.rs`). Each is a probe of the range-dependency index.
    pub range_probes: u64,
    /// Registered range boxes EXAMINED across those probes. Every probe scans
    /// every registered box today (the M1 interval-index seam), so this is
    /// probes × boxes — the quadratic the interval index removes.
    pub range_keys_scanned: u64,
    /// Range-box × dirty-candidate containment tests in
    /// `DepGraph::precedents_in` (`graph.rs`, per dirty cell from `topo`).
    pub precedent_candidates_scanned: u64,
    /// Range views built (`argview.rs` `materialize_range*`) — one per range
    /// argument per evaluation.
    pub ranges_materialized: u64,
    /// Cells a kernel READ through those views inside the sheet's populated
    /// rows (`argview.rs`). Views borrow the model, so this is the kernel's
    /// own work (a SUM reads its range, a VLOOKUP reads until it matches);
    /// nothing is copied up front. (The `cells_materialized` copy counter and
    /// the per-evaluation `ast_clones` counter were retired 2026-10 when the
    /// copy and the clone they counted were removed.)
    pub cells_read: u64,
    /// Formula evaluations, scalar and rich door (`lib.rs`).
    pub evaluations: u64,
    /// `Engine::recalc_dirty` invocations — one per committed edit today.
    pub recalcs: u64,
    /// Fixpoint passes inside those recalcs (more than `recalcs` only when a
    /// spill reflows).
    pub recalc_passes: u64,
    /// Cells NEWLY added to the dirty set (mark, mark_all, propagation).
    pub cells_marked_dirty: u64,
}

thread_local! {
    static COUNTERS: Cell<PerfCounters> = const {
        Cell::new(PerfCounters {
            range_probes: 0,
            range_keys_scanned: 0,
            precedent_candidates_scanned: 0,
            ranges_materialized: 0,
            cells_read: 0,
            evaluations: 0,
            recalcs: 0,
            recalc_passes: 0,
            cells_marked_dirty: 0,
        })
    };
}

/// Read every counter on this thread.
pub fn snapshot() -> PerfCounters {
    COUNTERS.with(Cell::get)
}

/// Zero every counter on this thread.
pub fn reset() {
    COUNTERS.with(|c| c.set(PerfCounters::default()));
}

/// Apply one bump. Called only through `perf_count!`.
#[doc(hidden)]
#[inline]
pub fn bump(f: impl FnOnce(&mut PerfCounters)) {
    COUNTERS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}
