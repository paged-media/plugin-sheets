# ADR 016 — The chart engine: plotters + a custom `DrawingBackend` → frozen `ChartGeometry` IR

**2026-06-12 (records a 2026-06-08 decision) · decision record · status:
ACCEPTED (records paged.sheet's chart-layout choice; NOT charming, NOT
hand-rolled).**

**Sources:** an internal design note (the decision, the rejected
alternatives, the IR shape, the `packages/sheet-host-model/src/chart.ts` translator);
[`../concept.md`](../concept.md) (§8.4 charts); the plugin-sheets internals (`sheet_chart::generate`
drives plotters; the frozen `ChartGeometry` IR of Rect/Line/Polygon/Wedge/Text);
[ADR 013](https://github.com/paged-media/core/blob/main/docs/adr/013-in-frame-scenelayer.md) (in-frame `SceneLayer` — the one rendering surface charts lower into).

## The decision

**Chart-layout math (axes, scaling, pie angles, legends) comes from the pure-Rust
`plotters` crate via a *custom* `DrawingBackend` that emits the frozen
`ChartGeometry` IR (Rect / Line / Polygon / Wedge / Text). The existing
`chart.ts` translator turns that IR into native `paged.draw` wire ops
(`insertPath` / `insertOval` / `insertLine` / `insertText`).** Charts are never
an image, never an SVG, never an in-browser charting widget — they decompose to
the same document primitives everything else renders from.

## Why plotters-as-layout-only, and why not the alternatives

Three live alternatives were weighed:

- **Not `charming` (the ECharts wrapper) — structurally impossible here.** Its
  only real SVG output is the `ssr`/`ImageRenderer` path via `deno_core` (V8),
  which **cannot compile to `wasm32-unknown-unknown`** and would blow the 8 MiB
  bundle budget; in-wasm it can only emit an ECharts *option spec* that still
  needs ECharts-in-a-browser to render — a **forbidden second rendering surface**
  (the same doctrine that rejects a DOM overlay in
  [ADR 013](https://github.com/paged-media/core/blob/main/docs/adr/013-in-frame-scenelayer.md)). Charming is
  acceptable only as a clean-room ANALYST reference (the analyst/implementer
  protocol, [`../concept.md`](../concept.md) §3), never as a runtime
  dependency.
- **SVG is not a shortcut.** The wire has no "insert SVG" op and the page renders
  from the document model via Vello, so any SVG would have to be decomposed into
  path/line/oval/text primitives *anyway* — producing the primitives directly is
  simpler **and** lets each fill reference a document swatch (live restyle), which
  baked-SVG colours cannot.
- **Not hand-rolled.** `plotters` has mature, correct layout math (axis ticks,
  scaling, pie geometry, legend placement); reinventing it would be a large
  surface of subtle bugs for no gain. The decision keeps plotters for the *math*
  and rejects it for any *pixels* — its drawing calls are intercepted by the
  custom backend and turned into geometry, never rasterized.

## Consequences

- **`ChartGeometry` is frozen and is the seam.** Only `sheet_chart::generate()`
  drives plotters; everything downstream consumes the IR. Freezing it keeps the
  plotters dependency from leaking into the translator or the wire.
- **Charts inherit live restyle and print fidelity** because they are document
  primitives: a chart fill referencing a document swatch restyles with the
  document ([ADR 008](https://github.com/paged-media/core/blob/main/docs/adr/008-read-surfaces-first-class-wire-collections.md)'s
  "document styles are the single source of truth" spirit),
  and the chart exports at full fidelity through the same Vello lane as native
  content — including, in a frame, via the `SceneLayer` path
  ([ADR 013](https://github.com/paged-media/core/blob/main/docs/adr/013-in-frame-scenelayer.md)).
- **The rejected-alternatives knowledge now lives in the ADR**, not only in
  unpublished notes — which is the failure mode this record closes (a future contributor
  reaching for `charming` or "just emit SVG" finds the recorded reason it does not
  work here before spending the week).

## Amendment — 2026-10-02

Checked against the code at `71f37d7`. The decision stands. `sheet-chart` is the only crate
whose manifest names `plotters`, and the workspace turns its default features off
(`Cargo.toml:44-57`, `sheet-chart/Cargo.toml:23-24`); `GeomBackend` implements `DrawingBackend`
(`sheet-chart/src/backend.rs:146`); the IR is `ChartGeometry` over five primitive kinds
(`sheet-chart/src/geometry.rs:122-170`, `:178-182`). Points 1, 3 and 4 below supersede text
above. Point 2 records growth that the text allowed for; point 5 adds a second limit.

**1. The translator emits `insertPath` and `insertTextFrame`. No chart code emits
`insertOval` or `insertLine`.**

- `packages/sheet-host-model/src/chart.ts:446-510` — rect, line, polygon and wedge primitives
  each become one `insertPath` (`:449`, `:454`, `:461`, `:471`); a text primitive becomes one
  `insertTextFrame` (`:494-497`).
- `packages/sheet-host-model/src/chart.ts:205`, `:224-235` — a wedge is flattened to a ring of
  straight segments at 6° steps before it becomes a path.
- `packages/sheet-bundle/src/lower-chart.ts:133`, `:152-169` — the bundle applies that batch,
  then pours each label with its own `insertText` into the stories the batch created.
  `insertText` is issued by this driver, not by the translator.
- The batch also carries one `createSwatch` per distinct colour the document lacks
  (`packages/sheet-host-model/src/chart.ts:420`,
  `packages/sheet-host-model/src/palette.ts:349-363`), `setElementProperty` ops for fill,
  stroke and stroke weight (`packages/sheet-host-model/src/chart.ts:333-372`) and one
  `setPluginMetadata` that writes the binding (`:431-444`). The swatch ids are recorded in
  [ADR 508](508-content-addressed-swatches.md).

This supersedes, in The decision, "(`insertPath` / `insertOval` / `insertLine` /
`insertText`)". Neither `insertOval` nor `insertLine` has appeared in the translator at any
commit since its first (`ca1c59f`, 2026-06-08). In this repository `insertLine` is emitted
only by the tab-text fallback lowering of a cell range
(`packages/sheet-host-model/src/lower-to-mutations.ts:400`, `:410`), and `insertOval` is
emitted nowhere. Three places still name the two ops for charts and are stale:
`sheet-chart/src/lib.rs:41`, `sheet-chart/src/geometry.rs:37` and the title at
`registry/features/chart.yaml:240` (the ruling of the same row, `:241`, says the translator
"emits insertPath + insertTextFrame native wire ops").

**2. There are ten chart kinds on the same IR.** The text above names no number, so no
sentence is superseded.

- `sheet-chart/src/model.rs:59-76` — `ChartKind` has ten variants: Bar, Column, Line, Area,
  Pie, Donut, Scatter, StackedColumn, StackedBar, Radar. The last three were added on
  2026-08-05 (commit `f2fb100`); before that there were seven.
- `sheet-chart/src/geometry.rs:245-262` — `generate` dispatches all ten.
- `sheet-chart/src/model.rs:52-57` — the comment states that a new kind is expressed in the
  existing primitives. The definitions of `Primitive` and `ChartGeometry` are unchanged since
  the commit that introduced plotters (`d2a2755`, 2026-06-08).
- `sheet-chart/src/geometry.rs:544-551` — one cost of the frozen IR: `Primitive::Text` has no
  rotation, so the value-axis title is set horizontally above its axis.

The crate documentation at `sheet-chart/src/lib.rs:48-53` still lists seven kinds and is
stale.

**3. plotters supplies the cartesian coordinate system. Pie, donut, radar and legend layout
are the crate's own arithmetic.**

- `sheet-chart/src/geometry.rs:701`, `:799`, `:888`, `:984` — the column, bar, line/area and
  scatter generators build a plotters `ChartBuilder` with `build_cartesian_2d` and read
  positions back through `map_coordinate` (for example `:720`, `:745-750`).
- `sheet-chart/src/geometry.rs:367-413` — the value range handed to plotters is computed by
  the crate's `ValueScale`. `:1224-1236` — tick labels are `VAL_TICKS` (5, `:218`) evenly
  spaced values, placed through `map_coordinate`.
- `sheet-chart/src/geometry.rs:1026-1031`, `:1057-1074` — pie and donut: the "wedge geometry
  is emitted DIRECTLY"; each angle is the value's share of the total times 360.
- `sheet-chart/src/geometry.rs:1105-1110` — radar: "the polar layout is emitted DIRECTLY".
- `sheet-chart/src/geometry.rs:590-617`, `:621-648`, `:229-232` — legend rows are placed from
  the plot area and fixed constants, then drawn as plotters `Rectangle` and `Text` elements so
  that they pass through the backend.

This supersedes "pie angles" and "legends" in the first sentence of The decision, and, in the
"Not hand-rolled" bullet, the list "(axis ticks, scaling, pie geometry, legend placement)" as
a description of what is taken from plotters: of that list the code takes the mapping from
values and categories to points. Pie was already emitted directly in the commit that
introduced plotters (`d2a2755`). "Only `sheet_chart::generate()` drives plotters" still holds.

**4. No chart is drawn through a `SceneLayer`.** A lowered chart is a set of ordinary page
items (paths and text frames).

- `packages/sheet-bundle/src/lower-chart.ts:67-173` — the only chart lowering. It is reached
  from the command `media.paged.sheet.command.lowerChartToFrame`
  (`packages/sheet-bundle/src/activate.ts:154-166`) and from the workbook panel
  (`packages/sheet-bundle/src/panels/workbook-panel.tsx:730`), and it writes through
  `host.document.mutate` (`packages/sheet-bundle/src/lower-chart.ts:133`, `:162-165`).
- `packages/sheet-bundle/src/session.ts:638` — the only scene-layer submission in the
  repository. It submits `gridSceneToSceneLayer(scene)`, the cell grid
  (`packages/sheet-host-model/src/grid.ts:575`). Neither `grid.ts` nor the `sheet-grid` crate
  reads chart geometry.

This supersedes, in Sources, "the one rendering surface charts lower into" and, in
Consequences, "including, in a frame, via the `SceneLayer` path".

**5. Two limits apply to the wasm artifact.** The application-wide budget is 100 MB; the
manifest declares a tighter 8 MiB for this artifact.

- `scripts/build-wasm.sh:15-20` — "The budget is now 100 MB for the WHOLE APP including every
  plugin"; the script fails when this one artifact is above that size (`:44-47`). See
  [ADR 308](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/308-plugin-wasm.md).
- `packages/sheet-bundle/manifest.json:27` — `"maxBytes": 8388608`;
  `packages/sheet-bundle/test/manifest.spec.ts:31`, `:98` assert it, and the manifest
  validator applies the smaller of `maxBytes` and its own ceiling
  (`plugin-sdk: packages/plugin-cli/bin/paged-plugin.mjs:346-360`).

"the 8 MiB bundle budget" in the `charming` bullet therefore still names this plugin's
declared ceiling; what changed is that the budget governing the application is 100 MB. The
bullet's first reason, that the dependency cannot be built for `wasm32-unknown-unknown`, does
not depend on either number.
