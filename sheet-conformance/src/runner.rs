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

//! The ONE replay path for a golden formula case, shared by the hand-golden
//! gate (`tests/corpus_runner.rs`) and the Excel oracle lane
//! (`tests/excel_oracle.rs`). Both lanes must seed the same inputs and host
//! the formula in the same cell, or a disagreement between them would say
//! something about the harness rather than about the engine.
//!
//! The setup grammar (`text:` / `bool:` / `empty` / `@Addr` / `-`) is
//! documented on `tests/corpus_runner.rs`; the Excel generator
//! (`oracle/excel/oracle_cases.py`) mirrors it.

use crate::CorpusCase;
use sheet_calc::{Engine, EngineConfig, SetInput};
use sheet_core::{CellValue, SheetId, SheetModel};
use sheet_fn::coerce;

/// The single sheet every case runs on.
pub const SHEET: SheetId = 0;

/// A fresh one-sheet engine.
pub fn fresh_engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    Engine::new(m, EngineConfig::default())
}

/// 1-based A1 address (`B3`) → 0-based `(row, col)`.
pub fn parse_addr(addr: &str) -> (u32, u32) {
    let upper = addr.trim().to_ascii_uppercase();
    let split = upper
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or_else(|| panic!("bad A1 address {addr:?}"));
    let (col_s, row_s) = upper.split_at(split);
    let col = sheet_core::a1_to_col(col_s).unwrap_or_else(|| panic!("bad column in {addr:?}"));
    let row: u32 = row_s
        .parse()
        .unwrap_or_else(|_| panic!("bad row in {addr:?}"));
    (row - 1, col)
}

/// Apply one setup `(addr, raw)` seed: `empty` = blank, `text:`/`bool:` force
/// the type, anything else goes through `enter`'s literal detection.
pub fn apply_setup(e: &mut Engine, addr: &str, raw: &str, case_id: &str) {
    let (row, col) = parse_addr(addr);
    if raw == "empty" {
        e.set_cell(SHEET, row, col, SetInput::Empty);
    } else if let Some(rest) = raw.strip_prefix("text:") {
        e.set_cell(
            SHEET,
            row,
            col,
            SetInput::Value(CellValue::Text(rest.into())),
        );
    } else if let Some(rest) = raw.strip_prefix("bool:") {
        let b = match rest.trim().to_ascii_uppercase().as_str() {
            "TRUE" => true,
            "FALSE" => false,
            other => panic!("[{case_id}] bad bool: setup value {other:?}"),
        };
        e.set_cell(SHEET, row, col, SetInput::Value(CellValue::Bool(b)));
    } else {
        e.enter(SHEET, row, col, raw)
            .unwrap_or_else(|err| panic!("[{case_id}] setup {addr}={raw:?} parse error: {err:?}"));
    }
}

/// Top-level `;` → `,` (the engine's en-US dialect); `;` inside a quoted
/// string is kept, and so is `;` inside an array literal `{1,2;3,4}`, where it
/// IS the row separator (rewriting it turned a 2x2 constant into a 1x4 one).
/// Iterates chars so non-ASCII literals survive.
pub fn normalize_separators(formula: &str) -> String {
    let mut out = String::with_capacity(formula.len());
    let mut in_string = false;
    let mut braces = 0usize;
    let mut chars = formula.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            if in_string && chars.peek() == Some(&'"') {
                out.push('"');
                out.push('"');
                chars.next();
                continue;
            }
            in_string = !in_string;
            out.push('"');
        } else if ch == '{' && !in_string {
            braces += 1;
            out.push(ch);
        } else if ch == '}' && !in_string {
            braces = braces.saturating_sub(1);
            out.push(ch);
        } else if ch == ';' && !in_string && braces == 0 {
            out.push(',');
        } else {
            out.push(ch);
        }
    }
    out
}

/// The General projection the goldens assert: the error token for an error,
/// `coerce::to_text` for everything else.
pub fn project(value: &CellValue) -> String {
    match value {
        CellValue::Error(e) => e.as_str().to_string(),
        other => coerce::to_text(other).to_string(),
    }
}

/// Replay one case: fresh engine, setup, formula at the host cell (Z99 unless
/// an `@Addr` seed says otherwise), recalc. Returns the host cell's value, or
/// the parse error as text.
pub fn run_case(case: &CorpusCase) -> Result<CellValue, String> {
    let mut e = fresh_engine();
    let (mut frow, mut fcol) = (98u32, 25u32);
    for (addr, raw) in &case.setup {
        if addr == "-" {
            continue;
        }
        if let Some(host) = addr.strip_prefix('@') {
            (frow, fcol) = parse_addr(host);
            continue;
        }
        apply_setup(&mut e, addr, raw, &case.id);
    }
    let formula = normalize_separators(&case.formula);
    e.enter(SHEET, frow, fcol, &formula)
        .map_err(|err| format!("parse error: {err:?}"))?;
    Ok(e.model()
        .sheet(SHEET)
        .and_then(|ws| ws.cell(frow, fcol))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_only_top_level_semicolons() {
        assert_eq!(normalize_separators("=IF(A1;1;2)"), "=IF(A1,1,2)");
        assert_eq!(
            normalize_separators("=CONCAT(\"a;b\";\"c\")"),
            "=CONCAT(\"a;b\",\"c\")"
        );
        assert_eq!(normalize_separators("=SUM(A1,A2)"), "=SUM(A1,A2)");
        // `;` inside an array literal is the ROW separator — kept.
        assert_eq!(
            normalize_separators("=SUM({1,2;3,4};5)"),
            "=SUM({1,2;3,4},5)"
        );
    }

    #[test]
    fn addr_parsing() {
        assert_eq!(parse_addr("A1"), (0, 0));
        assert_eq!(parse_addr("B3"), (2, 1));
        assert_eq!(parse_addr("Z99"), (98, 25));
    }
}
