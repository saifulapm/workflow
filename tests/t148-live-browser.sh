#!/usr/bin/env bash
# An open hub web page in a real browser: a question that reaches the store
# shows within 2 s with no manual reload, a page with text in a field waits
# until the field is left, and an idle page sends nothing.
#
# The browser is playwright-cli's own isolated one, pointed at a small proxy
# (jwt-proxy.mjs) that adds Cloudflare Access's JWT, since a browser cannot
# put that header on its own WebSocket. One browser runs at a time on this
# machine, so the browser part holds the lock hub/tests/sandbox.sh takes.
#
# The idle stretch is 60 s here. The 10-minute check is on the real deploy.
#
# Needs what t140 needs (hubweb/node_modules, worker-build, openssl, python3)
# and playwright-cli, without which the test skips.

# The real cache and home, read before t_init moves HOME: worker-build keeps
# its tools in the cache, and the browser and its config live under the home.
real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
caller_home=$HOME
caller_config=${XDG_CONFIG_HOME:-$HOME/.config}
# The real runtime dir, which t_init moves too.
short_tmp=${XDG_RUNTIME_DIR:-/tmp}
source "$(dirname -- "$0")/lib.sh"
t_init

if ! command -v playwright-cli >/dev/null 2>&1; then
	printf 'ok 1 # skip playwright-cli is not installed\n'
	_checks=1
	exit 0
fi

HUBWEB=$WF_ROOT/hubweb
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
	exit 1
fi

t_access_team

pids=()
session=t148-$$
# The daemon's socket lives under TMPDIR, and a socket path is capped at 108
# bytes, which a task's own TMPDIR can pass on its own.
pw() { HOME=$caller_home XDG_CONFIG_HOME=$caller_config XDG_CACHE_HOME=$real_cache TMPDIR=$short_tmp playwright-cli -s="$session" "$@"; }
stop_all() {
	local status=$?
	pw close >/dev/null 2>&1
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	kill "$team_pid" "$proxy_pid" 2>/dev/null
	for pid in "${pids[@]}" "$team_pid" "$proxy_pid"; do wait "$pid" 2>/dev/null; done
	(exit "$status")
	t_done
}
proxy_pid=""
trap stop_all EXIT

token=test-store-token
app=$T_TMP/hubweb
t_hubweb_copy "$app"
t_cf_dev "$app" "$T_TMP/hubweb.log" STORE_TOKEN=$token ACCESS_AUD="$aud" ACCESS_TEAM="$team"
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "cf dev answers on port $CF_DEV_PORT" "$(tail -40 "$T_TMP/hubweb.log")"
	exit 1
fi
share=ws://127.0.0.1:$CF_DEV_PORT/store/share

# The hub sandbox's four projects, sent to the store as a machine would.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(source "$WF_ROOT/hub/tests/seed.sh") >/dev/null
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "$share" "$token" "$XDG_DATA_HOME/mem/store" 2>&1)" \
	'^pushed [1-9][0-9]*$' "the seeded store's files reach the store"

# A question as a machine writes it: the seed's question file with a new id
# and text, in the same project, sent over the sharing socket.
seeded=$(grep -rl 'Which hour' "$XDG_DATA_HOME/mem/store/projects" | head -1)
items=$(dirname -- "$seeded")
ask() { # ask <suffix> <text> -- stores a new question; prints the ms it was sent at
	local id
	id=$(basename -- "$seeded" .md)
	id=${id%????}$1
	sed -e "s/^id = .*/id = \"$id\"/" -e '/^options = /d' -e '/^recommend = /d' \
		-e "s/Which hour does the nag go out?/$2/" "$seeded" >"$T_TMP/question.md"
	node "$WF_ROOT/tests/share-write.mjs" "$share" "$token" "${items#"$XDG_DATA_HOME/mem/store/"}/$id.md" \
		"$T_TMP/question.md" | sed -n 's/^sent //p'
}

node "$WF_ROOT/tests/jwt-proxy.mjs" "$CF_DEV_PORT" "$valid" >"$T_TMP/proxy.out" 2>&1 &
proxy_pid=$!
for _ in $(seq 50); do
	proxy_port=$(sed -n 's/^proxy //p' "$T_TMP/proxy.out")
	[ -n "$proxy_port" ] && break
	sleep 0.1
done
home=http://127.0.0.1:$proxy_port/

exec 9>"$short_tmp/hub-browser.lock"
flock 9
pw open >/dev/null

# browse <name> <body> -- runs the async body in the browser with `page`, the
# home's url as `home`, and prints the JSON it returns.
browse() {
	printf 'async (page) => { const home = %s;\n%s\n}\n' "\"$home\"" "$2" >"$T_TMP/$1.js"
	pw run-code --filename "$T_TMP/$1.js" | sed -n '/^### Result/{n;p}'
}
# shown <text> <ms> -- waits up to <ms> for <text> on the page; prints the ms
# it showed at, the page's navigation type and whether the window kept the
# marker set before (a page that only changed in place keeps it).
shown() {
	browse shown "
	let at = null;
	try {
	  await page.waitForFunction((t) => document.body.innerText.includes(t), '$1', { timeout: $2, polling: 50 });
	  at = Date.now();
	} catch {}
	return await page.evaluate((at) => ({ at, nav: performance.getEntriesByType('navigation')[0].type, marked: window.__marker === 1 }), at);
	"
}
# json <field> -- reads a field of the JSON on stdin
json() { python3 -c "import json, sys; print(json.load(sys.stdin)['$1'])"; }

# The answer box is the paired device's, so pair this browser first.
code=$(curl -s -X POST -H "Authorization: Bearer $token" "http://127.0.0.1:$CF_DEV_PORT/store/pair")
browse pair "
await page.goto(home + 'pair/$code');
await page.getByRole('button', { name: 'Pair' }).click();
await page.goto(home);
await page.evaluate(() => { window.__marker = 1; });
return await page.locator('textarea').count();
" >/dev/null

# 1. No field in use: a question written over the sharing socket shows by
# itself, within 2 s of the write, and the page reloaded to show it.
(sleep 1.5; ask ZA01 'Does the first live question show' >"$T_TMP/sent1") &
writer=$!
out=$(shown 'Does the first live question show' 8000)
wait "$writer"
at=$(json at <<<"$out")
sent=$(cat "$T_TMP/sent1")
is "$(json marked <<<"$out")" False "the page reloaded itself to show a new question"
is "$(json nav <<<"$out")" reload "the page's own navigation was a reload"
like "$at" '^[0-9]+$' "the new question shows on the open page"
echo "# shown $((at - sent)) ms after the write was sent"
[ "$((at - sent))" -lt 2000 ]
is $? 0 "the new question shows within 2 s of reaching the store"

# 2. Text in the answer box: the next question waits until the box is left.
browse type "
await page.evaluate(() => { window.__marker = 1; });
await page.locator('textarea').first().fill('half an answer');
return 1;
" >/dev/null
(sleep 0.5; ask ZA02 'Does the second live question wait' >"$T_TMP/sent2") &
writer=$!
out=$(shown 'Does the second live question wait' 3500)
wait "$writer"
is "$(json at <<<"$out")" None "a question written while a field holds text does not show"
is "$(json marked <<<"$out")" True "the page did not reload under the typed text"
is "$(browse kept "return await page.locator('textarea').first().inputValue();")" '"half an answer"' \
	"the typed text is still in the box"

browse leave "
await page.locator('textarea').first().fill('');
await page.evaluate(() => document.activeElement.blur());
return 1;
" >/dev/null
out=$(shown 'Does the second live question wait' 3000)
at=$(json at <<<"$out")
like "$at" '^[0-9]+$' "the question shows once the field is cleared and left"
is "$(json marked <<<"$out")" False "the page reloaded when the field was left"
is "$(json nav <<<"$out")" reload "the page's own navigation was a reload"

# 3. An idle page: count what it asks for. Listeners go on first and the page
# is reloaded under them, so its socket opens where they can see it; they
# count from a second after it settles. 60 s here, 10 minutes on the deploy.
out=$(browse idle "
let requests = [], frames = 0, sockets = 0;
page.on('request', (r) => requests.push(r.method() + ' ' + r.url()));
page.on('websocket', (ws) => { sockets++; ws.on('framesent', () => frames++); });
await page.reload();
await page.waitForTimeout(1000);
requests = []; frames = 0; sockets = 0;
await page.waitForTimeout(60000);
return { requests, frames, sockets };
")
is "$(json requests <<<"$out")" '[]' "an idle open page makes no HTTP request in 60 s"
is "$(json frames <<<"$out")" 0 "an idle open page sends no WebSocket frame in 60 s"
is "$(json sockets <<<"$out")" 0 "an idle open page opens no new WebSocket in 60 s"

