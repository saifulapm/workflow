#!/usr/bin/env bash
# What a paired browser writes on the hub web under `cf dev` lands in the
# store as mem would write it: Approve and Request changes on a draft
# roadmap, Pause and Unpause, the Idea and Brief forms, and a finding with
# a 4 MB phone photo. An unpaired browser's POSTs get 403 and change
# nothing.
#
# Needs what t140 needs: hubweb/node_modules, worker-build, the wasm
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
t_hubweb_copy "$T_TMP/hubweb"
t_cf_dev "$T_TMP/hubweb" "$T_TMP/dev.log" STORE_TOKEN=$token ACCESS_AUD="$aud" ACCESS_TEAM="$team"
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "cf dev answers on port $CF_DEV_PORT" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
base=http://127.0.0.1:$CF_DEV_PORT
printf '%s\n' "$(with_jwt "$valid")" >"$T_TMP/jwt.hdr"

# The hub sandbox's projects plus epsilon, a second draft roadmap with its
# review question, so Approve and Request changes each have one to act on.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(
	source "$WF_ROOT/hub/tests/seed.sh"
	project epsilon
	mem brief --set 'A seed packet that counts its own days.' >/dev/null
	mem roadmap --stdin >/dev/null <<'ROAD'
# roadmap: epsilon

- [ ] e1-count Days are counted
      Show: a packet opened today counts one
ROAD
	mem roadmap --status draft >/dev/null
	mem ask --for human --options approve,changes --recommend approve 'Review the epsilon roadmap' >/dev/null
) >/dev/null
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$CF_DEV_PORT/store/share" "$token" \
	"$XDG_DATA_HOME/mem/store" "$base" 2>&1)" '^pushed [1-9][0-9]*$' "the seeded store reaches the store"

code=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/pair")
cookie=$(curl -s -X POST -H "@$T_TMP/jwt.hdr" -H 'Sec-Fetch-Site: same-origin' -D - -o /dev/null \
	"$base/pair/$code" | tr -d '\r' | sed -n 's/^[Ss]et-[Cc]ookie: \(hub_device=[^;]*\);.*/\1/p')
like "$cookie" '^hub_device=' "a browser pairs"

get() { curl -s -H "@$T_TMP/jwt.hdr" "$base$1" | tr -d '\n'; }
post() { # post <path> [curl args...] -- paired, from the page itself; prints the status
	local path=$1
	shift
	curl -s -X POST -H "@$T_TMP/jwt.hdr" -H "Cookie: $cookie" -H 'Sec-Fetch-Site: same-origin' \
		-o "$T_TMP/post.out" -w '%{http_code}' "$@" "$base$path"
}
control() { # control <project> <do> [text]
	post "/p/$1/control" --data-urlencode "do=$2" --data-urlencode "text=${3:-}"
}
status_of() { get "/p/$1/roadmap" | grep -oE 'status: (draft|approved)' | head -1 | cut -d' ' -f2; }

# 1. Approve
is "$(status_of beta)" draft "beta's roadmap starts as a draft"
like "$(get /p/beta/questions)" 'Review the beta roadmap' "the review question is listed"
is "$(control beta approve)" 303 "Approve on a draft is taken"
is "$(status_of beta)" approved "Approve makes the roadmap approved"
after=$(get /p/beta/questions)
like "$after" 'Review the beta roadmap.*approve' "Approve answers the review question approve"
unlike "$(get /)" 'Review the beta roadmap' "the answered review question no longer waits"
isnt "$(control beta approve)" 303 "a second Approve is refused, the roadmap is not a draft"
like "$(cat "$T_TMP/post.out")" 'not a draft' "the refusal says so"
is "$(get /p/beta/questions)" "$after" "the second Approve wrote nothing"

# 2. Request changes
is "$(control epsilon changes)" 303 "Request changes with no text goes back to the roadmap"
like "$(get /p/epsilon/questions)" 'Review the epsilon roadmap' "an empty Request changes answers nothing"
is "$(control epsilon changes 'Split e1 in two')" 303 "Request changes with a text is taken"
like "$(get /p/epsilon/questions)" 'changes: Split e1 in two' "Request changes answers changes: <text>"
is "$(status_of epsilon)" draft "Request changes leaves the roadmap a draft"

# 3. Pause and Unpause
is "$(control gamma pause)" 303 "Pause is taken"
gamma=$(get /p/gamma)
like "$gamma" 'class="pill wait">paused</span> <span class="meta">20[0-9-]+T[0-9:]+Z by hub-web<' \
	"gamma's page shows it paused, by hub-web, with the time"
is "$(control gamma unpause)" 303 "Unpause is taken"
unlike "$(get /p/gamma)" 'class="pill wait">(pausing|paused)<' "gamma's page no longer shows it paused"

# 4. Idea and Brief: delta is done, so its page lists ideas; alpha has only
# a brief, so its page shows it.
is "$(post /p/delta/new/idea --data-urlencode 'text=Rename videos by their date too')" 303 "the Idea form is taken"
like "$(get /p/delta)" '<li>Rename videos by their date too</li>' "the project page lists the idea"
is "$(post /p/alpha/new/brief --data-urlencode 'text=A shared list that two phones keep in step, offline too.')" 303 \
	"the Brief form is taken"
like "$(get /p/alpha)" 'A shared list that two phones keep in step, offline too\.' "the project page shows the new brief"
unlike "$(get /p/alpha)" 'A shared shopping list two phones keep in step\.' "the old brief is gone"

# 5. A finding with a 4 MB phone photo
python3 -c 'import os, sys; sys.stdout.buffer.write(b"\xff\xd8\xff\xe0" + os.urandom(4 * 1024 * 1024 - 4))' >"$T_TMP/phone.jpg"
want=$(sha256sum <"$T_TMP/phone.jpg" | cut -c1-64)
is "$(stat -c %s "$T_TMP/phone.jpg")" 4194304 "the test photo is 4 MB"
is "$(post /p/gamma/new/finding -F milestone=g2-nag -F step=2 -F 'text=the nag sound clips on the phone' \
	-F "photo=@$T_TMP/phone.jpg;type=image/jpeg;filename=phone.jpg")" 303 "the Finding form with the photo is taken"
evidence=$(get /p/gamma/evidence)
like "$evidence" 'the nag sound clips on the phone' "the evidence page lists the finding"
src=$(grep -oE '<img src="[^"]*"' <<<"${evidence#*the nag sound clips on the phone}" | head -1 | sed 's/<img src="\(.*\)"/\1/')
like "$src" '^/p/gamma/file/' "the finding shows its image"
curl -s -H "@$T_TMP/jwt.hdr" -D "$T_TMP/file.hdr" -o "$T_TMP/got.jpg" "$base$src"
like "$(tr -d '\r' <"$T_TMP/file.hdr")" '^HTTP/[0-9.]+ 200' "GET the finding's file answers 200"
is "$(stat -c %s "$T_TMP/got.jpg")" 4194304 "the file is 4 MB"
is "$(sha256sum <"$T_TMP/got.jpg" | cut -c1-64)" "$want" "the file's sha256 is the photo's"

# 6. Unpaired POSTs get 403 and write nothing
# Ages like "44s" move with the clock, so they are left out of the comparison.
snap() { { get /; get /p/gamma/evidence; get /p/gamma; get /p/delta; get /p/alpha; get /p/epsilon/questions; } | sed -E 's/(>| · )[0-9]+[smhd](<| · )/\1\2/g'; }
before=$(snap)
unpaired() { # unpaired <path> [curl args...] -- prints the status
	local path=$1
	shift
	curl -s -X POST -H "@$T_TMP/jwt.hdr" -H 'Sec-Fetch-Site: same-origin' -o /dev/null -w '%{http_code}' "$@" "$base$path"
}
is "$(unpaired /p/epsilon/control --data-urlencode do=approve)" 403 "an unpaired Approve is refused"
is "$(unpaired /p/epsilon/control --data-urlencode do=changes --data-urlencode text=no)" 403 "an unpaired Request changes is refused"
is "$(unpaired /p/gamma/control --data-urlencode do=pause)" 403 "an unpaired Pause is refused"
is "$(unpaired /p/gamma/new/idea --data-urlencode text=sneaky)" 403 "an unpaired Idea is refused"
is "$(unpaired /p/gamma/new/brief --data-urlencode text=sneaky)" 403 "an unpaired Brief is refused"
is "$(unpaired /p/gamma/new/finding -F milestone=g2-nag -F step=2 -F text=sneaky \
	-F "photo=@$T_TMP/phone.jpg;type=image/jpeg;filename=phone.jpg")" 403 "an unpaired Finding is refused"
is "$(snap | sha256sum)" "$(printf %s "$before" | sha256sum)" "the home, the evidence page and the rest are unchanged"
