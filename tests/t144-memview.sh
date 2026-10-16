#!/usr/bin/env bash
# memview against the real mem: the hub sandbox's seeded store, and for
# every read the hub's pages make, `mem <argv>` beside `memview <store>
# <argv>`. The two documents must be equal once the keys mem takes from the
# machine rather than the store are set aside: a project's aliases, checkouts,
# current and worktree, the store directory in front of every path, and the
# file mtime mem reports as a page's or a stored plan's modified date. Where
# mem prints nothing, memview must print nothing too.
#
# Needs python3 and cargo.

source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HUBWEB/target}
if ! cargo build -q --manifest-path "$HUBWEB/Cargo.toml" --example memview; then
	notok "the memview example builds"
	exit 1
fi

export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
source "$WF_ROOT/hub/tests/seed.sh"
# Outside every checkout, so no read leans on the working directory.
cd "$T_TMP" || exit 1
store=$XDG_DATA_HOME/mem/store

cat >"$T_TMP/same.py" <<'PY'
import json, sys

store, mem_file, view_file = sys.argv[1:4]
MACHINE = {"aliases", "checkouts", "current", "worktree"}

def norm(value, parent=None):
    if isinstance(value, dict):
        out = {}
        for key, inner in value.items():
            if key in MACHINE:
                continue
            if key == "path" and isinstance(inner, str):
                inner = inner.removeprefix(store + "/")
            # A page's date is its file's mtime, which the store does not keep.
            if key == "modified" and parent in ("pages", "plans"):
                continue
            if key in ("created", "modified") and value.get("kind") == "wiki":
                continue
            out[key] = norm(inner)
        return out
    if isinstance(value, list):
        return [norm(inner, parent) for inner in value]
    return value

def load(path):
    text = open(path).read().strip()
    return text if not text else norm({k: norm(v, k) for k, v in json.loads(text).items()})

mem, view = load(mem_file), load(view_file)
if mem == view:
    sys.exit(0)
print("mem:     " + json.dumps(mem, sort_keys=True))
print("memview: " + json.dumps(view, sort_keys=True))
sys.exit(1)
PY

# same <argv...>: mem and memview print the same document, or both nothing.
same() {
	"$MEM_BIN" "$@" >"$T_TMP/mem.out" 2>/dev/null
	cargo run -q --manifest-path "$HUBWEB/Cargo.toml" --example memview -- "$store" "$@" \
		>"$T_TMP/view.out" 2>"$T_TMP/view.err"
	if out=$(python3 -I "$T_TMP/same.py" "$store" "$T_TMP/mem.out" "$T_TMP/view.out" 2>&1); then
		ok "memview answers: mem $*"
	else
		notok "memview answers: mem $*" "$out
$(cat "$T_TMP/view.err")"
	fi
}

asked=$("$MEM_BIN" questions --pending --project=gamma --json |
	python3 -I -c 'import json, sys; print(json.load(sys.stdin)["questions"][0]["short_id"])')

same projects --json
same questions --pending --all-projects --for human --json
for p in alpha beta gamma delta nobody; do
	same questions --project="$p" --json
	same questions --pending --for human --project="$p" --json
	same brief --project="$p" --json
	same roadmap --project="$p" --json
	same plan --project="$p" --json
	same plan --list --project="$p" --json
	same handoff --project="$p" --json
	same wiki --project="$p" --json
	same log --limit 5 --project="$p" --json
	same log --kind=ruling --limit 3 --project="$p" --json
	same search --kind ruling --limit 100 --project="$p" --json
	same search --kind idea --limit 20 --project="$p" --json
	same evidence list --project="$p" --json
	same finding list --project="$p" --json
	same finding list --open --project="$p" --json
done
same plan --project=beta --json -- b1-box
same plan --project=beta --json -- b2-scale
same plan --project=beta --json -- missing
for slug in index spec research-summary missing; do
	same wiki --project=gamma --json -- "$slug"
	same wiki --project=gamma --sections --json -- "$slug"
done
same wiki --sections --project=gamma --json -- spec
# A score is BM25 over the whole index, and mem's index still counts the
# words of the finding the seed closed in place, which the store no longer
# holds. The index is disposable: built afresh, it counts what the store
# holds, as memview does.
rm -f "$XDG_CACHE_HOME"/mem/index.db*
same search --project=gamma --limit 20 --json -- quiet
same search --project=gamma --limit 20 --json -- nag
same search --project=gamma --limit 20 --json -- nothing-matches
same show --json -- "$asked"
same show --json -- ZZZZZZZZ
