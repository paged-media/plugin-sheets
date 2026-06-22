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

//! Full number-format engine conformance — the M1 FORMAT2 track (spec §9;
//! registry `registry/features/format2.yaml`). Drives the
//! `corpus/format-corpus/{elapsed,fractions,conditional,color,padding,
//! currency}.golden.tsv` goldens through [`sheet_format`] and pins each
//! adopted Excel ruling with targeted asserts.
//!
//! Self-contained: a tiny in-file TSV loader (no `sheet-conformance` lib
//! import) so format rows whose 4th `expected` column may be blank (a hidden
//! value) parse without the generic 4-column-strict loader — mirroring
//! `format.rs`. The same TSVs are re-consumed by the Phase-2 LibreOffice
//! oracle.
//!
//! ## Excel rulings documented here (the bug-for-bug ledger)
//!
//! - **elapsed-brackets** (`sheet.format.elapsed-brackets`): `[h]`/`[m]`/`[s]`
//!   are TOTAL elapsed accumulators over the whole serial, not the modular
//!   wall-clock component — `[h]` over `1.5` days renders `36`, while a `:mm`
//!   in the same code keeps its modular `00`. `[hh]`/`[mm]`/`[ss]` set a
//!   minimum field width.
//! - **fractions** (`sheet.format.fractions`): the fitted form fits the best
//!   `n/d` within the denominator-digit budget (smaller `d` wins ties); the
//!   fixed form (`# ?/16`) reduces to the literal denominator; a numerator
//!   that rounds up carries into the integer (`# ??/??` of `2.96` -> `2 24/25`,
//!   not `3 0/...`); `?` slots space-pad to their width (a documented T0
//!   alignment approximation, e.g. `# ??/??` of `2/3` -> `"  2/ 3"`); a zero
//!   fraction with an integer part blanks the slot (`# ?/?` of `4` -> `"4    "`
//!   with trailing spaces).
//! - **conditional-sections** (`sheet.format.conditional-sections`): the first
//!   conditioned section whose test passes (against the raw signed value) wins;
//!   otherwise the first UNCONDITIONED section is the "otherwise" default. NO
//!   selected section auto-prefixes a minus — a matched conditional section
//!   owns its sign, and the unconditioned fallthrough behaves like the negative
//!   section (the `#,##0;#,##0` minus-suppression rule), so `[>=100]0;0` of
//!   `-5` is `5` and `[>=100]0;-0` of `-5` is `-5` (not the doubled `--5`).
//! - **color-brackets** (`sheet.format.color-brackets`): the eight named colors
//!   are a SIDECAR — they never change the glyphs, only the cell color — so
//!   [`format_value`] drops the color and [`format_value_styled`] surfaces it.
//!   The reported color follows section selection. Indexed `[ColorN]` palette
//!   entries are not modeled (dropped, like an unknown bracket).
//! - **padding** (`sheet.format.padding`): the `*x` repeat-fill char fills a
//!   column to its width in Excel; the engine has NO column width, so T0 emits
//!   the fill char EXACTLY ONCE at its position (column-width expansion is the
//!   typeset lane's job downstream).
//! - **locale-currency-token** (`sheet.format.locale-currency-token`):
//!   `[$<symbol>-<locale-hex>]` emits exactly the SYMBOL portion; a pure
//!   `[$-409]` locale tag contributes no literal. UPDATED by the M3 LOCALIZATION
//!   track (spec §9, D-8; registry `locale.yaml`
//!   `sheet.format.locale.locale-from-workbook`): the `-<LCID>` suffix is NO
//!   longer dropped — it selects the RENDERED separators for the D-8 v1 set, so
//!   `[$€-407]#,##0` of `1234` is `€1.234` (de-DE `.` grouping), while every
//!   non-de LCID (`409`, `809`, `411`, `414`, …) stays en grouping (en is the
//!   unmodelled-LCID fallback). The symbol/position rules are unchanged.

use sheet_core::{CellValue, DateSystem, Locale};
use sheet_format::{compile, format_value, format_value_styled, FormatColor, FormatCtx};
use std::path::PathBuf;

/// One golden row: `id<TAB>format_code<TAB>value<TAB>expected`. The 4th
/// column may be absent/empty (a hidden value renders to "").
struct Row {
    id: String,
    code: String,
    value: String,
    expected: String,
}

/// Load a format-corpus TSV by repo-relative path. Skips `#` comments and
/// blank lines. Accepts 3 or 4 columns (a 3-column row means `expected` is
/// the empty string). Mirrors `format.rs::load`.
fn load(repo_relative: &str) -> Vec<Row> {
    let path: PathBuf = repo_root().join(repo_relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("format corpus: cannot read {}: {e}", path.display()));
    let mut rows = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 || cols.len() > 4 {
            panic!(
                "format corpus: {}:{} has {} columns, expected 3 or 4 \
                 (id<TAB>code<TAB>value[<TAB>expected])",
                path.display(),
                lineno + 1,
                cols.len()
            );
        }
        rows.push(Row {
            id: cols[0].to_string(),
            code: cols[1].to_string(),
            value: cols[2].to_string(),
            expected: cols.get(3).copied().unwrap_or("").to_string(),
        });
    }
    assert!(
        rows.len() >= 10,
        "format corpus {repo_relative} must carry >= 10 rows (has {})",
        rows.len()
    );
    rows
}

/// Repo root: `CARGO_MANIFEST_DIR/..`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has a parent (the repo root)")
        .to_path_buf()
}

/// Parse a corpus `value` column into a [`CellValue`]: `text:...` ⇒ text,
/// `bool:true`/`bool:false` ⇒ bool, otherwise a bare f64.
fn parse_value(v: &str) -> CellValue {
    if let Some(t) = v.strip_prefix("text:") {
        CellValue::from(t)
    } else if let Some(b) = v.strip_prefix("bool:") {
        CellValue::Bool(b == "true")
    } else {
        CellValue::Number(v.parse().unwrap_or_else(|_| panic!("bad value {v:?}")))
    }
}

/// Run every row of a corpus through the formatter and assert byte-equality
/// (under the 1900 date system, as the elapsed brackets need a serial axis).
fn run_corpus(repo_relative: &str) {
    let ctx = FormatCtx::new(DateSystem::Date1900, Locale::EnUs);
    for row in load(repo_relative) {
        let fmt = compile(&row.code)
            .unwrap_or_else(|e| panic!("[{}] compile {:?} failed: {e}", row.id, row.code));
        let got = format_value(&parse_value(&row.value), &fmt, &ctx);
        assert_eq!(
            got, row.expected,
            "[{}] format_value({:?}, {:?}) = {:?}, want {:?}",
            row.id, row.value, row.code, got, row.expected
        );
    }
}

/// Format a numeric code under the 1900 system (convenience for asserts).
fn nfmt(code: &str, x: f64) -> String {
    let f = compile(code).unwrap();
    format_value(
        &CellValue::Number(x),
        &f,
        &FormatCtx::new(DateSystem::Date1900, Locale::EnUs),
    )
}

// ---- Registry-pointer test fns (one per `format2.yaml` row). ----

/// `sheet.format.elapsed-brackets` — total elapsed accumulators.
#[test]
fn sheet_format_elapsed_brackets() {
    run_corpus("corpus/format-corpus/elapsed.golden.tsv");

    // [h] is TOTAL hours over the whole serial, not the modular wall clock.
    assert_eq!(nfmt("[h]", 1.5), "36");
    assert_eq!(nfmt("[h]", 2.0), "48");
    // A modular :mm next to [h] keeps its wall-clock component.
    assert_eq!(nfmt("[h]:mm", 1.5), "36:00");
    assert_eq!(nfmt("[h]:mm", 0.520_833_333_3), "12:30");
    // [m] and [s] total minutes / seconds across the whole serial.
    assert_eq!(nfmt("[m]", 1.0), "1440");
    assert_eq!(nfmt("[s]", 0.001_157_407_4), "100");
    // [hh]/[mm]/[ss] set a minimum field width (zero-padded).
    assert_eq!(nfmt("[mm]", 0.000_694_444_4), "01");
    assert_eq!(nfmt("[hh]:mm:ss", 1.5), "36:00:00");
}

/// `sheet.format.fractions` — best-denominator fitting and fixed denominators.
#[test]
fn sheet_format_fractions() {
    run_corpus("corpus/format-corpus/fractions.golden.tsv");

    // Single-digit fit (smaller denominator wins ties).
    assert_eq!(nfmt("# ?/?", 0.5), " 1/2");
    assert_eq!(nfmt("# ?/?", 2.25), "2 1/4");
    assert_eq!(nfmt("# ?/?", 0.666_666_666_7), " 2/3");
    // Two-digit fit reaches /16.
    assert_eq!(nfmt("# ??/??", 0.3125), "  5/16");
    // Fixed denominator reduces to the literal denominator (not lowest terms).
    assert_eq!(nfmt("# ?/16", 0.5), " 8/16");
    // A numerator that rounds up to the denominator carries into the integer.
    assert_eq!(nfmt("# ??/??", 2.96), "2 24/25");
    // Improper fraction (no integer placeholder).
    assert_eq!(nfmt("?/?", 0.5), "1/2");
    // Zero fraction with an integer part blanks the slot to spaces (trailing
    // whitespace — asserted here, not in the trailing-whitespace-free corpus).
    assert_eq!(nfmt("# ?/?", 4.0), "4    ");
    // The ? denominator slot space-pads to its width (documented T0 alignment).
    assert_eq!(nfmt("# ??/??", 0.666_666_666_7), "  2/ 3");
    // Negative carries its minus.
    assert_eq!(nfmt("# ?/?", -2.25), "-2 1/4");
}

/// `sheet.format.conditional-sections` — comparison-gated section selection.
#[test]
fn sheet_format_conditional_sections() {
    run_corpus("corpus/format-corpus/conditional.golden.tsv");

    // Matched conditional section vs the unconditioned default.
    assert_eq!(nfmt("[>=100]0\"+\";0", 150.0), "150+");
    assert_eq!(nfmt("[>=100]0\"+\";0", 50.0), "50");
    // The unconditioned fallthrough is the "otherwise" (negative) section: it
    // does NOT auto-prefix a minus (the #,##0;#,##0 minus-suppression rule).
    assert_eq!(nfmt("[>=100]0;0", -5.0), "5");
    // The author's own minus is honored exactly once — no doubled "--5".
    assert_eq!(nfmt("[>=100]0;-0", -5.0), "-5");
    // A matched [<0] section also suppresses the auto-minus (author owns sign).
    assert_eq!(nfmt("[>100]0;[<0]0;0", -5.0), "5");
    // Comparison is against the RAW signed value.
    assert_eq!(nfmt("[<=50]\"lo\";[>50]\"hi\"", 30.0), "lo");
    assert_eq!(nfmt("[<=50]\"lo\";[>50]\"hi\"", 80.0), "hi");
    // Conditions compose with scaling/literals in the same section.
    assert_eq!(nfmt("[>=1000]0,\"K\";0", 2500.0), "3K");
}

/// `sheet.format.color-brackets` — the color sidecar.
#[test]
fn sheet_format_color_brackets() {
    run_corpus("corpus/format-corpus/color.golden.tsv");

    let ctx = FormatCtx::new(DateSystem::Date1900, Locale::EnUs);
    let styled = |code: &str, x: f64| {
        let f = compile(code).unwrap();
        format_value_styled(&CellValue::Number(x), &f, &ctx)
    };

    // The string is identical to a no-color code; the color is a sidecar.
    assert_eq!(
        styled("[Red]0.00", 5.0),
        ("5.00".to_string(), Some(FormatColor::Red))
    );
    assert_eq!(nfmt("[Red]0.00", 5.0), "5.00"); // frozen format_value drops it
                                                // All eight named colors parse.
    assert_eq!(styled("[Blue]0", 3.0).1, Some(FormatColor::Blue));
    assert_eq!(styled("[Green]0", 1.0).1, Some(FormatColor::Green));
    assert_eq!(styled("[Black]0", 1.0).1, Some(FormatColor::Black));
    assert_eq!(styled("[White]0", 1.0).1, Some(FormatColor::White));
    assert_eq!(styled("[Cyan]0", 1.0).1, Some(FormatColor::Cyan));
    assert_eq!(styled("[Magenta]0", 1.0).1, Some(FormatColor::Magenta));
    assert_eq!(styled("[Yellow]0", 1.0).1, Some(FormatColor::Yellow));
    // The color follows section selection (negatives blue, positives red).
    assert_eq!(styled("[Red]0.0;[Blue]0.0", 5.0).1, Some(FormatColor::Red));
    assert_eq!(
        styled("[Red]0.0;[Blue]0.0", -5.0).1,
        Some(FormatColor::Blue)
    );
    // Indexed [ColorN] palette entries are not modeled — no sidecar color.
    assert_eq!(styled("[Color12]0", 8.0).1, None);
    // A color bracket on a text section colors text values too.
    let ftext = compile("[Red]@").unwrap();
    let (s, c) = format_value_styled(&CellValue::from("hi"), &ftext, &ctx);
    assert_eq!((s.as_str(), c), ("hi", Some(FormatColor::Red)));
}

/// `sheet.format.padding` — the `*x` repeat-fill char (emitted once in T0).
#[test]
fn sheet_format_padding() {
    run_corpus("corpus/format-corpus/padding.golden.tsv");

    // The fill char is emitted EXACTLY ONCE (no column width in the engine).
    assert_eq!(nfmt("*-0", 5.0), "-5");
    assert_eq!(nfmt("0*x", 5.0), "5x");
    assert_eq!(nfmt("0\"x\"*-", 5.0), "5x-");
    // The classic accounting `$* #,##0` — fill once between $ and the number.
    assert_eq!(nfmt("$* #,##0", 1234.0), "$ 1,234");
    // The fill char composes with the auto-minus (single-section negative).
    assert_eq!(nfmt("*-0", -5.0), "--5");
    // The companion _x (skip a char width) is approximated as one space (M0).
    assert_eq!(nfmt("_(0", 5.0), " 5");
}

/// `sheet.format.locale-currency-token` — `[$symbol-locale]` parsing/emit.
#[test]
fn sheet_format_locale_currency_token() {
    run_corpus("corpus/format-corpus/currency.golden.tsv");

    // The symbol portion is emitted; the -locale suffix selects the separators
    // (M3 localization track): en LCIDs keep en grouping, de LCID 407 localizes.
    assert_eq!(nfmt("[$$-409]#,##0", 1234.0), "$1,234");
    assert_eq!(nfmt("[$€-407]#,##0", 1234.0), "€1.234"); // de-DE "." grouping
    assert_eq!(nfmt("[$£-809]#,##0.00", 12.5), "£12.50"); // en-GB stays en
                                                          // A pure locale tag [$-409] has an empty symbol — no literal contributed.
    assert_eq!(nfmt("[$-409]#,##0", 1234.0), "1,234");
    // A bare [$USD] (no -locale) emits the whole symbol "USD".
    assert_eq!(nfmt("[$USD]0", 5.0), "USD5");
    // The currency literal sits at its token position (prefix or suffix).
    assert_eq!(nfmt("#,##0\" \"[$kr-414]", 1234.0), "1,234 kr");
    // A single-section negative auto-signs around the currency literal.
    assert_eq!(nfmt("[$$-409]#,##0", -1234.0), "-$1,234");
}
