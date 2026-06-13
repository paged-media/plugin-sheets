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
        FindOptions, FrameBoxArg, GridSceneOptions, LowerOptions, PaginateOptionsArg,
        SheetSession,
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
            self.session = SheetSession::load_xlsx(bytes).map_err(map_err)?;
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
        /// (0-based, RELATIVE to the range). VALUES-ONLY ranges sort fully;
        /// a range containing formula cells (or spill output) REFUSES with a
        /// boundary error (the honest subset — no silent reference
        /// corruption; semantics documented on the session method). Returns
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
            let hits = self.session.find_all(sheet, needle, opts).map_err(map_err)?;
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

        /// Read a range (`"A1:D9"` or `"A1"`) as a rectangular grid of
        /// formatted DISPLAY strings (K-6 / S-14 — the clipboard copy
        /// interchange). Returns `string[][]` (row-major, `""` for empty
        /// cells). Junk endpoints / an OOB sheet are boundary errors.
        pub fn get_range_values(
            &self,
            sheet: u16,
            range: &str,
        ) -> Result<JsValue, JsValue> {
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

    /// Install the panic hook once (readable wasm panics in the console).
    #[wasm_bindgen(start)]
    fn start() {
        console_error_panic_hook::set_once();
    }
}
