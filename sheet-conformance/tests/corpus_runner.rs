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

use sheet_conformance::runner::{project, run_case as replay};
use sheet_conformance::{load_corpus, CorpusCase};

/// The outcome of running one corpus case.
enum Outcome {
    /// The projection matched the golden.
    Pass,
    /// A mismatch / parse failure — carries a diagnostic.
    Fail(String),
}

/// Run one corpus case through the shared replay path
/// ([`sheet_conformance::runner::run_case`]) and compare the projection
/// against `expected`.
fn run_case(case: &CorpusCase) -> Outcome {
    let value = match replay(case) {
        Ok(v) => v,
        Err(err) => {
            return Outcome::Fail(format!(
                "[{}] formula {:?} {err}",
                case.id, case.formula
            ))
        }
    };
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

// ── Wave 7 — the dynamic-array family (now that a nested array reaches its
// caller, INDEX/ROWS/SUM over an array result project a checkable scalar) and
// the LET/LAMBDA family.

#[test]
fn sheet_calc_corpus_array() {
    run_family("array");
}

#[test]
fn sheet_calc_corpus_lambda() {
    run_family("lambda");
}
