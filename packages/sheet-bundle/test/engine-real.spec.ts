// The REAL engine boot (sheet.plugin.engine.boot): boots the actual
// wasm-bindgen artifact in Node via the same glue path engine.ts uses
// (initSync over bytes — the S-10 bundle-realm pattern) and drives one
// full entry -> recalc -> lower -> save -> reload loop.
//
// Artifact-gated: when packages/sheet-bundle/bin/sheet_js_bg.wasm has not
// been built (scripts/build-wasm.sh), the suite SKIPS — the pure-TS vitest
// lane stays green without a Rust toolchain. CI's rust lane builds the
// artifact, so the real boot is exercised there.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const BIN = join(dirname(fileURLToPath(import.meta.url)), "..", "bin");
const WASM = join(BIN, "sheet_js_bg.wasm");
const built = existsSync(WASM);

describe.skipIf(!built)("real engine boot (wasm artifact)", () => {
  it("boots, calculates, lowers, and round-trips xlsx", async () => {
    const glue = await import(/* @vite-ignore */ join(BIN, "sheet_js.js"));
    glue.initSync({ module: readFileSync(WASM) });

    const e = new glue.SheetEngine();
    e.set_cell(0, 0, 0, "100");
    e.set_cell(0, 1, 0, "250.5");
    e.set_cell(0, 2, 0, "=SUM(A1:A2)*0.19");
    expect(e.get_cell_display(0, 2, 0)).toBe("66.595");

    // Editing a precedent recalculates the dependent chain.
    const r = e.set_cell(0, 0, 0, "1000");
    const changed = r.changed.map((c: { row: number; display: string }) => [c.row, c.display]);
    expect(changed).toContainEqual([2, "237.595"]);

    // Lowered IR carries the formatted text (the §8.3 contract).
    const low = e.get_range_lowered(0, "A1:A3", { includeGridRules: true });
    expect(low.rows.map((row: { cells: { text: string }[] }) => row.cells[0].text)).toEqual([
      "1000",
      "250.5",
      "237.595",
    ]);
    expect(low.rules.h.length).toBeGreaterThan(0);

    // Save -> reload: formulas and values survive.
    const saved = e.save_xlsx();
    const e2 = new glue.SheetEngine();
    e2.load_xlsx(saved);
    expect(e2.get_cell_display(0, 2, 0)).toBe("237.595");
    e.free();
    e2.free();
  });
});
