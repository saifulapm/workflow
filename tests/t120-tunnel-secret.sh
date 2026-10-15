#!/usr/bin/env bash
# The tunnel's token stays in pass. A cloudflared token is base64 JSON
# that opens with {"a":", so every one starts eyJhIjoi; no tracked file
# may hold one, and the deploy reads it from pass, never from the tree.
source "$(dirname -- "$0")/lib.sh"
t_init

found=$(git -C "$WF_ROOT" grep -l 'eyJhIjoi' -- . ':!tests/t120-tunnel-secret.sh' || true)
is "$found" "" "no tracked file holds a cloudflared tunnel token"
like "$(cat "$WF_ROOT/vps/deploy")" 'pass show dulali/cloudflared-token' \
	"vps/deploy reads the tunnel token from pass"

t_done
