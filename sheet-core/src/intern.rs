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
//!
//! Each value is stored ONCE: the lookup index maps a value's 64-bit hash to
//! the ids carrying that hash and resolves collisions against the value
//! table. (Before 2026-10 the index was a `HashMap<T, u32>` — a second full
//! copy of every interned formula tree and string.)
//!
//! Ids are never freed: a formula replaced by an edit keeps its slot. Ids are
//! stored in cells (`FormulaId`) and must stay stable, so reclaiming needs a
//! sweep that remaps every live cell's id (e.g. at save or after a structural
//! edit) — a GC design, not done here.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use smallvec::SmallVec;

/// Append-only value interner: `intern` dedups, returning a stable `u32`
/// id; `get` resolves it back. Ids are dense and ordered by first arrival.
#[derive(Debug)]
pub struct Interner<T> {
    values: Vec<T>,
    /// Value hash → the ids whose value has that hash (almost always one).
    index: HashMap<u64, SmallVec<[u32; 1]>>,
}

/// A deterministic 64-bit hash of a value (`DefaultHasher::new()` uses fixed
/// keys, so ids and lookups never depend on a per-process seed).
fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

impl<T: Eq + Hash> Interner<T> {
    pub fn new() -> Self {
        Interner {
            values: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Intern `value`, returning its id. Equal values share an id.
    pub fn intern(&mut self, value: T) -> u32 {
        let h = hash_of(&value);
        let ids = self.index.entry(h).or_default();
        if let Some(&id) = ids.iter().find(|&&id| self.values[id as usize] == value) {
            return id;
        }
        let id = self.values.len() as u32;
        self.values.push(value);
        ids.push(id);
        id
    }

    /// Append `value` WITHOUT deduplication, returning its new id. An equal
    /// earlier value keeps answering [`intern`](Self::intern) (first wins).
    /// For tables whose ids are positional in an external format (the xlsx
    /// `cellXfs` index is a cell's `StyleId`), where two equal entries must
    /// still keep two ids.
    pub fn push(&mut self, value: T) -> u32 {
        let id = self.values.len() as u32;
        self.index.entry(hash_of(&value)).or_default().push(id);
        self.values.push(value);
        id
    }

    /// Replace the value at `id` (no-op when out of range), keeping every
    /// other id. The dedup index is rebuilt (first occurrence wins).
    pub fn replace(&mut self, id: u32, value: T) {
        let Some(slot) = self.values.get_mut(id as usize) else {
            return;
        };
        *slot = value;
        self.index.clear();
        for (i, v) in self.values.iter().enumerate() {
            self.index.entry(hash_of(v)).or_default().push(i as u32);
        }
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

impl<T: Eq + Hash> Default for Interner<T> {
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
    fn push_keeps_duplicates_positional() {
        let mut it: Interner<String> = Interner::new();
        let a = it.push("x".into());
        let b = it.push("x".into());
        assert_eq!((a, b), (0, 1));
        assert_eq!(it.intern("x".into()), 0, "first occurrence answers intern");
        it.replace(0, "y".into());
        assert_eq!(it.get(0).map(String::as_str), Some("y"));
        assert_eq!(it.intern("x".into()), 1, "index rebuilt after replace");
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

    #[test]
    fn colliding_hashes_resolve_by_value() {
        // A type whose hash is constant: every value collides, ids still
        // dedupe by equality.
        #[derive(PartialEq, Eq)]
        struct Same(u8);
        impl Hash for Same {
            fn hash<H: Hasher>(&self, h: &mut H) {
                0u8.hash(h);
            }
        }
        let mut it: Interner<Same> = Interner::new();
        let a = it.intern(Same(1));
        let b = it.intern(Same(2));
        assert_ne!(a, b);
        assert_eq!(it.intern(Same(1)), a);
        assert_eq!(it.intern(Same(2)), b);
        assert_eq!(it.len(), 2);
    }
}
