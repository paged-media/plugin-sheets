# ADR 506 — The workbook is stored whole in a container part

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `packages/sheet-bundle` (`workbook-part.ts`, `session.ts`, `activate.ts`, `manifest.json`)

## Context

A sheet frame on a page carries only a small binding in plugin metadata: a worksheet name, a
range and a version number ([ADR 505](505-native-table-and-edit-grid.md)). The workbook it was
lowered from lives in the plugin's engine, in memory.

The first persistent home for the workbook was the host's blob store (commit `fb0bef3`,
2026-06-10). `packages/sheet-bundle/src/workbook-part.ts:21-27` states what was wrong with
that: the blob store is per-browser storage that does not travel with the document, so the
workbook had to be imported again after opening the document on another machine. The host
door `host.parts` writes bytes into the document container under the plugin's own namespace.

The stored form is the imported `.xlsx` file, not a serialisation of the engine's model.
The repository does not record why.

## Decision

The plugin's persistent state is the imported XLSX file itself, byte for byte, written as a
part of the document container and, as a second copy, to the per-browser blob store.

- On import, `persistWorkbook` writes two parts, `workbook.xlsx` (the bytes handed to the
  import) and `workbook.name` (the display name as UTF-8), then writes the same bytes to
  `host.blob` under the key `workbook` and the name to `host.storage`.
- The part paths are relative; the host prefixes them with `paged/<plugin id>/`, which gives
  `paged/media.paged.sheet/workbook.xlsx`. A part reaches the file on the next document save.
- The part write is skipped when `host.supports("storage.parts@1")` is false, and the blob
  write when `host.supports("storage.blob@1")` is false. A failure of either is logged and
  does not fail the import.
- On activation the session restores: the part first; if there is none, the blob, and after a
  successful load from the blob it writes the part once so the workbook travels from then on.
- The store is a singleton, not per frame: the last workbook imported. The part belongs to the
  document; the blob copy is keyed per plugin and is not scoped to a document (commit `fb0bef3`).
- The manifest declares the part type as
  `{ "type": "workbook", "role": "source", "format": "xlsx", "linkable": false }`.

## Evidence

- `packages/sheet-bundle/src/workbook-part.ts:19-33` — the rationale; `:41-42` the part names; `:50-58` the write and its `supports` guard; `:62-71` the read
- `packages/sheet-bundle/src/session.ts:66-70`, `:760-773` — the blob keys; `persistWorkbook` writes the part, then the blob
- `packages/sheet-bundle/src/session.ts:814`, `:823-825` — the only call of `persistWorkbook`, on the import path
- `packages/sheet-bundle/src/session.ts:827-862` — `restore`: part, then blob, then the one-time copy into the part
- `packages/sheet-bundle/src/activate.ts:103` — `void session.restore()` at activation
- `packages/sheet-bundle/manifest.json:18-21`, `:63-65` — blob storage with `quotaBytes` 33554432; the declared part type
- `plugin-sdk: packages/plugin-sdk/src/host-impl.ts:2665`; `plugin-sdk: packages/plugin-api/src/host.ts:1282-1284` — the namespace prefix `paged/${manifest.id}/`; a written part is persisted on the next document save
- `packages/sheet-host-model/src/binding.ts:51-59` — the binding data: `sheet`, `range`, `contentVersion`

## Alternatives considered

In memory only, the state before commit `fb0bef3`. Blob store only, the state between `fb0bef3`
and commit `30f6102` (2026-06-18); the blob is kept as a local cache and as the read source for
documents saved before the part existed (`packages/sheet-bundle/src/workbook-part.ts:26-33`).

## Consequences

`restore` runs once, when the bundle is activated (`packages/sheet-bundle/src/activate.ts:103`); nothing calls it again when a
document is opened later. It reads the part first, so a document that is loaded at that moment and carries the part restores its
workbook in a browser whose blob store is empty; the comment at `packages/sheet-bundle/src/session.ts:828-830` states this as
the intent, and no test in this repository runs it against a real host. When no part is found, the blob fallback loads whatever
was last imported in that browser (`:839-851`); the blob is keyed per plugin, not per document. If no document is loaded, the
part read answers nothing and a part write fails and is logged (`plugin-sdk: packages/plugin-sdk/src/host-impl.ts:2703-2717`).

Only an import persists. The following change the engine in memory and are written to
neither the part nor the blob:

- cell edits, from the in-frame grid or the grid panel, and sort, replace-all and paste;
- charts authored in the plugin;
- a workbook seeded from a data provider (`sourceFromDataset`,
  `packages/sheet-bundle/src/session.ts:1564-1646`), which replaces the engine and stores nothing.

After a reload the session restores the file as it was imported. The edited workbook is written out only by the XLSX exporter,
which calls `engine.saveXlsx` (`packages/sheet-bundle/src/activate.ts:418-426`, `packages/sheet-bundle/src/session.ts:1669-1682`);
what that writer carries is recorded in [ADR 503](503-xlsx-patched-not-regenerated.md).

The binding names a worksheet and a range but no workbook. A second import overwrites the
part, and frames lowered from the earlier workbook keep their bindings.

Comments contradict the code. `packages/sheet-bundle/src/session.ts:19-21`, `packages/sheet-bundle/src/activate.ts:24-25`,
`packages/sheet-host-model/src/binding.ts:25-26` and `README.md:73-74` still say the workbook is not persisted;
`packages/sheet-bundle/src/activate.ts:100-102` and `CLAUDE.md:45-46` name only the blob store.

## Related

- [ADR 118](https://github.com/paged-media/core/blob/main/docs/adr/118-paged-file-is-a-valid-idml-package.md), [ADR 021](https://github.com/paged-media/core/blob/main/docs/adr/021-paged-native-document-model-idml-as-format.md) — the container the part is written into; content parts in their own formats
- [ADR 311](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/311-plugin-state-under-own-id.md), [ADR 305](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/305-doors-always-present.md) — plugin state under the plugin's own id; a door that is present but unbacked, reported by `supports()`
- [ADR 503](503-xlsx-patched-not-regenerated.md), [ADR 505](505-native-table-and-edit-grid.md) — the XLSX writer behind the exporter; the frame and its binding
