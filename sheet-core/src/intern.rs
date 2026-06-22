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

//! Generic interner (spec §5.1). Deduplicates values to small `u32` ids —
//! the workbook model interns formulas and shared strings through it.
//! Insertion order is preserved (id == arrival index).

use std::collections::HashMap;
use std::hash::Hash;

/// Append-only value interner: `intern` dedups, returning a stable `u32`
/// id; `get` resolves it back. Ids are dense and ordered by first arrival.
#[derive(Debug)]
pub struct Interner<T> {
    values: Vec<T>,
    index: HashMap<T, u32>,
}

impl<T: Clone + Eq + Hash> Interner<T> {
    pub fn new() -> Self {
        Interner {
            values: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Intern `value`, returning its id. Equal values share an id.
    pub fn intern(&mut self, value: T) -> u32 {
        if let Some(&id) = self.index.get(&value) {
            return id;
        }
        let id = self.values.len() as u32;
        self.values.push(value.clone());
        self.index.insert(value, id);
        id
    }

    /// Resolve an id to its value, or `None` if out of range.
    pub fn get(&self, id: u32) -> Option<&T> {
        self.values.get(id as usize)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Iterate `(id, value)` in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.values.iter().enumerate().map(|(i, v)| (i as u32, v))
    }
}

impl<T: Clone + Eq + Hash> Default for Interner<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_returns_same_id() {
        let mut it: Interner<String> = Interner::new();
        let a = it.intern("x".into());
        let b = it.intern("y".into());
        let a2 = it.intern("x".into());
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(it.len(), 2);
    }

    #[test]
    fn stable_get_and_order() {
        let mut it: Interner<String> = Interner::new();
        assert!(it.is_empty());
        it.intern("first".into());
        it.intern("second".into());
        it.intern("first".into()); // dup, no new id
        it.intern("third".into());
        assert_eq!(it.get(0).map(String::as_str), Some("first"));
        assert_eq!(it.get(1).map(String::as_str), Some("second"));
        assert_eq!(it.get(2).map(String::as_str), Some("third"));
        assert_eq!(it.get(3), None);

        let collected: Vec<(u32, &str)> = it.iter().map(|(i, v)| (i, v.as_str())).collect();
        assert_eq!(collected, vec![(0, "first"), (1, "second"), (2, "third")]);
    }
}
