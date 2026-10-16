#!/usr/bin/env bash
# The first import under `cf dev`: `hub share seed` sends a machine's whole
# store, which shares nothing, into the store as imports, then its files.
#
#   - a first run sends every item, page, singleton and global item, and
#     each evidence and raw file, and exits 0;
#   - a second run sends nothing;
#   - writes made meanwhile (a log, a page edit, a cleared plan) go with the
#     next run, the page based on the store's head;
#   - an empty replica filled from the store then holds the same items and
#     pages as the machine that imported, file for file;
#   - the importing machine's outbox and heads are untouched.
#
# Needs what t153 needs: hubweb/node_modules, worker-build, the wasm target,
# python3, and mem's and the hub's release binaries, which tests/run.sh builds.

real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
HUB_BIN=${HUB_BIN:-$WF_ROOT/hub/target/release/hub}
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
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
app=$T_TMP/hubweb
t_hubweb_copy "$app"
t_cf_dev "$app" "$T_TMP/dev.log" STORE_TOKEN=$token
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "cf dev answers" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi
store="ws://127.0.0.1:$CF_DEV_PORT/store/share"

PROJECT=01K0SEEDWA7K00000000000000

on() { # on <machine> <command...> -- runs a command with that machine's store
	local root=$T_TMP/m/$1
	shift
	(cd "$T_TMP" && env HOME="$root" XDG_DATA_HOME="$root/data" XDG_CACHE_HOME="$root/cache" \
		XDG_STATE_HOME="$root/state" XDG_CONFIG_HOME="$root/config" \
		HUB_LOCAL_SOCKET="$root/state/hub/local.sock" MEM_SYNC_CMD=true "$@")
}
for m in mini fresh walker nuc; do
	mkdir -p "$T_TMP/m/$m"/{data,cache,state,config/qshell}
	printf '%s\n' "$m" >"$T_TMP/m/$m/config/qshell/machine"
done
top=$T_TMP/m/mini/data/mem/store
mkdir -p "$top/projects/$PROJECT/items" "$top/projects/$PROJECT/evidence/m1" "$top/projects/$PROJECT/raw"
cat >"$top/projects/$PROJECT/project.toml" <<TOML
created = "2026-10-16T00:00:00Z"
id = "$PROJECT"
name = "walk"
TOML
for n in 1 2 3; do on mini mem --project walk log "log $n from the VPS days" >/dev/null; done
on mini mem --project walk handoff --set "Next: m2." >/dev/null
printf '# plan: m1\n\n- [ ] t1 one\n' | on mini mem --project walk plan --stdin >/dev/null
printf '# Lessons\n\nfirst\n' | on mini mem --project walk wiki lessons --stdin --note first >/dev/null
on mini mem save "a standing rule" >/dev/null
printf 'png bytes' >"$top/projects/$PROJECT/evidence/m1/shot.png"
printf 'old bytes' >"$top/projects/$PROJECT/raw/old.bin"
touch -d '100 days ago' "$top/projects/$PROJECT/raw/old.bin"
like "$(find "$top/global/items" -name '*.md' | wc -l)" '^1$' "the rule is a global item"

seed() { on mini env HUB_STORE_URL="$store" HUB_STORE_TOKEN=$token HUB_STORE_NAME=main timeout 60 "$HUB_BIN" share seed; }
out=$(seed 2>"$T_TMP/seed1.err")
is "$?" 0 "the first import exits 0"
texts=$(find "$top" -type f ! -path '*/evidence/*' ! -path '*/raw/*' ! -name VERSION | wc -l)
like "$out" "sent $texts of $texts writes \(0 refused, 0 errors\), uploaded 2 of 2 files" \
	"it sends every text file and both files, the old one too"
unlike "$(on mini mem doctor)" 'outbox [1-9]' "the importing machine's outbox stays empty"

out=$(seed 2>>"$T_TMP/seed1.err")
is "$?" 0 "a second import exits 0"
like "$out" "sent 0 of 0 writes" "and sends nothing"

on mini mem --project walk log "a log the VPS took meanwhile" >/dev/null
printf '# Lessons\n\nfirst\nsecond\n' | on mini mem --project walk wiki lessons --stdin --note second >/dev/null
on mini mem --project walk plan --clear >/dev/null
out=$(seed 2>>"$T_TMP/seed1.err")
is "$?" 0 "the import after more writes exits 0"
like "$out" 'sent ([3-9]) of \1 writes \(0 refused' "it sends the new log, the edited page and the cleared plan"

on fresh env HUB_STORE_URL="$store" HUB_STORE_TOKEN=$token HUB_STORE_NAME=main timeout 30 "$HUB_BIN" share sync 2>/dev/null
fresh=$T_TMP/m/fresh/data/mem/store
listing() { (cd "$1" && find . -type f ! -path '*/evidence/*' ! -path '*/raw/*' ! -name VERSION | sort); }
is "$(listing "$fresh")" "$(listing "$top")" "an empty replica filled from the store holds the same text files"
if diff -r -x VERSION -x evidence -x raw "$top" "$fresh" >"$T_TMP/diff" 2>&1; then
	ok "and each holds the same bytes"
else
	notok "and each holds the same bytes" "$(head -20 "$T_TMP/diff")"
fi
counts() { on "$1" mem doctor | sed -n 's/^counts *\(items [0-9]*, pages [0-9]*\).*/\1/p'; }
is "$(counts fresh)" "$(counts mini)" "mem doctor counts the same items and pages on both"
is "$(curl -s -H "Authorization: Bearer $token" -H 'X-Hub-Store: main' \
	"http://127.0.0.1:$CF_DEV_PORT/store/files/projects/$PROJECT/evidence/m1/shot.png")" "png bytes" \
	"the file is in the store byte for byte"
is "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" -H 'X-Hub-Store: main' \
	"http://127.0.0.1:$CF_DEV_PORT/store/files/projects/$PROJECT/raw/old.bin")" 200 \
	"and so is the file older than 90 days"
files_at() {
	curl -s -H "Authorization: Bearer $token" -H 'X-Hub-Store: main' \
		"http://127.0.0.1:$CF_DEV_PORT/store/files?project=$PROJECT"
}
before=$(files_at)
like "$before" '"at":[0-9]+' "the store lists the file with its upload time"
cp -a "$top" "$T_TMP/m/nuc/data/mem/"
out=$(on nuc env HUB_STORE_URL="$store" HUB_STORE_TOKEN=$token HUB_STORE_NAME=main timeout 60 "$HUB_BIN" share seed 2>>"$T_TMP/seed1.err")
is "$?" 0 "a second machine holding the same store imports and exits 0"
like "$out" "sent 0 of 0 writes" "it sends no write the first machine sent"
is "$(files_at)" "$before" "and puts no file again, so its upload time stands"

on walker env HUB_STORE_URL="$store" HUB_STORE_TOKEN=$token HUB_STORE_NAME=walk timeout 30 "$HUB_BIN" share sync 2>/dev/null
is "$(find "$T_TMP/m/walker/data/mem/store" -type f 2>/dev/null | wc -l)" 0 \
	"the walk's store object holds none of the import"
unlike "$(cat "$T_TMP/seed1.err")" 'error|refused' "no import printed an error"
