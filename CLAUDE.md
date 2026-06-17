# CLAUDE.md — paged-media/plugin-sheets

Orientation for Claude sessions in **paged-media/plugin-sheets** — the
paged.sheet spreadsheet subsystem, delivered as a Paged plugin (private
repo, And The Next GmbH).

## What this is

A Rust/WASM calculation engine + sheet document model: live spreadsheets
inside a print-grade layout document — a **publishing instrument, not an
Excel replacement** (every scope decision follows from that). The page
surface is COMPILED to native Paged content (a real `<Table>` via
insertTable + cell pour + spans + cell strokes/fills — S-03 resolved; the
tab-text + drawn-rules degradation is retained as the explicit fallback
lane); the sheets-mode grid (T1+) renders vector on an SDK surface
(S-02). XLSX round-trip safety ("Paged never destroys a workbook") is a
launch property.

Spec (the authority): `thoughts/docs/paged/plugin-sheets/base-idea.md`.
SDK gap tracker: the cross-repo RFI `thoughts/docs/paged/plugin-platform/rfi-core-sdk-gaps.md` (S-NN ids in §6; per-plugin BREAKAGE_LOG retired 2026-06-12).

**STATUS: M0–M3 shipped** (11 Rust crates, ~1188 nextest; the registry
rows below are the live ledger). Rust crates (Cargo workspace, top level
per spec §4): `sheet-core` (frozen types + AST), `sheet-parser`,
`sheet-calc`, `sheet-fn`, `sheet-format`, `sheet-xlsx`, `sheet-lower`,
`sheet-grid` (windowed GridScene — the in-frame surface), `sheet-chart`
(plotters + a custom paged.draw `DrawingBackend` → a frozen ChartGeometry
IR — NOT charming, NOT hand-rolled SVG), `sheet-js` (wasm-bindgen
surface), `sheet-conformance` (TEST-ONLY). TS packages (pnpm `packages/*`,
draw/web convention): `sheet-host-model` (pure LoweredContent→Mutation
translation) + `sheet-bundle` (manifest + `activate(host)` + workbook
panel + grid panel + engine boot).

What landed (verified — see `state/registry/features/plugin-sheets.yaml`):
the calc engine, function library, spill, structured tables, the chart
engine, and **XLSX round-trip** (preservation-safe both directions). The
page surface **lowers to a native `<Table>`** (S-03 / `sheet.lower.page`)
with live **multi-frame pagination** across the host frame chain (S-05).
The **in-frame grid renders + edits via K-1** (`sheet.grid.inframe`):
`host.contribute.sceneLayer` paints the windowed grid (C-1 vector v0.39.0
+ text v0.40.0), and the modal edit session is wired end-to-end
(double-click entry → content-coord cell select → keystroke editor →
Enter/Esc, session-scoped Cmd-Z per ADR-012) — driven by the editor
journey `apps/canvas/tests/journey/plugins/sheet.journey.spec.ts` and the
e2e `sheet-modal-session.spec.ts`. Workbook **persistence** rides
`host.blob` (OPFS, S-08) + the metadata binding envelope; the
**data-provider CONSUMER** (S-15, `sheetFromDataset`) sources a sheet from
a governed dataset.

## Project State & Feature Matrix (paged-media/state)

The canonical feature inventory + live status for ALL Paged repos live in
`paged-media/state` (dashboard: https://state.paged.media). There is NO
feature matrix in this repo; do not create one. NEW CAPABILITY → registry
row; EVERY NEW TEST → feature linkage (until the `#[feature_test]` macro
ships from state, the naming convention
`fn <feature_id_with_underscores>_…()` + the row's `tests:` pointer);
STATUS CHANGE → registry, not prose. The status-ledger row
`state/registry/features/plugin-sheets.yaml` lives in the STATE repo
(separate PR there). The local `registry/` here is the BUILD-CONSUMED
half (see "Two-registry split" below).

## Hard rules (this repo's constitution — spec §1/§2/§3)

- **ALL SPREADSHEET SEMANTICS LIVE IN RUST.** Formula parsing,
  evaluation, the function library, coercion, number formatting, XLSX
  I/O, and lowering geometry are `sheet-*` crates compiled to ONE wasm
  module (`sheet-js`). The TS packages are thin glue: bundle lifecycle,
  panel, file input, and translating the engine's already-computed
  output into host mutations. **Never implement an Excel-like operation
  in TypeScript** — if the bundle seems to need one, the missing piece
  is a `sheet-js` API.
- **ISOLATION CONTRACT, superset (§2.1).** Zero core contact AND zero
  inter-plugin contact: the only `@paged-media/*` dependencies are
  `plugin-api`, `plugin-sdk`, and published package contracts — never
  `plugin-image` or any other plugin, not at build time, runtime, or via
  side channels, even co-installed. (`paged.draw` chart lowering in T2
  is a CORE SDK surface, not a plugin.) TS guard:
  `scripts/check-contract-imports.mjs`; Rust guard: `deny.toml`
  [sources] + the cargo-tree CI guards. SDK gaps become
  RFI §6 entries / plugin-platform RFCs — NEVER core
  modifications from this project.
- **REGISTRY-DRIVEN DISPATCH (§7/§12.2).** The function table is
  generated at build time from `registry/functions/*.yaml`
  (`sheet-core/build.rs` emits the name→id table; `sheet-fn/build.rs`
  emits the dispatch match). No row → no dispatch entry → **an
  unregistered function is uncallable by construction**. Same principle
  queued for XLSX part handlers and lowering rules. The coverage gate
  (`cargo run -p sheet-conformance --bin coverage-gate`) fails below
  100% tests-per-implemented-row.
- **PURE KERNELS.** `sheet-fn` functions are pure
  `fn(&[Arg], &EvalCtx) -> CellValue` — they never see the dependency
  graph, the scheduler, or the SDK (spec §4 rule 1). `sheet-lower` is
  pure model→IR. `sheet-host-model` (TS) is pure data→Mutation[]. Every
  behavior change lands with a test.
- **PRESERVATION INVARIANT (§10.2).** "Paged never destroys a
  workbook." Unknown parts byte-identical; unknown subtrees in known
  parts retained and re-emitted in place; untouched understood parts
  re-emit original bytes (lazy-verbatim). Zero-edit round-trip is
  byte-identical except the dropped `calcChain.xml` (registry ruling
  `sheet.xlsx.calcchain.drop`).
- **EXCEL-COMPAT IS A RULING, NEVER AN ACCIDENT (§3).** f64 arithmetic
  (D-6; the `Numeric` trait keeps exact-decimal open), 1900 leap-bug
  serial 60, 15-significant-digit display — each adopted defect is an
  explicit registry ruling with provenance. Oracle disagreements get
  recorded rulings.
- **PUBLISHING-FIRST SCOPE (§1, permanent).** Pivot tables, data
  validation, what-if, external links, VBA execution are NEVER
  interpreted — they round-trip preserved. This is a product decision,
  not a deferral; don't "helpfully" implement them.
- **The bundle touches host surfaces + React only.** No
  `@paged-media/shell`/`client` imports — writes via
  `host.document.mutate`, binding via `setPluginMetadata` (namespace
  `x-paged:media.paged.sheet`), persistence via `host.blob` (OPFS,
  S-08 shipped — per-plugin keyed, restores the LAST imported workbook;
  the panel says so). Panels are factories closing over `BundleHost`;
  styling = the token layer (`--pg-*`, `--status-*`, `--font-mono`,
  `--space-*`, `--radius-*`).
- **Reserved seams stay honest.** The page-lowering (S-03), pagination
  (S-05), in-frame grid + K-1 modal edit session (S-02/C-1/K-1), importer
  registration (S-06/S-11), OPFS persistence (S-08), and the data-provider
  consumer (S-15) all SHIP now (registry-confirmed). What is still open
  stays labelled, never faked: per-cell fill/borders await a `tableCell`
  ElementId kind; the editor-side metrics RPC (K-7) still returns the
  estimate path; the §8.0 seamless-undo boundary is TBD; in-frame text v1
  has caveats (default face, upright glyphs); full sheets-MODE (S-01),
  owned-content interception (S-09), and range clipboard (S-14) are open.
  The manifest + UI must keep saying so for the unshipped pieces.
- **CLEAN-ROOM (§3).** `references/` (LibreOffice/IronCalc, IF ever
  mounted) is read-only, analyst-only, gitignored, excluded from all
  artifacts; implementers never read it. **T0: references/ is NOT
  mounted** — implementation derives from ECMA-376 / ISO-IEC 29500,
  OpenFormula, public Microsoft documentation, and golden corpora.
- **LICENSE ASYMMETRY.** Rust crates are dual MPL-2.0 OR PMEL — every
  `.rs` carries the 13-line MPL/PMEL header (copy from
  `sheet-core/src/lib.rs`). TS files (`packages/`, `scripts/`) carry NO
  header (private-side convention, like plugin-draw/plugin-web).
- **Interface freeze.** `sheet-core` types, the AST, the `sheet-fn`
  calling convention (`Arg`/`EvalCtx`), and the registry YAML schema are
  FROZEN (M0 phase 0). Changes go through the orchestrator as versioned
  amendments, never drive-by edits.

## Two-registry split

- `paged-media/state` `registry/features/plugin-sheets.yaml` — the
  STATUS ledger (stage `plugin.sheet`; planned/partial/shipped).
- `plugin-sheets/registry/` (here) — build-consumed metadata:
  `functions/*.yaml` (one row per function: arity, volatility,
  range-awareness, provenance, test pointers — drives codegen) and
  `features/*.yaml` (calc/format/xlsx/lower rulings + test pointers).
  The ids mirror the state `sheet.*` ids so the registries join by id.

## Commands

```bash
# Rust (the engine)
cargo build --workspace && cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p sheet-conformance --bin coverage-gate    # the §12.2 gate

# Dependency guards (CI runs these; run before claiming green)
cargo tree -p sheet-fn --edges normal | grep -E 'sheet-(calc|parser|xlsx|lower|js)' && echo LEAK
cargo tree -p sheet-js --target wasm32-unknown-unknown | grep -E 'sheet-conformance|proptest' && echo LEAK
cargo deny check

# wasm artifact (8 MiB budget; lands in packages/sheet-bundle/bin/)
bash scripts/build-wasm.sh

# TS (the bundle) — install order: editor → plugin-sdk → plugin-sheets
pnpm install && pnpm test && pnpm typecheck
pnpm validate:manifest

# Optional LibreOffice differential oracle (CI container; not local)
PAGED_SHEET_ORACLE=1 cargo test -p sheet-conformance -- --ignored
```
