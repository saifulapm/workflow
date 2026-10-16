#!/usr/bin/env bash
# Answering on the hub web reaches the session that asked. One machine (a
# replica with its own `hub local` holding the sharing socket) asks a
# question in a shared project; a paired browser answers it on the hub web
# under `cf dev`; the machine's waiting `mem questions --wait` prints the
# answer within 2 s, signed hub-web, and the home no longer lists it.
#
# Needs what t140 and t142 need: hubweb/node_modules, worker-build, the
# wasm target, openssl, python3, node and the hub's release binary.

real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
HUB_BIN=${HUB_BIN:-$WF_ROOT/hub/target/release/hub}
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ] || [ ! -x "$HUB_BIN" ]; then
	notok "hubweb/node_modules and the hub binary are there"
	exit 1
fi
free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
now_ms() { date +%s%3N; }

t_access_team
pids=()
stop_all() {
	local status=$?
	[ -n "${hub:-}" ] && kill "$hub" 2>/dev/null
	[ -n "${waiter:-}" ] && kill "$waiter" 2>/dev/null
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	kill "$team_pid" 2>/dev/null
	wait 2>/dev/null
	(exit "$status")
	t_done
}
trap stop_all EXIT

token=test-store-token
t_hubweb_copy "$T_TMP/hubweb"
t_cf_dev "$T_TMP/hubweb" "$T_TMP/dev.log" STORE_TOKEN=$token ACCESS_AUD="$aud" ACCESS_TEAM="$team"
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "cf dev answers" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
base=http://127.0.0.1:$CF_DEV_PORT
printf '%s\n' "$(with_jwt "$valid")" >"$T_TMP/jwt.hdr"

# The machine: a replica holding the shared project, and its local hub.
PROJECT=01K0SHAREWA7K0000000000000
root=$T_TMP/mac
mkdir -p "$root"/{data,cache,state,config/qshell} "$root/data/mem/store/projects/$PROJECT/items"
printf 'mac\n' >"$root/config/qshell/machine"
cat >"$root/data/mem/store/projects/$PROJECT/project.toml" <<TOML
created = "2026-10-16T00:00:00Z"
id = "$PROJECT"
name = "walk"
share = "true"
TOML
on() {
	env HOME="$root" XDG_DATA_HOME="$root/data" XDG_CACHE_HOME="$root/cache" \
		XDG_STATE_HOME="$root/state" XDG_CONFIG_HOME="$root/config" \
		HUB_LOCAL_SOCKET="$root/state/hub/local.sock" MEM_SYNC_CMD=true MEM_NOTIFY_CMD=true "$@"
}
walk() { on mem --project walk "$@"; }
on env HUB_PEER_TOKEN=peer-token HUB_PEERS= \
	HUB_STORE_URL="ws://127.0.0.1:$CF_DEV_PORT/store/share" HUB_STORE_TOKEN=$token \
	"$HUB_BIN" local --port "$(free_port)" >>"$T_TMP/hub.log" 2>&1 &
hub=$!
for _ in $(seq 1 100); do grep -q 'share: connected' "$T_TMP/hub.log" && break; sleep 0.1; done

# Through mem, so project.toml travels to the store as any shared write.
walk project set verify true >/dev/null
asked=$(walk ask --for human --options 'yes,no' 'Ship the walk?' | sed -n 's/^#\([0-9A-Z]*\).*/\1/p')
like "$asked" '^[0-9A-Z]{8}$' "the machine asks a question in the shared project"

home() { curl -s -H "@$T_TMP/jwt.hdr" "$@" "$base/" | tr -d '\n'; }
for _ in $(seq 1 100); do home | grep -q 'Ship the walk?' && break; sleep 0.1; done
like "$(home)" 'Ship the walk\?' "the question reaches the hub web's home"

code=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/pair")
cookie=$(curl -s -X POST -H "@$T_TMP/jwt.hdr" -H 'Sec-Fetch-Site: same-origin' -D - -o /dev/null \
	"$base/pair/$code" | tr -d '\r' | sed -n 's/^[Ss]et-[Cc]ookie: \(hub_device=[^;]*\);.*/\1/p')
like "$cookie" '^hub_device=' "a browser pairs"

walk questions --wait "$asked" --timeout 30s >"$T_TMP/waited.out" 2>&1 &
waiter=$!
sleep 1
sent=$(now_ms)
status=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "@$T_TMP/jwt.hdr" -H "Cookie: $cookie" \
	-H 'Sec-Fetch-Site: same-origin' --data-urlencode "id=$asked" --data-urlencode "text=yes" "$base/answer")
is "$status" 303 "the paired browser's answer is taken"
for _ in $(seq 1 100); do [ -s "$T_TMP/waited.out" ] && break; sleep 0.05; done
took=$(($(now_ms) - sent))
like "$(cat "$T_TMP/waited.out")" 'yes' "the waiting session prints the answer"
truthy "$([ "$took" -lt 2000 ]; echo $?)" "within 2 s of the answer (took ${took} ms)"
by=$(walk questions --json | python3 -c "import json,sys; print([q['answer_by'] for q in json.load(sys.stdin)['questions'] if q['short_id']=='$asked'][0])")
is "$by" hub-web "the answer is signed hub-web"
unlike "$(home)" 'Ship the walk\?' "the home no longer lists it as waiting"
