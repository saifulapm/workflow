#!/usr/bin/env bash
# Three machines share one project through the store under `cf dev`. Each
# machine is its own replica (an XDG tree), its own `hub local` holding the
# sharing socket, and a TCP proxy in front of the store that the test stops
# to cut that machine off from Cloudflare.
#
#   - a log written on one machine is listed on the other two within 2 s;
#   - a wiki page written on one reads the same, byte for byte, on another;
#   - writes made while cut off wait in the outbox, reads keep working, and
#     the outbox drains within 10 s of the proxy coming back;
#   - a machine that was off gets every write it missed;
#   - two offline edits of one page leave exactly one conflict file, on the
#     machine that reached the store second;
#   - two items written at once on two machines both arrive.
#
# Needs what t141 needs (hubweb/node_modules, worker-build, the wasm target,
# python3) and the hub's release binary, which tests/run.sh builds.

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
now_ms() { date +%s%3N; }

pids=()
declare -A hub_pid proxy_pid proxy_port hub_port
stop_all() {
	local status=$?
	for name in "${!hub_pid[@]}"; do kill "${hub_pid[$name]}" 2>/dev/null; done
	for name in "${!proxy_pid[@]}"; do kill "${proxy_pid[$name]}" 2>/dev/null; done
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
store_port=$CF_DEV_PORT
if [ "$up" != 0 ]; then
	notok "cf dev answers" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi

# A byte pipe to the store: stopping it is this machine losing Cloudflare.
cat >"$T_TMP/proxy.py" <<'PY'
import asyncio, sys
listen, target = int(sys.argv[1]), int(sys.argv[2])
async def pipe(r, w):
    try:
        while data := await r.read(65536):
            w.write(data)
            await w.drain()
    finally:
        w.close()
async def serve(r, w):
    tr, tw = await asyncio.open_connection("127.0.0.1", target)
    await asyncio.gather(pipe(r, tw), pipe(tr, w), return_exceptions=True)
async def main():
    server = await asyncio.start_server(serve, "127.0.0.1", listen, reuse_address=True)
    async with server:
        await server.serve_forever()
asyncio.run(main())
PY

PROJECT=01K0SHAREWA7K0000000000000

# on <machine> <command...> -- runs a command with that machine's replica.
on() {
	local name=$1 root=$T_TMP/m/$1
	shift
	env HOME="$root" XDG_DATA_HOME="$root/data" XDG_CACHE_HOME="$root/cache" \
		XDG_STATE_HOME="$root/state" XDG_CONFIG_HOME="$root/config" \
		HUB_LOCAL_SOCKET="$root/state/hub/local.sock" MEM_SYNC_CMD=true "$@"
}
walk() { local name=$1; shift; on "$name" mem --project walk "$@"; }

unblock() { # unblock <machine> -- (re)starts its proxy on the same port
	local name=$1
	python3 "$T_TMP/proxy.py" "${proxy_port[$name]}" "$store_port" >>"$T_TMP/$name.proxy.log" 2>&1 &
	proxy_pid[$name]=$!
	for _ in $(seq 1 50); do
		python3 -c "import socket; socket.create_connection(('127.0.0.1', ${proxy_port[$name]}), 0.2)" 2>/dev/null && return
		sleep 0.1
	done
}
block() { kill "${proxy_pid[$1]}" 2>/dev/null; wait "${proxy_pid[$1]}" 2>/dev/null; }

start_hub() { # start_hub <machine>
	local name=$1
	on "$name" env HUB_PEER_TOKEN=peer-token HUB_PEERS= \
		HUB_STORE_URL="ws://127.0.0.1:${proxy_port[$name]}/store/share" HUB_STORE_TOKEN=$token \
		"$HUB_BIN" local --port "${hub_port[$name]}" >>"$T_TMP/$name.hub.log" 2>&1 &
	hub_pid[$name]=$!
}
stop_hub() { kill "${hub_pid[$1]}" 2>/dev/null; wait "${hub_pid[$1]}" 2>/dev/null; }

machine() { # machine <name> -- a replica holding the shared project, its proxy and hub
	local name=$1 root=$T_TMP/m/$1
	mkdir -p "$root"/{data,cache,state,config/qshell} "$root/data/mem/store/projects/$PROJECT/items"
	printf '%s\n' "$name" >"$root/config/qshell/machine"
	cat >"$root/data/mem/store/projects/$PROJECT/project.toml" <<-TOML
		created = "2026-10-16T00:00:00Z"
		id = "$PROJECT"
		name = "walk"
		share = "true"
	TOML
	proxy_port[$name]=$(free_port)
	hub_port[$name]=$(free_port)
	unblock "$name"
	start_hub "$name"
}

connected() { # connected <machine> <count> -- the hub has said hello <count> times
	local n
	for _ in $(seq 1 100); do
		n=$(grep -c 'share: connected' "$T_TMP/$1.hub.log" 2>/dev/null)
		[ "${n:-0}" -ge "$2" ] && return 0
		sleep 0.1
	done
	return 1
}

# within <ms> <command...> -- runs the command until it succeeds; prints the
# milliseconds it took, and fails after <ms>.
within() {
	local limit=$1 start
	shift
	start=$(now_ms)
	while true; do
		if "$@" >/dev/null 2>&1; then
			echo $(($(now_ms) - start))
			return 0
		fi
		[ $(($(now_ms) - start)) -gt "$limit" ] && return 1
		sleep 0.05
	done
}
lists() { walk "$1" log --limit 50 | grep -qF "$2"; }
outbox_is() { on "$1" mem doctor | grep -qE "^sharing +seq [0-9]+, outbox $2\$"; }

for m in mini macbook nuc; do machine "$m"; done
for m in mini macbook nuc; do
	connected "$m" 1 || { notok "$m's hub connects to the store" "$(cat "$T_TMP/$m.hub.log")"; exit 1; }
done
ok "three hubs hold a sharing socket each"

# Behaviour 1: a log, then a page, within 2 s.
walk mini log "probe from the mini" >/dev/null
for m in macbook nuc; do
	if took=$(within 2000 lists "$m" "probe from the mini"); then
		ok "the $m lists the mini's log within 2 s (${took} ms)"
	else
		notok "the $m lists the mini's log within 2 s" "$(walk "$m" log 2>&1; cat "$T_TMP/$m.hub.log")"
	fi
done

printf '# Notes\n\nfrom the nuc: tabs\tand ünïcode\n' >"$T_TMP/notes.md"
walk nuc wiki notes --stdin --note "add notes" <"$T_TMP/notes.md" >/dev/null
same_notes() { walk mini wiki notes | cmp -s - "$T_TMP/notes.md"; }
if took=$(within 2000 same_notes); then
	ok "a wiki page from the nuc reads the same, byte for byte, on the mini within 2 s (${took} ms)"
else
	notok "the nuc's page reaches the mini byte for byte" "$(walk mini wiki notes 2>&1)"
fi

# Behaviour 2: five writes while the nuc is cut off.
block nuc
for n in 1 2 3 4 5; do walk nuc log "offline write $n" >/dev/null; done
run on nuc mem doctor
like "$OUT" "^sharing +seq [0-9]+, outbox 5$" "with the store unreachable, the nuc's doctor shows an outbox of 5"
run walk nuc log --limit 50
like "$OUT" "offline write 5" "reads on the cut-off nuc keep working: log"
run walk nuc search "offline write"
is "$RC" 0 "reads on the cut-off nuc keep working: search"
run walk nuc context
is "$RC" 0 "reads on the cut-off nuc keep working: context"
unblock nuc
if took=$(within 10000 outbox_is nuc 0); then
	ok "the nuc's outbox empties within 10 s of the store returning (${took} ms)"
else
	notok "the nuc's outbox empties within 10 s" "$(on nuc mem doctor; tail -20 "$T_TMP/nuc.hub.log")"
fi
all_five() { for n in 1 2 3 4 5; do lists mini "offline write $n" || return 1; done; }
if within 2000 all_five >/dev/null; then
	ok "the mini lists all five offline writes"
else
	notok "the mini lists all five offline writes" "$(walk mini log --limit 50)"
fi

# A machine that was off gets every write it missed, in order.
stop_hub macbook
for n in 1 2 3; do walk mini log "while the macbook slept $n" >/dev/null; done
start_hub macbook
missed() { for n in 1 2 3; do lists macbook "while the macbook slept $n" || return 1; done; }
if within 10000 missed >/dev/null; then
	ok "the macbook gets every write it missed when it comes back"
else
	notok "the macbook gets the writes it missed" "$(walk macbook log --limit 50; tail -20 "$T_TMP/macbook.hub.log")"
fi
seqs() { on "$1" mem share hello | python3 -c 'import json,sys; print(json.load(sys.stdin)["since"])'; }
caught_up() { [ "$(seqs macbook)" = "$(seqs mini)" ]; }
if within 5000 caught_up >/dev/null; then
	ok "the macbook's sequence catches up with the mini's"
else
	notok "the macbook's sequence catches up" "macbook $(seqs macbook), mini $(seqs mini)"
fi

# Behaviour 3: both offline, both edit one page; the second to arrive keeps a
# conflict file.
block mini
block nuc
printf '# Lessons\n\nfrom the mini\n' | walk mini wiki lessons --stdin --note "mini edit" >/dev/null
printf '# Lessons\n\nfrom the nuc\n' | walk nuc wiki lessons --stdin --note "nuc edit" >/dev/null
unblock mini
within 10000 outbox_is mini 0 >/dev/null || notok "the mini's outbox empties" "$(on mini mem doctor)"
unblock nuc
within 10000 outbox_is nuc 0 >/dev/null || notok "the nuc's outbox empties" "$(on nuc mem doctor)"
conflicts() { find "$T_TMP/m/$1/data/mem/store" -name '*.conflict*' | sort; }
is "$(conflicts mini | wc -l)" 0 "the mini, first to reach the store, has no conflict file"
is "$(conflicts nuc | wc -l)" 1 "the nuc, second to reach the store, has exactly one conflict file"
copy=$(conflicts nuc)
is "$(cat "$copy" 2>/dev/null)" "$(printf '# Lessons\n\nfrom the nuc')" "the conflict file holds the nuc's own version"
run on nuc mem doctor
like "$OUT" "conflict +$copy" "the nuc's doctor lists the conflict file by path"
lessons_agree() { [ "$(walk nuc wiki lessons)" = "$(printf '# Lessons\n\nfrom the mini')" ] &&
	[ "$(walk macbook wiki lessons)" = "$(printf '# Lessons\n\nfrom the mini')" ]; }
if within 2000 lessons_agree >/dev/null; then
	ok "every machine reads the mini's lessons page"
else
	notok "every machine reads the mini's lessons" "$(walk nuc wiki lessons; walk macbook wiki lessons)"
fi

# Two items written at the same moment on two machines both arrive.
walk mini log "same moment from the mini" >/dev/null &
first=$!
walk nuc log "same moment from the nuc" >/dev/null &
wait "$first" $!
both() { lists macbook "same moment from the mini" && lists macbook "same moment from the nuc"; }
if within 2000 both >/dev/null; then
	ok "two items written at once on two machines both reach the macbook"
else
	notok "two items written at once both arrive" "$(walk macbook log --limit 50)"
fi
is "$(conflicts macbook | wc -l)" 0 "items never make a conflict file"

# Every replica ends the same.
sleep 1
tree() { (cd "$T_TMP/m/$1/data/mem/store/projects/$PROJECT" && find items wiki -type f ! -name '*.conflict*' ! -name '.*' -exec sha256sum {} + | sort -k2); }
is "$(tree macbook)" "$(tree mini)" "the macbook's replica matches the mini's"
is "$(tree nuc)" "$(tree mini)" "the nuc's replica matches the mini's"
