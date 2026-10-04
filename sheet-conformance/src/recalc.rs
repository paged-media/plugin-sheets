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

//! Recalculate a real workbook and compare every formula cell with the value
//! the PRODUCER cached in `<v>` — Excel's own answer, recorded in the file.
//!
//! Shared by the CI lane over the committed subset (`corpus/xlsx-recalc/`)
//! and the opt-in full lane over the private corpus (`PAGED_XLSX_CORPUS`),
//! both in `tests/xlsx_recalc_corpus.rs`. The engine values come from the
//! product loader itself (`sheet-js`'s `SheetSession::load_xlsx`); this
//! module keeps the cached values that load overwrites.
//!
//! Every formula cell lands in exactly one class, so a report adds up:
//! `agree`, `differs`, `unparsed` (the parser refused the text), `volatile`
//! (NOW/RAND/OFFSET/INDIRECT/... — a cached value is a moment, not a fact),
//! `no-cache` (the file carries no `<v>`).

use std::collections::BTreeMap;

use sheet_core::{CellValue, SheetId, SheetModel};
use sheet_js::core::SheetSession;
use sheet_xlsx::XlsxDocument;

/// Functions whose cached value is not reproducible by recomputation.
pub const VOLATILE: &[&str] = &[
    "NOW",
    "TODAY",
    "RAND",
    "RANDBETWEEN",
    "RANDARRAY",
    "OFFSET",
    "INDIRECT",
    "CELL",
    "INFO",
];

/// The outcome class of one formula cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Recomputed value matches the cached one.
    Agree,
    /// Recomputed value differs from the cached one.
    Differs,
    /// The parser refused the formula text.
    Unparsed,
    /// A volatile function — not comparable.
    Volatile,
    /// The file has no cached value for the cell.
    NoCache,
}

/// One formula cell's result.
#[derive(Debug, Clone)]
pub struct CellOutcome {
    /// Sheet name.
    pub sheet: String,
    /// 0-based (row, col).
    pub at: (u32, u32),
    /// Formula text as stored (no `=`).
    pub formula: String,
    /// The class.
    pub class: Class,
    /// Cached value (Excel's).
    pub cached: CellValue,
    /// Recomputed value (the engine's).
    pub engine: CellValue,
}

/// Excel's 15-significant-digit view.
fn sig15(x: f64) -> String {
    format!("{x:.14e}")
}

/// The oracle's agreement rule for numbers (relative 1e-12 or 15 sig. digits).
pub fn numbers_agree(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs()) || sig15(a) == sig15(b)
}

/// Value agreement: numbers by [`numbers_agree`], everything else exact. An
/// empty-string text result and an empty cell agree (Excel caches `""` for a
/// formula that yields nothing to show).
pub fn values_agree(engine: &CellValue, cached: &CellValue) -> bool {
    match (engine, cached) {
        (CellValue::Number(a), CellValue::Number(b)) => numbers_agree(*a, *b),
        (CellValue::Empty, CellValue::Text(t)) | (CellValue::Text(t), CellValue::Empty) => {
            t.is_empty()
        }
        (a, b) => a == b,
    }
}

/// Upper-cased function names called in a formula text (outside strings).
pub fn functions_in(formula: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_str = false;
    let mut word = String::new();
    for ch in formula.chars() {
        if ch == '"' {
            in_str = !in_str;
            word.clear();
            continue;
        }
        if in_str {
            continue;
        }
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            word.push(ch);
        } else {
            if ch == '(' && !word.is_empty() {
                let name = word.to_ascii_uppercase();
                let name = name
                    .trim_start_matches("_XLFN._XLWS.")
                    .trim_start_matches("_XLFN.")
                    .to_string();
                out.push(name);
            }
            word.clear();
        }
    }
    out
}

/// Recalculate `bytes` and classify every formula cell. `Err` = the package
/// does not open (a separate lane's concern).
///
/// The engine side is the PRODUCT load path, `SheetSession::load_xlsx`
/// (formula parse, name-target resolution, recalc) — a lane with its own
/// copy of that path measured a loader nobody ships. The cached side is the
/// same package opened once more by the xlsx reader, before any recalc.
pub fn recalc_workbook(bytes: &[u8]) -> Result<Vec<CellOutcome>, String> {
    let doc = XlsxDocument::open(bytes).map_err(|e| e.to_string())?;
    let session = SheetSession::load_xlsx(bytes).map_err(|e| e.to_string())?;
    let texts: Vec<((SheetId, u32, u32), String)> = doc
        .formula_texts
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    let cell = |m: &SheetModel, (s, r, c): (SheetId, u32, u32)| {
        m.sheet(s).and_then(|ws| ws.cell(r, c)).cloned()
    };
    let cached: BTreeMap<(SheetId, u32, u32), CellValue> = texts
        .iter()
        .map(|(k, _)| {
            let v = cell(&doc.model, *k).map(|c| c.value).unwrap_or(CellValue::Empty);
            (*k, v)
        })
        .collect();
    let model = session.model();
    // A formula text the loader could not parse leaves a value cell.
    let unparsed: std::collections::BTreeSet<(SheetId, u32, u32)> = texts
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| cell(model, *k).is_none_or(|c| c.formula.is_none()))
        .collect();

    let mut out = Vec::with_capacity(texts.len());
    for ((sheet, row, col), text) in texts {
        let key = (sheet, row, col);
        let cached_v = cached.get(&key).cloned().unwrap_or(CellValue::Empty);
        let engine_v = model
            .sheet(sheet)
            .and_then(|ws| ws.cell(row, col))
            .map(|c| c.value.clone())
            .unwrap_or(CellValue::Empty);
        let fns = functions_in(&text);
        let class = if unparsed.contains(&key) {
            Class::Unparsed
        } else if fns.iter().any(|f| VOLATILE.contains(&f.as_str())) {
            Class::Volatile
        } else if matches!(cached_v, CellValue::Empty) {
            Class::NoCache
        } else if values_agree(&engine_v, &cached_v) {
            Class::Agree
        } else {
            Class::Differs
        };
        out.push(CellOutcome {
            sheet: model
                .sheet(sheet)
                .map(|ws| ws.name.to_string())
                .unwrap_or_default(),
            at: (row, col),
            formula: text,
            class,
            cached: cached_v,
            engine: engine_v,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functions_are_found_outside_strings() {
        assert_eq!(
            functions_in("SUM(A1:A3)+_xlfn.IFS(B1,\"MAX(\",1,2)"),
            vec!["SUM".to_string(), "IFS".to_string()]
        );
    }

    #[test]
    fn agreement_rules() {
        assert!(numbers_agree(0.1 + 0.2, 0.3));
        assert!(!numbers_agree(1.0, 1.0001));
        assert!(values_agree(&CellValue::Empty, &CellValue::Text("".into())));
    }
}
