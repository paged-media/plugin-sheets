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

//! Function family modules (spec §7). Each module mirrors Excel's
//! documentation taxonomy and holds the pure
//! `fn(&[Arg], &EvalCtx) -> CellValue` kernels named by the registry
//! `rust` field (e.g. `math::sum` ⇐ `rust: math::sum`). The generated
//! [`crate::dispatch`] table calls into these by path; a row whose status
//! is still `planned` is never wired, so an empty module compiles cleanly
//! while its rows await a family track. **Family agents own these files**;
//! Track FN-CONV only seeds them empty so the workspace builds.
pub mod agg;
pub mod date;
pub mod info;
pub mod logical;
pub mod lookup;
pub mod math;
pub mod text;

// ---- M1 family tracks (spec §13 M1). Seeded EMPTY here so the workspace
// builds while every M1 row is `planned`; each module is populated by its
// named M1 track (the row's `rust:` symbol lands then). ----
pub mod array;
pub mod date2;
pub mod fin;
pub mod info2;
pub mod logical2;
pub mod lookup2;
pub mod math2;
pub mod stat;
pub mod text2;
