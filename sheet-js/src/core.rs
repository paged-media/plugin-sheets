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

//! The plain-Rust engine session behind the wasm surface (spec §4 sheet-js).
//!
//! ALL spreadsheet semantics live here in Rust (constitution hard rule); the
//! `#[wasm_bindgen]` `SheetEngine` in [`crate`] is a thin `cfg(wasm32)` shim
//! that forwards to a [`SheetSession`] and serialises its serde structs across
//! the wasm door via `serde-wasm-bindgen`. Keeping the logic native-typed lets
//! `sheet-conformance` exercise the FULL load → recalc → set → save → lower
//! loop without a wasm runtime (`tests/js_surface.rs`).
//!
//! ## The engine/container coherence dance (the price of the frozen API)
//!
//! [`sheet_calc::Engine`] OWNS the [`SheetModel`]; [`sheet_xlsx::XlsxDocument`]
//! ALSO has a (public) `model` field plus the private container/bindings needed
//! to re-write the workbook. We cannot destructure `XlsxDocument` (private
//! fields), so on load we `std::mem::take(&mut doc.model)` the model OUT (a
//! `Default` empty model is left in the doc), parse the formula texts into it,
//! and move it into the engine. On save we take the engine BY VALUE
//! ([`Engine::into_model`]), drop the model back into `doc.model`, re-print the
//! EDITED formula cells into `doc.formula_texts`, mark their sheets dirty, and
//! `doc.save()`; then we rebuild a fresh engine from the model so the session
//! stays usable. The rebuild re-marks everything dirty (no recalc — the cached
//! values are already correct), so the next edit simply recomputes its cut.

use std::collections::BTreeSet;

use sheet_calc::{Engine, EngineConfig};
use sheet_chart::{generate as generate_chart, ChartGeometry, PlotData};
use sheet_core::{parse_a1, CellValue, DateSystem, RangeRef, SheetId, SheetModel};
use sheet_format::{FormatCache, FormatCtx};
use sheet_lower::{
    lower_range, paginate as lower_paginate, CellRange, FrameBox, Page, ViewOptions,
};
use sheet_parser::{parse, print, ParseCtx, SheetNames};
use sheet_xlsx::{XlsxChart, XlsxDocument};

// ─────────────────────────────────────────── serde wire structs (camelCase)

/// One changed cell after an edit (spec §9: `display` is the number-FORMATTED
/// value). Matches the TS `CellChange` shape in `sheet-bundle/src/engine.ts`.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct CellChange {
    pub sheet: u16,
    pub row: u32,
    pub col: u32,
    pub display: String,
}

/// A `(sheet, row, col)` address on a circular reference (the
/// `sheet.calc.circular` set). The engine stores `#REF!` in the cell; this
/// reports the membership so the panel can warn.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct CircularRef {
    pub sheet: u16,
    pub row: u32,
    pub col: u32,
}

/// The result of [`SheetSession::set_cell`]: the dirty-cut display changes plus
/// the circular set. The TS facade reads only `changed`; `circular` is an
/// additive field (the DESIGN ruling) and is harmless to structural typing.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Default)]
pub struct SetCellResult {
    pub changed: Vec<CellChange>,
    pub circular: Vec<CircularRef>,
}

/// One worksheet's identity + used-range extent (matches TS `SheetInfo`).
/// `rows`/`cols` are the 1-based extent of the populated range (0 when empty).
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct SheetInfo {
    pub id: u16,
    pub name: String,
    pub rows: u32,
    pub cols: u32,
}

/// One chart in the workbook (M2 charts track, spec §8.4) for the panel's
/// chart list. `index` is the position in the engine's parsed-chart vector (the
/// handle [`SheetSession::get_chart_geometry`] takes); `hostSheet` is the model
/// sheet the chart is anchored to; `kind`/`title`/`seriesCount` summarize it.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChartInfo {
    pub index: u32,
    pub host_sheet: u16,
    /// The lowercase kind tag (`"column"`, `"bar"`, `"line"`, `"area"`,
    /// `"pie"`, `"donut"`, `"scatter"`) — the geometry generator's tag space.
    pub kind: &'static str,
    pub title: Option<String>,
    pub series_count: u32,
}

/// Workbook metadata for the panel (`dateSystem`/`unparsedFormulas`/`dirty`).
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    /// `"1900"` or `"1904"` — the workbook serial-date epoch.
    pub date_system: &'static str,
    /// Formula texts that did not parse on load (T1 features etc.), kept as
    /// raw text + cached value (spec §10.2 preservation invariant).
    pub unparsed_formulas: u32,
    /// Whether unsaved edits are pending (any `set_cell` since load/save).
    pub dirty: bool,
}

/// Lower options forwarded verbatim from the TS `LowerOptions` (serde defaults
/// so an absent/partial object is accepted).
#[derive(serde::Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LowerOptions {
    pub include_grid_rules: Option<bool>,
    pub header_rows: Option<u32>,
}

/// Grid-scene options forwarded verbatim from the TS `GridSceneOptions` (serde
/// defaults so an absent/partial object is accepted). `include_gridlines`
/// toggles the [`sheet_grid::RuleSet`] at every visible track boundary.
#[derive(serde::Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GridSceneOptions {
    pub include_gridlines: Option<bool>,
}

/// One frame's content box, deserialized from the TS chain's content boxes
/// (`{ widthPt, heightPt }` — Wave 2D, S-05). The host reads the chain via
/// `host.document.frameChain(storyId)` + `elementGeometry`; only the height
/// drives pagination, the width rides along (mirrors [`FrameBox`]).
#[derive(serde::Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameBoxArg {
    pub width_pt: f64,
    pub height_pt: f64,
}

/// Pagination options forwarded verbatim from the TS `PaginateOptions` (serde
/// defaults so an absent/partial object is accepted). Mirrors
/// [`sheet_lower::PaginateOptions`] across the wasm door (Wave 2D, S-05).
#[derive(serde::Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PaginateOptionsArg {
    pub repeated_header_rows: Option<u32>,
    pub continued_marker: Option<bool>,
    pub keep_rows_together: Option<Vec<(u32, u32)>>,
}

// ─────────────────────────────────────────────────────────── the session

/// The native engine session: an [`Engine`] (owning the model) + the
/// [`XlsxDocument`] container kept for save, the edited-cell set, and the
/// unparsed-formula count. See the module docs for the coherence dance.
pub struct SheetSession {
    /// `Some` except transiently inside [`SheetSession::save_xlsx`]. Holding it
    /// in an `Option` lets us take the engine BY VALUE for `into_model`.
    engine: Option<Engine>,
    config: EngineConfig,
    /// The XLSX container/bindings/formula_texts. Its `model` field is a
    /// `Default` placeholder while the real model lives in the engine.
    doc: XlsxDocument,
    /// Cells edited since load (`set_cell`): re-printed into `formula_texts` on
    /// save (formula cells) or cleared from it (cells that became values).
    edited: BTreeSet<(SheetId, u32, u32)>,
    /// Formula texts that failed to parse on load (kept verbatim).
    unparsed_formulas: u32,
    /// The sheets-mode selection rectangle (spec §8.1): engine state set by
    /// [`SheetSession::set_grid_selection`] and folded into the next
    /// [`SheetSession::get_grid_scene`] for the SAME sheet. `None` until the
    /// panel records one; a selection on sheet A is not shown on sheet B.
    selection: Option<(SheetId, sheet_grid::GridSelection)>,
}

/// The T0 cap on cells materialized by a single [`SheetSession::get_range_lowered`]
/// call (FREEZE AMENDMENT, audit finding 1).
///
/// `lower_range` eagerly materializes `rows * cols` [`sheet_lower::LoweredCell`]s
/// to cover the range positionally. A full-sheet range like `A1:XFD1048576`
/// (1,048,576 rows × 16,384 cols ≈ 1.7e10 cells) would abort the wasm
/// allocator and poison the wasm-bindgen borrow, bricking the session. We
/// reject the lowering BEFORE materializing whenever the range area exceeds
/// this cap. The cap is the Excel single-column row count (1,048,576) — exactly
/// one full column lowers in well under a second (it is sparse/empty), while
/// any genuinely huge rectangle is rejected. T1 virtualization (S-02 sheets
/// grid) will lift this; until then a degraded-text frame over a million-cell
/// rectangle is not a real publishing target.
const T0_LOWER_CELL_CAP: u64 = 1_048_576;

/// Error type — a plain string the wasm shim maps to `JsValue::from_str`.
/// Calc errors (`#DIV/0!` etc.) are NOT boundary errors; they are display
/// strings, never surfaced here.
#[derive(Debug)]
pub struct SessionError(pub String);

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

impl SheetSession {
    /// An empty workbook with one sheet "Sheet1" — lets the panel start without
    /// a file. The container is a fresh in-memory minimal XLSX so `save_xlsx`
    /// stays total. We `mem::take` the model out (a `Default` placeholder is
    /// left in the doc) and move it into the engine, same as `load_xlsx`.
    pub fn new() -> SheetSession {
        let config = EngineConfig::default();
        let mut doc = XlsxDocument::open(&empty_workbook_bytes())
            .expect("the embedded empty workbook is a valid package");
        let mut model = std::mem::take(&mut doc.model);
        // `open` already added "Sheet1"; guard the invariant defensively.
        if model.sheets.is_empty() {
            model.add_sheet("Sheet1");
        }
        let engine = Engine::new(model, config);
        SheetSession {
            engine: Some(engine),
            config,
            doc,
            edited: BTreeSet::new(),
            unparsed_formulas: 0,
            selection: None,
        }
    }

    /// Parse + load an xlsx (spec §4 / §10.2). Opens the document, takes its
    /// model, parses every `formula_texts` entry through `sheet-parser`
    /// (interning the AST onto the cell's `FormulaId`); unparseable formulas
    /// keep their raw text + cached value and are counted. Then builds the
    /// engine and `recalc_all`s.
    pub fn load_xlsx(bytes: &[u8]) -> Result<SheetSession, SessionError> {
        let config = EngineConfig::default();
        let mut doc = XlsxDocument::open(bytes).map_err(|e| SessionError(e.to_string()))?;
        let mut model = std::mem::take(&mut doc.model);

        // Parse each captured formula text into an interned FormulaId on the
        // cell. xlsx formula text has NO leading '=' — parse it directly.
        let mut unparsed_formulas = 0u32;
        // Collect first (the parser borrows the model immutably via ctx).
        let entries: Vec<((SheetId, u32, u32), String)> = doc
            .formula_texts
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        for ((sheet, row, col), text) in entries {
            let parsed = {
                let ctx = ModelParseCtx {
                    model: &model,
                    current: sheet,
                };
                parse(&text, &ctx)
            };
            match parsed {
                Ok(formula) => {
                    let fid = model.intern_formula(formula);
                    if let Some(ws) = model.sheet_mut(sheet) {
                        if let Some(cell) = ws.cells.get_mut(&(row, col)) {
                            cell.formula = Some(fid);
                        }
                    }
                }
                Err(_) => {
                    // Unparseable (T1 feature / unknown function): keep the raw
                    // text + cached value — the cell stays a value cell with no
                    // FormulaId (preservation invariant, spec §10.2).
                    unparsed_formulas += 1;
                }
            }
        }

        let mut engine = Engine::new(model, config);
        engine.recalc_all();

        Ok(SheetSession {
            engine: Some(engine),
            config,
            doc,
            edited: BTreeSet::new(),
            unparsed_formulas,
            selection: None,
        })
    }

    /// Print every EDITED cell's AST back into `formula_texts` (formula cells)
    /// or clear it (cells that became values), mark the touched sheets dirty,
    /// and `container.save()`. Returns the bytes. See the module docs for the
    /// model swap dance.
    pub fn save_xlsx(&mut self) -> Result<Vec<u8>, SessionError> {
        // 1. Take the engine by value to reach `into_model`.
        let engine = self.engine.take().expect("engine present outside save");
        let model = engine.into_model();
        // 2. Drop the model back into the doc so the writer can read it.
        self.doc.model = model;

        // 3. Re-print edited formula cells into formula_texts; clear entries for
        //    cells that became plain values. Mark each touched sheet dirty.
        let edited: Vec<(SheetId, u32, u32)> = self.edited.iter().copied().collect();
        let mut dirty_sheets: BTreeSet<SheetId> = BTreeSet::new();
        for (sheet, row, col) in edited {
            let names = ModelSheetNames {
                model: &self.doc.model,
            };
            let formula_id = self
                .doc
                .model
                .sheet(sheet)
                .and_then(|ws| ws.cell(row, col))
                .and_then(|c| c.formula);
            match formula_id {
                Some(fid) => {
                    if let Some(formula) = self.doc.model.formula(fid) {
                        // xlsx formula text carries NO leading '=' — print bare.
                        let text = print(formula, sheet, &names);
                        self.doc.formula_texts.insert((sheet, row, col), text);
                    }
                }
                None => {
                    // The cell is now a value (or empty) — drop any stale text.
                    self.doc.formula_texts.remove(&(sheet, row, col));
                }
            }
            dirty_sheets.insert(sheet);
        }
        for sheet in dirty_sheets {
            self.doc.mark_sheet_dirty(sheet);
        }

        // 4. Serialise.
        let bytes = self.doc.save().map_err(|e| SessionError(e.to_string()))?;

        // 5. Rebuild the engine from the model so the session stays usable.
        let model = std::mem::take(&mut self.doc.model);
        self.engine = Some(Engine::new(model, self.config));

        // 6. The edits are now persisted in the bytes — clear the pending set so
        //    `metadata().dirty` reads false until the next edit.
        self.edited.clear();

        Ok(bytes)
    }

    /// Commit one cell input — value or formula (spec §6.2: Excel-like
    /// literal/formula detection lives in `Engine::enter`; `'='` marks a
    /// formula). Returns every cell whose DISPLAY changed plus the circular
    /// set. A parse error is a boundary error.
    pub fn set_cell(
        &mut self,
        sheet: u16,
        row: u32,
        col: u32,
        input: &str,
    ) -> Result<SetCellResult, SessionError> {
        // Validate the sheet id BEFORE entering (FREEZE AMENDMENT, audit
        // finding 2). `Engine::enter` -> `ensure_sheet` would otherwise
        // AUTO-CREATE phantom sheets for an out-of-range id; their data then
        // silently drops on save. The session is the boundary that rejects an
        // OOB id (`sheet-calc` stays frozen).
        let sheet_count = self
            .engine
            .as_ref()
            .expect("engine present")
            .model()
            .sheets
            .len();
        if (sheet as usize) >= sheet_count {
            return Err(SessionError(format!(
                "sheet id {sheet} out of range ({sheet_count} sheets)"
            )));
        }

        let engine = self.engine.as_mut().expect("engine present");
        let result = engine
            .enter(sheet, row, col, input)
            .map_err(|e| SessionError(e.to_string()))?;

        // Record the entered cell as edited (for save re-printing).
        self.edited.insert((sheet, row, col));

        // Build the changed list with each cell's formatted display.
        let model = engine.model();
        let mut cache = FormatCache::default();
        let ctx = FormatCtx::new(model.calc.date_system, model.calc.locale);
        let changed = result
            .changed
            .iter()
            .map(|cref| CellChange {
                sheet: cref.sheet,
                row: cref.row,
                col: cref.col,
                display: cell_display(model, cref.sheet, cref.row, cref.col, &mut cache, &ctx),
            })
            .collect();
        let circular = result
            .circular
            .iter()
            .map(|cref| CircularRef {
                sheet: cref.sheet,
                row: cref.row,
                col: cref.col,
            })
            .collect();

        Ok(SetCellResult { changed, circular })
    }

    /// The current formatted display of one cell (spec §9). `""` for an empty
    /// cell or an out-of-range address.
    pub fn get_cell_display(&self, sheet: u16, row: u32, col: u32) -> String {
        let model = self.engine.as_ref().expect("engine present").model();
        let mut cache = FormatCache::default();
        let ctx = FormatCtx::new(model.calc.date_system, model.calc.locale);
        cell_display(model, sheet, row, col, &mut cache, &ctx)
    }

    /// The cell's re-enterable INPUT text (ADR-012 — the in-session undo
    /// journal's faithful inverse): a formula cell re-prints as
    /// `"=" + print(AST)` (exactly what `set_cell` accepts back); a value
    /// cell re-prints its literal (number / text / TRUE / FALSE; an error
    /// its `#…!` code); an empty or out-of-range cell `""`. The DISPLAY
    /// string is NOT a valid inverse — re-entering a formula's display
    /// would bake the computed value over the formula.
    pub fn get_cell_input(&self, sheet: u16, row: u32, col: u32) -> String {
        let model = self.engine.as_ref().expect("engine present").model();
        let Some(ws) = model.sheet(sheet) else {
            return String::new();
        };
        let Some(cell) = ws.cell(row, col) else {
            return String::new();
        };
        if let Some(fid) = cell.formula {
            if let Some(formula) = model.formula(fid) {
                let names = ModelSheetNames { model };
                return format!("={}", print(formula, sheet, &names));
            }
        }
        match &cell.value {
            CellValue::Empty => String::new(),
            // Shortest round-trip float text — `Engine::enter` parses it
            // back to the same f64.
            CellValue::Number(n) => n.to_string(),
            CellValue::Text(t) => t.to_string(),
            CellValue::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).to_string(),
            CellValue::Error(e) => e.as_str().to_string(),
        }
    }

    /// Lower a range (`"A1:D9"` or a single cell `"A1"`) to the
    /// [`sheet_lower::LoweredContent`] IR the host-model translator consumes
    /// (spec §8.2). Junk endpoints are a boundary error.
    pub fn get_range_lowered(
        &self,
        sheet: u16,
        range: &str,
        opts: LowerOptions,
    ) -> Result<sheet_lower::LoweredContent, SessionError> {
        let cell_range = parse_range(range)?;

        // Validate the sheet id (FREEZE AMENDMENT, audit finding 2). Lowering
        // an unknown sheet is itself harmless (it yields an empty-but-shaped
        // region), but the boundary rejects an OOB id for the same contract as
        // `set_cell`.
        let sheet_count = self
            .engine
            .as_ref()
            .expect("engine present")
            .model()
            .sheets
            .len();
        if (sheet as usize) >= sheet_count {
            return Err(SessionError(format!(
                "sheet id {sheet} out of range ({sheet_count} sheets)"
            )));
        }

        // Cap the materialized area BEFORE lowering (FREEZE AMENDMENT, audit
        // finding 1). u64 math so a full-sheet range (XFD1048576) cannot
        // overflow `rows * cols`.
        let (top, left, bottom, right) = (
            cell_range.r0.min(cell_range.r1) as u64,
            cell_range.c0.min(cell_range.c1) as u64,
            cell_range.r0.max(cell_range.r1) as u64,
            cell_range.c0.max(cell_range.c1) as u64,
        );
        let area = (bottom - top + 1) * (right - left + 1);
        if area > T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "range exceeds the T0 lowering cap ({T0_LOWER_CELL_CAP} cells)"
            )));
        }

        let view = ViewOptions {
            include_grid_rules: opts.include_grid_rules.unwrap_or(true),
            header_rows: opts.header_rows.unwrap_or(0),
        };
        let model = self.engine.as_ref().expect("engine present").model();
        Ok(lower_range(model, sheet, cell_range, &view))
    }

    /// Paginate `range` of `sheet` across `frames` (the host frame chain's
    /// content boxes; Wave 2D, S-05). Threads a tall range into the ordered
    /// frame list — rows that do not fit flow to the next frame, headers can
    /// repeat, keep-together blocks never split — and returns one self-contained
    /// [`Page`] per filled frame (each a [`sheet_lower::LoweredContent`] plus its
    /// `frame_index` + `continued`/`oversize` flags). Reuses the pure
    /// [`sheet_lower::paginate`]; this method is the same boundary-validated
    /// surface as [`get_range_lowered`](Self::get_range_lowered).
    ///
    /// Junk endpoints are a boundary error; an OOB sheet id is rejected
    /// (finding-2 discipline); the per-frame area is bounded by the SAME T0
    /// cell cap as single-frame lowering (the full range is lowered once
    /// internally, so the cap guards the same materialization).
    pub fn paginate(
        &self,
        sheet: u16,
        range: &str,
        frames: Vec<FrameBoxArg>,
        opts: PaginateOptionsArg,
    ) -> Result<Vec<Page>, SessionError> {
        let cell_range = parse_range(range)?;

        // Validate the sheet id (FREEZE AMENDMENT, audit finding 2 — matches
        // `set_cell`/`get_range_lowered`).
        let sheet_count = self
            .engine
            .as_ref()
            .expect("engine present")
            .model()
            .sheets
            .len();
        if (sheet as usize) >= sheet_count {
            return Err(SessionError(format!(
                "sheet id {sheet} out of range ({sheet_count} sheets)"
            )));
        }

        // Cap the materialized area BEFORE paginating (FREEZE AMENDMENT, audit
        // finding 1). `paginate` lowers the full range once internally, so the
        // same cap that guards `get_range_lowered` applies here. u64 math so a
        // full-sheet range cannot overflow `rows * cols`.
        let (top, left, bottom, right) = (
            cell_range.r0.min(cell_range.r1) as u64,
            cell_range.c0.min(cell_range.c1) as u64,
            cell_range.r0.max(cell_range.r1) as u64,
            cell_range.c0.max(cell_range.c1) as u64,
        );
        let area = (bottom - top + 1) * (right - left + 1);
        if area > T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "range exceeds the T0 lowering cap ({T0_LOWER_CELL_CAP} cells)"
            )));
        }

        let boxes: Vec<FrameBox> = frames
            .into_iter()
            .map(|f| FrameBox {
                width_pt: f.width_pt,
                height_pt: f.height_pt,
            })
            .collect();
        let paginate_opts = sheet_lower::PaginateOptions {
            repeated_header_rows: opts.repeated_header_rows.unwrap_or(0),
            continued_marker: opts.continued_marker.unwrap_or(false),
            keep_rows_together: opts.keep_rows_together.unwrap_or_default(),
        };

        let model = self.engine.as_ref().expect("engine present").model();
        Ok(lower_paginate(
            model,
            sheet,
            cell_range,
            &boxes,
            &paginate_opts,
        ))
    }

    /// Window a worksheet into a [`sheet_grid::GridScene`] for the sheets-mode
    /// vector grid surface (spec §8.1, S-02). The engine windows from the
    /// `(first_row, first_col)` scroll origin bounded by `(w_pt, h_pt)` and
    /// materializes only the visible populated cells (`sheet_grid::grid_scene`
    /// does the O(visible) windowing); the stored selection for THIS sheet
    /// (see [`set_grid_selection`](Self::set_grid_selection)) is folded into the
    /// returned scene. An OOB sheet id is a boundary error (finding-2 discipline,
    /// matching `set_cell`/`get_range_lowered`).
    pub fn get_grid_scene(
        &self,
        sheet: u16,
        first_row: u32,
        first_col: u32,
        w_pt: f64,
        h_pt: f64,
        opts: GridSceneOptions,
    ) -> Result<sheet_grid::GridScene, SessionError> {
        // Validate the sheet id (FREEZE AMENDMENT, audit finding 2). An unknown
        // sheet would otherwise yield an empty-but-shaped scene; the boundary
        // rejects an OOB id for the same contract as `set_cell`.
        let sheet_count = self
            .engine
            .as_ref()
            .expect("engine present")
            .model()
            .sheets
            .len();
        if (sheet as usize) >= sheet_count {
            return Err(SessionError(format!(
                "sheet id {sheet} out of range ({sheet_count} sheets)"
            )));
        }

        let grid_opts = sheet_grid::GridOptions {
            include_gridlines: opts.include_gridlines.unwrap_or(true),
        };
        let model = self.engine.as_ref().expect("engine present").model();
        let mut scene =
            sheet_grid::grid_scene(model, sheet, first_row, first_col, w_pt, h_pt, &grid_opts);

        // Fold in the session's stored selection for THIS sheet (spec §8.1 —
        // selection is engine state the panel requests, not scene-derived).
        // `grid_scene` always returns `selection: None` (Phase A); a selection
        // recorded for a different sheet is not shown here.
        if let Some((sel_sheet, sel)) = &self.selection {
            if *sel_sheet == sheet {
                scene.selection = Some(sheet_grid::GridSelection {
                    anchor_row: sel.anchor_row,
                    anchor_col: sel.anchor_col,
                    rows: sel.rows,
                    cols: sel.cols,
                });
            }
        }

        Ok(scene)
    }

    /// Record the sheets-mode selection rectangle for `sheet`, consumed by the
    /// next [`get_grid_scene`](Self::get_grid_scene) for the same sheet (spec
    /// §8.1 — selection is engine state, the panel only requests it). An OOB
    /// sheet id is a boundary error (finding-2 discipline).
    pub fn set_grid_selection(
        &mut self,
        sheet: u16,
        anchor_row: u32,
        anchor_col: u32,
        rows: u32,
        cols: u32,
    ) -> Result<(), SessionError> {
        let sheet_count = self
            .engine
            .as_ref()
            .expect("engine present")
            .model()
            .sheets
            .len();
        if (sheet as usize) >= sheet_count {
            return Err(SessionError(format!(
                "sheet id {sheet} out of range ({sheet_count} sheets)"
            )));
        }

        self.selection = Some((
            sheet,
            sheet_grid::GridSelection {
                anchor_row,
                anchor_col,
                rows,
                cols,
            },
        ));
        Ok(())
    }

    /// Enumerate the workbook's sheets (id, name, used extent). `rows`/`cols`
    /// are the 1-based extent of the populated range (0 when the sheet is
    /// empty).
    pub fn list_sheets(&self) -> Vec<SheetInfo> {
        let model = self.engine.as_ref().expect("engine present").model();
        model
            .sheets
            .iter()
            .enumerate()
            .map(|(i, ws)| {
                let (rows, cols) = match ws.used_range() {
                    Some(ur) => (ur.row1 + 1, ur.col1 + 1),
                    None => (0, 0),
                };
                SheetInfo {
                    id: i as u16,
                    name: ws.name.to_string(),
                    rows,
                    cols,
                }
            })
            .collect()
    }

    /// Enumerate the workbook's parsed charts (M2 charts track, spec §8.4):
    /// index (the [`get_chart_geometry`](Self::get_chart_geometry) handle), host
    /// sheet, kind, title, and series count. Empty for a workbook with no
    /// charts. The charts were parsed from the XLSX `xl/charts/chartN.xml` parts
    /// on load (`sheet_xlsx::XlsxDocument::charts`); the parts stay opaque /
    /// round-trip-preserved.
    pub fn list_charts(&self) -> Vec<ChartInfo> {
        self.doc
            .charts
            .iter()
            .enumerate()
            .map(|(i, c)| ChartInfo {
                index: i as u32,
                host_sheet: c.host_sheet,
                kind: chart_kind_tag(c.model.kind),
                title: c.model.title.as_ref().map(|t| t.to_string()),
                series_count: c.model.series.len() as u32,
            })
            .collect()
    }

    /// Resolve a parsed chart's series ranges to [`PlotData`] against the LIVE
    /// model (so the geometry is live to recalculation — spec §8.4) and call the
    /// PURE `sheet_chart::generate` for a `w_pt × h_pt` content box. `chart_index`
    /// is an index into [`list_charts`](Self::list_charts); an OOB index is a
    /// boundary error (finding-2 discipline, matching `set_cell`). The returned
    /// [`ChartGeometry`] is the same IR the page-surface paged.draw lowering AND
    /// the grid view consume — one generator, two projections.
    pub fn get_chart_geometry(
        &self,
        chart_index: u32,
        w_pt: f64,
        h_pt: f64,
    ) -> Result<ChartGeometry, SessionError> {
        let chart: &XlsxChart = self.doc.charts.get(chart_index as usize).ok_or_else(|| {
            SessionError(format!(
                "chart index {chart_index} out of range ({} charts)",
                self.doc.charts.len()
            ))
        })?;
        let model = self.engine.as_ref().expect("engine present").model();

        // Resolve each series' values range to a numeric vector, and the shared
        // category labels from the FIRST series that carries a category range.
        let mut data = PlotData::default();
        for series in &chart.model.series {
            data.series.push(resolve_values(model, &series.values));
        }
        if let Some(cat_ref) = chart
            .model
            .series
            .iter()
            .find_map(|s| s.categories.as_ref())
        {
            let ctx = FormatCtx::new(model.calc.date_system, model.calc.locale);
            data.categories = resolve_labels(model, cat_ref, &ctx);
        }

        Ok(generate_chart(&chart.model, &data, w_pt, h_pt))
    }

    /// Workbook metadata for the panel.
    pub fn metadata(&self) -> Metadata {
        let model = self.engine.as_ref().expect("engine present").model();
        let date_system = match model.calc.date_system {
            DateSystem::Date1900 => "1900",
            DateSystem::Date1904 => "1904",
        };
        Metadata {
            date_system,
            // "dirty" = unsaved edits pending (panel's save-button state). The
            // container's own dirty flag only flips at save time, so we track
            // the edited-cell set instead (cleared on a successful save).
            dirty: !self.edited.is_empty(),
            unparsed_formulas: self.unparsed_formulas,
        }
    }

    /// Update the `NOW`/`TODAY` serial (volatile reseed on the next recalc).
    pub fn set_now(&mut self, serial: f64) {
        self.config.now_serial = serial;
        if let Some(engine) = self.engine.as_mut() {
            engine.set_now(serial);
        }
    }
}

impl Default for SheetSession {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────────────────────────────────────────── helpers

/// Format one cell's stored value through its style's number format (spec §9).
/// Empty/out-of-range → `""`. A malformed stored format falls back to General
/// (never panics — mirrors the lower/format never-panic posture).
fn cell_display(
    model: &SheetModel,
    sheet: SheetId,
    row: u32,
    col: u32,
    cache: &mut FormatCache,
    ctx: &FormatCtx,
) -> String {
    let Some(ws) = model.sheet(sheet) else {
        return String::new();
    };
    let Some(cell) = ws.cell(row, col) else {
        return String::new();
    };
    if matches!(cell.value, CellValue::Empty) {
        return String::new();
    }
    let code = model.styles.num_fmt_of(cell.style);
    match cache.get(code) {
        Ok(fmt) => sheet_format::format_value(&cell.value, fmt, ctx),
        Err(_) => sheet_format::format_general(&cell.value),
    }
}

/// The lowercase kind tag for a [`sheet_chart::model::ChartKind`] — the same
/// space the geometry IR's `Primitive` tags / the registry use.
fn chart_kind_tag(kind: sheet_chart::model::ChartKind) -> &'static str {
    use sheet_chart::model::ChartKind::*;
    match kind {
        Column => "column",
        Bar => "bar",
        Line => "line",
        Area => "area",
        Pie => "pie",
        Donut => "donut",
        Scatter => "scatter",
    }
}

/// Resolve a series' values [`RangeRef`] to a numeric vector against the LIVE
/// model (chart geometry is live to recalc — spec §8.4). Cells iterate in
/// natural order (row-major), so a column range `$B$2:$B$4` and a row range
/// `$B$2:$D$2` both yield their values left-to-right / top-to-bottom. A numeric
/// cell contributes its value; a bool contributes 0/1; empty / text / error
/// cells contribute 0.0 (the publishing reading — a gap in a chart is a zero,
/// never a panic).
fn resolve_values(model: &SheetModel, range: &RangeRef) -> Vec<f64> {
    let r = range.normalized();
    let sheet = r.start.sheet;
    let mut out = Vec::new();
    for row in r.start.row..=r.end.row {
        for col in r.start.col..=r.end.col {
            let v = model
                .sheet(sheet)
                .and_then(|ws| ws.cell(row, col))
                .map(|c| match &c.value {
                    CellValue::Number(n) => *n,
                    CellValue::Bool(b) => {
                        if *b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => 0.0,
                })
                .unwrap_or(0.0);
            out.push(v);
        }
    }
    out
}

/// Resolve a category [`RangeRef`] to formatted label strings against the live
/// model (the category axis shows the cells' DISPLAY text — spec §9, the same
/// formatting the grid / page lower use). Empty cells become empty labels.
fn resolve_labels(model: &SheetModel, range: &RangeRef, ctx: &FormatCtx) -> Vec<String> {
    let r = range.normalized();
    let sheet = r.start.sheet;
    let mut cache = FormatCache::default();
    let mut out = Vec::new();
    for row in r.start.row..=r.end.row {
        for col in r.start.col..=r.end.col {
            out.push(cell_display(model, sheet, row, col, &mut cache, ctx));
        }
    }
    out
}

/// Parse a range string (`"A1:D9"` or a single cell `"A1"`) into a
/// [`CellRange`] using `sheet_core::parse_a1` on each endpoint.
fn parse_range(range: &str) -> Result<CellRange, SessionError> {
    let bad = || SessionError(format!("invalid range: {range:?}"));
    let (start, end) = match range.split_once(':') {
        Some((a, b)) => (a, b),
        None => (range, range), // single cell
    };
    let (r0, c0, _, _) = parse_a1(start).ok_or_else(bad)?;
    let (r1, c1, _, _) = parse_a1(end).ok_or_else(bad)?;
    Ok(CellRange { r0, c0, r1, c1 })
}

/// The embedded minimal one-sheet ("Sheet1") XLSX package for [`SheetSession::new`].
fn empty_workbook_bytes() -> Vec<u8> {
    use std::io::Write as _;
    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: &str, body: &str| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        add(
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        );
        add(
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        );
        add(
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        );
        add(
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        );
        add(
            "xl/worksheets/sheet1.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#,
        );
        zip.finish().unwrap();
    }
    buf
}

/// A [`ParseCtx`] over a [`SheetModel`]: resolves sheet names + defined names,
/// with the formula's home sheet as `current`. Mirrors `sheet-calc`'s internal
/// `ModelParseCtx` (which is private) so load-time parsing matches engine entry.
struct ModelParseCtx<'a> {
    model: &'a SheetModel,
    current: SheetId,
}

impl ParseCtx for ModelParseCtx<'_> {
    fn sheet_id(&self, name: &str) -> Option<SheetId> {
        self.model.sheet_id(name)
    }
    fn name_id(&self, name: &str) -> Option<sheet_core::NameId> {
        self.model.names.resolve(name, self.current)
    }
    fn current_sheet(&self) -> SheetId {
        self.current
    }
}

/// A [`SheetNames`] over a [`SheetModel`] for the printer (save path).
struct ModelSheetNames<'a> {
    model: &'a SheetModel,
}

impl SheetNames for ModelSheetNames<'_> {
    fn sheet_name(&self, id: SheetId) -> Option<&str> {
        self.model.sheet(id).map(|ws| ws.name.as_str())
    }
}
