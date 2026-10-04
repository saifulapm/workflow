#!/usr/bin/env bash
# hub/tests/sandbox.sh: the hub built from this checkout, over a seeded
# throwaway store. Every page the seed gives something to show answers 200,
# and `shot` leaves a phone-width PNG behind and nothing running.
source "$(dirname -- "$0")/lib.sh"
# The browser lives under the caller's HOME, which t_init replaces.
real_home=$HOME
t_init

sandbox="$WF_ROOT/hub/tests/sandbox.sh"
out="$T_TMP/sandbox.out"

bash "$sandbox" >"$out" 2>"$T_TMP/sandbox.err" &
pid=$!
url=""
for _ in $(seq 600); do
	url=$(sed -n '1s/^sandbox \(http:\/\/127\.0\.0\.1:[0-9]*\/\)$/\1/p' "$out")
	[ -n "$url" ] && break
	kill -0 "$pid" 2>/dev/null || break
	sleep 0.5
done
like "$(head -n 1 "$out")" '^sandbox http://127\.0\.0\.1:[0-9]+/$' 'the first line is the sandbox URL'

# The pages under /p/<project>/ the nav links to, for every project; the
# wiki pages gamma was seeded with; the evidence file it filed.
pages=(/ /wiki /wiki/gamma/index /wiki/gamma/spec /wiki/gamma/research-summary)
for p in alpha beta gamma delta; do
	pages+=("/p/$p")
	for page in roadmap run questions evidence wiki decisions new; do
		pages+=("/p/$p/$page")
	done
done
store_env() { HOME=$1/home XDG_DATA_HOME=$1/home/.local/share XDG_CONFIG_HOME=$1/home/.config XDG_STATE_HOME=$1/home/.local/state XDG_CACHE_HOME=$1/home/.cache "${@:2}"; }
root=$(sed -n 's/^store \(.*\)$/\1/p' "$out" | head -n 1)
if [ -n "$root" ]; then
	id=$(store_env "$root" "$MEM_BIN" --project gamma evidence list --json 2>/dev/null | jq -r '.items[0].id // empty')
	pages+=("/p/gamma/file/$id")
	isnt "$id" "" 'gamma has its picture filed as evidence'
fi
bad=""
if [ -n "$url" ]; then
	for page in "${pages[@]}"; do
		code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 "${url%/}$page")
		[ "$code" = 200 ] || bad+="$page $code"$'\n'
	done
fi
is "$bad" "" "every seeded page answers 200 (${#pages[@]} pages)"
code=$([ -n "$url" ] && curl -s -o /dev/null -w '%{http_code}' --max-time 30 "${url%/}/p/omega")
is "$code" 404 'and a project the seed never made does not'

# The seed itself, read back through mem in the sandbox's own store.
if [ -n "$root" ]; then
	is "$(store_env "$root" "$MEM_BIN" --project beta roadmap --status 2>/dev/null)" draft 'beta has a draft roadmap'
	like "$(store_env "$root" "$MEM_BIN" --project beta questions 2>/dev/null)" 'Approve roadmap beta\?' 'and the pending approval question'
	like "$(store_env "$root" "$MEM_BIN" --project gamma log --type run 2>/dev/null)" 'dogfood ' 'gamma carries dogfood run lines'
	like "$(store_env "$root" "$MEM_BIN" --project delta roadmap --status 2>/dev/null)" maintenance 'delta is in maintenance'
fi

kill "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
is "$( [ -n "$root" ] && [ -e "$root" ] && echo left || echo gone)" gone 'killed, the sandbox removes its directory'
port=${url##*:}
port=${port%/}
code=$([ -n "$port" ] && curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$url")
is "$code" 000 'and its hub is no longer listening'

if ! command -v playwright-cli >/dev/null 2>&1; then
	printf '# skip: playwright-cli is not installed\n'
	exit 0
fi

# The browser and its config sit under the caller's own HOME and XDG roots,
# not the ones t_init made; TMPDIR is this test's so what is left is visible.
png="$T_TMP/home.png"
mkdir -p "$T_TMP/tmp"
run_out env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_STATE_HOME \
	HOME="$real_home" TMPDIR="$T_TMP/tmp" bash "$sandbox" shot / "$png"
is "$RC" 0 'shot / exits 0'
is "$(file -b "$png" 2>/dev/null | sed -n 's/^PNG image data, \([0-9]*\) x .*/\1/p')" 390 'and leaves a PNG 390 pixels wide'
url=$(printf '%s\n' "$OUT" | sed -n '1s/^sandbox //p')
like "$url" '^http://127\.0\.0\.1:[0-9]+/$' 'shot names the sandbox it took'
is "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$url")" 000 'whose hub is gone once shot returns'
is "$(pgrep -f -- "$T_TMP/tmp" | tr '\n' ' ')" "" 'with no process left running from its directory'
is "$(ls -A "$T_TMP/tmp")" "" 'and the directory empty'
