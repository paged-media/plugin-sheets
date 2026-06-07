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

//! Per-worksheet binding state held alongside the model.
//!
//! Each parsed worksheet ties a [`SheetId`] in the model to its OPC part
//! name (so the writer can find a dirty sheet's part) and carries that
//! sheet's captured unknown subtrees (preserve.rs) for re-emission on a
//! dirty re-encode.

use crate::preserve::CapturedSubtrees;
use sheet_core::SheetId;

/// Binds a model sheet to its XLSX part + preserved unknown subtrees.
#[derive(Debug, Clone)]
pub struct SheetBinding {
    pub sheet_id: SheetId,
    /// The worksheet part name (e.g. `xl/worksheets/sheet1.xml`).
    pub part_name: String,
    /// Unknown `<worksheet>` children captured at parse time.
    pub captured: CapturedSubtrees,
}
