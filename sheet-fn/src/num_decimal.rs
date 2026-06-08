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

//! The D-6 exact-decimal SPIKE backend (spec §3, M3) — gated behind the
//! `exact-decimal` cargo feature, OFF by default.
//!
//! This is a STUB. Phase A only carves the seam: an `exact-decimal` feature
//! flag plus this module, so the workspace builds with AND without the
//! feature and the Phase B spike track has a place to land a real `Decimal`
//! [`crate::num::Numeric`] impl.
//!
//! TODO (Phase B / spike track, registry `sheet.calc.decimal.*`):
//! - Choose a permissive decimal carrier — `rust_decimal` (MIT) or a
//!   hand-rolled fixed-decimal — and add it as an OPTIONAL dependency keyed
//!   on this same feature (`rust_decimal = { version = "…", optional = true }`
//!   + `exact-decimal = ["dep:rust_decimal"]`).
//! - Implement `Decimal(pub <carrier>)` with `impl Numeric for Decimal`
//!   (the trait's `from_f64`/`to_f64`/`add`/`sub`/`mul`/`div`/`pow`).
//! - Build the divergence corpus (`sheet.calc.decimal.divergence-corpus`):
//!   cases where f64 and decimal differ (the canonical `0.1 + 0.2`, etc.).
//! - Write the adopt/defer recommendation (`sheet.calc.decimal.spike-report`).
//!
//! Exploratory: f64 ([`crate::num::F64`]) stays the v1 default regardless of
//! the spike outcome — this seam never becomes the default without an explicit
//! ruling.
