# plugin-sheets — paged.sheet

The spreadsheet subsystem of the Paged ecosystem: a Rust/WASM
calculation engine and sheet document model delivered as a **Paged
plugin**, whose output is displayed through sheet frames on the canvas.
Live spreadsheets inside a print-grade layout document — financial
reports, price lists, data sheets — with the sheet as the live source of
truth and the page as its typeset projection. **A publishing instrument,
not an Excel replacement.**

Concept (the authority):
`thoughts/docs/paged/plugin-sheets/base-idea.md` (v0.3). SDK gap punch
list: [`BREAKAGE_LOG.md`](./BREAKAGE_LOG.md) (S-NN). Dual-licensed
MPL-2.0 OR PMEL (see [`LICENSE.md`](./LICENSE.md)). First-party in
authorship, third-party in discipline: the only dependency surface is
the published plugin SDK.

## Architecture rule

**All Excel-like operations live in the plugin's own Rust core** —
formula parsing, evaluation, the registry-driven function library,
number formatting, XLSX round-trip, and lowering geometry compile to one
self-contained wasm module (`sheet-js`). TypeScript is thin glue:
bundle lifecycle, the workbook panel, and translating the engine's
already-computed output into committed host mutations.

## Packages

| Package | Contents |
|---|---|
| `sheet-core` | frozen types: `CellValue`, `CellRef`/`RangeRef`, `SheetModel`, the canonical AST, interners; registry codegen (name→id) |
| `sheet-parser` | Excel-dialect formula lexer + Pratt parser → canonical AST; printer; reference extraction; structural rewrite |
| `sheet-calc` | dependency graph, dirty propagation, topological recalc, cycle detection, volatility, deterministic RNG |
| `sheet-fn` | registry-driven function dispatch; pure `fn(&[Arg], &EvalCtx) -> CellValue` kernels; coercion machinery |
| `sheet-format` | ECMA-376 number-format engine (T0 core), General semantics, 1900/1904 date serials (leap-bug ruling) |
| `sheet-xlsx` | OPC/zip + SpreadsheetML parse, preservation model, writer — zero-edit round-trip byte-identical |
| `sheet-lower` | pure range → `LoweredContent` IR (formatted text, widths, rules) for the page surface |
| `sheet-js` | wasm-bindgen surface consumed by the bundle (`SheetHandle`: load/save xlsx, set_cell, get_range_lowered, …) |
| `sheet-conformance` | TEST-ONLY: golden corpora, property tests, env-gated LibreOffice oracle skeleton, the coverage gate |
| `@paged-media/sheet-host-model` | pure TS: `LoweredContent` → `Mutation[]` translation (no spreadsheet semantics) |
| `@paged-media/sheet-bundle` | manifest (`media.paged.sheet`) + `activate(host)` + workbook panel + engine boot |

## Setup

Sibling checkouts; install order matters (`link:` chain):

```
~/paged/
├── editor/         pnpm install   (1st)
├── plugin-sdk/     pnpm install   (2nd)
└── plugin-sheets/  pnpm install   (3rd)
```

```bash
# Rust engine
cargo build --workspace && cargo test --workspace
bash scripts/build-wasm.sh        # → packages/sheet-bundle/bin/ (8 MiB budget)

# TS bundle
pnpm install && pnpm test && pnpm typecheck
node ../plugin-sdk/packages/plugin-cli/bin/paged-plugin.mjs validate packages/sheet-bundle/manifest.json
```

## T0 scope honesty

- Page lowering emits a **native Paged `<Table>`** (S-03 resolved):
  `insertTable` + per-cell text pour + merge spans + cell fills/edge
  strokes. The tab-text + drawn-rules degradation is retained as the
  explicit fallback lane (and engages when a host rejects `insertTable`).
- **No sheets mode yet**: frame activation (S-01) and the grid rendering
  surface (S-02) are SDK gaps; the T0 flow is the workbook panel +
  commands.
- **Workbook bytes are in-memory only** (no OPFS capability — S-08);
  the lowered page content persists, the live workbook does not.
- Import via the panel's file input (S-06/S-11).
- Recalc is single-threaded (no worker capability — S-07).
- Verification is golden-corpora + property tests; the LibreOffice
  differential oracle is an env-gated CI skeleton.

## Milestones (spec §13)

All four milestones are implemented; the registry coverage gate is at
0 gaps (345 implemented rows, 2 honest deferrals — see below).

- **M0 — spine + round-trip + first lowering** ✅ — frozen `sheet-core`,
  parser, dep graph + recalc, T0 functions via registry-driven dispatch,
  number-format core, XLSX parse + preservation + writer (zero-edit
  round-trip byte-identical), single-frame lowering, coverage gate live.
- **M1 — the publishing product** ✅ — ~145 T1 functions, dynamic
  arrays/spill, structured references + Excel tables, full number-format
  engine, pagination engine (multi-frame chains), the panel grid
  (`sheet-grid` GridScene), document-coherent styling.
- **M2 — depth** ✅ — charts (`plotters` layout → custom backend →
  `ChartGeometry` → `paged.draw` native ops; colors via document
  swatches), conditional formatting (→ style overrides), iterative calc
  (D-7), database D-functions + the misc T2 set.
- **M3 — breadth** ✅ — localization (en/de, D-8), the exact-decimal
  spike (D-6: `rust_decimal` behind the off-by-default `exact-decimal`
  feature + `DECIMAL-SPIKE.md` recommending defer-to-v2-as-opt-in),
  external-link cached-value reads (cached-only, never followed).

**Honest deferrals** (registry `planned`, SDK/spec-gated, never faked):
in-frame sheets mode (S-01/S-02 SDK surface), conditional-formatting
data-bar drawn geometry, the "new style from selection" document-style
group (S-04), and full external-reference *evaluation* (needs a frozen-AST
amendment). The interim panel grid stands in for in-frame sheets mode;
page lowering is native-table (S-03 resolved; the tab-text degradation
is the retained fallback lane). SDK gaps are tracked in the cross-repo
RFI (`thoughts/docs/paged/plugin-platform/rfi-core-sdk-gaps.md` §6).

## License

Dual-licensed **AGPL-3.0 OR the Paged Media Enterprise License (PMEL)** —
the same as the paged editor (a plugin is part of the editor app). The engine
(`paged-media/core`) and the plugin SDK (`paged-media/plugin-sdk`) it builds on
are MPL-2.0 OR PMEL. See [`LICENSE.md`](./LICENSE.md), [`LICENSE`](./LICENSE),
and [`CONTRIBUTING.md`](./CONTRIBUTING.md) (contributions under a CLA).

`SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-PMEL`
