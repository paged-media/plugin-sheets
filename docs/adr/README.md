# Architecture decision records

An ADR records one load-bearing decision that has already been made: what was decided, what
in the code shows it, and what it obliges other code to do. It is a record, not a proposal.
When the code stops matching a record, the body is left as it is and a dated amendment is
added at the end.

ADR numbers are unique across the paged-media repositories, so a number names the same
record wherever it is cited. New records in this repository use 500–549. Record 016 predates
that scheme and keeps its number. Records 500–509 were written on 2026-10-02 from the code as
it stood, for decisions made earlier; their status says so.

| ADR | Title | Status |
|---|---|---|
| [016](016-chart-engine-plotters-chartgeometry.md) | The chart engine: plotters + a custom `DrawingBackend` → frozen `ChartGeometry` IR | Accepted (amended 2026-10-02) |
| [500](500-own-calculation-engine.md) | The calculation engine is own-built and clean-room | Accepted, recorded retroactively 2026-10-02 |
| [501](501-f64-numbers.md) | Numbers are f64 because Excel's are; exact decimal stays behind a seam | Accepted, recorded retroactively 2026-10-02 |
| [502](502-recalculation-and-spill.md) | Recalculation: a range-keyed graph, deterministic order, spill by fixpoint | Accepted, recorded retroactively 2026-10-02 |
| [503](503-xlsx-patched-not-regenerated.md) | XLSX is patched, never regenerated | Accepted, recorded retroactively 2026-10-02 |
| [504](504-publishing-first-scope.md) | Publishing-first scope: pivots, validation and macros are preserved and never interpreted | Accepted, recorded retroactively 2026-10-02 |
| [505](505-native-table-and-edit-grid.md) | A sheet on the page is a native table; editing happens in a scene-layer grid | Accepted, recorded retroactively 2026-10-02 |
| [506](506-workbook-in-a-container-part.md) | The workbook is stored whole in a container part | Accepted, recorded retroactively 2026-10-02 |
| [507](507-golden-corpora-coverage-gate.md) | Verification: authored goldens tied to the registry by a coverage gate | Accepted, recorded retroactively 2026-10-02 |
| [508](508-content-addressed-swatches.md) | Workbook colours become swatches at content-addressed ids | Accepted, recorded retroactively 2026-10-02 |
| [509](509-excel-first-dialect.md) | Excel-first formula dialect; locales are data | Accepted, recorded retroactively 2026-10-02 |

The measured comparison behind ADR 501 is [`DECIMAL-SPIKE.md`](../../DECIMAL-SPIKE.md) at the
repo root. Decisions made in other repositories that this plugin's code rests on are listed
in [`../README.md`](../README.md).
