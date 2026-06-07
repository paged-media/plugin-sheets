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

//! Parser-internal helpers for assembling [`sheet_core::CellRef`] /
//! [`sheet_core::RangeRef`] from lexer pieces (spec §6.1). The sheet
//! qualifier resolved by [`crate::ParseCtx`] rides on both endpoints; a
//! range's qualifier applies to the whole range (`Sheet1!A1:B2`).

use sheet_core::{CellRef, RangeRef, SheetId};

/// Build a [`CellRef`] from a lexed A1 cell on a given sheet.
pub(crate) fn cell(sheet: SheetId, row: u32, col: u32, row_abs: bool, col_abs: bool) -> CellRef {
    CellRef {
        sheet,
        row,
        col,
        row_abs,
        col_abs,
    }
}

/// Fold two cell endpoints into a [`RangeRef`]. Both endpoints share the
/// `start`'s sheet (a range qualifier applies to the whole range, §6.1).
pub(crate) fn range(start: CellRef, end: CellRef) -> RangeRef {
    RangeRef {
        start,
        end: CellRef {
            sheet: start.sheet,
            ..end
        },
    }
}
