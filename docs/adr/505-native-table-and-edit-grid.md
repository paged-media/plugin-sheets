# ADR 505 — A sheet on the page is a native table; editing happens in a scene-layer grid

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `sheet-lower`, `sheet-grid`, `packages/sheet-host-model` (`lower-to-table.ts`, `lower-to-mutations.ts`, `binding.ts`, `grid.ts`), `packages/sheet-bundle` (`lower.ts`, `activate.ts`, `session.ts`)

## Context

The plugin holds a workbook in a Rust engine. It has to put a range on a page and let the user
edit the workbook from there.

For the first, `CLAUDE.md:11-16` states the rule: the page surface is compiled to native
document content. `packages/sheet-host-model/src/lower-to-table.ts:19-24` names what that
gives: a real table that is selectable, printable, round-trips through IDML and transforms
with its frame. Before the host's wire carried `insertTable`, the only lane was tab-separated
text in a text frame with drawn rules (`packages/sheet-host-model/src/lower-to-mutations.ts:26-34`).

For the second, the code does not edit the table's cells in the document. It paints a grid
over the frame while a modal edit context is active. The repository does not record why.

## Decision

A range is placed on a page as ordinary document content: a text frame holding a native
table, written through `host.document.mutate`. Editing uses a second, temporary surface: a
grid computed in Rust for the visible cells and submitted as a scene layer on the frame.

- **Lowering.** The engine lowers `(sheet, range)` to `LoweredContent`: formatted cell text,
  column widths, row heights, merges, grid rules. It is pure and uses cached formula values.
- **Writing.** `lowerSelectionToFrame` writes in three steps: one batch of `insertTextFrame`
  and `setPluginMetadata` (the binding); `insertTable` into the new frame's story, with column
  widths measured through `host.text.measureString`; then one `insertText` mutation per
  non-empty cell and one batch of merges, cell fills and edge strokes.
- **Binding.** The metadata under `x-paged:media.paged.sheet` is what makes a frame a sheet
  frame: `{ v: 1, data: { sheet, range, contentVersion } }`, with the worksheet name in `sheet`.
- **Fallback.** The tab-text lane stays. It runs when a caller passes `lane: "tab-text"`, which
  no command or panel does; when the host rejects `insertTable`, the tab-separated text is
  poured into the same story.
- **Entering.** Behind `host.supports`, object type `sheetFrame` matches an item whose metadata
  parses as a binding; double-click enters edit context `sheet` (`toolIds: []`, Workbook panel).
- **Editing.** The session asks the engine for a `GridScene` sized to the frame's content box,
  translates it with `gridSceneToSceneLayer` and submits it to the frame. A pointer event is
  resolved with `hitCell` against the last scene. Keys fill a buffer drawn by submitting the
  layer again; Enter writes one `engine.setCell`. Undo and redo are answered from a session
  journal of cell input texts. Exit clears the journal and the layer.

## Evidence

- `sheet-lower/src/lib.rs:33-41`, `:377-384` — pure lowering from cached values; `lower_range` passes `NoStyles`
- `packages/sheet-bundle/src/lower.ts:298-415` — the three steps; `:192-236` text per cell, then one decor batch; `:323-325`, `:380-401` the two ways into the tab-text lane
- `packages/sheet-host-model/src/lower-to-table.ts:98-115` — the `insertTable` arguments; `packages/sheet-host-model/src/binding.ts:41`, `:51-59` — the key and the binding data
- `packages/sheet-bundle/src/activate.ts:280-287`, `:288-349` — object type, edit context and its hooks, including `onExit`
- `packages/sheet-bundle/src/session.ts:498-502`, `:622-644`, `:900-938` — the scene channel behind `supports("rendering.sceneLayer@1")`, the submit, sizing to the frame; `:524-567`, `:993-1017` — the journal, commit, hide
- `sheet-grid/src/lib.rs:35-41`; `packages/sheet-host-model/src/grid.ts:575`, `:696` — the windowed scene; `gridSceneToSceneLayer`, `hitCell`
- `packages/sheet-bundle/src/lower.ts:626-680`, `:692-732`; `packages/sheet-bundle/src/index.ts:87-93` — the chain functions and their only reference outside the tests

## Alternatives considered

The tab-text lane was the only lane until commit `88060a2` (2026-06-09) and is kept as the fallback. Two shapes
of the native lane were replaced: cell text and decor in one batch, which the engine rejected whole
(`packages/sheet-bundle/src/lower.ts:182-191`, commit `9560573`); and finding the new frame's story by hit test,
which reports no story for an empty frame (`packages/sheet-bundle/src/lower.ts:280-286`, commit `7ca75f8`). The
SVG grid panel that came first is still registered (`packages/sheet-bundle/src/activate.ts:113-121`).

## Consequences

A lowering is several mutations; `packages/sheet-bundle/src/lower.ts:182-191` records
that this gave up single-undo atomicity. Parts of the design are not built:

- Nothing lowers again after an edit. `onExit` clears the journal and the layer only
  (`packages/sheet-bundle/src/activate.ts:345-348`). `lowerSelectionToFrame` always inserts a new frame
  and the bundle emits no mutation that deletes content, so no path updates a table already on the page.
  `contentVersion` is always written as 0 (`packages/sheet-bundle/src/lower.ts:319-321`) and nothing
  compares it. Cell edits are not persisted either ([ADR 506](506-workbook-in-a-container-part.md)).
- The binding's `sheet` and `range` are not read back: `parseBinding` has one caller, the `matches`
  predicate. The in-frame grid shows the session's active sheet from its first row and column
  (`packages/sheet-bundle/src/session.ts:727-750`, `:934`), whatever range the frame was lowered
  from. The chart lowering writes the same envelope (`packages/sheet-bundle/src/lower-chart.ts:104`).
- `get_range_lowered` resolves no visual styles (`sheet-js/src/core.rs:1104`, `:1114-1122`)
  and the grid scene carries the default style only (`sheet-grid/src/lib.rs:415-417`,
  `:538-540`). Workbook fonts, fills and text colours reach neither surface.
- A user cannot reach pagination across a frame chain: only tests call `lowerPaginatedToChain`
  and `subscribeChainReflow`, though `CLAUDE.md:37-38` lists live pagination as landed.
- Comments contradict the code: `packages/sheet-bundle/src/lower.ts:32-34` calls step three one batch;
  `packages/sheet-bundle/src/lower.ts:42-48` says the story is found by hit test, true only of the chain path (`:497-504`);
  `packages/sheet-bundle/src/session.ts:281-282` and `packages/sheet-bundle/src/activate.ts:339-340` refer to a re-lowered batch on exit.

## Related

- [ADR 012](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/012-k1-modal-session-undo-coalescing.md) — modal-session undo. Its Tier 2, one document step on exit that lowers the session's net change again, is not implemented here; the journal is an array of input texts replayed through `setCell`.
- [ADR 013](https://github.com/paged-media/core/blob/main/docs/adr/013-in-frame-scenelayer.md), [ADR 024](https://github.com/paged-media/editor/blob/main/docs/adr/024-context-sensitivity-is-a-core-concept.md), [ADR 316](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/316-native-content-and-baking.md), [ADR 310](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/310-one-write-door.md) — the scene-layer channel; a context that declares its tools and panels; plugin content as native document content; the one write door
- [ADR 506](506-workbook-in-a-container-part.md), [ADR 508](508-content-addressed-swatches.md) — where the workbook is stored; the swatches a lowering mints
