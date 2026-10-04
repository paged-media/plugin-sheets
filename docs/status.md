# Status

What `paged.sheet` ships and what it does not, read from the code at commit `71f37d7`
(`@paged-media/sheet` 0.1.0-canary.8). How the parts fit is in [`architecture.md`](architecture.md).
The gaps against the everyday fundamentals of a spreadsheet, with their classes and the order
in which they are closed, are in [`design/analysis-2026-10-04.md`](design/analysis-2026-10-04.md).

## Shipped

- **Open a workbook.** An `.xlsx` file reaches the plugin through the host's importer, the "Import workbook (.xlsx)" command
  (host file picker) or the Workbook panel's file input. The bytes are stored in the document when the host supports container
  parts, and also in the browser's per-plugin store when the host has one; `restore` loads them again when the plugin activates.
- **Calculation.** 224 registered functions, all implemented; dynamic arrays that spill;
  Excel tables with structured references; recalculation after every cell edit; the cells
  of a circular reference show `#REF!`. Number formats render in five display locales.
- **Workbook panel.** Pick a sheet and a range, place the range on the page, sort a range,
  find and replace, list and author charts, create a cell style from a selected cell, see an
  inventory of frozen panes, data validations and comments, browse the function list.
- **Grid panel.** The active sheet drawn as SVG, scrolled with buttons, with click
  selection, a cell editor with function completion, and copy and paste of cell ranges.
- **A range on the page.** One text frame holding a native table: formatted cell text,
  merged cells, column widths measured with the document's font metrics, row heights, grid
  rules as cell edges. The frame carries the binding to its sheet and range.
- **Editing in place.** A double-click on a sheet frame shows the grid inside the frame.
  Click selects a cell, typing edits it, Enter commits, Escape cancels. Undo and redo work on
  cell edits while the context is active; a sort, a replace-all or a paste is one step.
- **Charts.** Ten kinds, read from the workbook's chart parts or authored over a range,
  placed on the page as vector paths and text frames with document swatches.
- **Host panels in the sheet context.** The host's Swatches panel shows the colours of the workbook's charts and
  can edit one; a workbook without charts leaves the panel on the document's own swatches. The host's Character
  and Paragraph panels show font family, style and size of the selected cells, read-only.
- **Datasets.** A panel lists the datasets the host offers and fills a new workbook from one.
- **Export.** An `.xlsx` exporter. Parts the session did not touch are written back with
  their original bytes, including pivot caches, macros and other parts never interpreted.

## Limits of what is shipped

- **Edits are not stored.** The workbook is stored on import only (`persistWorkbook` in
  `packages/sheet-bundle/src/session.ts`). Cell edits, sorts, replacements, pastes, authored
  charts and a dataset-sourced workbook stay in memory and reach a file only through the
  exporter. Authored charts are never written to XLSX. One workbook is stored at a time;
  importing another replaces it.
- **The page table does not follow edits.** Leaving the edit context clears the journal and
  the in-frame grid; nothing lowers the range again. `contentVersion` in the binding is
  always 0, and the binding is read only to recognise the frame.
- **The page table is unstyled.** The lowering the bundle calls (`get_range_lowered`)
  carries no workbook fonts, fills, borders or conditional formatting, and cell alignment is
  not applied. The translator can write cell fills and style borders, and those paths are
  tested, but this call gives it no styles.
- **One frame per range**, placed 24 pt from the top-left of the active page and clamped to
  540 x 720 pt. A range of more than 1,048,576 cells is refused.
- **The in-frame grid** always shows the session's active sheet from its first row and
  column, whatever the frame is bound to. It does not scroll, and draws text left-aligned in
  one style with no text cursor.
- **Undo** of cell edits is reachable only inside the `sheet` edit context; the journal is
  cleared on exit and on load. Placing a range is not one undo step in the document: the
  frame, the table, each cell's text and the cell decoration are separate writes.
- **Sorting** refuses a range that contains formulas or spilled cells. **Cell style from
  selection** creates and fills the style and applies it to the selected cell; the panel
  reports whether the host accepted the apply.
- **Clock.** The engine takes the current time as an injected serial; the bundle never sets
  it, so `NOW` and `TODAY` evaluate from serial 0. The random seed is a fixed default.
- **Formulas** are the Excel en-US dialect only. On load, a formula with an unregistered
  function is not parsed and the cell keeps its cached value. A typed date is stored as text.
- **Locale.** The display locale is taken from the workbook's number formats on load; there is no setting for it.
- **Saving an edited sheet** drops unknown attributes on its rows and cells and unknown
  elements inside `<sheetData>`; unknown children of `<worksheet>` are kept.
- **Verification.** Both LibreOffice oracle tests end in `todo!()`. The real-workbook tests
  are opt-in, need a corpus that is not in this repo, and assert that files open or are
  refused, not that values match. In the vitest workflow a failing spec does not fail the job.

## Not built

- **Pagination across linked frames from the UI.** The engine paginates a range over a list
  of frame boxes, and `lowerPaginatedToChain` and `subscribeChainReflow` are exported and
  tested, but no command or panel calls them.
- **Refreshing a placed table** after an edit, and saving edits to the stored workbook.
- **Inserting and deleting rows or columns:** `Engine::apply_edit` exists, `SheetSession`
  has no method for it. **Iterative calculation:** the engine supports it, but neither the
  XLSX reader nor `SheetSession` turns it on.
- **Conditional formatting on the page.** `lower_range_condfmt` is tested but not called by
  `sheet-js`; the grid draws data bars only.
- **The tab-separated lane with drawn rules** as a user choice: it runs only through an
  option the bundle never sets. The runtime fallback pours text without rules.
- **Exact decimal arithmetic:** a Cargo feature `sheet-js` does not enable ([ADR 501](adr/501-f64-numbers.md)).
- **Interpreting pivot tables, data validation, external links or macros**
  ([ADR 504](adr/504-publishing-first-scope.md)). External links are never followed.
- **Other file formats.** `.xlsx` is the only one read or written; a legacy `.xls` is refused.
- **Localised function names and argument separators**, and CJK display locales.
- **A worker-hosted engine.** The manifest declares no worker capability.
- **Three registry rows marked `planned`:** `sheet.chart.design-markers`,
  `sheet.format.locale.cjk-followup`, `sheet.style.doc-style-group`.
