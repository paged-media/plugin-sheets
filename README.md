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

- Page lowering is **degraded**: tab-aligned text + drawn rules — the
  wire has no table-creation op (S-03). Upgrades to native tables when
  the table-model RFC lands.
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

- **M0 — spine + round-trip + first lowering** *(this repo's current
  campaign)*: frozen `sheet-core`, parser, dep graph + recalc, T0
  functions via registry-driven dispatch, number-format core, XLSX
  parse + preservation + writer (zero-edit round-trip), single-frame
  lowering, coverage gate at 100%.
- **M1 — the publishing product**: T1 functions, dynamic arrays, full
  number formats, sheets mode (grid on the SDK surface), threading +
  pagination, document-style mapping.
- **M2 — depth**: charts (via `paged.draw` core surface), conditional
  formatting, iterative calc.
- **M3 — breadth**: localization, exact-decimal spike.
