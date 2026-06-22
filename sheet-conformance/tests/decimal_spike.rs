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

//! The D-6 exact-decimal SPIKE conformance (spec §3, §5.1, M3; registry
//! `registry/features/decimal.yaml`). Replays a divergence corpus
//! (`corpus/decimal-corpus/divergence.golden.tsv`) through BOTH
//! [`sheet_fn::num::Numeric`] backends — the v1 default [`sheet_fn::num::F64`]
//! and the spike's [`sheet_fn::num_decimal::Decimal`] — and pins where
//! IEEE-754 f64 and exact base-10 decimal disagree.
//!
//! ## Why a spike test, not a kernel test
//!
//! No shipping kernel changes for this track. f64 stays v1 (the format goldens
//! must not move; en-US output is byte-identical). The deliverable is *evidence
//! for a decision*: the corpus quantifies the correctness win, and
//! [`sheet_calc_decimal_spike_report`] anchors the written adopt/defer
//! recommendation (`DECIMAL-SPIKE.md`). The decimal half is cfg-gated on the
//! `exact-decimal` feature; the f64 half always runs (so the corpus and the
//! report assertion stay live in the default build).
//!
//! Run the full spike:
//! ```text
//! cargo test -p sheet-conformance --features exact-decimal --test decimal_spike
//! ```
//!
//! ## Rulings documented here (the spike ledger)
//!
//! - **decimal-impl** (`sheet.calc.decimal.decimal-impl`): the [`Decimal`]
//!   `Numeric` impl over `rust_decimal` (MIT, pure-Rust, 28-29 sig digits) is a
//!   drop-in for `F64` — same trait, same call sites. Integer powers are exact;
//!   fractional powers fall back to the `e^(y·ln x)` approximation (caveat).
//! - **divergence-corpus** (`sheet.calc.decimal.divergence-corpus`): tenths and
//!   pennies drift in binary (`0.1 + 0.2 = 0.30000000000000004`) but are exact
//!   in base-10 (`= 0.3`). The corpus also carries HONEST contrast rows where
//!   f64 happens to round back to the clean value (0.1 added ten times = 1.0),
//!   so the spike does not overstate the win.
//! - **1/3 is non-terminating in base-10 too** — decimal is exact for
//!   *terminating* decimals (money, tenths), NOT a universal cure: `1/3 * 3`
//!   carries 28 nines in decimal, asserted in
//!   [`sheet_calc_decimal_impl_thirds_caveat`]. This is the central honesty of
//!   the spike report.

use std::path::{Path, PathBuf};

use sheet_fn::num::{Numeric, F64};

/// Repo root: `CARGO_MANIFEST_DIR/..` (the crate sits one level under the
/// workspace root, §4 layout) — mirrors the coverage gate's resolution.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has a parent (the repo root)")
        .to_path_buf()
}

/// One parsed corpus row.
struct Case {
    id: String,
    op: String,
    operands: String,
    f64_golden: f64,
    /// Only consumed by the cfg-gated decimal half — dead in the default build.
    #[cfg_attr(not(feature = "exact-decimal"), allow(dead_code))]
    decimal_golden: f64,
    diverges: bool,
}

/// Load + parse `corpus/decimal-corpus/divergence.golden.tsv` (skipping
/// `#`-comments and blank lines). 6 TAB-separated columns; the two result
/// columns parse as f64 so comparison is by IEEE bit value, not by string
/// formatting (avoids Rust-vs-Python `to_string()` skew).
fn load_corpus() -> Vec<Case> {
    let path = repo_root().join("corpus/decimal-corpus/divergence.golden.tsv");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut cases = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            6,
            "line {}: expected 6 TAB columns, got {} in {:?}",
            lineno + 1,
            cols.len(),
            line
        );
        cases.push(Case {
            id: cols[0].to_string(),
            op: cols[1].to_string(),
            operands: cols[2].to_string(),
            f64_golden: cols[3]
                .parse()
                .unwrap_or_else(|_| panic!("line {}: bad f64 `{}`", lineno + 1, cols[3])),
            decimal_golden: cols[4]
                .parse()
                .unwrap_or_else(|_| panic!("line {}: bad decimal `{}`", lineno + 1, cols[4])),
            diverges: match cols[5] {
                "yes" => true,
                "no" => false,
                other => panic!(
                    "line {}: diverges must be yes/no, got `{}`",
                    lineno + 1,
                    other
                ),
            },
        });
    }
    assert!(!cases.is_empty(), "divergence corpus is empty");
    cases
}

/// Replay one case through a chosen [`Numeric`] backend, returning the result
/// lowered to f64. Generic over the backend so the f64 and decimal halves run
/// the IDENTICAL code path (the whole point of the D-6 seam).
fn replay<N: Numeric>(op: &str, operands: &str) -> f64 {
    match op {
        "add" => {
            let mut acc = N::from_f64(0.0);
            for tok in operands.split(';') {
                acc = acc.add(N::from_f64(parse_f64(tok)));
            }
            acc.to_f64()
        }
        "accum" => {
            let parts: Vec<&str> = operands.split(';').collect();
            assert_eq!(parts.len(), 2, "accum needs `count;step`, got {operands:?}");
            let count: usize = parts[0].parse().expect("accum count");
            let step = N::from_f64(parse_f64(parts[1]));
            let mut acc = N::from_f64(0.0);
            for _ in 0..count {
                acc = acc.add(step);
            }
            acc.to_f64()
        }
        "binop" => {
            // "a OP b"
            let parts: Vec<&str> = operands.split_whitespace().collect();
            assert_eq!(parts.len(), 3, "binop needs `a OP b`, got {operands:?}");
            let a = N::from_f64(parse_f64(parts[0]));
            let b = N::from_f64(parse_f64(parts[2]));
            let r = match parts[1] {
                "+" => a.add(b),
                "-" => a.sub(b),
                "*" => a.mul(b),
                "/" => a.div(b),
                other => panic!("unknown binop `{other}`"),
            };
            r.to_f64()
        }
        other => panic!("unknown op `{other}`"),
    }
}

fn parse_f64(tok: &str) -> f64 {
    tok.trim()
        .parse()
        .unwrap_or_else(|_| panic!("bad operand `{tok}`"))
}

// ---------------------------------------------------------------------------
// sheet.calc.decimal.divergence-corpus
// ---------------------------------------------------------------------------

/// The f64 half ALWAYS runs: every corpus row, replayed through `F64`, equals
/// its `f64` golden EXACTLY (bit-for-bit). This proves the corpus goldens are
/// the true IEEE-754 values Excel itself would carry (D-6) — and it runs in the
/// default (no-feature) build, so the evidence never bit-rots.
#[test]
fn sheet_calc_decimal_divergence_f64_matches_golden() {
    for c in load_corpus() {
        let got = replay::<F64>(&c.op, &c.operands);
        assert_eq!(
            got.to_bits(),
            c.f64_golden.to_bits(),
            "[{}] f64 replay {got} != golden {} (op {} {})",
            c.id,
            c.f64_golden,
            c.op,
            c.operands
        );
    }
}

/// The decimal half (cfg-gated): every corpus row, replayed through `Decimal`,
/// equals its `decimal` golden; and the `diverges` flag is exactly
/// `f64_golden != decimal_golden`. This is the divergence evidence — and the
/// proof the [`Decimal`] backend drops straight into the same `replay` code as
/// `F64` (D-6 seam).
#[cfg(feature = "exact-decimal")]
#[test]
fn sheet_calc_decimal_divergence_decimal_is_exact() {
    use sheet_fn::num_decimal::Decimal;

    for c in load_corpus() {
        let dec = replay::<Decimal>(&c.op, &c.operands);
        assert_eq!(
            dec.to_bits(),
            c.decimal_golden.to_bits(),
            "[{}] decimal replay {dec} != golden {} (op {} {})",
            c.id,
            c.decimal_golden,
            c.op,
            c.operands
        );

        // The corpus `diverges` flag must agree with the actual f64-vs-decimal
        // comparison at the f64 boundary.
        let f = replay::<F64>(&c.op, &c.operands);
        let actually_diverges = f.to_bits() != dec.to_bits();
        assert_eq!(
            actually_diverges, c.diverges,
            "[{}] diverges flag wrong: f64={f}, decimal={dec}, corpus says {}",
            c.id, c.diverges
        );

        // Spot-check the marquee row: where f64 fails, decimal is right.
        if c.id == "classic-0.1+0.2" {
            assert_ne!(f, 0.3, "f64 0.1+0.2 should NOT be 0.3");
            assert_eq!(dec, 0.3, "decimal 0.1+0.2 SHOULD be 0.3");
        }
    }
}

/// At least one corpus row must actually diverge AND at least one must NOT —
/// the spike must be honest about both. (Runs in the default build off the
/// `diverges` column; the decimal test above proves the column is accurate.)
#[test]
fn sheet_calc_decimal_divergence_has_both_outcomes() {
    let cases = load_corpus();
    assert!(
        cases.iter().any(|c| c.diverges),
        "corpus has no divergence cases — it proves nothing"
    );
    assert!(
        cases.iter().any(|c| !c.diverges),
        "corpus has no convergence cases — it overstates the win"
    );
}

// ---------------------------------------------------------------------------
// sheet.calc.decimal.decimal-impl
// ---------------------------------------------------------------------------

/// The `Decimal` `Numeric` impl: trait round-trip + the four exact ops + exact
/// integer pow. Cfg-gated (the impl only exists under the feature).
#[cfg(feature = "exact-decimal")]
#[test]
fn sheet_calc_decimal_impl_trait_ops() {
    use sheet_fn::num_decimal::Decimal;

    assert_eq!(Decimal::from_f64(2.5).to_f64(), 2.5);
    assert_eq!(
        Decimal::from_f64(2.0).add(Decimal::from_f64(3.0)),
        Decimal::from_f64(5.0)
    );
    assert_eq!(
        Decimal::from_f64(5.0).sub(Decimal::from_f64(3.0)),
        Decimal::from_f64(2.0)
    );
    assert_eq!(
        Decimal::from_f64(2.0).mul(Decimal::from_f64(3.0)),
        Decimal::from_f64(6.0)
    );
    assert_eq!(
        Decimal::from_f64(6.0).div(Decimal::from_f64(3.0)),
        Decimal::from_f64(2.0)
    );
    // Integer pow is EXACT (1.1^2 = 1.21 in base-10; f64 1.1*1.1 drifts).
    assert_eq!(
        Decimal::from_f64(1.1).pow(Decimal::from_f64(2.0)).to_f64(),
        1.21
    );
    assert_ne!(1.1_f64 * 1.1_f64, 1.21_f64, "f64 1.1^2 drifts off 1.21");
}

/// The honest caveat: decimal is exact for TERMINATING decimals, not a
/// universal cure. `1/3` is non-terminating in base-10 too — decimal carries 28
/// nines for `1/3 * 3`, which is NOT the exact `1`, even though it rounds back
/// to `1.0` at the f64 boundary. The central honesty of the spike report.
#[cfg(feature = "exact-decimal")]
#[test]
fn sheet_calc_decimal_impl_thirds_caveat() {
    use sheet_fn::num_decimal::Decimal;

    let third = Decimal::from_f64(1.0).div(Decimal::from_f64(3.0));
    let back = third.mul(Decimal::from_f64(3.0));
    // The underlying decimal is 0.9999...9 (28 nines), NOT exactly 1. Compared
    // through the public `Decimal` newtype (no `rust_decimal` import needed):
    // `from_f64(1.0)` is the exact `1`, and the derived `PartialEq` delegates to
    // the carrier's value comparison.
    assert_ne!(
        back,
        Decimal::from_f64(1.0),
        "decimal 1/3*3 should carry 28 nines, not exact 1"
    );
    // ...but it rounds back to 1.0 at the f64 boundary (same as f64's own 1.0).
    assert_eq!(back.to_f64(), 1.0);
    // f64 also lands on exactly 1.0 here (lucky rounding) — so this is a
    // CONVERGENCE at the boundary despite different internal representations.
    assert_eq!((1.0_f64 / 3.0) * 3.0, 1.0);
}

// ---------------------------------------------------------------------------
// sheet.calc.decimal.spike-report
// ---------------------------------------------------------------------------

/// The written adopt/defer recommendation (`DECIMAL-SPIKE.md`) must EXIST at
/// the repo root and carry the load-bearing decision tokens, so the registry
/// row cannot be flipped `implemented` without the deliverable actually present.
/// (Always runs — the report is the D-6 spike's primary artifact.)
#[test]
fn sheet_calc_decimal_spike_report_exists() {
    let path = repo_root().join("DECIMAL-SPIKE.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("DECIMAL-SPIKE.md missing at {}: {e}", path.display()));
    assert!(
        text.len() > 1000,
        "DECIMAL-SPIKE.md is suspiciously short ({} bytes) — the spike report must be substantive",
        text.len()
    );
    // The report must state a clear recommendation and cite the carrier + D-6.
    for token in ["rust_decimal", "D-6", "f64 stays the v1 default"] {
        assert!(
            text.contains(token),
            "DECIMAL-SPIKE.md must mention `{token}` (the spike's load-bearing facts)"
        );
    }
    // A clear ADOPT-as-opt-in / DEFER verdict must be present.
    assert!(
        text.contains("ADOPT") || text.contains("DEFER"),
        "DECIMAL-SPIKE.md must state an ADOPT/DEFER recommendation"
    );
}
