# ADR 502 — Recalculation: a range-keyed graph, deterministic order, spill by fixpoint

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `sheet-calc` (`Engine`, `graph`, `dirty`, `topo`, `spill`, `iterate`, `volatile`)

## Context

`sheet_calc::Engine` owns the workbook model, a dependency graph, a dirty set, a pass
counter and a spill ledger. A write through `set_cell` replaces the cell's registration in
the graph, marks its dependents dirty and recalculates the dirty set.

The module comments give reasons for single parts. A range is one node because "a
`A1:A1000000` edge set would be ruinous" (`sheet-calc/src/graph.rs:41-42`). The order is
stable so that "every computed value, never depends on hash iteration order"
(`sheet-calc/src/topo.rs:36-40`); a registry row and a property test hold that. The loop
is "Capped so a pathological spill chain cannot loop forever"
(`sheet-calc/src/lib.rs:400-401`). No error value exists for a circular reference because
circularity "is a sheet-calc *diagnostic*, never a stored wire value"
(`sheet-core/src/value.rs:55-58`).

No place argues the design as a whole. Why a dynamic-array result is written into the
model as cells, and why the cap is 64, is not stated. The repository does not record why.

## Decision

Recalculation is incremental over a dirty set and ordered by Kahn's algorithm with a
stable frontier; a range reference is one graph node. A dynamic-array result is written
into the model as cells, and the pass repeats until nothing is dirty, at most 64 times.

- **Graph.** A range reference, a defined name that targets a range and a table reference
  each become one `RangeKey`, the normalised box. The dependents of a written cell are its
  direct readers plus the readers of every range key whose box contains it.
- **Order.** `topo::order` counts in-degrees inside the dirty set and drains a
  `BinaryHeap<Reverse<CellRef>>`, so the smallest `CellRef` is evaluated first.
- **Cycles.** Cells that are never emitted form the `cycle` set. With `CalcSettings::iterative`
  off, the default, each stores `#REF!`, and the set found in the first pass of the call is
  reported on `RecalcResult::circular`. With it on, they are seeded at 0 and recomputed in sorted
  order for up to `max_iter` passes (default 100), until the largest change is at most `max_change`
  (default 0.001); if that is not reached, the whole set of the first pass is reported on `non_converged`.
- **Spill.** A formula spills only when its root is an array literal or a call to a
  function whose registry row sets `returns_array`. The anchor keeps the formula; the
  other cells of the rectangle become plain value cells, recorded in `SpillState`. If one
  of them already holds a value or a formula, the anchor stores `#SPILL!` instead. The
  previous region is cleared before every recompute of the anchor.
- **Fixpoint.** Writing or clearing spilled cells marks their dependents dirty, so
  `recalc_dirty` drains the dirty set in a loop bounded by `MAX_PASSES = 64`. Volatile
  cells join the dirty set once per call; their seed is `cell_seed(rng_seed, pass, cell)`.

## Evidence

- `sheet-calc/src/graph.rs:39-47`, `:62-69`, `:236-250` — `RangeKey` nodes; the scan over
  all range keys in `dependents_of`
- `sheet-calc/src/dirty.rs:95-107`, `:121-125` — transitive marking; the volatile reseed
- `sheet-calc/src/topo.rs:86-124` — the stable frontier; cells not emitted are the cycle
- `sheet-calc/src/lib.rs:153-165`, `:389-469` — what `Engine` owns; `recalc_dirty` with
  the cap (`:402`), the spill and scalar paths (`:418-431`), the cycle policies (`:439-455`)
- `sheet-calc/src/lib.rs:663-729`, `sheet-calc/src/spill.rs:43-67` — `materialize_spill`;
  the rules of the ledger
- `sheet-calc/src/lib.rs:536-582`, `sheet-core/src/calc_settings.rs:94-103` — iteration
- `sheet-calc/src/volatile.rs:54-59`, `sheet-conformance/tests/calc_order.rs:161-190` —
  the seed; the order-independence property test

## Alternatives considered

One edge per cell of a range was rejected (`sheet-calc/src/graph.rs:41-42`). An interval
index for range invalidation is named as the upgrade over the linear scan
(`sheet-calc/src/graph.rs:45-46`, `registry/features/calc.yaml:14-21`) and is not built.

## Consequences

Results do not depend on insertion or hash order. The golden corpora rely on that and
compare without a tolerance (`sheet-conformance/src/lib.rs:61-62`). The `Engine` surface
is declared frozen for `sheet-js` (`sheet-calc/src/lib.rs:44`). Limits of the code today:

- Two scans are linear: `dependents_of` visits every range key for each cell it expands,
  and `precedents_in` (`sheet-calc/src/graph.rs:269-278`) every dirty cell per range key.
- When the 64-pass cap is reached, the cells still dirty are dropped
  (`sheet-calc/src/lib.rs:458`); `RecalcResult` has no field that reports it.
- The `cycle` set is every dirty cell that never reaches in-degree zero, so it includes
  dirty cells that depend on a cycle without lying on it.
- Function arguments are evaluated before dispatch, so `IF` and `IFERROR` evaluate the
  branch they discard (`sheet-calc/src/eval.rs:53-64`).
- Recalculation runs on the calling thread: no crate starts a thread, no worker is created.

## Related

- [ADR 500](500-own-calculation-engine.md), [ADR 507](507-golden-corpora-coverage-gate.md)
  — the rule this scheduler was written under; the tests that rely on determinism
- [ADR 317](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/317-registry-driven-dispatch.md)
  — the registry whose `returns_array` and `volatility` columns drive spill and reseeding
