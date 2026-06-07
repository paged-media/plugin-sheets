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

//! Coercion conformance (spec §7) — the cross-engine hot zone. Self-contained
//! golden-corpus tests that call directly into `sheet_fn::coerce`. Each test
//! function is named with the prefix the registry rows in
//! `registry/features/coerce.yaml` point at (`sheet_fn_coerce_*`), so the
//! coverage gate (§12.2) finds it; the `.golden.tsv` files under
//! `corpus/fn-corpus/coerce/` are the same fixtures the Phase-2 LibreOffice
//! differential oracle will replay.
//!
//! The typed-literal mini-format (documented in each TSV header) keeps the
//! cases declarative and engine-agnostic: `n:`/`t:`/`b:`/`e:`/`empty` encode a
//! `CellValue`; the expected column encodes the result for the surface under
//! test.

use std::path::PathBuf;

use sheet_core::{CellError, CellValue};
use sheet_fn::arg::{Arg, RangeView};
use sheet_fn::coerce;

/// Resolve a corpus file relative to the workspace root (the crate sits one
/// level below it).
fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus/fn-corpus/coerce")
        .join(name)
}

/// Read a golden TSV, dropping `#` comment lines and blank lines, yielding
/// `(id, input, expected)` triples.
fn rows(name: &str) -> Vec<(String, String, String)> {
    let path = corpus(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read corpus {}: {e}", path.display()));
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let id = parts.next().unwrap_or("").to_string();
        let input = parts.next().unwrap_or("").to_string();
        let expected = parts.next().unwrap_or("").to_string();
        out.push((id, input, expected));
    }
    assert!(!out.is_empty(), "corpus {name} has no cases");
    out
}

/// Parse one typed `CellValue` literal (the corpus mini-format).
fn parse_value(s: &str) -> CellValue {
    if s == "empty" {
        return CellValue::Empty;
    }
    if let Some(rest) = s.strip_prefix("n:") {
        return CellValue::Number(
            rest.parse()
                .unwrap_or_else(|_| panic!("bad number {rest:?}")),
        );
    }
    if let Some(rest) = s.strip_prefix("t:") {
        return CellValue::from(rest);
    }
    if let Some(rest) = s.strip_prefix("b:") {
        return match rest {
            "TRUE" => CellValue::Bool(true),
            "FALSE" => CellValue::Bool(false),
            _ => panic!("bad bool {rest:?}"),
        };
    }
    if let Some(rest) = s.strip_prefix("e:") {
        return CellValue::Error(
            CellError::parse(rest).unwrap_or_else(|| panic!("bad error {rest:?}")),
        );
    }
    panic!("unrecognized value literal {s:?}");
}

/// Parse an `expected` cell holding `n:<f64>` (Ok) or `e:<#TOKEN!>` (Err).
fn parse_num_expect(s: &str) -> Result<f64, CellError> {
    if let Some(rest) = s.strip_prefix("n:") {
        Ok(rest.parse().unwrap())
    } else if let Some(rest) = s.strip_prefix("e:") {
        Err(CellError::parse(rest).unwrap())
    } else {
        panic!("bad num-expect {s:?}");
    }
}

/// Parse an `expected` cell holding `b:TRUE`/`b:FALSE` (Ok) or `e:` (Err).
fn parse_bool_expect(s: &str) -> Result<bool, CellError> {
    if let Some(rest) = s.strip_prefix("b:") {
        Ok(rest == "TRUE")
    } else if let Some(rest) = s.strip_prefix("e:") {
        Err(CellError::parse(rest).unwrap())
    } else {
        panic!("bad bool-expect {s:?}");
    }
}

#[test]
fn sheet_fn_coerce_to_number() {
    for (id, input, expected) in rows("to_number.golden.tsv") {
        let got = coerce::to_number(&parse_value(&input));
        let want = parse_num_expect(&expected);
        assert_eq!(got, want, "case {id}: to_number({input:?})");
    }
}

#[test]
fn sheet_fn_coerce_to_text() {
    for (id, input, expected) in rows("to_text.golden.tsv") {
        let got = coerce::to_text(&parse_value(&input));
        let want = expected
            .strip_prefix("t:")
            .unwrap_or_else(|| panic!("case {id}: to_text expected must be t:<text>"));
        assert_eq!(got.as_str(), want, "case {id}: to_text({input:?})");
    }
}

#[test]
fn sheet_fn_coerce_to_bool() {
    for (id, input, expected) in rows("to_bool.golden.tsv") {
        let got = coerce::to_bool(&parse_value(&input));
        let want = parse_bool_expect(&expected);
        assert_eq!(got, want, "case {id}: to_bool({input:?})");
    }
}

/// Parse one arg token: a typed scalar, or a `r:`-prefixed range literal
/// (`r:<r0c0>,<r0c1>;<r1c0>,...`). The range is materialized into an owned
/// buffer; the returned closure borrows it via the [`Arg::Range`] view.
enum OwnedArg {
    Scalar(CellValue),
    Range {
        rows: u32,
        cols: u32,
        cells: Vec<CellValue>,
    },
}

fn parse_arg_token(tok: &str) -> OwnedArg {
    if let Some(rest) = tok.strip_prefix("r:") {
        let row_strs: Vec<&str> = rest.split(';').collect();
        let nrows = row_strs.len() as u32;
        let mut ncols = 0u32;
        let mut cells = Vec::new();
        for rs in &row_strs {
            let cs: Vec<&str> = rs.split(',').collect();
            ncols = cs.len() as u32;
            for c in cs {
                cells.push(parse_value(c));
            }
        }
        OwnedArg::Range {
            rows: nrows,
            cols: ncols,
            cells,
        }
    } else {
        OwnedArg::Scalar(parse_value(tok))
    }
}

#[test]
fn sheet_fn_coerce_error_propagation() {
    use sheet_core::CellRef;
    let origin = CellRef {
        sheet: 0,
        row: 0,
        col: 0,
        row_abs: false,
        col_abs: false,
    };
    for (id, input, expected) in rows("errors.golden.tsv") {
        // Materialize the owned args first (ranges need a stable buffer to
        // borrow), then build the borrowing `Arg` slice.
        let owned: Vec<OwnedArg> = if input == "none" {
            Vec::new()
        } else {
            input.split(' ').map(parse_arg_token).collect()
        };
        let args: Vec<Arg> = owned
            .iter()
            .map(|o| match o {
                OwnedArg::Scalar(v) => Arg::Scalar(v.clone()),
                OwnedArg::Range { rows, cols, cells } => {
                    Arg::Range(RangeView::from_slice(origin, *rows, *cols, cells))
                }
            })
            .collect();

        let got = coerce::first_error(&args);
        let want = if expected == "none" {
            None
        } else {
            Some(CellError::parse(expected.strip_prefix("e:").unwrap()).unwrap())
        };
        assert_eq!(got, want, "case {id}: first_error({input:?})");
    }
}

#[test]
fn sheet_fn_coerce_empty_semantics() {
    for (id, input, expected) in rows("empty.golden.tsv") {
        let (op, val) = input
            .split_once(':')
            .unwrap_or_else(|| panic!("case {id}: empty op must be <op>:<value>"));
        match op {
            "num" => {
                let got = coerce::to_number(&parse_value(val));
                assert_eq!(got, parse_num_expect(&expected), "case {id}: num:{val:?}");
            }
            "bool" => {
                let got = coerce::to_bool(&parse_value(val));
                assert_eq!(got, parse_bool_expect(&expected), "case {id}: bool:{val:?}");
            }
            "text" => {
                let got = coerce::to_text(&parse_value(val));
                let want = expected.strip_prefix("t:").unwrap();
                assert_eq!(got.as_str(), want, "case {id}: text:{val:?}");
            }
            "blank" => {
                let got = parse_value(val).is_blank();
                let want = expected == "b:TRUE";
                assert_eq!(got, want, "case {id}: blank:{val:?}");
            }
            other => panic!("case {id}: unknown empty op {other:?}"),
        }
    }
}

#[test]
fn sheet_fn_coerce_comparison() {
    use std::cmp::Ordering;
    for (id, input, expected) in rows("comparison.golden.tsv") {
        let (a, b) = input
            .split_once(' ')
            .unwrap_or_else(|| panic!("case {id}: comparison input must be <a> <b>"));
        let got = coerce::compare(&parse_value(a), &parse_value(b));
        let want = match expected.as_str() {
            "lt" => Ordering::Less,
            "eq" => Ordering::Equal,
            "gt" => Ordering::Greater,
            other => panic!("case {id}: bad ordering {other:?}"),
        };
        assert_eq!(got, want, "case {id}: compare({a:?}, {b:?})");
    }
}
