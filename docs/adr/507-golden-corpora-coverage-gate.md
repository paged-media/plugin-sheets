# ADR 507 — Verification: authored goldens tied to the registry by a coverage gate

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `sheet-conformance`, `corpus/`, the `tests:` and `ruling:` fields in `registry/`, `.github/workflows/rust.yml`

## Context

The engine is written in this repository from public specifications
([ADR 500](500-own-calculation-engine.md)). Function dispatch is generated from `registry/`,
where a function without a row cannot be called
([ADR 317](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/317-registry-driven-dispatch.md)).
`sheet-conformance/src/bin/coverage_gate.rs:33-36` names the other half that was wanted:
every row marked `implemented` must point at tests that exist.

Two further rules are stated in the repo. `CLAUDE.md:114-118`: behaviour copied from Excel,
including its defects, is adopted as an explicit ruling with provenance, never by accident.
`sheet-conformance/src/lib.rs:61-62`: the engine is bit-stable, so goldens are compared as
bytes and there is no tolerance machinery.

## Decision

The conformance harness is one test-only crate, `sheet-conformance`, which no shipping crate
may depend on. It rests on golden files authored in this repository, and a gate run in CI
binds every implemented registry row to tests that exist.

- **Goldens.** `corpus/` holds 237 golden TSV files: 221 under `fn-corpus/`, 15 for number
  formats, 1 for the decimal spike. A formula golden is one case per line: `id`, `formula`,
  `setup`, `expected`, tab-separated. `sheet-conformance/tests/corpus_runner.rs` replays 17
  families through `sheet_calc::Engine` and compares the result string with `expected`.
- **Coverage gate.** The `coverage-gate` binary reads every YAML file under
  `registry/functions` and `registry/features`. For each row with `status: implemented` it
  requires a `tests:` block. A `rust` pointer `file.rs::prefix` must name an existing file
  that contains `fn <prefix>`; a `corpus` or `vitest` pointer must name an existing file. Any
  gap makes the binary exit 1. CI runs it after the test suite.
- **Rulings.** A deliberate behavioural choice is recorded on the registry row as a `ruling:`
  field, next to its `provenance:` and its tests. 45 feature rows carry one.
- **XLSX fixtures.** The 15 workbooks in `corpus/xlsx-corpus/` are built by two Python scripts
  that use the standard library only and a fixed timestamp, so the bytes are reproducible.
- **Real workbooks.** `sheet-conformance/tests/real_xlsx_corpus.rs` holds four tests over
  workbooks kept outside this repository. They are `#[ignore]`d and need `PAGED_XLSX_CORPUS`.
  The bar differs by source: files from Apache POI's test corpus must never panic the parser,
  and their open rate is printed, not gated; workbooks written by Excel must all open; legacy
  `.xls` files must be refused.

## Evidence

- `sheet-conformance/Cargo.toml:8`; `.github/workflows/rust.yml:33` — test-only by description; CI fails if the wasm dependency tree contains `sheet-conformance` or `proptest`
- `sheet-conformance/src/lib.rs:42-62` — the golden format and the byte comparison; `sheet-conformance/tests/corpus_runner.rs:180-226`, `:274-362` — one case, and the 17 family tests
- `sheet-conformance/src/bin/coverage_gate.rs:121-139`, `:169-208`, `:240-246` — status filter, lanes checked, the `fn <prefix>` match; `.github/workflows/rust.yml:49-52` — nextest, then the gate
- `registry/features/format.yaml:61-67` — a ruling row: `sheet.format.date.leap1900`, "Adopted bug-for-bug", with provenance and a test pointer
- `corpus/xlsx-corpus/generate.py:2-12` — standard library only, fixed timestamp
- `sheet-conformance/tests/real_xlsx_corpus.rs:26-42`, `:188-364` — the reason for the different bars; the four tests, each calling only `XlsxDocument::open`
- `sheet-conformance/tests/oracle.rs:74-104` — both oracle tests: `#[ignore]`, an early return, then `todo!()`

## Alternatives considered

A differential oracle: `sheet-conformance/tests/oracle.rs:33-53` describes running headless
LibreOffice as an external process and comparing its values with the engine's. Only the
skeleton exists. A comparison with tolerance is ruled out by the bit-stable claim above.

## Consequences

A function cannot be called without a registry row, and a row cannot say `implemented`
without pointing at tests that exist; CI checks this on every pull request. Feature rows are
checked the same way, but no build script reads them, so nothing obliges a feature to have one.

What the gate does not prove:

- It checks that a pointer resolves, not that the test exercises the row, and not that the
  test passes. For Rust the preceding nextest step fails the job on a red test. For `vitest`
  pointers nothing does: the vitest workflow runs on pushes to `main` and on manual dispatch,
  not on pull requests, and its test step ends in `|| true` (`.github/workflows/vitest.yml:2-5`, `:73-77`).
- No test invokes another spreadsheet program; the expected values are the ones authored
  into the golden files. The oracle tests end in `todo!()`, and no workflow sets
  `PAGED_SHEET_ORACLE`. Registry text still refers to the oracle as pending
  (`registry/features/coerce.yaml:41`, `registry/features/parser.yaml:35`).
- The real-workbook tests assert only that a file opens or is refused. They do not recalculate
  it or compare against the values cached in the file, and they do not run in this repo's CI:
  no workflow sets `PAGED_XLSX_CORPUS` or passes `--ignored`.
- The fixtures in `corpus/xlsx-corpus/` are all generated by this repo's scripts; none was
  written by Excel (`sheet-conformance/tests/real_xlsx_corpus.rs:19-22`).

The module comment of `corpus_runner.rs` (`:42`) says seven families are replayed; the file has 17.

## Related

- [ADR 500](500-own-calculation-engine.md), [ADR 502](502-recalculation-and-spill.md) — the engine under test; the deterministic recalculation order behind the byte comparison
- [ADR 317](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/317-registry-driven-dispatch.md) — the registry and the gate as a pattern across plugins
- [ADR 501](501-f64-numbers.md), [ADR 503](503-xlsx-patched-not-regenerated.md) — the decimal corpus; the round-trip invariants the XLSX fixtures test
