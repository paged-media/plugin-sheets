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

// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
#![allow(non_snake_case)]

//! The EXCEL differential oracle (spec §12.4) — replaces the LibreOffice
//! `todo!()` skeleton that `tests/oracle.rs` used to carry.
//!
//! Every formula golden under `corpus/fn-corpus/` was written into a workbook,
//! recalculated by desktop Microsoft Excel and read back
//! (`oracle/excel/{generate.py,drive.sh,read.py}` — a maintainer step on a Mac
//! with Excel). The values Excel computed are committed as
//! `oracle/excel/recorded/<family>.tsv`, so THIS lane needs no Excel and runs
//! in CI: it replays each case through the same path as the hand-golden gate
//! ([`sheet_conformance::runner::run_case`]) and compares with what Excel said.
//!
//! ## Agreement rules
//!
//! - number vs number: equal, or within a relative 1e-12, or equal at Excel's
//!   15 significant digits (dates and times are serial numbers and compare by
//!   the same rule — there is no separate date type in a cell).
//! - text vs text, bool vs bool: exact.
//! - error vs error: the same error token.
//! - an empty result agrees only with an empty Excel cell.
//!
//! ## Disagreements are LISTED, and the list is checked both ways
//!
//! `oracle/excel/divergences.tsv` names every case where the engine and Excel
//! disagree, with BOTH values and a kind:
//!
//! - `defect` — the engine is wrong; fixing it must also delete the row.
//! - `diverges` — a deliberate, recorded ruling (with its reason).
//!
//! The lane fails on an UNLISTED disagreement (a new regression or a new
//! case), on a listed row whose values no longer match what the engine or
//! Excel now produce, and on a listed row that now AGREES (the fix must
//! remove it). There is no `#[ignore]` anywhere in this lane.
//!
//! Cases Excel cannot take at all — a formula its parser rejects
//! (`excel-rejects.tsv`) or a setup it cannot store (a literal `#SPILL!`) —
//! are recorded as `rejected` / `unrepresentable` and reported, never
//! counted as agreement.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use sheet_conformance::runner::run_case;
use sheet_conformance::{load_corpus, CorpusCase};
use sheet_core::CellValue;

fn oracle_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("oracle/excel")
}

/// One recorded Excel result.
struct Recorded {
    formula: String,
    kind: String,
    value: String,
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `recorded/<family>.tsv` keyed by `(file, id)`.
fn load_recorded(family: &str) -> BTreeMap<(String, String), Recorded> {
    let path = oracle_dir().join("recorded").join(format!("{family}.tsv"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols.len(), 5, "{}: bad row {line:?}", path.display());
        let key = (cols[0].to_string(), cols[1].to_string());
        let prev = out.insert(
            key.clone(),
            Recorded {
                formula: unescape(cols[2]),
                kind: cols[3].to_string(),
                value: unescape(cols[4]),
            },
        );
        assert!(prev.is_none(), "{}: duplicate key {key:?}", path.display());
    }
    out
}

/// The fn-corpus formula families (every dir but the 3-column `coerce/`;
/// `recorded/format.tsv` belongs to the format lane below).
fn families() -> Vec<String> {
    let root = sheet_conformance::corpus_root().join("fn-corpus");
    let mut out: Vec<String> = std::fs::read_dir(&root)
        .expect("fn-corpus dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "coerce")
        .collect();
    out.sort();
    out
}

/// Every golden case of a family, as `(file, case)`, in corpus order.
fn family_cases(family: &str) -> Vec<(String, CorpusCase)> {
    let dir = sheet_conformance::corpus_root()
        .join("fn-corpus")
        .join(family);
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("family dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".golden.tsv"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        for c in load_corpus(&format!("corpus/fn-corpus/{family}/{f}")) {
            out.push((f.clone(), c));
        }
    }
    out
}

/// The engine value as a typed token, the vocabulary `divergences.tsv` uses.
fn engine_token(v: &Result<CellValue, String>) -> String {
    match v {
        Err(e) => format!("parse-error:{e}"),
        Ok(CellValue::Empty) => "blank".into(),
        Ok(CellValue::Number(n)) => format!("n:{n}"),
        Ok(CellValue::Text(t)) => format!("s:{t}"),
        Ok(CellValue::Bool(b)) => format!("b:{}", if *b { "TRUE" } else { "FALSE" }),
        Ok(CellValue::Error(e)) => format!("e:{}", e.as_str()),
    }
}

/// The recorded Excel value as a typed token.
fn excel_token(r: &Recorded) -> String {
    match r.kind.as_str() {
        "blank" => "blank".into(),
        "b" => format!("b:{}", if r.value == "1" { "TRUE" } else { "FALSE" }),
        k => format!("{k}:{}", r.value),
    }
}

/// Excel's 15-significant-digit view of a number.
fn sig15(x: f64) -> String {
    format!("{x:.14e}")
}

fn numbers_agree(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs()) || sig15(a) == sig15(b)
}

/// Do the engine and Excel agree under the rules in the module docs?
fn agrees(engine: &Result<CellValue, String>, excel: &Recorded) -> bool {
    let Ok(v) = engine else { return false };
    match (v, excel.kind.as_str()) {
        (CellValue::Number(n), "n") => excel
            .value
            .parse::<f64>()
            .is_ok_and(|x| numbers_agree(*n, x)),
        (CellValue::Text(t), "s") => t.as_ref() as &str == excel.value,
        (CellValue::Bool(b), "b") => (excel.value == "1") == *b,
        (CellValue::Error(e), "e") => e.as_str() == excel.value,
        (CellValue::Empty, "blank") => true,
        _ => false,
    }
}

/// A `divergences.tsv` row.
struct Listed {
    kind: String,
    engine: String,
    excel: String,
}

fn load_divergences(list: &str) -> BTreeMap<(String, String), Listed> {
    let path = oracle_dir().join(list);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert!(
            cols.len() >= 6,
            "{}: want file, id, kind, engine, excel, reason: {line:?}",
            path.display()
        );
        assert!(
            cols[2] == "defect" || cols[2] == "diverges",
            "{}: kind must be defect|diverges: {line:?}",
            path.display()
        );
        assert!(
            !cols[5].trim().is_empty(),
            "{}: a listed disagreement needs a reason: {line:?}",
            path.display()
        );
        let key = (cols[0].to_string(), cols[1].to_string());
        assert!(
            !out.contains_key(&key),
            "{}: {key:?} is listed twice",
            path.display()
        );
        out.insert(
            key,
            Listed {
                kind: cols[2].to_string(),
                engine: unescape(cols[3]),
                excel: unescape(cols[4]),
            },
        );
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// The recordings must cover EXACTLY the live corpus, formula for formula —
/// otherwise the oracle is comparing against cases that no longer exist, or
/// silently skipping new ones. A changed golden formula means re-record.
#[test]
fn excel_recordings_match_the_live_corpus__feat__sheet_fn_library() {
    let mut problems = Vec::new();
    let mut total = 0usize;
    for fam in families() {
        let rec = load_recorded(&fam);
        let mut seen = BTreeSet::new();
        for (file, c) in family_cases(&fam) {
            total += 1;
            let key = (file.clone(), c.id.clone());
            match rec.get(&key) {
                None => problems.push(format!("{fam}/{file} {}: not recorded", c.id)),
                Some(r) if r.formula != c.formula => problems.push(format!(
                    "{fam}/{file} {}: STALE — recorded {:?}, corpus now {:?}",
                    c.id, r.formula, c.formula
                )),
                Some(_) => {}
            }
            seen.insert(key);
        }
        for key in rec.keys() {
            if !seen.contains(key) {
                problems.push(format!(
                    "{fam}: recorded {key:?} is no longer in the corpus"
                ));
            }
        }
    }
    assert!(
        total > 1000,
        "only {total} golden cases found — corpus walk broken?"
    );
    assert!(
        problems.is_empty(),
        "{} recording problem(s) — re-run oracle/excel (generate.py, drive.sh, read.py):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// One case ready to compare: where it came from, what the engine produced.
struct Compared {
    family: String,
    file: String,
    id: String,
    formula: String,
    engine: Result<CellValue, String>,
}

/// Compare engine results against the recordings, check the listed
/// disagreements both ways, print the per-family table and return problems.
fn compare(label: &str, list: &str, cases: Vec<Compared>) -> Vec<String> {
    let listed = load_divergences(list);
    let mut used: BTreeSet<(String, String)> = BTreeSet::new();
    let mut problems: Vec<String> = Vec::new();
    let mut unlisted_rows: Vec<String> = Vec::new();
    let (mut compared, mut agree, mut not_comparable) = (0usize, 0usize, 0usize);
    let (mut defects, mut diverges) = (0usize, 0usize);
    let mut per_family: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut recordings: BTreeMap<String, BTreeMap<(String, String), Recorded>> = BTreeMap::new();

    for c in cases {
        let rec = recordings
            .entry(c.family.clone())
            .or_insert_with(|| load_recorded(&c.family));
        let key = (c.file.clone(), c.id.clone());
        let Some(r) = rec.get(&key) else { continue };
        if r.kind == "rejected" || r.kind == "unrepresentable" {
            not_comparable += 1;
            continue;
        }
        compared += 1;
        let fam = per_family.entry(c.family.clone()).or_default();
        fam.1 += 1;
        let ok = agrees(&c.engine, r);
        let (et, xt) = (engine_token(&c.engine), excel_token(r));
        let (family, file, id) = (&c.family, &c.file, &c.id);
        match (ok, listed.get(&key)) {
            (true, None) => {
                agree += 1;
                fam.0 += 1;
            }
            (true, Some(l)) => {
                agree += 1;
                fam.0 += 1;
                used.insert(key.clone());
                problems.push(format!(
                    "{family}/{file} {id}: listed as {} but now AGREES ({et}) — delete its {list} row",
                    l.kind
                ));
            }
            (false, Some(l)) => {
                used.insert(key.clone());
                if l.kind == "defect" {
                    defects += 1;
                } else {
                    diverges += 1;
                }
                if l.engine != et || l.excel != xt {
                    problems.push(format!(
                        "{family}/{file} {id}: listed values moved — row says engine {:?} excel {:?}, now engine {et:?} excel {xt:?}",
                        l.engine, l.excel
                    ));
                }
            }
            (false, None) => {
                problems.push(format!(
                    "{family}/{file} {id}: UNLISTED disagreement {} — engine {et:?}, Excel {xt:?}",
                    c.formula
                ));
                unlisted_rows.push(format!(
                    "{file}\t{id}\t?\t{}\t{}\t{}",
                    esc(&et),
                    esc(&xt),
                    esc(&c.formula)
                ));
            }
        }
    }
    for (fam, (ok, n)) in &per_family {
        println!("  {fam:<10} {ok:>4}/{n:<4} agree");
    }
    for key in listed.keys() {
        if !used.contains(key) {
            problems.push(format!(
                "{list} lists {key:?}, which is not a compared case"
            ));
        }
    }
    let pct = if compared == 0 {
        0.0
    } else {
        100.0 * agree as f64 / compared as f64
    };
    println!(
        "{label}: {agree}/{compared} agree ({pct:.1}%), {defects} listed defect(s), \
         {diverges} listed divergence(s), {not_comparable} not comparable"
    );
    if !unlisted_rows.is_empty() {
        println!("--- candidate {list} rows ---");
        for r in &unlisted_rows {
            println!("{r}");
        }
    }
    if compared < 100 {
        problems.push(format!(
            "only {compared} cases compared — recordings missing?"
        ));
    }
    problems
}

#[test]
fn excel_oracle_fn_corpus__feat__sheet_fn_library() {
    let mut cases = Vec::new();
    for fam in families() {
        for (file, c) in family_cases(&fam) {
            let engine = run_case(&c);
            cases.push(Compared {
                family: fam.clone(),
                file,
                id: c.id.clone(),
                formula: c.formula.clone(),
                engine,
            });
        }
    }
    let problems = compare("excel oracle (fn-corpus)", "divergences.tsv", cases);
    assert!(
        problems.is_empty(),
        "{} oracle problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// ── the number-format corpus ────────────────────────────────────────────
//
// `corpus/format-corpus/*.golden.tsv` (id, code, value, expected) — every
// non-locale row was recorded as Excel's `=TEXT(A1,"<code>")` over the seeded
// value (generate.py builds the identical formula). The engine side is the
// formatter itself (`sheet_format::format_value`, en-US, 1900 system) — the
// same call `tests/format.rs` gates — so a disagreement is about the format
// engine. TEXT() is not the cell display in every respect (it has no colour
// and no column width for `*` fill); those rows are listed as divergences.

/// One format-corpus row.
struct FormatRow {
    file: String,
    id: String,
    code: String,
    value: String,
}

fn format_rows() -> Vec<FormatRow> {
    let dir = sheet_conformance::corpus_root().join("format-corpus");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("format-corpus dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".golden.tsv") && !n.starts_with("locale-"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(dir.join(&f)).expect("format corpus file");
        for line in text.lines() {
            let line = line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            assert!((3..=4).contains(&cols.len()), "{f}: bad row {line:?}");
            out.push(FormatRow {
                file: f.clone(),
                id: cols[0].to_string(),
                code: cols[1].to_string(),
                value: cols[2].to_string(),
            });
        }
    }
    out
}

/// The probe formula generate.py wrote for a format row.
fn format_formula(code: &str) -> String {
    format!("=TEXT(A1,\"{}\")", code.replace('"', "\"\""))
}

fn format_value_of(v: &str) -> CellValue {
    if let Some(t) = v.strip_prefix("text:") {
        CellValue::from(t)
    } else if let Some(b) = v.strip_prefix("bool:") {
        CellValue::Bool(b == "true")
    } else {
        CellValue::Number(v.parse().unwrap_or_else(|_| panic!("bad value {v:?}")))
    }
}

#[test]
fn excel_format_recordings_match_the_live_corpus__feat__sheet_format_engine() {
    let rec = load_recorded("format");
    let rows = format_rows();
    let mut problems = Vec::new();
    for r in &rows {
        match rec.get(&(r.file.clone(), r.id.clone())) {
            None => problems.push(format!("{} {}: not recorded", r.file, r.id)),
            Some(x) if x.formula != format_formula(&r.code) => problems.push(format!(
                "{} {}: STALE — recorded {:?}, corpus now {:?}",
                r.file,
                r.id,
                x.formula,
                format_formula(&r.code)
            )),
            Some(_) => {}
        }
    }
    assert!(rows.len() > 100, "only {} format rows found", rows.len());
    assert_eq!(rec.len(), rows.len(), "recorded vs corpus row count");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn excel_oracle_format_corpus__feat__sheet_format_engine() {
    use sheet_core::{DateSystem, Locale};
    use sheet_format::{compile, format_value, FormatCtx};
    let ctx = FormatCtx::new(DateSystem::Date1900, Locale::EnUs);
    let cases = format_rows()
        .into_iter()
        .map(|r| {
            let engine = compile(&r.code)
                .map(|f| CellValue::Text(format_value(&format_value_of(&r.value), &f, &ctx).into()))
                .map_err(|e| format!("compile: {e}"));
            Compared {
                family: "format".into(),
                file: r.file,
                id: r.id,
                formula: format_formula(&r.code),
                engine,
            }
        })
        .collect();
    let problems = compare(
        "excel oracle (format-corpus)",
        "format-divergences.tsv",
        cases,
    );
    assert!(
        problems.is_empty(),
        "{} oracle problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}
