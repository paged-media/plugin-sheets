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

//! XLSX preservation payload handle (spec §5.2/§10.2). The preservation
//! invariant — "Paged never destroys a workbook" — requires the model to
//! carry the original OPC parts opaquely. The rich type (`OpcContainer`)
//! lives in `sheet-xlsx`; `sheet-core` holds it as `Box<dyn Any>` so it
//! stays a leaf crate with no XLSX dependency.

use std::any::Any;
use std::fmt;

/// Opaque slot for the round-trip preservation payload. `None` when the
/// model did not originate from (or has not yet been loaded with) an XLSX.
#[derive(Default)]
pub struct PreservedParts(pub Option<Box<dyn Any + Send + Sync>>);

impl fmt::Debug for PreservedParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print presence only — the payload type is opaque to this crate.
        f.debug_tuple("PreservedParts")
            .field(&self.0.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_reports_presence() {
        let empty = PreservedParts::default();
        assert_eq!(format!("{empty:?}"), "PreservedParts(false)");
        let full = PreservedParts(Some(Box::new(42u32)));
        assert_eq!(format!("{full:?}"), "PreservedParts(true)");
    }
}
