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

//! The four understood SpreadsheetML parts (spec §10): `workbook.xml`,
//! `sheetN.xml`, `sharedStrings.xml`, `styles.xml`. Each module is a pure
//! parser (bytes -> a parsed struct); the writer re-encodes from the model.

pub mod shared_strings;
pub mod styles;
pub mod tables;
pub mod workbook;
pub mod worksheet;
