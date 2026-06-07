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

//! The generated dispatch table (spec §7/§12.2). The single public entry
//! point [`dispatch`] is emitted at build time by `build.rs` from
//! `registry/functions/*.yaml`, with arms in the same `id`-sorted order
//! `sheet-core` uses for [`sheet_core::FuncId`] — so the index that resolves
//! a name in `sheet-core::funcs::lookup_func` is the index that selects the
//! kernel here (**FuncId parity**).
//!
//! **No row, no dispatch — uncallable by construction.** A function with no
//! registry row has no `FuncId`, so it can never reach this table; a row with
//! `status: planned` reaches an arm that returns `#NAME?`; only an
//! `implemented` row is wired to a `crate::families::*` kernel. The dispatch
//! match is the choke point through which every formula function call passes.

include!(concat!(env!("OUT_DIR"), "/dispatch.rs"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arg::Arg;
    use crate::ctx::EvalCtx;
    use sheet_core::{CellError, CellRef, CellValue, DateSystem, FuncId};

    fn ctx() -> EvalCtx {
        let current = CellRef {
            sheet: 0,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        };
        EvalCtx::new(DateSystem::Date1900, current, 0.0, 1)
    }

    #[test]
    fn planned_row_is_name_error() {
        // A `planned` registry row dispatches to #NAME? (uncallable until its
        // family track wires it). Data-driven: at FN-CONV freeze every row was
        // planned; after the family fan-out the loop may be vacuous — the
        // generated-arm behavior stays locked either way.
        let args = [Arg::Scalar(CellValue::Number(1.0))];
        for (i, meta) in sheet_core::funcs::FUNC_META.iter().enumerate() {
            if !meta.implemented {
                assert_eq!(
                    dispatch(FuncId(i as u16), &args, &ctx()),
                    CellValue::Error(CellError::Name),
                    "{} is planned and must dispatch to #NAME?",
                    meta.name
                );
            }
        }
    }

    #[test]
    fn out_of_range_id_is_name_error() {
        // The fallthrough arm: an id past the table (cannot arise from
        // lookup_func, but the function stays total).
        let bogus = FuncId(u16::MAX);
        assert_eq!(
            dispatch(bogus, &[], &ctx()),
            CellValue::Error(CellError::Name)
        );
    }

    #[test]
    fn funcid_parity_with_sheet_core() {
        // sheet-core/build.rs and sheet-fn/build.rs sort the same registry
        // files by the same key, so every name resolves to the index whose
        // arm dispatch generated. Lock that invariant: for each registered
        // function, `lookup_func(name)` must equal its `FUNC_META` index, and
        // dispatching that id must reach a real arm — a planned row yields
        // exactly #NAME?, an implemented zero-arity-violating call yields the
        // arity guard's #VALUE! or a genuine result, NEVER the out-of-range
        // #NAME? fallthrough masked as a planned arm.
        for (i, meta) in sheet_core::funcs::FUNC_META.iter().enumerate() {
            let id = sheet_core::funcs::lookup_func(meta.name)
                .unwrap_or_else(|| panic!("lookup_func({}) returned None", meta.name));
            assert_eq!(
                id.0 as usize, i,
                "FuncId parity broken for {}: lookup={} meta_index={}",
                meta.name, id.0, i
            );
            let out = dispatch(id, &[], &ctx());
            if !meta.implemented {
                assert_eq!(
                    out,
                    CellValue::Error(CellError::Name),
                    "{} is planned and must dispatch to #NAME?",
                    meta.name
                );
            } else if meta.min_args > 0 {
                // Implemented row called with zero args: the generated arity
                // guard must fire (#VALUE!), proving the arm is wired.
                assert_eq!(
                    out,
                    CellValue::Error(CellError::Value),
                    "{} (implemented, min_args {}) must hit the arity guard",
                    meta.name,
                    meta.min_args
                );
            }
            // Implemented zero-arg functions (PI, NOW, RAND, ...) return a
            // real value here — any non-panicking result proves the arm.
        }
    }
}
