# DECIMAL-SPIKE — the D-6 exact-decimal evaluation

The M3 spike deliverable for **decision D-6** ("Numeric core: Excel-compat
`f64` vs exact-decimal mode — `f64` v1; the trait boundary keeps decimal
mode open"). This is the written adopt/defer recommendation, grounded in a
working implementation, a divergence corpus, and measured size/perf costs.

- **Spec:** `thoughts/docs/paged/plugin-sheets/base-idea.md` §3, §5.1, §13 M3.
- **Code:** `sheet-fn/src/num.rs` (the `Numeric` trait + the v1 `F64` impl),
  `sheet-fn/src/num_decimal.rs` (the spike's `Decimal` impl, behind the
  `exact-decimal` cargo feature).
- **Evidence:** `sheet-conformance/tests/decimal_spike.rs` +
  `corpus/decimal-corpus/divergence.golden.tsv`.
- **Registry ruling:** `registry/features/decimal.yaml`
  (`sheet.calc.decimal.*`).

## TL;DR — recommendation

**DEFER exact-decimal to v2, as an explicit opt-in flag — do NOT make it the
default. f64 stays the v1 default.**

The spike succeeds technically: an exact base-10 `Numeric` backend drops into
the D-6 seam as a pure type substitution, with no kernel rewrite and a small
(~40 KiB) wasm cost. But adopting it *now* would either (a) silently diverge
from Excel — the very oracle this engine is conformance-tested against — or
(b) require building a whole opt-in surface (per-workbook precision setting,
XLSX round-trip semantics, formatter/coercion review) for which there is no
v1 demand. The right move is to **keep the seam proven and shipped (this
spike), and gate the user-facing mode on real v2 demand** — paged.sheet is a
*publishing instrument*, and a print document showing `0.30000000000000004`
is already prevented by the 15-significant-digit display rule (D-6), so the
correctness win is mostly invisible in v1's actual output.

## What the spike built

`sheet-fn/src/num_decimal.rs` implements `impl Numeric for Decimal`, a
transparent newtype over `rust_decimal::Decimal`, gated behind the
`exact-decimal` cargo feature (OFF by default). It is a literal drop-in for
`F64`: the conformance test's `replay::<N: Numeric>(…)` runs the *identical*
code through both backends. That is the D-6 thesis demonstrated — the trait
boundary in `sheet-fn` keeps decimal mode open, and swapping it in is a type
substitution, not a rewrite.

The `from_f64` lift uses rust_decimal's default *remove-excess-bits*
rounding, so the user's literal `0.1` becomes exactly `0.1` (not the IEEE
`0.1000000000000000055…`). That is the hinge: summing the *typed* values in
base-10 avoids the binary representation error f64 carries through the same
arithmetic.

## Carrier evaluated — `rust_decimal`

| property            | finding |
|---------------------|---------|
| license             | **MIT** (transitive deps `arrayvec`, `num-traits`: MIT OR Apache-2.0) — clean for the dual MPL-2.0/PMEL repo |
| purity / wasm       | pure Rust, ships a `cfg(target_arch = "wasm32")` shim, **no native/C deps** (`cargo tree -p sheet-fn --features exact-decimal` shows only `arrayvec` + `num-traits`) |
| representation      | 96-bit integer significand × base-10 scale (0..=28) → **28-29 significant digits** |
| headroom vs Excel   | comfortably wider than Excel's 15-significant-digit *display* budget (D-6) |
| features used       | `default-features = false`, `["std", "maths"]` — drops serde/db/rand; `maths` adds `MathematicalOps` (powi/powu/powd) |

Alternatives considered and rejected: a **hand-rolled fixed-point `i128`**
(re-implements rounding, scale alignment, and `Display` from scratch — more
code, less tested, no upside at the spike stage); **`bigdecimal`** (arbitrary
precision, heavier, pulls `num-bigint`). 28-29 digits is the right amount of
exactness for a publishing workload.

## Correctness wins (the divergence corpus)

`corpus/decimal-corpus/divergence.golden.tsv` replays each case through both
backends. The f64 half runs in the default build (so the goldens are pinned
as the true IEEE-754 values Excel itself carries); the decimal half runs
under `--features exact-decimal`.

| case | f64 result | decimal result | diverges |
|------|-----------:|---------------:|:--------:|
| `0.1 + 0.2`            | `0.30000000000000004` | `0.3`  | yes |
| `0.1 + 0.2 + 0.3`      | `0.6000000000000001`  | `0.6`  | yes |
| `1.1 + 2.2`            | `3.3000000000000003`  | `3.3`  | yes |
| `0.10 + 0.20 + 0.05`   | `0.35000000000000003` | `0.35` | yes |
| `0.01 added ×100`      | `1.0000000000000007`  | `1`    | yes |
| `0.1 added ×10` (left-fold) | `0.9999999999999999` | `1` | yes |
| `0.5 + 0.25 + 0.125`   | `0.875`               | `0.875`| **no** |
| `19.99 + 5.99 + 2.49`  | `28.47`               | `28.47`| **no** |

The corpus is deliberately **honest about both outcomes**: f64 is not always
wrong — sums of exactly-representable binary fractions (halves, quarters,
eighths) and of integers foot exactly, so the spike does not overstate the
win. The divergence also depends on accumulation ORDER: these goldens are a
strict left-fold from `0.0` (the order the test replays), so ten `0.1`s fold
to `0.9999999999999999`, not the `1.0` a pairwise or Kahan sum might reach —
itself a useful reminder that f64 error is path-dependent while decimal is
not.

### The central caveat: decimal is not a universal cure

`1/3` is non-terminating in **base-10 too**. Decimal `1/3 * 3` carries 28
nines (`0.9999…9`), which is *not* exactly `1` — it only rounds back to `1.0`
at the f64 boundary (same as f64's own lucky `1.0`). Asserted in
`sheet_calc_decimal_impl_thirds_caveat`. Exact-decimal buys exactness for
**terminating** decimals (money, tenths, the `+ - * /` of accumulation), not
for ratios with non-terminating base-10 expansions, and not for
transcendentals.

### `pow` is only partly exact

The `Decimal::pow` follows `MathematicalOps::powd`: **integer exponents are
exact** (repeated multiplication); a **fractional exponent** falls back to the
`e^(y·ln x)` Taylor approximation (rust_decimal's documented `~1e-7`
tolerance). Fractional `POWER`/`^` would remain an f64-domain operation in any
real adoption.

## Costs

### Size (wasm)

Measured by building representative throwaway `cdylib`s for
`wasm32-unknown-unknown` (`opt-level = "s"`, `lto`, exercising
`from_f64`/`+`/`-`/`*`/`/`/`powd`/`to_f64`):

| build | wasm size |
|-------|----------:|
| baseline (no `rust_decimal`)         | ~0.4 KiB |
| with `rust_decimal` (`std` + `maths`)| ~49 KiB  |
| **delta**                            | **~48 KiB** (~40 KiB after `wasm-opt -Oz`) |

Well within the 8 MiB plugin budget. `sheet-js` does not currently forward
the feature, so the **default shipped wasm is byte-unchanged** — `rust_decimal`
is absent from the default dependency tree (verified:
`cargo tree -p sheet-fn` shows no decimal).

### Speed

Fair add-only / multiply-only microbenchmarks (native, `opt-level = 3`):

- **add / sub:** roughly at parity with f64 (integer add on the significand
  when scales align).
- **multiply / divide:** **~6-7× slower** than f64 (scale normalization).

For spreadsheet workloads (sums of columns dominate; multiplies are sparse)
this is acceptable, but it is a real cost that argues against making decimal
the *default*.

## Why DEFER (not ADOPT-now), in full

1. **Excel-compat tension (the decisive one).** Excel is f64-based (D-6). The
   engine's whole conformance posture — the golden corpora, the planned
   LibreOffice differential oracle — assumes f64. An exact-decimal mode would
   **diverge from the oracle by design** on exactly the divergence-corpus
   cases. Shipping it as the default would turn passing conformance into
   failing conformance. It can only ever be an *opt-in* that the user accepts
   "this no longer matches Excel."
2. **The display rule already hides the v1 pain.** D-6's 15-significant-digit
   *display* semantics mean a v1 print document never shows
   `0.30000000000000004` — it shows `0.3`. The correctness win is largely
   invisible in v1's actual rendered output, which is the product surface.
3. **Opt-in is a surface, not a flag.** A real exact-decimal *mode* needs: a
   per-workbook precision setting in `CalcSettings`, an XLSX round-trip story
   (Excel has no "decimal mode" to round-trip *to*), and a review of the
   formatter + coercion boundaries. That is v2-sized work with no v1 demand.
4. **The seam is the deliverable.** D-6's actual requirement — "the CellValue
   design must not foreclose it" — is **met and now proven** by this spike. We
   do not need to ship the mode to satisfy D-6; we need to keep the door open,
   which it demonstrably is.

## What stays true regardless

**f64 stays the v1 default.** This spike changes no default build, no kernel,
and no format golden; en-US output is byte-identical. The `exact-decimal`
feature is OFF by default and `sheet-js` does not forward it, so the shipped
wasm is unchanged. The recommendation is recorded as a registry ruling
(`sheet.calc.decimal.spike-report`); if v2 demand materializes, the
implementation is already sitting behind the feature flag, ready to be wired
to an opt-in `CalcSettings.precision` mode.
