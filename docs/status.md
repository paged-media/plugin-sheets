# Status

What `paged.sheet` ships and what it does not, read from the development branch on 2026-10-04
(`@paged-media/sheet` 0.1.0-canary.10, not yet published; npm `canary` is 0.1.0-canary.8). How
the parts fit is in [`architecture.md`](architecture.md). The analysis behind this round of work,
and what changed against it, is in [`design/analysis-2026-10-04.md`](design/analysis-2026-10-04.md).

## Shipped

- **Open and keep a workbook.** An `.xlsx` file reaches the plugin through the host's importer,
  the "Import workbook (.xlsx)" command or the Workbook panel; CSV and TSV import as a one-sheet
  workbook, typed by locale; "New workbook" starts a blank one. The bytes are stored in the
  document when the host supports container parts, and in the browser's per-plugin store when it
  has one. Every committed edit re-saves the workbook after a short quiet period, and
  deactivation flushes, so a reload restores the edited workbook.
- **Calculation.** 259 registered functions, all implemented, among them `LET`, `LAMBDA` and its
  helpers; dynamic arrays that spill and nest; Excel tables with structured references;
  incremental recalculation with an interval index for range dependencies; dependents of
  volatile cells recalculate with them; iterative calculation when the workbook's `calcPr`
  asks for it; `NOW` and `TODAY` follow the host clock. Number formats render in five display
  locales. Expected values are recorded from Excel and replayed in CI.
- **Workbook panel.** Sheets (add, rename, delete), a range, placement on the page, sort (formula
  cells move with their row), find and replace, charts, cell styles, a Format & Layout section
  (number format, font, fill, borders, alignment, wrap, merge, column width, row height, freeze,
  defined names), rows and columns inserted and deleted, the function list.
- **Grid panel and in-frame grid.** Range selection by click, drag and shift; arrows, Tab, F2,
  Delete and scrolling in the frame; a cell editor with function completion; copy and paste of
  ranges with relative references re-addressed; the fill handle with number, date, weekday and
  month series; Fill Down / Fill Right; find in the sheet.
- **Undo.** In the edit context, every committed cell edit is one step, and so is each bulk
  operation (sort, replace-all, paste, fill, clear) and each format change (style, borders,
  merge, sizes, freeze, names, and the formats a fill carries). Outside it, the host's undo takes
  a placed table back and the workbook follows the version the table shows.
- **A range on the page.** One text frame holding a native table: the workbook's fills, borders,
  fonts and conditional formatting, numbers aligned right and centred cells centred, Excel's
  bottom alignment where a row is tall enough to show it, merged cells, column widths measured
  with the document's font metrics, and conditional-format data bars drawn under the table. The
  frame lands at the current selection. Placing a range is two document writes; the content is
  one undo step.
- **Placed tables follow the workbook.** After an edit the table is refreshed in place: only
  changed cells are re-poured, rows and columns reshaped, data bars redrawn, the binding's
  content version stamped, all in one document write and one undo step. A table placed in an
  earlier session is found again on restore and refreshes the same way; one whose page no longer
  shows the saved workbook is replaced.
- **Pagination.** A command threads a range over linked frames, repeats the header rows with
  their formatting in every frame, and re-paginates when a frame is resized, refreshing its own
  tables rather than adding new ones.
- **Charts.** Ten kinds, read from the workbook or authored over a range, placed as vector paths
  and labelled text frames in one document write, and replaced in one write when their data
  changes.
- **Host panels in the sheet context.** The host's Swatches panel shows and edits the workbook
  chart colours; the Character and Paragraph panels show the selected cells' type, read-only.
- **Datasets.** A panel lists the datasets the host offers and fills a new workbook from one.
- **Export.** An `.xlsx` exporter. Parts the session did not touch are written back with their
  original bytes, including pivot caches, macros and other parts never interpreted.

## Limits of what is shipped

- **Document writes.** Placing a range takes two writes and so two undo steps (frame and table,
  then content); one write needs the engine to resolve a table handle inside a batch, which
  is in the unreleased protocol-66 engine. Each in-frame grid change still invalidates every
  page cache on a released engine; per-page invalidation is in the same engine batch.
- **Protocol-66 doors are used when present.** With them, a shrinking pagination deletes the
  tables it no longer needs (otherwise they are emptied and kept), placed elements are read back
  from the batch's own list (otherwise from a scene-tree difference), and a frame's story is one
  read (otherwise a walk over the stories).
- **Data bars** are page paths stacked one step under the table's frame: a frame that later had
  another item stacked directly over it gets its bars under that item. Paginated chains and
  tables rediscovered from an earlier session draw no bars.
- **Rediscovery** finds tables placed by this version; a table placed before the binding
  carried its table record is not found again. Charts and paginated chains are not
  rediscovered.
- **Colours.** `getStyle` resolves theme and indexed colours through the theme part and the
  workbook palette; the page lowering still resolves theme colours with a six-slot default.
- **One workbook** is stored at a time; importing another replaces it.
- **Filter views** hide rows from the page lowering; they are not written into the xlsx and no
  panel calls them yet.
- **Formulas** are the Excel en-US dialect only. On load, a formula with an unregistered function
  keeps its cached value. `TREND` fits one regressor only. A typed date is stored as text.
- **Locale.** The display locale is taken from the workbook's number formats on load.
- **Saving an edited sheet** drops unknown attributes on its rows and cells and unknown elements
  inside `<sheetData>`; unknown children of `<worksheet>` are kept.

## Not built

- **The tab-separated lane with drawn rules** as a user choice: it runs only through an option
  the bundle never sets.
- **Exact decimal arithmetic:** a Cargo feature `sheet-js` does not enable
  ([ADR 501](adr/501-f64-numbers.md)).
- **Interpreting pivot tables, data validation, external links or macros**
  ([ADR 504](adr/504-publishing-first-scope.md)). External links are never followed.
- **Other file formats.** `.xlsx`, CSV and TSV are read; `.xlsx` is written; a legacy `.xls` is
  refused.
- **Localised function names and argument separators**, and CJK display locales.
- **A worker-hosted engine.** The manifest declares no worker capability.
- **Three registry rows marked `planned`:** `sheet.chart.design-markers`,
  `sheet.format.locale.cjk-followup`, `sheet.style.doc-style-group`.
