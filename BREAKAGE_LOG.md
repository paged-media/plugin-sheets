# BREAKAGE_LOG — paged.sheet vs. the plugin surface

Every place the de-facto plugin API (`@paged-media/plugin-api` /
`plugin-sdk`) fell short of what paged.sheet needs. **This log is the
API-v1 punch list** and the live resolution of the spec's §2.2 gap
table — entries drain as host/core work lands. Several rows are the
*same* RFCs plugin-image files (workers, OPFS, importer registration):
independence between plugins, convergence on the platform.

Format: `S-NN · date · area · status`.

---

- **S-01 · 2026-06-07 · shell / activation · OPEN** — no owned-frame
  activation / edit-context registry: `contribute.editContext` is
  RESERVED and throws `PluginApiNotImplemented` (plugin-api `host.ts`).
  Spec §8.0's canonical interaction — double-click a sheet frame →
  sheets mode — cannot be wired in T0; the flow is panel + command
  driven instead. Same RFC paged.draw tracks as B-02. Resolution
  direction: edit-context registry + modal editing-session contract
  (enter/exit, dirty-state, Esc/commit semantics) **including
  pointer/keyboard events delivered inverse-transformed into
  frame-content coordinates** for transformed frames (spec §8.5).

- **S-02 · 2026-06-07 · rendering surface · OPEN** — no SDK vector
  rendering surface for the sheets-mode grid (spec §8.1, D-10).
  `capabilities.rendering` offers only `overlay` (tool-preview
  rect/polyline), `hitTest`, and a reserved `sceneLayer`. Preferred
  contract: Vello scene / display-list submission in frame-content
  coordinates (core applies frame transforms — §8.5 makes an
  axis-aligned canvas overlay dishonest inside rotated frames).
  Blocks the entire sheets-mode grid; T1 gate, RFC to file jointly
  with plugin-image's GPU-surface row (I-01).

- **S-03 · 2026-06-07 · engine ops · OPEN (degradation active)** — no
  native table-creation Mutation: the wire has table ops
  (`insertTableRow`, `setCellSpan`, `setRowHeight`, cell styles, and
  `insertText` with a `TextCellAddr` qualifier) but they all require a
  **pre-existing** `tableId` — there is no `insertTable`. T0 page
  lowering therefore runs the spec §2.2 degradation: tab-aligned text
  in a text frame + drawn rules (`insertLine`), batched. Sub-gaps:
  (a) `insertText` keys off `storyId`, which exists only after the
  `insertTextFrame` applies → two-phase lower (frame batch, then text
  pour); (b) re-lower = `deleteFrame` + recreate (new element id;
  selection loss, two undo steps). Resolution: native table content
  model RFC (spec §8.2) — lowering upgrades from tab-text to real
  tables when it lands.

- **S-04 · 2026-06-07 · styles · OPEN** — style management is
  write-only: `createParagraphStyle` / `createCellStyle` /
  `createTableStyle` / `setStyleProperty` mutations exist, but there is
  no style **enumeration/read** door. The §8.3 document-coherent
  styling principle (grid styling tools as a front-end to document
  styles, "new style from selection", imported-workbook style mapping
  as a reviewable group) needs read+write. T0 lowers with literal
  local formatting only. Resolution: style-management capability RFC.

- **S-05 · 2026-06-07 · frames / threading · OPEN** — no frame-chain
  topology read for owned frames and no reflow/layout-change
  subscription. `linkFrames` exists (write) but chain reads, overflow
  notification, and the §8.5 resize-vs-transform distinction (the
  notification must carry content-box geometry; pure transforms must
  NOT fire it) are missing. Blocks threading + pagination (spec §8.2,
  the killer feature). T1 gate.

- **S-06 · 2026-06-07 · importer/exporter · OPEN** — no
  importer/exporter registration capability: `.xlsx` cannot register
  as a document open/import handler. T0 imports via an in-panel
  `<input type="file">` (see S-11). Shared RFC with plugin-image
  (I-05-shaped). Resolution: importer/exporter registration capability.

- **S-07 · 2026-06-07 · workers · OPEN** — no worker-spawn /
  SharedArrayBuffer capability (COOP/COEP guarantees). The calc
  engine's parallel recalc (spec §6.2: independent dirty subgraphs on
  a rayon/wasm-bindgen-rayon pool) cannot ship; T0 recalc is
  single-threaded (the `eval_layer` seam in sheet-calc is the
  parallelism point when this lands). Shared RFC with plugin-image.

- **S-08 · 2026-06-07 · storage · OPEN** — no OPFS / large-blob quota
  capability. `host.storage` is a localStorage-backed JSON KV —
  unfit for multi-MB workbook bytes. **T0: workbook bytes are
  in-memory only; reload requires re-import** (the panel says so —
  honesty rule). The small frame-binding envelope persists via
  `setPluginMetadata` and round-trips IDML. Shared RFC with
  plugin-image. Resolution: storage capability with quota declaration.

- **S-09 · 2026-06-07 · owned content · OPEN** — no owned-content
  attribute / edit-interception hook (spec D-5). Lowered frame content
  is plain document content; a user can hand-edit it with no "edit the
  sheet behind this frame" interception (which per spec v0.2 concretely
  means: enter sheets mode — itself blocked on S-01). T2 gate.

- **S-10 · 2026-06-07 · wasm packaging · OPEN (by design in T0)** —
  `loadBundleWasm` instantiates a RAW module (host-owned memory, only
  caller-passed imports, no glue). A wasm-bindgen artifact (`sheet-js`)
  cannot load that way — it needs its `__wbindgen_*` imports, its own
  exported memory, and the generated JS glue. T0 DECLARES the artifact
  under `capabilities.wasm[]` (governance + the plugin-cli 8 MiB size
  gate) but loads it via the wasm-bindgen `--target web` glue in the
  bundle realm (the canvas-wasm pattern). Resolution: a host loader
  door for wasm-bindgen-shaped modules, or ratify the glue path as the
  contract.

- **S-11 · 2026-06-07 · shell / file input · OPEN** — no host
  file-picker surface. T0 uses an in-panel `<input type="file"
  accept=".xlsx">` (the React expert-leaf escape hatch; the panel owns
  its own DOM, like paged.web's source panel). Clean path: a
  `host.shell.pickFile()` door or the S-06 importer registration.
