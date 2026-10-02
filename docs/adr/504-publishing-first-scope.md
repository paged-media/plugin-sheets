# ADR 504 — Publishing-first scope: pivots, validation and macros are preserved and never interpreted

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `sheet-xlsx`, `sheet-calc/src/external.rs`, `registry/features/`, the bundle manifest

## Context

The README defines the product as "A publishing instrument, not an Excel replacement"
(`README.md:8-9`), and `CLAUDE.md:10-11` adds that "every scope decision follows from
that". Workbooks arrive with features whose use is interaction or automation: pivot tables
and slicers, data-validation dropdowns, what-if tools, VBA, links to other workbooks.
`CLAUDE.md:119-122` states the rule for them and its standing: they "are NEVER interpreted
— they round-trip preserved", and "This is a product decision, not a deferral".

The reason is written out for pivot tables. A "pivot table's value is the INTERACTION
(drag-to-summarize, collapse/expand, slicers, re-pivot), exactly the surface a
print/publishing product does not own" (`registry/features/pivot.yaml:8-11`). The same
record assigns grouped aggregation to the `paged.data` plugin and visual summaries to the
chart engine, and says a summary table in a sheet is authored as ordinary cells and
formulas. For data validation, what-if tools, external links and VBA the repository states
the rule and the same premise, with no argument of its own.

## Decision

Pivot tables, slicers, data validation, what-if tools, VBA and links to other workbooks
are never evaluated, executed, enforced or made editable. Their parts and elements pass
through the preservation path of [ADR 503](503-xlsx-patched-not-regenerated.md). The
repository records this as permanent scope, not as a backlog.

- **Pivots, slicers, VBA.** Their parts are `PartEntry::Opaque` and are written back byte
  for byte. No source file of a shipping crate or of the bundle reads them.
- **Data validation.** `<dataValidations>` is a captured worksheet child. A read-only
  inventory (kind, ranges, raw operand text) is parsed from it, and the workbook panel
  shows a count per sheet with the words "preserved, not enforced". No edit is blocked and
  no dropdown is drawn.
- **External links.** The `externalLinkN.xml` parts stay opaque; their cached values are
  parsed into `XlsxDocument::external_links`. A link is never followed: no network and no
  file access. The formula AST has no external-reference variant and the parser does not
  accept the `[n]` prefix, so a cell with such a formula shows the cached value stored in
  the worksheet.
- **Network.** The manifest declares `"network": false`.

## Evidence

- `README.md:8-9`, `CLAUDE.md:10-11`, `CLAUDE.md:119-122` — the premise and the rule
- `registry/features/pivot.yaml:1-16`, `:29-37` — the pivot non-goal, its rationale, the ruling
- `sheet-xlsx/src/parts/data_validation.rs:36-61`, `sheet-xlsx/src/lib.rs:156-165` —
  preserve, no enforcement, inventory only
- `packages/sheet-bundle/src/panels/workbook-panel.tsx:596-625` — the inventory in the panel
- `sheet-calc/src/external.rs:36-57`, `:109-111` — links never followed, no AST variant,
  `#REF!` when the cache has no entry
- `sheet-xlsx/src/opc.rs:59-61`, `sheet-conformance/tests/xlsx_roundtrip.rs:236-253`,
  `:261-304` — pivot caches and VBA are opaque parts; both survive a save unchanged
- `packages/sheet-bundle/manifest.json:13` — `network`

## Alternatives considered

- **A pivot engine.** Refused: "Do NOT "helpfully" implement a pivot engine"
  (`registry/features/pivot.yaml:15-16`).
- **A dropdown for validated cells.** Not built: "we stop at the preserve-and-inventory
  line — strictly less than a runtime dropdown" (`sheet-xlsx/src/parts/data_validation.rs:57-61`).
- **Evaluating external references.** Left for later: "Full external-ref EVALUATION via an
  AST variant is a future versioned amendment" (`registry/features/extlink.yaml:45-47`).

## Consequences

`sheet-xlsx` has to carry content it will never model, which is what ADR 503 provides. The
line to `paged.data` is drawn in a comment; no code here depends on that plugin. Limits of
the code today:

- `resolve_cached` and the `ExternalCache` trait have no caller outside
  `sheet-conformance/tests/extlink.rs`; the engine never resolves an external reference
  through the parsed cache. What a user sees is the value cached in the cell. A formula
  that fails to parse on load is counted in `unparsed_formulas` and keeps its raw text and
  cached value (`sheet-js/src/core.rs:454-469`); no test runs an external-reference
  formula through that path.
- A validation rule is written back unchanged; edits to its cells are not checked against it.
- `registry/features/pivot.yaml:35-37` says pivot caches are exercised in
  `04-unknown-parts.xlsx`. That fixture holds custom XML, a fake VBA project and a calc
  chain (`corpus/xlsx-corpus/generate.py:312-336`). The pivot check is the separate test
  `sheet_xlsx_pivot_cache_preserved_byte_identical`, which no registry row points at.
- No test asserts anything about slicers or what-if data.

## Related

- [ADR 503](503-xlsx-patched-not-regenerated.md) — the preservation mechanism this scope relies on
- [ADR 500](500-own-calculation-engine.md), [ADR 509](509-excel-first-dialect.md) — the
  engine whose scope this bounds; what happens to a formula the parser rejects
- [ADR 315](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/315-isolation-contract.md)
  — why grouping in `paged.data` is not reached from this plugin's code
