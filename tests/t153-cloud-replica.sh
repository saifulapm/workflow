#!/usr/bin/env bash
# A cloud session's replica under `cf dev`: the mini shares a walk project
# through its local hub, and a cloud machine that starts with nothing fills
# its replica from the store with `hub share sync`.
#
#   - an empty replica gets every shared project, handoff included;
#   - cut off from the store, `hub share sync` fails at once and the replica
#     it filled before still answers.
#
# Needs what t142 needs: hubweb/node_modules, worker-build, the wasm target,
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

free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

pids=()
stop_all() {
	local status=$?
	[ -n "${hub_pid:-}" ] && kill "$hub_pid" 2>/dev/null
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
if [ "$up" != 0 ]; then
	notok "cf dev answers" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
store="ws://127.0.0.1:$CF_DEV_PORT/store/share"

PROJECT=01K0CL0UDWA7K0000000000000

# on <machine> <command...> -- runs a command with that machine's replica.
on() {
	local root=$T_TMP/m/$1
	shift
	env HOME="$root" XDG_DATA_HOME="$root/data" XDG_CACHE_HOME="$root/cache" \
		XDG_STATE_HOME="$root/state" XDG_CONFIG_HOME="$root/config" \
		HUB_LOCAL_SOCKET="$root/state/hub/local.sock" MEM_SYNC_CMD=true "$@"
}
for m in mini cloud; do
	mkdir -p "$T_TMP/m/$m"/{data,cache,state,config/qshell}
	printf '%s\n' "$m" >"$T_TMP/m/$m/config/qshell/machine"
done
mkdir -p "$T_TMP/m/mini/data/mem/store/projects/$PROJECT/items"
cat >"$T_TMP/m/mini/data/mem/store/projects/$PROJECT/project.toml" <<TOML
created = "2026-10-16T00:00:00Z"
id = "$PROJECT"
name = "walk"
share = "true"
TOML

on mini env HUB_PEER_TOKEN=peer-token HUB_PEERS= HUB_STORE_URL="$store" HUB_STORE_TOKEN=$token \
	"$HUB_BIN" local --port "$(free_port)" >>"$T_TMP/mini.hub.log" 2>&1 &
hub_pid=$!
# The project.toml went in before sharing, so it is written again to reach
# the store; then a handoff and a rule.
on mini mem --project walk project set verify "true" >/dev/null
on mini mem --project walk handoff --set "Next: m2. m1 landed in the walk." >/dev/null
on mini mem --project walk decide "walk rule from the mini" --by agent >/dev/null
drained() { on mini mem doctor | grep -qE '^sharing +seq [0-9]+, outbox 0$'; }
for _ in $(seq 1 100); do drained && break; sleep 0.1; done
if drained; then
	ok "the mini's writes reach the store"
else
	notok "the mini's writes reach the store" "$(on mini mem doctor; cat "$T_TMP/mini.hub.log")"
	exit 1
fi

sync() { on cloud env HUB_STORE_URL="$1" HUB_STORE_TOKEN=$token timeout 30 "$HUB_BIN" share sync; }
if out=$(sync "$store" 2>&1); then
	ok "hub share sync fills an empty replica and ends"
else
	notok "hub share sync fills an empty replica and ends" "$out"
fi
handoff=$(on mini mem --project walk handoff 2>&1)
like "$handoff" "Next: m2. m1 landed in the walk." "the mini reads its handoff"
is "$(on cloud mem --project walk handoff 2>&1)" "$handoff" "the cloud's replica holds the mini's handoff"
like "$(on cloud mem --project walk context 2>&1)" "walk rule from the mini" \
	"the cloud's mem context lists the mini's rule"
like "$(on cloud mem doctor 2>&1)" "sharing +seq [1-9]" "the cloud's replica remembers the store's sequence"

# Cut off: nothing listens on the port.
dead="ws://127.0.0.1:$(free_port)/store/share"
start=$(date +%s)
if out=$(sync "$dead" 2>&1); then
	notok "hub share sync fails when the store cannot be reached" "$out"
else
	ok "hub share sync fails when the store cannot be reached"
fi
is "$(($(date +%s) - start < 5))" 1 "and says so within 5 s"
is "$(on cloud mem --project walk handoff 2>&1)" "$handoff" \
	"the replica filled before still answers while the store is cut off"
