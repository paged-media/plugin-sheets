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

//! XLSX I/O errors (spec §10). Three shapes: a zip/OPC container failure, an
//! XML parse failure, and a structural failure (a well-formed zip/XML that is
//! not a recognizable SpreadsheetML package — missing `[Content_Types].xml`,
//! no `officeDocument` relationship, etc.). The preservation invariant means
//! we never error on *unknown* content; we only error on content we cannot
//! treat as a workbook at all.

use thiserror::Error;

/// Failure modes for [`XlsxDocument::open`](crate::XlsxDocument::open) and
/// [`save`](crate::XlsxDocument::save).
#[derive(Debug, Error)]
pub enum XlsxError {
    /// The OPC zip container could not be read or written.
    #[error("xlsx zip container error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// A modeled XML part was not well-formed.
    #[error("xlsx xml error: {0}")]
    Xml(#[from] quick_xml::Error),

    /// The package is well-formed but not a usable SpreadsheetML workbook
    /// (missing required parts/relationships, malformed refs, …).
    #[error("xlsx structure error: {0}")]
    Structure(String),
}

impl From<quick_xml::events::attributes::AttrError> for XlsxError {
    fn from(e: quick_xml::events::attributes::AttrError) -> Self {
        XlsxError::Xml(quick_xml::Error::InvalidAttr(e))
    }
}

impl From<std::io::Error> for XlsxError {
    fn from(e: std::io::Error) -> Self {
        // Decompression/read failures surface through the zip layer.
        XlsxError::Zip(zip::result::ZipError::Io(e))
    }
}
