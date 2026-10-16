// cf dev, cf build and cf deploy read this file. cf has no Rust dev server
// yet, so it hands the Worker to the wrangler in package.json, and the
// build step that runs worker-build lives in wrangler.config.ts.
import { bindings, defineConfig, exports } from "cf/config";

export default defineConfig({
  worker: {
    name: "hubweb",
    // worker-build's output: a JS module that loads build/index_bg.wasm.
    entrypoint: "./build/index.js",
    compatibilityDate: "2026-09-01",
    exports: {
      Store: exports.durableObject({ storage: "sqlite" }),
    },
    env: {
      STORE: bindings.durableObject({ worker: "hubweb", exportName: "Store" }),
      STORE_TOKEN: bindings.secret(),
    },
    // next-hub.on-rexa.com and next-mem.on-rexa.com are added when the
    // Worker is first deployed, not before.
  },
});
