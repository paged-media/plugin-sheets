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

//! Cell and range references (spec §5.1). The internal model is 0-based
//! `(row, col)`; A1 notation is purely a display/parse concern handled by
//! the helpers here. Absolute flags (`$`) ride on `CellRef` so a parsed
//! formula reference round-trips losslessly.

use compact_str::CompactString;

/// Worksheet index within a workbook (ECMA-376 caps sheets well under u16).
pub type SheetId = u16;

/// 0-based last row (1_048_576 rows, the OOXML/Excel grid limit).
pub const MAX_ROW: u32 = 1_048_575;
/// 0-based last column ("XFD", 16_384 columns).
pub const MAX_COL: u32 = 16_383;

/// A single cell reference. Ordering is derived (`sheet`, `row`, `col`,
/// then the absolute flags) — used to key sparse maps and sort spills.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellRef {
    pub sheet: SheetId,
    pub row: u32,
    pub col: u32,
    pub row_abs: bool,
    pub col_abs: bool,
}

/// A rectangular range. Not necessarily normalized (`parse` may yield
/// `end < start`); call [`RangeRef::normalized`] before iterating.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RangeRef {
    pub start: CellRef,
    pub end: CellRef,
}

/// 0-based column index -> A1 column label. `0 -> "A"`, `26 -> "AA"`,
/// `16383 -> "XFD"` (bijective base-26).
pub fn col_to_a1(col: u32) -> CompactString {
    // Bijective base-26: there is no "zero digit", so subtract 1 each step.
    let mut n = col as u64 + 1;
    let mut buf = [0u8; 8];
    let mut i = buf.len();
    while n > 0 {
        let rem = (n - 1) % 26;
        i -= 1;
        buf[i] = b'A' + rem as u8;
        n = (n - 1) / 26;
    }
    // SAFETY-free: the slice is ASCII by construction.
    CompactString::new(std::str::from_utf8(&buf[i..]).unwrap())
}

/// A1 column label -> 0-based column index. Case-insensitive. Rejects the
/// empty string, non-letters, and any label past `MAX_COL` ("XFE"+).
pub fn a1_to_col(s: &str) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut acc: u64 = 0;
    for b in s.bytes() {
        let d = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a',
            _ => return None,
        };
        acc = acc.checked_mul(26)?.checked_add(d as u64 + 1)?;
        if acc > MAX_COL as u64 + 1 {
            return None;
        }
    }
    let col = acc - 1;
    if col > MAX_COL as u64 {
        None
    } else {
        Some(col as u32)
    }
}

/// Parse an A1 cell token into `(row, col, row_abs, col_abs)`. The column
/// then row order is required (`$B$7`, `B7`, `$B7`, `B$7`). The row token
/// is 1-based in A1 text; the returned row is 0-based. Rejects row `0`,
/// rows past `MAX_ROW`, columns past `MAX_COL`, and any trailing junk.
pub fn parse_a1(s: &str) -> Option<(u32, u32, bool, bool)> {
    let bytes = s.as_bytes();
    let mut i = 0;

    let col_abs = if bytes.first() == Some(&b'$') {
        i += 1;
        true
    } else {
        false
    };

    // Column letters.
    let col_start = i;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == col_start {
        return None;
    }
    let col = a1_to_col(&s[col_start..i])?;

    let row_abs = if bytes.get(i) == Some(&b'$') {
        i += 1;
        true
    } else {
        false
    };

    // Row digits.
    let row_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == row_start {
        return None;
    }
    // Trailing junk (e.g. "A1B").
    if i != bytes.len() {
        return None;
    }
    // 1-based in text; parse and reject leading-zero-only / overflow.
    let row_1based: u64 = s[row_start..i].parse().ok()?;
    if row_1based == 0 {
        return None;
    }
    let row = row_1based - 1;
    if row > MAX_ROW as u64 {
        return None;
    }

    Some((row as u32, col, row_abs, col_abs))
}

/// Format a 0-based `(row, col)` back into A1 text with `$` flags.
pub fn format_a1(row: u32, col: u32, row_abs: bool, col_abs: bool) -> CompactString {
    let mut out = CompactString::default();
    if col_abs {
        out.push('$');
    }
    out.push_str(&col_to_a1(col));
    if row_abs {
        out.push('$');
    }
    // A1 row text is 1-based.
    out.push_str(itoa_u32(row + 1).as_str());
    out
}

/// Minimal allocation-free u32 -> CompactString (avoids pulling itoa as a
/// dep for a leaf crate).
fn itoa_u32(mut n: u32) -> CompactString {
    if n == 0 {
        return CompactString::new("0");
    }
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    CompactString::new(std::str::from_utf8(&buf[i..]).unwrap())
}

impl RangeRef {
    /// Return a copy with `start <= end` on both axes. The sheet is taken
    /// from `start`; absolute flags follow whichever endpoint won the axis.
    pub fn normalized(&self) -> RangeRef {
        let (r0, r0a, r1, r1a) = if self.start.row <= self.end.row {
            (
                self.start.row,
                self.start.row_abs,
                self.end.row,
                self.end.row_abs,
            )
        } else {
            (
                self.end.row,
                self.end.row_abs,
                self.start.row,
                self.start.row_abs,
            )
        };
        let (c0, c0a, c1, c1a) = if self.start.col <= self.end.col {
            (
                self.start.col,
                self.start.col_abs,
                self.end.col,
                self.end.col_abs,
            )
        } else {
            (
                self.end.col,
                self.end.col_abs,
                self.start.col,
                self.start.col_abs,
            )
        };
        RangeRef {
            start: CellRef {
                sheet: self.start.sheet,
                row: r0,
                col: c0,
                row_abs: r0a,
                col_abs: c0a,
            },
            end: CellRef {
                sheet: self.start.sheet,
                row: r1,
                col: c1,
                row_abs: r1a,
                col_abs: c1a,
            },
        }
    }

    /// True if `(sheet, row, col)` falls inside the (normalized) box.
    pub fn contains(&self, sheet: SheetId, row: u32, col: u32) -> bool {
        let n = self.normalized();
        sheet == n.start.sheet
            && row >= n.start.row
            && row <= n.end.row
            && col >= n.start.col
            && col <= n.end.col
    }

    /// Row count of the (normalized) box.
    pub fn rows(&self) -> u32 {
        let n = self.normalized();
        n.end.row - n.start.row + 1
    }

    /// Column count of the (normalized) box.
    pub fn cols(&self) -> u32 {
        let n = self.normalized();
        n.end.col - n.start.col + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr(row: u32, col: u32) -> CellRef {
        CellRef {
            sheet: 0,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    #[test]
    fn col_a1_roundtrip() {
        for col in [0u32, 1, 25, 26, 27, 51, 52, 701, 702, 703, 16383] {
            let s = col_to_a1(col);
            assert_eq!(a1_to_col(&s), Some(col), "label {s} for col {col}");
        }
        assert_eq!(col_to_a1(0).as_str(), "A");
        assert_eq!(col_to_a1(25).as_str(), "Z");
        assert_eq!(col_to_a1(26).as_str(), "AA");
        assert_eq!(col_to_a1(16383).as_str(), "XFD");
    }

    #[test]
    fn a1_to_col_rejects_and_accepts() {
        assert_eq!(a1_to_col(""), None);
        assert_eq!(a1_to_col("XFE"), None); // one past XFD
        assert_eq!(a1_to_col("XFZ"), None);
        assert_eq!(a1_to_col("AAAA"), None); // overflow
        assert_eq!(a1_to_col("a"), Some(0)); // lowercase accepted
        assert_eq!(a1_to_col("xfd"), Some(16383));
        assert_eq!(a1_to_col("A1"), None); // digit not a letter
    }

    #[test]
    fn parse_format_a1_roundtrip() {
        for (s, row, col, ra, ca) in [
            ("A1", 0, 0, false, false),
            ("B7", 6, 1, false, false),
            ("$B$7", 6, 1, true, true),
            ("$B7", 6, 1, false, true),
            ("B$7", 6, 1, true, false),
            ("XFD1048576", MAX_ROW, MAX_COL, false, false),
        ] {
            assert_eq!(parse_a1(s), Some((row, col, ra, ca)), "parse {s}");
            assert_eq!(format_a1(row, col, ra, ca).as_str(), s, "format {s}");
        }
    }

    #[test]
    fn parse_a1_rejections() {
        assert_eq!(parse_a1("A0"), None); // row 0
        assert_eq!(parse_a1("1A"), None); // row before col
        assert_eq!(parse_a1("A1B"), None); // trailing junk
        assert_eq!(parse_a1(""), None);
        assert_eq!(parse_a1("$"), None);
        assert_eq!(parse_a1("A"), None); // no row
        assert_eq!(parse_a1("1"), None); // no col
        assert_eq!(parse_a1("XFE1"), None); // col overflow
        assert_eq!(parse_a1("A1048577"), None); // row overflow
    }

    #[test]
    fn range_normalized_and_contains() {
        let r = RangeRef {
            start: cr(5, 3),
            end: cr(2, 1),
        };
        let n = r.normalized();
        assert_eq!(n.start.row, 2);
        assert_eq!(n.start.col, 1);
        assert_eq!(n.end.row, 5);
        assert_eq!(n.end.col, 3);
        assert_eq!(n.rows(), 4);
        assert_eq!(n.cols(), 3);

        // corners + interior in, outside out
        assert!(r.contains(0, 2, 1));
        assert!(r.contains(0, 5, 3));
        assert!(r.contains(0, 3, 2));
        assert!(!r.contains(0, 1, 1)); // above
        assert!(!r.contains(0, 2, 0)); // left
        assert!(!r.contains(0, 6, 3)); // below
        assert!(!r.contains(1, 3, 2)); // wrong sheet

        // single cell
        let one = RangeRef {
            start: cr(4, 4),
            end: cr(4, 4),
        };
        assert_eq!(one.rows(), 1);
        assert_eq!(one.cols(), 1);
        assert!(one.contains(0, 4, 4));
    }
}
