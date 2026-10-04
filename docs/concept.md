# paged.sheet — Core Layer Technical Specification

June 2026. Concept paper. Sections describe intent; where the implementation differs, `status.md` and the ADRs in `adr/` are authoritative.

Sections not relevant outside the original planning context have been removed; numbering is unchanged.

**v0.3 changes (review feedback):**
- **Frame operations respected on both surfaces** (§8.5): Paged frames
  carry core-owned operations — scaling, rotation, skew, repositioning,
  cropping — and sheet-frame rendering must reflect them. Specified: the
  content-space principle (the plugin always works in frame-content
  coordinates; core applies frame transforms), the resize-vs-transform
  distinction for pagination, sheets-mode behavior in transformed frames,
  and the input-mapping SDK requirement. Strengthens D-10.

**v0.2 changes (review feedback):**
- **Publishing-first scope cut:** pivot tables, data validation, what-if,
  external links and similar analyst-grade machinery removed from semantic
  scope entirely (they remain round-trip-preserved, never interpreted) —
  Paged is a web-based publishing platform, and `paged.sheet` exists to
  publish live tables, not to replace Excel (§1, §11).
- **Sheets mode specified:** double-click on a sheet frame switches the
  editor into a full Excel-like grid mode (rows/cells, styling, formulas,
  charts) — activation contract and editing-surface requirements added
  (§8.0, §2.2).
- **Two-surface rendering model** replaces the single lowering answer:
  the *page* surface stays compiled-to-native content; the *sheets-mode
  grid* renders directly as vector data on a Vello-class surface provided
  through the SDK (§8.1).
- **Charts are in scope** (created in sheets mode, lowered through
  `paged.draw` — core SDK, permitted under §2.1); D-4 re-ruled (§14).
- **Document-coherent styling elevated to a constitutive principle:**
  document styles are the single source of styling truth; the grid's
  styling tools apply and manage document styles via the SDK rather than
  reinventing a parallel styling system (§8.3).

---

## 1. Purpose and scope

`paged.sheet` is the spreadsheet subsystem of the Paged ecosystem: a
Rust/WASM calculation engine and sheet document model delivered as a
**Paged plugin**, whose output is displayed through **sheet frames** on the
canvas provided by Paged itself.

The product thesis: live spreadsheets inside a print-grade layout document.
Financial reports, price lists, data sheets, annual reports — the B2B print
genres Paged's audience produces — are spreadsheet content trapped in
layout tools today (dead table imports; no layout fidelity in
web sheets). `paged.sheet` makes the sheet the live source of truth and the
page its typeset projection. **It is a publishing instrument, not an Excel
replacement** — every scope decision below follows from that.

Six properties are **constitutive** — they hold from M0 and are never
phased in:

- **Strict plugin independence.** `paged.sheet` is 100% independent of any
  other plugin. Its only dependency surface is the **Paged SDK**
  (`@paged-media/plugin-api` / `@paged-media/plugin-sdk`) and published
  package contracts. It never imports, calls, discovers, or communicates
  with another plugin (including `plugin-image`) — not at build time, not
  at runtime, not via side channels — even when co-installed (§2.1).
- **Publishing-first scope.** Analyst-grade machinery — pivot tables, data
  validation UIs, what-if analysis, external workbook links, query/Power-
  Query artifacts — is **never semantically supported**. Such content
  round-trips losslessly through the preservation invariant (§10.2) but is
  never interpreted, rendered, or editable. The supported surface is what
  publishing needs: values, formulas, formats, styles, tables, charts.
- **XLSX read and write from M0** (§10). XLSX is the interchange currency.
  Round-trip safety ("Paged never destroys a workbook") is a launch
  property — and the preservation invariant is precisely what makes the
  publishing-first scope cut safe: everything out of scope survives
  untouched.
- **Two-surface rendering, no third** (§8). On the **page**, sheet frames
  are compiled to native Paged content (tables/text/rules) via committed
  Operations — Parley typesets, Vello renders, print/PDF export needs
  nothing new. In **sheets mode** (double-click a sheet frame), the plugin
  renders a full Excel-like grid directly as vector data on an SDK-provided
  surface. No raster output anywhere.
- **Document-coherent styling** (§8.3). Typography, colors, backgrounds,
  borders — the document's style systems are the single source of styling
  truth. The grid's styling tools *apply and manage document styles through
  the SDK*; the plugin does not build a parallel styling universe. Core
  already owns these systems; we do not reinvent the wheel — and we still
  reach it only through the SDK.
- **100% tested and verified operations** (§12). Every formula function,
  number-format feature, XLSX part handler, lowering rule, and grid-render
  feature is registry-listed and tier-tested; unregistered functions are
  uncallable by construction.

The engine remains **CPU/WASM** for calculation; the sheets-mode grid is
the one place GPU-backed vector rendering appears, and it arrives via the
SDK surface contract, not via plugin-owned rendering machinery (§8.1).

**Out of scope** (companion specs): the sheets-mode editing UX in detail
(cell editor, formula bar, fill handles, selection model — the *mode
contract* is in scope here, §8.0), chart design UX (the chart *model and
lowering* are in scope, §8.4), collaboration semantics beyond the Operation
model, the Boa end-user API.

### 1.1 Non-goals

- No server-side calculation. The engine runs client-side WASM; the same
  crates compile natively (napi-rs) for optional bulk tooling only.
- No Emscripten, no C/C++ anywhere in this plugin.
- No third rendering surface: page = lowered native content; sheets mode =
  vector grid on the SDK surface; nothing is ever rasterized by this
  plugin, and the plugin never draws into the page outside sheets mode.
- No analyst-grade semantics, permanently: pivot tables, data validation,
  what-if/goal-seek, external workbook links, Power Query artifacts, VBA/
  XLM execution, ActiveX, RTD/web functions. All such payloads round-trip
  preserved (§10.2); none are interpreted. This is a product decision, not
  a deferral.
- No full Excel function parity. The function library is curated for
  publishing workloads (§11); coverage is honest in the public conformance
  matrix.

#### 1.1.1 Pivot tables — explicit non-goal (not a deferral)

Pivot tables are **permanently out of scope** for `paged.sheet`, by product
decision rather than schedule. `paged.sheet` is a *publishing instrument* — it
typesets live workbooks into print-grade pages; it is not an analysis tool. A
pivot table is interactive, exploratory analyst machinery (drag-to-summarize,
collapse/expand, slicers, re-pivot) whose value is the *interaction*, not a
static projection — exactly the surface a print/publishing product does not own.
The data-summary story Paged DOES tell lives elsewhere in the platform: **grouped
aggregation + record flow is `paged.data`'s job** (its DSL has grouping/rollup),
and **visual summaries are the chart engine** (`sheet-chart`, the frozen
`ChartGeometry` IR). Within `paged.sheet`, a summary table is authored as ordinary
cells with `SUMIFS`/`COUNTIFS`/`GETPIVOTDATA`-free formulas and rendered through
the normal lowering path. A workbook that *arrives* carrying pivot artifacts is
**never destroyed**: the `pivotCache*`/`pivotTable*` parts (and slicers) are
unknown subtrees retained byte-faithfully under the preservation invariant
(§10.2) and re-emitted in place on round-trip — they simply are not interpreted,
recalculated, or made editable. This is the same posture as data validation,
what-if, external links, and VBA (§1.1): preserved, never interpreted. Do not
"helpfully" implement a pivot engine; the honest answer to "can paged.sheet pivot?"
is "no, by design — use `paged.data` grouping or the chart engine."

#### 1.1.2 Exact-decimal arithmetic — non-goal; f64 (IEEE-754) is a ruling

`paged.sheet` evaluates in **`f64` (IEEE-754 double)**, exactly as Excel does
(decision D-6). An exact base-10 decimal numeric type is an **explicit non-goal
for v1** — and, for the default mode, a *permanent* one — because **fidelity
REQUIRES f64**: the whole conformance posture (the golden corpora, the planned
LibreOffice differential oracle) is tested against Excel, and Excel is f64-based.
An exact-decimal default would diverge from that oracle *by design* on precisely
the cases where binary and base-10 disagree, turning passing conformance into
failing conformance. The classic implication — `0.1 + 0.2` accumulates to the
IEEE value `0.30000000000000004` rather than `0.3` — is **inherited from Excel,
not a paged.sheet defect**, and it is **mitigated in the product surface** by the
15-significant-digit *display* rounding rule (D-6), implemented in
`sheet-format` (`sheet-format/src/general.rs::round_sig(x, 15)` for the General
path, plus the section decimal rounding in `sheet-format/src/number.rs`): a v1 print document shows `0.3`, never
the binary tail. The correctness win of exact-decimal is therefore largely
*invisible* in v1's actual rendered output, which is the product surface that
matters for a publishing instrument.

The door is nonetheless **proven open, not foreclosed** (the D-6 boundary
requirement): the `Numeric` trait in `sheet-fn` (`sheet-fn/src/num.rs`) abstracts the numeric
type, and the M3 spike (`DECIMAL-SPIKE.md`; `sheet-fn/src/num_decimal.rs` behind
the OFF-by-default `exact-decimal` cargo feature; corpus
`corpus/decimal-corpus/divergence.golden.tsv`; ruling `sheet.calc.decimal.*`)
demonstrated an exact base-10 backend dropping in as a pure type substitution
with no kernel rewrite (~40 KiB wasm, ~6-7× slower multiply). The spike's
recorded recommendation stands: **DEFER an exact-decimal *mode* to v2 as an
explicit opt-in flag** (it would need a per-workbook `CalcSettings.precision`
setting, an XLSX round-trip story Excel has no "decimal mode" to round-trip to,
and a formatter/coercion review), and **keep f64 the v1 default**. Note the
central caveat: decimal is not a universal cure — `1/3 × 3` is non-terminating in
base-10 too, so exactness is bought only for *terminating* decimals, not ratios
or transcendentals. Do not adopt exact-decimal as the default under any
circumstances; it can only ever be an opt-in the user accepts as "this no longer
matches Excel."

---

## 2. Position in the Paged ecosystem: an independent plugin

*Status note (2026-10-02): this section predates the implementation; see `architecture.md` (the repository is `paged-media/plugin-sheets`; the glue is the TypeScript package `packages/sheet-bundle`) and, in plugin-sdk, [ADR 314](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/314-plugin-shape.md) (the plugin shape) and [ADR 319](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/319-trust-line.md) (where a bundle runs).*

`paged.sheet` is a **Paged plugin bundle** in its own repository
`paged-media/plugin-sheet`. First-party in authorship, third-party in
discipline — the same posture as `plugin-image`, with one rule added.

```
┌────────────────────────────────────────────────────────────────┐
│ Paged (untouched)                                              │
│  Layer 4  Plugin bundles                                       │
│  Layer 3  React shell            ┌──────────────┐              │
│  Layer 2  Boa scripting          │ plugin-image │ ← NO contact │
│  Layer 1  Rust/WASM core         └──────────────┘   in either  │
│   (Vello · Parley · paged.draw · salsa · op log)     direction │
└───────────────────────┬────────────────────────────────────────┘
                        │ @paged-media/plugin-api / plugin-sdk
                        ▼            (the ONLY boundary)
┌────────────────────────────────────────────────────────────────┐
│ paged.sheet plugin bundle (own repo, own WASM module)          │
│  manifest (capabilities) · Boa-side glue · declarative panels  │
│  sheet-* crates (§4) compiled to a self-contained WASM module  │
└────────────────────────────────────────────────────────────────┘
```

### 2.1 Isolation contract (superset of the plugin-image contract)

*Status note (2026-10-02): this section predates the implementation; see [ADR 315](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/315-isolation-contract.md) in plugin-sdk (the isolation contract). In this repository the import rule is checked by `scripts/check-contract-imports.mjs` and `deny.toml`; the plugin spawns no workers; and data from another plugin arrives only through the host's data-provider door ([design/data-provider-consumer.md](design/data-provider-consumer.md)).*

1. **Zero core contact.** No imports from `core/` or `editor/` internals,
   no patched Paged builds, no core feature flags for this plugin. CI
   builds against *published* SDK canaries only.
2. **Zero inter-plugin contact.** No dependency on, import of, runtime
   discovery of, message-passing with, or shared state with any other
   plugin — including `plugin-image`. Anything another plugin could
   provide is either reached through a **core SDK surface** (e.g. images
   in cells go through the document's standard asset mechanism;
   vector output lowers through `paged.draw`, which is core, not a
   plugin) or is out of scope. Co-installation changes nothing.
3. **Capability-gated everything.** The manifest declares all needs
   (document read scopes, content-mutation scopes for owned frames, panel
   surfaces, worker spawn, OPFS quota). Read-broadly / write-narrowly:
   all writes are committed **Operations through the SDK mutation
   surface**; no back-channel into document state.
4. **Own runtime, own memory.** Self-contained WASM module, own heap, own
   worker pool. One Boa sandbox per plugin; the Boa side is thin glue
   (panels, op submission, lifecycle).
5. **Gaps become RFCs, not hacks.** SDK shortfalls produce plugin-platform
   RFCs (§2.2); never core modifications, never reach-arounds.

**CI enforcement:** dependency-cruiser/cargo-deny allows exactly
`@paged-media/plugin-api`, `@paged-media/plugin-sdk`, and published package
contracts; any other `@paged-media/*` dependency — and any dependency whose
provenance is another plugin's repo — fails the build.

### 2.2 Required SDK surface (gap analysis → RFCs, due before M0)

*Status note (2026-10-02): this section predates the implementation; most rows of this table are now host doors the plugin uses. See [ADR 505](adr/505-native-table-and-edit-grid.md) (native table content, frame activation, the editing surface) and [ADR 506](adr/506-workbook-in-a-container-part.md) (storage), and in other repositories [ADR 012](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/012-k1-modal-session-undo-coalescing.md) (plugin-sdk: the modal editing session), [ADR 013](https://github.com/paged-media/core/blob/main/docs/adr/013-in-frame-scenelayer.md) (core: the rendering surface) and [ADR 017](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/017-importer-exporter-door-shape.md) (plugin-sdk: importer and exporter registration). The plugin declares no worker capability.*

| Need | Likely status in plugin spec | If missing |
|---|---|---|
| Read access to document structure, styles, frames | covered (read capability) | — |
| **Frame activation hook: double-click on owned frame → plugin enters sheets mode** | to verify | **RFC: owned-frame activation events + modal editing-session contract (enter/exit, dirty-state, Esc/commit semantics) — incl. pointer/keyboard events delivered inverse-transformed into frame-content coordinates for transformed frames (§8.5)** |
| **Editing surface for sheets mode: vector rendering target the plugin draws the grid into** | to verify | **RFC: SDK rendering surface — preferred: a Vello scene/display-list contract (plugin submits content-space vector content, core renders it in-context and applies frame transforms); fallback: plugin-owned `GPUCanvasContext` overlay — which cannot honestly render inside rotated frames (§8.5). D-10 (§14)** |
| **Frame geometry read: content-box dimensions + transform, as distinct values** | to verify | RFC clause: reflow subscription carries content-box geometry; pure transforms (scale/rotate/skew) do not fire it (§8.5 resize-vs-transform) |
| Commit Operations producing **table/text/rule content** inside frames the plugin owns | mutation surface covered; *table content schema* to verify | **RFC: native table content model** (§8.2); if Paged tables are not yet first-class, lowering degrades to tab-aligned text + drawn rules until they are |
| **Document style read AND write (create/modify table, paragraph, character styles from the grid's styling tools)** | read covered; write to styles to verify | **RFC: style-management capability** — required by the §8.3 styling principle |
| Frame ownership & lock semantics (content marked plugin-compiled) | to verify | RFC: owned-content attribute + edit-interception hook |
| Frame linking/threading metadata (next/previous frame of a chain) | text threading exists in core; plugin visibility to verify | RFC: read access to frame-chain topology + overflow notification |
| Reflow notification (frame resized/moved → re-paginate rows) | to verify | RFC: layout-change subscription for owned frames |
| `paged.draw` access for chart lowering (core surface, §8.4) | covered in principle (core SDK) | verify scripting-level sufficiency for chart geometry |
| Asset placement in cells (images via standard asset mechanism) | covered (core asset surface) | — |
| Worker spawn + SharedArrayBuffer within bundle | shared with plugin-image RFC | RFC: worker capability with COOP/COEP guarantees |
| OPFS quota (workbook cache, large-model spill) | shared with plugin-image RFC | RFC: storage capability with quota declaration |
| Register importer/exporter (XLSX opens via plugin) | shared with plugin-image RFC | RFC: importer/exporter registration capability |

Several rows are the *same* RFCs `plugin-image` files — independence does
not mean duplicating platform work; both plugins consuming one RFC is the
plugin platform doing its job.

The viewer-grade subset (`sheet-core` + `sheet-calc` + `sheet-xlsx`,
read-only: parse, calculate, present lowered content) packages as a
viewer-compatible bundle without the mutation surface — mirroring
`EditorSession extends ViewerSession` at the plugin level.

---

## 3. Legal and methodological ground rules

*Status note (2026-10-02): this section predates the implementation; see [ADR 500](adr/500-own-calculation-engine.md) (no reference engine source was mounted) and [ADR 507](adr/507-golden-corpora-coverage-gate.md) (the LibreOffice oracle named below is not built; verification rests on authored golden corpora and a coverage gate).*

| Rule | Detail |
|---|---|
| **Specifications first** | Formula semantics, number formats, and XLSX structure are *publicly specified*: ECMA-376 / ISO-IEC 29500 (OOXML, incl. number-format codes and the SpreadsheetML formula grammar) and OASIS OpenFormula (ODF 1.3 part 4). Implementation derives from these standards plus black-box behavior — a materially better clean-room position than `plugin-image` has. |
| **Reference engines** | Any engine source mounted under `plugin-image`-style `references/` (candidates: LibreOffice Calc, IronCalc) follows the **analyst/implementer protocol** verbatim from the `plugin-image` concept §3.1 (`plugin-image: docs/concept.md`): analysts read references and produce behavior specs; implementers never read `references/`; CI-guarded; `references/` excluded from all published artifacts. Permissively licensed references (IronCalc, MIT/Apache — license to be confirmed in A-0) *may* be graduated to direct use only by an explicit legal ruling, never by default. |
| **Oracle use** | Headless LibreOffice Calc runs in CI as the primary differential oracle (§12.4); golden expected-value corpora are second; Excel itself is periodic manual verification. Behavior is not copyrightable. |
| **Bug-for-bug rulings** | Excel compatibility includes deliberate historical defects (the 1900 leap-year bug, 15-significant-digit display, date serial edge cases). Each adopted defect is an explicit registry ruling with provenance — compatibility decisions are documented, never accidental. |
| **Provenance log** | Every function, format feature, and XLSX part handler records its sources (`ECMA-376 §…`, `OpenFormula §…`, oracle observation, corpus file) in its registry `provenance:` block. |
| **License** | See `LICENSE.md`. CLA applies. |

---

## 4. Crate architecture

*Status note (2026-10-02): this section predates the implementation; see `architecture.md` (there is no `manifest/` or `glue/` directory: the bundle is `packages/sheet-bundle`, with the translation layer in `packages/sheet-host-model`; `sheet-lower`, `sheet-grid` and `sheet-chart` produce plain data and do not touch the SDK) and [ADR 314](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/314-plugin-shape.md) in plugin-sdk.*

Repository `paged-media/plugin-sheet`:

```
plugin-sheet/
├── manifest/            # plugin manifest, capability declarations, panel schemas
├── glue/                # Boa-side glue: lifecycle, panels, Operation submission via SDK
├── references/          # READ-ONLY if mounted (LibreOffice/IronCalc) — analyst-only, excluded from artifacts
├── sheet-core/          # types: CellRef, Range, CellValue, Style, NamedRange, SheetModel
├── sheet-parser/        # formula lexer/parser → canonical AST; Excel dialect + OpenFormula mapping
├── sheet-calc/          # dependency graph, dirty propagation, recalc scheduler, volatile policy
├── sheet-fn/            # function library: registry-driven dispatch, one module per family
├── sheet-format/        # number-format engine (ECMA-376 format codes), date systems, locale data
├── sheet-xlsx/          # OPC/zip + SpreadsheetML parse, preservation model, writer
├── sheet-lower/         # range → Paged content lowering, pagination, style mapping (§8.2–8.3)
├── sheet-grid/          # sheets-mode vector grid: virtualization, scene generation for the SDK surface (§8.1)
├── sheet-chart/         # chart model + geometry generator → paged.draw lowering / grid view (§8.4)
├── sheet-conformance/   # TEST-ONLY: oracle harness, corpora, coverage gate — never shipped
└── sheet-js/            # wasm-bindgen surface consumed by glue/ and the viewer bundle
```

**Dependency rules (CI-enforced):**

1. `sheet-fn` depends only on `sheet-core` (+ `sheet-format` for
   formatting-aware functions like `TEXT`). Functions never see the
   dependency graph, the scheduler, or the SDK — they are pure
   `fn(&[Value], &EvalCtx) -> Value`.
2. `sheet-lower` and `glue/` are the only crates touching the SDK mutation
   surface; `sheet-grid` touches only the SDK rendering-surface contract.
3. `sheet-chart`'s geometry generator is pure (model → vector geometry);
   only its lowering half sees `paged.draw` via the SDK.
4. The SDK rule and the inter-plugin rule of §2.1 apply to every crate.
5. `sheet-conformance` is dev-dependency-only; `cargo tree` check proves no
   test code is reachable from the wasm release build.

The load-bearing constraint: **functions are pure with a frozen signature**,
so the calc engine, the conformance harness, and (future) Boa exposure all
consume one definition — the same role kernels play in `plugin-image`.

---

## 5. Core types (`sheet-core`)

### 5.1 Values

```rust
pub enum CellValue {
    Empty,
    Number(f64),                 // Excel-compat: IEEE-754 double (D-6)
    Text(CompactString),
    Bool(bool),
    Error(CellError),            // #DIV/0!, #VALUE!, #REF!, #NAME?, #NUM!, #N/A, #NULL!, #SPILL!
    // Rich values (arrays/spill) are ranges of the above, not a variant —
    // dynamic-array results materialize into spill ranges (§6.4)
}
```

- **Precision policy (D-6):** v1 is Excel-compatible — `f64` arithmetic,
  15-significant-digit *display* semantics implemented in `sheet-format`,
  Excel's documented rounding quirks adopted as explicit registry rulings.
  An exact-decimal mode is a possible v2 differentiator; the `CellValue`
  design must not foreclose it (numeric type behind a trait boundary in
  `sheet-fn`).
- **Date/time** is not a type: dates are serial numbers + number formats,
  exactly as in Excel. `sheet-format` owns the 1900/1904 date systems and
  the 1900 leap-year bug as a documented compatibility ruling.

### 5.2 Model

*Status note (2026-10-02): this section predates the implementation; see [ADR 506](adr/506-workbook-in-a-container-part.md) (the workbook is stored whole, as XLSX bytes, in a container part of the document).*

```rust
pub struct SheetModel {
    sheets: Vec<Worksheet>,            // grid: sparse BTreeMap<(row, col), Cell> per sheet
    named_ranges: NameTable,           // workbook + sheet scope
    styles: StyleTable,                // interned: fonts, fills, borders, number formats
    defined_behaviors: CalcSettings,   // date system, iteration policy, precision flags
    preserved: PreservedParts,         // §10.2 — opaque OOXML parts/attributes
}

pub struct Cell {
    value: CellValue,                  // last calculated value (cached)
    formula: Option<FormulaId>,        // interned canonical AST
    style: StyleId,
}
```

- Sparse storage; 1M-row × 16k-col address space addressable, memory
  proportional to populated cells.
- Styles, formulas, and strings are interned (shared-string-table
  semantics fall out naturally and round-trip cheaply).
- The model is the plugin's document-level entity; it serializes into the
  Paged document as a plugin-owned payload via the SDK, with XLSX as the
  external interchange form.

---

## 6. Calculation engine (`sheet-parser`, `sheet-calc`)

### 6.1 Parser

*Status note (2026-10-02): this section predates the implementation; see [ADR 509](adr/509-excel-first-dialect.md) (the dialect that was built; R1C1 references, 3-D references and the OpenFormula mapping are not built).*

- Lexer/parser for the **Excel formula dialect** (D-2 default): A1 and
  R1C1 references, structured references (tables), 3-D references,
  defined names, union/intersection operators, array literals.
- Canonical AST is dialect-neutral; OpenFormula maps onto the same AST
  (one grammar module per dialect, one semantics). This keeps an ODS path
  open without committing to it (v2).
- References are stored **relative/absolute-aware** so row/column
  insert/delete and copy/fill rewrite formulas correctly — rewrite rules
  are registry-tested features, not incidental code.

### 6.2 Dependency graph and recalculation — the salsa-shaped core

*Status note (2026-10-02): this section predates the implementation; see [ADR 502](adr/502-recalculation-and-spill.md) (recalculation as built: single-threaded, with no worker pool; a range is one graph node found by a linear scan, not by an interval index).*

- Each formula cell is a node; edges come from reference extraction over
  the AST (including ranges, names, and 3-D spans; range edges are
  interval-tracked, not exploded per-cell).
- **Dirty propagation:** a committed cell edit marks dependents dirty
  transitively; recalculation computes the dirty cut in topological order;
  independent subgraphs recalculate **in parallel** on the worker pool
  (rayon / wasm-bindgen-rayon — the calc engine is the CPU-parallel
  citizen of this plugin, where `plugin-image` had the GPU).
- **Demand-driven where it pays:** off-screen, un-referenced dirty regions
  may defer until a sheet frame, export, or dependent needs them —
  identical philosophy to tile-pull in `plugin-image`, applied to cells.
- **Volatile functions** (`NOW`, `TODAY`, `RAND`, `RANDBETWEEN`, `OFFSET`,
  `INDIRECT`, …) carry a `volatility` flag in their registry rows; the
  scheduler implements Excel's documented triggers as explicit, tested
  policy (D-7 covers iteration/circular-reference policy: off by default,
  Excel-style max-iterations/max-change when enabled).
- Determinism: recalculation order must not affect results (property-
  tested: random valid topological orders → identical values, §12.4).

### 6.3 Mutation: Operations and Gestures

*Status note (2026-10-02): this section predates the implementation; see [ADR 505](adr/505-native-table-and-edit-grid.md) and, in plugin-sdk, [ADR 012](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/012-k1-modal-session-undo-coalescing.md) (cell edits are written to the engine and kept in a session journal that is cleared on exit; they are not operations on the document's undo log, and committing one does not regenerate the page content).*

| Paged concept | sheet realization |
|---|---|
| **Gesture** (ephemeral) | live preview while typing in a cell or dragging a fill handle: speculative parse + recalc of a bounded dependent cut, displayed but never serialized |
| **Operation** (committed) | `SetCell(ref, value/formula)` · `EditStructure(insert/delete rows/cols, rename sheet)` · `SetStyle(range, style)` · `SetName(name, range)` — submitted through the SDK, appended to the op log; each carries its inverse for undo |
| **Undo** | inverse op from the log; structural edits journal the displaced cells/formulas — O(affected), never O(sheet) |

Lowered frame content (§8) regenerates *as a consequence* of committed
Operations — it is derived state, never independently edited history.

### 6.4 Dynamic arrays

Modern Excel spill semantics are in scope from T1: a formula may produce a
range; the engine materializes the spill, owns the spilled cells, and
yields `#SPILL!` on collision. Legacy Ctrl-Shift-Enter arrays parse and
round-trip but evaluate through the same machinery.

---

## 7. Function library (`sheet-fn`)

- **Registry-driven dispatch:** the function table is generated from the
  registry YAML at build time. A function without a registry row has no
  dispatch entry — **an unregistered function is uncallable by
  construction**, which is exactly the property a calc engine wants under
  high-throughput development.
- Pure signature: `fn(&[Value], &EvalCtx) -> Value`. `EvalCtx` provides
  date system, locale, iteration settings, and deterministic RNG seed for
  volatile-function testing.
- Family modules mirror Excel's documentation taxonomy: math/trig, text,
  logical, lookup & reference, date/time, statistical, financial,
  information, engineering, database, web (stubbed/excluded per tier).
- Argument coercion rules (number↔text↔bool↔error propagation, implicit
  intersection) are **shared machinery with their own registry rows** —
  they are where most cross-engine incompatibilities live, and they get
  the dual-oracle disagreement protocol when LibreOffice and Excel differ.

---

## 8. The two-surface model: sheets mode and page lowering

The review question "isn't all of this vector data renderable on the Vello
surface?" has a precise answer: **yes — and there are two different vector
pipelines, each right for its surface.** The grid editor renders vector
content directly; the page compiles to native content so it flows through
core's text shaping, threading, and export machinery instead of this plugin
re-implementing typesetting. Same data, two projections.

### 8.0 Sheets mode: the editing workflow

*Status note (2026-10-02): this section predates the implementation; see [ADR 505](adr/505-native-table-and-edit-grid.md) and, in plugin-sdk, [ADR 012](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/012-k1-modal-session-undo-coalescing.md) (sheets mode is built as an edit context entered by double-click; while it is active, undo is the session's own journal, and leaving the mode does not re-lower the frame).*

The canonical interaction: **double-click a sheet frame → the editor
switches into sheets mode** — a full Excel-like grid over the frame's
backing sheet. In sheets mode the user:

- navigates and edits rows, columns, and cells; enters formulas with
  reference highlighting; uses fill and selection idioms;
- styles cells, rows, columns, and table regions — through the
  document-style mechanism of §8.3;
- creates and edits **charts** bound to ranges (§8.4);
- commits (mode exit, Enter-out, click-out) or abandons (Esc) per the
  modal-session contract (§2.2 activation RFC).

Mechanics: activation arrives via the SDK owned-frame activation hook; the
plugin opens a modal editing session; every confirmed cell/structure/style
change is a committed Operation (§6.3) — sheets mode is a *view*, not a
separate persistence world, so undo history is seamless across modes. On
session end, affected frames re-lower (§8.2). Grid-UX details (cell editor,
formula bar, fill handles) live in the companion UX spec; the mode
*contract* specified here is core-layer scope because it drives SDK RFCs.

### 8.1 Grid rendering: direct vector on the SDK surface

*Status note (2026-10-02): this section predates the implementation; see [ADR 505](adr/505-native-table-and-edit-grid.md) and, in core, [ADR 013](https://github.com/paged-media/core/blob/main/docs/adr/013-in-frame-scenelayer.md) (the preferred contract was built: the grid is submitted as a scene layer; the overlay fallback was not).*

The sheets-mode grid is rendered by the plugin as **vector data** —
virtualized rows/columns, cell text, rules, fills, selection chrome —
drawn into the editing surface the SDK provides:

- **Preferred contract (D-10):** a Vello scene / display-list submission —
  the plugin produces vector content, core renders it in-context. This is
  the cleanest expression of "it's all vector data": one renderer, perfect
  visual consistency with the page, zoom-crisp by construction.
- **Fallback contract:** a plugin-owned `GPUCanvasContext` overlay region;
  the plugin brings minimal vector rendering for the grid (still vector,
  still no rasters — but a second code path and a consistency burden).
- Virtualization is mandatory: render only the visible viewport over the
  sparse model; 1M-row sheets must scroll at frame rate
  (`sheet.grid.virtualization.*` perf gates, §12.5).
- Text in the grid is shaped through whatever text facility the surface
  contract provides; if the Vello-scene contract includes core text
  shaping, the grid gets Parley typography for free — another argument for
  the preferred contract.

### 8.2 Page rendering: lowering to native content

*Status note (2026-10-02): this section predates the implementation; see [ADR 505](adr/505-native-table-and-edit-grid.md) (a range is lowered to a native table by an explicit command; the content is not regenerated on recalculation; a double-click on the frame enters sheets mode, but the plugin does not lock the lowered table against other edits; and the driver that paginates across a frame chain is exported but called by no command or panel).*

On the page, sheet frames remain **compiled, not rendered** (D-1 ruling
unchanged):

- A sheet frame binds `(sheet, range | named range, view options)`. On
  recalculation, structure change, or binding change, `sheet-lower`
  regenerates the frame's content as ordinary Paged content — table
  content where the SDK table model exists (§2.2), with cell text, merged
  spans, borders/rules, and fills in document-native form — submitted as
  committed Operations replacing the previous compiled content.
- Consequences, all intended: Parley typesets it; Vello draws it; zoom is
  vector-crisp; text is selectable/searchable; print/PDF export and the
  IDML round-trip story need nothing new.
- **Ownership (D-5):** compiled content carries the owned-content
  attribute; manual editing is intercepted with "edit the sheet behind
  this frame" — which now has a concrete meaning: **enter sheets mode**.

**Threading and pagination — the killer feature.** Sheet frames link like
text frames thread: rows that do not fit flow to the next linked frame; a
400-row table paginates across a 12-page report and stays live to
recalculation. Per-chain view options: repeated header rows, repeated key
columns, "continued" markers, keep-together hints. Pagination is a bounded
fixed-point loop with layout (lower → reflow → overflow notification →
re-split → settle), conformance-tested for convergence
(`sheet.lower.pagination.*`), including pathological cases (a row taller
than its frame, zero-height frames, circular chain edits). Multiple frames
may bind different ranges of one sheet — summary on page 1, detail on
pages 7–9, one model, one recalc.

### 8.3 Document-coherent styling — the publishing principle

*Status note (2026-10-02): this section predates the implementation; see [ADR 508](adr/508-content-addressed-swatches.md) (workbook colours become document swatches) and, in the editor, [ADR 023](https://github.com/paged-media/editor/blob/main/docs/adr/023-shared-panels-binding-providers.md) (host panels served by the plugin). Mapping imported workbook styles into a document style group is recorded as planned in `registry/features/stylemap.yaml`.*

For a publishing platform this is **the most important property of the
whole plugin**, and it is a principle, not a feature:

- **Document styles are the single source of styling truth.** Typography,
  colors, fills, borders/strokes — core already owns these systems. Cell
  and table styling resolves to document paragraph/character/table styles
  plus the document color system (swatches), reached exclusively through
  the SDK.
- The grid's styling tools are **a front-end to document styles**:
  applying a style to a range references it; "new style from selection"
  creates a document style via the style-management capability (§2.2);
  redefining a document style restyles every sheet frame and every grid
  view instantly. Direct formatting lowers as constrained local overrides,
  visually flagged in the grid as off-style (a publishing affordance).
- A declared default mapping table per document covers imported XLSX
  styling: workbook fonts/fills/borders map into document styles on
  import (generated as a reviewable style group, not silently splattered
  as ten thousand local overrides).
- Number-format output (§9) is what gets typeset — the lowered text *is*
  the formatted value; format colors/conditions lower into style
  overrides.
- Conformance: `sheet.style.*` rows assert that grid view and lowered page
  content resolve to **identical visual style decisions** for the same
  range — the two surfaces may differ in pipeline, never in styling.

### 8.4 Charts

*Status note (2026-10-02): this section predates the implementation; see [ADR 016](adr/016-chart-engine-plotters-chartgeometry.md) (the chart engine and its lowering; the chart kinds built go beyond the set listed below).*

Charts are in scope: created and edited in sheets mode, bound to ranges,
live to recalculation. The architecture keeps them inside the two-surface
rule and the independence rule:

- **Chart model** (`sheet-chart`, T2): chart type, series/category range
  bindings, axes, legends, labels, styling via §8.3 (chart colors come
  from document swatches — publication-coherent charts are the point).
- **Page surface:** charts lower to **`paged.draw` vector content** —
  `paged.draw` is core, reached via SDK, explicitly permitted under §2.1.
  A chart on the page is document-native vector art, regenerated on
  recalculation, printable/exportable like anything Paged draws. Never via
  `plugin-image`, never rasterized.
- **Grid surface:** the same chart geometry generator feeds the sheets-mode
  view through the §8.1 surface.
- One geometry generator, two projections — mirroring the sheet itself.
  v1 chart-type set is deliberately publishing-curated: bar/column, line/
  area, pie/donut, scatter; registry rows `sheet.chart.*`; XLSX chart
  parts (`/charts/chart*.xml`) follow the fidelity ladder with
  `preserved` as the floor for unsupported types.

### 8.5 Frame operations: the content-space principle

*Status note (2026-10-02): this section predates the implementation; see [ADR 505](adr/505-native-table-and-edit-grid.md) (pointer events reach the plugin in frame-content coordinates, as required below; the `sheet.frame.transform.*` registry rows and the "straighten while editing" toggle do not exist).*

Paged itself owns a rich set of **frame operations** — scaling, rotation,
skew, repositioning, cropping — and sheet-frame rendering must reflect all
of them on both surfaces. The governing rule:

> **The plugin always produces content in frame-content coordinates.
> Frame transforms belong to core and are applied by core. The plugin
> never reimplements, anticipates, or compensates for them.**

**Page surface — transforms are free by construction.** Because lowered
frame content is *native Paged content*, every frame operation applies to
it exactly as it applies to any text frame: a rotated sheet frame renders
rotated tables with correctly rotated Parley-shaped text; a scaled frame
scales its rules and fills; charts (native `paged.draw` content) transform
identically. The plugin contributes nothing and can break nothing here —
this is the strongest single argument for the lowering model, now stated
as such.

**Resize vs transform — the pagination distinction.** Two superficially
similar frame operations have opposite consequences, and the spec fixes the
semantics:

| Operation | Effect on the plugin |
|---|---|
| **Content-box resize** (frame bounds change in content space) | re-pagination: the reflow notification (§2.2) fires, `sheet-lower` re-splits rows across the chain |
| **Pure transform** (scale/rotate/skew of the frame as an object) | **invisible to the plugin**: content space is unchanged; no re-pagination, no re-lowering; core renders the transformed result |

A frame scaled to 50% therefore shows the *same* rows smaller — it does
not fit twice as many. This matches text-frame behavior and InDesign user
expectations. The SDK reflow subscription must distinguish the two (the
notification carries content-box geometry, not display geometry) — added
to the §2.2 verification list.

**Sheets mode in transformed frames.** Double-clicking a rotated or scaled
sheet frame enters sheets mode *in place, under the transform* — the grid
renders within the frame's transformed appearance, exactly as Paged's
in-place text editing behaves in transformed frames. Under the preferred
D-10 contract this is nearly free: the plugin submits its grid scene in
frame-content coordinates and core composes the frame transform — one more
decisive argument for the Vello-scene contract, since a plugin-owned
axis-aligned canvas overlay cannot honestly render inside a rotated frame.
Two requirements follow:

- **Input mapping:** pointer/keyboard events delivered to the plugin in
  sheets mode must arrive **inverse-transformed into frame-content
  coordinates** (hit-testing cells in a rotated frame is core's transform
  math, not the plugin's). Added as an explicit clause of the
  activation-contract RFC (§2.2).
- **Usability escape hatch (companion-spec item):** for strongly
  transformed frames, sheets mode may offer an optional "straighten while
  editing" toggle — a temporary visual normalization owned by the *mode
  UI*, never a change to frame state.

**Conformance.** Registry rows `sheet.frame.transform.*` assert: identical
visual output of lowered content under core transforms vs reference
(rotation/scale/skew matrix corpus); pagination invariance under pure
transforms; re-pagination correctness under content-box resize; sheets-mode
hit-testing accuracy in transformed frames. These run on both surfaces —
the §8.3 cross-surface consistency principle extends to geometry.

---

## 9. Number formatting (`sheet-format`)

*Status note (2026-10-02): this section predates the implementation; see [ADR 509](adr/509-excel-first-dialect.md) (locales are data rows; en-US, de-DE, fr-FR, es-ES and it-IT ship).*

Quietly one of the largest sub-systems, specified as such:

- Full ECMA-376 number-format code engine: sections
  (positive;negative;zero;text), digit placeholders, thousands scaling,
  fractions, scientific, date/time tokens, elapsed-time brackets, locale
  and currency tokens, color/conditional brackets (colors lower into style
  overrides), text masks.
- Date systems: 1900 (with the leap-bug ruling) and 1904; serial↔calendar
  conversions are property-tested against the oracle across the full
  domain.
- 15-significant-digit display semantics and Excel's display rounding live
  here (the calc engine stays pure `f64`).
- Locale data (decimal/group separators, month/day names, default formats)
  ships as data tables; v1 locale set decided by corpus audit (D-8).

Every format-code feature is a registry row (`sheet.format.*`) with golden
corpora.

---

## 10. XLSX I/O (`sheet-xlsx`) — round-trip first

### 10.1 Strategy

Identical discipline to PSD in `plugin-image`: **full structural parse +
lossless preservation + faithful re-write first; semantic fidelity second,
feature by feature, in the registry.** The honest claim at every stage:
"Paged never destroys a workbook." XLSX makes this *easier* than PSD: OPC
is zip + XML, parts are addressable, and unknown content is preservable at
part, element, and attribute granularity.

### 10.2 Preservation invariant

*Status note (2026-10-02): this section predates the implementation; see [ADR 503](adr/503-xlsx-patched-not-regenerated.md) (untouched parts are re-emitted byte for byte; when a sheet is edited and re-encoded, unknown child elements of `<worksheet>` are kept but unknown attributes on rows and cells are not; `calcChain.xml` is always dropped).*

- Unknown **parts** (e.g. pivot caches, slicers, VBA `vbaProject.bin`,
  custom XML) are preserved byte-identical and re-emitted with their
  relationships intact.
- Unknown **elements/attributes** inside understood parts are preserved
  via a skeleton model that keeps unrecognized XML subtrees attached to
  their parents (namespace-aware), re-serialized in place.
- Understood content re-encodes from the model. Opening and saving a
  workbook without touching an unmodeled feature is lossless **by
  construction** — including VBA (preserved, never executed) and futures
  from newer Excel versions.
- Writer maintains content types, relationships, shared strings, and the
  calc chain so files stay valid for strict readers; `calcChain.xml` is
  regenerated or dropped per spec-permitted behavior (registry ruling).

### 10.3 Ecosystem reality check (A-0 audit items)

*Status note (2026-10-02): this section predates the implementation; see [ADR 500](adr/500-own-calculation-engine.md) and [ADR 503](adr/503-xlsx-patched-not-regenerated.md) (`sheet-xlsx` is built directly on `zip` and `quick-xml`; none of the crates named below is a dependency).*

`calamine` (reading) and `rust_xlsxwriter` (writing) are mature but
designed as one-way streets — neither targets lossless round-trip, so
`sheet-xlsx` is expected to be own-built on a low-level OPC/XML layer, with
both crates serving as *validation references* and possible internal
components where their scope fits. IronCalc's XLSX layer is evaluated in
the same audit. Decision recorded as D-3.

### 10.4 Fidelity ladder

Registry namespace `sheet.xlsx.*`, adapted tiers:
`parsed → preserved → calculated → rendered → round-trips`
("rendered" = lowers correctly into frames; "calculated" = formulas in
this part evaluate to oracle-matching values). Examples:
`sheet.xlsx.pivot` may sit at `preserved` permanently in v1;
`sheet.xlsx.conditional-formatting` targets `rendered` (lowers to style
overrides) in T2; `sheet.xlsx.formula.shared` targets `round-trips` in T1.

---

## 11. Feature inventory and tiering

*Status note (2026-10-02): this section predates the implementation; see `status.md` (what is shipped and what is not).*

| Tier | Content | Schedule |
|---|---|---|
| **T0 — spine** | model + parser + dependency graph + recalc scheduler; operators & coercion rules; ~60 core functions (math, logical, basic text, SUM/AVERAGE/COUNT family, IF family, basic lookup); number-format core (general, fixed, percent, basic date/time); XLSX parse + preservation + writer (zero-edit round-trip); basic lowering (single frame, no threading) | M0 |
| **T1 — the publishing product** | ~150 further functions curated for publishing workloads (full lookup incl. XLOOKUP/INDEX/MATCH, date/time, text, statistical core, financial core: NPV/IRR/PMT family); dynamic arrays/spill; full number-format engine; structured references & tables; **sheets mode: grid rendering + cell/formula editing + Operation wiring**; frame threading + pagination; document-style mapping incl. style-management capability | M1 |
| **T2 — depth where publishing needs it** | **charts** (model, geometry, `paged.draw` lowering, grid view, XLSX chart-part fidelity); conditional formatting (lowered to style overrides); remaining publishing-relevant functions; iterative calc (D-7); ODS read (OpenFormula path) if D-2 audit supports | M2 |
| **T3 — polish** | localization expansion; exact-decimal mode spike (D-6); grid UX depth per companion spec | M3+ |
| **T∞ — never (product decision, not deferral)** | pivot tables, data validation semantics, what-if/goal-seek, external workbook links, Power Query artifacts, VBA/XLM execution, ActiveX, RTD/web functions — **all preserved on round-trip, none interpreted** | excluded by design |

---

## 12. Conformance, testing, and the verification invariant

### 12.1 Identical environment to Paged core

*Status note (2026-10-02): this section predates the implementation; see [ADR 507](adr/507-golden-corpora-coverage-gate.md) (this repository's tests run under `cargo nextest` and vitest; it has no Playwright suite).*

Adopted verbatim, as in the `plugin-image` concept §12.1
(`plugin-image: docs/concept.md`): Playwright sole browser-side runner
(Chrome-only); `@feat:<id>` tags and `#[feature_test("id")]`;
`paged-results.json` reported to the project's internal feature registry;
public subset to `conformance.public.json` → `<ConformanceMatrix>` on
`docs.paged.media`; fingerprinted bug pipeline; registry → Projects strictly
one-way.

### 12.2 The 100% verification invariant

*Status note (2026-10-02): this section predates the implementation; see [ADR 507](adr/507-golden-corpora-coverage-gate.md) and, in plugin-sdk, [ADR 317](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/317-registry-driven-dispatch.md) (dispatch is generated from the registry for functions only; XLSX part handlers and lowering rules are ordinary code whose registry rows are checked by the coverage gate).*

1. **Registry-driven dispatch** (functions §7, XLSX part handlers §10,
   lowering rules §8): no registry row → no dispatch entry → unreachable
   by construction.
2. **Coverage gate:** CI verifies, per row, tests for every claimed tier;
   below 100% fails the build.
3. **Tier-regression gate:** forward-only status via green runs; a
   regressed tier auto-files a fingerprinted issue and blocks release.

### 12.3 Registry integration

*Status note (2026-10-02): this section predates the implementation; the registry is in this repository, under `registry/functions/` and `registry/features/`; see [ADR 507](adr/507-golden-corpora-coverage-gate.md).*

- `registry/features/sheet.*.yaml` — namespaces: `sheet.fn.*`,
  `sheet.calc.*` (graph/scheduler/volatility/rewrite rules),
  `sheet.format.*`, `sheet.xlsx.*`, `sheet.lower.*`, `sheet.grid.*`
  (sheets-mode rendering/virtualization/mode contract), `sheet.style.*`
  (document-style mapping + cross-surface consistency),
  `sheet.frame.transform.*` (frame-operation correctness, §8.5),
  `sheet.chart.*`, `sheet.plugin.*`.
- Taxonomies: functions
  `implemented → parity-oracle → coercion-complete → round-trips`;
  XLSX per §10.4; lowering
  `implemented → paginates → reflow-stable → round-trips`.

Sample row:

```yaml
# registry/features/sheet.fn.lookup.yaml
id: sheet.fn.lookup.xlookup
title: XLOOKUP (modes, search orders, if_not_found)
family: lookup
volatility: none
status: planned
provenance:
  - "ECMA-376 SpreadsheetML formula grammar"
  - "Microsoft function documentation (public)"
  - "LibreOffice oracle corpus: corpus/fn/xlookup/*"
tests:
  rust: ["sheet-conformance/tests/fn_lookup.rs::xlookup"]
  corpus: ["fn-corpus/xlookup.golden.csv"]
```

### 12.4 Oracles and harnesses (`sheet-conformance`, test-only)

*Status note (2026-10-02): this section predates the implementation; see [ADR 507](adr/507-golden-corpora-coverage-gate.md) (the LibreOffice oracle skeleton was replaced by the Excel oracle in `sheet-conformance/tests/excel_oracle.rs`; IronCalc is not used).*

- **LibreOffice Calc headless** (CI container): primary differential
  oracle — workbooks generated per function/feature, evaluated, values
  extracted, diffed.
- **Golden corpora:** expected-value tables (incl. published Excel
  results) for functions, format codes, and date serials.
- **IronCalc** (if A-0 confirms license): third oracle; three-way
  disagreement is high-value signal.
- **Disagreement protocol:** where oracles differ (coercion edges, date
  edge cases, financial-function day-count conventions), the registry row
  records behaviors, the chosen convention (Excel-compat by default), and
  rationale.
- **Property tests:** recalc-order independence (§6.2); structural-edit
  formula rewriting (random edits → references stay semantically
  anchored); pagination convergence (§8.2); preservation
  (random workbook → zero-edit round-trip → structural equality).
- **Determinism:** the engine is CPU/`f64` and bit-stable by the same
  rules as `plugin-image`'s reference path (no fast-math, fixed reduction
  orders); goldens are byte-comparable — no tolerance machinery needed
  outside oracle diffs.

### 12.5 Performance gates (CI-enforced)

*Status note (2026-10-02): this section predates the implementation; the workflows in `.github/workflows/` run no performance benchmark, so these targets are not enforced; see `status.md`.*

| Benchmark | Target |
|---|---|
| Cold open: 100k-cell workbook, parse + full calc (wasm, 4 workers) | < 1.5 s |
| Incremental: edit 1 cell, 10k-cell dependent chain | < 50 ms |
| Typing-latency Gesture (speculative parse + bounded preview) | < 16 ms |
| Lower + paginate 10k rows across a 20-frame chain | < 250 ms to settle |
| Sheets-mode grid: scroll a 1M-row sparse sheet | frame-rate (< 16 ms/frame) via virtualization |
| Sheets-mode grid: enter mode on double-click | < 150 ms to interactive |
| Zero-edit round-trip, 50 MB XLSX | streaming; peak heap < 256 MB |
| Recalc-order property suite | bit-identical across orders |

---

## 13. Milestones

*Status note (2026-10-02): this section predates the implementation; see `status.md` (what is shipped) and [ADR 507](adr/507-golden-corpora-coverage-gate.md) (the LibreOffice oracle harness named under M0 is not built).*

**M0 — Spine + round-trip + first lowering.**
Phase 0 (serial): **A-0 audit** — SDK surface for table content, frame
chains, owned content (resolve §2.2 to "covered"/RFC); evaluate
IronCalc/calamine/rust_xlsxwriter (D-3) and confirm licenses; rule D-2
dialect default; freeze `sheet-core` types, function signature, AST, the
XLSX skeleton model. Then: parser; dependency graph + scheduler; T0
functions via registry-driven dispatch; number-format core; XLSX parse +
preservation + writer, zero-edit round-trip green over the initial corpus;
single-frame lowering; LibreOffice oracle harness + coverage gate live.
*Exit:* T0 rows green at claimed
tiers; round-trip property green; plugin loads via SDK with zero core
changes; coverage gate at 100%.

**M1 — The publishing product.**
T1 function set; dynamic arrays; full number-format engine; structured
references; **sheets mode** (activation contract, vector grid on the SDK
surface, cell/formula editing through Operations); **frame threading +
pagination + document-style mapping** (incl. style-management capability);
XLSX `calculated/rendered` tiers across T1 scope; viewer-bundle packaging.
*Exit:* perf gates green incl. grid gates; a real annual-report-style
document (live sheet edited in sheets mode, threaded across pages,
document-styled) produced end-to-end as the demo artifact.

**M2 — Depth where publishing needs it.**
**Charts** (model, geometry generator, `paged.draw` lowering, grid view,
XLSX chart-part fidelity ladder); conditional formatting lowered; T2
function scope; D-7 iterative calc; ODS/OpenFormula path per audit; D-5
edit-interception polish ("edit the sheet behind this frame" → sheets
mode).

**M3 — Breadth.**
T3 scope; exact-decimal spike (D-6); localization expansion; external-link
reads.

---

## 14. Open decisions

*Status note (2026-10-02): this section predates the implementation; what was decided and built is recorded in [ADR 505](adr/505-native-table-and-edit-grid.md) (D-1, D-10), [ADR 509](adr/509-excel-first-dialect.md) (D-2, D-8), [ADR 500](adr/500-own-calculation-engine.md) and [ADR 503](adr/503-xlsx-patched-not-regenerated.md) (D-3), [ADR 016](adr/016-chart-engine-plotters-chartgeometry.md) (D-4), [ADR 501](adr/501-f64-numbers.md) (D-6) and [ADR 502](adr/502-recalculation-and-spill.md) (D-7). For D-5, a double-click enters sheets mode, and the lowered content is not locked.*

| ID | Decision | Default leaning | Resolve by |
|---|---|---|---|
| D-1 | Lowering model: compiled-to-native content vs live scene-provider contract | **compiled-to-native (ruled as default in §8)** — revisit only if the §2.2 table-model RFC fails | M0 |
| D-2 | Dialect priority: Excel-first with OpenFormula mapping vs OpenFormula-first | Excel-first (users' files are the product reality); OpenFormula kept mappable | M0 |
| D-3 | IronCalc/calamine/rust_xlsxwriter: build-on, internal component, or oracle-only | oracle/validation-reference unless A-0 shows decisive fit + clean license | M0 |
| D-4 | ~~Charts out of v1~~ **Re-ruled: charts are in scope (T2/M2)** — created in sheets mode, lowered through `paged.draw` (core SDK, permitted under §2.1); never via another plugin, never rasterized | publishing-curated type set first (bar/column, line/area, pie/donut, scatter) | M2 |
| D-5 | Manual edits on compiled frame content: locked + edit-interception vs round-trip-to-cells | locked + interception in v1 — interception now concretely means **enter sheets mode** | M2 |
| D-6 | Numeric core: Excel-compat `f64` vs exact-decimal mode | `f64` v1; trait boundary keeps decimal mode open | M0 (boundary), M3 (spike) |
| D-7 | Iterative/circular calculation policy and defaults | off by default; Excel-style settings when enabled | M2 |
| D-8 | v1 locale set for `sheet-format` | corpus-driven; minimum en/de | M1 |
| D-10 | Sheets-mode rendering surface contract: Vello scene/display-list submission vs plugin-owned `GPUCanvasContext` overlay (§8.1) | **Vello-scene strongly preferred** (one renderer, core text shaping in the grid, visual consistency by construction, and §8.5: correct in-place editing inside transformed frames — an overlay cannot honestly render in a rotated frame); ruled by what the plugin platform can offer | M0 |

