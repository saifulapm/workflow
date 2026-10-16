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
      // Each shared project's evidence and raw bytes, one object per project.
      Files: exports.durableObject({ storage: "sqlite" }),
    },
    env: {
      STORE: bindings.durableObject({ worker: "hubweb", exportName: "Store" }),
      FILES: bindings.durableObject({ worker: "hubweb", exportName: "Files" }),
      // Every evidence and raw file, and the built binaries, each one whole
      // object keyed by its store path. Only the Worker holds it.
      FILES_BUCKET: bindings.r2({ name: "hubweb-files" }),
      STORE_TOKEN: bindings.secret(),
      // The Access apps' auds, comma-separated, and the team URL, such as
      // https://saifulapm.cloudflareaccess.com. Neither is secret; they are
      // secrets because cf dev reads secrets from its environment.
      ACCESS_AUD: bindings.secret(),
      ACCESS_TEAM: bindings.secret(),
      // How old a machine's newest report may be, in ms, and it still counts
      // as online. Unset, it is 45000: an idle machine reports every 30 s.
      HUB_AGENT_WINDOW_MS: bindings.secret(),
      // Host names, comma-separated, that answer 404 to everything: the
      // next- names once the switch moved the hub web and the connector.
      RETIRED_HOSTS: bindings.secret(),
    },
    // The hub web and the connector on their own names until the switch:
    // a deploy with HUBWEB_DOMAINS=real moves hub.on-rexa.com and
    // mem.on-rexa.com here, keeping the next- names attached for
    // RETIRED_HOSTS to answer.
    domains:
      process.env.HUBWEB_DOMAINS === "real"
        ? ["hub.on-rexa.com", "mem.on-rexa.com", "next-hub.on-rexa.com", "next-mem.on-rexa.com"]
        : ["next-hub.on-rexa.com", "next-mem.on-rexa.com"],
  },
});
