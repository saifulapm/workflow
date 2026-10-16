#!/usr/bin/env bash
# The one-time move of every file into R2, under `cf dev`. The build before
# R2 (6fe2e3b) keeps two projects' evidence and a built binary as rows of
# their Files objects; the new build on the same state serves them from
# those rows, then `hub share move` has each Files object copy its files
# into R2, checking each copy's length and SHA-256 before its rows go.
# After the move each file reads the same, a binary put again since the new
# build keeps its newer copy, a file put again with the same bytes counts as
# moved, and a second move finds nothing left. Only the
# shared bearer moves.
#
# Needs what t143 needs, and git: the old build is hubweb and hubpages as
# they were at 6fe2e3b.

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

pids=()
stop_all() {
	local status=$?
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	wait 2>/dev/null
	(exit "$status")
	t_done
}
trap stop_all EXIT

token=test-store-token
P1=01K0M0VE000000000000000001
P2=01K0M0VE000000000000000002
put() { # put <file> <path> -- prints the status
	curl -s -o /dev/null -w '%{http_code}' --max-time 60 -X PUT --data-binary "@$1" \
		-H "Authorization: Bearer $token" "$base$2"
}
sha_of() { # sha_of <path> -- the sha256 of what a GET of the path gives
	curl -s --max-time 60 -H "Authorization: Bearer $token" "$base$1" | sha256sum | cut -d' ' -f1
}
sha() { sha256sum "$1" | cut -d' ' -f1; }

head -c 3000000 /dev/urandom >"$T_TMP/shot.png"
: >"$T_TMP/empty.txt"
echo "a note" >"$T_TMP/note.txt"
head -c 1500000 /dev/urandom >"$T_TMP/mem.old"
head -c 1200000 /dev/urandom >"$T_TMP/mem.new"
declare -A file=(
	["/store/files/projects/$P1/evidence/t1/shot.png"]=$T_TMP/shot.png
	["/store/files/projects/$P1/raw/empty.txt"]=$T_TMP/empty.txt
	["/store/files/projects/$P2/evidence/t2/a%20note.txt"]=$T_TMP/note.txt
)

old=$T_TMP/old
mkdir -p "$old"
git -C "$WF_ROOT" archive 6fe2e3b hubweb hubpages workflow/src/cloud-prompt.txt | tar -x -C "$old"
ln -s "$HUBWEB/node_modules" "$old/hubweb/node_modules"
if ! t_cf_dev "$old/hubweb" "$T_TMP/old.log" STORE_TOKEN=$token; then
	notok "the old build answers under cf dev" "$(tail -40 "$T_TMP/old.log")"
	exit 1
fi
base=http://127.0.0.1:$CF_DEV_PORT
stored=0
for path in "${!file[@]}"; do
	[ "$(put "${file[$path]}" "$path")" = 204 ] && stored=$((stored + 1))
done
[ "$(put "$T_TMP/mem.old" /store/dl/mem-x86_64)" = 204 ] && stored=$((stored + 1))
is "$stored" 4 "the old build keeps three files and a binary in its Files objects"
kill -TERM -- "-$CF_DEV_PID" 2>/dev/null
wait "$CF_DEV_PID" 2>/dev/null

app=$T_TMP/new/hubweb
t_hubweb_copy "$app"
cp -r "$old/hubweb/.wrangler" "$app/"
t_cf_dev "$app" "$T_TMP/new.log" STORE_TOKEN=$token
up=$?
pids+=("$CF_DEV_PID")
base=http://127.0.0.1:$CF_DEV_PORT
if [ "$up" != 0 ]; then
	notok "the new build answers under cf dev" "$(tail -40 "$T_TMP/new.log")"
	exit 1
fi

same=0
for path in "${!file[@]}"; do
	[ "$(sha_of "$path")" = "$(sha "${file[$path]}")" ] && same=$((same + 1))
done
is "$same" 3 "before the move, the new build reads each file from its Files object"
is "$(sha_of /store/dl/mem-x86_64)" "$(sha "$T_TMP/mem.old")" "and the binary"
is "$(put "$T_TMP/mem.new" /store/dl/mem-x86_64)" 204 "a newer binary goes to R2"
is "$(put "$T_TMP/shot.png" "/store/files/projects/$P1/evidence/t1/shot.png")" 204 \
	"the same shot put again goes to R2, where it counts as moved"

# The store has no items of these projects, so the move learns them from
# this replica's project folders.
mkdir -p "$T_TMP/replica/mem/store/projects/"{$P1,$P2}
move() { # move [VAR=value...] -- runs hub share move against the store
	env -u MEM_REMOTE "$@" XDG_DATA_HOME="$T_TMP/replica" HUB_STORE_URL="ws://127.0.0.1:$CF_DEV_PORT/store/share" \
		timeout 120 "$HUB_BIN" share move 2>"$T_TMP/move.err"
}
minted=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/machines/nuc")
out=$(move HUB_STORE_TOKEN="$minted")
isnt "$?" 0 "hub share move with a machine's own token fails"
is "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$base/store/move?project=$P1")" 401 \
	"a move with no bearer is 401"

out=$(move HUB_STORE_TOKEN=$token)
is "$?" 0 "hub share move exits 0"
is "$out" "moved 3 files into R2, 1 kept its newer copy, none left" "and says what it moved" ||
	sed 's/^/# /' "$T_TMP/move.err"

same=0
for path in "${!file[@]}"; do
	[ "$(sha_of "$path")" = "$(sha "${file[$path]}")" ] && same=$((same + 1))
done
is "$same" 3 "after the move, each file reads the same"
is "$(sha_of /store/dl/mem-x86_64)" "$(sha "$T_TMP/mem.new")" "the binary keeps its newer copy"
whole=$(find "$app/.wrangler/state" -path '*r2*' -type f -size 3000000c -exec sha256sum {} + |
	cut -d' ' -f1)
is "$whole" "$(sha "$T_TMP/shot.png")" "the bucket holds the 3 MB shot as one whole object"

out=$(move HUB_STORE_TOKEN=$token)
is "$out" "moved 0 files into R2, 0 kept its newer copy, none left" "a second move finds nothing left"
