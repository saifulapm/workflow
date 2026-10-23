#!/usr/bin/env bash
# The built binaries a cloud session's setup fetches, under `cf dev`:
#
#   - `hub share put-bin mem-x86_64 <file>` stores a 3 MB file, and a GET of
#     /store/dl/mem-x86_64 with the bearer gives it back byte for byte;
#   - without a bearer it is 401, a bad name 400, a missing name 404, and a
#     DELETE 405.
#
# A cloud session takes its builds from the public cloud-bins release now,
# which workflow/tests/cloud_setup.rs checks; the store keeps this route for
# the sessions whose setup predates that.
#
# Needs what t143 needs: hubweb/node_modules, worker-build, the wasm target,
# python3 and the hub's release binary, which tests/run.sh builds.

real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
HUB_BIN=${HUB_BIN:-$WF_ROOT/hub/target/release/hub}
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
	exit 1
fi
if [ ! -x "$HUB_BIN" ]; then
	notok "the hub binary is built" "missing $HUB_BIN"
	exit 1
fi

pids=()
stop_all() {
	local status=$?
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	wait 2>/dev/null
	(exit "$status")
	t_done
}
trap stop_all EXIT

token=test-store-token
app=$T_TMP/hubweb
t_hubweb_copy "$app"
t_cf_dev "$app" "$T_TMP/dev.log" STORE_TOKEN=$token
up=$?
pids+=("$CF_DEV_PID")
base=http://127.0.0.1:$CF_DEV_PORT
if [ "$up" != 0 ]; then
	notok "cf dev answers" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
store="ws://127.0.0.1:$CF_DEV_PORT/store/share"

head -c 3145728 /dev/urandom >"$T_TMP/mem.bin"
out=$(env -u MEM_REMOTE HUB_STORE_URL="$store" HUB_STORE_TOKEN=$token \
	timeout 60 "$HUB_BIN" share put-bin mem-x86_64 "$T_TMP/mem.bin" 2>&1)
is "$?" 0 "hub share put-bin exits 0"
is "$out" "stored mem-x86_64: 3145728 bytes" "and prints the size it stored"

status() { # status <path> [curl args...] -- prints the status of a request
	local path=$1
	shift
	curl -s -o /dev/null -w '%{http_code}' --max-time 60 "$@" "$base$path"
}
is "$(curl -s -o "$T_TMP/mem.back" -w '%{http_code}' --max-time 60 \
	-H "Authorization: Bearer $token" "$base/store/dl/mem-x86_64")" 200 "a GET with the bearer is 200"
if cmp -s "$T_TMP/mem.bin" "$T_TMP/mem.back"; then
	ok "and gives back the same bytes"
else
	notok "and gives back the same bytes" "$(stat -c %s "$T_TMP/mem.back") bytes back"
fi
is "$(status /store/dl/mem-x86_64)" 401 "a GET without a bearer is 401"
is "$(status /store/dl/mem-x86_64 -H "Authorization: Bearer wrong")" 401 "a wrong bearer is 401"
is "$(status /store/dl/Mem -H "Authorization: Bearer $token")" 400 "a bad name is 400"
is "$(status /store/dl/hub-x86_64 -H "Authorization: Bearer $token")" 404 "a name never stored is 404"
is "$(status /store/dl/mem-x86_64 -X DELETE -H "Authorization: Bearer $token")" 405 "a DELETE is 405"
