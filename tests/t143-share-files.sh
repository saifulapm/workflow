#!/usr/bin/env bash
# A shared project's evidence and raw bytes under `cf dev`: PUT and GET
# /store/files/<path> with the bearer STORE_TOKEN keep a file in the
# project's own Durable Object, in rows of at most 1 MiB, and give it back
# byte for byte. Then two machines end to end: evidence added on the mini is
# uploaded by its hub, and `mem evidence cat` on the macbook fetches it.
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
now_ms() { date +%s%3N; }

pids=()
declare -A hub_pid
stop_all() {
	local status=$?
	for name in "${!hub_pid[@]}"; do kill "${hub_pid[$name]}" 2>/dev/null; done
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
port=$CF_DEV_PORT
base=http://127.0.0.1:$port
if [ "$up" != 0 ]; then
	notok "cf dev answers on port $port" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi

files=$base/store/files/projects/P1/evidence/t1
put() { # put <file> <url> [curl args...] -- prints the status
	local file=$1 url=$2
	shift 2
	curl -s -o /dev/null -w '%{http_code}' --max-time 60 -X PUT --data-binary "@$file" \
		-H "Authorization: Bearer $token" "$@" "$url"
}
get() { # get <url> <out> -- prints the status
	curl -s -o "$2" -w '%{http_code}' --max-time 60 -H "Authorization: Bearer $token" "$1"
}
sha() { sha256sum "$1" | cut -d' ' -f1; }

head -c 16800000 /dev/urandom >"$T_TMP/big.bin"
is "$(put "$T_TMP/big.bin" "$files/big%20one.bin")" 204 "a 16.8 MB file is stored"
is "$(get "$files/big%20one.bin" "$T_TMP/big.back")" 200 "the 16.8 MB file is read back"
is "$(sha "$T_TMP/big.back")" "$(sha "$T_TMP/big.bin")" "the 16.8 MB file reads back with the same sha256"

head -c 3000000 /dev/urandom >"$T_TMP/first.bin"
head -c 3000000 /dev/urandom >"$T_TMP/second.bin"
put "$T_TMP/first.bin" "$files/shot.png" >/dev/null
is "$(put "$T_TMP/second.bin" "$files/shot.png")" 204 "a second PUT of the same path is stored"
get "$files/shot.png" "$T_TMP/shot.back" >/dev/null
is "$(sha "$T_TMP/shot.back")" "$(sha "$T_TMP/second.bin")" "the second PUT replaces the first"

is "$(get "$files/missing.png" /dev/null)" 404 "a missing path is 404"
is "$(curl -s -o /dev/null -w '%{http_code}' "$files/shot.png")" 401 "a GET with no bearer is 401"
is "$(curl -s -o /dev/null -w '%{http_code}' -X PUT --data-binary x "$files/shot.png")" 401 \
	"a PUT with no bearer is 401"
is "$(get "$base/store/files/projects/P1/wiki/a.md" /dev/null)" 400 "a path outside evidence and raw is 400"

# Two machines, each its own replica and its own hub, as in t142.
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

machine() { # machine <name> -- a replica holding the shared project, and its hub
	local name=$1 root=$T_TMP/m/$1
	mkdir -p "$root"/{data,cache,state,config/qshell} "$root/data/mem/store/projects/$PROJECT/items"
	printf '%s\n' "$name" >"$root/config/qshell/machine"
	cat >"$root/data/mem/store/projects/$PROJECT/project.toml" <<-TOML
		created = "2026-10-16T00:00:00Z"
		id = "$PROJECT"
		name = "walk"
		share = "true"
	TOML
	on "$name" env HUB_PEER_TOKEN=peer-token \
		HUB_STORE_URL="ws://127.0.0.1:$port/store/share" HUB_STORE_TOKEN=$token \
		"$HUB_BIN" local --port "$(free_port)" >>"$T_TMP/$name.hub.log" 2>&1 &
	hub_pid[$name]=$!
}

connected() { # connected <machine> -- the hub has said hello
	for _ in $(seq 1 100); do
		grep -q 'share: connected' "$T_TMP/$1.hub.log" 2>/dev/null && return 0
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

for m in mini macbook; do machine "$m"; done
for m in mini macbook; do
	connected "$m" || { notok "$m's hub connects to the store" "$(cat "$T_TMP/$m.hub.log")"; exit 1; }
done

head -c 3000000 /dev/urandom >"$T_TMP/screenshot.png"
id=$(walk mini evidence add --task t1 "$T_TMP/screenshot.png" --note "the screenshot" --json |
	python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
same_shot() {
	[ "$(walk macbook evidence cat "$id" | sha256sum | cut -d' ' -f1)" = "$(sha "$T_TMP/screenshot.png")" ]
}
if took=$(within 10000 same_shot); then
	ok "the mini's 3 MB screenshot reads the same on the macbook within 10 s (${took} ms)"
else
	notok "the mini's screenshot reaches the macbook byte for byte" \
		"$(walk macbook evidence list 2>&1; tail -n 20 "$T_TMP/mini.hub.log" "$T_TMP/macbook.hub.log")"
fi
