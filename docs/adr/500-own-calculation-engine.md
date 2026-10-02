# ADR 500 — The calculation engine is own-built and clean-room

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** all `sheet-*` crates, `registry/functions/`, `deny.toml`

## Context

`paged.sheet` needs a formula parser, a dependency graph with a scheduler, a function library, a number-format engine and
XLSX input and output. Open-source implementations of these exist. The repository names two of them as reference material,
LibreOffice and IronCalc, in two roles: as source that could be mounted for reference under `references/` (`CLAUDE.md:160`),
and as an oracle to compare results against (`sheet-conformance/tests/oracle.rs:33-40`, `:69-71`). Apache POI, OpenOffice
and Gnumeric are named only as the origin of test workbooks (`sheet-conformance/tests/real_xlsx_corpus.rs:29-32`).

`CLAUDE.md:160-164` states the rule the code follows. The reference mount is "read-only,
analyst-only, gitignored, excluded from all artifacts; implementers never read it", it is
not mounted, and the "implementation derives from ECMA-376 / ISO-IEC 29500, OpenFormula,
public Microsoft documentation, and golden corpora".

The rule is stated without the comparison behind it: why no existing engine, reader or
writer was adopted as a component. The repository does not record why. One neighbouring
reason is recorded: running LibreOffice as an oracle is held compatible with the rule
because "behaviour is not copyrightable and LibreOffice is *not linked* — it is invoked as
an external process" (`sheet-conformance/tests/oracle.rs:38-40`).

## Decision

The parser, the dependency graph and scheduler, the function library, the number-format
engine and the XLSX reader and writer were written in this repository from public
specifications. No spreadsheet engine, formula evaluator or XLSX reader or writer crate is
a dependency, and no reference implementation's source is in the tree.

- `references/` is reserved for reference source. It is ignored by git, it is not a
  workspace member, and no tracked file lives under it.
- The third-party crates the shipping crates depend on are `zip` and `quick-xml` (container and XML in `sheet-xlsx`),
  `plotters` and `plotters-backend` (chart layout), `ryu`, `serde`, `compact_str`, `smallvec`, `rustc-hash`, `thiserror`,
  the `wasm-bindgen` glue in `sheet-js`, and the optional `rust_decimal` ([ADR 501](501-f64-numbers.md)). `sheet-core`
  and `sheet-fn` also use `serde_yaml` in their build scripts to read the registry (`sheet-core/Cargo.toml:19-21`,
  `sheet-fn/Cargo.toml:40-42`). `Cargo.lock` holds 117 packages, 11 of them this workspace's own crates.
- Each of the 224 rows in `registry/functions/*.yaml` carries a `provenance` field: 180
  cite ECMA-376 and the other 44 cite public Microsoft documentation.
- `deny.toml` allows crates only from the crates.io index and only under the licences it
  lists; CI runs `cargo-deny`.

## Evidence

- `CLAUDE.md:160-164` — the clean-room rule and the public sources
- `.gitignore:4-5`, `Cargo.toml:1-4` — `references/` is never committed, never a member
- `Cargo.toml:37-72` — the complete list of third-party workspace dependencies
- `sheet-xlsx/Cargo.toml:39-42`, `sheet-calc/Cargo.toml:13-18`,
  `sheet-parser/Cargo.toml:13-17` — what the XLSX layer, the engine and the parser build on
- `registry/functions/math.yaml:15`, `registry/functions/agg.yaml:11` — the `provenance`
  column and one row
- `deny.toml:13-35`, `:42-45`, `.github/workflows/rust.yml:34` — the licence allow-list,
  the source rule, and the CI step
- `sheet-conformance/tests/oracle.rs:74-85`, `:94-104` — both oracle tests are `#[ignore]`d
  and end in `todo!()`

## Alternatives considered

None recorded in the repository. LibreOffice and IronCalc appear only as reference source
and as oracle, never as a component to build on. `git log -S` over the Cargo manifests and
`Cargo.lock` finds no commit that mentions `ironcalc`, `calamine`, `rust_xlsxwriter`,
`formualizer` or `umya`.

## Consequences

Every function, format rule and XLSX part handler is this repository's code to write and
to test. A new dependency has to pass `cargo-deny`. The `provenance` field is a convention
that nothing checks: both build scripts that read the registry ignore it
(`sheet-core/build.rs:81`, `sheet-fn/build.rs:80`).

Correctness is not inherited from another engine. It rests on the authored golden corpora
and the coverage gate ([ADR 507](507-golden-corpora-coverage-gate.md)). The differential
oracle that would compare results with LibreOffice is not built:
`sheet-conformance/tests/oracle.rs` holds two ignored tests whose bodies are `todo!()`.

Excel behaviour that is adopted on purpose, defects included, is recorded row by row as a
registry ruling (`CLAUDE.md:114-118`). Some rows still wait for the oracle:
`registry/features/coerce.yaml:41` gives its provenance as "Excel observed behavior
(oracle-verified later)".

## Related

- [ADR 501](501-f64-numbers.md), [ADR 502](502-recalculation-and-spill.md),
  [ADR 503](503-xlsx-patched-not-regenerated.md), [ADR 509](509-excel-first-dialect.md) —
  what was built: the number type, the scheduler, the XLSX layer, the parser
- [ADR 507](507-golden-corpora-coverage-gate.md) — how the own-built engine is verified
- [ADR 016](016-chart-engine-plotters-chartgeometry.md) — the third-party layout library that was adopted
- [ADR 317](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/317-registry-driven-dispatch.md)
  — the registry that carries the provenance column
