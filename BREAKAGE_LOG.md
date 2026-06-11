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
entries. **Platform Wave 2D (2026-06-10):** S-05 (frame-chain read +
content-box reflow event) RESOLVED by core protocol **v38** —
`host.document.frameChain(storyId)` + `DocumentChangeEvent.reflow`; live
multi-frame pagination now threads a tall range across the host chain and
re-paginates on a content-box resize (never on a pure transform, §8.5).
**Platform Wave 3 — IO slice (2026-06-10):** S-06 (importer/exporter
registration) + S-11 (host file picker) RESOLVED — pure SDK + editor
surface, NO wire/protocol change: `ContributionSurface.importer()/
exporter()` + `ShellSurface.pickFile()`; `.xlsx` now opens via the
plugin (File/Open + drag-drop route to the importer) and the workbook
panel picks through the host dialog. **Platform Wave 3b — persistence
(2026-06-10):** S-08 (OPFS/blob storage) RESOLVED — `host.blob`
(OPFS-backed, quota-gated); the workbook persists + restores on reload.
K-3 (S-07 workers) DEFERRED — no consumer until the engine threads.
**Platform Wave 4 — C-1 in-frame scene layer (2026-06-10):** S-02
PARTIALLY RESOLVED — the in-frame VECTOR surface shipped (`host
.contribute.sceneLayer()`, published core v0.39.0 paths/fills + v0.40.0
text) and the sheet CONSUMES it (`session.showGridInFrame` renders the
grid in the lowered frame). Residual = the editing/interactivity half
(K-1 modal session + coord-inverted input). Remaining: S-07 (workers),
S-02 editing residual + S-01 (sheets-MODE), S-09 (owned content), S-04
named-style write.
**Platform Wave 5 — data-provider consumer (2026-06-11):** S-15 RESOLVED
— `host.dataProviders` (the neutral cross-plugin registry; plugin-data
D-09 / sheets S-15) is LIVE and the sheet CONSUMES it: a `dataProviders:
{consume:["dataset"]}` capability + a datasets panel +
`session.discoverDatasets()`/`sourceFromDataset()` source a sheet from a
governed dataset (header + records seeded from a resolved snapshot; no
network on the sheet's side, §1.1). §2.1 intact — only the neutral
contract, never paged.data directly. Remaining: S-07 (workers), S-02
editing residual + S-01 (sheets-MODE), S-09 (owned content), S-04
named-style write, S-14 (range clipboard).

---

## §2.2 row dispositions

The spec's §2.2 gap-analysis table, resolved row-by-row:

- Read document structure / styles / frames — **COVERED**
  (`capabilities.document.read: "broad"`).
- Frame-activation hook (double-click owned frame → sheets mode) —
  **GAP** → S-01 (the registration door now ships; the residual is the
  modal-session lifecycle + frame-content coordinate inversion).
- Editing surface for sheets mode (vector rendering target) — **PARTIAL**
  → S-02 (the in-frame VECTOR render shipped via `host.contribute
  .sceneLayer()`, consumed by `showGridInFrame`; the EDITING half needs
  K-1 modal session + coord-inverted input; joint plugin-image I-01).
- Commit table/text/rule content inside owned frames — **COVERED**
  (S-03 RESOLVED — native `InsertTable`, protocol v37; lowering emits a
  real `<Table>`).
- Document style read AND write — read **COVERED** (the style
  *enumeration* read is served by `host.document.collection(...)` over the
  `paragraphStyles`/`characterStyles`/`objectStyles`/`cellStyles`/
  `tableStyles` collections — C-3); the named-style document-group
  write/redefine half stays a **GAP** → S-04 (not yet consumed here — no
  style read is wired in the pagination path).
- Frame ownership & lock (owned-content attribute + edit interception) —
  **GAP** → S-09.
- Frame linking / threading topology read — **COVERED** (S-05 RESOLVED —
  `host.document.frameChain(storyId)` reads the ordered chain, protocol
  v38; live multi-frame pagination threads a tall range across it).
- Reflow notification (content-box resize vs pure transform) — **COVERED**
  (S-05 RESOLVED — `DocumentChangeEvent.reflow` carries content-box
  geometry ONLY on a resize, never a pure transform, §8.5; the chain lower
  re-paginates on it and ignores transform-only changes).
- `paged.draw` access for chart lowering (core SDK, §8.4) — **COVERED**
  (S-12 RESOLVED — `insertPath`/`insertLine`/`insertOval` confirmed
  sufficient; charts lower as native vector content).
- Asset placement in cells (images via the standard asset mechanism) —
  **COVERED** (core asset surface).
- Worker spawn + SharedArrayBuffer — **GAP** → S-07 (joint I-02; deferred
  — no consumer until the engine threads).
- OPFS quota — **COVERED** (S-08 RESOLVED — `host.blob` OPFS/quota store;
  the workbook persists + restores on reload; joint I-03).
- Register importer/exporter (XLSX opens via the plugin) — **COVERED**
  (S-06 RESOLVED — `contribute.importer()/exporter()`; `.xlsx` opens via
  the plugin through File/Open + drag-drop; joint I-05).

Sheets-discovered, beyond the §2.2 table: the wasm-bindgen loader path
(S-10 — **RESOLVED**, ratified), the host file picker (S-11 —
**RESOLVED**, `host.shell.pickFile`), font metrics (S-13 —
**RESOLVED**, `measure_text`), and range clipboard (S-14).

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

- **S-02 · 2026-06-07 · rendering surface · PARTIALLY RESOLVED
  (2026-06-10)** — the in-frame VECTOR rendering surface SHIPPED (C-1,
  the platform-RFI flagship): `capabilities.rendering` ∋ `sceneLayer` +
  `host.contribute.sceneLayer().submit(elementId, layer)` — a plugin
  submits a `SceneLayer` (filled/stroked paths + single-line text in
  frame-content coords) and CORE composes it inside the frame, applying
  the frame's `ItemTransform` + content-box clip (§8.5), through the same
  `DisplayList → Vello/tiny-skia` lanes (published core v0.39.0 paths/
  fills + v0.40.0 text). The sheet now CONSUMES it: `session
  .showGridInFrame()` lowers the windowed `GridScene` →
  `gridSceneToSceneLayer` (gridlines + cell fills + cell values) →
  submit, sized to the lowered frame's content box. So the §8.1 grid
  RENDERS in-frame on the canvas (not just the interim SVG panel —
  retained as the secondary surface; one geometry, two surfaces).
  **RESIDUAL (the editing/interactivity half, still OPEN):** modal
  sheets-MODE entry + pointer/keyboard events delivered inverse-
  transformed into frame-content coords (K-1 / S-01 residual) — without
  it the in-frame grid is a render, not an editable surface. Plus the
  in-frame text v1 caveats (default font, left-aligned, upright glyphs).
  The raw-`GPUTexture`/`GPUDevice` stages (image viewport, I-01 + I-07)
  are separate, still open. **Joint with plugin-image I-01/I-07.**

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

- **S-05 · 2026-06-07 · frames / threading · RESOLVED (2026-06-10)** —
  core protocol **v38** added the two reads this gap named, and the whole
  path is now wired (Wave 2D, RFI C-2):
  (a) `host.document.frameChain(storyId) -> FrameChainLink[]`
  (`{frameId, next, overflow}`) reads the ordered thread topology
  (tail-overflow flagged);
  (b) `DocumentChangeEvent.reflow` (`{frameId, contentBox:[t,l,b,r]}`)
  fires ONLY on a content-box resize — never on a pure transform
  (move/scale/rotate is display-only, §8.5), the resize-vs-transform
  distinction this entry required, carrying **content-box geometry**.
  The Rust pagination engine (`sheet-lower/src/paginate.rs`) is now
  exposed over wasm — `SheetSession::paginate` + the `SheetEngine`
  wasm shim `paginate(sheet, range, frames, opts)` (`sheet-js/src/core.rs`,
  `sheet-js/src/lib.rs`), the `engine.ts` facade `paginate`, the pure
  per-page table translator `pageTableMutations`
  (`packages/sheet-host-model/src/lower-to-table.ts`), and the
  `lowerPaginatedToChain` / `subscribeChainReflow` chain flow
  (`packages/sheet-bundle/src/lower.ts`): it reads the real chain, resolves
  each frame's content box via `host.document.elementGeometry`, paginates
  into those boxes (all threading math in Rust), lowers each `Page` into ITS
  frame's story as a native `<Table>`, and re-paginates on a chain-frame
  reflow (ignoring transform-only changes). The single-frame
  `lowerSelectionToFrame` (S-03) stays unchanged. Tests:
  `sheet_js_paginate_*` (js_surface), the real-wasm boot smoke
  (engine-real), `pageTableMutations` (host-model), and the fake-host chain
  + reflow flow (lower-flow). Historical framing retained below.

  *(superseded)* no frame-chain
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

- **S-06 · 2026-06-07 · importer/exporter · RESOLVED (2026-06-10)** —
  the SDK gained the document-IO contributions (plugin-platform RFI
  Wave 3, the IO slice; NO wire/protocol change — pure SDK + editor
  surface). `ContributionSurface.importer()/exporter()`,
  `ImporterContribution`/`ExporterContribution` (+ `ImportRequest`/
  `ExportResult`), `PluginContributions.importers[]/exporters[]`
  (id-only, mirroring `commands`) + schema + CLI namespace check;
  `HOST_FEATURES` += `contribute.importer@1`/`contribute.exporter@1`.
  The editor owns the registries (`registries/document-io.ts` —
  `resolve(name,mime)` by extension-then-MIME, `acceptExtensions()`):
  the open + drag-drop flow consults the importer registry by extension
  BEFORE the default IDML load, so a `.xlsx` routes its bytes to the
  plugin's `import()` instead of replacing the document; exporters
  surface as one-click outputs in the Export Center (the host owns
  blob→download). paged.sheet registers both
  (`packages/sheet-bundle/src/activate.ts`): the `.xlsx` importer feeds
  `session.import` (then opens the panel), the exporter pulls
  `session.saveWorkbook → engine.saveXlsx` (preservation-first re-emit).
  Joint with plugin-image's document-type-handler (I-05) — the same
  contribution capability serves both. Tests: `activate.spec.ts`
  (registration + manifest match), `import-xlsx.spec.ts` (pick → import).

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

- **S-08 · 2026-06-07 · storage · RESOLVED (2026-06-10)** — the SDK
  gained `host.blob` (plugin-platform RFI Wave 3b; pure SDK + editor, NO
  wire/protocol change, NO publish). `BlobSurface`
  (write/read/delete/keys/usage) is an OPFS-backed, per-plugin,
  quota-bounded BINARY store distinct from the KV `host.storage`, gated on
  `capabilities.storage: { blob, quotaBytes? }`; the adapter owns
  namespacing + the quota (the stricter of the 64 MiB host default and the
  manifest's request). The editor backs it with OPFS
  (`apps/canvas/src/plugin-blob-store.ts`, injected as `blobStore`).
  paged.sheet now PERSISTS the imported workbook
  (`session.persistWorkbook` → `host.blob.write("workbook", bytes)`, name
  in KV) and RESTORES it at activate (`session.restore`, called from
  `activate.ts` — a cheap no-op when nothing is persisted; the engine
  boots only when there are bytes). The panel notice flips honestly
  ("saved to this browser's local store and restored on reload" when
  `supports("storage.blob@1")`, else the in-memory wording). Declared
  `capabilities.storage: { blob: true, quotaBytes: 33554432 }` (32 MiB).
  Per-plugin keyed: the LAST imported workbook is the one restored (not
  document-scoped in T0). The 64 KiB `setPluginMetadata` binding envelope
  is unchanged — it still carries the frame binding, round-tripping IDML.
  Joint with plugin-image (I-03). Tests: `session-restore.spec.ts`
  (no-op guards) + SDK `blob-store.spec.ts` (door/quota/gate). **K-3
  (workers, S-07) stays OPEN** — deferred until the engine actually
  threads (no speculative SDK surface).

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

- **S-11 · 2026-06-07 · shell / file picker · RESOLVED (2026-06-10)** —
  `ShellSurface.pickFile(options?) -> Promise<readonly PickedFile[]>` is
  LIVE (plugin-platform RFI Wave 3 IO; no wire/protocol change). It
  returns the chosen files' BYTES (read at the host boundary — a DOM
  `File` never crosses the contract, so the bundle stays isolate-ready);
  `[]` on cancel OR when no picker is wired (the honest no-picker door —
  probe `supports("shell.pickFile@1")`). The editor backs it with a
  transient `<input type="file">` (`apps/canvas/src/shell-file-picker.ts`,
  injected as the `shell.pickFile` host option in `main.tsx`). paged.sheet
  now imports through it: the workbook panel shows a "Choose .xlsx…"
  button (`workbook-panel.tsx`) and the `importXlsx` command calls
  `pickAndImport` (`import-xlsx.ts`), both routing to
  `host.shell.pickFile({ accept: [".xlsx"] })`. The in-panel `<input>` is
  RETAINED as the honest fallback ONLY when `supports("shell.pickFile@1")`
  is false (headless / a host predating the door). Tests:
  `import-xlsx.spec.ts` (pick + cancel + fallback paths).

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
  wired, else an estimate). `lower.ts` `measureColumnWidths` sizes
  native-table columns from the door's advances (the §8.3
  cross-surface-width requirement). RESIDUAL (editor consumption, not a
  platform gap): the editor still wires `PagedEditor.text.measure` to the
  canvas-wasm `CanvasWorker.measureText` over the render-worker RPC — a
  small follow-on; until then the door returns the honest estimate and
  `supports("text.measure@1")` is false. Historical framing retained
  below.

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

- **S-15 · 2026-06-10 · data-provider consumer · RESOLVED (2026-06-11)**
  — source a sheet from a governed external dataset (plugin-data §7.1;
  the "sheet over a governed query → print" composition). Needed a
  `dataProviders:{consume}` capability + `host.dataProviders.discover/
  get/onDidChange`. Consumer-only: pull a resolved `ProviderRecordSet`
  snapshot (no network on the sheet's side — `paged.data` owns the fetch
  + §11 consent; the sheet receives cached data, §1.1-consistent, NOT an
  external-workbook link). **The SDK door is LIVE** (plugin-api
  `DataProvidersSurface`; the editor injects a shared registry) and the
  consumer is BUILT here: the manifest declares
  `capabilities.dataProviders.consume: ["dataset"]`; the bundle adds a
  **datasets panel** (`media.paged.sheet.panel.datasets`) + a
  **`sheetFromDataset`** command; `WorkbookSession.discoverDatasets()`
  lists the governed datasets and `sourceFromDataset(id)` pulls the
  snapshot, boots a fresh EMPTY workbook, and seeds sheet 0 (row 0 =
  schema field names; rows 1.. = the column-major records). The linked
  `(providerId, revision)` is remembered + `onDidChange` marks the sheet
  STALE on a new revision (no auto-refetch — re-sourcing is explicit,
  §1.1). ISOLATION (§2.1) intact: the consumer touches ONLY
  `host.dataProviders` — never `paged.data` / any sibling, at build,
  runtime, or via a side channel (the contract-import guard stays clean);
  it learns a provider's `id`/`category`/`schema`, never the backing
  plugin. Graceful absence: no registry wired ⇒ `discover()` is empty +
  the panel shows an honest empty state. **Joint with plugin-data D-09**
  (the provider side built there; shared core contract). RFCs:
  `thoughts/docs/paged/plugin-sheets/rfc-data-provider-consumer.md`
  (consumer) + `…/plugin-data/rfc-data-provider.md` (provider/contract).
  **CONTRACT GAP (follow-up, NOT a blocker):** the contract leaves the
  per-cell value ENCODING under-specified — `columns[c][r]` may be a
  PLAIN JS value (`string|number|boolean|null`) OR the data engine's
  tagged `{ t, v }` form. The consumer's `cellToString` handles BOTH
  defensively; a contract amendment should standardize one encoding.

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
signal that these belong in plugin-api v1. The sheets-specific rows are
paged.sheet's own to carry: S-04 styles (read served by `collection()`;
named-style write half open), S-09 owned content, S-14 clipboard remain
open; S-03 tables, S-05 threading (Wave 2D), S-12 charts, S-13 metrics
are RESOLVED, and S-01 keeps only its sheets-mode residual.
