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

//! The stored cell value (spec §5.1). `CellValue` is the on-the-wire
//! result type — what a cell holds and what crosses the engine boundary.
//! Calculation diagnostics that are NOT storable values (notably
//! circular-reference detection) live in `sheet-calc`, never here.

use compact_str::CompactString;

/// A computed/stored cell value. `Empty` models a blank cell (distinct
/// from `Text("")`) and is the `Default`.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub enum CellValue {
    #[default]
    Empty,
    Number(f64),
    Text(CompactString),
    Bool(bool),
    Error(CellError),
}

/// The eight OOXML cell-error codes (ECMA-376 §18.17.4). `Copy`/`Eq`/`Hash`
/// so errors flow through interned literals and dedup cleanly.
///
/// NOTE: `#CIRCULAR!` is intentionally absent. Circularity is a
/// sheet-calc *diagnostic*, never a stored wire value — Excel surfaces it
/// as `0` (or `#REF!` under specific edits), so it has no place in the
/// canonical value enum.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub enum CellError {
    Div0,
    Value,
    Ref,
    Name,
    Num,
    Na,
    Null,
    Spill,
}

impl CellError {
    /// The display token, e.g. `Div0` -> `"#DIV/0!"`.
    pub fn as_str(self) -> &'static str {
        match self {
            CellError::Div0 => "#DIV/0!",
            CellError::Value => "#VALUE!",
            CellError::Ref => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Num => "#NUM!",
            CellError::Na => "#N/A",
            CellError::Null => "#NULL!",
            CellError::Spill => "#SPILL!",
        }
    }

    /// Parse a display token back into a code. Exact-match on punctuation,
    /// case-insensitive on the letters (`#div/0!` == `#DIV/0!`). Returns
    /// `None` on any non-error string.
    pub fn parse(s: &str) -> Option<CellError> {
        // ASCII-uppercase the letters; punctuation/digits are unaffected.
        let upper = s.to_ascii_uppercase();
        match upper.as_str() {
            "#DIV/0!" => Some(CellError::Div0),
            "#VALUE!" => Some(CellError::Value),
            "#REF!" => Some(CellError::Ref),
            "#NAME?" => Some(CellError::Name),
            "#NUM!" => Some(CellError::Num),
            "#N/A" => Some(CellError::Na),
            "#NULL!" => Some(CellError::Null),
            "#SPILL!" => Some(CellError::Spill),
            _ => None,
        }
    }
}

impl CellValue {
    /// True only for `Empty` (a blank cell). `Text("")` is NOT blank.
    pub fn is_blank(&self) -> bool {
        matches!(self, CellValue::Empty)
    }
}

impl From<f64> for CellValue {
    fn from(n: f64) -> Self {
        CellValue::Number(n)
    }
}

impl From<bool> for CellValue {
    fn from(b: bool) -> Self {
        CellValue::Bool(b)
    }
}

impl From<&str> for CellValue {
    fn from(s: &str) -> Self {
        CellValue::Text(CompactString::new(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [CellError; 8] = [
        CellError::Div0,
        CellError::Value,
        CellError::Ref,
        CellError::Name,
        CellError::Num,
        CellError::Na,
        CellError::Null,
        CellError::Spill,
    ];

    #[test]
    fn cellerror_parse_as_str_roundtrip() {
        for e in ALL {
            assert_eq!(CellError::parse(e.as_str()), Some(e));
        }
    }

    #[test]
    fn cellerror_parse_case_insensitive() {
        assert_eq!(CellError::parse("#div/0!"), Some(CellError::Div0));
        assert_eq!(CellError::parse("#n/a"), Some(CellError::Na));
        assert_eq!(CellError::parse("#Name?"), Some(CellError::Name));
    }

    #[test]
    fn cellerror_parse_rejects_junk() {
        assert_eq!(CellError::parse("#FOO!"), None);
        assert_eq!(CellError::parse(""), None);
        assert_eq!(CellError::parse("DIV/0"), None);
    }

    #[test]
    fn is_blank_only_for_empty() {
        assert!(CellValue::Empty.is_blank());
        assert!(!CellValue::from("").is_blank());
        assert!(!CellValue::from(0.0).is_blank());
    }

    #[test]
    fn from_conversions() {
        assert_eq!(CellValue::from(1.5), CellValue::Number(1.5));
        assert_eq!(CellValue::from(true), CellValue::Bool(true));
        assert_eq!(
            CellValue::from("hi"),
            CellValue::Text(CompactString::new("hi"))
        );
    }
}
