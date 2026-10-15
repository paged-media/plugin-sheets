# Status

What `paged.sheet` ships and what it does not, read from the code at commit `1a8ce82` (`main`,
2026-10-13): `@paged-media/sheet` 0.1.0-canary.20, published 2026-10-13, with
`@paged-media/plugin-api` and `@paged-media/plugin-sdk` `>=0.2.45-canary.0` as peers. The tests
run against `@paged-media/canvas-wasm` 0.71.8. How the parts fit is in
[`architecture.md`](architecture.md). The analysis behind the 2026-10-04 round of work, and what
changed against it, is in [`design/analysis-2026-10-04.md`](design/analysis-2026-10-04.md). What
each published canary added is under [Releases](#releases).

## Shipped

- **Open and keep a workbook.** An `.xlsx` file reaches the plugin through the host's importer,
  the "Import workbook (.xlsx)" command or the Workbook panel; CSV and TSV import as a one-sheet
  workbook, typed by locale; "New workbook" starts a blank one. The bytes are stored in the
  document when the host supports container parts, and in the browser's per-plugin store when it
  has one. Every committed edit re-saves the workbook after a short quiet period, and
  deactivation flushes, so a reload restores the edited workbook. Where the host announces saves
  (`document.onWillSave`), the workbook is written before the save takes the document's bytes.
  Where the host reports opens (`document.onDidOpen`), a reopened document restores its workbook
  there; an older host is followed through the editor's load broadcast.
- **Calculation.** 263 registered functions, all implemented, among them `LET`, `LAMBDA` and its
  helpers; dynamic arrays that spill and nest; Excel tables with structured references;
  incremental recalculation with an interval index for range dependencies; dependents of
  volatile cells recalculate with them; iterative calculation when the workbook's `calcPr`
  asks for it; `NOW` and `TODAY` follow the host clock. Number formats render in five display
  locales. Expected values are recorded from Excel and replayed in CI: 1,470 of 1,485 function
  cases and 148 of 164 number-format cases agree, every disagreement a recorded ruling.
  Recalculating real workbooks reproduces Excel's saved value in 93.6% of comparable formula
  cells (56,533 of 60,419 over 201 workbooks): shared formulas, defined names of every kind,
  structured references (including `[#Headers],[#Data]` and `[#Data],[#Totals]`), whole-column
  and whole-row references, omitted arguments, legacy array formulas and element-wise array
  arguments evaluate as Excel does. A number literal beyond the double-precision range is a
  formula error, as in Excel.
- **Workbook panel.** File and sheet controls on top (open, new, and sheets: add, rename,
  delete), then collapsible sections: Place on page (open by default), Rows and columns, Format
  and layout (borders, merge, column width, row height, freeze), Names, Sort range (formula cells
  move with their row), Cell styles, Charts, Functions and Workbook contents. Number format,
  font, fill, alignment, wrap and iterative calculation are rows of the Sheet properties panel;
  the Format and layout section points there, and shows those controls itself only on a host
  that does not render schema panels. Deleting a sheet, rows or columns, starting a blank
  workbook over an open one and filling a workbook from a dataset over an open one ask for a
  second click.
- **Sheet properties panel.** A schema panel of property rows that the host renders with its
  `PropertyField` over `host.objects` (`src/panels/properties-panel.ts`): Cell (the grid's
  active cell), Selected range (values and inputs edited row by row at the range's shape,
  formula fill, and the format rows across the range), Sheet, Workbook, Named range and Chart
  (each of Sheet, Named range and Chart picked from a list first). A chart's series are a list
  of items (name, values, categories, colour) that can be added, removed and reordered. Every
  writable row of every kind is a field (a spec holds the panel to the schemas); each commit is
  one undo step, and each field offers the host's "Bind to data…".
- **Grid panel and in-frame grid.** Range selection by click, drag and shift; arrows, Tab, F2,
  Delete and scrolling in the frame; a cell editor with function completion; copy and paste of
  ranges with relative references re-addressed; the fill handle with number, date, weekday and
  month series; Fill Down / Fill Right; find and replace in the sheet (Find all, Replace all,
  Match case, Entire cell; the "Find & replace…" command opens the grid). The grid's viewport
  follows the panel's width, and copy, paste, fill and edit refusals show in the panel.
- **Undo.** In the edit context, every committed cell edit is one step, and so is each bulk
  operation (sort, replace-all, paste, fill, clear) and each format change (style, borders,
  merge, sizes, freeze, names, and the formats a fill carries). Outside it, the host's undo takes
  a placed table back and the workbook follows the version the table shows.
- **A range on the page.** One text frame holding a native table: the workbook's fills, borders,
  fonts and conditional formatting, numbers aligned right and centred cells centred, Excel's
  bottom alignment where a row is tall enough to show it, merged cells, column widths measured
  with the document's font metrics, and conditional-format data bars drawn under the table. The
  frame lands at the current selection, as tall as the table the engine lays out (its rows grown
  to their tallest cell), so every row shows. On an engine of protocol 66 or later, placing a
  range is one document write: the frame, its binding, the table and the content in one batch.
  An older engine takes two, the frame and table first and the content second.
- **Placed tables follow the workbook.** After an edit the table is refreshed in place: only
  changed cells are re-poured, rows and columns reshaped, data bars redrawn, the binding's
  content version stamped, all in one document write and one undo step. A table placed in an
  earlier session is found again on restore and refreshes the same way; one whose page no longer
  shows the saved workbook is replaced.
- **Pagination.** A command threads a range over linked frames, repeats the header rows with
  their formatting in every frame, and re-paginates when a frame is resized, refreshing its own
  tables rather than adding new ones.
- **Charts.** Ten kinds, read from the workbook or authored over a range, placed as vector paths
  and labelled text frames in one document write, grouped as one object, and replaced in one
  write when their data changes. Each label frame is sized to its text (measured with the
  document's font metrics), set at the chart's size and justified to its anchor, inside the
  chart. A chart placed while something is selected lands beside it (below, else to its right),
  never on top of it.
- **Host panels in the sheet context.** The host's Swatches panel shows and edits the workbook
  chart colours; the Character and Paragraph panels show the selected cells' type, read-only.
- **Datasets.** A panel lists the datasets the host offers, by a display name derived from the
  provider id (the id is in the tooltip), and fills a new workbook from one.
- **Export.** An `.xlsx` exporter. Parts the session did not touch are written back with their
  original bytes, including pivot caches, macros and other parts never interpreted.

- **Object model.** On `main` and published since 0.1.0-canary.15. The contract is plugin-sdk's
  [`DESIGN.md`](https://github.com/paged-media/plugin-sdk/blob/main/DESIGN.md) §21, with the
  fix-ups from its first consumers in §21.8 and the panel property row in §12.7. Seven
  kinds under `plugin:media.paged.sheet/…`: `workbook/main`, `sheet/<name>`,
  `cell/<sheet>!<A1>` (value, formula, input, display, number format, font, fill, alignment,
  wrap), `range/<sheet>!<A1:B2>` (values, inputs, formula fill, display and the same format
  rows), `namedRange/<name>`, `table/<name>` and `chart/<index>` (type, title, legend, series
  ranges, axis titles and bounds). The rows are in `packages/sheet-bundle/object-model/`. A
  `host.objects` batch reaches ONE plugin-level planner (every kind's ops, in batch order),
  applies to the engine, stores the workbook as a content-addressed version
  (`versions/<hash>.xlsx` + `.json`) and answers a `state` write hosted on `doc` under the
  sub-key `x-paged:media.paged.sheet.wb`, which the registry merges into the document label's
  `data.wb` (other sub-keys survive; labels written before read unchanged): one undo step, and
  document undo/redo reloads the version the label names. The range `values`, `inputs` and
  `display` rows are `heavy` (skipped by `getAll` unless asked); typed command arguments carry
  struct defaults (`count` 1, `live` true, `header` true, the find options false). A
  100-cell range write is one engine write, one save, one commit and two part writes (a pinned
  budget). Typed commands (16 typed twins of palette commands, plus `publishDataset` /
  `unpublishDataset`) take value-typed arguments; 8 commands remain untyped. The bundle boots in
  `createHeadlessHost` with the real engine for get, set, batch and undo.
- **Datasets, both ways.** A dataset-sourced sheet re-pulls on every provider revision by default
  (`live: false` keeps the old mark-stale behaviour), and a sheet range can be published as a
  `dataset` provider whose revision moves with every edit.

## Limits of what is shipped

- **Object model.** Tables are read-only (the xlsx table part is written back byte for byte, so
  a table edit would not survive a save). The version label is the document's, not a frame's:
  core keeps one label per plugin per host, and a frame's already holds the placement binding.
  The page refresh after a write is a second document step. Charts the object model creates or
  changes are kept as a chart-op journal next to each version; charts authored in the panel are
  not.
- **Engines before protocol 66.** Placing a range takes two writes and so two undo steps (frame
  and table, then content), because one write needs the engine to resolve a table handle inside
  a batch; and each in-frame grid change invalidates every page cache. Core v0.66.0 and later
  resolve the handle, and a grid change re-encodes only the pages that show its frame.
- **Protocol-66 doors are feature-detected.** Every published engine since core v0.66.0 has
  them. With them, a shrinking pagination deletes the tables it no longer needs (otherwise they
  are emptied and kept), placed elements are read back from the batch's own list (otherwise from
  a scene-tree difference), and a frame's story is one read (otherwise a walk over the stories).
- **Engines before core v0.71.8** leave a blank line above a table placed in a fresh frame. The
  frame is sized without that line, so on those engines a freshly placed table's frame is one
  line short.
- **Data bars** are page paths stacked one step under the table's frame: a frame that later had
  another item stacked directly over it gets its bars under that item. Paginated chains and
  tables rediscovered from an earlier session draw no bars.
- **Rediscovery** finds tables placed by this version; a table placed before the binding
  carried its table record is not found again. Charts and paginated chains are not
  rediscovered.
- **Colours.** `getStyle` and the page lowering resolve theme and indexed colours through the
  theme part and the workbook palette; conditional-format colour-scale stops still use the
  default Office theme.
- **One workbook** is stored at a time; importing another replaces it.
- **Filter views** hide rows from the page lowering; they are not written into the xlsx and no
  panel calls them yet.
- **Formulas** are the Excel en-US dialect only. On load, a formula the parser does not read
  keeps its cached value: an unregistered function, an external defined name or DDE item
  (`[1]!Name`).
  `TREND` fits one regressor only. A typed date is stored as text.
- **Locale.** The display locale is taken from the workbook's number formats on load.
- **Saving an edited sheet** drops unknown attributes on its rows and cells and unknown elements
  inside `<sheetData>`; unknown children of `<worksheet>` are kept.

## Not built

- **The tab-separated lane with drawn rules** as a user choice: it runs only through an option
  the bundle never sets.
- **Exact decimal arithmetic:** a Cargo feature `sheet-js` does not enable
  ([ADR 501](adr/501-f64-numbers.md)).
- **Interpreting pivot tables, data validation, external links or macros**
  ([ADR 504](adr/504-publishing-first-scope.md)). External links are never followed; a formula
  over another workbook (`[1]Sheet1!A1`) evaluates from the values cached in the file.
- **Other file formats.** `.xlsx`, CSV and TSV are read; `.xlsx` is written; a legacy `.xls` is
  refused.
- **Localised function names and argument separators**, and CJK display locales.
- **A worker-hosted engine.** The manifest declares no worker capability.
- **Three registry rows marked `planned`:** `sheet.chart.design-markers`,
  `sheet.format.locale.cjk-followup`, `sheet.style.doc-style-group`.

## Releases

What each published canary of `@paged-media/sheet` added since 0.1.0-canary.8. A merge that
leaves the version unchanged publishes nothing; 0.1.0-canary.9 to .12 were never published.

| Version | Published | Added |
|---|---|---|
| 0.1.0-canary.20 | 2026-10-13 | No blank line above a placed table on core v0.71.8, which puts the table in the frame's first paragraph, so the frame is sized to the rows alone. The real-engine tests move to canvas-wasm 0.71.8. |
| 0.1.0-canary.19 | 2026-10-13 | A number literal beyond the double-precision range is a formula error instead of infinity. |
| 0.1.0-canary.18 | 2026-10-13 | A placed table's frame is sized to the rows the engine lays out, so no row is overset. A placed chart is one grouped object, its labels sized to their text, beside the selection. The registry is read at build time with `serde_norway` instead of `serde_yaml`. |
| 0.1.0-canary.17 | 2026-10-11 | Warnings a user sees say what happens in plain words instead of naming internal ids and wire terms. |
| 0.1.0-canary.16 | 2026-10-11 | The panel pass: the grid follows the panel's width and shows refusals; the Workbook panel in collapsible sections; find and replace in the Grid panel; number format, font, fill, alignment and iterative calculation in Sheet properties; "Place on page" for "Lower to frame"; a second click before deleting; dataset display names. |
| 0.1.0-canary.15 | 2026-10-11 | The object model (seven kinds, 16 typed command twins, `publishDataset` / `unpublishDataset`) and the Sheet properties panel; datasets that re-pull on every revision and ranges published as datasets; one engine instance per realm, with a trap reported as a diagnostic. Engine protocol 71 (canvas-wasm 0.71.1). Peers `>=0.2.45-canary.0`. |
| 0.1.0-canary.14 | 2026-10-06 | A reopened document restores its workbook on `document.onDidOpen` where the host reports opens. Tests against canvas-wasm 0.70.0. |
| 0.1.0-canary.13 | 2026-10-05 | The first release since canary.8, with the 2026-10-04 round: the Excel oracle, the format and layout, names, sort, find, fill and clipboard work, placed tables that refresh in place, data bars, pagination with repeated headers, the protocol-66 doors, persistence of every edit, restore on open, the wheel in the in-frame grid, the will-save flush and one-call placement. |
