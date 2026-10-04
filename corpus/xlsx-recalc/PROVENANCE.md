# `corpus/xlsx-recalc/` — recalc fixtures written by Excel

28 workbooks, ~0.6 MB, read by
`sheet-conformance/tests/xlsx_recalc_corpus.rs` (the CI half of the
recalc lane): every formula is recomputed by the engine and compared with
the value Excel cached in the file.

**Source.** Apache POI's spreadsheet test corpus
(https://github.com/apache/poi `test-data/spreadsheet`, commit
`29c9cacc354613eedf6c7ee24007a597f64c1951`), legacy `.xls`/`.xlsb` files
re-saved as `.xlsx` by desktop Microsoft Excel 16.112 on macOS
(2026-08-20, the private corpus's `harness/convert-office.sh`). Copied
byte-for-byte from the private corpus's `xlsx/poi-converted/`; names keep
the whole source filename (`Simple.xls.xlsx`).

**Licence.** Apache License 2.0, inherited from POI — `LICENSE.apache-2.0`
and `NOTICE` in this directory are POI's own. Redistributable.

**Selection — by content, not by name.** Each file is a ZIP that opens and
carries formula cells with cached values; chosen for POI's
function-test workbooks (lookup, index, match, IF, boolean, IRR/NPV,
roman, fixed, D-functions, TREND, rank, countif, matrix formulas, date
coercion), for the shared-formula cases (`shared_formulas`,
`SharedFormulaTest`, `overlapSharedFormula`, `ex47747-sharedFormula`) and
one large mixed workbook (`54686_fraction_formats`, 4k formulas).

**What is pinned.** `expected.tsv` holds, per file, how many formula cells
agree with Excel, differ, fail to parse, are volatile, or carry no cached
value. Any change in a count fails the lane — refresh the table in the
same commit as the fix (or regression) that moved it.
