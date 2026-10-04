/*
 * This file is part of paged (https://paged.media).
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 * paged is distributed in the hope that it will be useful, but WITHOUT ANY
 * WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
 * FOR A PARTICULAR PURPOSE. See the licenses for details.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! # sheet-js — the wasm-bindgen surface (spec §4, the final Rust join)
//!
//! ALL spreadsheet semantics live in the Rust `sheet-*` crates (constitution
//! hard rule). This crate is the THIN boundary that exposes one wasm class —
//! `SheetEngine` — over the plain-Rust [`core::SheetSession`]. Every method
//! forwards to the session and serialises its serde structs across the wasm
//! door with `serde-wasm-bindgen`; nothing computes here.
//!
//! ## Two layers, one logic
//!
//! - [`core::SheetSession`] — plain Rust, native-typed. The full engine
//!   (load → recalc → set → save → lower) lives here, so `sheet-conformance`
//!   exercises it WITHOUT a wasm runtime (`tests/js_surface.rs`).
//! - `SheetEngine` (below) — `#[cfg(target_arch = "wasm32")]` only, because
//!   `JsValue`-returning `#[wasm_bindgen]` methods compile only for wasm32.
//!   It is a forwarding shim with NO logic of its own.
//!
//! ## The TS consumer contract (`sheet-bundle/src/engine.ts`)
//!
//! The facade boots `new mod.SheetEngine()` (an empty workbook), then calls the
//! snake_case instance methods `load_xlsx` / `save_xlsx` / `set_cell` /
//! `get_cell_display` / `get_range_lowered` / `paginate` / `get_grid_scene` /
//! `set_grid_selection` / `list_sheets` / `free`. The names and JSON shapes
//! below match that contract exactly; `metadata` / `set_now` are additive (the
//! panel uses them).

pub mod core;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use crate::core::{
        CellInput, FindOptions, FrameBoxArg, GridSceneOptions, LowerOptions, PaginateOptionsArg,
        SheetSession, StructuralEdit,
    };
    use wasm_bindgen::prelude::*;

    /// The wasm class the bundle consumes (`sheet-bundle/src/engine.ts`'s
    /// `SheetWasmEngine`). A thin shim over [`SheetSession`] — every method
    /// forwards; nothing computes here (semantics live in the Rust crates).
    #[wasm_bindgen]
    pub struct SheetEngine {
        session: SheetSession,
    }

    #[wasm_bindgen]
    impl SheetEngine {
        /// Construct an empty workbook (one sheet "Sheet1") — lets the panel
        /// start without a file. The facade calls `new mod.SheetEngine()`.
        #[wasm_bindgen(constructor)]
        pub fn new() -> SheetEngine {
            SheetEngine {
                session: SheetSession::new(),
            }
        }

        /// Parse + load an xlsx into this engine (replaces the current
        /// workbook). Recalc runs as part of the load.
        pub fn load_xlsx(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
            // Carry the host clock across the load so its recalc sees it.
            let now = self.session.now_serial();
            self.session = SheetSession::load_xlsx_at(bytes, now).map_err(map_err)?;
            Ok(())
        }

        /// Import delimited text (CSV/TSV) as a fresh one-sheet workbook
        /// (replaces the current one). `delimiter` empty = sniff; the
        /// `locale_tag` (host language, `"de-DE"`) reads numbers and dates;
        /// `sheet_name` names the sheet. Typing is Rust's (`core::csv`).
        pub fn load_csv(
            &mut self,
            text: &str,
            delimiter: &str,
            locale_tag: &str,
            sheet_name: &str,
        ) -> Result<(), JsValue> {
            let now = self.session.now_serial();
            let delim = delimiter.chars().next();
            self.session = SheetSession::load_csv(text, delim, locale_tag, sheet_name, now)
                .map_err(map_err)?;
            Ok(())
        }

        /// Re-emit the workbook as XLSX bytes (lazy-verbatim preservation).
        pub fn save_xlsx(&mut self) -> Result<Vec<u8>, JsValue> {
            self.session.save_xlsx().map_err(map_err)
        }

        /// Commit one cell input (value or formula). Returns
        /// `{changed:[{sheet,row,col,display}], circular:[{sheet,row,col}]}`.
        pub fn set_cell(
            &mut self,
            sheet: u16,
            row: u32,
            col: u32,
            input: &str,
        ) -> Result<JsValue, JsValue> {
            let result = self
                .session
                .set_cell(sheet, row, col, input)
                .map_err(map_err)?;
            to_js(&result)
        }

        /// Commit a batch of inputs `[{sheet,row,col,input}]` with ONE
        /// recalc. Every input is validated and parsed first: a bad sheet id
        /// or a parse error rejects the whole batch (boundary error), the
        /// workbook untouched. Returns the slim `{changedCount, circular}`.
        pub fn set_cells(&mut self, inputs: JsValue) -> Result<JsValue, JsValue> {
            let inputs: Vec<CellInput> = serde_wasm_bindgen::from_value(inputs)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            let result = self.session.set_cells(&inputs).map_err(map_err)?;
            to_js(&result)
        }

        /// The current formatted display of one cell (`""` for empty/OOB).
        pub fn get_cell_display(&self, sheet: u16, row: u32, col: u32) -> String {
            self.session.get_cell_display(sheet, row, col)
        }

        /// The cell's re-enterable INPUT text (`"=…"` for a formula cell;
        /// `""` for empty/OOB) — the ADR-012 undo journal's faithful inverse.
        pub fn get_cell_input(&self, sheet: u16, row: u32, col: u32) -> String {
            self.session.get_cell_input(sheet, row, col)
        }

        /// Stable publishing-grade sort of a range's rows by `key_col`
        /// (0-based, RELATIVE to the range). Formula cells move with their
        /// row (relative references re-addressed, Excel's rule); a range
        /// holding spilled array output REFUSES with a boundary error
        /// (semantics documented on the session method). Returns
        /// `{changed,circular,edits}` — `edits` carries the per-cell
        /// prev/next inputs for the bundle's ADR-012 journal.
        pub fn sort_range(
            &mut self,
            sheet: u16,
            range: &str,
            key_col: u32,
            ascending: bool,
            has_header: bool,
        ) -> Result<JsValue, JsValue> {
            let result = self
                .session
                .sort_range(sheet, range, key_col, ascending, has_header)
                .map_err(map_err)?;
            to_js(&result)
        }

        /// Set one column criterion of the sheet's filter VIEW over `range`
        /// (header row first): `kind` is `"equals"`, `"contains"`, `"top"`
        /// or `"bottom"`, `col` 0-based within the range. The page lowering
        /// skips the hidden rows; nothing is written into the xlsx. Returns
        /// `{hiddenRows}` (0-based sheet rows).
        pub fn set_filter(
            &mut self,
            sheet: u16,
            range: &str,
            col: u32,
            kind: &str,
            value: &str,
        ) -> Result<JsValue, JsValue> {
            let result = self
                .session
                .set_filter(sheet, range, col, kind, value)
                .map_err(map_err)?;
            to_js(&result)
        }

        /// Remove the sheet's filter view; returns `{hiddenRows}` (the rows
        /// still hidden by it — always empty).
        pub fn clear_filter(&mut self, sheet: u16) -> Result<JsValue, JsValue> {
            let result = self.session.clear_filter(sheet).map_err(map_err)?;
            to_js(&result)
        }

        /// Find every populated cell matching `needle`. `sheet` scopes to
        /// one sheet; `undefined` scans the whole workbook. `opts` is
        /// `{matchCase?, entireCell?, inFormulas?}` (undefined/partial
        /// accepted). Returns `[{sheet,row,col,excerpt}]` in row-major
        /// order. An empty needle is a boundary error.
        pub fn find_all(
            &self,
            sheet: Option<u16>,
            needle: &str,
            opts: JsValue,
        ) -> Result<JsValue, JsValue> {
            let opts: FindOptions = if opts.is_undefined() || opts.is_null() {
                FindOptions::default()
            } else {
                serde_wasm_bindgen::from_value(opts)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let hits = self
                .session
                .find_all(sheet, needle, opts)
                .map_err(map_err)?;
            to_js(&hits)
        }

        /// Replace every occurrence of `needle` with `replacement` over the
        /// scope, operating on cell INPUT texts re-entered through the
        /// normal `set_cell` lane (a replacement that fails to parse SKIPS
        /// that cell — reported, never half-applied). Returns
        /// `{occurrences,changed,circular,edits,skipped}`.
        pub fn replace_all(
            &mut self,
            sheet: Option<u16>,
            needle: &str,
            replacement: &str,
            opts: JsValue,
        ) -> Result<JsValue, JsValue> {
            let opts: FindOptions = if opts.is_undefined() || opts.is_null() {
                FindOptions::default()
            } else {
                serde_wasm_bindgen::from_value(opts)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let result = self
                .session
                .replace_all(sheet, needle, replacement, opts)
                .map_err(map_err)?;
            to_js(&result)
        }

        /// Lower a range (`"A1:D9"` or `"A1"`) to the `LoweredContent` IR.
        pub fn get_range_lowered(
            &self,
            sheet: u16,
            range: &str,
            opts: JsValue,
        ) -> Result<JsValue, JsValue> {
            // Accept undefined/null/partial — serde defaults fill the rest.
            let opts: LowerOptions = if opts.is_undefined() || opts.is_null() {
                LowerOptions::default()
            } else {
                serde_wasm_bindgen::from_value(opts)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let lowered = self
                .session
                .get_range_lowered(sheet, range, opts)
                .map_err(map_err)?;
            to_js(&lowered)
        }

        /// Lower a range to the `LoweredContent` IR with the workbook's REAL
        /// per-cell visual styles resolved (the M1 style-map track), rather
        /// than the frozen key-0-only table `get_range_lowered` emits.
        ///
        /// ADR-023: the host's Character/Paragraph panels read cell text
        /// formatting through this door while paged.sheet's edit context is
        /// active. Base styles only — see `SheetSession::get_range_styled`.
        pub fn get_range_styled(
            &self,
            sheet: u16,
            range: &str,
            opts: JsValue,
        ) -> Result<JsValue, JsValue> {
            let opts: LowerOptions = if opts.is_undefined() || opts.is_null() {
                LowerOptions::default()
            } else {
                serde_wasm_bindgen::from_value(opts)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let lowered = self
                .session
                .get_range_styled(sheet, range, opts)
                .map_err(map_err)?;
            to_js(&lowered)
        }

        /// Lower a range for the PAGE (Wave 4): real per-cell styles with
        /// conditional formatting folded on top, grid rules on by default.
        /// The placed table's door; `get_range_lowered` keeps its key-0
        /// contract.
        pub fn get_range_page(
            &self,
            sheet: u16,
            range: &str,
            opts: JsValue,
        ) -> Result<JsValue, JsValue> {
            let opts: LowerOptions = if opts.is_undefined() || opts.is_null() {
                LowerOptions::default()
            } else {
                serde_wasm_bindgen::from_value(opts)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let lowered = self
                .session
                .get_range_page(sheet, range, opts)
                .map_err(map_err)?;
            to_js(&lowered)
        }

        /// Read a range (`"A1:D9"` or `"A1"`) as a rectangular grid of
        /// formatted DISPLAY strings (K-6 / S-14 — the clipboard copy
        /// interchange). Returns `string[][]` (row-major, `""` for empty
        /// cells). Junk endpoints / an OOB sheet are boundary errors.
        pub fn get_range_values(&self, sheet: u16, range: &str) -> Result<JsValue, JsValue> {
            let rows = self
                .session
                .get_range_values(sheet, range)
                .map_err(map_err)?;
            to_js(&rows)
        }

        /// Paginate a range across the host frame chain's content boxes (Wave
        /// 2D, S-05). `frames` is the chain's content boxes
        /// (`[{widthPt,heightPt}]`); returns the serialized `Vec<Page>` (each
        /// `{frameIndex, content, continued, oversize}`). Reuses
        /// `sheet_lower::paginate`. Accepts undefined/null/partial `opts`.
        pub fn paginate(
            &self,
            sheet: u16,
            range: &str,
            frames: JsValue,
            opts: JsValue,
        ) -> Result<JsValue, JsValue> {
            let frames: Vec<FrameBoxArg> = if frames.is_undefined() || frames.is_null() {
                Vec::new()
            } else {
                serde_wasm_bindgen::from_value(frames)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let opts: PaginateOptionsArg = if opts.is_undefined() || opts.is_null() {
                PaginateOptionsArg::default()
            } else {
                serde_wasm_bindgen::from_value(opts)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let pages = self
                .session
                .paginate(sheet, range, frames, opts)
                .map_err(map_err)?;
            to_js(&pages)
        }

        /// Window a sheet into a `GridScene` for the sheets-mode grid surface
        /// (`{viewport,cells,styles,gridlines,selection}`; spec §8.1). Folds in
        /// any selection recorded by `set_grid_selection` for the same sheet.
        pub fn get_grid_scene(
            &self,
            sheet: u16,
            first_row: u32,
            first_col: u32,
            w_pt: f64,
            h_pt: f64,
            opts: JsValue,
        ) -> Result<JsValue, JsValue> {
            // Accept undefined/null/partial — serde defaults fill the rest.
            let opts: GridSceneOptions = if opts.is_undefined() || opts.is_null() {
                GridSceneOptions::default()
            } else {
                serde_wasm_bindgen::from_value(opts)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let scene = self
                .session
                .get_grid_scene(sheet, first_row, first_col, w_pt, h_pt, opts)
                .map_err(map_err)?;
            to_js(&scene)
        }

        /// Record the sheets-mode selection rectangle (consumed by the next
        /// `get_grid_scene` for the same sheet).
        pub fn set_grid_selection(
            &mut self,
            sheet: u16,
            anchor_row: u32,
            anchor_col: u32,
            rows: u32,
            cols: u32,
        ) -> Result<(), JsValue> {
            self.session
                .set_grid_selection(sheet, anchor_row, anchor_col, rows, cols)
                .map_err(map_err)
        }

        /// Enumerate the workbook's sheets (`[{id,name,rows,cols}]`).
        pub fn list_sheets(&self) -> JsValue {
            to_js(&self.session.list_sheets()).unwrap_or(JsValue::NULL)
        }

        /// Enumerate the workbook's charts (M2, spec §8.4):
        /// `[{index,hostSheet,kind,title,seriesCount}]`.
        pub fn list_charts(&self) -> JsValue {
            to_js(&self.session.list_charts()).unwrap_or(JsValue::NULL)
        }

        /// The chart-kind tags `add_chart` accepts, in panel order —
        /// `["column","stackedColumn","bar","stackedBar","line","area",
        /// "scatter","pie","donut","radar"]`. The panel READS the set
        /// rather than hard-coding it, so a kind can never be offered
        /// that the engine refuses (all chart semantics stay in Rust).
        pub fn chart_kinds(&self) -> JsValue {
            to_js(&crate::core::CHART_KIND_TAGS.to_vec()).unwrap_or(JsValue::NULL)
        }

        /// AUTHOR a chart over live data (empty `categories` = none; kind
        /// ∈ `chart_kinds()`; empty title = none). `series_in` is the
        /// transpose control: `"columns"` (or empty) reads one series per
        /// values COLUMN, `"rows"` one per ROW — the same cells read the
        /// other way round, never a rewrite of the user's data. Returns
        /// the new chart index — the same handle the geometry/lowering
        /// lanes take. Publishing-first: authored charts are PAGE-side
        /// only (the xlsx writer never re-derives chart parts).
        pub fn add_chart(
            &mut self,
            sheet: u16,
            values: &str,
            categories: &str,
            kind: &str,
            title: &str,
            series_in: &str,
        ) -> Result<u32, JsValue> {
            self.session
                .add_chart(sheet, values, categories, kind, title, series_in)
                .map_err(map_err)
        }

        /// Enumerate the worksheets with a FROZEN PANE (spec §8.1):
        /// `[{sheet,rows,cols}]`. The split also folds into `get_grid_scene`.
        pub fn list_freeze_panes(&self) -> JsValue {
            to_js(&self.session.list_freeze_panes()).unwrap_or(JsValue::NULL)
        }

        /// Enumerate the worksheets carrying DATA VALIDATIONS (spec §1.1/§11 —
        /// PRESERVE-ONLY, never enforced/rendered): `[{sheet,count,kinds}]`. A
        /// read-only inventory for preservation transparency (the panel shows
        /// that the workbook carries validations Paged preserves but does not
        /// enforce); the rules round-trip byte-identical regardless.
        pub fn list_data_validations(&self) -> JsValue {
            to_js(&self.session.list_data_validations()).unwrap_or(JsValue::NULL)
        }

        /// Enumerate the workbook's cell comments / notes (preserve-first, spec
        /// §10.2): `[{sheet,row,col,author,text}]`. The grid shows an indicator
        /// (folded into `get_grid_scene`); this carries the text for the
        /// panel/hover. The comments parts round-trip byte-identical (opaque).
        pub fn list_comments(&self) -> JsValue {
            to_js(&self.session.list_comments()).unwrap_or(JsValue::NULL)
        }

        /// Enumerate the engine's registered IMPLEMENTED functions for the
        /// formula-bar autocomplete (S-04). The name table is codegen'd from
        /// the function registry (`registry/functions/*.yaml`) — the bundle's
        /// completion list is the engine's truth (constitution §7), never a
        /// hand-kept TS list. Returns `[{name,family,minArgs,maxArgs}]`
        /// (`maxArgs` null = variadic).
        pub fn list_functions(&self) -> JsValue {
            to_js(&self.session.list_functions()).unwrap_or(JsValue::NULL)
        }

        /// Resolve chart `chart_index`'s series ranges against the live model
        /// and generate its geometry IR for a `w_pt × h_pt` content box (the
        /// same IR the page paged.draw lowering AND the grid view consume).
        /// Returns `{widthPt,heightPt,prims:[...]}`. An OOB index errors.
        pub fn get_chart_geometry(
            &self,
            chart_index: u32,
            w_pt: f64,
            h_pt: f64,
        ) -> Result<JsValue, JsValue> {
            let geom = self
                .session
                .get_chart_geometry(chart_index, w_pt, h_pt)
                .map_err(map_err)?;
            to_js(&geom)
        }

        /// Workbook metadata (`{dateSystem,unparsedFormulas,dirty}`).
        pub fn metadata(&self) -> JsValue {
            to_js(&self.session.metadata()).unwrap_or(JsValue::NULL)
        }

        /// Update the `NOW`/`TODAY` serial.
        pub fn set_now(&mut self, serial: f64) {
            self.session.set_now(serial);
        }

        /// Set `NOW`/`TODAY` from the host clock (`Date.now()` + the host's
        /// `getTimezoneOffset()`); the serial conversion (date system,
        /// local time) is Rust's. Returns the serial. No recalc.
        pub fn set_clock(&mut self, unix_ms: f64, tz_offset_min: f64) -> f64 {
            self.session.set_clock(unix_ms, tz_offset_min)
        }

        /// Recalculate volatile cells against the current clock:
        /// `{changed,circular}`.
        pub fn recalc_volatile(&mut self) -> Result<JsValue, JsValue> {
            to_js(&self.session.recalc_volatile())
        }

        /// The `<calcPr>` iteration knobs in effect:
        /// `{iterative,maxIter,maxChange}`.
        pub fn calc_settings(&self) -> Result<JsValue, JsValue> {
            to_js(&self.session.calc_settings())
        }

        /// Toggle iterative calculation (written back to `<calcPr>` on save).
        pub fn set_iterative(
            &mut self,
            on: bool,
            max_iter: u32,
            max_change: f64,
        ) -> Result<JsValue, JsValue> {
            to_js(&self.session.set_iterative(on, max_iter, max_change))
        }

        /// Add a worksheet at the end (empty name = next free `SheetN`);
        /// returns its id.
        pub fn add_sheet(&mut self, name: &str) -> Result<u16, JsValue> {
            self.session.add_sheet(name).map_err(map_err)
        }

        /// Rename a worksheet (Excel's name rules; unique).
        pub fn rename_sheet(&mut self, sheet: u16, name: &str) -> Result<(), JsValue> {
            self.session.rename_sheet(sheet, name).map_err(map_err)
        }

        /// Delete a worksheet (references to it become `#REF!`); returns
        /// `{changed,circular}`.
        pub fn delete_sheet(&mut self, sheet: u16) -> Result<JsValue, JsValue> {
            let r = self.session.delete_sheet(sheet).map_err(map_err)?;
            to_js(&r)
        }

        /// Insert/delete rows or columns: `kind` is `"insertRows"`,
        /// `"deleteRows"`, `"insertCols"` or `"deleteCols"`; `at` is
        /// 0-based. References are rewritten; refused (model untouched)
        /// when preserved content would be left addressing the wrong cells.
        /// Returns `{changed,circular}`.
        pub fn structural_edit(
            &mut self,
            sheet: u16,
            kind: &str,
            at: u32,
            n: u32,
        ) -> Result<JsValue, JsValue> {
            let kind = match kind {
                "insertRows" => StructuralEdit::InsertRows,
                "deleteRows" => StructuralEdit::DeleteRows,
                "insertCols" => StructuralEdit::InsertCols,
                "deleteCols" => StructuralEdit::DeleteCols,
                other => {
                    return Err(JsValue::from_str(&format!(
                        "unknown structural edit {other:?}"
                    )))
                }
            };
            let r = self
                .session
                .structural_edit(sheet, kind, at, n)
                .map_err(map_err)?;
            to_js(&r)
        }
    }

    #[wasm_bindgen]
    impl SheetEngine {
        // ── Wave 6: formatting & layout ──────────────────────────────

        /// Apply a partial cell style to a range (or a defined name / table
        /// name): `patch` is `{numFmt?, fontName?, fontSize?, bold?,
        /// italic?, underline?, fontColor?, fill?, borderTop?/Right?/
        /// Bottom?/Left?: {style, color?}, hAlign?, vAlign?, wrap?}`.
        /// Returns `{cells, styles}`.
        pub fn set_style(
            &mut self,
            sheet: u16,
            range: &str,
            patch: JsValue,
        ) -> Result<JsValue, JsValue> {
            let patch: crate::core::StylePatchArg = serde_wasm_bindgen::from_value(patch)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            let r = self
                .session
                .set_style(sheet, range, patch)
                .map_err(map_err)?;
            to_js(&r)
        }

        /// The full style of one cell (the `set_style` patch shape, every
        /// field present).
        pub fn get_style(&self, sheet: u16, row: u32, col: u32) -> Result<JsValue, JsValue> {
            let r = self.session.get_style(sheet, row, col).map_err(map_err)?;
            to_js(&r)
        }

        /// Merge a range (top-left keeps its content; other cells are
        /// cleared). Returns `{changed, circular, edits}`.
        pub fn merge(&mut self, sheet: u16, range: &str) -> Result<JsValue, JsValue> {
            let r = self.session.merge(sheet, range).map_err(map_err)?;
            to_js(&r)
        }

        /// Remove every merge intersecting a range; returns how many.
        pub fn unmerge(&mut self, sheet: u16, range: &str) -> Result<u32, JsValue> {
            self.session.unmerge(sheet, range).map_err(map_err)
        }

        /// Set (`undefined` clears) the width of columns `first..=last`, in
        /// characters.
        pub fn set_col_width(
            &mut self,
            sheet: u16,
            first: u32,
            last: u32,
            width: Option<f64>,
        ) -> Result<(), JsValue> {
            self.session
                .set_col_width(sheet, first, last, width)
                .map_err(map_err)
        }

        /// Set (`undefined` clears) the height of rows `first..=last`, in
        /// points.
        pub fn set_row_height(
            &mut self,
            sheet: u16,
            first: u32,
            last: u32,
            height: Option<f64>,
        ) -> Result<(), JsValue> {
            self.session
                .set_row_height(sheet, first, last, height)
                .map_err(map_err)
        }

        /// `{colWidths, rowHeights, merges, freezeRows, freezeCols}` of a
        /// sheet.
        pub fn get_layout(&self, sheet: u16) -> Result<JsValue, JsValue> {
            let r = self.session.get_layout(sheet).map_err(map_err)?;
            to_js(&r)
        }

        /// Resolve a range argument (A1, `Sheet!A1:B2`, a defined name or a
        /// table name) as seen from `sheet`: `{sheet, range}` (range in A1).
        pub fn resolve_range(&self, sheet: u16, text: &str) -> Result<JsValue, JsValue> {
            let r = self
                .session
                .resolve_range_a1(sheet, text)
                .map_err(map_err)?;
            to_js(&r)
        }
    }

    impl Default for SheetEngine {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Map a session error to a JS string error. Calc errors (`#DIV/0!`) are
    /// NOT boundary errors — they are display strings, never reach here.
    fn map_err(e: crate::core::SessionError) -> JsValue {
        JsValue::from_str(&e.to_string())
    }

    /// Serialise a serde value to a `JsValue` (camelCase shapes are decided in
    /// the serde derives, matching the TS contract).
    fn to_js<T: serde::Serialize>(value: &T) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(value).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// The hash of the sources this wasm was built from
    /// (`scripts/source-hash.mjs`, stamped by `scripts/build-wasm.sh`;
    /// "unstamped" for any other build). `packages/sheet-bundle/test/
    /// wasm-fresh.spec.ts` compares it with the checkout, so a stale wasm
    /// fails the suite instead of being tested in place of the code.
    #[wasm_bindgen]
    pub fn engine_source_hash() -> String {
        option_env!("SHEET_JS_SOURCE_HASH")
            .unwrap_or("unstamped")
            .to_string()
    }

    /// The engine's work counters (`sheet_calc::perf`) as a plain object —
    /// ONLY in a wasm built with the `perf-counters` feature
    /// (`scripts/build-wasm-perf.sh`); the shipped wasm has no such export.
    /// The TS perf harness reads it beside its own door counts.
    #[cfg(feature = "perf-counters")]
    #[wasm_bindgen(js_name = perfCounters)]
    pub fn perf_counters() -> JsValue {
        // f64, not u64: serde-wasm-bindgen turns u64 into a BigInt, and the
        // harness wants plain numbers (a count never nears 2^53).
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Out {
            range_probes: f64,
            range_keys_scanned: f64,
            precedent_candidates_scanned: f64,
            ranges_materialized: f64,
            cells_read: f64,
            evaluations: f64,
            recalcs: f64,
            recalc_passes: f64,
            cells_marked_dirty: f64,
        }
        let c = sheet_calc::perf::snapshot();
        to_js(&Out {
            range_probes: c.range_probes as f64,
            range_keys_scanned: c.range_keys_scanned as f64,
            precedent_candidates_scanned: c.precedent_candidates_scanned as f64,
            ranges_materialized: c.ranges_materialized as f64,
            cells_read: c.cells_read as f64,
            evaluations: c.evaluations as f64,
            recalcs: c.recalcs as f64,
            recalc_passes: c.recalc_passes as f64,
            cells_marked_dirty: c.cells_marked_dirty as f64,
        })
        .unwrap_or(JsValue::NULL)
    }

    /// Zero the engine's work counters (`perf-counters` builds only).
    #[cfg(feature = "perf-counters")]
    #[wasm_bindgen(js_name = resetPerfCounters)]
    pub fn reset_perf_counters() {
        sheet_calc::perf::reset();
    }

    /// Install the panic hook once (readable wasm panics in the console).
    #[wasm_bindgen(start)]
    fn start() {
        console_error_panic_hook::set_once();
    }
}
