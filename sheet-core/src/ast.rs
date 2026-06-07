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

//! The canonical, dialect-neutral formula AST (spec §5.1/§6). This is pure
//! data: parser logic lives in `sheet-parser`, evaluation in `sheet-calc`.
//! Functions are referenced by [`FuncId`] (the registry-generated index),
//! so an unregistered function is unrepresentable here by construction.

use crate::refs::{CellRef, RangeRef};
use compact_str::CompactString;

use crate::value::CellError;

/// Registry index of a function (see `funcs` module). Stable per build:
/// the sorted position of a row in `registry/functions/*.yaml`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FuncId(pub u16);

/// Index into a [`crate::names::NameTable`] entry (a defined name).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NameId(pub u32);

/// A parsed formula. Wraps the root expression so it can be interned
/// (`Eq + Hash`) in `SheetModel::formulas`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Formula {
    pub root: Expr,
}

/// A formula expression node. `Array` is parse-only in T0 (constructed but
/// not evaluated). The `StructuredRef`/`SpillRef` variants are reserved for
/// T1 and are intentionally NOT present yet — adding them is a versioned
/// amendment, not a drive-by edit (the AST is frozen at M0 phase 0).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Expr {
    Lit(LitValue),
    Ref(CellRef),
    Range(RangeRef),
    Name(NameId),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Func(FuncId, Vec<Expr>),
    /// Array literal `{1,2;3,4}` — outer Vec = rows, inner = columns.
    /// Parse-only in T0.
    Array(Vec<Vec<Expr>>),
    // T1 slots (documented, not constructed in T0): StructuredRef, SpillRef.
}

/// A literal embedded in a formula. Numbers use [`OrderedF64`] so the AST
/// stays `Eq + Hash` (and therefore internable).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LitValue {
    Number(OrderedF64),
    Text(CompactString),
    Bool(bool),
    Error(CellError),
}

/// Binary operators (ECMA-376 §18.17.2). `Range`/`Union`/`Isect` are the
/// reference operators `:`, `,`, and ` ` (space).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Range,
    Union,
    Isect,
}

/// Unary operators. `Plus` is the (semantically inert) unary `+`;
/// `Percent` is the postfix `%`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Plus,
    Percent,
}

/// A total-ordering / hashable wrapper over `f64`, comparing by raw bits
/// (`to_bits`). The AST must be `Eq + Hash` so `Formula` can be interned;
/// plain `f64` is neither (NaN != NaN, and floats have no `Hash`). Bit
/// equality is the right key for dedup: two formulas are "the same" only
/// if their literals are bit-identical. This is NOT for arithmetic — it is
/// an interning key only.
#[derive(Copy, Clone, Debug)]
pub struct OrderedF64(f64);

impl OrderedF64 {
    pub fn new(v: f64) -> Self {
        OrderedF64(v)
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for OrderedF64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for OrderedF64 {}

impl std::hash::Hash for OrderedF64 {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orderedf64_bit_equality() {
        assert_eq!(OrderedF64::new(1.5), OrderedF64::new(1.5));
        assert_ne!(OrderedF64::new(1.5), OrderedF64::new(1.6));
        // NaN bits equal themselves (unlike raw f64).
        let nan = OrderedF64::new(f64::NAN);
        assert_eq!(nan, nan);
        // +0.0 and -0.0 differ by bit pattern.
        assert_ne!(OrderedF64::new(0.0), OrderedF64::new(-0.0));
        assert_eq!(OrderedF64::new(2.0).get(), 2.0);
    }

    #[test]
    fn orderedf64_hash_consistent_with_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(OrderedF64::new(42.5));
        assert!(set.contains(&OrderedF64::new(42.5)));
        assert!(!set.contains(&OrderedF64::new(42.6)));
    }
}
