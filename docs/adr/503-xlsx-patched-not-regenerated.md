# ADR 503 — XLSX is patched, never regenerated

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `sheet-xlsx`, and `load_xlsx` / `save_xlsx` in `sheet-js/src/core.rs`

## Context

An imported workbook can be exported again, and it holds content the engine never models:
pivot caches, VBA projects, custom XML, drawings, themes. `CLAUDE.md:108-113` states the
invariant the XLSX layer is built around: "Paged never destroys a workbook." Unknown parts
stay byte-identical, unknown subtrees in known parts are retained, and untouched
understood parts are written back as their original bytes.

A writer that builds a worksheet from the model loses what the model does not hold.
`sheet-xlsx/src/preserve.rs:43-48` says so for the one place where that happens: when a
worksheet is encoded again, "the cell grid is rebuilt, so any unknown *child element of
`<worksheet>`* would be lost", so those children are captured at parse time.

The layer is built on `zip` and `quick-xml`, not on an XLSX reader or writer crate. The
repository does not record why.

## Decision

On open, every part of the package is kept, in its original order, as its decompressed
bytes. On save a part is written back as those bytes, with two exceptions: a worksheet
that holds an edited cell is encoded again from the model, and the calc chain is dropped.

- A part is `PartEntry::Opaque` or `Modeled { kind, raw, dirty }`. Six kinds are modelled:
  `Workbook`, `WorkbookRels`, `Worksheet`, `SharedStrings`, `Styles`, `ContentTypes`.
- Charts, comments, external links, conditional formats, data validations and freeze panes
  are parsed into read-only models; nothing is written from those models.
- `sheet-xlsx` does not parse formulas. It exposes the raw text per cell in
  `formula_texts`; `sheet-js` parses it on load and prints edited formulas back before save.
- `save_xlsx` marks dirty only the sheets that hold a cell in the `edited` set of `SheetSession`.
  A dirty sheet is written as: captured children that stood before `<sheetData>`,
  `<dimension>`, `<cols>`, `<sheetData>`, `<mergeCells>`, captured children that stood after.
- `xl/calcChain.xml` is dropped with its content-type override and workbook relationship:
  "Dropping avoids maintaining a stale chain after edits" (`registry/features/xlsx.yaml:103`).
- The claim is identity of the decompressed bytes of each part, not of the zip file.
- A file with the CFB magic number is refused as `LegacyBinaryXls` before the zip reader runs.

## Evidence

- `sheet-xlsx/src/opc.rs:35-41`, `:53-82`, `:221-234` — order kept, per-part identity
  only; the two part variants and the six kinds; the container sniff
- `sheet-xlsx/src/write.rs:88-143`, `:217-306` — the save loop (stored bytes unless the
  part is a dirty worksheet or references the calc chain); `encode_worksheet`
- `sheet-xlsx/src/parts/worksheet.rs:83`, `:122-130` — the four modelled worksheet children;
  every other child is captured with a before/after anchor
- `sheet-js/src/core.rs:496-527` — edited cells printed back, their sheets marked dirty
- `sheet-xlsx/src/lib.rs:118-176` — the read-only models and why they are never re-emitted
- `sheet-conformance/tests/xlsx_roundtrip.rs:236-253`, `:261-304`, `:351-395` — unknown
  parts, a pivot cache, and zero-edit identity over six fixtures (`:48-55`)

## Alternatives considered

Keeping `calcChain.xml` up to date was rejected by the ruling quoted above. No other
alternative is recorded in the repository.

## Consequences

An untouched sheet and every unknown part survive a save byte for byte. An edited sheet keeps cell values and formula text,
merges, column widths, the custom heights of rows that hold a cell, and the captured worksheet children. It loses unknown
attributes on `<worksheet>`, `<row>` and `<c>` and unknown elements inside `<sheetData>` (`sheet-xlsx/src/preserve.rs:50-58`),
and a row that has a custom height and no cell is not written back (`sheet-xlsx/src/write.rs:265-285`). A cell's style index
is not kept as read: the writer emits the model's `StyleId` (`sheet-xlsx/src/write.rs:319-323`), the id of an interned
`CellStyle` (`sheet-xlsx/src/parts/styles.rs:299-318`, `sheet-core/src/intern.rs:57-65`), while `styles.xml` is written back
unchanged. The two numbers agree only when entry 0 of `cellXfs` equals the default style and no two entries intern to the same
`CellStyle`. The round-trip test compares cells by value only (`sheet-conformance/tests/xlsx_roundtrip.rs:486-492`).

Never written from the model, so they leave the file as they came in: styles, the workbook part and shared strings
(`sheet-xlsx/src/write.rs:134-136`), conditional formats, data validations and comments. Every literal text cell of an edited sheet, loaded or new,
is written as an inline string (`:339-349`). A chart authored in the editor is not written to the workbook (`sheet-js/src/core.rs:1453-1455`). The
six fixtures of the identity test are written by the repository's own generator, `corpus/xlsx-corpus/generate.py`.

Comments that the code contradicts:

- `sheet-xlsx/src/write.rs:50-51` says shared strings are rebuilt when strings were added;
  `:77-80` says they are not, and the code never rebuilds them.
- `sheet-xlsx/src/parts/worksheet.rs:346-350` says a shared-formula member receives the master's text unchanged
  and that `sheet-js` re-derives the references. The member is given the master's text (`:358-369`), `load_xlsx`
  parses that text as it stands (`sheet-js/src/core.rs:446-453`), and no other code handles shared formulas.

## Related

- [ADR 500](500-own-calculation-engine.md) — no third-party engine, reader or writer is a dependency
- [ADR 504](504-publishing-first-scope.md) — the content that is preserved and never interpreted
- [ADR 506](506-workbook-in-a-container-part.md) — where the workbook bytes are stored
- [ADR 017](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/017-importer-exporter-door-shape.md)
  — the importer and exporter doors that carry the bytes
