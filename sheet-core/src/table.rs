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

//! Excel structured tables (ListObjects, spec §6.4 / ECMA-376 §18.5
//! `table`). A [`Table`] names a rectangular [`RangeRef`] region whose
//! columns carry header labels, so a formula can address it symbolically
//! (`Table1[Col]`) instead of by A1 geometry — see
//! [`crate::ast::StructuredRef`]. This is the **model** half (M1 Phase A):
//! the type plus name/column resolution helpers. Parsing structured-ref
//! TEXT, evaluating it, and the XLSX `table` part are M1 Phase B (tables
//! track); this file freezes the shape they build on.

use crate::refs::{RangeRef, SheetId};
use crate::SheetModel;
use compact_str::CompactString;

/// One Excel structured table (a named, columned region). `range` is the
/// FULL extent — it includes the header row (when [`header_row`]) and the
/// totals row (when [`totals_row`]), so the data body is `range` minus
/// those edge rows. `columns` lists the header labels left-to-right; their
/// order is the column order within `range`. `style_name` is the optional
/// table-style id carried for round-trip / lowering.
///
/// [`header_row`]: Table::header_row
/// [`totals_row`]: Table::totals_row
#[derive(Clone, Debug)]
pub struct Table {
    pub name: CompactString,
    /// Full extent incl. header/totals rows.
    pub range: RangeRef,
    pub columns: Vec<CompactString>,
    pub header_row: bool,
    pub totals_row: bool,
    pub style_name: Option<CompactString>,
}

impl Table {
    /// 0-based offset of `name` within [`Table::range`]'s columns
    /// (case-insensitive, Excel column-name semantics). Returns the offset
    /// from the range's left edge — add `range.start.col` for an absolute
    /// column. `None` if no column matches.
    pub fn column_index(&self, name: &str) -> Option<u32> {
        self.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }
}

impl SheetModel {
    /// Resolve a table by name across every sheet (case-insensitive, Excel
    /// table-name semantics — table names are workbook-scoped). Returns the
    /// owning [`SheetId`] and a borrow of the [`Table`]; first match wins.
    pub fn resolve_table(&self, name: &str) -> Option<(SheetId, &Table)> {
        for (i, ws) in self.sheets.iter().enumerate() {
            if let Some(t) = ws.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name)) {
                return Some((i as SheetId, t));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::CellRef;

    fn mk_range(sheet: SheetId, r0: u32, c0: u32, r1: u32, c1: u32) -> RangeRef {
        let cr = |row, col| CellRef {
            sheet,
            row,
            col,
            row_abs: false,
            col_abs: false,
        };
        RangeRef {
            start: cr(r0, c0),
            end: cr(r1, c1),
        }
    }

    fn sample_table(name: &str, sheet: SheetId) -> Table {
        Table {
            name: name.into(),
            range: mk_range(sheet, 0, 2, 9, 4), // C1:E10
            columns: vec!["Region".into(), "Units".into(), "Total".into()],
            header_row: true,
            totals_row: false,
            style_name: Some("TableStyleMedium2".into()),
        }
    }

    #[test]
    fn column_index_is_case_insensitive_offset() {
        let t = sample_table("Sales", 0);
        assert_eq!(t.column_index("Region"), Some(0));
        assert_eq!(t.column_index("units"), Some(1)); // case-insensitive
        assert_eq!(t.column_index("TOTAL"), Some(2));
        assert_eq!(t.column_index("Missing"), None);
    }

    #[test]
    fn resolve_table_across_sheets_case_insensitive() {
        let mut m = SheetModel::new();
        let s0 = m.add_sheet("Sheet1");
        let s1 = m.add_sheet("Data");
        m.sheet_mut(s1)
            .unwrap()
            .tables
            .push(sample_table("Sales", s1));

        let (sid, t) = m.resolve_table("sales").unwrap();
        assert_eq!(sid, s1);
        assert_eq!(t.name.as_str(), "Sales");
        // No table on Sheet1; unknown names resolve to None.
        assert!(m.sheet(s0).unwrap().tables.is_empty());
        assert!(m.resolve_table("nope").is_none());
    }

    #[test]
    fn resolve_table_first_match_wins() {
        let mut m = SheetModel::new();
        let s0 = m.add_sheet("A");
        let _s1 = m.add_sheet("B");
        m.sheet_mut(s0).unwrap().tables.push(sample_table("T", s0));
        // Duplicate names are an authoring error; the model returns the
        // first (lowest sheet index) deterministically.
        let (sid, _t) = m.resolve_table("T").unwrap();
        assert_eq!(sid, s0);
    }
}
