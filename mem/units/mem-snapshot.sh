#!/usr/bin/env bash
# Puts a dated `mem snapshot` tarball in Dropbox, then lets the store on
# Cloudflare drop each file over 90 days old that the tarball holds. The
# store goes only after Dropbox took the tarball: set -e stops the run at the
# first failed step.
set -euo pipefail

tarball=$(mem snapshot)
rclone copy "$tarball" Dropbox:mem-snapshots/
hub share archive "$tarball"
