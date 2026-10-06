// The manifest's `contributes.objectModel` is what plugin-cli validates
// and what cockpit's object matrix reads (ADR 323 rule 4): it must say
// exactly what the bundle registers at runtime. The schema rows live in
// `object-model/<kind>.schema.json` (bundle-relative); the typed commands'
// args inline. `UPDATE_OBJECT_MODEL=1 vitest run object-model-manifest`
// regenerates both from the source.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import type { BundleHost, ObjectModelContribution } from "@paged-media/plugin-api";

import { contributeObjectModel, SHEET_KINDS, SHEET_SCHEMAS } from "../src/object-model";
import type { WorkbookSession } from "../src/session";

const HERE = dirname(fileURLToPath(import.meta.url));
const PKG = join(HERE, "..");
const MANIFEST = join(PKG, "manifest.json");

/** The model the bundle registers, captured without an engine. */
function registeredModel(): ObjectModelContribution {
  let model: ObjectModelContribution | null = null;
  const host = {
    contribute: {
      objectModel(m: ObjectModelContribution) {
        model = m;
        return { dispose() {}, changed() {} };
      },
    },
    document: {},
    log: { debug() {}, info() {}, warn() {}, error() {} },
  } as unknown as BundleHost;
  const session = {
    objectBridge: () => ({
      engine: () => null,
      epoch: () => 0,
      afterWrite() {},
      loadVersion: () => false,
      setName() {},
      hostFrames: () => [],
      discoverHostFrames: async () => [],
    }),
    onDidChange: () => ({ dispose() {} }),
  } as unknown as WorkbookSession;
  contributeObjectModel(host, session).dispose();
  return model!;
}

function declaration() {
  const m = registeredModel();
  return {
    kinds: m.kinds.map((k) => ({ kind: k.kind, title: k.title, schema: `object-model/${k.kind}.schema.json` })),
    commands: (m.commands ?? []).map((c) => ({
      id: c.id,
      title: c.title,
      args: c.args,
      ...(c.result ? { result: c.result } : {}),
    })),
  };
}

describe("the manifest declares the object model the bundle registers [sheet.objects]", () => {
  if (process.env.UPDATE_OBJECT_MODEL === "1") {
    const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
    manifest.contributes.objectModel = declaration();
    writeFileSync(MANIFEST, JSON.stringify(manifest, null, 2) + "\n");
    mkdirSync(join(PKG, "object-model"), { recursive: true });
    for (const kind of SHEET_KINDS) {
      writeFileSync(
        join(PKG, "object-model", `${kind}.schema.json`),
        JSON.stringify(SHEET_SCHEMAS[kind], null, 2) + "\n",
      );
    }
  }

  const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));

  it("kinds and typed commands match", () => {
    expect(manifest.contributes.objectModel).toEqual(declaration());
  });

  it("every schema file holds the runtime rows", () => {
    for (const kind of SHEET_KINDS) {
      const rows = JSON.parse(readFileSync(join(PKG, "object-model", `${kind}.schema.json`), "utf8"));
      expect(rows).toEqual(JSON.parse(JSON.stringify(SHEET_SCHEMAS[kind])));
    }
  });

  it("each typed command shares its id with the palette command it types (the untyped list shrinks)", () => {
    const untyped = (manifest.contributes.commands as string[]).filter(
      (id) => !manifest.contributes.objectModel.commands.some((c: { id: string }) => c.id === id),
    );
    // Shrink-only: 24 untyped at the object-model baseline (2026-10-06).
    expect(untyped.sort()).toEqual([
      "media.paged.sheet.command.copySelection",
      "media.paged.sheet.command.findNext",
      "media.paged.sheet.command.hideGridInFrame",
      "media.paged.sheet.command.openGrid",
      "media.paged.sheet.command.paginateToChain",
      "media.paged.sheet.command.pasteSelection",
      "media.paged.sheet.command.showGridInFrame",
      "media.paged.sheet.command.styleFromCell",
    ]);
  });
});
