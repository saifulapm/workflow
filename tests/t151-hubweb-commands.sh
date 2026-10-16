#!/usr/bin/env bash
# A command the hub web queues reaches a machine under `cf dev`: Resume
# pressed on a project is kept in the store and sent at once as a cmd frame
# to a listening machine, the project reads `waiting for mini` while its
# machine is away with the command waiting, and a done from the machine
# clears it. Run in cloud goes to the one online device that has the
# project when the mini is away, and pressing it twice queues one start.
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

# agent <machine> -- a machine that reports once, then listens: each cmd
# frame it hears is a line in frames.<machine>, and it answers done for the
# command id it finds in done.<machine>.
agent() {
	setsid node --input-type=module -e '
import fs from "node:fs";
const [url, token, machine, dir] = process.argv.slice(1);
const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } });
ws.onerror = () => { console.error("could not connect"); process.exit(1); };
ws.onopen = () => ws.send(JSON.stringify({ t: "hello", machine, since: 0 }));
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.t === "ready") {
    ws.send(JSON.stringify({ t: "report", machine, agents: [],
      checkouts: ["gamma"], worktrees: [], unpushed: {}, cloud: null }));
    fs.writeFileSync(`${dir}/ready.${machine}`, "");
  } else if (m.t === "cmd") {
    fs.appendFileSync(`${dir}/frames.${machine}`, e.data + "\n");
  }
};
setInterval(() => {
  const file = `${dir}/done.${machine}`;
  if (!fs.existsSync(file)) return;
  const id = fs.readFileSync(file, "utf8").trim();
  fs.unlinkSync(file);
  ws.send(JSON.stringify({ t: "done", id, machine }));
}, 100);
' "ws://127.0.0.1:$CF_DEV_PORT/store/share" "$token" "$1" "$T_TMP" &
	pids+=("$!")
	for _ in $(seq 50); do [ -e "$T_TMP/ready.$1" ] && return 0; sleep 0.1; done
	return 1
}
frames() { cat "$T_TMP/frames.$1" 2>/dev/null; }
wait_frame() {
	for _ in $(seq 50); do [ -s "$T_TMP/frames.$1" ] && return 0; sleep 0.1; done
	return 1
}

agent mini
is "$?" 0 "mini says hello and reports"
status=$(control --data-urlencode do=go --data-urlencode machine=mini)
is "$status" 303 "Resume is taken"
wait_frame mini
is "$?" 0 "mini hears a frame within 5 s"
like "$(frames mini)" '"command":\{"kind":"start","project":"gamma"\},"id":"[^"]*","machine":"mini","t":"cmd"' \
	"the frame is a start of gamma for mini"
id=$(frames mini | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)

sleep $((window / 1000 + 1))
like "$(get /p/gamma)" 'waiting for mini' "past the window the project reads waiting for mini"

printf '%s\n' "$id" >"$T_TMP/done.mini"
for _ in $(seq 50); do
	get /p/gamma | grep -q 'waiting for mini' || break
	sleep 0.1
done
unlike "$(get /p/gamma)" 'waiting for mini' "once mini is done the project no longer waits"

# Only nuc is online now: Run in cloud goes to it, once.
agent nuc
is "$?" 0 "nuc says hello and reports"
like "$(paired_get /p/gamma)" 'name="do" value="cloud"' "with the mini away the page still offers Run in cloud"
status=$(control --data-urlencode do=cloud)
is "$status" 303 "Run in cloud is taken"
control --data-urlencode do=cloud >/dev/null
sleep 1
cloud_frames=$(frames nuc | grep -c '"command":{"cloud":true,"kind":"start","project":"gamma"},"id":"[^"]*","machine":"nuc"')
is "$cloud_frames" 1 "two presses queue one cloud start, for nuc"
