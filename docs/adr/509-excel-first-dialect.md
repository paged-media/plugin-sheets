# ADR 509 — Excel-first formula dialect; locales are data

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `71f37d7`.
- **Scope:** `sheet-parser`, the `Locale` type in `sheet-core`, `sheet-format/src/locale.rs`, the locale derivation in `sheet-xlsx`

## Context

The engine parses formulas from two places: the formula text stored in an XLSX file, and what
a user types into a cell after `=`. Both go through `sheet_parser::parse`
(`sheet-js/src/core.rs:452`, `sheet-calc/src/lib.rs:269`).

`registry/features/parser.yaml:1-2` records the order chosen: Excel first, with the note
"OpenFormula maps onto the same canonical AST later". The repository does not record why.

Separately, number formats have to render in the language of the workbook: decimal and group
separators, month and day names. `registry/features/locale.yaml:16-19` draws the line: the
formula dialect stays English; only the parsing and formatting of values localises.
The repository does not record why.

## Decision

`sheet-parser` accepts one formula dialect, Excel's en-US form, and produces the
dialect-neutral AST defined in `sheet-core`. Localisation never touches the formula
language: a locale is a table of separators and names read by the number-format engine.

- The dialect: A1 and structured table references, `,` as the argument separator, `;` only
  as the row separator inside an array literal, and function names from the generated
  registry table, which are English.
- The printer emits canonical text: parsing what it prints gives the same AST. The same crate
  extracts references for the dependency graph and rewrites a formula when rows or columns
  are inserted or deleted.
- An unknown function name is a `ParseError`, not a `#NAME?` node. The stated reason: a name
  without a registry row has no `FuncId`, so the AST cannot represent it, and a `#NAME?`
  literal would lose the round trip. On XLSX load a formula that fails to parse stays a value
  cell with its raw text and cached value, and is counted in `unparsed_formulas`.
- `Locale` has five variants: `EnUs` (the default), `DeDe`, `FrFr`, `EsEs`, `ItIt`. Each has
  one `LocaleData` constant: decimal, group and list separators, long and short month and day
  names, AM/PM strings and a default short-date pattern.
- The format-code grammar is locale-neutral; the locale decides only which glyphs and names
  the renderer emits. An explicit AM/PM token renders `AM`/`PM` in all five; the ruling cites
  Excel's German locale, which does not substitute its own strings.
- A `[$<symbol>-<LCID>]` token in a format code selects the locale for that code. The workbook
  locale is taken from the first custom number format whose LCID resolves to one of the four
  non-English locales; otherwise it is en-US.

## Evidence

- `sheet-parser/src/lib.rs:33-39`, `:41-55` — what the crate produces; the rulings on unknown functions, `,` and `;`
- `sheet-parser/src/pratt.rs:312-324`, `:362-380` — the registry lookup and its error; `;` consumed only in an array literal
- `registry/features/parser.yaml:25-31`, `:48-53` — the unknown-function ruling; the fixpoint as a tested row
- `sheet-js/src/core.rs:446-470`; `sheet-calc/src/lib.rs:257-275` — load keeps and counts unparsed formulas; cell entry parses after `=` and takes no locale
- `sheet-core/src/calc_settings.rs:55-83`; `sheet-format/src/locale.rs:39-47`, `:88-119`, `:350-381` — the `Locale` type; what localises; `LocaleData`; the two lookups
- `registry/features/locale.yaml:48-57`, `:113-132`, `:134-152` — the AM/PM ruling; fr/es/it as table rows; why CJK is not a row
- `sheet-format/src/parse.rs:224-233`; `sheet-xlsx/src/parts/styles.rs:280-297`; `sheet-xlsx/src/lib.rs:292-299` — the per-code LCID; the workbook locale and its en-US fallback

## Alternatives considered

OpenFormula is named as a later mapping onto the same AST; no OpenFormula parser exists in
the repository. Teaching the parser `;` as an argument separator was not done: goldens written
that way are rewritten to `,` by the test runner before they reach the engine
(`sheet-conformance/tests/corpus_runner.rs:48-55`). Japanese and Chinese locales are a
registry row with `status: planned`: era calendars and their own AM/PM strings need rendering
code and verification against Excel, not a table row (`registry/features/locale.yaml:134-152`).

## Consequences

A formula with localised function names or `;` between arguments does not parse. Typed into
a cell it is an error; found in a file it leaves the cell showing its cached value. The
printer emits the same dialect, so an edited formula is written back to XLSX in it
([ADR 503](503-xlsx-patched-not-regenerated.md)).

A new Latin-script locale is a `Locale` variant in `sheet-core`, a `LocaleData` constant and
an arm in `locale_data` and `locale_from_lcid`, with no change to the rendering code; commit
`70ed3f3` added fr, es and it that way. An LCID outside the five resolves to en-US.

Limits of what is built:

- Nothing sets the locale except the XLSX loader. `sheet-js` and the bundle have no setter, so
  a workbook renders as en-US unless a custom number format carries such an LCID.
- Locale-aware number parsing exists as `parse_number_locale` and `parse_number_seps`
  (`sheet-format/src/number.rs:63-74`), but only tests call them. Cell entry and implicit
  coercion are locale-independent (`registry/features/locale.yaml:104-108`).

## Related

- [ADR 500](500-own-calculation-engine.md), [ADR 502](502-recalculation-and-spill.md) — the engine that evaluates the AST; the graph that uses the extracted references
- [ADR 317](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/317-registry-driven-dispatch.md) — the generated function table the parser resolves names against
- [ADR 503](503-xlsx-patched-not-regenerated.md), [ADR 507](507-golden-corpora-coverage-gate.md) — formula text on save; the locale goldens
