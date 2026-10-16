#!/usr/bin/env bash
# hubweb under `cf dev`: the store takes records only with the bearer
# STORE_TOKEN, and GET / renders one card per project record with its stage,
# its milestone and a Server-Timing header for the page build.
#
# Needs hubweb/node_modules (pnpm -C hubweb install --frozen-lockfile),
# worker-build and the wasm32-unknown-unknown target.
#
# cf dev hands the Worker to wrangler's cf delegate, which takes --port and
# nothing for state or secrets: state goes to .wrangler/state beside
# cloudflare.config.ts, and secrets come from the environment. So the test runs
# cf dev in a copy of hubweb inside T_TMP, which t_done removes with the state,
# and passes STORE_TOKEN in cf dev's environment, where ps does not show it.

# The real cache dir, read before t_init moves HOME: worker-build keeps
# wasm-bindgen, wasm-opt and esbuild there, and would download them into a
# sandboxed one on every run.
real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
	exit 1
fi

app=$T_TMP/hubweb
mkdir -p "$app"
cp -r "$HUBWEB"/{Cargo.toml,Cargo.lock,src,package.json,cloudflare.config.ts,wrangler.config.ts} "$app/"
ln -s "$HUBWEB/node_modules" "$app/node_modules"

free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
port=$(free_port)
base=http://127.0.0.1:$port

token=test-store-token
printf 'Authorization: Bearer %s\n' "$token" >"$T_TMP/auth.hdr"
printf 'Authorization: Bearer wrong-%s\n' "$token" >"$T_TMP/wrong.hdr"

# setsid gives cf dev, wrangler and workerd one process group to kill at the
# end. CARGO_TARGET_DIR reuses hubweb's compiled dependencies. cf and
# wrangler each send usage telemetry unless told not to; the sandbox HOME
# holds no Cloudflare login, so nothing else can reach the account.
(
	cd "$app" &&
		STORE_TOKEN=$token XDG_CACHE_HOME=$real_cache CARGO_TARGET_DIR=$HUBWEB/target \
			CF_SEND_TELEMETRY=false WRANGLER_SEND_METRICS=false \
			exec setsid "$HUBWEB/node_modules/.bin/cf" dev --port "$port"
) </dev/null >"$T_TMP/dev.log" 2>&1 &
dev_pid=$!

stop_dev() {
	local status=$?
	kill -TERM -- "-$dev_pid" 2>/dev/null
	wait "$dev_pid" 2>/dev/null
	(exit "$status")
	t_done
}
trap stop_dev EXIT

# A cold run compiles the crate to wasm first, so the wait is long.
up=0
for _ in $(seq 1 600); do
	if curl -s -o /dev/null "$base/" 2>/dev/null; then
		up=1
		break
	fi
	kill -0 "$dev_pid" 2>/dev/null || break
	sleep 0.5
done
if [ "$up" != 1 ]; then
	notok "cf dev answers on $base" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi

put() { # put <body> [curl args...] -- prints the status code
	local body=$1
	shift
	curl -s -o /dev/null -w '%{http_code}' -X PUT "$@" \
		-H 'Content-Type: application/json' --data "$body" "$base/store/records"
}

walkone='{"name":"walkone","roadmap_status":"approved","milestone":"m1-x","plan_slug":"m1-x","plan_ticked":1,"plan_total":3,"milestones_done":0,"milestones_total":2}'
records='[{"project":"walkone","kind":"project","key":"walkone","body":'$walkone'},{"project":"walkone","kind":"handoff","key":"walkone","body":"Next: build the store."}]'

is "$(put "$records")" 401 "PUT /store/records without a token is refused"
is "$(put "$records" -H "@$T_TMP/wrong.hdr")" 401 "PUT /store/records with a wrong token is refused"
like "$(put "$records" -H "@$T_TMP/auth.hdr")" '^2[0-9][0-9]$' "PUT /store/records with the token stores two records"

walkdone='{"name":"walkdone","roadmap_status":"approved","milestone":"m2-y","plan_slug":"m2-y","plan_ticked":2,"plan_total":4,"milestones_done":2,"milestones_total":2}'
done_records='[{"project":"walkdone","kind":"project","key":"walkdone","body":'$walkdone'}]'
like "$(put "$done_records" -H "@$T_TMP/auth.hdr")" '^2[0-9][0-9]$' "PUT /store/records stores a finished project"

home=$(curl -s -D "$T_TMP/home.hdr" -o "$T_TMP/home.html" -w '%{http_code}' "$base/")
is "$home" 200 "GET / answers 200"

# One line per card, so each check reads the card of the project it names.
card() { tr -d '\n' <"$T_TMP/home.html" | sed 's/<article/\n<article/g' | grep -F ">$1<"; }
one=$(card walkone)
like "$one" 'class="pill">execution<' "walkone's card shows the stage execution"
like "$one" 'm1-x' "walkone's card shows its milestone"
first=1
like "$one" "milestone $first of 2" "walkone's card shows milestone $first of 2"
like "$(card walkdone)" 'class="pill ok">done<' "a project with every milestone ticked shows done"

like "$(tr -d '\r' <"$T_TMP/home.hdr")" '^[Ss]erver-[Tt]iming: build;dur=[0-9.]+' \
	"GET / carries a Server-Timing build duration"
