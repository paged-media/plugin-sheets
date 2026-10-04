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

//! Coverage gate — the §12.2 "100% verification invariant" made
//! executable. Registry-driven dispatch already guarantees *no row → no
//! dispatch*; this binary enforces the dual: *every `implemented` row →
//! real tests on disk*.
//!
//! It reads every `registry/functions/*.yaml` and `registry/features/*.yaml`
//! (rows are YAML sequences of maps; unknown fields tolerated — function
//! rows and feature rows have different shapes) and, for each row with
//! `status: implemented`, verifies its `tests:` pointers actually resolve:
//!
//! - `tests.rust` — `"path/to/file.rs::prefix"`: the file must exist
//!   (repo-relative) AND contain at least one RUNNABLE test whose name
//!   *starts with* the prefix (so `sheet_fn_agg_sum` registered as
//!   `…::sheet_fn_agg_sum` matches a `fn sheet_fn_agg_sum_basic()`).
//!   Runnable = carries a test attribute (`#[test]`, `#[…::test]`,
//!   `#[rstest]`), is NOT `#[ignore]`d, and its body is not a `todo!()` /
//!   `unimplemented!()` stub. Until 2026-10-04 the gate was a text grep
//!   for `fn <prefix>`, so an ignored stub or a non-test helper counted
//!   as coverage.
//! - `tests.corpus` / `tests.vitest` — the file must exist (repo-relative).
//! - any other lane (e.g. `cli`) — informational only, never a gap.
//!
//! An `implemented` row with NO tests at all is a gap. The binary prints a
//! per-file summary plus the gap list and exits `1` iff any gap exists.
//! Dependency-light: `serde_yaml` + `std` only (no extra deps).
//!
//! Repo root resolves as `CARGO_MANIFEST_DIR/..` (the crate sits one level
//! under the workspace root, §4 layout).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// A single resolved problem with an `implemented` row.
struct Gap {
    /// Registry file the row came from, repo-relative.
    source: String,
    /// The row's `id` (or `<no id>` if absent).
    id: String,
    /// What is wrong.
    reason: String,
}

/// Per-registry-file tallies for the summary table.
#[derive(Default)]
struct FileStats {
    implemented: usize,
    planned: usize,
    other: usize,
    gaps: usize,
}

fn main() -> ExitCode {
    let root = repo_root();
    let registry = root.join("registry");

    let mut files = Vec::new();
    for sub in ["functions", "features"] {
        match collect_yaml(&registry.join(sub)) {
            Ok(mut found) => {
                found.sort();
                files.extend(found);
            }
            Err(e) => {
                // A missing registry subdir is a real misconfiguration —
                // fail loudly rather than silently pass.
                eprintln!(
                    "coverage-gate: cannot scan {}: {e}",
                    registry.join(sub).display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    let mut gaps: Vec<Gap> = Vec::new();
    let mut stats: BTreeMap<String, FileStats> = BTreeMap::new();
    // Cache file-existence and fn-presence checks across rows that share a
    // pointer (e.g. every agg row points into one `fn_agg.rs`).
    let mut text_cache: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();

    for file in &files {
        let rel = rel_to_root(&root, file);
        let stat = stats.entry(rel.clone()).or_default();

        let rows = match parse_rows(file) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("coverage-gate: cannot parse {rel}: {e}");
                return ExitCode::FAILURE;
            }
        };

        for row in &rows {
            let status = row_str(row, "status").unwrap_or_default();
            match status.as_str() {
                "implemented" => stat.implemented += 1,
                "planned" => {
                    stat.planned += 1;
                    continue;
                }
                _ => {
                    stat.other += 1;
                    continue;
                }
            }

            let id = row_str(row, "id").unwrap_or_else(|| "<no id>".to_string());
            let before = gaps.len();
            check_row(&root, &rel, &id, row, &mut gaps, &mut text_cache);
            stat.gaps += gaps.len() - before;
        }
    }

    print_summary(&stats, &gaps);

    if gaps.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Validate one `implemented` row's `tests:` pointers, pushing a [`Gap`]
/// per problem found.
fn check_row(
    root: &Path,
    source: &str,
    id: &str,
    row: &serde_yaml::Value,
    gaps: &mut Vec<Gap>,
    text_cache: &mut BTreeMap<PathBuf, Option<String>>,
) {
    let mut push = |reason: String| {
        gaps.push(Gap {
            source: source.to_string(),
            id: id.to_string(),
            reason,
        })
    };

    let tests = row.get("tests").and_then(|t| t.as_mapping());
    let Some(tests) = tests else {
        push("implemented row has no `tests:` block".to_string());
        return;
    };

    let mut saw_known_lane = false;

    for (lane, entries) in tests {
        let Some(lane) = lane.as_str() else { continue };
        let pointers = as_str_list(entries);
        match lane {
            "rust" => {
                saw_known_lane = true;
                for ptr in pointers {
                    check_rust_pointer(root, &ptr, &mut push, text_cache);
                }
            }
            "corpus" | "vitest" => {
                saw_known_lane = true;
                for ptr in pointers {
                    if !root.join(&ptr).is_file() {
                        push(format!("{lane} pointer `{ptr}` — file does not exist"));
                    }
                }
            }
            other => {
                // Lanes the gate doesn't verify (e.g. `cli`): note, don't fail.
                println!(
                    "  info: {source} [{id}] {other}: {} entr{} (not gate-verified)",
                    pointers.len(),
                    if pointers.len() == 1 { "y" } else { "ies" }
                );
            }
        }
    }

    if !saw_known_lane {
        push("implemented row has a `tests:` block but no rust/corpus/vitest lane".to_string());
    }
}

/// Verify one `tests.rust` pointer `"path/to/file.rs::prefix"`.
fn check_rust_pointer(
    root: &Path,
    ptr: &str,
    push: &mut impl FnMut(String),
    text_cache: &mut BTreeMap<PathBuf, Option<String>>,
) {
    let Some((rel_path, prefix)) = ptr.split_once("::") else {
        push(format!(
            "rust pointer `{ptr}` — missing `::prefix` (expected `file.rs::fn_prefix`)"
        ));
        return;
    };
    if prefix.is_empty() {
        push(format!("rust pointer `{ptr}` — empty fn prefix after `::`"));
        return;
    }

    let abs = root.join(rel_path);
    let entry = text_cache
        .entry(abs.clone())
        .or_insert_with(|| std::fs::read_to_string(&abs).ok());
    let Some(text) = entry else {
        push(format!(
            "rust pointer `{ptr}` — file `{rel_path}` does not exist"
        ));
        return;
    };

    // Match `fn <prefix>` so a registered `…::sum` is satisfied by
    // `fn sum_basic()` (suffixed variants count, per the gate contract) —
    // but only a RUNNABLE test counts (see the module doc).
    let found = test_fns_with_prefix(text, prefix);
    if found.is_empty() {
        push(format!(
            "rust pointer `{ptr}` — `{rel_path}` has no `fn {prefix}` (test fn missing)"
        ));
        return;
    }
    if found.iter().any(|f| f.verdict == Verdict::Runnable) {
        return;
    }
    let why: Vec<String> = found
        .iter()
        .map(|f| format!("`{}` {}", f.name, f.verdict.describe()))
        .collect();
    push(format!(
        "rust pointer `{ptr}` — no runnable test: {}",
        why.join(", ")
    ));
}

/// Why a `fn` matching a pointer does or does not count as a test.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Runnable,
    NotATest,
    Ignored,
    Stub,
}

impl Verdict {
    fn describe(&self) -> &'static str {
        match self {
            Verdict::Runnable => "runs",
            Verdict::NotATest => "has no test attribute",
            Verdict::Ignored => "is #[ignore]d",
            Verdict::Stub => "is a todo!()/unimplemented!() stub",
        }
    }
}

struct FoundFn {
    name: String,
    verdict: Verdict,
}

/// Every `fn <prefix>…` item in `text`, classified.
fn test_fns_with_prefix(text: &str, prefix: &str) -> Vec<FoundFn> {
    let needle = format!("fn {prefix}");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(off) = text[from..].find(&needle) {
        let at = from + off;
        from = at + needle.len();
        // `fn` must start a token (not `xfn foo`).
        if at > 0 {
            let prev = text.as_bytes()[at - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                continue;
            }
        }
        let name: String = text[at + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let attrs = attributes_before(text, at);
        let verdict = if !attrs.iter().any(|a| is_test_attr(a)) {
            Verdict::NotATest
        } else if attrs.iter().any(|a| is_ignore_attr(a)) {
            Verdict::Ignored
        } else if fn_body(text, at).is_some_and(is_stub_body) {
            Verdict::Stub
        } else {
            Verdict::Runnable
        };
        out.push(FoundFn { name, verdict });
    }
    out
}

/// The attribute lines directly above the `fn` at byte `at` (doc comments
/// and comments are skipped; the walk stops at the first line that is
/// neither an attribute, its continuation, nor a comment).
fn attributes_before(text: &str, at: usize) -> Vec<String> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    // Text on the fn's own line before `fn` (e.g. `#[test] fn x()`).
    let mut attrs = vec![text[line_start..at].to_string()];
    let mut pending = String::new();
    for line in text[..line_start].lines().rev() {
        let t = line.trim();
        if t.starts_with("//") {
            continue;
        }
        if t.starts_with("#[") {
            attrs.push(format!("{t}{pending}"));
            pending.clear();
            continue;
        }
        // A continuation line of a multi-line attribute (`#[ignore =\n "…"]`).
        if !t.is_empty() && (t.ends_with(']') || t.ends_with(',') || t.starts_with('"')) {
            pending = format!(" {t}{pending}");
            continue;
        }
        break;
    }
    attrs
}

fn is_test_attr(attr: &str) -> bool {
    // `#[test]`, `#[tokio::test]`, `#[wasm_bindgen_test]`, `#[rstest]`.
    attr.split("#[").skip(1).any(|a| {
        let path: String = a
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
            .collect();
        path == "test" || path.ends_with("::test") || path.ends_with("_test") || path == "rstest"
    })
}

fn is_ignore_attr(attr: &str) -> bool {
    attr.split("#[").skip(1).any(|a| {
        let a = a.trim_start();
        a.starts_with("ignore") || (a.starts_with("cfg_attr") && a.contains("ignore"))
    })
}

/// The `{ … }` body of the fn at byte `at`, brace-matched while skipping
/// string/char literals and comments. `None` when it cannot be delimited.
fn fn_body(text: &str, at: usize) -> Option<&str> {
    let b = text.as_bytes();
    let open = at + text[at..].find('{')?;
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'r' if matches!(b.get(i + 1), Some(b'#') | Some(b'"'))
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
            {
                // Raw string r"…" / r#"…"#.
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) == Some(&b'"') {
                    let close = format!("\"{}", "#".repeat(hashes));
                    i = j + 1 + text[j + 1..].find(&close)? + close.len() - 1;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'\'' => {
                // A char literal ('x', '\n', '{') — not a lifetime ('a).
                if b.get(i + 2) == Some(&b'\'') {
                    i += 2;
                } else if b.get(i + 1) == Some(&b'\\') {
                    i += 1 + text[i + 1..].find('\'')?;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_stub_body(body: &str) -> bool {
    body.contains("todo!(") || body.contains("unimplemented!(")
}

/// Print the per-file summary table and the gap list.
fn print_summary(stats: &BTreeMap<String, FileStats>, gaps: &[Gap]) {
    println!("\ncoverage-gate (§12.2) — implemented rows must carry real tests\n");
    println!(
        "  {:<34} {:>11} {:>7} {:>5} {:>4}",
        "registry file", "implemented", "planned", "othr", "gaps"
    );
    println!("  {}", "-".repeat(34 + 1 + 11 + 1 + 7 + 1 + 5 + 1 + 4));

    let (mut t_impl, mut t_plan, mut t_other, mut t_gaps) = (0, 0, 0, 0);
    for (file, s) in stats {
        println!(
            "  {:<34} {:>11} {:>7} {:>5} {:>4}",
            file, s.implemented, s.planned, s.other, s.gaps
        );
        t_impl += s.implemented;
        t_plan += s.planned;
        t_other += s.other;
        t_gaps += s.gaps;
    }
    println!("  {}", "-".repeat(34 + 1 + 11 + 1 + 7 + 1 + 5 + 1 + 4));
    println!(
        "  {:<34} {:>11} {:>7} {:>5} {:>4}",
        "TOTAL", t_impl, t_plan, t_other, t_gaps
    );

    if gaps.is_empty() {
        println!("\nGREEN — {t_impl} implemented row(s), 0 gaps, {t_plan} still planned.");
    } else {
        println!("\nFAILED — {} gap(s):", gaps.len());
        for g in gaps {
            println!("  - {} [{}]: {}", g.source, g.id, g.reason);
        }
    }
}

/// Read every `*.yaml` (and `*.yml`) directly under `dir`. A non-existent
/// dir is the caller's problem; here it surfaces as an `Err`.
fn collect_yaml(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let is_yaml = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "yaml" || e == "yml");
        if path.is_file() && is_yaml {
            out.push(path);
        }
    }
    Ok(out)
}

/// Parse a registry file into its sequence of row maps. Tolerates unknown
/// fields by deserializing to untyped [`serde_yaml::Value`]s.
fn parse_rows(path: &Path) -> Result<Vec<serde_yaml::Value>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
    match doc {
        serde_yaml::Value::Sequence(rows) => Ok(rows),
        serde_yaml::Value::Null => Ok(Vec::new()), // empty / comment-only file
        other => Err(format!(
            "expected a top-level sequence of rows, found {}",
            yaml_kind(&other)
        )),
    }
}

/// A string field of a row, if present and a scalar string.
fn row_str(row: &serde_yaml::Value, key: &str) -> Option<String> {
    row.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// Coerce a `tests.<lane>` value to a list of strings. Accepts a YAML
/// sequence of strings (the registry convention) or a lone scalar string.
fn as_str_list(v: &serde_yaml::Value) -> Vec<String> {
    match v {
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect(),
        serde_yaml::Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn yaml_kind(v: &serde_yaml::Value) -> &'static str {
    match v {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "bool",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "sequence",
        serde_yaml::Value::Mapping(_) => "mapping",
        serde_yaml::Value::Tagged(_) => "tagged",
    }
}

/// Repo-relative display path for a file under `root` (falls back to the
/// absolute path if it is somehow not under the root).
fn rel_to_root(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .into_owned()
}

/// Repo root: `CARGO_MANIFEST_DIR/..`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has a parent (the repo root)")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdicts(src: &str, prefix: &str) -> Vec<(String, Verdict)> {
        test_fns_with_prefix(src, prefix)
            .into_iter()
            .map(|f| (f.name, f.verdict))
            .collect()
    }

    #[test]
    fn coverage_gate_counts_a_plain_test() {
        let src = "#[test]\nfn sheet_x_basic() {\n    assert_eq!(1, 1);\n}\n";
        assert_eq!(
            verdicts(src, "sheet_x"),
            vec![("sheet_x_basic".into(), Verdict::Runnable)]
        );
    }

    #[test]
    fn coverage_gate_rejects_ignored_tests_single_and_multi_line() {
        let src = "#[test]\n#[ignore]\nfn a_one() {}\n\n\
                   #[test]\n#[ignore = \"opt-in lane\"]\nfn a_two() {}\n\n\
                   #[test]\n#[ignore =\n    \"long reason\"]\nfn a_three() {}\n\n\
                   #[cfg_attr(not(feature = \"x\"), ignore)]\n#[test]\nfn a_four() {}\n";
        let v = verdicts(src, "a_");
        assert_eq!(v.len(), 4);
        assert!(v.iter().all(|(_, d)| *d == Verdict::Ignored), "{v:?}");
    }

    #[test]
    fn coverage_gate_rejects_todo_and_unimplemented_bodies() {
        let src = "#[test]\nfn s_todo() {\n    todo!(\"later\")\n}\n\
                   #[test]\nfn s_unimpl() { unimplemented!() }\n";
        let v = verdicts(src, "s_");
        assert!(v.iter().all(|(_, d)| *d == Verdict::Stub), "{v:?}");
    }

    #[test]
    fn coverage_gate_rejects_a_helper_without_a_test_attribute() {
        let src = "/// doc\nfn h_helper() -> bool { true }\n";
        assert_eq!(
            verdicts(src, "h_"),
            vec![("h_helper".into(), Verdict::NotATest)]
        );
    }

    #[test]
    fn coverage_gate_body_scan_survives_braces_in_literals() {
        // A `}` in a string, raw string, char literal or comment must not
        // end the body early — the todo!() after it is still found.
        let src = "#[test]\nfn b_lit() {\n    let _ = \"}\";\n    let _ = r#\"}\"#;\n    \
                   let _ = '}';\n    // }\n    todo!()\n}\n";
        assert_eq!(verdicts(src, "b_"), vec![("b_lit".into(), Verdict::Stub)]);
    }

    #[test]
    fn coverage_gate_accepts_path_and_suffixed_test_attributes() {
        let src = "#[tokio::test]\nasync fn t_a() {}\n#[wasm_bindgen_test]\nfn t_b() {}\n\
                   #[rstest]\nfn t_c() {}\n";
        let v = verdicts(src, "t_");
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|(_, d)| *d == Verdict::Runnable), "{v:?}");
    }

    #[test]
    fn coverage_gate_does_not_match_inside_another_identifier() {
        let src = "#[test]\nfn xfn_y() {}\n";
        assert!(verdicts(src, "y").is_empty());
    }
}
