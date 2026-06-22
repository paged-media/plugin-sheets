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

//! Dispatch codegen (spec §7/§12.2). Reads every
//! `../registry/functions/*.yaml` and emits `$OUT_DIR/dispatch.rs`: one
//! match arm per function row, keyed by the same `id`-sorted [`FuncId`]
//! index that `sheet-core/build.rs` assigns (the two builders MUST mirror
//! each other's sort so the indices agree — **FuncId parity**). For an
//! `implemented` row the arm range-checks the arity, then calls
//! `crate::families::<rust>`; a `planned` row returns `#NAME?` (unimplemented
//! but registered); an arity violation returns `#VALUE!`. No registry row →
//! no arm → uncallable by construction.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Arity {
    min: u8,
    #[serde(default)]
    max: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct FnRow {
    id: String,
    name: String,
    arity: Arity,
    /// The kernel symbol under `sheet_fn::families::` (e.g. `math::sum`).
    /// Only consulted for `implemented` rows.
    #[serde(default)]
    rust: String,
    /// True for dynamic-array kernels (M1 array track). An `implemented`
    /// array row's kernel is `fn(&[Arg], &EvalCtx) -> FnResult`; a scalar
    /// row's kernel is `fn(&[Arg], &EvalCtx) -> CellValue`.
    #[serde(default)]
    returns_array: bool,
    /// True for EVALUATOR SPECIAL FORMS (M2 Phase A): OFFSET/INDIRECT/
    /// FORMULATEXT/ISFORMULA read the MODEL and are handled in
    /// `sheet-calc/eval.rs` BEFORE dispatch. The pure dispatch door here never
    /// calls a kernel for such a row — it returns `#NAME?` (documented: eval
    /// intercepts first; reaching the pure door is an internal invariant break).
    #[serde(default)]
    special_form: bool,
    #[serde(default)]
    status: String,
    // Unknown fields (family, volatility, provenance, tests, …) ignored.
}

fn main() {
    // Re-run when any registry function YAML changes (same trigger set as
    // sheet-core/build.rs so the two regenerate in lockstep).
    println!("cargo:rerun-if-changed=../registry/functions");
    println!("cargo:rerun-if-changed=build.rs");

    let dir = Path::new("../registry/functions");
    let mut rows: Vec<FnRow> = Vec::new();

    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("sheet-fn build.rs: cannot read {dir:?}: {e}"))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().map(|x| x == "yaml").unwrap_or(false))
        .collect();
    entries.sort();

    for path in &entries {
        println!("cargo:rerun-if-changed={}", path.display());
        let text = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("sheet-fn build.rs: read {path:?}: {e}"));
        let parsed: Vec<FnRow> = serde_yaml::from_str(&text)
            .unwrap_or_else(|e| panic!("sheet-fn build.rs: parse {path:?}: {e}"));
        rows.extend(parsed);
    }

    // ---- Validation: an implemented row MUST name a kernel symbol. ----
    let mut seen_id: BTreeMap<&str, ()> = BTreeMap::new();
    for r in &rows {
        if seen_id.insert(r.id.as_str(), ()).is_some() {
            panic!("sheet-fn build.rs: duplicate function id {:?}", r.id);
        }
        let implemented = r.status.eq_ignore_ascii_case("implemented");
        // A special-form row has NO pure kernel (it is handled in eval.rs), so
        // it is exempt from the `rust` symbol requirement.
        if implemented && !r.special_form && r.rust.trim().is_empty() {
            panic!(
                "sheet-fn build.rs: function {} is implemented but has no `rust` symbol",
                r.id
            );
        }
        if let Some(max) = r.arity.max {
            if r.arity.min > max {
                panic!(
                    "sheet-fn build.rs: function {} has arity min {} > max {}",
                    r.id, r.arity.min, max
                );
            }
        }
    }

    // ---- Sort by id → FuncId == sorted index (mirrors sheet-core). ----
    rows.sort_by(|a, b| a.id.cmp(&b.id));

    // ---- Emit the dispatch function. ----
    let mut out = String::new();
    out.push_str(
        "// @generated by sheet-fn/build.rs from ../registry/functions/*.yaml — DO NOT EDIT.\n\n",
    );

    out.push_str(
        "/// Registry-driven dispatch (spec §7). `id` is a sheet-core\n\
         /// [`sheet_core::FuncId`]; the match arm is generated from the same\n\
         /// `id`-sorted registry, so the index here is the index there\n\
         /// (FuncId parity). An `implemented` row arity-checks then calls its\n\
         /// kernel; a `planned` row returns `#NAME?`; an arity violation\n\
         /// returns `#VALUE!`. There is no fallthrough table — an out-of-range\n\
         /// id is an internal invariant break and returns `#NAME?`.\n\
         // Generated guards are uniform (`len < min`/`len > max` straight from\n\
         // the YAML), so the zero/one/u8::MAX special cases clippy wants are\n\
         // noise here — blanket-allow on the generated fn only.\n\
         #[allow(clippy::len_zero, clippy::absurd_extreme_comparisons, unused_comparisons)]\n\
         pub fn dispatch(\n\
         \x20   id: sheet_core::FuncId,\n\
         \x20   args: &[crate::arg::Arg],\n\
         \x20   ctx: &crate::ctx::EvalCtx,\n\
         ) -> sheet_core::CellValue {\n\
         \x20   use sheet_core::value::{CellError, CellValue};\n\
         \x20   // `args`/`ctx` are bound for kernels; both are unused only in\n\
         \x20   // the degenerate all-`planned` build, hence the discards.\n\
         \x20   let _ = (args, ctx);\n\
         \x20   match id.0 {\n",
    );

    for (i, r) in rows.iter().enumerate() {
        let implemented = r.status.eq_ignore_ascii_case("implemented");
        if implemented && r.special_form {
            // A special-form row (OFFSET/INDIRECT/FORMULATEXT/ISFORMULA) is
            // handled in `sheet-calc/eval.rs` BEFORE dispatch — it reads the
            // model and has no pure kernel. Reaching the pure door is an
            // internal invariant break (eval must intercept first); return
            // #NAME? so the door stays total. (M2 Phase A special-form track.)
            writeln!(
                out,
                "        {i} => CellValue::Error(CellError::Name), // {name} (special_form — handled in eval.rs)",
                i = i,
                name = r.name,
            )
            .unwrap();
        } else if implemented && r.returns_array {
            // A dynamic-array row is uncallable through the SCALAR door: it
            // has no `-> CellValue` kernel. The scalar door stays total and
            // returns #VALUE! — the evaluator must route array rows through
            // `dispatch_rich`. (M1 array track; documented in dispatch.rs.)
            writeln!(
                out,
                "        {i} => CellValue::Error(CellError::Value), // {name} (returns_array — use dispatch_rich)",
                i = i,
                name = r.name,
            )
            .unwrap();
        } else if implemented {
            // `rust: math::sum` → `crate::families::math::sum`.
            let path = format!("crate::families::{}", r.rust.trim());
            // Arity guard: len < min || (max.is_some() && len > max).
            let max_guard = match r.arity.max {
                Some(m) => format!(" || args.len() > {m}"),
                None => String::new(),
            };
            writeln!(
                out,
                "        {i} => {{ // {name}\n\
                 \x20           if args.len() < {min}{max_guard} {{\n\
                 \x20               return CellValue::Error(CellError::Value);\n\
                 \x20           }}\n\
                 \x20           {path}(args, ctx)\n\
                 \x20       }}",
                i = i,
                name = r.name,
                min = r.arity.min,
                max_guard = max_guard,
                path = path,
            )
            .unwrap();
        } else {
            // Registered but unimplemented: callable name, no kernel → #NAME?.
            writeln!(
                out,
                "        {i} => CellValue::Error(CellError::Name), // {name} (planned)",
                i = i,
                name = r.name,
            )
            .unwrap();
        }
    }

    out.push_str(
        "        _ => CellValue::Error(CellError::Name),\n\
         \x20   }\n\
         }\n\n",
    );

    // ---- Emit the rich dispatch function (dynamic-array door). ----
    out.push_str(
        "/// Rich registry-driven dispatch (spec §6.4, M1 array track). The\n\
         /// dynamic-array door: an `implemented` `returns_array` row calls its\n\
         /// `-> FnResult` kernel directly (after the same arity guard); a\n\
         /// scalar `implemented` row is wrapped `FnResult::Scalar(<kernel>)`;\n\
         /// a `planned` row returns `FnResult::Scalar(#NAME?)`; an arity\n\
         /// violation returns `FnResult::Scalar(#VALUE!)`. Every formula\n\
         /// function call SHOULD route here so array results are not lost; the\n\
         /// scalar `dispatch` stays for callers that only ever need a scalar\n\
         /// (and returns #VALUE! for array rows).\n\
         #[allow(clippy::len_zero, clippy::absurd_extreme_comparisons, unused_comparisons)]\n\
         pub fn dispatch_rich(\n\
         \x20   id: sheet_core::FuncId,\n\
         \x20   args: &[crate::arg::Arg],\n\
         \x20   ctx: &crate::ctx::EvalCtx,\n\
         ) -> crate::result::FnResult {\n\
         \x20   use sheet_core::value::{CellError, CellValue};\n\
         \x20   use crate::result::FnResult;\n\
         \x20   let _ = (args, ctx);\n\
         \x20   match id.0 {\n",
    );

    for (i, r) in rows.iter().enumerate() {
        let implemented = r.status.eq_ignore_ascii_case("implemented");
        if implemented && r.special_form {
            // Special form — handled in eval.rs before dispatch_rich too.
            writeln!(
                out,
                "        {i} => FnResult::Scalar(CellValue::Error(CellError::Name)), // {name} (special_form — handled in eval.rs)",
                i = i,
                name = r.name,
            )
            .unwrap();
        } else if implemented {
            let path = format!("crate::families::{}", r.rust.trim());
            let max_guard = match r.arity.max {
                Some(m) => format!(" || args.len() > {m}"),
                None => String::new(),
            };
            if r.returns_array {
                // Array kernel: returns FnResult directly.
                writeln!(
                    out,
                    "        {i} => {{ // {name} (returns_array)\n\
                     \x20           if args.len() < {min}{max_guard} {{\n\
                     \x20               return FnResult::Scalar(CellValue::Error(CellError::Value));\n\
                     \x20           }}\n\
                     \x20           {path}(args, ctx)\n\
                     \x20       }}",
                    i = i,
                    name = r.name,
                    min = r.arity.min,
                    max_guard = max_guard,
                    path = path,
                )
                .unwrap();
            } else {
                // Scalar kernel: wrap in FnResult::Scalar.
                writeln!(
                    out,
                    "        {i} => {{ // {name}\n\
                     \x20           if args.len() < {min}{max_guard} {{\n\
                     \x20               return FnResult::Scalar(CellValue::Error(CellError::Value));\n\
                     \x20           }}\n\
                     \x20           FnResult::Scalar({path}(args, ctx))\n\
                     \x20       }}",
                    i = i,
                    name = r.name,
                    min = r.arity.min,
                    max_guard = max_guard,
                    path = path,
                )
                .unwrap();
            }
        } else {
            writeln!(
                out,
                "        {i} => FnResult::Scalar(CellValue::Error(CellError::Name)), // {name} (planned)",
                i = i,
                name = r.name,
            )
            .unwrap();
        }
    }

    out.push_str(
        "        _ => FnResult::Scalar(CellValue::Error(CellError::Name)),\n\
         \x20   }\n\
         }\n",
    );

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let dest = Path::new(&out_dir).join("dispatch.rs");
    fs::write(&dest, out).unwrap_or_else(|e| panic!("sheet-fn build.rs: write {dest:?}: {e}"));
}
