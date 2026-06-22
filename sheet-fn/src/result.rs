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

//! The rich function result (spec §6.4, M1 array track). A classic kernel
//! returns a single [`CellValue`] through the scalar [`crate::dispatch`]
//! door; a dynamic-array kernel (registry `returns_array: true`) returns a
//! 2-D block through [`crate::dispatch::dispatch_rich`]. [`FnResult`] is the
//! union of the two. The scalar door stays total: it wraps an array kernel's
//! `#VALUE!` (the evaluator MUST use `dispatch_rich` for array rows).

use sheet_core::CellValue;

/// A function kernel's result. `Scalar` is the classic single value; `Array`
/// is a row-major 2-D block (outer = rows, inner = columns) — the spilled
/// region a dynamic-array function produces. M1 Phase B (spill track)
/// materializes an `Array` onto the sheet; Phase A only freezes the shape
/// and the `dispatch_rich` plumbing.
#[derive(Clone, Debug, PartialEq)]
pub enum FnResult {
    Scalar(CellValue),
    Array(Vec<Vec<CellValue>>),
}
