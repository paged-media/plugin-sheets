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

//! RECALC lane over real workbooks: recompute every formula and compare with
//! the value Excel cached in the file (`sheet_conformance::recalc`).
//!
//! Two halves:
//!
//! * **CI** — `corpus/xlsx-recalc/` in this repo: a small content-selected
//!   subset of Apache POI's legacy workbooks re-saved by desktop Excel 16
//!   (Apache-2.0; see that directory's PROVENANCE.md). `expected.tsv` pins,
//!   per file, how many formula cells fall in each class. The lane fails when
//!   ANY count moves — a regression lowers `agree`, a fix raises it, and both
//!   must update the table in the same commit. A file present without a row,
//!   or a row without a file, fails too.
//! * **Local, full** — `PAGED_XLSX_CORPUS=1` (or a corpus root) walks the
//!   private corpus's `xlsx/authored` + `xlsx/poi-converted`, selects by
//!   CONTENT (a ZIP that opens and carries formula cells — never by
//!   extension), and REPORTS pass rates and the top disagreement classes.
//!   Set `PAGED_RECALC_REPORT=<path>` for a per-cell TSV.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sheet_conformance::recalc::{functions_in, recalc_workbook, CellOutcome, Class};
use sheet_core::CellValue;

fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    formulas: usize,
    agree: usize,
    differs: usize,
    unparsed: usize,
    volatile: usize,
    no_cache: usize,
}

fn count(outcomes: &[CellOutcome]) -> Counts {
    let mut c = Counts {
        formulas: outcomes.len(),
        ..Counts::default()
    };
    for o in outcomes {
        match o.class {
            Class::Agree => c.agree += 1,
            Class::Differs => c.differs += 1,
            Class::Unparsed => c.unparsed += 1,
            Class::Volatile => c.volatile += 1,
            Class::NoCache => c.no_cache += 1,
        }
    }
    c
}

fn token(v: &CellValue) -> String {
    match v {
        CellValue::Empty => "blank".into(),
        CellValue::Number(n) => format!("n:{n}"),
        CellValue::Text(t) => format!("s:{t}"),
        CellValue::Bool(b) => format!("b:{b}"),
        CellValue::Error(e) => format!("e:{}", e.as_str()),
    }
}

/// The class key a disagreement is filed under: the first function called,
/// or `(no function)` for plain references/arithmetic.
fn class_key(o: &CellOutcome) -> String {
    functions_in(&o.formula)
        .into_iter()
        .next()
        .unwrap_or_else(|| "(no function)".to_string())
}

fn subset_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../corpus/xlsx-recalc")
}

fn load_expected(path: &Path) -> BTreeMap<String, Counts> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let c: Vec<&str> = line.split('\t').collect();
        assert_eq!(c.len(), 7, "bad expected.tsv row {line:?}");
        let n = |i: usize| c[i].parse::<usize>().expect("count");
        out.insert(
            c[0].to_string(),
            Counts {
                formulas: n(1),
                agree: n(2),
                differs: n(3),
                unparsed: n(4),
                volatile: n(5),
                no_cache: n(6),
            },
        );
    }
    out
}

fn row(name: &str, c: &Counts) -> String {
    format!(
        "{name}\t{}\t{}\t{}\t{}\t{}\t{}",
        c.formulas, c.agree, c.differs, c.unparsed, c.volatile, c.no_cache
    )
}

#[test]
fn xlsx_recalc_matches_excel_cached_values_ci_subset__feat__sheet_calc_engine() {
    let dir = subset_dir();
    let expected = load_expected(&dir.join("expected.tsv"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("corpus/xlsx-recalc")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "xlsx"))
        .collect();
    files.sort();
    assert!(files.len() >= 10, "only {} subset workbooks", files.len());

    let mut problems = Vec::new();
    let mut total = Counts::default();
    let mut seen = Vec::new();
    for p in &files {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(p).expect("read subset workbook");
        assert!(is_zip(&bytes), "{name} is not a ZIP package");
        let outcomes = recalc_workbook(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let c = count(&outcomes);
        total.formulas += c.formulas;
        total.agree += c.agree;
        total.differs += c.differs;
        total.unparsed += c.unparsed;
        total.volatile += c.volatile;
        total.no_cache += c.no_cache;
        assert!(
            c.formulas > 0,
            "{name}: no formula cells — not a recalc fixture"
        );
        match expected.get(&name) {
            None => problems.push(format!("no expected.tsv row: {}", row(&name, &c))),
            Some(e) if *e != c => problems.push(format!(
                "{name}: counts moved\n  expected {}\n  now      {}",
                row(&name, e),
                row(&name, &c)
            )),
            Some(_) => {}
        }
        seen.push(name);
    }
    for name in expected.keys() {
        if !seen.contains(name) {
            problems.push(format!(
                "expected.tsv lists {name}, which is not in the subset"
            ));
        }
    }
    let comparable = total.agree + total.differs + total.unparsed;
    println!(
        "xlsx recalc (CI subset): {} files, {} formula cells — agree {}/{} comparable \
         ({:.1}%), differs {}, unparsed {}, volatile {}, no cache {}",
        files.len(),
        total.formulas,
        total.agree,
        comparable,
        100.0 * total.agree as f64 / comparable.max(1) as f64,
        total.differs,
        total.unparsed,
        total.volatile,
        total.no_cache
    );
    assert!(
        problems.is_empty(),
        "{} problem(s) — a fix or a regression moved the recalc counts; \
         update corpus/xlsx-recalc/expected.tsv in the same commit:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

fn corpus_root() -> Option<PathBuf> {
    let v = std::env::var_os("PAGED_XLSX_CORPUS")?;
    let v = v.to_string_lossy().into_owned();
    let root = if v == "1" || v.is_empty() {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../corpus")
    } else {
        PathBuf::from(v)
    };
    Some(root.join("xlsx"))
}

#[test]
#[ignore = "full recalc lane: opt-in (PAGED_XLSX_CORPUS=1 + the private corpus mount)"]
fn xlsx_recalc_full_corpus_report() {
    let Some(root) = corpus_root() else {
        eprintln!("SKIP: PAGED_XLSX_CORPUS unset");
        return;
    };
    // An ABSENT corpus is a skip; a set PATH that is wrong is a failure.
    assert!(
        root.is_dir(),
        "PAGED_XLSX_CORPUS points at {}, which has no xlsx/",
        root.display()
    );
    let mut report: Vec<String> =
        vec!["source\tfile\tsheet\tcell\tclass\tformula\tcached\tengine".into()];
    let mut by_src: BTreeMap<String, (usize, Counts)> = BTreeMap::new();
    let mut differs_by_fn: BTreeMap<String, usize> = BTreeMap::new();
    let mut unparsed_by_fn: BTreeMap<String, usize> = BTreeMap::new();
    let (mut not_zip, mut refused, mut no_formulas) = (0usize, 0usize, 0usize);
    for src in ["authored", "poi-converted"] {
        let dir = root.join(src);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for p in files {
            let bytes = std::fs::read(&p).expect("read");
            if !is_zip(&bytes) {
                not_zip += 1; // PROVENANCE.md etc. — selection is by content
                continue;
            }
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let Ok(outcomes) = recalc_workbook(&bytes) else {
                refused += 1;
                continue;
            };
            if outcomes.is_empty() {
                no_formulas += 1;
                continue;
            }
            let c = count(&outcomes);
            let e = by_src.entry(src.to_string()).or_default();
            e.0 += 1;
            e.1.formulas += c.formulas;
            e.1.agree += c.agree;
            e.1.differs += c.differs;
            e.1.unparsed += c.unparsed;
            e.1.volatile += c.volatile;
            e.1.no_cache += c.no_cache;
            for o in &outcomes {
                match o.class {
                    Class::Differs => *differs_by_fn.entry(class_key(o)).or_default() += 1,
                    Class::Unparsed => *unparsed_by_fn.entry(class_key(o)).or_default() += 1,
                    _ => {}
                }
                if matches!(o.class, Class::Differs | Class::Unparsed) {
                    report.push(format!(
                        "{src}\t{name}\t{}\t{}{}\t{:?}\t{}\t{}\t{}",
                        o.sheet,
                        sheet_core::col_to_a1(o.at.1),
                        o.at.0 + 1,
                        o.class,
                        o.formula.replace('\t', " "),
                        token(&o.cached).replace(['\t', '\n'], " "),
                        token(&o.engine).replace(['\t', '\n'], " ")
                    ));
                }
            }
        }
    }
    println!("xlsx recalc (full corpus): skipped {not_zip} non-ZIP, {refused} refused, {no_formulas} without formulas");
    let mut all = Counts::default();
    for (src, (n, c)) in &by_src {
        let comparable = c.agree + c.differs + c.unparsed;
        println!(
            "  {src:<14} {n:>4} workbooks {:>7} formula cells — agree {}/{} ({:.1}%), differs {}, unparsed {}, volatile {}, no cache {}",
            c.formulas,
            c.agree,
            comparable,
            100.0 * c.agree as f64 / comparable.max(1) as f64,
            c.differs,
            c.unparsed,
            c.volatile,
            c.no_cache
        );
        all.formulas += c.formulas;
        all.agree += c.agree;
        all.differs += c.differs;
        all.unparsed += c.unparsed;
    }
    let top = |label: &str, m: &BTreeMap<String, usize>| {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!("  top {label}:");
        for (k, n) in v.into_iter().take(20) {
            println!("    {n:>6}  {k}");
        }
    };
    top("differs by first function", &differs_by_fn);
    top("unparsed by first function", &unparsed_by_fn);
    if let Some(path) = std::env::var_os("PAGED_RECALC_REPORT") {
        std::fs::write(&path, report.join("\n") + "\n").expect("write report");
        println!("  per-cell report: {}", PathBuf::from(path).display());
    }
    assert!(
        all.formulas > 0,
        "the full corpus produced zero formula cells"
    );
}
