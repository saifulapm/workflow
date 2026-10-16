// cloudflare.config.ts holds no build settings, so wrangler reads the Rust
// build from here: cf dev and cf build run worker-build before bundling.
import { defineWranglerConfig } from "wrangler/experimental-config";

export default defineWranglerConfig({
  build: {
    command: "worker-build --release",
    watchDir: "src",
  },
});
