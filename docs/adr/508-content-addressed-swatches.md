# ADR 508 — Workbook colours become swatches at content-addressed ids

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `packages/sheet-host-model/src/palette.ts` and its callers (`chart.ts`, `lower-to-table.ts`, `lower-to-mutations.ts`); `packages/sheet-bundle` (`swatch-mints.ts`, `lower.ts`, `lower-chart.ts`, `binding-provider/swatches-provider.ts`)

## Context

A colour in the engine's output is a hex string. A colour reference in the document is a swatch id.
`packages/sheet-host-model/src/lower-to-table.ts:233-243` records what happens when the two
are confused: a cell fill that passed the raw hex as its reference was rendered through the
engine and painted nothing. The chart and data-bar lowering already created a swatch per
colour (`packages/sheet-host-model/src/palette.ts:24-28`).

Two things followed. The lowering lanes each carried their own copy of the id scheme,
and the provider that serves the host's Swatches panel has to offer exactly the ids the
lowering creates (`packages/sheet-host-model/src/palette.ts:34-40`). And because the ids
are derived from the colour, a second lowering asks for ids the document already has.
The engine refuses a `createSwatch` for an existing id, and a refused operation rolls
back the whole batch it is in. `packages/sheet-bundle/src/swatch-mints.ts:26-29` gives
the measurement: lowering a chart twice, or lowering any second chart, left the second
one absent, because every chart shares the axis grey.

## Decision

The plugin creates one document swatch per distinct colour and facet, at the id
`Color/uPagedSheet<Facet><HEX>`. Every lane that creates swatches inside a batch first reads
the document's swatches and emits `createSwatch` only for ids that are absent; if the read
fails it creates none.

- The facets are `Chart`, `DataBar`, `CellFill` and `CellText`. `<HEX>` is the upper-case
  six-digit form; `#RGB` is expanded, and a malformed colour yields no swatch. The swatch is
  an RGB process colour with channels 0 to 255, named `paged.sheet <facet label> <HEX>`.
- The facet is part of the id so that recolouring a chart colour does not move a data bar of
  the same colour (`packages/sheet-host-model/src/palette.ts:55-58`).
- `palette.ts` is the only implementation of the id, the name and the list of mints.
  `swatchMintOps(facet, hexes, known)` takes what the caller knows: a set of ids (skip those
  present), `null` (the read failed: emit nothing), or nothing (emit all).
- `readKnownSwatchIds` is the one host read the lowering lanes share, `host.document.collection("swatches")`;
  it returns `null` when the read throws. Mints precede the operations that reference them.
- While the `sheet` edit context is active, a binding provider serves the same palette to the
  host's Swatches panel. It handles `editSwatch` only: if the document does not yet have the
  id it sends `createSwatch` at that id, otherwise `editSwatch`.

## Evidence

- `packages/sheet-host-model/src/palette.ts:34-40`, `:55-59`, `:106-125` — the convention as a contract, the facet rationale, the four prefixes and `paletteSwatchId`
- `packages/sheet-host-model/src/palette.ts:84-92`, `:376-384` — hex normalisation; the RGB process spec
- `packages/sheet-host-model/src/palette.ts:299`, `:349-363` — `KnownSwatchIds` and `swatchMintOps`
- `packages/sheet-bundle/src/swatch-mints.ts:19-40`, `:50-60` — the rule and the read
- `packages/sheet-bundle/src/lower-chart.ts:115-120`; `packages/sheet-bundle/src/lower.ts:226-231`, `:435-439`; `packages/sheet-host-model/src/chart.ts:415-420` — the three drivers pass the read through; in a chart the mints lead the batch
- `core: crates/paged-mutate/src/apply/layer.rs:229-238`, `:2274-2298` — a duplicate id is refused; a failed child rolls back its batch
- `registry/features/lower.yaml:72-109`; `CLAUDE.md:123-140` — the ruling with its measurements; the rule as a repo rule
- `packages/sheet-bundle/src/binding-provider/swatches-provider.ts:184-190`, `:251-265` — what the provider declares; create on first edit

## Alternatives considered

A raw hex value as the colour reference: it paints nothing for a cell fill and the default
colour for text (commit `c321642`, 2026-08-05). Creating swatches without reading first: the
duplicate fails the batch (commit `aa7f9e3`, 2026-08-05). A separate copy of the id scheme
per lane: merged into `palette.ts` when the Swatches provider was added (commit `25a6b05`).

## Consequences

One colour in one facet is one swatch across primitives, across charts and across repeated
lowerings. The ids and names are written into the document and stay in saved files.

The chart lowering always costs one collection read; the page lowering reads only when it has
something to create. When the read fails, the geometry still lands and is left unpainted:
`registry/features/lower.yaml:87-90` records that an unresolved colour reference is a paint
miss and not an operation error. `packages/sheet-host-model/src/palette.ts:342-345` states
the rule as general for any resource the engine keys by id.

The id records the colour at creation. After an `editSwatch` through the panel the swatch
holds a new colour under an id that still spells the old hex; a later lowering finds the id
present, creates nothing and references the edited swatch.

Not every facet is reachable at this commit:

- `Chart` is live, through the `Lower chart to frame` command.
- `CellFill` has its mint and its reference, but the page lowering receives the default style only, so it has
  nothing to create (`packages/sheet-bundle/src/lower.ts:217-218`; [ADR 505](505-native-table-and-edit-grid.md)).
- `DataBar` is created only in the tab-text lane, for a region with data bars, which
  `getRangeLowered` never returns (`packages/sheet-bundle/src/lower.ts:425-428`).
- `CellText` is built by `cellTextSwatchOps`, which has no caller (`packages/sheet-host-model/src/palette.ts:263-269`).

## Related

- [ADR 023](https://github.com/paged-media/editor/blob/main/docs/adr/023-shared-panels-binding-providers.md) — the host panel that the provider serves
- [ADR 016](016-chart-engine-plotters-chartgeometry.md), [ADR 505](505-native-table-and-edit-grid.md) — the chart lowering, whose facet is the live one; the page lowering
- [ADR 310](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/310-one-write-door.md) — batches and failures as outcomes on the write door
