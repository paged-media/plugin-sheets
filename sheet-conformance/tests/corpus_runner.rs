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

//! THE end-to-end calc gate (spec §12.4). Walks the formula-level golden
//! corpora under `corpus/fn-corpus/<family>/*.golden.tsv` and replays every
//! case through the FROZEN [`sheet_calc::Engine`] — the SAME path `sheet-js`
//! drives: fresh one-sheet engine, seed the setup via [`Engine::enter`], enter
//! the formula at an unused cell, recalc, and compare the General projection
//! ([`sheet_fn::coerce::to_text`]) against the golden `expected` (an error
//! literal compares against the stored `CellValue::Error`'s `as_str`).
//!
//! One `#[test]` per family directory (`sheet_calc_corpus_<family>`) so a
//! failure localizes to one family. Only the seven FORMULA families are
//! replayed; the `coerce/` directory holds 3-column coercion unit fixtures (a
//! different schema) consumed by `tests/coerce.rs`, not formula cases.
//!
//! ## Two authoring-dialect accommodations (engine is right; goldens vary)
//!
//! 1. **Argument separator.** The engine's parser is the en-US dialect: `,`
//!    separates function arguments; `;` is ONLY an array-row separator (a
//!    documented `sheet-parser` ruling). Some logical-family goldens were
//!    authored with `;` as the argument separator (against a Python mirror).
//!    Rather than rewrite those goldens (the per-family `tests/fn_logical.rs`
//!    decoder reads them with its own `;`-split), this gate NORMALIZES a
//!    top-level `;` to `,` before entering — so the canonical engine evaluates
//!    them under its real dialect. `;` inside a quoted string is left intact.
//! 2. **Typed setup tags.** A setup value may carry a `text:` or `bool:`
//!    prefix forcing the cell's type (so `text:123` stays Text, not Number).
//!    Stripped + applied as a typed value; a bare value goes through `enter`'s
//!    Excel-like literal detection.
//!
//! ## Two more corpus authoring conventions
//!
//! - A setup column of a lone `-` means "no setup" (the agg/math families use
//!   `-` as the empty-setup sentinel; `load_corpus` hands it back as a seed
//!   with address `-`, which this runner skips).
//! - A setup token `@<Addr>` (e.g. `@D11`) declares the formula's HOST cell —
//!   where `ROW()`/`COLUMN()` with no argument evaluate. The lookup family uses
//!   it so a no-arg reference function has a known anchor. The runner places
//!   the formula at that cell instead of the default Z99.

use sheet_calc::{Engine, EngineConfig, SetInput};
use sheet_conformance::{load_corpus, CorpusCase};
use sheet_core::{CellValue, SheetId, SheetModel};
use sheet_fn::coerce;

/// The single column-0 sheet id every case runs on.
const SHEET: SheetId = 0;

/// Build a fresh one-sheet engine.
fn fresh_engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    Engine::new(m, EngineConfig::default())
}

/// 1-based A1 cell address (`B3`) → 0-based `(row, col)`. Uppercased input.
fn parse_addr(addr: &str) -> (u32, u32) {
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

/// Apply one setup `(addr, raw)` seed. A `text:`/`bool:` prefix forces the
/// cell type via `set_cell`; otherwise the raw value goes through `enter`'s
/// Excel-like literal detection.
fn apply_setup(e: &mut Engine, addr: &str, raw: &str, case_id: &str) {
    let (row, col) = parse_addr(addr);
    if raw == "empty" {
        // The corpus `empty` sentinel = a genuinely BLANK cell (not the text
        // "empty"). Matches the coerce-corpus `empty -> CellValue::Empty` rule.
        e.set_cell(SHEET, row, col, SetInput::Empty);
    } else if let Some(rest) = raw.strip_prefix("text:") {
        // Forced text — even numeric-looking strings stay Text.
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
        // Bare value: Excel-like literal detection (number/bool/error/blank/text).
        e.enter(SHEET, row, col, raw)
            .unwrap_or_else(|err| panic!("[{case_id}] setup {addr}={raw:?} parse error: {err:?}"));
    }
}

/// Normalize a formula's argument separator: convert a TOP-LEVEL `;` to `,`
/// (the engine's en-US dialect), leaving `;` inside quoted strings intact.
/// String literals in the dialect are double-quoted with `""` escaping.
fn normalize_separators(formula: &str) -> String {
    // Iterate CHARS, not bytes: a non-ASCII string literal (e.g. UNICODE("€"))
    // must reach the engine intact — byte iteration would split the multi-byte
    // char into Latin-1 fragments. Only the ASCII `"` and `;` are significant.
    let mut out = String::with_capacity(formula.len());
    let mut in_string = false;
    let mut chars = formula.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            // A doubled quote inside a string is an escaped quote; copy both.
            if in_string && chars.peek() == Some(&'"') {
                out.push('"');
                out.push('"');
                chars.next();
                continue;
            }
            in_string = !in_string;
            out.push('"');
        } else if ch == ';' && !in_string {
            out.push(',');
        } else {
            out.push(ch);
        }
    }
    out
}

/// The display projection: the General text of a value, or the error token for
/// an error value (matching the golden `expected` grammar).
fn project(value: &CellValue) -> String {
    match value {
        CellValue::Error(e) => e.as_str().to_string(),
        other => coerce::to_text(other).to_string(),
    }
}

/// The outcome of running one corpus case.
enum Outcome {
    /// The projection matched the golden.
    Pass,
    /// A mismatch / parse failure — carries a diagnostic.
    Fail(String),
}

/// Run one corpus case: fresh engine, setup, enter the formula at the host
/// cell, recalc, compare the projection against `expected`.
fn run_case(case: &CorpusCase) -> Outcome {
    let mut e = fresh_engine();

    // Default formula host: Z99 = col 25, row 98 (0-based) — outside every
    // corpus setup (max D5). A `@Addr` setup token overrides this.
    let (mut frow, mut fcol) = (98u32, 25u32);

    for (addr, raw) in &case.setup {
        // `-` is the empty-setup sentinel (agg/math families).
        if addr == "-" {
            continue;
        }
        // `@Addr` declares the formula's host cell (ROW()/COLUMN() anchor).
        if let Some(host) = addr.strip_prefix('@') {
            let (hr, hc) = parse_addr(host);
            frow = hr;
            fcol = hc;
            continue;
        }
        apply_setup(&mut e, addr, raw, &case.id);
    }

    let formula = normalize_separators(&case.formula);
    if let Err(err) = e.enter(SHEET, frow, fcol, &formula) {
        return Outcome::Fail(format!(
            "[{}] formula {:?} parse error: {err:?}",
            case.id, case.formula
        ));
    }

    let value = e
        .model()
        .sheet(SHEET)
        .and_then(|ws| ws.cell(frow, fcol))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty);

    let got = project(&value);
    if got == case.expected {
        Outcome::Pass
    } else {
        Outcome::Fail(format!(
            "[{}] {} (setup {:?}) -> got {:?}, want {:?}",
            case.id, case.formula, case.setup, got, case.expected
        ))
    }
}

/// Load + replay every `.golden.tsv` in a family directory, collecting all
/// mismatches so one run reports the full set (not just the first failure).
fn run_family(family: &str) {
    let dir = sheet_conformance::corpus_root()
        .join("fn-corpus")
        .join(family);
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read corpus dir {}: {e}", dir.display()))
        .filter_map(|ent| ent.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".golden.tsv"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no .golden.tsv files in {}",
        dir.display()
    );

    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;
    for file in &files {
        // The repo-relative path is what `load_corpus` wants.
        let rel = format!(
            "corpus/fn-corpus/{}/{}",
            family,
            file.file_name().unwrap().to_string_lossy()
        );
        for case in load_corpus(&rel) {
            total += 1;
            match run_case(&case) {
                Outcome::Pass => {}
                Outcome::Fail(msg) => failures.push(format!("{rel}: {msg}")),
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} corpus: {}/{} case(s) failed:\n{}",
        family,
        failures.len(),
        total,
        failures.join("\n")
    );
}

#[test]
fn sheet_calc_corpus_agg() {
    run_family("agg");
}

#[test]
fn sheet_calc_corpus_date() {
    run_family("date");
}

#[test]
fn sheet_calc_corpus_info() {
    run_family("info");
}

#[test]
fn sheet_calc_corpus_logical() {
    run_family("logical");
}

#[test]
fn sheet_calc_corpus_lookup() {
    run_family("lookup");
}

#[test]
fn sheet_calc_corpus_math() {
    run_family("math");
}

#[test]
fn sheet_calc_corpus_text() {
    run_family("text");
}

// ── M1 T1 families — the same end-to-end calc gate over the T1 goldens.
// Each family's kernels are also direct-dispatch tested in fn_<family>.rs;
// these replay the goldens through the full parse -> calc -> fn -> format
// path so the e2e projection is conformance-verified too.

#[test]
fn sheet_calc_corpus_stat() {
    run_family("stat");
}

#[test]
fn sheet_calc_corpus_fin() {
    run_family("fin");
}

#[test]
fn sheet_calc_corpus_text2() {
    run_family("text2");
}

#[test]
fn sheet_calc_corpus_date2() {
    run_family("date2");
}

#[test]
fn sheet_calc_corpus_math2() {
    run_family("math2");
}

#[test]
fn sheet_calc_corpus_logical2() {
    run_family("logical2");
}

#[test]
fn sheet_calc_corpus_info2() {
    run_family("info2");
}

#[test]
fn sheet_calc_corpus_lookup2() {
    run_family("lookup2");
}

#[test]
fn sheet_calc_corpus_database() {
    run_family("database");
}

#[test]
fn sheet_calc_corpus_t2misc() {
    run_family("t2misc");
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn normalize_only_top_level_semicolons() {
        assert_eq!(normalize_separators("=IF(A1;1;2)"), "=IF(A1,1,2)");
        // `;` inside a string is preserved.
        assert_eq!(
            normalize_separators("=CONCAT(\"a;b\";\"c\")"),
            "=CONCAT(\"a;b\",\"c\")"
        );
        // No semicolons -> unchanged.
        assert_eq!(normalize_separators("=SUM(A1,A2)"), "=SUM(A1,A2)");
    }

    #[test]
    fn addr_parsing() {
        assert_eq!(parse_addr("A1"), (0, 0));
        assert_eq!(parse_addr("B3"), (2, 1));
        assert_eq!(parse_addr("Z99"), (98, 25));
    }
}
