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
  read?: () => Promise<Uint8Array | null>;
}) {
  const read = vi.fn(opts.read ?? (async () => null));
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => f === "storage.blob@1" && opts.blobSupported,
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
  return { host, read };
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
});
