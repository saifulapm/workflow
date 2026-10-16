#!/usr/bin/env bash
# The hub web under `cf dev` sees the fleet the store keeps: a machine's
# report makes its run show under Running as `<project> · <machine> ·
# <milestone>`, a paused project whose run is live reads `pausing`, and once
# the report is older than HUB_AGENT_WINDOW_MS the run is gone from Running.
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
window=4000
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

# gamma has an approved roadmap with g2-nag open; its orchestrator is
# recorded as running on mini under the name workflow gives it.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(
	source "$WF_ROOT/hub/tests/seed.sh"
	cd "$sb/gamma" || exit 1
	mem project set runner mini gamma-g2-nag --milestone g2-nag
) >/dev/null
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$CF_DEV_PORT/store/share" "$token" \
	"$XDG_DATA_HOME/mem/store" "$base" 2>&1)" '^pushed [1-9][0-9]*$' "the seeded store reaches the store"

code=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/pair")
cookie=$(curl -s -X POST -H "@$T_TMP/jwt.hdr" -H 'Sec-Fetch-Site: same-origin' -D - -o /dev/null \
	"$base/pair/$code" | tr -d '\r' | sed -n 's/^[Ss]et-[Cc]ookie: \(hub_device=[^;]*\);.*/\1/p')
like "$cookie" '^hub_device=' "a browser pairs"

get() { curl -s -H "@$T_TMP/jwt.hdr" "$base$1" | tr -d '\n'; }
running() { get / | sed -n 's/.*<h2>Running<\/h2>\(.*\)/\1/p' | sed 's/<h2>.*//; s/<[^>]*>//g'; }

# A machine says hello, then reports gamma's orchestrator running.
node --input-type=module -e '
const [url, token] = process.argv.slice(1);
const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } });
ws.onerror = () => { console.error("could not connect"); process.exit(1); };
ws.onopen = () => ws.send(JSON.stringify({ t: "hello", machine: "mac-mini-m2", since: 0 }));
ws.onmessage = (e) => {
  if (JSON.parse(e.data).t !== "ready") return;
  ws.send(JSON.stringify({ t: "report", machine: "mini",
    agents: [{ id: "gamma-g2-nag", state: "running", ended: 0, created: 1 }],
    checkouts: ["gamma"], worktrees: [], unpushed: {}, cloud: null }));
  setTimeout(() => process.exit(0), 500);
};
' "ws://127.0.0.1:$CF_DEV_PORT/store/share" "$token"
is "$?" 0 "mini reports gamma's agent running"

like "$(running)" 'gamma · mini · g2-nag' "a live run is listed under Running"
unlike "$(get /p/gamma)" 'class="pill[^"]*">pausing<' "an unpaused project does not read pausing"

status=$(curl -s -X POST -H "@$T_TMP/jwt.hdr" -H "Cookie: $cookie" -H 'Sec-Fetch-Site: same-origin' \
	-o /dev/null -w '%{http_code}' --data-urlencode do=pause "$base/p/gamma/control")
is "$status" 303 "Pause is taken"
like "$(get /p/gamma)" 'class="pill[^"]*">pausing<' "a paused project with a live run reads pausing"

sleep $((window / 1000 + 1))
unlike "$(running)" 'gamma · mini' "past the window the run is no longer under Running"
