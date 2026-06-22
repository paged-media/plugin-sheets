import { defineConfig } from "tsup";

// Leave Vite-style `?url` asset imports (the wasm in ../bin) for the consuming
// bundler (the editor's Vite) instead of letting esbuild try to load .wasm.
export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm"],
  dts: true,
  clean: true,
  external: [/\?url$/],
});
