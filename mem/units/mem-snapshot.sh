#!/usr/bin/env bash
# Puts a dated `mem snapshot` tarball in Dropbox: the weekly backup. set -e
# stops the run if the snapshot fails, so nothing is copied then.
set -euo pipefail

tarball=$(mem snapshot)
rclone copy "$tarball" Dropbox:mem-snapshots/
