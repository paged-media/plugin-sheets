/*
 * This file is part of paged (https://paged.media).
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 * paged is distributed in the hope that it will be useful, but WITHOUT ANY
 * WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
 * FOR A PARTICULAR PURPOSE. See the licenses for details.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

// [sheet.plugin.bundle] Is the wasm these specs execute the one the Rust
// sources build?
//
// Every spec that boots the engine runs bin/sheet_js_bg.wasm, which is
// build output (gitignored) — whatever scripts/build-wasm.sh last left
// there. Until this spec a Rust change not followed by a rebuild left the
// suite testing the OLD engine and passing: on 2026-10-04 the local wasm
// was a build from 2026-08-05. The build script stamps the source hash
// into the wasm (`engine_source_hash()`) and beside it (bin/SOURCE_HASH);
// this recomputes the hash from the checkout and compares all three.
//
// Same dual gate as engine-real.spec.ts: no artifact at all skips locally
// (the pure-TS lane needs no Rust toolchain) and FAILS under
// REQUIRE_REAL_ENGINE=1. A PRESENT artifact is always checked — a stale
// one is never silently tested.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

// @ts-expect-error — a plain .mjs script with no type declarations.
import { sourceHash } from "../../../scripts/source-hash.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const BIN = join(HERE, "..", "bin");
const WASM = join(BIN, "sheet_js_bg.wasm");
const STAMP = join(BIN, "SOURCE_HASH");
const present = existsSync(WASM);
const required = process.env.REQUIRE_REAL_ENGINE === "1";

const REBUILD = "rebuild it with `bash scripts/build-wasm.sh`";

describe.skipIf(!present && !required)(
  "the engine wasm in bin/ matches the Rust sources [sheet.plugin.bundle]",
  () => {
    it("the wasm and its SOURCE_HASH are present", () => {
      expect(existsSync(WASM), `${WASM} missing — ${REBUILD}`).toBe(true);
      expect(existsSync(STAMP), `${STAMP} missing — ${REBUILD}`).toBe(true);
    });

    it("SOURCE_HASH equals the hash of the checkout's sources", () => {
      const stamped = existsSync(STAMP) ? readFileSync(STAMP, "utf8").trim() : "(none)";
      expect(
        stamped,
        `the wasm was built from different sources than this checkout — ${REBUILD}`,
      ).toBe(sourceHash());
    });

    it("the wasm itself carries the same hash (the stamp file was not edited by hand)", async () => {
      const glue = (await import(/* @vite-ignore */ join(BIN, "sheet_js.js"))) as {
        initSync(o: { module: Buffer }): unknown;
        engine_source_hash?: () => string;
      };
      glue.initSync({ module: readFileSync(WASM) });
      expect(
        typeof glue.engine_source_hash,
        `the wasm exports no engine_source_hash — it predates the stamp; ${REBUILD}`,
      ).toBe("function");
      expect(glue.engine_source_hash!()).toBe(readFileSync(STAMP, "utf8").trim());
    });
  },
);
