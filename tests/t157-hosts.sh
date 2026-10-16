#!/usr/bin/env bash
# The hostnames under `cf dev`: once the switch moves hub.on-rexa.com and
# mem.on-rexa.com to the Worker, the names in RETIRED_HOSTS (next-hub and
# next-mem) answer 404 to everything, pages, connector and store alike.
#
# Needs hubweb/node_modules, worker-build and the wasm target.

real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
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
t_cf_dev "$app" "$T_TMP/dev.log" STORE_TOKEN=$token \
	RETIRED_HOSTS="next-hub.on-rexa.com, next-mem.on-rexa.com"
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "cf dev answers" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
base=http://127.0.0.1:$CF_DEV_PORT

status() { # status <host> <path> [curl args...]
	local host=$1 path=$2
	shift 2
	curl -s -o /dev/null -w '%{http_code}' -H "Host: $host" "$@" "$base$path"
}
is "$(status next-hub.on-rexa.com /)" 404 "next-hub's home answers 404"
is "$(status next-mem.on-rexa.com /mcp -X POST -H 'Content-Type: application/json' --data '{}')" 404 \
	"next-mem's connector answers 404"
is "$(status next-hub.on-rexa.com /store/files/projects/P/evidence/x)" 404 \
	"next-hub's store answers 404 where it would ask for the bearer"
# cf dev pins every request to the config's first name, next-hub, so the
# names that still serve are checked in hubweb/src/hosts.rs's own tests;
# every other cf dev test runs with RETIRED_HOSTS unset and is served.
