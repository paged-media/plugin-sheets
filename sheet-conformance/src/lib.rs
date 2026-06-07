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

//! Conformance harness (spec §12) — the TEST-ONLY crate. Never a
//! dependency of any shipping crate (§4 rule 5). Provides:
//!
//! - the golden-corpus loader ([`load_corpus`]) shared by the Phase-2
//!   per-family runners (`tests/fn_*.rs`, `tests/format.rs`, …);
//! - the coverage gate (`bin/coverage-gate`, the §12.2 verification
//!   invariant) and the env-gated LibreOffice oracle skeleton
//!   (`tests/oracle.rs`, §12.4).
//!
//! ## Golden TSV format (§12.4 "golden corpora")
//!
//! One case per line, four TAB-separated columns:
//!
//! ```text
//! id<TAB>formula<TAB>setup<TAB>expected
//! ```
//!
//! - `id` — case identifier, unique within the file.
//! - `formula` — the formula under test (e.g. `=SUM(A1:A3)`).
//! - `setup` — `;`-separated cell seeds, each `ADDR=VALUE` (split on the
//!   FIRST `=` only, so `A1==B1` seeds the formula `=B1` into `A1`). The
//!   `VALUE` grammar is the runner's concern, not the loader's — the
//!   loader hands back the raw `(addr, value)` pairs verbatim (the spec
//!   `text:foo` tag survives as the value string `text:foo`). Empty
//!   `setup` ⇒ no seeds.
//! - `expected` — the golden display/value string the runner asserts.
//!
//! Lines whose first non-empty character is `#` are comments; blank lines
//! are skipped. Goldens are byte-comparable — the engine is bit-stable
//! (§12.4 "Determinism"), so no tolerance machinery lives here.

use std::path::{Path, PathBuf};

/// One golden-corpus case (spec §12.4). The runner seeds [`setup`] into a
/// model, evaluates [`formula`], and asserts the result renders to
/// [`expected`].
///
/// [`setup`]: CorpusCase::setup
/// [`formula`]: CorpusCase::formula
/// [`expected`]: CorpusCase::expected
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusCase {
    /// Case identifier, unique within its file.
    pub id: String,
    /// Formula under test (e.g. `=SUM(A1:A3)`).
    pub formula: String,
    /// Cell seeds as `(address, raw_value)` pairs, in file order.
    pub setup: Vec<(String, String)>,
    /// Golden expected string the runner asserts against.
    pub expected: String,
}

/// The corpus root: `CARGO_MANIFEST_DIR/../corpus` (the repo-level
/// `corpus/` tree, sibling of the crate dirs — spec §4 layout).
pub fn corpus_root() -> PathBuf {
    repo_root().join("corpus")
}

/// Load a golden TSV by its **repo-relative** path (e.g.
/// `corpus/fn-corpus/agg/sum.golden.tsv`) — the exact string a registry
/// row's `tests.corpus` pointer carries, so a runner can feed its own
/// registered path straight through.
///
/// Panics with a located message on a missing file or a malformed line
/// (wrong column count): a corpus that does not parse is a test-author
/// error, surfaced loudly, never silently skipped.
pub fn load_corpus(repo_relative: &str) -> Vec<CorpusCase> {
    let path = repo_root().join(repo_relative);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("load_corpus: cannot read {}: {e}", path.display());
    });
    parse_corpus(&text, &path)
}

/// Parse golden-TSV text. Split out from [`load_corpus`] so the in-crate
/// unit tests can exercise the grammar without touching the filesystem.
/// `origin` only flavours panic messages.
fn parse_corpus(text: &str, origin: &Path) -> Vec<CorpusCase> {
    let mut cases = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 4 {
            panic!(
                "load_corpus: {}:{} has {} tab-separated columns, expected 4 \
                 (id<TAB>formula<TAB>setup<TAB>expected)",
                origin.display(),
                lineno + 1,
                cols.len()
            );
        }
        cases.push(CorpusCase {
            id: cols[0].to_string(),
            formula: cols[1].to_string(),
            setup: parse_setup(cols[2]),
            expected: cols[3].to_string(),
        });
    }
    cases
}

/// Parse the `setup` column: `;`-separated `ADDR=VALUE` seeds, each split
/// on the FIRST `=`. An empty column yields no seeds; a seed with no `=`
/// is treated as an address with an empty value.
fn parse_setup(col: &str) -> Vec<(String, String)> {
    if col.is_empty() {
        return Vec::new();
    }
    col.split(';')
        .filter(|seed| !seed.is_empty())
        .map(|seed| match seed.split_once('=') {
            Some((addr, value)) => (addr.to_string(), value.to_string()),
            None => (seed.to_string(), String::new()),
        })
        .collect()
}

/// Repo root: `CARGO_MANIFEST_DIR/..` (the crate sits one level under the
/// workspace root, per the §4 top-level-crates layout).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has a parent (the repo root)")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# a comment line is skipped
   # an indented comment too

sum_basic\t=SUM(A1:A3)\tA1=1;A2=2;A3=3\t6
text_seed\t=A1&\"!\"\tA1=text:foo\tfoo!
no_setup\t=1+1\t\t2
formula_seed\t=B1*2\tA1==B1;B1=21\t42
";

    #[test]
    fn parses_columns_comments_and_blanks() {
        let cases = parse_corpus(SAMPLE, Path::new("<sample>"));
        assert_eq!(cases.len(), 4, "two comments + one blank are skipped");

        assert_eq!(cases[0].id, "sum_basic");
        assert_eq!(cases[0].formula, "=SUM(A1:A3)");
        assert_eq!(cases[0].expected, "6");
        assert_eq!(
            cases[0].setup,
            vec![
                ("A1".into(), "1".into()),
                ("A2".into(), "2".into()),
                ("A3".into(), "3".into()),
            ]
        );
    }

    #[test]
    fn setup_value_keeps_typed_tag_verbatim() {
        let cases = parse_corpus(SAMPLE, Path::new("<sample>"));
        // The `text:` tag is the runner's grammar — the loader passes the
        // raw value through untouched.
        assert_eq!(cases[1].setup, vec![("A1".into(), "text:foo".into())]);
    }

    #[test]
    fn empty_setup_column_yields_no_seeds() {
        let cases = parse_corpus(SAMPLE, Path::new("<sample>"));
        assert_eq!(cases[2].id, "no_setup");
        assert!(cases[2].setup.is_empty());
    }

    #[test]
    fn setup_splits_on_first_equals_only() {
        let cases = parse_corpus(SAMPLE, Path::new("<sample>"));
        // `A1==B1` ⇒ address `A1`, value `=B1` (a seeded formula).
        assert_eq!(
            cases[3].setup,
            vec![("A1".into(), "=B1".into()), ("B1".into(), "21".into())]
        );
    }

    #[test]
    #[should_panic(expected = "expected 4")]
    fn wrong_column_count_panics() {
        parse_corpus("only\ttwo\n", Path::new("<sample>"));
    }

    #[test]
    fn corpus_root_is_repo_corpus_dir() {
        assert!(corpus_root().ends_with("corpus"));
        // Sibling of this crate dir, i.e. under the workspace root.
        assert_eq!(corpus_root().parent(), Some(repo_root().as_path()));
    }
}
