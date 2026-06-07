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

//! LibreOffice Calc differential-oracle SKELETON (spec §12.4).
//!
//! The primary differential oracle runs headless LibreOffice in a CI
//! container: workbooks are generated per golden-corpus case, evaluated by
//! `soffice`, the resulting values are extracted, and diffed against the
//! engine's own output. Per §3 ("Oracle use") behaviour is not
//! copyrightable and LibreOffice is *not linked* — it is invoked as an
//! external process, so this lane stays clean-room.
//!
//! These tests are DOUBLY guarded so the default `cargo test` lane never
//! touches `soffice`:
//!
//! 1. `#[ignore]` — excluded unless run with `-- --ignored`.
//! 2. an early `return` unless `PAGED_SHEET_ORACLE` is set in the env.
//!
//! Wiring (`PAGED_SHEET_ORACLE=1 cargo test -p sheet-conformance --
//! --ignored`) belongs to the CI container that ships `soffice`; the
//! `todo!()` below the env gate marks the unimplemented harness body so an
//! accidental local run fails loudly rather than silently passing. These
//! are harness plumbing, NOT registry-claimed rows — no `status` flips
//! hang off them.

/// Has the operator opted into the LibreOffice oracle lane?
fn oracle_enabled() -> bool {
    std::env::var("PAGED_SHEET_ORACLE").is_ok()
}

/// Differential oracle for the **function** corpora
/// (`corpus/fn-corpus/**`).
///
/// Intended harness (§12.4):
/// 1. `sheet_conformance::load_corpus(...)` every `fn-corpus/*.golden.tsv`.
/// 2. For each [`CorpusCase`](sheet_conformance::CorpusCase), emit a
///    minimal `.xlsx`/`.fods` workbook: seed `setup` cells, place
///    `formula` in a probe cell.
/// 3. `soffice --headless --convert-to csv --outdir <tmp> <workbook>`.
/// 4. Read back the probe cell from the CSV; compare to BOTH the golden
///    `expected` AND this engine's computed value (a three-way check once
///    IronCalc joins — §12.4 "Disagreement protocol").
/// 5. On disagreement, record the behaviours and the chosen convention as
///    a registry ruling; never silently coerce to match.
#[test]
#[ignore = "LibreOffice oracle: PAGED_SHEET_ORACLE=1 + soffice in a CI container"]
fn sheet_oracle_functions() {
    if !oracle_enabled() {
        // Default lane: env not set ⇒ no-op (also `#[ignore]`d).
        return;
    }
    todo!(
        "wire the fn-corpus oracle harness: load_corpus → generate workbooks → \
         soffice --headless --convert-to csv → diff values (§12.4)"
    );
}

/// Differential oracle for the **number-format / date-serial** corpora
/// (`corpus/format-corpus/**`).
///
/// Same shape as [`sheet_oracle_functions`], but the probe asserts the
/// *displayed* string under an ECMA-376 number-format code (and the
/// 1900/1904 date systems incl. the leap-year-bug serial 60, §9 / §3
/// "Bug-for-bug rulings"), which is exactly where engines diverge most.
#[test]
#[ignore = "LibreOffice oracle: PAGED_SHEET_ORACLE=1 + soffice in a CI container"]
fn sheet_oracle_formats() {
    if !oracle_enabled() {
        return;
    }
    todo!(
        "wire the format-corpus oracle harness: load_corpus → generate workbooks with \
         number-format codes → soffice convert-to csv → diff displayed strings (§12.4)"
    );
}
