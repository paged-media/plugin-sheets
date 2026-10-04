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

//! OOXML storage prefixes for "future functions" ([MS-XLSX] §2.2.2 Formulas,
//! and the Microsoft "_xlfn prefix" documentation). Excel writes every
//! function introduced after the ECMA-376 (Excel 2007) base set as
//! `_xlfn.NAME(` in the `<f>` text — `_xlfn._xlws.` for `FILTER` and `SORT`
//! — and every `LET`/`LAMBDA` parameter as `_xlpm.name`. A reader that does
//! not strip them sees an unknown function; a writer that omits them makes
//! Excel show `#NAME?`. This module is the one table of which registered
//! functions carry which prefix; [`crate::print_ooxml`] adds them and
//! [`strip_storage_prefixes`] removes them on load.

/// Registered functions Excel stores with `_xlfn.` (Excel 2010 and later).
const XLFN: &[&str] = &[
    "AGGREGATE",
    "ANCHORARRAY",
    "ARABIC",
    "BASE",
    "BYCOL",
    "BYROW",
    "CHOOSECOLS",
    "CHOOSEROWS",
    "CONCAT",
    "DAYS",
    "DECIMAL",
    "DROP",
    "FORECAST.LINEAR",
    "FORMULATEXT",
    "HSTACK",
    "IFNA",
    "IFS",
    "ISFORMULA",
    "ISOWEEKNUM",
    "LAMBDA",
    "LET",
    "MAKEARRAY",
    "MAP",
    "MAXIFS",
    "MINIFS",
    "MODE.SNGL",
    "NETWORKDAYS.INTL",
    "NORM.DIST",
    "NORM.INV",
    "NORM.S.DIST",
    "NORM.S.INV",
    "NUMBERVALUE",
    "PERCENTILE.EXC",
    "PERCENTILE.INC",
    "PERCENTRANK.EXC",
    "PERCENTRANK.INC",
    "QUARTILE.EXC",
    "QUARTILE.INC",
    "RANDARRAY",
    "RANK.EQ",
    "REDUCE",
    "SCAN",
    "SEQUENCE",
    "SHEET",
    "SHEETS",
    "SINGLE",
    "SORTBY",
    "STDEV.P",
    "STDEV.S",
    "SWITCH",
    "TAKE",
    "TEXTAFTER",
    "TEXTBEFORE",
    "TEXTJOIN",
    "TEXTSPLIT",
    "TOCOL",
    "TOROW",
    "UNICHAR",
    "UNICODE",
    "UNIQUE",
    "VAR.P",
    "VAR.S",
    "VSTACK",
    "WORKDAY.INTL",
    "XLOOKUP",
    "XMATCH",
    "XOR",
];

/// Registered functions Excel stores with `_xlfn._xlws.` (worksheet-only
/// dynamic-array functions).
const XLWS: &[&str] = &["FILTER", "SORT"];

/// The storage prefix for a registered function name (upper-case), or `""`
/// for a base-set function.
pub fn storage_prefix(name: &str) -> &'static str {
    if XLWS.contains(&name) {
        "_xlfn._xlws."
    } else if XLFN.contains(&name) {
        "_xlfn."
    } else {
        ""
    }
}

/// Remove the OOXML storage prefixes (`_xlfn.`, `_xlws.`, `_xlpm.`, any
/// case) from formula text read out of an xlsx cell, outside string literals,
/// and turn the storage form `ANCHORARRAY(<ref>)` back into `<ref>#`. The
/// result is the display dialect [`crate::parse`] reads. Text without
/// prefixes is returned unchanged.
pub fn strip_storage_prefixes(text: &str) -> String {
    const PREFIXES: [&str; 3] = ["_xlfn.", "_xlws.", "_xlpm."];
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut in_quote = false; // a 'quoted sheet' name
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < text.len() {
        let c = bytes[i];
        if !in_quote && c == b'"' {
            in_string = !in_string;
            out.push('"');
            i += 1;
            continue;
        }
        if !in_string && c == b'\'' {
            in_quote = !in_quote;
            out.push('\'');
            i += 1;
            continue;
        }
        if !in_string && !in_quote && c == b'_' {
            let rest = &text[i..];
            if let Some(p) = PREFIXES
                .iter()
                // Byte-wise: `rest[..6]` may split a multi-byte char
                // (`_français!A1` panicked here).
                .find(|p| {
                    rest.len() >= p.len()
                        && rest.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
                })
            {
                // Only at an identifier start (not inside `A_xlfn.`).
                let prev_ident = out
                    .chars()
                    .last()
                    .is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.');
                if !prev_ident {
                    i += p.len();
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().expect("char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    rewrite_anchorarray(&out)
}

/// `ANCHORARRAY(X)` → `(X)#` outside string literals (the spill operator's
/// storage form). Unbalanced parentheses leave the text unchanged.
fn rewrite_anchorarray(text: &str) -> String {
    const KW: &str = "ANCHORARRAY(";
    let upper = text.to_ascii_uppercase();
    if !upper.contains(KW) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while i < text.len() {
        let c = text.as_bytes()[i];
        if c == b'"' {
            in_string = !in_string;
        }
        if !in_string && upper[i..].starts_with(KW) {
            let prev_ident = out
                .chars()
                .last()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.');
            if !prev_ident {
                // Find the matching ')'.
                let start = i + KW.len();
                let mut depth = 1usize;
                let mut j = start;
                let mut s = false;
                while j < text.len() && depth > 0 {
                    match text.as_bytes()[j] {
                        b'"' => s = !s,
                        b'(' if !s => depth += 1,
                        b')' if !s => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                if depth != 0 {
                    return text.to_string();
                }
                let inner = rewrite_anchorarray(&text[start..j - 1]);
                out.push_str(&inner);
                out.push('#');
                i = j;
                continue;
            }
        }
        let ch = text[i..].chars().next().expect("char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_after_underscore_does_not_panic() {
        // Found by the full corpus lane: a defined name over a sheet called
        // `_français` split the `ç` while probing for `_xlfn.`.
        assert_eq!(
            strip_storage_prefixes("_français!$A$1:$G$37"),
            "_français!$A$1:$G$37"
        );
    }

    #[test]
    fn strips_function_and_parameter_prefixes() {
        assert_eq!(
            strip_storage_prefixes("_xlfn.XLOOKUP(A1,B1:B3,C1:C3)"),
            "XLOOKUP(A1,B1:B3,C1:C3)"
        );
        assert_eq!(
            strip_storage_prefixes("_xlfn._xlws.FILTER(A1:A3,B1:B3)"),
            "FILTER(A1:A3,B1:B3)"
        );
        assert_eq!(
            strip_storage_prefixes("_xlfn.LET(_xlpm.x,1,_xlpm.x+1)"),
            "LET(x,1,x+1)"
        );
        // Inside a string literal the text is data, not a prefix.
        assert_eq!(
            strip_storage_prefixes("CONCATENATE(\"_xlfn.\",A1)"),
            "CONCATENATE(\"_xlfn.\",A1)"
        );
        assert_eq!(strip_storage_prefixes("SUM(A1:A3)"), "SUM(A1:A3)");
    }

    #[test]
    fn anchorarray_becomes_the_spill_operator() {
        assert_eq!(
            strip_storage_prefixes("SUM(_xlfn.ANCHORARRAY(A1))"),
            "SUM(A1#)"
        );
    }

    #[test]
    fn prefix_table() {
        assert_eq!(storage_prefix("XLOOKUP"), "_xlfn.");
        assert_eq!(storage_prefix("FILTER"), "_xlfn._xlws.");
        assert_eq!(storage_prefix("SUM"), "");
        assert_eq!(storage_prefix("SUMIFS"), "");
    }
}
