#!/usr/bin/env bash
# The hub web draws each of the hub's page kinds like the old hub does. Both
# serve one seeded store (hub/tests/seed.sh): the old hub reads it in place,
# and the hub web under `cf dev` gets its files over the sharing socket and
# its evidence over the Files store. Each page is fetched with curl as a paired
# device, saved with the fonts and images it names, served from loopback, and
# shot by a 390 px browser. The test passes a page when the hub web answers
# 200 and fewer than 1% of the pixels in the two shots differ.
#
# Differences decided on purpose are blanked in the browser, the same on both
# sides, before the shot:
#   - the header's machine name: the hub's host against hub;
#   - the route line and its form;
#   - home's nav: sibling hubs and the subscribe link;
#   - relative ages ("10s", "2m"), which move with the clock.
# Evidence images are not blanked: the test PUTs the seeded bytes to the
# Files store, so both pages show them.
#
# A failing page leaves <page>.old.png, <page>.new.png and the percentage in
# T_TMP; WF_KEEP_TMP=1 keeps it.
#
# Needs hubweb/node_modules, worker-build, the wasm32-unknown-unknown target,
# openssl, python3, node and playwright-cli (without it the test skips).

# The real homes, read before t_init moves HOME: worker-build keeps its tools
# in the cache, and the browser keeps its profile and config under HOME.
real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
caller_home=$HOME
caller_config=${XDG_CONFIG_HOME:-$HOME/.config}
short_tmp=${XDG_RUNTIME_DIR:-/tmp}
lock=$short_tmp/hub-browser.lock
source "$(dirname -- "$0")/lib.sh"
t_init

if ! command -v playwright-cli >/dev/null 2>&1; then
	printf 'ok 1 # skip playwright-cli is not installed\n'
	_checks=1
	exit 0
fi

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

t_access_team

pids=()
stop_all() {
	local status=$?
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	kill "$team_pid" "${plain_pids[@]}" 2>/dev/null
	for pid in "${pids[@]}" "$team_pid" "${plain_pids[@]}"; do wait "$pid" 2>/dev/null; done
	(exit "$status")
	t_done
}
plain_pids=()
trap stop_all EXIT

free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

# The seeded store, written by real mem, with one paired device for the old hub.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(source "$WF_ROOT/hub/tests/seed.sh") >/dev/null
item=$(cd "$sb/gamma" && mem log --kind=ruling --limit 1 --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["items"][0]["short_id"])')

mkdir -p "$XDG_CONFIG_HOME/hub" "$XDG_STATE_HOME/hub"
printf 'ntfy_base = "http://127.0.0.1:9"\n' >"$XDG_CONFIG_HOME/hub/config.toml"
printf 'SANDBOXDEVICESANDBOXDEVICE 2026-01-01T00:00:00Z sandbox\n' >"$XDG_STATE_HOME/hub/devices"
chmod 600 "$XDG_STATE_HOME/hub/devices"
(cd "$sb" && exec "$HUB_BIN" --port 0 >"$T_TMP/hub.out" 2>"$T_TMP/hub.err") &
plain_pids+=($!)
hub_port=""
for _ in $(seq 100); do
	hub_port=$(sed -n 's/^hub listening on 127\.0\.0\.1:\([0-9]*\)$/\1/p' "$T_TMP/hub.out")
	[ -n "$hub_port" ] && break
	sleep 0.1
done
if [ -z "$hub_port" ]; then
	notok "the old hub starts" "$(cat "$T_TMP/hub.err")"
	exit 1
fi
old_base=http://127.0.0.1:$hub_port

token=test-store-token
printf 'Authorization: Bearer %s\n' "$token" >"$T_TMP/auth.hdr"
printf '%s\n' "$(with_jwt "$valid")" >"$T_TMP/jwt.hdr"
app=$T_TMP/hubweb
t_hubweb_copy "$app"
if ! t_cf_dev "$app" "$T_TMP/hubweb.log" STORE_TOKEN=$token ACCESS_AUD="$aud" ACCESS_TEAM="$team"; then
	notok "cf dev answers on port $CF_DEV_PORT" "$(tail -40 "$T_TMP/hubweb.log")"
	exit 1
fi
pids+=("$CF_DEV_PID")
new_base=http://127.0.0.1:$CF_DEV_PORT

push=$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$CF_DEV_PORT/store/share" "$token" \
	"$XDG_DATA_HOME/mem/store" "$new_base" 2>&1)
like "$push" '^pushed [1-9][0-9]*$' "the seeded store reaches the hub web"

code=$(curl -s -X POST -H "@$T_TMP/auth.hdr" "$new_base/store/pair")
curl -s -X POST -H "@$T_TMP/jwt.hdr" -H 'Sec-Fetch-Site: same-origin' -D "$T_TMP/pair.hdr" -o /dev/null "$new_base/pair/$code"
new_cookie=$(tr -d '\r' <"$T_TMP/pair.hdr" | sed -n 's/^[Ss]et-[Cc]ookie: \(hub_device=[^;]*\);.*/\1/p')
old_cookie=hub_device=SANDBOXDEVICESANDBOXDEVICE

paths=(/ /p/gamma /p/beta/roadmap /p/gamma/plan /p/beta/plan/b1-box /p/gamma/questions
	/p/gamma/evidence /p/gamma/wiki /wiki/gamma/spec /p/gamma/decisions /p/gamma/new "/p/gamma/item/$item")

# fetch <side> <base> <path> <n> -- the page as <side>/page-<n>.html, with
# the files its src attributes name, under the same paths. Prints the status.
fetch() {
	local side=$1 base=$2 path=$3 n=$4 src
	local -a auth
	if [ "$side" = new ]; then auth=(-H "@$T_TMP/jwt.hdr" -H "Cookie: $new_cookie"); else auth=(-H "Cookie: $old_cookie"); fi
	curl -s "${auth[@]}" -o "$T_TMP/site/$side/page-$n.html" -w '%{http_code}' "$base$path"
	while IFS= read -r src; do
		src=${src//&amp;/&}
		mkdir -p "$T_TMP/site/$side$(dirname -- "${src%%\?*}")"
		curl -s "${auth[@]}" -o "$T_TMP/site/$side${src%%\?*}" "$base$src"
	done < <(grep -o 'src="/[^"]*"' "$T_TMP/site/$side/page-$n.html" | sed 's/^src="//; s/"$//')
}

for side in old new; do
	mkdir -p "$T_TMP/site/$side/assets"
	cp "$WF_ROOT"/hubpages/public/assets/*.woff2 "$T_TMP/site/$side/assets/"
	port=$(free_port)
	python3 -m http.server --bind 127.0.0.1 --directory "$T_TMP/site/$side" "$port" >"$T_TMP/site-$side.log" 2>&1 &
	plain_pids+=($!)
	eval "${side}_site=http://127.0.0.1:$port"
done

job='{"out":"'$T_TMP'","pages":['
sep=
n=0
for path in "${paths[@]}"; do
	n=$((n + 1))
	is "$(fetch new "$new_base" "$path" "$n")" 200 "the hub web answers 200 for $path"
	fetch old "$old_base" "$path" "$n" >/dev/null
	job+=$sep'{"name":"page-'$n'","old":"'$old_site'/page-'$n'.html","new":"'$new_site'/page-'$n'.html"}'
	sep=,
done
job+=']}'

# One browser at a time on this machine.
pw() { HOME=$caller_home XDG_CONFIG_HOME=$caller_config XDG_CACHE_HOME=$real_cache TMPDIR=$short_tmp playwright-cli -s="parity-$$" "$@"; }
shopt -u patsub_replacement 2>/dev/null
js=$(<"$WF_ROOT/tests/parity-shot.js")
printf '%s\n' "${js//JOB/$job}" >"$T_TMP/shot.js"
exec 9>"$lock"
flock 9
opened=$(pw open about:blank 2>&1)
shots=$(pw run-code --filename "$T_TMP/shot.js" 2>&1)
pw close >/dev/null 2>&1
exec 9>&-
like "$shots" '"shot"' "the browser takes every shot"

n=0
for path in "${paths[@]}"; do
	n=$((n + 1))
	read -r differing total < <(node "$WF_ROOT/tests/parity-diff.mjs" "$T_TMP/page-$n.old.png" "$T_TMP/page-$n.new.png" 2>&1)
	pct=$(python3 -c "print('%.2f' % (100 * $differing / $total))" 2>/dev/null)
	if [ -n "$pct" ] && python3 -c "import sys; sys.exit(not $pct < 1)"; then
		ok "$path differs from the old hub in $pct% of its pixels"
		[ -n "${WF_KEEP_TMP:-}" ] || rm -f "$T_TMP/page-$n.old.png" "$T_TMP/page-$n.new.png"
	else
		notok "$path differs from the old hub in ${pct:-?}% of its pixels" \
			"shots: $T_TMP/page-$n.old.png and $T_TMP/page-$n.new.png (WF_KEEP_TMP=1 keeps them)"
		keep=1
	fi
done
if [ -n "${keep:-}" ]; then export WF_KEEP_TMP=1; fi
