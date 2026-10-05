#!/usr/bin/env bash
# The hub over hub/tests/sandbox.sh's seeded store, in a browser at phone
# width: every seeded page answers 200; the Show path walks in one browser,
# approving beta's draft roadmap, answering gamma's question, opening its
# screenshot and following a wiki section hit; no page is wider than the
# phone; and `shot` leaves a phone-width PNG behind and nothing running.
# With no playwright-cli the walk runs over curl and the rest is skipped.
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

## ------------------------------------------------------------ the walk

# One browser at a time on this machine, the same lock sandbox.sh's `shot`
# takes: the suite and two evidence captures would otherwise run three.
base=${url%/}
shots="$T_TMP/shots"
mkdir -p "$shots" "$T_TMP/pw"
browser=""
command -v playwright-cli >/dev/null 2>&1 && [ -n "$url" ] && browser=1
short_tmp=${XDG_RUNTIME_DIR:-/tmp}
session=hub-walk-$$
# The browser keeps its config under the caller's HOME, its socket under a
# TMPDIR short enough for a socket path, and its snapshots in its cwd.
pw() {
	(cd "$T_TMP/pw" && env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_STATE_HOME \
		HOME="$real_home" TMPDIR="$short_tmp" playwright-cli -s="$session" "$@" 2>/dev/null)
}
# js <expression>: its value in the open page, as text.
js() { pw --raw eval "$1" | jq -r 'if type == "string" then . else tostring end' 2>/dev/null; }
# post <path> <form>: the status of a same-origin form POST.
post() { curl -s -o /dev/null -w '%{http_code}' --max-time 30 -H "Origin: $base" --data "$2" "$base$1"; }
get() { curl -s --max-time 30 "$base$1"; }
if [ -n "$browser" ]; then
	printf '# shots: %s\n' "$shots"
	exec 9>"$short_tmp/hub-browser.lock"
	flock 9
	trap 'st=$?; pw close >/dev/null; (exit $st); t_done' EXIT
	pw open "$base/" >/dev/null
	pw resize 390 844 >/dev/null
fi

# 1. Approve beta's draft roadmap.
if [ -n "$browser" ]; then
	pw goto "$base/p/beta/roadmap" >/dev/null
	pw click 'role=button[name="Approve"]' >/dev/null
	roadmap=$(js 'document.documentElement.outerHTML')
	pw screenshot --filename "$shots/step-1.png" >/dev/null
else
	post /p/beta/control do=approve >/dev/null
	roadmap=$(get /p/beta/roadmap)
fi
like "$roadmap" 'approved: sent, waiting for the engine' 'Approve leaves beta sent, waiting for the engine'

# 2. Answer gamma's question with the recommended option.
if [ -n "$browser" ]; then
	pw goto "$base/p/gamma/questions" >/dev/null
	pw click 'button.recommended' >/dev/null
	questions=$(js 'document.documentElement.outerHTML')
	pw screenshot --filename "$shots/step-2.png" >/dev/null
else
	id=$(get /p/gamma/questions | grep -B3 'class="recommended"' | sed -n 's/.*name="id" value="\([^"]*\)".*/\1/p')
	post /answer "id=$id&project=gamma&text=nine" >/dev/null
	questions=$(get /p/gamma/questions)
fi
answered=$(printf '%s\n' "$questions" | sed -n '/<h2>Answered<\/h2>/,$p' | tr -d '\n')
like "$answered" 'Which hour does the nag go out\?</p><p class="meta">answer: nine</p>' \
	'the recommended option answers the question, listed under answered'

# 3. Open gamma's screenshot from its evidence.
if [ -n "$browser" ]; then
	pw goto "$base/p/gamma/evidence" >/dev/null
	pw click 'a:has(img[alt$="week.png"])' >/dev/null
	type=$(js 'document.contentType')
	pw screenshot --filename "$shots/step-3.png" >/dev/null
else
	href=$(get /p/gamma/evidence | sed -n 's/.*<a href="\([^"]*\)"><img [^>]*alt="[^"]*week\.png".*/\1/p' | head -n 1)
	type=$(curl -s -o /dev/null -w '%{content_type}' --max-time 30 "$base$href")
fi
is "$type" image/png 'the evidence opens its screenshot as image/png'

# 4. Search gamma's wiki and follow the section hit to its heading.
if [ -n "$browser" ]; then
	pw goto "$base/p/gamma/wiki" >/dev/null
	pw fill 'input[name=q]' quiet >/dev/null
	pw press Enter >/dev/null
	pw click 'a[href*="#"]' >/dev/null
	hit=$(js 'location.hash + " " + (document.querySelector(":target:is(h1,h2,h3,h4,h5,h6)")?.textContent ?? "")')
	pw screenshot --filename "$shots/step-4.png" >/dev/null
else
	href=$(get '/p/gamma/wiki?q=quiet' | sed -n 's/.*<li><a href="\([^"]*#[^"]*\)".*/\1/p' | head -n 1)
	anchor=${href#*#}
	heading=$(get "${href%%#*}" | sed -n "s/.*<h[1-6] id=\"$anchor\">\([^<]*\)<.*/\1/p")
	hit="#$anchor $heading"
fi
is "$hit" '#quiet-hours Quiet hours' 'the search hit leads to its section heading'

if [ -n "$browser" ]; then
	shot_widths=""
	for n in 1 2 3 4; do
		shot_widths+="$(file -b "$shots/step-$n.png" 2>/dev/null | sed -n 's/^PNG image data, \([0-9]*\) x .*/\1/p') "
	done
	is "$shot_widths" '390 390 390 390 ' 'each step leaves a 390 px screenshot'

	# The phone's width: no page scrolls sideways.
	wide=""
	widths=(/ /p/gamma /wiki/gamma/spec)
	for page in roadmap run questions evidence wiki decisions new; do
		widths+=("/p/gamma/$page")
	done
	for page in "${widths[@]}"; do
		pw goto "$base$page" >/dev/null
		w=$(js 'document.documentElement.scrollWidth')
		[ -n "$w" ] && [ "$w" -le 390 ] || wide+="$page ${w:-none}"$'\n'
	done
	is "$wide" "" "no page is wider than 390 px (${#widths[@]} pages)"

	# The seeded answer is one word longer than the phone's line, and it
	# wraps inside its box rather than widening the page.
	pw goto "$base/p/gamma/questions" >/dev/null
	answered=$(js 'document.documentElement.outerHTML' | sed -n '/<h2>Answered<\/h2>/,$p' | tr -d '\n')
	w=$(js 'document.documentElement.scrollWidth')
	fit="${w:-none}"
	case $answered in
	*the_streak_kept_every_single_day_from_the_first_tick_to_the_last_without_a_break*)
		[ -n "$w" ] && [ "$w" -le 390 ] && fit=fits ;;
	*) fit="no long answer, width ${w:-none}" ;;
	esac
	is "$fit" fits 'a long unbroken answer wraps on the questions page at 390 px'

	# The gallery's lazy image widens the page only once it has arrived, so
	# the width is read after it decodes.
	pw goto "$base/p/gamma/evidence" >/dev/null
	fit=$(js 'async () => { const i = document.querySelector("img[alt$=\"wide.png\"]"); await i.decode(); return i.naturalWidth + " " + document.documentElement.scrollWidth; }')
	natural=${fit%% *}
	w=${fit##* }
	[ "$natural" = 1280 ] && [ -n "$w" ] && [ "$w" -le 390 ] && fit=fits
	is "$fit" fits 'the 1280 px screenshot fits the evidence page at 390 px'
	pw close >/dev/null
	trap t_done EXIT
	exec 9>&-
else
	printf '# skip: playwright-cli is not installed\n'
fi

kill "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
is "$( [ -n "$root" ] && [ -e "$root" ] && echo left || echo gone)" gone 'killed, the sandbox removes its directory'
port=${url##*:}
port=${port%/}
code=$([ -n "$port" ] && curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$url")
is "$code" 000 'and its hub is no longer listening'

[ -n "$browser" ] || exit 0

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
sessions=$(env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_STATE_HOME \
	HOME="$real_home" playwright-cli list 2>/dev/null)
unlike "$sessions" 'hub-sandbox-' 'and no browser session of its own'
