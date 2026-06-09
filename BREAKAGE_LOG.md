# BREAKAGE_LOG — paged.sheet vs. the plugin surface

Every place the published plugin surface (`@paged-media/plugin-api` v0.2
/ `plugin-sdk`) falls short of what paged.sheet needs. This log is BOTH
the API-v1 punch list AND the live resolution of the spec's §2.2 gap
table (`thoughts/docs/paged/plugin-sheets/base-idea.md`) — entries drain
as host/core work lands. Several rows are the *same* RFCs plugin-image
files (GPU surface, workers, OPFS, importer registration, wasm budgets):
independence between plugins, convergence on the platform — see the
joint-RFC summary at the foot of this file.

Format: `S-NN · date · area · status`. Verified against the published
SDK + the repo's own code on 2026-06-08 (M1 phase B+C, commit `9906aef`).
**Platform Wave 1 (2026-06-09):** S-03 (native `InsertTable`), S-13
(`measure_text`), S-10 (wasm-bindgen loader ratified), S-12 (paged.draw
verified) RESOLVED by core protocol **v37** + the SDK door — see those
entries. Waves 2–4 (S-05 threading, S-06/S-07/S-08/S-11 IO+workers+OPFS,
S-02/S-01 in-frame sheets mode) remain.

---

## §2.2 row dispositions

The spec's §2.2 gap-analysis table, resolved row-by-row:

- Read document structure / styles / frames — **COVERED**
  (`capabilities.document.read: "broad"`).
- Frame-activation hook (double-click owned frame → sheets mode) —
  **GAP** → S-01 (the registration door now ships; the residual is the
  modal-session lifecycle + frame-content coordinate inversion).
- Editing surface for sheets mode (vector rendering target) — **GAP** →
  S-02 (joint with plugin-image I-01).
- Commit table/text/rule content inside owned frames — **COVERED**
  (S-03 RESOLVED — native `InsertTable`, protocol v37; lowering emits a
  real `<Table>`).
- Document style read AND write — read **COVERED**; the write/enumerate
  half is a **GAP** → S-04.
- Frame ownership & lock (owned-content attribute + edit interception) —
  **GAP** → S-09.
- Frame linking / threading topology read — **GAP** → S-05.
- Reflow notification (content-box resize vs pure transform) — **GAP** →
  S-05.
- `paged.draw` access for chart lowering (core SDK, §8.4) — **COVERED**
  (S-12 RESOLVED — `insertPath`/`insertLine`/`insertOval` confirmed
  sufficient; charts lower as native vector content).
- Asset placement in cells (images via the standard asset mechanism) —
  **COVERED** (core asset surface).
- Worker spawn + SharedArrayBuffer — **GAP** → S-07 (joint I-02).
- OPFS quota — **GAP** → S-08 (joint I-03).
- Register importer/exporter (XLSX opens via the plugin) — **GAP** →
  S-06 (joint I-05).

Sheets-discovered, beyond the §2.2 table: the wasm-bindgen loader path
(S-10 — **RESOLVED**, ratified), the host file picker (S-11), font
metrics (S-13 — **RESOLVED**, `measure_text`), and range clipboard
(S-14).

---

## Entries

- **S-01 · 2026-06-07 · shell / activation · PARTIALLY RESOLVED
  (2026-06-08)** — the edit-context / object-type **registration door
  shipped** and no longer throws: `contribute.editContext` /
  `contribute.objectType` are implemented (`plugin-sdk` `host-impl.ts`
  `editContext()`/`objectType()`, both in `HOST_FEATURES` as
  `contribute.editContext@1` / `contribute.objectType@1`; the door is a
  tracked registration even when no shell registry is wired — "the door
  no longer throws"). This is the same gap paged.draw tracked as **B-02
  (resolved)**. The original entry — "`contribute.editContext` is
  RESERVED and throws `PluginApiNotImplemented`" — is therefore
  **superseded**. Residual gaps that keep S-01 open for paged.sheet:
  (a) the **modal editing-session contract** beyond `onEnter`/`onExit` —
  dirty-state, Esc/commit, and the seamless-undo boundary sheets mode
  needs (spec §8.0); (b) **pointer/keyboard events delivered
  inverse-transformed into frame-content coordinates** for transformed
  (rotated/scaled/skewed) frames (spec §8.5 — the real remaining
  contract; "the plugin never reimplements, anticipates, or compensates
  for frame transforms"); (c) paged.sheet has not yet declared
  `contributes.editContexts[]` — sheets-mode entry is gated on the grid
  surface (S-02) regardless, so T0 stays panel + command driven.
  Resolution direction: confirm/extend the modal-session lifecycle +
  the §8.5 content-coordinate event-delivery clause. T1 gate.

- **S-02 · 2026-06-07 · rendering surface · OPEN** — no SDK vector
  rendering surface for the sheets-mode grid (spec §8.1, D-10).
  `capabilities.rendering` offers only `overlay` (tool-preview
  rect/polyline/cubic-path), `hitTest`, and a reserved `sceneLayer`
  (`manifest.ts` rendering enum). Preferred contract: Vello scene /
  display-list submission in frame-content coordinates (core applies
  frame transforms — §8.5 makes an axis-aligned canvas overlay
  dishonest inside rotated frames); fallback: a plugin-owned
  `GPUCanvasContext` overlay (inferior for transformed frames). Blocks
  the entire in-frame sheets-mode grid; T1 gate. **Joint RFC with
  plugin-image's GPU-surface row (I-01) and the WebGPU-reach question
  (I-07).** *Updated 2026-06-08:* an interim SVG **panel** grid landed
  (M1 phase B+C — `sheet-grid` GridScene IR + `grid-panel.tsx`). It is a
  separate TS-side tool, NOT the reserved in-frame SDK surface, and does
  not fake it — `capabilities.rendering` is still `["hitTest"]` only.
  The SDK gate is unchanged.

- **S-03 · 2026-06-07 · engine ops · RESOLVED (2026-06-09)** — core
  protocol **v37** added `Mutation::InsertTable { story_id, rows, cols,
  header_rows, footer_rows, column_widths, row_heights }` (translate →
  `Operation::InsertNode { parent: Story, NodeSpec::Table }`, createdId =
  the minted tableId). Page lowering now emits a **native `<Table>`** —
  `packages/sheet-host-model/src/lower-to-table.ts` (`tableInsertOp` +
  `tableCellOps`) + the three-phase `lower.ts` flow (frame → table →
  per-cell `insertText` with the `TextCellAddr` qualifier). The tab-text
  + drawn-rules degradation (`lower-to-mutations.ts`) is retired to the
  old-engine fallback. RESIDUAL (next increment, NOT a regression — the
  tab-text path placed neither): per-cell FILL background + BORDERS need
  a `tableCell` `ElementId` kind so `cellFillColor`/`cell*EdgeStroke*`
  (real PropertyPaths) can be `setElementProperty`-addressed; tracked
  forward. Historical degradation note retained below for provenance.

  *(superseded)* no native table-creation Mutation: the wire has table ops
  (`insertTableRow`, `insertTableColumn`, `setCellSpan`, `setRowHeight`,
  `setColumnWidth`, header/footer-row ops, cell styles, and `insertText`
  with an optional `cell: TextCellAddr` qualifier) but they all require a
  **pre-existing** `tableId` — confirmed there is **no `insertTable`** in
  the Mutation union (only `insertTableRow` / `insertTableColumn`). T0
  page lowering therefore runs the spec §2.2 degradation: tab-aligned
  text in a text frame + drawn rules (`insertLine`), batched. Sub-gaps:
  (a) `insertText` keys off `storyId`, which exists only after the
  `insertTextFrame` applies → two-phase lower (frame batch, then text
  pour — `packages/sheet-host-model/src/lower-to-mutations.ts`);
  (b) re-lower = `deleteFrame` + recreate (new element id; selection
  loss, two undo steps). Resolution: native table content model RFC
  (spec §8.2) — lowering upgrades from tab-text to real tables when it
  lands. *Updated 2026-06-08:* M1 added the **internal** table model
  (structured references + the `tableN.xml` XLSX part), but **page
  lowering still degrades to tab-text + rules** — the degradation is
  unchanged pending `insertTable`.

- **S-04 · 2026-06-07 · styles · OPEN (partial)** — style management is
  write-only: `createParagraphStyle` / `createCellStyle` /
  `createTableStyle` / `setStyleProperty` mutations exist, but there is
  no style **enumeration / read** door (`DocumentSurface` has no
  styles-collection read). The §8.3 document-coherent styling principle
  ("document styles are the single source of styling truth"; grid
  styling tools as a front-end to document styles, "new style from
  selection", imported-workbook style mapping as a reviewable group)
  needs read+write. Resolution: style-management capability RFC.
  *Updated 2026-06-08:* M1 landed an IR-v2 style map with
  **character-level** override emission (`sheet-lower/src/style.rs`;
  `styleProps()` emits font style/size/face/color, `blockedFacets()`
  reports fill/border as BLOCKED rather than faking them). The
  named-style document-group path — "new style from selection",
  redefining a doc style to restyle every frame — still needs the
  style read door. Direct local formatting stays honest in the meantime.

- **S-05 · 2026-06-07 · frames / threading · OPEN** — no frame-chain
  topology read for owned frames and no reflow/layout-change
  subscription. `linkFrames` exists (write); chain reads, overflow
  notification, and the §8.5 resize-vs-transform distinction are
  missing. The reflow notification must carry **content-box geometry,
  not display geometry** — a pure transform (scale/rotate/skew) must NOT
  fire it; only a content-box resize re-paginates (`DocumentChangeEvent`
  carries only `{kind, pageIds}` today — no geometry deltas).
  Blocks live threading + pagination across a frame chain (spec §8.2,
  the killer feature). T1 gate. *Updated 2026-06-08:* the Rust
  **pagination engine** landed (`sheet-lower/src/paginate.rs` — greedy
  row packing over a *caller-supplied* chain, repeated headers,
  continued markers, keep-together, tall-row handling, convergence
  property). It paginates against a chain handed in by the caller; the
  SDK gate — reading the host's actual frame-chain topology and
  receiving content-box reflow notifications — is unchanged.

- **S-06 · 2026-06-07 · importer/exporter · OPEN** — no
  importer/exporter registration capability: `ContributionSurface`
  offers tool/panel/schemaPanel/command/keybinding/overlay/editContext/
  objectType but no `importer()`/`exporter()`, and `PluginContributions`
  has no `importers`/`exporters` field — so `.xlsx` cannot register as a
  document open/import handler. T0 imports via an in-panel
  `<input type="file">` (see S-11). **Joint RFC with plugin-image's
  document-type-handler row (I-05)** — both want the same contribution
  capability ("XLSX opens via the plugin" / "PSD opens via the plugin").
  Resolution: importer/exporter registration capability.

- **S-07 · 2026-06-07 · workers · OPEN** — no worker-spawn /
  SharedArrayBuffer capability (`BundleHost` has no `spawn`/`worker`;
  `docs/wasm-packaging.md`: "SharedArrayBuffer / threads are **OFF in
  v1** … the loader never sets `shared: true`"). The calc engine's
  parallel recalc (spec §6.2: independent dirty subgraphs on a
  rayon/wasm-bindgen-rayon pool) cannot ship; T0 recalc is
  single-threaded (`sheet-calc` `recalc_dirty` is a sequential topo-order
  loop — the parallelism seam when this lands). **Joint RFC with
  plugin-image (I-02)** — both need a worker capability with COOP/COEP
  guarantees; the editor is already cross-origin isolated (plugin-image
  A-0 audit), so the platform can host it — the gap is the contract.

- **S-08 · 2026-06-07 · storage · OPEN** — no OPFS / large-blob quota
  capability. `host.storage` is a localStorage-backed JSON KV
  (`StorageSurface` = get/set/delete/keys) — unfit for multi-MB workbook
  bytes. **T0: workbook bytes are in-memory only; reload requires
  re-import** (the panel says so — honesty rule;
  `packages/sheet-bundle/src/session.ts` + `workbook-panel.tsx`). The
  small frame-binding envelope persists via `setPluginMetadata`
  (namespace `x-paged:media.paged.sheet`) and round-trips IDML — but
  note that door caps at **64 KiB** per element, so only the binding
  envelope fits, never workbook bytes. **Joint RFC with plugin-image
  (I-03).** Resolution: storage capability with a quota declaration +
  an OPFS/blob store distinct from the KV door.

- **S-09 · 2026-06-07 · owned content · OPEN** — no owned-content
  attribute / edit-interception hook (spec D-5). Lowered frame content
  is plain document content; a user can hand-edit it with no "edit the
  sheet behind this frame" interception (which per spec v0.2 concretely
  means: enter sheets mode). *Updated 2026-06-08:* with
  `contribute.objectType` now shipping (S-01), the "edit → enter sheets
  mode" path is partially expressible via an object-type registration;
  the residual narrows to (a) the **owned-content attribute** stamped on
  compiled content and (b) the **edit-interception delivery** (intercept
  a manual edit attempt on owned content → invoke sheets-mode entry).
  T2 gate.

- **S-10 · 2026-06-07 · wasm packaging · RESOLVED (2026-06-09, by
  ratification)** — the two-loader split is now the documented v1
  contract (`plugin-sdk/docs/wasm-packaging.md` "Two loaders, ratified"):
  raw modules load via `host.loadBundleWasm`; **wasm-bindgen** modules
  (sheet-js, the canvas-wasm pattern) load via the bundle's own
  `--target web` glue in the bundle realm (`engine.ts` `bootEngine`),
  still declared under `capabilities.wasm[]` for the budget gate. No
  host-side wasm-bindgen loader is needed; the glue path is the answer.
  (A host-owned wasm-bindgen instantiation — to share a `GPUDevice` — is
  the separate I-01/I-07 GPU-surface RFC, not this door.) Historical
  framing retained below.

  *(superseded)* `loadBundleWasm` instantiates a RAW module (host-owned memory, only
  caller-passed imports, no glue — `docs/wasm-packaging.md`). A
  wasm-bindgen artifact (`sheet-js`) cannot load that way — it needs its
  `__wbindgen_*` imports, its own exported memory, and the generated JS
  glue. T0 DECLARES the artifact under `capabilities.wasm[]` (governance
  + the plugin-cli **8 MiB** per-artifact size gate) but loads it via the
  wasm-bindgen `--target web` glue in the bundle realm (the canvas-wasm
  pattern — `packages/sheet-bundle/src/engine.ts` `bootEngine`, NOT
  `host.loadBundleWasm`; build by `scripts/build-wasm.sh`). **Joint RFC
  with plugin-image (I-07)** — both ship a wasm-bindgen engine this way
  and both must measure against the 8 MiB ceiling. Resolution: a host
  loader door for wasm-bindgen-shaped modules, or ratify the glue path
  as the contract.

- **S-11 · 2026-06-07 · shell / file input · OPEN** — no host
  file-picker surface (`ShellSurface` = openPanel/closePanel only). T0
  uses an in-panel `<input type="file" accept=".xlsx">` (the React
  expert-leaf escape hatch; the panel owns its own DOM, like paged.web's
  source panel — `workbook-panel.tsx`). Clean path: a
  `host.shell.pickFile()` door or the S-06 importer registration.

- **S-12 · 2026-06-08 · charts / paged.draw · RESOLVED (2026-06-09,
  verified)** — confirmed the published wire carries `insertPath` /
  `insertLine` / `insertOval` routed through `host.document.mutate()`, so
  the M2 chart lowering (`packages/sheet-host-model/src/chart.ts` →
  `chartGeometryToMutations`) submits chart geometry as native
  `paged.draw` vector content (paths + frame fills via document swatches)
  with NO new platform surface — paged.draw is reached as a CORE wire
  surface, never as a plugin (§2.1). The "verify scripting-level
  sufficiency" question is closed: the path is real and shipped.
  Historical framing retained below.

  *(superseded)* charts
  are in scope (spec D-4 re-ruled; T2/§8.4): one pure geometry generator
  feeds both the sheets-mode grid view and page lowering, the latter
  emitting native vector content through **`paged.draw`** (a CORE SDK
  surface, allowed under the §2.1 zero-inter-plugin rule — paged.draw is
  not a plugin). The §2.2 "verify scripting-level sufficiency" question
  is unconfirmed: it is not established that the published SDK scripting
  surface can submit arbitrary chart geometry (paths/fills/text) to
  paged.draw from this bundle. T2 gate. Resolution: confirm the
  paged.draw geometry-submission surface (or file the clause that closes
  it) before M2 chart lowering.

- **S-13 · 2026-06-08 · font metrics · RESOLVED (2026-06-09)** — core
  added a `measure_text(family, style, text, size_pt) -> {advance,
  ascender, descender}` query on the canvas-wasm surface (wrapping
  `paged-text::shape_run`; no wire/protocol change — a read), surfaced as
  `host.text.measureString` on `BundleHost` (a read door, no capability
  gate; `supports("text.measure@1")` reports whether a real shaper is
  wired, else an estimate). `lower.ts` `measureColumnWidths` now sizes
  native-table columns from real advances (the §8.3 cross-surface-width
  requirement). Historical framing retained below.

  *(superseded)* no text-measurement
  door. The lowerer must size grid columns and make the sheets-mode grid
  view and the lowered page content resolve to the **same** widths
  (a §8.3 cross-surface-consistency requirement) — both need the
  document's font **advance widths / metrics**. The asset surface serves
  font **bytes** only (`@font-face`-style), with no measurement API, so
  column auto-fit and width-fidelity rely on bundle-side estimates today.
  T1 gate (sharpens once S-02's surface, if Vello-scene-shaped, brings
  core text shaping with it — which would also supply metrics).
  Resolution: a font-measurement door, or fold metrics into the S-02
  rendering-surface contract.

- **S-14 · 2026-06-08 · clipboard / ranges · OPEN (forward)** — the
  manifest declares `clipboard: "none"` and there is no tabular
  read/write contract: the sheets-mode grid's range copy/paste (cells,
  rows, columns, with values + formats) has no `host.clipboard` surface
  to land on. Not a T0 blocker (T0 is panel + lowering, no live grid
  editing of ranges); a forward-looking row owned by the editing-UX
  companion spec (`plugin-sheet-editing-ux`). Resolution: a clipboard
  capability carrying a tabular/range payload shape.

---

## Convergent joint RFCs (with plugin-image)

Five rows here are the *same* platform RFCs paged.image filed
independently — the platform should design each once, for both plugins:

| paged.sheet | paged.image | Joint RFC |
|---|---|---|
| S-02 | I-01 (+ I-07) | GPU / scene rendering surface + WebGPU reach from a bundle |
| S-06 | I-05 | importer/exporter (document-type handler) registration |
| S-07 | I-02 | worker spawn + SharedArrayBuffer (COOP/COEP) |
| S-08 | I-03 | OPFS / large-blob storage capability with quota |
| S-10 | I-07 | wasm-bindgen loader door + the 8 MiB artifact budget |

Two plugins, filed independently, converging on the same surface is the
signal that these belong in plugin-api v1. The sheets-specific rows
(S-01 residual, S-03 tables, S-04 styles, S-05 threading, S-09 owned
content, S-12 charts, S-13 metrics, S-14 clipboard) are paged.sheet's
own to carry.
