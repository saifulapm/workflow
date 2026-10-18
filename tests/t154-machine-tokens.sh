#!/usr/bin/env bash
# Each machine's own store token under `cf dev`: the holder of the shared
# bearer mints one for mini (`hub share mint`), one for nuc and one for the
# cloud sessions, each for a runner and a mem machine name. A socket opened
# with mini's token reports as mini, but a report or a done naming nuc is
# refused and changes nothing; the cloud's token reports as any session.
# The shared bearer still speaks for any machine, a client cannot bind its
# socket by sending the store's own header, mini's token works on
# /store/files and /store/pair, and it cannot mint.
#
# Needs what t151 needs, and the hub's release binary, which tests/run.sh
# builds.

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
t_hubweb_copy "$T_TMP/hubweb"
t_cf_dev "$T_TMP/hubweb" "$T_TMP/dev.log" STORE_TOKEN=$token ACCESS_AUD="$aud" ACCESS_TEAM="$team"
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "cf dev answers on port $CF_DEV_PORT" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
base=http://127.0.0.1:$CF_DEV_PORT
share=ws://127.0.0.1:$CF_DEV_PORT/store/share
printf '%s\n' "$(with_jwt "$valid")" >"$T_TMP/jwt.hdr"

# gamma's orchestrator is recorded on nuc, and no report lists it: a dead
# run that Resume starts on nuc. beta's is recorded on mini.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(
	source "$WF_ROOT/hub/tests/seed.sh"
	cd "$sb/gamma" || exit 1
	mem project set runner nuc gamma-g2-nag --milestone g2-nag
	mem project set remote https://example.invalid/gamma.git
	cd "$sb/beta" || exit 1
	mem project set runner mini beta-b1-box --milestone b1-box
) >/dev/null
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "$share" "$token" \
	"$XDG_DATA_HOME/mem/store" "$base" 2>&1)" '^pushed [1-9][0-9]*$' "the seeded store reaches the store"

code=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/pair")
cookie=$(curl -s -X POST -H "@$T_TMP/jwt.hdr" -H 'Sec-Fetch-Site: same-origin' -D - -o /dev/null \
	"$base/pair/$code" | tr -d '\r' | sed -n 's/^[Ss]et-[Cc]ookie: \(hub_device=[^;]*\);.*/\1/p')
like "$cookie" '^hub_device=' "a browser pairs"

get() { curl -s -H "@$T_TMP/jwt.hdr" "$base$1" | tr -d '\n'; }
running() { get / | sed -n 's/.*<h2>Running<\/h2>\(.*\)/\1/p' | sed 's/<h2>.*//; s/<[^>]*>//g'; }

# say <bearer> <bound header, or ""> <frame>... -- says hello as
# mac-mini-m2, sends each frame, and prints every reply but recs and routes.
say() {
	node --input-type=module -e '
const [url, bearer, bound, ...frames] = process.argv.slice(1);
const headers = { Authorization: `Bearer ${bearer}` };
if (bound) headers["X-Hub-Bound"] = bound;
const ws = new WebSocket(url, { headers });
ws.onerror = () => { console.error("could not connect"); process.exit(1); };
ws.onopen = () => ws.send(JSON.stringify({ t: "hello", machine: "mac-mini-m2", since: 0 }));
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.t === "ready") {
    for (const frame of frames) ws.send(frame);
    setTimeout(() => process.exit(0), 800);
  } else if (m.t !== "rec" && m.t !== "route") {
    console.log(e.data);
  }
};
' "$share" "$@"
}
report() { # report <machine> <agent id, or ""> -- a report frame
	local agents='[]'
	[ -n "$2" ] && agents="[{\"id\":\"$2\",\"state\":\"running\",\"ended\":0,\"created\":1}]"
	printf '{"t":"report","machine":"%s","agents":%s,"checkouts":["gamma"],"worktrees":[],"unpushed":{},"cloud":null}' \
		"$1" "$agents"
}

mini=$(printf '%s\n' "$token" | HUB_STORE_URL=$share "$HUB_BIN" share mint mini mac-mini-m2)
like "$mini" '^[0-9a-f]{60}$' "hub share mint prints mini's new token"
nuc=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/machines/nuc/nuc")
like "$nuc" '^[0-9a-f]{60}$' "the shared bearer mints nuc's token"
isnt "$mini" "$nuc" "each machine gets a token of its own"
cloud=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/machines/session_*/cloud-*")
like "$cloud" '^[0-9a-f]{60}$' "the shared bearer mints the cloud's token for every session"
status=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $token" "$base/store/machines/x")
is "$status" 400 "a mint names the machine as well as the runner"
status=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $token" "$base/store/machines/x/a*b")
is "$status" 400 "a star ends a name or is not there"
status=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $mini" "$base/store/machines/x/x")
is "$status" 401 "a machine token cannot mint"
code=$(curl -s -X POST -H "Authorization: Bearer $mini" "$base/store/pair")
like "$code" '^[0-9A-HJKMNP-TV-Z]{6}$' "a machine token gets a pairing code, as hub pair --web asks with it"
status=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$base/store/machines/x/x")
is "$status" 401 "no bearer cannot mint"
status=$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer nope" "$base/store/share")
is "$status" 401 "an unknown token opens no socket"

# nuc is online with gamma checked out, and Resume queues a start for it.
is "$(say "$token" "" "$(report nuc "")")" "" "the shared bearer reports as nuc"
status=$(curl -s -X POST -H "@$T_TMP/jwt.hdr" -H "Cookie: $cookie" -H 'Sec-Fetch-Site: same-origin' \
	-o /dev/null -w '%{http_code}' --data-urlencode do=go --data-urlencode machine=nuc "$base/p/gamma/control")
is "$status" 303 "Resume on nuc is taken"
held=$(say "$token" "" "$(report nuc "")")
like "$held" '"machine":"nuc","t":"cmd"' "nuc's report hears its start"
id=$(printf '%s\n' "$held" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)

# mini's token speaks for mini only.
own=$(say "$mini" "" "$(report mini beta-b1-box)")
unlike "$own" '"t":"error"' "mini's token reports as mini"
like "$(running)" 'beta · mini · b1-box' "mini's report is kept"
forged=$(say "$mini" "" "$(report nuc gamma-g2-nag)" "{\"t\":\"done\",\"id\":\"$id\",\"machine\":\"nuc\"}")
is "$(printf '%s\n' "$forged" | grep -c '"t":"error"')" 2 "a report and a done naming nuc are refused"
unlike "$(running)" 'gamma · nuc' "the refused report changed nothing"
is "$(say "$cloud" "" "$(report session_01cloud "")" | grep -c '"t":"error"')" 0 \
	"the cloud's token reports as a session"
is "$(say "$cloud" "" "$(report mini "")" | grep -c '"t":"error"')" 1 \
	"the cloud's token cannot report as mini"
like "$(say "$token" "" "$(report nuc "")")" "\"id\":\"$id\"" "nuc's start is still waiting"

# A client cannot bind a socket by sending the store's own header: the
# shared bearer's socket still reports as nuc.
is "$(say "$token" mini "$(report nuc gamma-g2-nag)" | grep -c '"t":"error"')" 0 \
	"a client's X-Hub-Bound is not the store's"
like "$(running)" 'gamma · nuc · g2-nag' "the shared bearer reports as anyone"

# mini's token keeps and fetches a project's file.
printf 'evidence bytes\n' >"$T_TMP/ev"
status=$(curl -s -o /dev/null -w '%{http_code}' -X PUT --data-binary "@$T_TMP/ev" \
	-H "Authorization: Bearer $mini" "$base/store/files/projects/gamma/evidence/t1")
is "$status" 204 "mini's token stores a file"
is "$(curl -s -H "Authorization: Bearer $mini" "$base/store/files/projects/gamma/evidence/t1")" \
	"evidence bytes" "mini's token fetches it back"
