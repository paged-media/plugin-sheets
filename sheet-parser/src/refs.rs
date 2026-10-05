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

//! Parser-internal helpers for assembling [`sheet_core::CellRef`] /
//! [`sheet_core::RangeRef`] from lexer pieces (spec §6.1). The sheet
//! qualifier resolved by [`crate::ParseCtx`] rides on both endpoints; a
//! range's qualifier applies to the whole range (`Sheet1!A1:B2`).

use sheet_core::{CellRef, RangeRef, SheetId, MAX_COL, MAX_ROW};

use crate::lexer::TokKind;

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

/// The [`RangeRef`] a whole-column (`A:C`) or whole-row (`1:3`) token stands
/// for on `sheet`; `None` for any other token. A whole column spans rows
/// `0..=MAX_ROW` with BOTH row flags absolute (so a copy/fill never moves the
/// row extent); a whole row spans columns `0..=MAX_COL`, column flags
/// absolute. The printer recognises exactly that shape and prints it back as
/// `A:C` / `1:3` (an explicit `A$1:A$1048576` therefore prints as `A:A`, as
/// Excel itself displays it).
pub(crate) fn band(sheet: SheetId, kind: &TokKind) -> Option<RangeRef> {
    let (start, end) = match *kind {
        TokKind::Cols { c0, c1, abs0, abs1 } => (
            cell(sheet, 0, c0, true, abs0),
            cell(sheet, MAX_ROW, c1, true, abs1),
        ),
        TokKind::Rows { r0, r1, abs0, abs1 } => (
            cell(sheet, r0, 0, abs0, true),
            cell(sheet, r1, MAX_COL, abs1, true),
        ),
        _ => return None,
    };
    Some(range(start, end))
}
