#!/usr/bin/env node
// The hash of every source file the engine wasm (sheet-js) is built from.
//
// build-wasm.sh stamps it into the wasm (`engine_source_hash()`) and next
// to it (packages/sheet-bundle/bin/SOURCE_HASH);
// packages/sheet-bundle/test/wasm-fresh.spec.ts recomputes it and fails
// when the wasm in bin/ was built from different sources. Without that
// check every vitest run exercised whatever wasm was last built: on
// 2026-10-04 the local artifact was a build from 2026-08-05, two months
// of Rust changes older than the checkout, and nothing said so.
//
// Inputs: sheet-js and every crate it reaches through `path = "../…"`
// dependencies (src/, build.rs, Cargo.toml), the build-consumed
// registry/functions/*.yaml (sheet-core + sheet-fn build.rs read them),
// the root Cargo.toml + Cargo.lock, rust-toolchain.toml. Dev-dependencies,
// tests/ and benches/ are not inputs to the wasm and are left out, so
// editing a test does not demand a rebuild.
//
// Usage: node scripts/source-hash.mjs        → prints the hex digest
//        node scripts/source-hash.mjs --list → prints the input files

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

/** The normal (and build) `path = "../x"` dependencies of a crate. */
function pathDeps(crate) {
  const toml = readFileSync(join(root, crate, "Cargo.toml"), "utf8");
  const deps = [];
  let section = "";
  for (const line of toml.split("\n")) {
    const header = line.match(/^\s*\[([^\]]+)\]/);
    if (header) {
      section = header[1];
      continue;
    }
    if (/dev-dependencies/.test(section)) continue;
    if (!/dependencies/.test(section)) continue;
    const m = line.match(/path\s*=\s*"\.\.\/([^"]+)"/);
    if (m) deps.push(m[1]);
  }
  return deps;
}

function crates() {
  const seen = new Set();
  const walk = (c) => {
    if (seen.has(c)) return;
    seen.add(c);
    for (const d of pathDeps(c)) walk(d);
  };
  walk("sheet-js");
  return [...seen].sort();
}

function filesUnder(dir) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...filesUnder(p));
    else out.push(p);
  }
  return out;
}

export function inputs() {
  const files = [];
  for (const c of crates()) {
    files.push(...filesUnder(join(root, c, "src")));
    for (const f of ["build.rs", "Cargo.toml"]) {
      if (existsSync(join(root, c, f))) files.push(join(root, c, f));
    }
  }
  files.push(
    ...filesUnder(join(root, "registry", "functions")).filter((f) => f.endsWith(".yaml")),
  );
  for (const f of ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]) {
    files.push(join(root, f));
  }
  return files.map((f) => relative(root, f).split("\\").join("/")).sort();
}

export function sourceHash() {
  const h = createHash("sha256");
  for (const rel of inputs()) {
    // Path and content both count; CRLF is normalised so a Windows
    // checkout hashes the same as the one that built the wasm.
    h.update(rel);
    h.update("\0");
    h.update(readFileSync(join(root, rel), "utf8").replace(/\r\n/g, "\n"));
    h.update("\0");
  }
  return h.digest("hex");
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  if (process.argv.includes("--list")) console.log(inputs().join("\n"));
  else console.log(sourceHash());
}
