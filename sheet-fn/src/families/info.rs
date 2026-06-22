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

//! The information family (spec §7, §11 T0) — the `IS*` type/error
//! predicates plus `NA()`. Each kernel is the frozen pure signature
//! `fn(&[Arg], &EvalCtx) -> CellValue`; the generated [`crate::dispatch`]
//! arity-checks (1 arg each, `NA` 0 args) before calling, so a kernel only
//! ever sees a well-formed argument list.
//!
//! # The defining ruling: info functions DO NOT propagate argument errors
//!
//! `ISERROR(#DIV/0!)` is `TRUE`, not `#DIV/0!`. These predicates *inspect*
//! their argument — an error value is the subject under test, never a
//! short-circuit — so they deliberately bypass the shared
//! [`crate::coerce::first_error`] pre-pass that math/text/logical kernels
//! run (ECMA-376 §18.17.7). They likewise apply **no coercion**:
//! `ISNUMBER`/`ISTEXT`/`ISLOGICAL`/`ISBLANK` are strict checks on the
//! stored [`CellValue`] variant (`ISNUMBER("1")` is `FALSE`).
//!
//! A range argument is reduced to its top-left cell here (implicit
//! intersection is the caller's job for scalar-context functions, but a
//! `range_aware: false` info kernel that nonetheless receives a [`Arg::Range`]
//! reads its origin cell so the contract stays total — Excel evaluates
//! `=ISNUMBER(A1:A3)` in a scalar cell against the intersected single cell).

use sheet_core::{CellError, CellValue};

use crate::arg::Arg;
use crate::ctx::EvalCtx;

/// Resolve a single info-predicate argument to the [`CellValue`] under test.
/// A scalar is taken as-is; a range collapses to its relative-`(0,0)` cell
/// (the implicit-intersection fallback — out-of-bounds yields `Empty`).
fn subject(arg: &Arg) -> CellValue {
    match arg {
        Arg::Scalar(v) => v.clone(),
        Arg::Range(view) => view.get(0, 0),
    }
}

/// `ISBLANK(value)` — `TRUE` only for a genuinely blank cell
/// ([`CellValue::Empty`]). `Text("")` is **not** blank (ECMA-376 §18.17.7);
/// the distinction is the whole point of the predicate.
pub fn isblank(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(matches!(subject(&args[0]), CellValue::Empty))
}

/// `ISNUMBER(value)` — `TRUE` iff the value is a stored number. Strict: no
/// coercion, so numeric text (`"1"`) is `FALSE` (ECMA-376 §18.17.7).
pub fn isnumber(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(matches!(subject(&args[0]), CellValue::Number(_)))
}

/// `ISTEXT(value)` — `TRUE` iff the value is stored text. Strict: a number
/// is `FALSE`, and a blank cell is `FALSE` (`Empty` is not `Text("")`).
pub fn istext(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(matches!(subject(&args[0]), CellValue::Text(_)))
}

/// `ISLOGICAL(value)` — `TRUE` iff the value is a stored boolean. Strict:
/// `1`/`0` and `"TRUE"`/`"FALSE"` text are `FALSE` (no coercion).
pub fn islogical(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(matches!(subject(&args[0]), CellValue::Bool(_)))
}

/// `ISERROR(value)` — `TRUE` for **any** of the eight error codes, `#N/A`
/// included. The argument error is the subject, not a short-circuit
/// (ECMA-376 §18.17.7); this never returns the error itself.
pub fn iserror(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(matches!(subject(&args[0]), CellValue::Error(_)))
}

/// `ISERR(value)` — `TRUE` for any error **except** `#N/A`. The narrow
/// sibling of `ISERROR`, used to let `#N/A` (a "not-found" sentinel) flow
/// through error guards (ECMA-376 §18.17.7).
pub fn iserr(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(matches!(
        subject(&args[0]),
        CellValue::Error(e) if e != CellError::Na
    ))
}

/// `ISNA(value)` — `TRUE` only for `#N/A`; every other error (and every
/// non-error) is `FALSE` (ECMA-376 §18.17.7).
pub fn isna(args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Bool(matches!(subject(&args[0]), CellValue::Error(CellError::Na)))
}

/// `NA()` — returns the `#N/A` error value (ECMA-376 §18.17.7). The only
/// info function that *produces* rather than *inspects* an error. Arity is
/// fixed at zero by the registry, enforced by the dispatch guard.
pub fn na(_args: &[Arg], _ctx: &EvalCtx) -> CellValue {
    CellValue::Error(CellError::Na)
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_str::CompactString;
    use sheet_core::{CellRef, DateSystem};

    fn cr() -> CellRef {
        CellRef {
            sheet: 0,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        }
    }

    fn ctx() -> EvalCtx {
        EvalCtx::new(DateSystem::Date1900, cr(), 45000.5, 42)
    }

    fn t(b: bool) -> CellValue {
        CellValue::Bool(b)
    }

    #[test]
    fn isblank_only_empty() {
        assert_eq!(isblank(&[Arg::Scalar(CellValue::Empty)], &ctx()), t(true));
        assert_eq!(
            isblank(&[Arg::Scalar(CellValue::from(""))], &ctx()),
            t(false)
        );
        assert_eq!(
            isblank(&[Arg::Scalar(CellValue::Number(0.0))], &ctx()),
            t(false)
        );
    }

    #[test]
    fn strict_type_checks_no_coercion() {
        // "1" is text, not a number; ISNUMBER must NOT coerce.
        assert_eq!(
            isnumber(&[Arg::Scalar(CellValue::from("1"))], &ctx()),
            t(false)
        );
        assert_eq!(
            isnumber(&[Arg::Scalar(CellValue::Number(1.0))], &ctx()),
            t(true)
        );
        // 1 is a number, not logical; ISLOGICAL must NOT coerce.
        assert_eq!(
            islogical(&[Arg::Scalar(CellValue::Number(1.0))], &ctx()),
            t(false)
        );
        assert_eq!(
            islogical(&[Arg::Scalar(CellValue::Bool(true))], &ctx()),
            t(true)
        );
        // "TRUE" is text, not logical.
        assert_eq!(
            islogical(&[Arg::Scalar(CellValue::from("TRUE"))], &ctx()),
            t(false)
        );
        assert_eq!(
            istext(&[Arg::Scalar(CellValue::from("x"))], &ctx()),
            t(true)
        );
        assert_eq!(
            istext(&[Arg::Scalar(CellValue::Number(1.0))], &ctx()),
            t(false)
        );
    }

    #[test]
    fn error_predicates_do_not_propagate() {
        // The defining ruling: an error ARGUMENT is the subject, never
        // returned. ISERROR(#DIV/0!) is TRUE, not #DIV/0!.
        let div0 = Arg::Scalar(CellValue::Error(CellError::Div0));
        assert_eq!(iserror(std::slice::from_ref(&div0), &ctx()), t(true));
        assert_eq!(iserr(std::slice::from_ref(&div0), &ctx()), t(true));
        assert_eq!(isna(std::slice::from_ref(&div0), &ctx()), t(false));

        let na_arg = Arg::Scalar(CellValue::Error(CellError::Na));
        // ISERROR includes #N/A, ISERR excludes it, ISNA matches only it.
        assert_eq!(iserror(std::slice::from_ref(&na_arg), &ctx()), t(true));
        assert_eq!(iserr(std::slice::from_ref(&na_arg), &ctx()), t(false));
        assert_eq!(isna(std::slice::from_ref(&na_arg), &ctx()), t(true));

        // Non-error subjects are FALSE everywhere.
        let num = Arg::Scalar(CellValue::Number(5.0));
        assert_eq!(iserror(std::slice::from_ref(&num), &ctx()), t(false));
        assert_eq!(iserr(std::slice::from_ref(&num), &ctx()), t(false));
        assert_eq!(isna(std::slice::from_ref(&num), &ctx()), t(false));
    }

    #[test]
    fn na_returns_na_error() {
        assert_eq!(na(&[], &ctx()), CellValue::Error(CellError::Na));
    }

    #[test]
    fn range_arg_uses_top_left() {
        use crate::arg::RangeView;
        let cells = [
            CellValue::Text(CompactString::new("a")),
            CellValue::Number(2.0),
        ];
        let view = RangeView::from_slice(cr(), 1, 2, &cells);
        // Top-left is text → ISTEXT TRUE, ISNUMBER FALSE.
        assert_eq!(istext(&[Arg::Range(view)], &ctx()), t(true));
        let view2 = RangeView::from_slice(cr(), 1, 2, &cells);
        assert_eq!(isnumber(&[Arg::Range(view2)], &ctx()), t(false));
    }
}
