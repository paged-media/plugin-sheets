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

//! # sheet-fn — the paged.sheet function library (spec §7)
//!
//! Pure function kernels plus the machinery they share. A kernel is the
//! frozen signature `fn(&[Arg], &EvalCtx) -> CellValue`: it never sees the
//! dependency graph, the scheduler, or the SDK (repo constitution,
//! CLAUDE.md §"Pure kernels"). `sheet-calc` evaluates a formula, builds the
//! `&[Arg]` slice, and routes the call through the registry-generated
//! [`dispatch`] — the one choke point every function call crosses.
//!
//! ## Registry-driven, uncallable by construction
//!
//! The function table is generated at build time from
//! `registry/functions/*.yaml` by `build.rs`, in the same `id`-sorted order
//! `sheet-core` uses for [`sheet_core::FuncId`]. No registry row → no
//! `FuncId` → no dispatch arm. A `planned` row dispatches to `#NAME?`; only
//! an `implemented` row is wired to its `crate::families::*` kernel.
//!
//! ## Module map (Track FN-CONV — the frozen calling convention)
//!
//! - [`arg`] — [`Arg`] / [`RangeView`]: the argument convention. **FROZEN.**
//! - [`ctx`] — [`EvalCtx`]: date system, current cell, injected clock,
//!   deterministic RNG. **FROZEN.**
//! - [`coerce`] — number/text/bool coercion, error propagation, the
//!   cross-type comparison ruling (the cross-engine hot zone, §7).
//! - [`criteria`] — `SUMIF`/`COUNTIF` criterion parsing + wildcard matching.
//! - [`num`] — the [`Numeric`] seam (D-6: `f64` now, exact-decimal later).
//! - [`families`] — the per-family kernels (owned by the family tracks; seeded
//!   empty here so the workspace builds while every row is `planned`).
//! - [`dispatch`] — the generated dispatch match.

pub mod arg;
pub mod coerce;
pub mod criteria;
pub mod ctx;
pub mod dispatch;
pub mod families;
pub mod num;

// ---- Crate-root re-exports of the frozen calling convention (spec §7). ----

pub use arg::{Arg, RangeView};
pub use ctx::EvalCtx;
pub use num::{Numeric, F64};

pub use dispatch::dispatch;
