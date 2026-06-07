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

//! The generated function registry (spec §7/§12.2). The table here is
//! emitted at build time by `build.rs` from `registry/functions/*.yaml`,
//! sorted by `id` so each [`crate::ast::FuncId`] is the stable sorted
//! index. An unregistered function has no row, hence no `FuncId`, hence no
//! dispatch entry downstream — **an unregistered function is uncallable by
//! construction**. The only way to add a callable is to add a registry row.

include!(concat!(env!("OUT_DIR"), "/funcs.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_insensitive() {
        let upper = lookup_func("SUM");
        let lower = lookup_func("sum");
        assert!(upper.is_some());
        assert_eq!(upper, lower);
    }

    #[test]
    fn sum_meta_shape() {
        let id = lookup_func("SUM").unwrap();
        let m = meta(id);
        assert_eq!(m.min_args, 1);
        assert_eq!(m.max_args, None); // variadic
        assert!(m.range_aware);
    }

    #[test]
    fn unregistered_is_none() {
        assert_eq!(lookup_func("NOPE"), None);
        assert_eq!(lookup_func("definitely_not_a_function"), None);
    }

    #[test]
    fn registry_is_nonempty_and_sorted() {
        assert!(
            FUNC_META.len() >= 80,
            "expected >=80 registered functions, got {}",
            FUNC_META.len()
        );
        for w in FUNC_META.windows(2) {
            assert!(
                w[0].id < w[1].id,
                "FUNC_META not sorted by id: {:?} !< {:?}",
                w[0].id,
                w[1].id
            );
        }
    }

    #[test]
    fn volatility_flag_from_registry() {
        // NOW is a volatile function.
        let id = lookup_func("NOW").unwrap();
        assert!(meta(id).volatile);
        // SUM is not volatile.
        assert!(!meta(lookup_func("SUM").unwrap()).volatile);
    }

    #[test]
    fn ref_args_flag_from_registry() {
        // ROW takes reference args.
        let id = lookup_func("ROW").unwrap();
        assert!(meta(id).ref_args);
        // SUM does not.
        assert!(!meta(lookup_func("SUM").unwrap()).ref_args);
    }

    #[test]
    fn lookup_then_meta_roundtrips_name() {
        let id = lookup_func("VLOOKUP").unwrap();
        assert_eq!(meta(id).name, "VLOOKUP");
    }
}
