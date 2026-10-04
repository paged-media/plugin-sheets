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

//! Wall-clock benches for the session-level edits (`SheetSession`) —
//! TRENDED, never gated (the gates are the count budgets in
//! `tests/perf_budgets.rs`).
//!
//! `cargo bench -p sheet-js` (add `-- --quick` for a fast pass).
//!
//! `load_corpus_largest_authored` loads the largest Excel-authored workbook
//! of the private corpus (`~/paged/corpus/xlsx/authored/`, or
//! `$PAGED_XLSX_CORPUS/xlsx/authored/` when that names a corpus root). It
//! is chosen by CONTENT, not by name or extension: every file that is a ZIP
//! and opens as a workbook is loaded once, and the one with the most used
//! cells wins. Without the corpus mount the bench is skipped with a note.

use std::path::{Path, PathBuf};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use sheet_js::core::SheetSession;

const ROWS: u32 = 1000;

/// A1:A1000 = 1000..1, C1 = SUM(A1:A1000).
fn descending_with_sum() -> SheetSession {
    let mut s = SheetSession::new();
    for i in 0..ROWS {
        s.set_cell(0, i, 0, &(ROWS - i).to_string()).unwrap();
    }
    s.set_cell(0, 0, 2, &format!("=SUM(A1:A{ROWS})")).unwrap();
    s
}

fn sort(c: &mut Criterion) {
    let range = format!("A1:A{ROWS}");
    c.bench_function("sort_1k_rows_with_sum", |b| {
        b.iter_batched(
            descending_with_sum,
            |mut s| {
                s.sort_range(0, &range, 0, true, false).unwrap();
                assert_eq!(s.get_cell_display(0, 0, 2), "500500");
                s
            },
            BatchSize::LargeInput,
        )
    });
}

/// `$PAGED_XLSX_CORPUS` as a corpus root (any value but "1"), else the
/// workspace layout's `~/paged/corpus`.
fn corpus_root() -> PathBuf {
    match std::env::var("PAGED_XLSX_CORPUS") {
        Ok(v) if v != "1" && !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../corpus"),
    }
}

fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

/// The authored workbook with the most used cells (rows × cols summed over
/// its sheets), with that count. Chosen by opening, not by name.
fn largest_authored(dir: &Path) -> Option<(PathBuf, Vec<u8>, u64)> {
    let mut best: Option<(PathBuf, Vec<u8>, u64)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if !is_zip(&bytes) {
            continue;
        }
        let Ok(s) = SheetSession::load_xlsx(&bytes) else {
            continue;
        };
        let cells: u64 = s
            .list_sheets()
            .iter()
            .map(|i| u64::from(i.rows) * u64::from(i.cols))
            .sum();
        if best.as_ref().is_none_or(|(_, _, n)| cells > *n) {
            best = Some((path, bytes, cells));
        }
    }
    best
}

fn load_corpus(c: &mut Criterion) {
    let dir = corpus_root().join("xlsx/authored");
    let Some((path, bytes, cells)) = largest_authored(&dir) else {
        eprintln!(
            "load_corpus_largest_authored: SKIPPED — no openable workbook under {}",
            dir.display()
        );
        return;
    };
    eprintln!(
        "load_corpus_largest_authored: {} ({} bytes, {cells} used cells)",
        path.file_name().unwrap_or_default().to_string_lossy(),
        bytes.len()
    );
    let mut g = c.benchmark_group("corpus");
    g.sample_size(10);
    g.bench_function("load_corpus_largest_authored", |b| {
        b.iter(|| SheetSession::load_xlsx(&bytes).expect("it opened during selection"))
    });
    g.finish();
}

criterion_group!(benches, sort, load_corpus);
criterion_main!(benches);
