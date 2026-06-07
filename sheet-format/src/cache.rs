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

//! A format-code -> [`CompiledFormat`] memo (spec §9). Compiling a format
//! code is non-trivial; a worksheet reuses a handful of codes across
//! thousands of cells, so callers keep one [`FormatCache`] and look codes up
//! by string. Backed by an `FxHashMap` for fast, deterministic hashing.

use crate::parse::{compile, FormatError};
use crate::sections::CompiledFormat;
use rustc_hash::FxHashMap;

/// String-keyed memo of compiled format codes. A failed compile is NOT
/// cached (it is re-attempted on the next lookup), keeping the map free of
/// error sentinels.
#[derive(Default)]
pub struct FormatCache {
    map: FxHashMap<String, CompiledFormat>,
}

impl FormatCache {
    /// Look up (or compile-and-store) the format for `code`. Returns a
    /// borrow into the cache on success.
    pub fn get(&mut self, code: &str) -> Result<&CompiledFormat, FormatError> {
        if !self.map.contains_key(code) {
            let compiled = compile(code)?;
            self.map.insert(code.to_string(), compiled);
        }
        Ok(self.map.get(code).expect("just inserted"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memoizes() {
        let mut c = FormatCache::default();
        let a = c.get("0.00").unwrap().clone();
        let b = c.get("0.00").unwrap().clone();
        assert_eq!(a, b);
    }

    #[test]
    fn error_not_cached() {
        let mut c = FormatCache::default();
        assert!(c.get("0;0;0;0;0").is_err());
        // Map stays empty after a failed compile.
        assert!(c.get("0;0;0;0;0").is_err());
    }

    #[test]
    fn distinct_codes() {
        let mut c = FormatCache::default();
        let n = c.get("0").unwrap().pos.decimals();
        let m = c.get("0.000").unwrap().pos.decimals();
        assert_eq!(n, 0);
        assert_eq!(m, 3);
    }
}
