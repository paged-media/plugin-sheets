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

//! Iterative (circular) calculation (spec §6.2, decision D-7; milestone M2).
//!
//! ## The two policies for a dependency cycle
//!
//! The scheduler ([`crate::topo`]) returns the cells it could not topologically
//! order — the members of a cycle. There are two ways to resolve them, selected
//! by [`sheet_core::CalcSettings::iterative`]:
//!
//! 1. **OFF (default, the `sheet.calc.circular` ruling).** Each cycle member's
//!    stored value becomes [`sheet_core::CellValue::Error`]`(`[`sheet_core::CellError::Ref`]`)`
//!    and the whole cycle is reported on [`crate::RecalcResult::circular`].
//!    Behavior is UNCHANGED from M1.
//!
//! 2. **ON (this module, `sheet.calc.iterative.*`).** The cycle members are
//!    evaluated *iteratively* to a fixed point, Excel-style:
//!    - seed every cycle cell at `0` (Excel's documented initial value);
//!    - repeatedly recompute the members in a **stable order** (sorted
//!      [`CellRef`] — so results are reproducible run-to-run, the §6.2
//!      determinism property), each write immediately visible to later cells in
//!      the same pass (Gauss–Seidel style — matches Excel's in-pass update);
//!    - stop after `max_iter` passes, OR early when the largest absolute change
//!      of any cycle cell between two consecutive passes is `<= max_change`.
//!
//!    On convergence [`crate::RecalcResult::circular`] is **empty** (the cycle
//!    settled); a system that hits `max_iter` without settling is reported on
//!    [`crate::RecalcResult::non_converged`] (and the last-iterate values are
//!    kept — Excel likewise leaves the partial result in place).
//!
//! ## ECMA-376 provenance
//!
//! `iterate` / `iterateCount` / `iterateDelta` (`<calcPr>`, ECMA-376 §18.2.2)
//! are the workbook knobs mirrored by [`sheet_core::CalcSettings`]
//! (`iterative` / `max_iter` / `max_change`). Excel's out-of-box defaults are
//! 100 iterations and a 0.001 delta.
//!
//! ## Delta metric
//!
//! Convergence is measured on the numeric magnitude of each cycle cell. A pass's
//! per-cell change is `|new - old|` when BOTH the old and new values are numbers;
//! a value that flips to/from a non-number (text, bool, error, blank) counts as
//! an "infinite" change for that pass (it cannot have settled), so a system that
//! oscillates between non-numeric states runs to `max_iter` rather than falsely
//! reporting convergence.

use sheet_core::CellValue;

/// The numeric magnitude used for the convergence delta, if the value is a
/// plain number. Non-numbers (text/bool/error/blank) have no magnitude.
fn numeric(v: &CellValue) -> Option<f64> {
    match v {
        CellValue::Number(n) => Some(*n),
        _ => None,
    }
}

/// The per-cell absolute change between two passes (the `<= max_change` test
/// operand). Both numeric → `|old - new|`; a non-numeric on either side →
/// [`f64::INFINITY`] unless the two values are exactly equal (a stable
/// non-number has settled at delta `0`).
pub(crate) fn cell_delta(old: &CellValue, new: &CellValue) -> f64 {
    match (numeric(old), numeric(new)) {
        (Some(a), Some(b)) => (a - b).abs(),
        _ if old == new => 0.0,
        _ => f64::INFINITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::CellError;

    #[test]
    fn cell_delta_numeric() {
        assert_eq!(
            cell_delta(&CellValue::Number(1.0), &CellValue::Number(1.5)),
            0.5
        );
        assert_eq!(
            cell_delta(&CellValue::Number(2.0), &CellValue::Number(2.0)),
            0.0
        );
    }

    #[test]
    fn cell_delta_nonnumeric_is_infinite_unless_equal() {
        assert!(cell_delta(&CellValue::Number(1.0), &CellValue::from("x")).is_infinite());
        assert!(
            cell_delta(&CellValue::Error(CellError::Div0), &CellValue::Number(0.0)).is_infinite()
        );
        // Identical non-numerics are stable (delta 0).
        assert_eq!(
            cell_delta(&CellValue::from("x"), &CellValue::from("x")),
            0.0
        );
    }
}
