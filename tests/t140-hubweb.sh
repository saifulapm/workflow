#!/usr/bin/env bash
# hubweb under `cf dev`: every path but the store write wants Cloudflare
# Access's JWT, signed RS256 by a key in the team's JWKS, for one of the
# Access apps' auds, inside its lifetime. The store takes records only with
# the bearer STORE_TOKEN, GET / renders one card per project record with its
# stage, its milestone and a Server-Timing header for the page build, and
# POST /mcp serves the handoff tool from the store.
#
# Needs hubweb/node_modules (pnpm -C hubweb install --frozen-lockfile),
# worker-build, the wasm32-unknown-unknown target, openssl and python3.
#
# cf dev hands the Worker to wrangler's cf delegate, which takes --port and
# nothing for state or secrets: state goes to .wrangler/state beside
# cloudflare.config.ts, and secrets come from the environment. So the test runs
# cf dev in a copy of hubweb inside T_TMP, which t_done removes with the state,
# and passes STORE_TOKEN and the Access settings in cf dev's environment,
# where ps does not show them.
#
# The test stands in for the Access team: it makes an RSA key, serves its
# JWKS at <team>/cdn-cgi/access/certs from a local HTTP server, and mints
# the tokens itself.

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

free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

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

start_dev() { # start_dev <name> [VAR=value...] -- cf dev on a copy of hubweb; sets CF_DEV_PORT
	local name=$1
	shift
	local app=$T_TMP/$name
	t_hubweb_copy "$app"
	t_cf_dev "$app" "$T_TMP/$name.log" "$@"
	local up=$?
	pids+=("$CF_DEV_PID")
	if [ "$up" != 0 ]; then
		notok "cf dev answers on port $CF_DEV_PORT" "$(tail -40 "$T_TMP/$name.log")"
		exit 1
	fi
}

token=test-store-token
printf 'Authorization: Bearer %s\n' "$token" >"$T_TMP/auth.hdr"
printf 'Authorization: Bearer wrong-%s\n' "$token" >"$T_TMP/wrong.hdr"
printf '%s\n' "$(with_jwt "$valid")" >"$T_TMP/jwt.hdr"

# Two auds: next-hub and next-mem are two Access apps on one Worker.
start_dev hubweb STORE_TOKEN=$token ACCESS_AUD="next-mem-aud, $aud" ACCESS_TEAM="$team"
port=$CF_DEV_PORT
base=http://127.0.0.1:$port

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
like "$(put "$records" -H "@$T_TMP/auth.hdr")" '^2[0-9][0-9]$' \
	"PUT /store/records with the token and no JWT stores two records"

walkdone='{"name":"walkdone","roadmap_status":"approved","milestone":"m2-y","plan_slug":"m2-y","plan_ticked":2,"plan_total":4,"milestones_done":2,"milestones_total":2}'
done_records='[{"project":"walkdone","kind":"project","key":"walkdone","body":'$walkdone'}]'
like "$(put "$done_records" -H "@$T_TMP/auth.hdr")" '^2[0-9][0-9]$' "PUT /store/records stores a finished project"

get_home() { # get_home [curl args...] -- prints the status code of GET /
	curl -s -o /dev/null -w '%{http_code}' "$@" "$base/"
}
fresh=$((now + 600))
is "$(get_home)" 401 "GET / without a JWT is refused"
is "$(get_home -H "$(with_jwt not-a-jwt)")" 401 "GET / with a malformed JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint key HS256 k1 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with an HS256 JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint key none k1 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with an unsigned (alg none) JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint other RS256 k1 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with a JWT signed by another key is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k1 "$(claims wrong-aud "$team" $fresh)")")")" 401 \
	"GET / with another app's aud is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k1 "$(claims "$aud" http://127.0.0.1:1 $fresh)")")")" 401 \
	"GET / with another team's iss is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k1 "$(claims "$aud" "$team" $((now - 60)))")")")" 401 \
	"GET / with an expired JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k2 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with a JWT naming an unknown kid is refused"

# The hub sandbox's four projects, written by real mem and sent to the
# store as a machine sends its writes: the pages draw from these files.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(source "$WF_ROOT/hub/tests/seed.sh") >/dev/null
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$port/store/share" "$token" \
	"$XDG_DATA_HOME/mem/store" 2>&1)" '^pushed [1-9][0-9]*$' "the seeded store's files reach the store"

home=$(curl -s -H "@$T_TMP/jwt.hdr" -D "$T_TMP/home.hdr" -o "$T_TMP/home.html" -w '%{http_code}' "$base/")
is "$home" 200 "GET / with a valid JWT answers 200"

# One line per card, so each check reads the card of the project it names.
card() { tr -d '\n' <"$T_TMP/home.html" | sed 's/<article/\n<article/g' | grep -F ">$1<"; }
like "$(card gamma)" 'class="pill">execution<' "gamma's card shows the stage execution"
like "$(card delta)" 'class="pill ok">done<' "a project with every milestone ticked shows done"
like "$(card beta)" 'planning' "a draft roadmap shows planning"
home_text=$(tr -d '\n' <"$T_TMP/home.html")
like "$home_text" 'Which hour does the nag go out\?' "home lists the question waiting on Saiful"
unlike "$home_text" 'Nothing waiting' "home never says nothing waits while a question does"
unlike "$home_text" '[Dd]ulali' "home names no Dulali"
like "$home_text" "new WebSocket\(.*'/live'\)" "home listens on /live"
unlike "$home_text" '/api/stamp' "home polls nothing"
like "$home_text" 'Route: Phone</p><p class="banner warn">Pair this device first' \
	"an unpaired browser sees the route and the pairing line in place of its buttons"

like "$(tr -d '\r' <"$T_TMP/home.hdr")" '^[Ss]erver-[Tt]iming: build;dur=[0-9.]+, store;dur=[0-9.]+' \
	"GET / carries the store's build time and the round trip in Server-Timing"

page_of() { # page_of <path> -- prints the status code, the page in $T_TMP/page.html
	curl -s -H "@$T_TMP/jwt.hdr" -o "$T_TMP/page.html" -w '%{http_code}' "$base$1"
}
is "$(page_of /p/gamma/questions)" 200 "a project's questions page answers 200"
like "$(tr -d '\n' <"$T_TMP/page.html")" 'Which hour does the nag go out\?' \
	"the questions page lists gamma's question"
is "$(page_of /p/nope/questions)" 404 "a project the store lacks is not found"

# Every page kind the hub has, drawn from the store with nothing missing.
item=$(cd "$sb/gamma" && mem log --kind=ruling --limit 1 --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["items"][0]["short_id"])')
for path in /p/gamma /p/beta/roadmap /p/gamma/plan /p/beta/plan/b1-box /p/gamma/questions \
	/p/gamma/evidence /p/gamma/wiki /wiki/gamma/spec /p/gamma/decisions /p/gamma/new "/p/gamma/item/$item"; do
	is "$(page_of "$path")" 200 "$path answers 200"
	unlike "$(tr -d '\n' <"$T_TMP/page.html")" 'the store does not answer|class="banner degraded"' \
		"$path reads everything it needs from the store"
done

# Pairing: a machine with the store token gets a single-use code; the
# device opens /pair/<code> and presses Pair from the page itself.
mint_code() { curl -s -X POST "$@" "$base/store/pair"; }
is "$(mint_code -o /dev/null -w '%{http_code}')" 401 "POST /store/pair without the store token is refused"
code=$(mint_code -H "@$T_TMP/auth.hdr")
like "$code" '^[0-9A-HJKMNP-TV-Z]{6}$' "POST /store/pair with the store token makes a six-character code"
is "$(page_of "/pair/$code")" 200 "the pairing page answers 200 while the code is pending"
like "$(cat "$T_TMP/page.html")" "action=\"/pair/$code\"" "the pairing page has its Pair button"
pair_post() { # pair_post <code> [curl args...] -- prints the status code, headers in $T_TMP/pair.hdr
	local c=$1
	shift
	curl -s -X POST -H "@$T_TMP/jwt.hdr" -D "$T_TMP/pair.hdr" -o /dev/null -w '%{http_code}' "$@" "$base/pair/$c"
}
is "$(pair_post "$code" -H 'Sec-Fetch-Site: cross-site')" 403 "a cross-site Pair is refused"
is "$(pair_post "$code" -H 'Sec-Fetch-Site: same-origin')" 303 "Pair from the page itself pairs"
cookie=$(tr -d '\r' <"$T_TMP/pair.hdr" | sed -n 's/^[Ss]et-[Cc]ookie: \(hub_device=[^;]*\);.*/\1/p')
like "$(tr -d '\r' <"$T_TMP/pair.hdr")" 'hub_device=[0-9A-Z]{16}; Path=/; Max-Age=31536000; HttpOnly; Secure; SameSite=Strict' \
	"the device gets a year's cookie, HttpOnly, Secure and SameSite=Strict"
is "$(pair_post "$code" -H 'Sec-Fetch-Site: same-origin')" 404 "a spent code pairs nothing more"
is "$(page_of /pair/ZZZZZZ)" 404 "an unknown code is not found"

paired_home=$(curl -s -H "@$T_TMP/jwt.hdr" -H "Cookie: $cookie" "$base/" | tr -d '\n')
like "$paired_home" '>Phone</button>.*>Device</button>' "a paired device's home offers Phone and Device"
like "$paired_home" 'name="text" value="nine"' "a paired device's home has the question's option buttons"
unlike "$paired_home" 'Pair this device first' "a paired device is not asked to pair"

# A POST from an unpaired browser gets the 403 page and writes nothing.
qid=$(cd "$sb/gamma" && mem questions --pending --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["questions"][0]["id"])')
answer_post() { # answer_post [curl args...] -- prints the status code, the page in $T_TMP/answer.html
	curl -s -X POST -H "@$T_TMP/jwt.hdr" -o "$T_TMP/answer.html" -w '%{http_code}' "$@" \
		--data-urlencode "id=$qid" --data-urlencode "text=nine" "$base/answer"
}
is "$(answer_post -H 'Sec-Fetch-Site: same-origin')" 403 "an unpaired browser's answer is refused"
like "$(cat "$T_TMP/answer.html")" 'This device is not paired, so this was not sent' "the 403 page says nothing was sent"
is "$(answer_post -H 'Sec-Fetch-Site: cross-site' -H "Cookie: $cookie")" 403 \
	"a paired device's cookie on a cross-site POST is refused"

font=$base/assets/maple-mono-400.woff2
curl -s -o /dev/null "$font"
fonthdr=$(curl -s -D - -o /dev/null "$font" | tr -d '\r')
like "$fonthdr" '^HTTP/[0-9.]+ 200' "the font is served"
like "$fonthdr" '[Cc][Ff]-[Cc]ache-[Ss]tatus: HIT' "the font's second load comes from Cloudflare's cache"
like "$fonthdr" '[Cc]ache-[Cc]ontrol: public, max-age=31536000, immutable' "the font is cached for a year"

mcp() { # mcp <path> <json-rpc message> [curl args...] -- prints the reply body
	local path=$1 message=$2
	shift 2
	curl -s -X POST "$@" -H 'Content-Type: application/json' \
		-H 'Accept: application/json, text/event-stream' --data "$message" "$base$path"
}
field() { # field <python expression on r> -- reads the JSON reply on stdin
	python3 -c "import json, sys; r = json.load(sys.stdin); print($1)" 2>/dev/null
}
handoff_call='{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"handoff","arguments":{"project":"walkone"}}}'

is "$(mcp /mcp "$handoff_call" -o /dev/null -w '%{http_code}')" 401 "POST /mcp without a JWT is refused"
init=$(mcp /mcp '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' -H "@$T_TMP/jwt.hdr")
is "$(field 'r["result"]["serverInfo"]["name"], r["result"]["protocolVersion"]' <<<"$init")" "mem 2025-06-18" \
	"initialize names the server and the protocol version asked for"
is "$(mcp /mcp '{"jsonrpc":"2.0","method":"notifications/initialized"}' -H "@$T_TMP/jwt.hdr" \
	-o /dev/null -w '%{http_code}')" 202 "notifications/initialized is accepted with no answer"
list=$(mcp /mcp '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' -H "@$T_TMP/jwt.hdr")
is "$(field '[t["name"] for t in r["result"]["tools"]], r["result"]["tools"][0]["inputSchema"]["required"]' <<<"$list")" \
	"['handoff'] ['project']" "tools/list lists handoff, which needs a project"
is "$(mcp /mcp "$handoff_call" -H "@$T_TMP/jwt.hdr" | field 'r["result"]["content"][0]["text"], r["result"]["isError"]')" \
	"Next: build the store. False" "tools/call handoff answers walkone's stored handoff"
is "$(mcp / "$handoff_call" -H "@$T_TMP/jwt.hdr" | field 'r["result"]["content"][0]["text"]')" \
	"Next: build the store." "POST / serves MCP too"

# An open page listens on /live: the store nudges it after every stored
# write, and a page socket can say nothing that the store acts on.
cat >"$T_TMP/live.mjs" <<'JS'
const [base, jwt, token] = process.argv.slice(2);
const ws = (path, headers) => {
  const s = new WebSocket(base.replace("http", "ws") + path, { headers });
  const inbox = [];
  s.onmessage = (e) => inbox.push(String(e.data));
  const opened = new Promise((ok, fail) => { s.onopen = ok; s.onerror = () => fail(new Error(path)); });
  const next = async (ms) => {
    const until = Date.now() + ms;
    while (!inbox.length && Date.now() < until) await new Promise((r) => setTimeout(r, 20));
    return inbox.shift() ?? null;
  };
  return { s, opened, next };
};
const page = ws("/live", { "Cf-Access-Jwt-Assertion": jwt });
const mac = ws("/store/share", { Authorization: `Bearer ${token}` });
await Promise.all([page.opened, mac.opened]);
mac.s.send(JSON.stringify({ t: "hello", machine: "mac", since: 0 }));
for (let m = await mac.next(2000); m && !m.includes('"ready"'); m = await mac.next(2000));
const at = Date.now();
mac.s.send(JSON.stringify({ t: "write", id: "01LIVE1", path: "projects/P9/items/01LIVEITEM.md", base: null, body: "x" }));
const nudge = await page.next(2000);
console.log("nudge", nudge, Date.now() - at < 2000);
console.log("mac-ack", (await mac.next(2000))?.includes('"ack"'));
page.s.send(JSON.stringify({ t: "hello", machine: "page", since: 0 }));
page.s.send(JSON.stringify({ t: "write", id: "01LIVE2", path: "projects/P9/items/01FORGED.md", base: null, body: "y" }));
console.log("page-answer", await page.next(800));
console.log("mac-after-forgery", await mac.next(800));
page.s.close(); mac.s.close();
JS
node "$T_TMP/live.mjs" "$base" "$valid" "$token" >"$T_TMP/live.out" 2>&1
live() { sed -n "s/^$1 //p" "$T_TMP/live.out"; }
is "$(live nudge)" '{"t":"changed"} true' "an open page hears changed within 2 s of a machine's write"
is "$(live mac-ack)" true "the machine's write is stored as usual"
is "$(live page-answer)" null "a page socket's hello and write get no answer"
is "$(live mac-after-forgery)" null "a write sent on a page socket reaches no machine"
is "$(curl -s -o /dev/null -w '%{http_code}' -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
	-H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' "$base/live")" 401 \
	"/live without a JWT is refused"

# No Access settings: every guarded path is refused, the store write is not.
start_dev bare STORE_TOKEN=$token
bare_port=$CF_DEV_PORT
is "$(curl -s -o /dev/null -w '%{http_code}' -H "@$T_TMP/jwt.hdr" "http://127.0.0.1:$bare_port/")" 401 \
	"GET / without ACCESS_AUD and ACCESS_TEAM is refused, even with a JWT"
like "$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "@$T_TMP/auth.hdr" \
	-H 'Content-Type: application/json' --data "$done_records" \
	"http://127.0.0.1:$bare_port/store/records")" '^2[0-9][0-9]$' \
	"PUT /store/records works without the Access settings"
