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
      // The Access apps' auds, comma-separated, and the team URL, such as
      // https://saifulapm.cloudflareaccess.com. Neither is secret; they are
      // secrets because cf dev reads secrets from its environment.
      ACCESS_AUD: bindings.secret(),
      ACCESS_TEAM: bindings.secret(),
    },
    // The hub web and the connector on their own names until the switch,
    // when hub.on-rexa.com and mem.on-rexa.com move here.
    domains: ["next-hub.on-rexa.com", "next-mem.on-rexa.com"],
  },
});
