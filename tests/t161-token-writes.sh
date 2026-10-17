#!/usr/bin/env bash
# The store ties each write to the token it came through, under `cf dev`,
# with real replicas: mac-mini-m2 and nuc, each mem and `hub local` on its
# own machine token, sharing one project and the global items.
#
#   - a token the build before machine names (9a7b41e) minted still reports
#     as its runner on the new build over the same Durable Object, but its
#     writes get an error naming `hub share mint` and nothing is stored;
#   - the nuc's token cannot write a global rule, nor an item naming
#     another machine: both are refused and never reach the mini;
#   - a global rule the mini writes with mem reaches the nuc within 2 s, and
#     the store's record of it names mac-mini-m2;
#   - the cloud's token writes as the cloud session its hello names, and
#     only as it;
#   - a cloud session's .mem-outbox.json imported on the nuc goes out with
#     the nuc's token and reaches the mini still naming the cloud session.
#
# Needs what t142 needs, and git: the old build is hubweb and hubpages as
# they were at 9a7b41e.

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

free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
now_ms() { date +%s%3N; }

pids=()
declare -A hub_pid
stop_all() {
	local status=$?
	for name in "${!hub_pid[@]}"; do kill "${hub_pid[$name]}" 2>/dev/null; done
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	wait 2>/dev/null
	(exit "$status")
	t_done
}
trap stop_all EXIT

token=test-store-token
PROJECT=01K0T161PR0JECT00000000000

# The build before machine names mints nuc's token, for one name only.
old=$T_TMP/old
mkdir -p "$old"
git -C "$WF_ROOT" archive 9a7b41e hubweb hubpages workflow/src/cloud-prompt.txt | tar -x -C "$old"
ln -s "$HUBWEB/node_modules" "$old/hubweb/node_modules"
if ! t_cf_dev "$old/hubweb" "$T_TMP/old.log" STORE_TOKEN=$token; then
	notok "the old build answers under cf dev" "$(tail -40 "$T_TMP/old.log")"
	exit 1
fi
pids+=("$CF_DEV_PID")
early=$(curl -s -X POST -H "Authorization: Bearer $token" "http://127.0.0.1:$CF_DEV_PORT/store/machines/nuc")
like "$early" '^[0-9a-f]{60}$' "the old build mints nuc's token for its runner alone"
kill -TERM -- "-$CF_DEV_PID" 2>/dev/null
wait "$CF_DEV_PID" 2>/dev/null

app=$T_TMP/new/hubweb
t_hubweb_copy "$app"
cp -r "$old/hubweb/.wrangler" "$app/"
t_cf_dev "$app" "$T_TMP/new.log" STORE_TOKEN=$token
up=$?
pids+=("$CF_DEV_PID")
if [ "$up" != 0 ]; then
	notok "the new build answers under cf dev" "$(tail -40 "$T_TMP/new.log")"
	exit 1
fi
base=http://127.0.0.1:$CF_DEV_PORT
share=ws://127.0.0.1:$CF_DEV_PORT/store/share

# say <bearer> <hello machine> <frame>... -- says hello since 0, sends each
# frame once ready, and prints every reply but routes, the replayed recs
# included. Ends once each frame is answered, or 2 s after ready.
cat >"$T_TMP/say.mjs" <<'JS'
const [url, bearer, machine, ...frames] = process.argv.slice(2);
const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${bearer}` } });
ws.onerror = () => { console.error("could not connect"); process.exit(1); };
ws.onopen = () => ws.send(JSON.stringify({ t: "hello", machine, since: 0 }));
let answers = 0;
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.t === "route") return;
  console.log(e.data);
  if (m.t === "ready") {
    if (!frames.length) process.exit(0);
    for (const frame of frames) ws.send(frame);
    setTimeout(() => process.exit(0), 2000);
  } else if (["ack", "refused", "error", "budget"].includes(m.t) && ++answers === frames.length) {
    process.exit(0);
  }
};
JS
say() { node "$T_TMP/say.mjs" "$share" "$@"; }
# answer <output> <write id> -- the store's answer to that write, as `t why`
answer() {
	python3 -c '
import json, sys
for line in sys.argv[1].splitlines():
    m = json.loads(line)
    if m.get("id") == sys.argv[2] and m["t"] != "rec":
        print(m["t"], m.get("why", ""))
' "$1" "$2"
}
# stored_by <path> -- the machine of each rec the store holds for <path>
stored_by() {
	say "$token" seed | python3 -c '
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m["t"] == "rec" and m["path"] == sys.argv[1]:
        print(m.get("machine"))
' "$1"
}
frame() { # frame <write id> <path> <body> -- a write frame
	python3 -c 'import json, sys; print(json.dumps({"t": "write", "id": sys.argv[1], "path": sys.argv[2], "base": None, "body": sys.argv[3]}))' "$@"
}
item() { # item <ulid> <machine> <text> -- an item's file
	printf '+++\ncreated = "2026-10-17T00:00:00Z"\nid = "%s"\nkind = "fact"\nmachine = "%s"\nmodified = "2026-10-17T00:00:00Z"\ntitle = "%s"\n+++\n\n%s\n' \
		"$1" "$2" "$3" "$3"
}
report() { # report <machine> -- a report frame
	printf '{"t":"report","machine":"%s","agents":[],"checkouts":[],"worktrees":[],"unpushed":{},"cloud":null}' "$1"
}

# 1. The old token on the new build: still nuc, but it writes nothing.
out=$(say "$early" nuc "$(report nuc)" "$(report mini)")
like "$out" '"t":"ready"' "the old token opens the socket on the new build"
is "$(grep -c '"t":"error"' <<<"$out")" 1 "it reports as nuc, and not as mini"
early_path=projects/$PROJECT/items/01K0T161000000000000000001.md
out=$(say "$early" nuc "$(frame 01T161OLD "$early_path" "$(item 01K0T161000000000000000001 nuc 'from the old token')")")
# The store's error frame carries only t and why, no write id.
like "$(grep -F '"t":"error"' <<<"$out")" 'hub share mint' "its item write gets an error naming hub share mint"
is "$(stored_by "$early_path")" "" "and the store holds nothing at that path"

# 2. The new build mints each machine's token, and two replicas start on them.
mini=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/machines/mini/mac-mini-m2")
nuc=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/machines/nuc/nuc")
cloud=$(curl -s -X POST -H "Authorization: Bearer $token" "$base/store/machines/session_*/cloud-*")
is "$(printf '%s\n' "$mini" "$nuc" "$cloud" | grep -cE '^[0-9a-f]{60}$')" 3 \
	"the shared bearer mints mini's, nuc's and the cloud's tokens"

# on <machine> <command...> -- runs a command with that machine's replica.
on() {
	local root=$T_TMP/m/$1
	shift
	env HOME="$root" XDG_DATA_HOME="$root/data" XDG_CACHE_HOME="$root/cache" \
		XDG_STATE_HOME="$root/state" XDG_CONFIG_HOME="$root/config" \
		HUB_LOCAL_SOCKET="$root/state/hub/local.sock" MEM_SYNC_CMD=true "$@"
}
replica() { # replica <machine> -- a replica holding the shared project
	local root=$T_TMP/m/$1
	mkdir -p "$root"/{data,cache,state,config/qshell} "$root/data/mem/store/projects/$PROJECT/items"
	printf '%s\n' "$1" >"$root/config/qshell/machine"
	cat >"$root/data/mem/store/projects/$PROJECT/project.toml" <<-TOML
		created = "2026-10-17T00:00:00Z"
		id = "$PROJECT"
		name = "walk"
		share = "true"
	TOML
}
start_hub() { # start_hub <machine> <token>
	on "$1" env HUB_PEER_TOKEN=peer-token HUB_PEERS= HUB_STORE_URL="$share" HUB_STORE_TOKEN="$2" \
		"$HUB_BIN" local --port "$(free_port)" >>"$T_TMP/$1.hub.log" 2>&1 &
	hub_pid[$1]=$!
}
connected() { # connected <machine> -- its hub has said hello
	for _ in $(seq 1 100); do
		grep -q 'share: connected' "$T_TMP/$1.hub.log" 2>/dev/null && return 0
		sleep 0.1
	done
	return 1
}
# within <ms> <command...> -- runs the command every 100 ms until it
# succeeds; prints the milliseconds it took, and fails after <ms>.
within() {
	local limit=$1 start
	shift
	start=$(now_ms)
	while true; do
		if "$@" >/dev/null 2>&1; then
			echo $(($(now_ms) - start))
			return 0
		fi
		[ $(($(now_ms) - start)) -gt "$limit" ] && return 1
		sleep 0.1
	done
}
store_of() { printf '%s' "$T_TMP/m/$1/data/mem/store"; }

# Global items share only on a machine switched to the store.
for m in mac-mini-m2 nuc; do
	replica "$m"
	on "$m" mem share switch >/dev/null
done
start_hub mac-mini-m2 "$mini"
start_hub nuc "$nuc"
for m in mac-mini-m2 nuc; do
	connected "$m" || { notok "$m's hub connects to the store" "$(cat "$T_TMP/$m.hub.log")"; exit 1; }
done
ok "the mini's and the nuc's hubs hold a sharing socket each, on their own tokens"

# 3. Forgeries on the nuc's socket are refused and reach no one. The nuc's
# own item, sent after them, shows the mini's stream is past them.
rule=01K0T161000000000000000002
forged=01K0T161000000000000000003
control=01K0T161000000000000000004
out=$(say "$nuc" nuc \
	"$(frame 01T161RULE "global/items/$rule.md" "$(item "$rule" macbook-m2 'forged global rule')")" \
	"$(frame 01T161PROJ "projects/$PROJECT/items/$forged.md" "$(item "$forged" macbook-m2 'forged project item')")" \
	"$(frame 01T161CTRL "projects/$PROJECT/items/$control.md" "$(item "$control" nuc 'the nuc speaks for itself')")")
like "$(answer "$out" 01T161RULE)" '^refused ' "a global rule naming macbook-m2 on the nuc's socket is refused"
like "$(answer "$out" 01T161PROJ)" '^refused ' "a project item naming macbook-m2 on the nuc's socket is refused"
is "$(answer "$out" 01T161CTRL)" "ack " "the nuc's item naming itself is acked"
on_mini() { [ -f "$(store_of mac-mini-m2)/projects/$PROJECT/items/$control.md" ]; }
if took=$(within 3000 on_mini); then
	ok "the nuc's own item reaches the mini (${took} ms)"
else
	notok "the nuc's own item reaches the mini within 3 s" "$(tail -20 "$T_TMP/mac-mini-m2.hub.log")"
fi
is "$(ls "$(store_of mac-mini-m2)/global/items" 2>/dev/null)" "" "the forged rule is not on the mini"
unlike "$(cd "$T_TMP" && on mac-mini-m2 mem search forged 2>&1)" "forged global rule" \
	"mem search on the mini does not list the forged rule"
is "$(ls "$(store_of mac-mini-m2)/projects/$PROJECT/items" 2>/dev/null)" "$control.md" \
	"the forged project item is not on the mini"

# 4. The mini's global rule reaches the nuc, recorded as the mini's.
(cd "$T_TMP/m/mac-mini-m2" && on mac-mini-m2 mem save "standing rule from the mini" >/dev/null)
mini_rule=$(cd "$(store_of mac-mini-m2)/global/items" && grep -l 'standing rule from the mini' -- *.md)
like "$mini_rule" '^[0-9A-Z]{26}\.md$' "mem on the mini writes a global rule"
on_nuc() { [ -f "$(store_of nuc)/global/items/$mini_rule" ]; }
if took=$(within 2000 on_nuc); then
	ok "the mini's global rule reaches the nuc within 2 s (${took} ms)"
else
	notok "the mini's global rule reaches the nuc within 2 s" "$(on mac-mini-m2 mem doctor; tail -20 "$T_TMP/nuc.hub.log")"
fi
is "$(stored_by "global/items/$mini_rule")" mac-mini-m2 "the store's record of it names mac-mini-m2"

# 5. The cloud's token writes as the session its hello names, and only as it.
own=01K0T161000000000000000005
posed=01K0T161000000000000000006
out=$(say "$cloud" cloud-ab12 \
	"$(frame 01T161CL0 "projects/$PROJECT/items/$own.md" "$(item "$own" cloud-ab12 'from cloud-ab12')")" \
	"$(frame 01T161CL1 "projects/$PROJECT/items/$posed.md" "$(item "$posed" nuc 'cloud posing as the nuc')")")
is "$(answer "$out" 01T161CL0)" "ack " "the cloud's item naming cloud-ab12 is acked"
cloud_on_nuc() { [ -f "$(store_of nuc)/projects/$PROJECT/items/$own.md" ]; }
if took=$(within 2000 cloud_on_nuc); then
	ok "the cloud's item reaches the nuc (${took} ms)"
else
	notok "the cloud's item reaches the nuc within 2 s" "$(tail -20 "$T_TMP/nuc.hub.log")"
fi
like "$(answer "$out" 01T161CL1)" '^refused ' "the cloud's item naming nuc is refused"

# 6. A cloud session's outbox, left on a branch, imported and sent by the nuc.
replica cloud-ab12
on cloud-ab12 mem --project walk log "unsent from cloud-ab12" >/dev/null
repo=$T_TMP/checkout
git init -q -b main "$repo"
git -C "$repo" commit -q --allow-empty -m start
(cd "$repo" && on cloud-ab12 mem share export >/dev/null)
git -C "$repo" commit -q -m "Push the cloud's outbox"
is "$(python3 -c 'import json, sys; o = json.load(open(sys.argv[1])); print(o["machine"], len(o["writes"]))' \
	"$repo/.mem-outbox.json")" "cloud-ab12 1" "the branch carries cloud-ab12's outbox with its one log"
(cd "$repo" && on nuc mem --project walk log --limit 5 >/dev/null)
imported() { grep -rlF 'unsent from cloud-ab12' "$(store_of mac-mini-m2)/projects/$PROJECT/items"; }
if took=$(within 5000 imported); then
	ok "the imported log reaches the mini (${took} ms)"
else
	notok "the imported log reaches the mini" "$(on nuc mem doctor; tail -20 "$T_TMP/nuc.hub.log")"
fi
landed=$(imported)
like "$(cat "$landed" 2>/dev/null)" '^machine = "cloud-ab12"$' "the mini's copy names cloud-ab12 in its frontmatter"
is "$(stored_by "projects/$PROJECT/items/$(basename -- "$landed")")" nuc "the store took it from the nuc's token"
