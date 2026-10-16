// cloudflare.config.ts holds no build settings, so wrangler reads the Rust
// build from here: cf dev and cf build run worker-build before bundling.
import { defineWranglerConfig } from "wrangler/experimental-config";

export default defineWranglerConfig({
  // The fonts the hub's pages load, served from Cloudflare's cache before
  // the Worker runs; hubpages keeps them for the local hub too.
  assetsDirectory: "../hubpages/public",
  build: {
    command: "worker-build --release",
    watchDir: "src",
  },
});
