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
//!   (repo-relative) AND contain `fn <prefix>` (a test fn whose name
//!   *starts with* the prefix — so `sheet_fn_agg_sum` registered as
//!   `…::sheet_fn_agg_sum` matches a `fn sheet_fn_agg_sum_basic()`).
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
    // `fn sum_basic()` (suffixed variants count, per the gate contract).
    if !text.contains(&format!("fn {prefix}")) {
        push(format!(
            "rust pointer `{ptr}` — `{rel_path}` has no `fn {prefix}` (test fn missing)"
        ));
    }
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
