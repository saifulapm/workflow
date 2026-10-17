#!/usr/bin/env bash
# The hub web's pages under `cf dev` say in Server-Timing how many rows their
# store queries read (`rows;desc=<n>`, the cursors' own rowsRead, which
# Cloudflare bills), on a store with thousands of items in one project, so a
# page that reads every item of the store shows.
#
# Needs what t150 needs: hubweb/node_modules, worker-build, the wasm target,
# openssl, python3 and node.

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

# big holds ITEMS items of every kind the pages read, small a handful.
ITEMS=3000
BIG=01K0R0WSB1G000000000000000
SMALL=01K0R0WSSMA110000000000000
store=$T_TMP/store
python3 - "$store" "$BIG" "$SMALL" "$ITEMS" <<'PY'
import os, sys
root, big, small, n = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
kinds = ["log"] * 12 + ["decision", "finding", "question", "answer", "handoff", "idea", "evidence", "brief"]

def put(path, text):
    path = os.path.join(root, path)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)

def project(pid, name, count):
    put(f"projects/{pid}/project.toml",
        f'created = "2026-10-01T00:00:00Z"\nid = "{pid}"\nname = "{name}"\nroadmap_status = "approved"\n')
    put(f"projects/{pid}/roadmap.md", f"# Roadmap\n\n- [x] m1-one\n- [ ] m2-two\n")
    put(f"projects/{pid}/plan.md", "# plan: m2-two\n\n- [x] t1 one\n- [ ] t2 two\n")
    put(f"projects/{pid}/plans/m2-two.md", "# plan: m2-two\n\nGoal: two.\n")
    put(f"projects/{pid}/wiki/notes.md", "# Notes\n\n## One\n\nfirst\n")
    last_question = None
    for i in range(count):
        kind = kinds[i % len(kinds)]
        iid = f"01K0R0WS{name[:1].upper()}{i:017d}"
        at = f"2026-10-{1 + i * 15 // max(count, 1):02d}T{i % 24:02d}:{i % 60:02d}:00Z"
        extra = ""
        if kind == "finding":
            extra = 'milestone = "m2-two"\nstatus = "open"\nstep = "a step"\n'
        if kind == "question":
            extra = 'audience = "human"\n'
            last_question = iid
        if kind == "answer" and last_question:
            extra = f'answers = "{last_question}"\n'
        if kind == "evidence":
            extra = f'file = "evidence/m2/{iid}.png"\n'
        put(f"projects/{pid}/items/{iid}.md",
            f'+++\ncreated = "{at}"\n{extra}id = "{iid}"\nkind = "{kind}"\nmachine = "mini"\n'
            f'modified = "{at}"\nproject = "{name}"\ntitle = "{kind} {i}"\n+++\n\n{kind} {i}\n')

project(big, "big", n)
project(small, "small", 40)
PY
like "$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$CF_DEV_PORT/store/share" "$token" "$store" 2>&1)" \
	"^pushed $((ITEMS + 40 + 10))\$" "the generated store reaches the store"

# rows <path> -- the rows the page's store queries read, from Server-Timing.
rows() {
	curl -s -o /dev/null -D - -H "@$T_TMP/jwt.hdr" "$base$1" | tr -d '\r' |
		sed -n 's/^[Ss]erver-[Tt]iming:.*rows;desc=\([0-9]*\).*/\1/p'
}
item=01K0R0WSB00000000000000013 # a decision of big's
for page in / /wiki /p/big /p/big/wiki /wiki/big/notes /p/big/roadmap /p/big/plan /p/big/plan/m2-two \
	/p/big/questions /p/big/evidence /p/big/decisions "/p/big/item/$item" /p/small; do
	read=$(rows "$page")
	like "$read" '^[0-9]+$' "$page says the rows it read ($read)"
done
