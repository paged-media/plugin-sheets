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

//! The stored cell record (spec §5.1). A `Cell` is the value plus indices
//! into the workbook's interned formula and style tables — never the
//! payloads themselves, keeping the sparse cell map cheap to clone.

use crate::value::CellValue;

/// Index into `SheetModel::formulas`. A cell with `Some(FormulaId)` is a
/// formula cell whose `value` is the last computed result (cached).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FormulaId(pub u32);

/// Index into `StyleTable`. `StyleId(0)` is always the default style.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct StyleId(pub u32);

/// A populated grid cell. `value` defaults to `Empty`, `style` to the
/// default `StyleId(0)`, `formula` to `None` (a plain literal cell).
#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub value: CellValue,
    pub formula: Option<FormulaId>,
    pub style: StyleId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_default_is_empty_default_style() {
        let c = Cell::default();
        assert_eq!(c.value, CellValue::Empty);
        assert_eq!(c.formula, None);
        assert_eq!(c.style, StyleId(0));
    }
}
