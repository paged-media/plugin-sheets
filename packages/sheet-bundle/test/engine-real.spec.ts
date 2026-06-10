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

import {
  chartGeometryToMutations,
  makeBinding,
  type ChartGeometry,
} from "@paged-media/sheet-host-model";

const HERE = dirname(fileURLToPath(import.meta.url));
const BIN = join(HERE, "..", "bin");
const WASM = join(BIN, "sheet_js_bg.wasm");
const built = existsSync(WASM);
const CHART_XLSX = join(HERE, "..", "..", "..", "corpus/xlsx-corpus/09-chart.xlsx");

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

  // Wave 2D (S-05): paginate a tall range across a 2-frame chain end-to-end
  // across the real wasm door — the Rust engine threads the rows and serialises
  // a Vec<Page> the TS facade reads (camelCase frameIndex/continued).
  it("paginates a range across a frame chain (Wave 2D, S-05)", async () => {
    const glue = await import(/* @vite-ignore */ join(BIN, "sheet_js.js"));
    glue.initSync({ module: readFileSync(WASM) });
    const e = new glue.SheetEngine();
    for (let r = 0; r < 6; r++) e.set_cell(0, r, 0, `r${r}`);

    // Two 45pt frames (= 3 rows of 15pt each) → the 6 rows split 3/3.
    const pages = e.paginate(
      0,
      "A1:A6",
      [
        { widthPt: 200, heightPt: 45 },
        { widthPt: 200, heightPt: 45 },
      ],
      { continuedMarker: true },
    ) as Array<{
      frameIndex: number;
      continued: boolean;
      content: { rows: { cells: { text: string }[] }[] };
    }>;

    expect(pages.length).toBe(2);
    expect(pages[0].frameIndex).toBe(0);
    expect(pages[1].frameIndex).toBe(1);
    expect(pages[0].continued).toBe(true); // more body rows follow
    expect(pages[1].continued).toBe(false);
    expect(pages[0].content.rows[0].cells[0].text).toBe("r0");
    expect(pages[1].content.rows[0].cells[0].text).toBe("r3");
    e.free();
  });

  // FREEZE AMENDMENT (audit finding 1): a full-sheet lowered range must throw a
  // JS error across the wasm door — NOT abort the allocator and poison the
  // wasm-bindgen borrow (which used to brick every later &mut call). The
  // session must stay usable after the rejection.
  it("rejects an oversize lowered range and stays usable (finding 1)", async () => {
    const glue = await import(/* @vite-ignore */ join(BIN, "sheet_js.js"));
    glue.initSync({ module: readFileSync(WASM) });
    const e = new glue.SheetEngine();

    // The full-sheet range A1:XFD1048576 (~1.7e10 cells) throws, it does not
    // crash the module.
    expect(() => e.get_range_lowered(0, "A1:XFD1048576", {})).toThrow(/T0 lowering cap/);

    // The session is NOT poisoned: a following &mut call still works.
    e.set_cell(0, 0, 0, "42");
    expect(e.get_cell_display(0, 0, 0)).toBe("42");

    // A range exactly at the cap is still accepted end-to-end.
    const atCap = e.get_range_lowered(0, "A1:A1048576", {});
    expect(atCap.rows.length).toBe(1_048_576);
    expect(atCap.rows[0].cells[0].text).toBe("42");
    e.free();
  });

  // FREEZE AMENDMENT (audit finding 2): set_cell with an out-of-range sheet id
  // must throw a JS error and must NOT auto-create a phantom sheet (whose data
  // would silently drop on save). The workbook stays one sheet, clean.
  it("rejects an out-of-range sheet id without phantom sheets (finding 2)", async () => {
    const glue = await import(/* @vite-ignore */ join(BIN, "sheet_js.js"));
    glue.initSync({ module: readFileSync(WASM) });
    const e = new glue.SheetEngine();

    expect(e.list_sheets().length).toBe(1);

    // set_cell(5, ...) on a 1-sheet workbook throws across the door.
    expect(() => e.set_cell(5, 0, 0, "boom")).toThrow(/out of range/);

    // No phantom sheet was created; the workbook stays clean (not dirty).
    expect(e.list_sheets().length).toBe(1);
    expect(e.metadata().dirty).toBe(false);

    // get_cell_display on the OOB sheet returns "" by contract, creating
    // nothing.
    expect(e.get_cell_display(5, 0, 0)).toBe("");
    expect(e.list_sheets().length).toBe(1);
    e.free();
  });

  // FINDING 1 end-to-end smoke: get_chart_geometry on the real 09-chart
  // workbook, lower it through chartGeometryToMutations, and prove the chart's
  // FILL/STROKE colours now ride out as document swatches + frame colour refs
  // (pre-fix the whole palette was silently dropped).
  it("lowers a real chart's palette to swatches + frame colour refs (finding 1)", async () => {
    const glue = await import(/* @vite-ignore */ join(BIN, "sheet_js.js"));
    glue.initSync({ module: readFileSync(WASM) });
    const e = new glue.SheetEngine();
    e.load_xlsx(readFileSync(CHART_XLSX));

    const charts = e.list_charts() as Array<{ index: number }>;
    expect(charts.length).toBeGreaterThan(0);

    const geom = e.get_chart_geometry(charts[0].index, 300, 200) as ChartGeometry;
    // The geometry carries colour-bearing primitives (the bug was that these
    // colours never reached the host mutations).
    const colourBearing = geom.prims.filter(
      (p) =>
        (p.kind === "rect" && p.fill) ||
        (p.kind === "wedge" && p.fill) ||
        (p.kind === "polygon" && p.fill) ||
        (p.kind === "line" && p.stroke),
    );
    expect(colourBearing.length).toBeGreaterThan(0);

    const { batch } = chartGeometryToMutations(
      geom,
      { pageId: "Page/u1", bounds: [0, 0, 200, 300] },
      makeBinding("Sheet1", "A1:B5", 1),
    );
    const ops = (batch as { args: { ops: Array<{ op: string; args: Record<string, unknown> }> } })
      .args.ops;

    // At least one createSwatch (the palette becomes real document swatches).
    const swatches = ops.filter((o) => o.op === "createSwatch");
    expect(swatches.length).toBeGreaterThan(0);
    // Each swatch is an RGB swatch with 0..255 channels.
    for (const s of swatches) {
      const spec = s.args.spec as { space: string; value: number[] };
      expect(spec.space).toBe("RGB");
      expect(spec.value).toHaveLength(3);
      for (const ch of spec.value) {
        expect(ch).toBeGreaterThanOrEqual(0);
        expect(ch).toBeLessThanOrEqual(255);
      }
    }

    // At least one frameFillColor or frameStrokeColor ref, pointing at a minted
    // swatch id (NOT a default-coloured path — the finding-1 fix).
    const colourRefs = ops.filter(
      (o) =>
        o.op === "setElementProperty" &&
        ((o.args.path as string) === "frameFillColor" ||
          (o.args.path as string) === "frameStrokeColor"),
    );
    expect(colourRefs.length).toBeGreaterThan(0);
    const swatchIds = new Set(
      swatches.map((s) => (s.args.spec as { selfId: string }).selfId),
    );
    for (const c of colourRefs) {
      const v = c.args.value as { type: string; value: string };
      expect(v.type).toBe("colorRef");
      expect(swatchIds.has(v.value)).toBe(true);
    }

    e.free();
  });
});
