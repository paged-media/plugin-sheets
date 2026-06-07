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

//! Defined names (spec §5.1). A name may be workbook- or sheet-scoped;
//! resolution lets a sheet-local name shadow a workbook name of the same
//! spelling. Formula-targeted names hold raw text resolved lazily in T1.

use crate::ast::NameId;
use crate::refs::{RangeRef, SheetId};
use compact_str::CompactString;

/// A single defined name.
#[derive(Clone, Debug)]
pub struct NameDef {
    pub name: CompactString,
    pub scope: NameScope,
    pub target: NameTarget,
}

/// Where a name is visible.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NameScope {
    Workbook,
    Sheet(SheetId),
}

/// What a name points at. `Formula` keeps the raw text — resolution into an
/// AST is deferred to T1 (the parser is not a dependency of this leaf crate).
#[derive(Clone, Debug)]
pub enum NameTarget {
    Range(RangeRef),
    Formula(CompactString),
}

/// The workbook's defined-name table. Append-only; ids are positional.
#[derive(Default, Debug)]
pub struct NameTable {
    defs: Vec<NameDef>,
}

impl NameTable {
    /// Append a definition, returning its id.
    pub fn define(&mut self, def: NameDef) -> NameId {
        let id = NameId(self.defs.len() as u32);
        self.defs.push(def);
        id
    }

    pub fn get(&self, id: NameId) -> Option<&NameDef> {
        self.defs.get(id.0 as usize)
    }

    /// Resolve `name` as seen from `sheet`. A `Sheet(sheet)`-scoped def
    /// shadows a `Workbook` def of the same (case-insensitive) name. Later
    /// definitions win within a scope.
    pub fn resolve(&self, name: &str, sheet: SheetId) -> Option<NameId> {
        let mut workbook: Option<NameId> = None;
        let mut sheet_local: Option<NameId> = None;
        for (i, def) in self.defs.iter().enumerate() {
            if !def.name.eq_ignore_ascii_case(name) {
                continue;
            }
            match def.scope {
                NameScope::Sheet(s) if s == sheet => sheet_local = Some(NameId(i as u32)),
                NameScope::Workbook => workbook = Some(NameId(i as u32)),
                NameScope::Sheet(_) => {}
            }
        }
        sheet_local.or(workbook)
    }

    /// Iterate `(id, def)` in definition order.
    pub fn iter(&self) -> impl Iterator<Item = (NameId, &NameDef)> {
        self.defs
            .iter()
            .enumerate()
            .map(|(i, d)| (NameId(i as u32), d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::CellRef;

    fn range() -> RangeRef {
        let c = CellRef {
            sheet: 0,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        };
        RangeRef { start: c, end: c }
    }

    #[test]
    fn sheet_scope_shadows_workbook() {
        let mut t = NameTable::default();
        let wb = t.define(NameDef {
            name: "Rate".into(),
            scope: NameScope::Workbook,
            target: NameTarget::Range(range()),
        });
        let sh = t.define(NameDef {
            name: "Rate".into(),
            scope: NameScope::Sheet(1),
            target: NameTarget::Range(range()),
        });
        // From sheet 1, the sheet-local def wins.
        assert_eq!(t.resolve("Rate", 1), Some(sh));
        // From any other sheet, only the workbook def is visible.
        assert_eq!(t.resolve("Rate", 0), Some(wb));
    }

    #[test]
    fn resolve_case_insensitive() {
        let mut t = NameTable::default();
        let id = t.define(NameDef {
            name: "TaxRate".into(),
            scope: NameScope::Workbook,
            target: NameTarget::Formula("0.2".into()),
        });
        assert_eq!(t.resolve("taxrate", 0), Some(id));
        assert_eq!(t.resolve("TAXRATE", 5), Some(id));
        assert_eq!(t.resolve("missing", 0), None);
        assert!(t.get(id).is_some());
        assert_eq!(t.iter().count(), 1);
    }
}
