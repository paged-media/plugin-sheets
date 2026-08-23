/**
 * paged.sheet — the menu entries name real commands.
 *
 * `contribute.menu()` REFUSES an entry whose command the bundle has not
 * registered, and that refusal happens at activate time in a browser —
 * so a mistyped suffix would surface as one quietly missing menu item
 * and nowhere else. Comparing the table against the manifest catches it
 * in CI instead, and catches the reverse drift too: a command renamed
 * while the table still points at the old id.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { describe, expect, it } from "vitest";

import { MENU_COMMAND_PREFIX, MENU_ENTRIES } from "../src/menu";

const HERE = dirname(fileURLToPath(import.meta.url));
const MANIFEST = join(HERE, "..", "manifest.json");

const declared: string[] = JSON.parse(readFileSync(MANIFEST, "utf8"))
  .contributes.commands;

describe("sheet menu entries", () => {
  it("every entry points at a command the manifest declares", () => {
    const set = new Set(declared);
    const missing = MENU_ENTRIES.map(
      ([, s]) => `${MENU_COMMAND_PREFIX}.${s}`,
    ).filter((id) => !set.has(id));
    expect(missing, `unknown commands: ${missing.join(", ")}`).toEqual([]);
  });

  it("no command and no path appears twice", () => {
    const suffixes = MENU_ENTRIES.map(([, s]) => s);
    expect(suffixes.length).toBe(new Set(suffixes).size);
    const paths = MENU_ENTRIES.map(([p]) => p);
    expect(paths.length).toBe(new Set(paths).size);
  });

  it("only this plugin's top level and host menus are used", () => {
    const ALLOWED = ["Sheet", "Object"];
    const stray = MENU_ENTRIES.map(([p]) => p.split("/")[0]).filter(
      (t) => !ALLOWED.includes(t),
    );
    expect(stray, `unexpected top-level menus: ${stray.join(", ")}`).toEqual([]);
  });

  it("is not empty", () => {
    // A floor: with an empty table every assertion above passes
    // vacuously — an empty list names no unknown command and has no
    // duplicates.
    expect(MENU_ENTRIES.length).toBeGreaterThanOrEqual(10);
    expect(MENU_ENTRIES.length).toBeLessThanOrEqual(declared.length);
  });
});
