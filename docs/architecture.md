# Architecture

How the `paged.sheet` plugin is built: a spreadsheet engine written in Rust and compiled to
one wasm module, and a TypeScript bundle that connects it to a page-layout editor. This page
describes what the code does at commit `71f37d7`. The reason behind each choice is in an ADR
under [`adr/`](adr/README.md), linked where it applies.

## Two workspaces in one repo

- **Cargo workspace:** eleven crates, each a directory at the repo root. The toolchain is
  pinned to Rust 1.93.0 (`rust-toolchain.toml`).
- **pnpm workspace:** two packages under `packages/`.
- **`registry/`:** YAML tables. `registry/functions/` has 224 rows in 18 files, one row per
  spreadsheet function; `registry/features/` has 162 rows in 24 files, one per behaviour or
  compatibility ruling. The function rows are build input; all rows feed the coverage gate.
- **`corpus/`:** 237 golden `.tsv` files and, in `corpus/xlsx-corpus/`, 15 workbook fixtures.

## The Rust crates

Formula parsing and evaluation, number formatting and XLSX reading and writing are in these
crates; the TypeScript side calls them and translates their results
([ADR 314](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/314-plugin-shape.md)).
The engine is written in this repo and depends on no third-party spreadsheet engine
([ADR 500](adr/500-own-calculation-engine.md)).

| Crate | What it owns | Workspace crates it depends on |
|---|---|---|
| `sheet-core` | The stored types: `CellValue`, `CellRef`/`RangeRef`, `SheetModel`, the formula AST, interners. Its `build.rs` generates the function name table from `registry/functions/`. | none |
| `sheet-format` | Number-format codes (compile and render), `General` display, 1900/1904 date serials, locale tables for en-US, de-DE, fr-FR, es-ES, it-IT. | core |
| `sheet-parser` | Lexer and Pratt parser for the Excel formula dialect, the printer, reference extraction, and the rewrite of references for row and column insert/delete. | core |
| `sheet-fn` | The function kernels, each a pure `fn(&[Arg], &EvalCtx) -> CellValue`, coercion rules, and the dispatch `match` its `build.rs` generates from the registry. | core, format |
| `sheet-calc` | `Engine`: owns the model, the dependency graph, the dirty set, the recalculation scheduler, the evaluator, spill bookkeeping. | core, parser, fn |
| `sheet-lower` | `(sheet, range, options)` to the `LoweredContent` IR: formatted cell text, column widths, row heights, merges, grid rules, a style table. Also pagination across a list of frame boxes, and conditional-format evaluation. | core, format |
| `sheet-grid` | A windowed view of one sheet as the `GridScene` IR: only the cells visible from a scroll origin inside a width and height. | core, format, lower |
| `sheet-chart` | The chart model (ten kinds) and the generator that turns a chart plus resolved data into `ChartGeometry`, a list of five primitive kinds. Layout is done by `plotters` with its default features off. | core, format |
| `sheet-xlsx` | The OPC zip container, the SpreadsheetML parts, the writer. | core, lower, chart, format |
| `sheet-js` | `SheetSession`, a plain-Rust session over the engine and the XLSX document, and the wasm class `SheetEngine` that forwards to it. | all nine above |
| `sheet-conformance` | Tests only: the corpus loader, 47 integration test files, the `coverage-gate` binary. | all ten above |

The function table is generated: `sheet-core/build.rs` and `sheet-fn/build.rs` both read
`registry/functions/*.yaml` and sort by id, so a function without a registry row has no id
and no dispatch arm
([ADR 317](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/317-registry-driven-dispatch.md)).
`.github/workflows/rust.yml` fails if `sheet-fn` reaches `sheet-calc`, `sheet-parser`,
`sheet-xlsx`, `sheet-lower` or `sheet-js`, or if the wasm build of `sheet-js` contains
`sheet-conformance` or `proptest`. `sheet-lower`, `sheet-grid` and `sheet-chart` do not
evaluate formulas; they read the values the engine has already stored in the model.

Decisions inside the engine: numbers are `f64` ([ADR 501](adr/501-f64-numbers.md), with the
measured comparison in [`DECIMAL-SPIKE.md`](../DECIMAL-SPIKE.md) at the repo root);
recalculation order and spilled arrays ([ADR 502](adr/502-recalculation-and-spill.md)); the
formula dialect and locales ([ADR 509](adr/509-excel-first-dialect.md)); charts
([ADR 016](adr/016-chart-engine-plotters-chartgeometry.md)).

## The TypeScript packages

**`packages/sheet-host-model`** — private, no host calls, no React, no wasm. It holds
hand-written TypeScript copies of the three Rust IRs (`lowered.ts`, `grid.ts`, `chart.ts`)
and the pure translators from those IRs to host data: `lower-to-table.ts` (native table
operations), `lower-to-mutations.ts` (the older tab-separated text lane), `chart.ts` (vector
paths and label frames), `grid.ts` (a scene layer, an SVG string, and cell hit-testing),
`binding.ts` (the frame's metadata envelope), `palette.ts` (swatch ids), `cell-style.ts`,
`completions.ts` and `placement.ts`.

**`packages/sheet-bundle`** — published to npm as `@paged-media/sheet` (`dist`, `bin` with
the wasm, and `manifest.json`). It holds `activate(host)` (`src/activate.ts`), the workbook
session (`src/session.ts`), three React panels (`src/panels/`), the engine facade and loader
(`src/engine.ts`), the page lowering (`src/lower.ts`, `src/lower-chart.ts`), the container
part (`src/workbook-part.ts`) and two binding providers (`src/binding-provider/`). It is the
only package that touches the host.

The bundle imports the model by relative source path (`../../sheet-host-model/src`) and
`tsup` inlines it, so the published package does not depend on the private one.
`@paged-media/plugin-api`, `@paged-media/plugin-sdk` and `react` are peer dependencies, and
`scripts/check-contract-imports.mjs` fails on a package import in `packages/*/src` that is none of
these and not one of this repo's own `@paged-media/sheet-*` packages ([ADR 315](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/315-isolation-contract.md)).

## The wasm boundary

`sheet-js` builds as a `cdylib` and an `rlib`. `SheetEngine` is its one `#[wasm_bindgen]`
class; it is compiled only for `wasm32`, and every method forwards to `SheetSession` in
`sheet-js/src/core.rs`. Because the session is plain Rust, `sheet-conformance` tests the
load, recalculate, edit, save and lower loop natively. Bytes cross as byte slices, cell
addresses as integers, ranges as A1 strings, and structured results as serde values through
`serde-wasm-bindgen` with camelCase field names.

`packages/sheet-bundle/src/engine.ts` wraps the snake_case methods in a camelCase facade
and loads the module itself: it imports the `wasm-bindgen --target web` glue
`bin/sheet_js.js`, then in a browser passes the URL of `bin/sheet_js_bg.wasm` (a `?url`
import that `tsup` leaves to the consuming bundler) to the init function, and in Node reads
the file and calls `initSync`. No types are generated across the boundary: the TypeScript
interfaces are kept in step with the Rust structs by hand, and
`packages/sheet-bundle/test/engine-real.spec.ts` runs against the built wasm.

## Data paths

```
.xlsx bytes --(importer, file picker, panel input)--> session.import
                                                        |  engine.loadXlsx
                                                        v
   sheet-xlsx: open parts --> SheetModel --> sheet-calc: Engine (recalc_all)
                                                        |
        +-------------------+---------------------------+----------------------+
        v                   v                           v                      v
  get_range_lowered    get_grid_scene            get_chart_geometry        save_xlsx
  (LoweredContent)     (GridScene)               (ChartGeometry)           (bytes)
        |                   |                           |                      |
  lower-to-table.ts    grid.ts                     chart.ts               exporter
        |                   |                           |
  document.mutate      sceneLayer.submit           document.mutate
  (text frame +        (in-frame grid) or          (paths + label frames)
   native table)       SVG in the Grid panel
```

**Load.** `SheetSession::load_xlsx` opens the package (a legacy binary `.xls` is refused),
parses each formula text with `sheet-parser`, builds the `Engine` and runs `recalc_all`. A
formula that does not parse (for example one that calls an unregistered function) keeps its
raw text and the value cached in the file, and is counted.

**Edit.** `set_cell` goes to `Engine::enter`, which decides between a formula (leading `=`)
and a literal, commits the cell and recalculates the dirty part of the graph. It returns
the cells whose display changed and the cells on a circular reference.

**Place a range on the page.** `lowerSelectionToFrame` (`src/lower.ts`) asks the engine for
the `LoweredContent` of the range, then writes in three steps: one batch that inserts a text
frame and attaches the binding as plugin metadata; `insertTable` into the new frame's story,
with column widths measured through `host.text.measureString`; then one `insertText` per
non-empty cell, followed by one batch with the merges (`setCellSpan`) and the grid rules as
0.5 pt cell edge strokes. The new frame's story is found by comparing the document's story
list before and after the insert. If the host rejects `insertTable`, the cell text is poured
into the frame as tab-separated text instead. The result is ordinary document content
([ADR 505](adr/505-native-table-and-edit-grid.md)).

**Edit in place.** The frame is recognised as the object type `sheetFrame` by its metadata.
A double-click enters the `sheet` edit context; the session asks the engine for a
`GridScene` sized to the frame, translates it with `gridSceneToSceneLayer` and submits it
to the frame through the host's scene-layer surface. Pointer events arrive in frame-content
coordinates and are hit-tested against the last scene; printable keys fill a text buffer
that is drawn by submitting the layer again; Enter writes the buffer with `set_cell`. While
the context is active, undo and redo run on a journal of cell inputs kept in the session.

**Charts and colours.** Chart parts of the workbook are parsed on load, and a chart can be
authored over a range in the Workbook panel. `lowerChartToFrame` (`src/lower-chart.ts`)
asks for the `ChartGeometry` at 360 x 240 pt, writes one batch of `insertPath` and
`insertTextFrame` operations, then pours each label's text. A colour reference in the
document names a swatch, so the plugin creates swatches at ids derived from the colour
(`Color/uPagedSheetChart<HEX>` and three more prefixes in `packages/sheet-host-model/src/palette.ts`).
Every path that creates swatches inside a batch first reads the document's swatch list
(`src/swatch-mints.ts`) and creates only the missing ones
([ADR 508](adr/508-content-addressed-swatches.md)).

**Source a sheet from a dataset.** The Datasets panel lists the providers in the `dataset`
category from `host.dataProviders.discover`. `sourceFromDataset` pulls one snapshot, starts
an empty workbook, and writes the field names into row 0 and the records below, each value
as a string through `set_cell`. It records the provider id and revision and marks the sheet
stale when the provider announces a newer revision. See
[`design/data-provider-consumer.md`](design/data-provider-consumer.md).

**Save as XLSX.** `save_xlsx` prints the formulas of edited cells back to text, marks their
sheets dirty and writes the package. Untouched parts are written back with their original
bytes; only sheets with an edited cell are encoded again from the model; `calcChain.xml` is
always dropped. See [ADR 503](adr/503-xlsx-patched-not-regenerated.md) and, for what is
preserved but never interpreted, [ADR 504](adr/504-publishing-first-scope.md).

## Where data is stored

- **The workbook.** On import the original XLSX bytes are written to the document's container
  as the parts `workbook.xlsx` and `workbook.name` in this plugin's namespace (`host.parts`,
  when the host supports it), and to the per-browser blob store under the key `workbook`,
  with the name in `host.storage`. On activation `session.restore()` reads the part first
  and falls back to the blob. One workbook is stored: the last one imported
  ([ADR 506](adr/506-workbook-in-a-container-part.md)).
- **The frame binding.** `{ v: 1, data: { sheet, range, contentVersion } }` as plugin
  metadata on the frame, under the key `x-paged:media.paged.sheet`. A lowered chart carries
  the same envelope with the range `chart:<index>`.
- **Everything else is session state** and is gone when the bundle is disposed: the engine
  and its model, the edit journal, the grid selection, the scene layer.

## Host doors

| Door | What the plugin uses it for |
|---|---|
| `contributePanel`, `host.contribute.command`, `host.contribute.menu` | three panels (Workbook, Grid, Datasets), twelve commands, twelve menu entries |
| `host.contribute.objectType`, `host.contribute.editContext` | the `sheetFrame` type and the `sheet` context, which declares no canvas tools and one panel |
| `host.contribute.importer`, `host.contribute.exporter` | `.xlsx` in and out of the session |
| `host.contribute.sceneLayer()` | the in-frame grid |
| `host.contribute.bindingProvider` | the host's Swatches, Character and Paragraph panels read workbook values while the `sheet` context is active |
| `host.document.mutate` | every write: frames, tables, cell text, paths, swatches, cell styles, metadata |
| `host.document.collection`, `meta`, `elementGeometry`, `elementProperties` | stories, swatches and pages; the active page; frame size; a table cell's properties |
| `host.text.measureString` | column widths of a lowered table |
| `host.selection.set` | select the frame after lowering |
| `host.parts`, `host.blob`, `host.storage` | the stored workbook and its name |
| `host.dataProviders` | discover datasets, pull a snapshot, watch its revision |
| `host.clipboard` | copy and paste cell ranges; copy a function name |
| `host.shell.pickFile`, `host.shell.openPanel`, `host.log`, `host.supports` | file picker, opening panels, logging, probing optional doors |
| `host.document.frameChain`, `hitTest`, `onDidChange` | only the paginated chain lowering in `src/lower.ts`, which the bundle exports but does not call (see [`status.md`](status.md)) |

The manifest declares `document` (read `broad`, write `scoped`), `rendering` (`hitTest`,
`sceneLayer`), `network: false`, `dataProviders` (consume `dataset`), `clipboard` (`full`),
`storage` (blob, 32 MiB quota) and one wasm module.

## Build and test

- `bash scripts/build-wasm.sh` builds `sheet-js` for `wasm32-unknown-unknown` in release
  mode, checks that the installed `wasm-bindgen` CLI has the version in `Cargo.lock`, runs
  `wasm-bindgen --target web` into `packages/sheet-bundle/bin/` (gitignored) and, if
  installed, `wasm-opt -Oz`. It fails above 100 MB; the manifest declares 8 MiB for the same
  file. `.github/workflows/publish.yml` runs it, then `pnpm -r build` (`tsup`), and
  publishes `@paged-media/sheet` under the `canary` tag.
- Rust lane (`.github/workflows/rust.yml`, on pull requests and on `main`): `cargo fmt`,
  `clippy` with warnings denied, the dependency guards, `cargo deny`, `cargo nextest`, then
  `coverage-gate`, which fails when a registry row marked `implemented` names no test or
  one that is not on disk ([ADR 507](adr/507-golden-corpora-coverage-gate.md)). A third job
  builds the wasm.
- TypeScript lane: `pnpm test` runs the import lint, then vitest in both packages (29 spec
  files; the bundle specs use hand-built fake hosts). `.github/workflows/vitest.yml` runs on
  pushes to `main` and on manual dispatch, not on pull requests. It builds the wasm first and sets
  `REQUIRE_REAL_ENGINE=1`, so the real-engine spec fails instead of skipping when the
  artefact is missing. The vitest step itself ends in `|| true`, so a failing spec does not
  fail that job.
