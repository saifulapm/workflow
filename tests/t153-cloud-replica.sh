#!/usr/bin/env bash
# A cloud session's replica under `cf dev`: the mini shares a walk project
# through its local hub, and a cloud machine that starts with nothing fills
# its replica from the store with `hub share sync`.
#
#   - an empty replica gets every shared project, handoff included;
#   - cut off from the store, `hub share sync` fails at once and the replica
#     it filled before still answers;
#   - the cloud's local hub reaches the store only through the VM's HTTP
#     proxy (CONNECT), and once the proxy comes back after a long cut, a
#     write the mini made meanwhile is listed in the cloud within 2 s.
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
	[ -n "${cloud_hub:-}" ] && kill "$cloud_hub" 2>/dev/null
	[ -n "${proxy_pid:-}" ] && kill "$proxy_pid" 2>/dev/null
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

# The VM's proxy: CONNECT <host:port>, then bytes both ways. Stopping it is
# the cloud losing Cloudflare.
cat >"$T_TMP/connect.py" <<'PY'
import asyncio, sys
async def pipe(r, w):
    try:
        while data := await r.read(65536):
            w.write(data)
            await w.drain()
    finally:
        w.close()
async def serve(r, w):
    head = await r.readuntil(b"\r\n\r\n")
    host, port = head.split(b" ")[1].decode().rsplit(":", 1)
    with open(sys.argv[2], "a") as log:
        log.write(head.split(b"\r\n")[0].decode() + "\n")
    # Only the proxy knows the store's name, as only the VM's proxy reaches
    # Cloudflare.
    tr, tw = await asyncio.open_connection("127.0.0.1" if host == "store.invalid" else host, int(port))
    w.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
    await w.drain()
    await asyncio.gather(pipe(r, tw), pipe(tr, w), return_exceptions=True)
async def main():
    server = await asyncio.start_server(serve, "127.0.0.1", int(sys.argv[1]), reuse_address=True)
    async with server:
        await server.serve_forever()
asyncio.run(main())
PY
proxy_port=$(free_port)
unblock() {
	python3 "$T_TMP/connect.py" "$proxy_port" "$T_TMP/connects.log" >>"$T_TMP/proxy.log" 2>&1 &
	proxy_pid=$!
	for _ in $(seq 1 50); do
		python3 -c "import socket; socket.create_connection(('127.0.0.1', $proxy_port), 0.2)" 2>/dev/null && return
		sleep 0.1
	done
}
block() { kill "$proxy_pid" 2>/dev/null; wait "$proxy_pid" 2>/dev/null; }
now_ms() { date +%s%3N; }
lists() { on cloud mem --project walk log --limit 50 | grep -qF "$1"; }

unblock
on cloud env HUB_PEER_TOKEN=peer-token HUB_PEERS= HUB_STORE_URL="ws://store.invalid:$CF_DEV_PORT/store/share" \
	HUB_STORE_TOKEN=$token HTTPS_PROXY="http://127.0.0.1:$proxy_port" "$HUB_BIN" local --port "$(free_port)" >>"$T_TMP/cloud.hub.log" 2>&1 &
cloud_hub=$!
on mini mem --project walk log "before the cut" >/dev/null
for _ in $(seq 1 40); do lists "before the cut" && break; sleep 0.1; done
if lists "before the cut"; then
	ok "the cloud's hub takes the mini's writes through the proxy"
else
	notok "the cloud's hub takes the mini's writes through the proxy" "$(cat "$T_TMP/cloud.hub.log" "$T_TMP/proxy.log")"
fi
like "$(cat "$T_TMP/connects.log" 2>/dev/null)" "^CONNECT store.invalid:$CF_DEV_PORT HTTP/1.1" \
	"the socket asked the proxy to CONNECT to the store"

block
on mini mem --project walk log "made during the cut" >/dev/null
# Long enough for a doubling wait to have grown past 2 s.
sleep 10
unblock
start=$(now_ms)
while ! lists "made during the cut" && [ $(($(now_ms) - start)) -le 2000 ]; do sleep 0.05; done
if lists "made during the cut"; then
	ok "a write made during the cut is listed within 2 s of the proxy coming back ($(($(now_ms) - start)) ms)"
else
	notok "a write made during the cut is listed within 2 s of the proxy coming back" "$(tail -20 "$T_TMP/cloud.hub.log")"
fi
