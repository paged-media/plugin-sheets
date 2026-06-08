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
//! `get_cell_display` / `get_range_lowered` / `get_grid_scene` /
//! `set_grid_selection` / `list_sheets` / `free`. The names and JSON shapes
//! below match that contract exactly; `metadata` / `set_now` are additive (the
//! panel uses them).

pub mod core;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use crate::core::{GridSceneOptions, LowerOptions, SheetSession};
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
