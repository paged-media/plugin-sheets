// sheet.plugin.session.restore (S-08) — the activate-time restore is a
// cheap, safe no-op when nothing is persisted or no blob store is wired:
// it must NOT boot the engine in those cases (the guard that keeps
// startup cost zero when there's nothing to restore). The full persist →
// reload round-trip rides the engine-real wasm lane (needs the artifact).

import { describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { createWorkbookSession } from "../src/session";

function fakeHost(opts: {
  blobSupported: boolean;
  partsSupported?: boolean;
  read?: () => Promise<Uint8Array | null>;
  partRead?: (path: string) => Promise<Uint8Array | null>;
}) {
  const read = vi.fn(opts.read ?? (async () => null));
  const partRead = vi.fn(opts.partRead ?? (async () => null));
  const partWrite = vi.fn(async () => {});
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) =>
      (f === "storage.blob@1" && opts.blobSupported) ||
      (f === "storage.parts@1" && (opts.partsSupported ?? false)),
    parts: {
      read: partRead,
      write: partWrite,
      list: vi.fn(async () => []),
    },
    blob: {
      read,
      write: vi.fn(async () => {}),
      delete: vi.fn(async () => {}),
      keys: vi.fn(async () => []),
      usage: vi.fn(async () => ({ used: 0, quota: 0 })),
    },
    storage: {
      get: () => undefined,
      set() {},
      delete() {},
      keys: () => [],
    },
  } as unknown as BundleHost;
  return { host, read, partRead, partWrite };
}

describe("sheet_plugin_session_restore", () => {
  it("returns false without reading the blob when no store is wired", async () => {
    const { host, read } = fakeHost({ blobSupported: false });
    const session = createWorkbookSession(host);
    expect(await session.restore()).toBe(false);
    expect(read).not.toHaveBeenCalled();
    // No engine was booted (no workbook in state).
    expect(session.state().engine).toBeNull();
    expect(session.state().bootError).toBeNull();
  });

  it("reads the blob but returns false (no engine boot) when nothing persisted", async () => {
    const { host, read } = fakeHost({ blobSupported: true });
    const session = createWorkbookSession(host);
    expect(await session.restore()).toBe(false);
    expect(read).toHaveBeenCalledOnce();
    // Empty store → no bytes → engine stays unbooted (cheap no-op).
    expect(session.state().engine).toBeNull();
    expect(session.state().bootError).toBeNull();
  });

  it("prefers the .paged container part over the per-browser blob", async () => {
    // A workbook part present → restore reads it and NEVER touches the blob.
    // The blob being un-consulted is the proof of preference: had the part read
    // returned null, restore would have fallen through and read the blob. (The
    // bytes here are a stub, so the engine load itself is the engine-real
    // lane's concern, not this routing test's.)
    const { host, read, partRead } = fakeHost({
      blobSupported: true,
      partsSupported: true,
      partRead: async (path) =>
        path === "workbook.xlsx" ? new Uint8Array([0x50, 0x4b]) : null,
    });
    const session = createWorkbookSession(host);
    await session.restore();
    expect(partRead).toHaveBeenCalled();
    expect(read).not.toHaveBeenCalled(); // blob never consulted → the part won
  });

  it("ignores the part door when no container part is present (blob fallback intact)", async () => {
    // parts wired but empty → restore consults the part, finds nothing, then
    // falls back to the blob (also empty here) → the cheap no-op holds.
    const { host, read, partRead } = fakeHost({
      blobSupported: true,
      partsSupported: true,
    });
    const session = createWorkbookSession(host);
    expect(await session.restore()).toBe(false);
    expect(partRead).toHaveBeenCalled();
    expect(read).toHaveBeenCalledOnce(); // fell back to the blob
    expect(session.state().engine).toBeNull();
  });
});
