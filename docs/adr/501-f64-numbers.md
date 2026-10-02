# ADR 501 — Numbers are f64 because Excel's are; exact decimal stays behind a seam

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `sheet-core` (`CellValue`), `sheet-fn` (`num`, `num_decimal`), `sheet-calc`, `sheet-format`

## Context

An exact base-10 number type removes the error that binary floating point makes on sums
of money and tenths: in f64, `0.1 + 0.2` is `0.30000000000000004`. The engine is also meant
to give the results Excel gives, and `DECIMAL-SPIKE.md:144` states that "Excel is
f64-based". Code comments and that report call the choice decision `D-6`.

The seam came first: `sheet-fn/src/num.rs` defines a `Numeric` trait so that a decimal
backend could be added later. On 2026-06-08 a spike (commit `e7724f2`) implemented the
trait over `rust_decimal`, built a divergence corpus, measured size and speed, and wrote
its recommendation into `DECIMAL-SPIKE.md` at the repository root.

The report gives four reasons to defer (`DECIMAL-SPIKE.md:142-162`). The decisive one: the
golden corpora and the planned oracle assume f64, so a decimal mode "would **diverge from
the oracle by design**", and as a default it "would turn passing conformance into failing
conformance". The others: rounding to 15 significant digits on display already hides the
binary tail; an opt-in mode needs a per-workbook setting and an answer for XLSX, which has
no decimal mode; and the requirement was only that the value type must not rule one out.

## Decision

`CellValue::Number` holds an `f64` and all arithmetic is IEEE-754 double precision. Exact
decimal was not adopted. The report's recommendation is to "DEFER exact-decimal to v2, as
an explicit opt-in flag" (`DECIMAL-SPIKE.md:19`).

- `Numeric` has seven operations: `from_f64`, `to_f64`, `add`, `sub`, `mul`, `div`, `pow`.
  The default build has one implementation, the newtype `F64`.
- The decimal implementation, `num_decimal::Decimal`, compiles only under the cargo
  feature `exact-decimal` of `sheet-fn`, which is off by default and pulls in the optional
  `rust_decimal`. `sheet-js` declares no features and the wasm build passes none, so the
  wasm module contains no decimal code.
- The `General` format rounds to 15 significant digits before it prints.

## Evidence

- `sheet-core/src/value.rs:43-50` — `CellValue::Number(f64)`
- `sheet-fn/src/num.rs:33-45`, `:52-64`, `:70-101` — the purpose of the seam, the trait,
  and `F64`
- `sheet-fn/Cargo.toml:13-21`, `:35-38`, `sheet-fn/src/lib.rs:77-78`,
  `sheet-fn/src/num_decimal.rs:95-144` — the feature, the optional dependency, the `cfg`
  gate on the module, and `impl Numeric for Decimal`
- `sheet-js/Cargo.toml:1-41`, `scripts/build-wasm.sh:22` — no feature is declared for the
  wasm crate or passed to its build
- `sheet-fn/src/families/agg.rs:114-118`, `sheet-calc/src/eval.rs:426-449` — a kernel
  calling the trait's methods on `F64`; the formula operators on plain `f64`
- `sheet-format/src/general.rs:85-86` — `round_sig(x, 15)`
- `sheet-conformance/tests/decimal_spike.rs:148`, `:201-215`, `:222-255` — the generic
  `replay`, the f64 half that always runs, and the decimal half behind the feature

## Alternatives considered

- **Exact decimal as the default.** Rejected for the reasons above. Measured cost: about
  48 KiB of wasm, and multiply and divide about 6 to 7 times slower than f64
  (`DECIMAL-SPIKE.md:111-140`).
- **Carrier for the spike.** `rust_decimal` was chosen over a hand-rolled fixed-point
  `i128` and over `bigdecimal` (`DECIMAL-SPIKE.md:60-64`).

## Consequences

Results carry the representation error of binary floating point; the report holds that
these are "the true IEEE-754 values Excel itself carries" (`DECIMAL-SPIKE.md:69-70`). It
also records limits of the decimal backend: `1/3 * 3` is not exactly 1 in base 10, and a
fractional exponent falls back to an approximation (`DECIMAL-SPIKE.md:93-109`).

The seam covers less than some texts in the repository say. Six of the eighteen family
modules in `sheet-fn/src/families/` import `Numeric` (`agg`, `database`, `math`, `math2`,
`stat`, `t2misc`) and call its methods on the concrete type `F64`. No kernel is generic
over the trait; the only generic code is the `replay` function of the spike test. Other
arithmetic is plain `f64`: the evaluator's `+ - * / ^`, and expressions inside those
modules such as `v - m` at `sheet-fn/src/families/stat.rs:292`. The registry note "Kernels
route arithmetic through the trait, so a `Decimal` impl is a type substitution, not a
rewrite" (`registry/features/decimal.yaml:19-20`) holds for the accumulation loops, not
for the engine as a whole.

The decimal backend is not exercised by CI: no workflow under `.github/` enables
`exact-decimal`. There is no precision setting either; `CalcSettings` has no such field
(`sheet-core/src/calc_settings.rs:41-53`). Two comments are stale:
`sheet-fn/Cargo.toml:16` and `registry/features/decimal.yaml:9` still call `num_decimal`
a stub.

## Related

- `DECIMAL-SPIKE.md` (repository root) — the full report this record summarises
- [ADR 500](500-own-calculation-engine.md), [ADR 507](507-golden-corpora-coverage-gate.md)
  — the engine these numbers flow through; the corpora whose expected values are f64 results
