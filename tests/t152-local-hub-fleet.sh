#!/usr/bin/env bash
# The local hub alone does workflow agent's job under `cf dev`: a real
# `hub local` reports its machine to the store, so the hub web offers
# Resume for its dead run; Resume pressed on the page runs `workflow go`
# there within 5 s; and a Resume queued while that hub is down waits, the
# project reading `waiting for mini`, until the hub comes back and runs it.
#
# Needs what t147 needs: hubweb/node_modules, worker-build, the wasm
# target, openssl, python3 and node.

real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
	exit 1
fi

t_access_team
pids=()
stop_all() {
	local status=$?
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	kill "$team_pid" 2>/dev/null
	for pid in "${pids[@]}" "$team_pid"; do wait "$pid" 2>/dev/null; done
	(exit "$status")
	t_done
}
trap stop_all EXIT

token=test-store-token
window=3000
t_hubweb_copy "$T_TMP/hubweb"
t_cf_dev "$T_TMP/hubweb" "$T_TMP/dev.log" STORE_TOKEN=$token ACCESS_AUD="$aud" ACCESS_TEAM="$team" \
	HUB_AGENT_WINDOW_MS=$window
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "cf dev answers on port $CF_DEV_PORT" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
base=http://127.0.0.1:$CF_DEV_PORT
printf '%s\n' "$(with_jwt "$valid")" >"$T_TMP/jwt.hdr"

# gamma has an approved roadmap with g2-nag open, a git remote, and an
# orchestrator recorded on mini that no report lists: a dead run to resume.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(
	source "$WF_ROOT/hub/tests/seed.sh"
	cd "$sb/gamma" || exit 1
	mem project set runner mini gamma-g2-nag --milestone g2-nag
	mem project set remote https://example.invalid/gamma.git
) >/dev/null
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$CF_DEV_PORT/store/share" "$token" \
	"$XDG_DATA_HOME/mem/store" "$base" 2>&1)" '^pushed [1-9][0-9]*$' "the seeded store reaches the store"

code=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/pair")
cookie=$(curl -s -X POST -H "@$T_TMP/jwt.hdr" -H 'Sec-Fetch-Site: same-origin' -D - -o /dev/null \
	"$base/pair/$code" | tr -d '\r' | sed -n 's/^[Ss]et-[Cc]ookie: \(hub_device=[^;]*\);.*/\1/p')
like "$cookie" '^hub_device=' "a browser pairs"

get() { curl -s -H "@$T_TMP/jwt.hdr" "$base$1" | tr -d '\n'; }
# As the paired browser sees it: its forms, not the pairing notice.
paired_get() { curl -s -H "@$T_TMP/jwt.hdr" -H "Cookie: $cookie" "$base$1" | tr -d '\n'; }
control() {
	curl -s -X POST -H "@$T_TMP/jwt.hdr" -H "Cookie: $cookie" -H 'Sec-Fetch-Site: same-origin' \
		-o /dev/null -w '%{http_code}' "$@" "$base/p/gamma/control"
}

HUB_BIN=${HUB_BIN:-$WF_ROOT/hub/target/release/hub}

# The machine's tools: amx lists no session, so gamma's run is dead, and
# systemd-run writes down what it was asked to run.
fake=$T_TMP/fakebin
mkdir -p "$fake"
printf '#!/bin/sh\necho "[]"\n' >"$fake/amx"
printf '#!/bin/sh\necho "$*" >> "%s/ran"\n' "$T_TMP" >"$fake/systemd-run"
chmod +x "$fake/amx" "$fake/systemd-run"
ran() { cat "$T_TMP/ran" 2>/dev/null; }

hub_pid=
start_hub() {
	env PATH="$fake:$PATH" HUB_PEER_TOKEN=peer-token HUB_PEERS= HUB_TAILNET_NAME= \
		WORKFLOW_MACHINE=mini WORKFLOW_AMX="$fake/amx" WORKFLOW_SYSTEMD_RUN="$fake/systemd-run" \
		HUB_REPORT_EVERY_MS=1000 HUB_REPORT_CHECK_MS=200 \
		HUB_LOCAL_SOCKET="$T_TMP/local.sock" \
		HUB_STORE_URL="ws://127.0.0.1:$CF_DEV_PORT/store/share" HUB_STORE_TOKEN=$token \
		setsid "$HUB_BIN" local --port 0 >>"$T_TMP/hub.log" 2>&1 &
	hub_pid=$!
	pids+=("$hub_pid")
}
stop_hub() { kill -TERM -- "-$hub_pid" 2>/dev/null; wait "$hub_pid" 2>/dev/null; }
# until <seconds> <command...> -- true once the command is, polling every 0.1 s.
until_true() {
	local n=$(($1 * 10))
	shift
	for _ in $(seq "$n"); do "$@" && return 0; sleep 0.1; done
	return 1
}
offers_resume() { paired_get /p/gamma | grep -q 'name="machine" value="mini"'; }
waiting() { get /p/gamma | grep -q 'waiting for mini'; }
ran_lines() { [ "$(ran | grep -c 'workflow go gamma')" -ge "$1" ]; }

start_hub
until_true 15 offers_resume
is "$?" 0 "mini's local hub reports, and the page offers Resume on mini"

at=$(date +%s%N)
status=$(control --data-urlencode do=go --data-urlencode machine=mini)
is "$status" 303 "Resume is taken"
until_true 5 ran_lines 1
is "$?" 0 "the local hub runs it"
took=$((($(date +%s%N) - at) / 1000000))
[ "$took" -lt 5000 ] && ok "within 5 s ($took ms)" || notok "within 5 s" "took $took ms"
is "$(ran)" "--user --scope --quiet workflow go gamma" "as workflow go in a scope of its own"
until_true 5 offers_resume
is "$?" 0 "done, so the project waits no more"

# The hub goes down; a Resume pressed before the page knows waits for it.
# Its done goes out on the socket's next tick; a hub killed before that
# leaves the first command waiting, and the second press adds nothing.
sleep 1
stop_hub
status=$(control --data-urlencode do=go --data-urlencode machine=mini)
is "$status" 303 "a Resume for the stopped hub is taken"
sleep $((window / 1000 + 1))
until_true 5 waiting
is "$?" 0 "the project reads waiting for mini"
is "$(ran | grep -c .)" 1 "nothing ran while the hub was down"

start_hub
until_true 10 ran_lines 2
is "$?" 0 "the hub back up runs the waiting Resume"
until_true 5 offers_resume
is "$?" 0 "and the project waits no more"
unlike "$(cat "$T_TMP/hub.log")" "$token" "the store token never reaches the hub's log"
