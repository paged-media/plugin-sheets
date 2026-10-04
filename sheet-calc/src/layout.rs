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

//! Layout doors (Wave 6): edits that change how cells LOOK or SIT, never
//! what they compute — cell styles (and the style table), merges, column
//! widths, row heights. None of them touches a value, a formula or the
//! dependency graph, so nothing recalculates and the dirty set is left as
//! it was (rebuilding the engine for these would mark every formula dirty).

use crate::Engine;
use sheet_core::{Cell, CellValue, RangeRef, SheetId, StyleId, StyleTable};
use std::collections::BTreeMap;

/// The non-value geometry of one worksheet, borrowed mutably.
pub struct SheetLayoutMut<'a> {
    /// Merged ranges.
    pub merges: &'a mut Vec<RangeRef>,
    /// Column widths in characters, by 0-based column.
    pub col_widths: &'a mut BTreeMap<u32, f64>,
    /// Row heights in points, by 0-based row.
    pub row_heights: &'a mut BTreeMap<u32, f64>,
}

impl Engine {
    /// Replace the workbook style table (the xlsx layer re-parses
    /// `styles.xml` after authoring a style; ids are positional, so every
    /// existing cell style keeps its meaning).
    pub fn replace_styles(&mut self, table: StyleTable) {
        self.model.styles = table;
    }

    /// Set one cell's style. A missing cell is created blank (a styled empty
    /// cell — a fill on an empty cell is real formatting); a blank,
    /// formula-less cell set back to the default style is removed. `false`
    /// for an unknown sheet.
    pub fn set_cell_style(&mut self, sheet: SheetId, row: u32, col: u32, style: StyleId) -> bool {
        let Some(ws) = self.model.sheet_mut(sheet) else {
            return false;
        };
        match ws.cells.get_mut(&(row, col)) {
            Some(c) => {
                c.style = style;
                if style == StyleId(0) && c.formula.is_none() && c.value == CellValue::Empty {
                    ws.cells.remove(&(row, col));
                }
            }
            None if style != StyleId(0) => {
                ws.cells.insert(
                    (row, col),
                    Cell {
                        value: CellValue::Empty,
                        formula: None,
                        style,
                    },
                );
            }
            None => {}
        }
        true
    }

    /// Mutable access to a sheet's merges and sizes (`None` for an unknown
    /// sheet).
    pub fn sheet_layout_mut(&mut self, sheet: SheetId) -> Option<SheetLayoutMut<'_>> {
        let ws = self.model.sheet_mut(sheet)?;
        Some(SheetLayoutMut {
            merges: &mut ws.merges,
            col_widths: &mut ws.col_widths,
            row_heights: &mut ws.row_heights,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EngineConfig;
    use sheet_core::SheetModel;

    #[test]
    fn style_door_creates_and_removes_blank_styled_cells() {
        let mut m = SheetModel::new();
        m.add_sheet("S");
        let mut e = Engine::new(m, EngineConfig::default());
        e.enter(0, 0, 0, "=1+1").unwrap();
        e.recalc_dirty();
        assert!(e.set_cell_style(0, 3, 3, StyleId(2)));
        assert_eq!(
            e.model().sheet(0).unwrap().cell(3, 3).unwrap().style,
            StyleId(2)
        );
        assert!(e.set_cell_style(0, 3, 3, StyleId(0)));
        assert!(e.model().sheet(0).unwrap().cell(3, 3).is_none());
        // A formula cell keeps its formula and value.
        assert!(e.set_cell_style(0, 0, 0, StyleId(1)));
        let c = e.model().sheet(0).unwrap().cell(0, 0).unwrap();
        assert!(c.formula.is_some());
        assert_eq!(c.value, CellValue::Number(2.0));
        assert!(!e.set_cell_style(9, 0, 0, StyleId(1)));
        // Nothing went dirty: a recalc changes nothing.
        assert!(e.recalc_dirty().changed.is_empty());
    }
}
