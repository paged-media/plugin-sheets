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

//! # sheet-core — the frozen paged.sheet type contract
//!
//! The leaf crate of the paged.sheet engine (spec §5): the stored
//! [`CellValue`], cell/range [`refs`], the canonical dialect-neutral [`ast`],
//! the [`SheetModel`] document model, interners, and the registry-generated
//! function table ([`funcs`]). It depends on nothing in the workspace —
//! every other `sheet-*` crate depends on it.
//!
//! ## FROZEN at M0 phase 0 (interface freeze)
//!
//! These types, the AST, and the registry YAML schema are **frozen**.
//! Changes go through the orchestrator as **versioned amendments**, never
//! drive-by edits (repo constitution, CLAUDE.md §"Interface freeze"). The
//! function table is **registry-driven**: it is generated at build time
//! from `registry/functions/*.yaml` (`build.rs`), so an unregistered
//! function has no [`ast::FuncId`] and is uncallable by construction.

pub mod ast;
pub mod calc_settings;
pub mod cell;
pub mod funcs;
pub mod intern;
pub mod model;
pub mod names;
pub mod preserved;
pub mod refs;
pub mod style;
pub mod value;

// ---- Crate-root re-exports of the key public types (spec §5). ----

pub use value::{CellError, CellValue};

pub use refs::{
    a1_to_col, col_to_a1, format_a1, parse_a1, CellRef, RangeRef, SheetId, MAX_COL, MAX_ROW,
};

pub use cell::{Cell, FormulaId, StyleId};

pub use ast::{BinOp, Expr, Formula, FuncId, LitValue, NameId, OrderedF64, UnOp};

pub use names::{NameDef, NameScope, NameTable, NameTarget};

pub use style::{Align, CellStyle, NumFmtId, StyleTable};

pub use calc_settings::{CalcSettings, DateSystem};

pub use intern::Interner;

pub use preserved::PreservedParts;

pub use model::{SheetModel, UsedRange, Worksheet};
