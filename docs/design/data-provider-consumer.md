# RFC: data-provider consumer — source a sheet from a governed dataset (S-15)

*Design note, written in June 2026 as a proposal, before the implementation; it was then built. What was built is recorded in [ADR 014](https://github.com/paged-media/plugin-data/blob/main/docs/adr/014-data-provider-arrow-seam.md) (in plugin-data); where this plugin's consumer differs from the proposal, a status note says so.*

**Origin:** the plugin-data concept §7.1 (`plugin-data: docs/concept.md`), which names the
sheets plugin as the canonical consumer, so that a sheet can be sourced from a governed
query rather than from a static import; and this plugin's concept
([concept.md](../concept.md)) §1.1 (publishing-first / no-network) and §2.1
(isolation superset). Bare section numbers below refer to this plugin's concept.

**Counterpart:** `plugin-sdk: docs/design/data-provider.md`
— the **provider** + shared-contract half. Both plugins build to the **same**
neutral `host.dataProviders` SDK surface; neither builds to the other. The
provider side is already implemented + tested in plugin-data
(`publish_provider`); this is the missing consumer half.

## Problem

*Status note (2026-10-02): this section predates the implementation; the door asked for below exists now, as `host.dataProviders`, announced by `host.supports("dataProviders@1")`.*

A `paged.sheet` workbook is sourced today from an **`.xlsx` import** or
hand entry. plugin-data §7.1 calls for a more powerful composition: a sheet
sourced from a **governed query** that `paged.data` resolves — "a spreadsheet
computing over a governed, live query result and then lowering to print." The
author binds a sheet to `fct_products`, edits formulas over it, and the print
output is a projection of governed data.

Two constraints shape how this is allowed to work:

1. **§2.1 isolation superset — zero inter-plugin contact.** `paged.sheet` may
   not import `paged.data`, may not discover it by identity, may not
   message-pass with it, even co-installed. The two rendezvous **only at a
   neutral core contract** (`host.dataProviders`). If `paged.data` is absent, the
   feature simply offers no sources.

2. **§1.1 no-network / publishing-first — and the external-link exclusion.**
   `paged.sheet` deliberately **never follows external references**: xlsx
   external-workbook links are *preserved on round-trip, never interpreted*
   (§1.1, a permanent product decision). This RFC does **not** breach that. A
   data-sourced sheet does not follow a link or open a socket — it **receives an
   already-resolved `RecordSet` snapshot** handed to it by the platform, exactly
   as the `.xlsx` importer receives bytes. **`paged.data` owns the network and the
   consent (§11 of its concept);** the sheet only ever consumes cached, resolved data through the
   SDK. (Distinct model from external links: those are live xlsx-link semantics
   excluded by design; this is a governed snapshot the platform delivers.)

The platform offers no consumer surface for this — there is no
`host.dataProviders`. That door is the shared ask (the counterpart RFC); this
document specifies what `paged.sheet` builds on top of it.

## Proposal (three stages)

### Stage 1 — a `dataProviders` capability (manifest)

```jsonc
"dataProviders": { "consume": ["dataset"] }
```

`paged.sheet` declares it may **discover and read** providers in the `dataset`
category. It declares no `publish` role. A bundle that declares neither role
gets no `host.dataProviders` surface (closed-vocabulary capability → an
`apiVersion` minor bump, as in the counterpart RFC).

### Stage 2 — consume `host.dataProviders` (the shared surface)

The consumer touches only the read side of the shared surface (defined in full
in the counterpart RFC):

```ts
interface DataProvidersSurface {
  // ── Consumer side (paged.sheet uses these three) ───────────────────────────
  discover(category?: string): readonly DataProviderInfo[];   // schema + revision, NO rows
  get(id: string): Promise<DataProviderSnapshot | null>;      // pull the rows
  onDidChange(id: string, listener: (revision: string) => void): Disposable;
  // register(...) is the provider side — paged.sheet never calls it.
}
interface DataProviderInfo     { id: string; category: string; schema: ProviderSchema; revision: string }
interface DataProviderSnapshot { id: string; revision: string; records: RecordSet }
```

`ProviderSchema` / `RecordSet` are the Arrow-aligned interchange substrate
(`{ fields: { name, type }[] }` + columnar rows) — the field shape the
contract-owning provider RFC (D-09) defines, and the same shape `paged.sheet`
already handles internally for the grid, so the mapping is mechanical.

### Stage 3 — `paged.sheet` integration

*Status note (2026-10-02): this section predates the implementation, and what was built differs. Sourcing a dataset starts a fresh workbook that replaces the open one: row 0 holds the schema field names, and every value is written as text and typed by the engine. The session keeps the provider id, the revision and a stale flag; a newer revision marks the sheet stale and the Datasets panel offers "Re-source". A linked range inside an existing workbook, pin and unlink are not built, and the provider link is not stored with the workbook. See `packages/sheet-bundle/src/session.ts` (`sourceFromDataset`) and `packages/sheet-bundle/src/panels/datasets-panel.tsx`.*

**A "New sheet from dataset" command + panel.**
`discover("dataset")` → present the providers (id + the schema's field labels) →
on pick, `get(id)` → seed a sheet from the snapshot.

**The `RecordSet` → cell-model mapping (the sheet-specific work).** One provider
snapshot → one sheet (or a named range):
- a **header row** from the schema field labels/names;
- one column per `RecordSet` column, each `FieldType` mapping to a native sheet
  cell value: `text → text`, `int`/`float → number`, `bool → boolean`,
  `date`/`datetime → date serial`, `null → blank`. (This is the inverse of the
  shape the grid already renders; reuse the existing value bridge.)

**A "data-sourced range" lifecycle (non-destructive, mirroring plugin-data
sync).** The seeded range is a **linked** region: the bundle remembers
`(providerId, lastRevision, anchor)` in the workbook payload. Formulas and edits
the author writes **around** the range are untouched; only the linked cells
track the source. `onDidChange(id, rev)` → mark the range *stale* and offer a
one-click refresh (`get(id)` → re-seed the linked cells). The author can **pin**
a range (freeze the snapshot) or **unlink** it (convert to plain values) — the
same Linked / Pinned / Overridden posture plugin-data uses (§8 there).

**Persistence honesty (§1.1-consistent).** The linked range stores
`(providerId, revision)` in the workbook payload, but the **cell values are
committed content** — the snapshot travels *with* the document. Reopening a
workbook shows the saved values and **does not auto-refetch**; a refresh is an
explicit author action. (No silent network on open — the sheet never fetches at
all; it asks the SDK, which asks the governed provider.)

## Security notes (consumer side — symmetric to the provider RFC)

- **Pull-only; no control.** The consumer API (`discover`/`get`/`onDidChange`)
  has **no parameter** by which `paged.sheet` can hand the provider a query, a
  source, a parameter, or an origin. The sheet reads published snapshots; it
  **cannot drive `paged.data`'s network/file reach**. A malicious or buggy sheet
  cannot induce a fetch `paged.data` is not consented to perform.
- **No auto-refetch on open.** A reopened workbook renders its committed
  snapshot; re-pulling is explicit. This keeps the §1.1 no-network thesis intact
  (the sheet never fetches) and composes with plugin-data's rule that documents carrying
  queries are inert until consented (§11 there).
- **No identity leak.** `paged.sheet` learns a provider's `id`/`category`/
  `schema` — never that `paged.data` (or any specific plugin) backs it.

## Graceful absence (§2.1 intact)

If `paged.data` is not installed, `discover("dataset")` is empty; "New sheet from
dataset" shows no sources and the command is a no-op with an honest "no datasets
available" message. `paged.sheet` keeps `.xlsx` + hand entry as its sourcing
paths. Neither plugin hard-depends on the other; both depend only on the SDK.

## Why not the alternatives

- **Import an exported dataset as `.xlsx`** (already supported) — static,
  stale, manual re-export on every data change. The whole point is a **live,
  governed** source with a refresh signal.
- **A shared `@paged-media/*` package, or calling `paged.data` directly** —
  violates §2.1 (build-time / runtime coupling) and couples release cycles.
- **Model it as an external-workbook link** — wrong model and a §1.1 breach:
  external links are live xlsx-link semantics deliberately *excluded* (preserved,
  never interpreted). A data-sourced range is a platform-delivered governed
  snapshot, not a followed link.

## Milestone posture + what's needed

- **Gated on the SDK `host.dataProviders` door** — the shared ask in the
  counterpart RFC (plugin-data D-09). `paged.sheet`'s consumer side is then: the
  `consume` capability + a `discover`/`get`/`onDidChange` flow + the
  `RecordSet → cells` seeding + the linked-range lifecycle. No engine (Rust)
  change is required for v1 — the mapping is bundle-side glue over the existing
  value bridge; a Rust helper is only needed if the linked-range diff is pushed
  into `sheet-core` (optional, mirrors plugin-data's record-identity diff).
- **Recommend landing alongside plugin-data's provider registration** so the
  composition ships end-to-end in one cut — `paged.data` publishes `fct_products`,
  `paged.sheet` sources a sheet from it, both through the neutral contract.
