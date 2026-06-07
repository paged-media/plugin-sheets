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

//! # sheet-parser — the Excel-dialect formula front end (spec §6.1)
//!
//! Lexer + Pratt parser producing the dialect-neutral [`sheet_core::ast`],
//! a canonical printer (`parse ∘ print` is an AST fixpoint), reference
//! extraction for the dependency graph (§6.2), and structural rewrite for
//! row/column insert/delete (§6.3). Phase 2's `sheet-calc` and `sheet-js`
//! build against the FROZEN public API below.
//!
//! ## Rulings (documented Excel-compat decisions, constitution §3)
//!
//! - **Unknown function name → [`ParseError`]** (not a `#NAME?` literal).
//!   An unregistered function has no [`sheet_core::ast::FuncId`], so it is
//!   unrepresentable in the AST; a `#NAME?` literal would also lose the
//!   round-trip. The xlsx layer maps a parse failure to a cell that keeps
//!   raw text + cached value (constitution §10.2 preservation invariant).
//! - **Unary minus binds tighter than `^`** (`-2^2 == 4`); **`^` is
//!   left-associative** (`2^3^2 == 64`) — see [`pratt`].
//! - **`,` is the union operator only inside `(...)`**; at function-argument
//!   depth it separates arguments. **`;` is only an array-row separator**
//!   (T0 en-US dialect).
//! - **`print` takes a `home: SheetId`** (an amendment to the sketch
//!   signature): a ref prints a sheet prefix iff its sheet differs from
//!   `home`. Without a home sheet the prefix decision is undefined.

mod error;
mod extract;
mod lexer;
mod pratt;
mod print;
mod refs;
mod rewrite;

pub use error::ParseError;
pub use extract::{extract_refs, RefSet};
pub use print::print;
pub use rewrite::{rewrite, Edit};

use sheet_core::ast::{Formula, NameId};
use sheet_core::SheetId;

/// Resolution context the parser needs: sheet-name and defined-name lookup,
/// plus the formula's home sheet (for unqualified refs). Supplied by the
/// caller (the workbook model in `sheet-calc`/`sheet-js`).
pub trait ParseCtx {
    /// Resolve a sheet name to its id, or `None` if it does not exist.
    fn sheet_id(&self, name: &str) -> Option<SheetId>;
    /// Resolve a defined-name spelling to its id, or `None`.
    fn name_id(&self, name: &str) -> Option<NameId>;
    /// The sheet the formula being parsed lives on (for unqualified refs).
    fn current_sheet(&self) -> SheetId;
}

/// Reverse map for printing: a [`SheetId`] back to its display name.
pub trait SheetNames {
    /// The sheet's display name, or `None` for an unknown id (prints
    /// `#REF!`).
    fn sheet_name(&self, id: SheetId) -> Option<&str>;
}

/// Parse a formula `input` (WITHOUT the leading `=`) into a [`Formula`].
///
/// Errors carry a message + byte span into `input`. See the crate-level
/// rulings for the unknown-function and unknown-name behavior.
pub fn parse(input: &str, ctx: &dyn ParseCtx) -> Result<Formula, ParseError> {
    let tokens = lexer::lex(input)?;
    if tokens.is_empty() {
        return Err(ParseError::new("empty formula", 0..input.len()));
    }
    let root = pratt::parse_tokens(&tokens, ctx, input.len())?;
    Ok(Formula { root })
}

#[cfg(test)]
mod tests;
