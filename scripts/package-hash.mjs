#!/usr/bin/env node
// The version-bump guard for @paged-media/sheet.
//
// publish.yml skips a version that is already on npm. Before this guard
// that made "changed the code, forgot the bump" a silent no-op: the ADR 031
// fix (71f37d7) sat unshipped behind 0.1.0-canary.8 because the publish run
// was green and shipped nothing. Now every published tarball carries
// bin/PACKAGE_HASH — the hash of everything the package is built from —
// and the check compares it with the checkout:
//
//   version not on npm yet               → ok (a new version; publish it)
//   on npm, same PACKAGE_HASH            → ok (nothing changed; skip)
//   on npm, different or missing hash    → FAIL: bump the version
//
// Inputs: the engine wasm's source hash (scripts/source-hash.mjs), the
// bundle's src/, manifest.json, build config and package.json (minus its
// "version"), the inlined sheet-host-model's src/ and package.json (minus
// "version"), tsconfig.base.json. Tests are not inputs.
//
// Usage:
//   node scripts/package-hash.mjs             → print the hash
//   node scripts/package-hash.mjs --stamp     → write packages/sheet-bundle/bin/PACKAGE_HASH
//   node scripts/package-hash.mjs --check     → compare with the published version (npm)
//   node scripts/package-hash.mjs --check --against <file.tgz>
//                                             → compare with a local tarball (for tests)

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { sourceHash } from "./source-hash.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const BUNDLE = "packages/sheet-bundle";
const HOST_MODEL = "packages/sheet-host-model";
const STAMP = join(root, BUNDLE, "bin", "PACKAGE_HASH");

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

/** package.json with the version removed — a bump alone is not a change. */
function manifestWithoutVersion(rel) {
  const pkg = JSON.parse(readFileSync(join(root, rel), "utf8"));
  delete pkg.version;
  return JSON.stringify(pkg);
}

export function packageInputs() {
  const files = [
    ...filesUnder(join(root, BUNDLE, "src")),
    ...filesUnder(join(root, HOST_MODEL, "src")),
    ...["manifest.json", "tsup.config.ts", "tsconfig.json"].map((f) => join(root, BUNDLE, f)),
    join(root, "tsconfig.base.json"),
  ].filter((f) => existsSync(f));
  return files.map((f) => relative(root, f).split("\\").join("/")).sort();
}

export function packageHash() {
  const h = createHash("sha256");
  h.update(`engine-wasm\0${sourceHash()}\0`);
  for (const rel of [`${BUNDLE}/package.json`, `${HOST_MODEL}/package.json`]) {
    h.update(`${rel}\0${manifestWithoutVersion(rel)}\0`);
  }
  for (const rel of packageInputs()) {
    h.update(rel);
    h.update("\0");
    h.update(readFileSync(join(root, rel), "utf8").replace(/\r\n/g, "\n"));
    h.update("\0");
  }
  return h.digest("hex");
}

/** PACKAGE_HASH inside a tarball, or null when the tarball has none. */
function hashInTarball(tgz) {
  try {
    return execFileSync("tar", ["-xzOf", tgz, "package/bin/PACKAGE_HASH"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null;
  }
}

async function publishedTarball(name, version) {
  let url;
  try {
    url = execFileSync("npm", ["view", `${name}@${version}`, "dist.tarball"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    url = "";
  }
  if (!url) return null;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`could not download ${url}: HTTP ${res.status}`);
  const file = join(mkdtempSync(join(tmpdir(), "sheet-pkg-")), "published.tgz");
  writeFileSync(file, Buffer.from(await res.arrayBuffer()));
  return file;
}

async function check(against) {
  const pkg = JSON.parse(readFileSync(join(root, BUNDLE, "package.json"), "utf8"));
  const id = `${pkg.name}@${pkg.version}`;
  const tgz = against ?? (await publishedTarball(pkg.name, pkg.version));
  if (!tgz) {
    console.log(`ok: ${id} is not published yet — a new version`);
    return 0;
  }
  const published = hashInTarball(tgz);
  const local = packageHash();
  if (published === local) {
    console.log(`ok: ${id} is published from these exact sources (PACKAGE_HASH ${local.slice(0, 12)})`);
    return 0;
  }
  console.error(
    `error: ${id} is already published, and the sources changed since ` +
      (published
        ? `(published PACKAGE_HASH ${published.slice(0, 12)}, checkout ${local.slice(0, 12)}).`
        : "(or it predates the PACKAGE_HASH stamp, so it cannot be shown otherwise)."),
  );
  console.error(
    `       The publish would skip it and ship nothing. Bump "version" in ${BUNDLE}/package.json.`,
  );
  return 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const args = process.argv.slice(2);
  if (args.includes("--stamp")) {
    writeFileSync(STAMP, `${packageHash()}\n`);
    console.log(`wrote ${relative(root, STAMP)}`);
  } else if (args.includes("--check")) {
    const i = args.indexOf("--against");
    process.exitCode = await check(i >= 0 ? args[i + 1] : undefined);
  } else {
    console.log(packageHash());
  }
}
