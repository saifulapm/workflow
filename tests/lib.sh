# shellcheck shell=bash
# Shared harness for the workflow tests. Sourced by every tests/t*.sh.
#
# No test framework: each script prints TAP-ish lines (`ok N - desc`) and a
# trailing plan (`1..N`); tests/run.sh tallies them. Every test runs against a
# throwaway HOME so nothing on the machine is read or written -- see t_init.

set -uo pipefail

TESTS_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
WF_ROOT=$(dirname -- "$TESTS_DIR")
MEM_BIN=${MEM_BIN:-$WF_ROOT/mem/target/release/mem}

# The program under test. The suite drives the CLI, so it validates whichever
# implementation this points at; the default is the crate's release binary.
WORKFLOW_BIN=${WORKFLOW_BIN:-$WF_ROOT/workflow/target/release/workflow}
case "$WORKFLOW_BIN" in
/*) ;;
*) WORKFLOW_BIN=$(cd -- "$(dirname -- "$WORKFLOW_BIN")" && pwd)/$(basename -- "$WORKFLOW_BIN") ;;
esac
export WORKFLOW_BIN

_checks=0
_failed=0

ok()    { _checks=$((_checks + 1)); printf 'ok %d - %s\n' "$_checks" "$1"; }
notok() {
	_checks=$((_checks + 1))
	_failed=$((_failed + 1))
	printf 'not ok %d - %s\n' "$_checks" "$1"
	[ $# -gt 1 ] && printf '%s\n' "$2" | sed 's/^/#   /'
	return 0
}

# is <got> <want> <desc>
is() {
	if [ "$1" = "$2" ]; then ok "$3"; else notok "$3" "got:  $1
want: $2"; fi
}

# isnt <got> <unwanted> <desc>
isnt() {
	if [ "$1" != "$2" ]; then ok "$3"; else notok "$3" "got the unwanted value: $1"; fi
}

# like <got> <extended-regex> <desc>
like() {
	if printf '%s' "$1" | grep -Eq -- "$2"; then ok "$3"; else notok "$3" "no match for /$2/ in:
$1"; fi
}

# unlike <got> <extended-regex> <desc>
unlike() {
	if printf '%s' "$1" | grep -Eq -- "$2"; then notok "$3" "unwanted match for /$2/ in:
$1"; else ok "$3"; fi
}

# truthy <status> <desc>
truthy() { is "$1" 0 "$2"; }

# Run a command, capturing combined output in OUT and the status in RC.
run() {
	OUT=$("$@" 2>&1)
	RC=$?
	return 0
}

# Same, but stdout only (stderr passed through to the log).
run_out() {
	OUT=$("$@" 2>/dev/null)
	RC=$?
	return 0
}

# Print the plan and exit with the right status. Registered as an EXIT trap by
# t_init so a test that dies mid-way still reports what it managed to run.
t_done() {
	local status=$?
	trap - EXIT
	printf '1..%d\n' "$_checks"
	[ "$status" -ne 0 ] && [ "$_failed" -eq 0 ] && printf '# aborted with status %s\n' "$status"
	[ -n "${T_TMP:-}" ] && [ -z "${WF_KEEP_TMP:-}" ] && rm -rf -- "$T_TMP"
	if [ "$_failed" -gt 0 ] || [ "$status" -ne 0 ]; then exit 1; fi
	exit 0
}

# t_init -- a sandbox HOME, an isolated git and mem, workflow on PATH.
t_init() {
	T_TMP=$(mktemp -d "${TMPDIR:-/tmp}/wf-test.XXXXXX")
	trap t_done EXIT

	# Toolchains keep their real homes: the sandbox is about state workflow and
	# mem write, not about reinstalling rust inside every test.
	export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
	export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

	export HOME="$T_TMP/home"
	export XDG_DATA_HOME="$HOME/.local/share"
	export XDG_CACHE_HOME="$HOME/.cache"
	export XDG_STATE_HOME="$HOME/.local/state"
	export XDG_CONFIG_HOME="$HOME/.config"
	mkdir -p "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$T_TMP/bin"

	export GIT_CONFIG_GLOBAL="$HOME/.gitconfig"
	export GIT_CONFIG_NOSYSTEM=1
	git config --global user.email tester@example.invalid
	git config --global user.name 'Workflow Tester'
	git config --global init.defaultBranch main
	git config --global commit.gpgsign false
	git config --global advice.detachedHead false

	ln -sf "$WORKFLOW_BIN" "$T_TMP/bin/workflow"
	[ -x "$MEM_BIN" ] && ln -sf "$MEM_BIN" "$T_TMP/bin/mem"
	export PATH="$T_TMP/bin:$PATH"
	export WORKFLOW_MEM="$MEM_BIN"
	export MEM_SYNC_CMD=true MEM_NOTIFY_CMD=true

	# WORKFLOW_SUITE_LOCK_HELD comes with it: running the suite through
	# `workflow verify` exports the marker to say the parent holds this
	# project's lock, and a sandbox that inherited it would skip a lock it
	# does not hold -- which is what t012 exists to catch.
	unset WORKFLOW_AGENT WORKFLOW_HOOK_SEEN WORKFLOW_ALLOW_PUSH WORKFLOW_SUITE_LOCK_HELD
	unset GIT_DIR GIT_INDEX_FILE GIT_PREFIX GIT_WORK_TREE

	HOOKS="$WF_ROOT/hooks"
	cd "$T_TMP" || exit 1
}

# new_repo <dir> -- an initialised repo with one commit, cd'd into.
new_repo() {
	mkdir -p "$T_TMP/$1"
	cd "$T_TMP/$1" || exit 1
	git init -q .
	printf 'seed\n' >README.md
	git add README.md
	git -c core.hooksPath=/dev/null commit -qm 'seed'
}

# mem_register -- make the cwd repo a checkout mem knows (a write verb registers).
mem_register() {
	"$MEM_BIN" log 'registered by the test harness' >/dev/null 2>&1
}

# age_mem_items -- backdate every item in the sandbox store so `--since` windows
# no longer cover them. mem reads created/modified from the frontmatter, so the
# rewrite has to go through a reindex.
age_mem_items() {
	local f
	while IFS= read -r f; do
		sed -i 's/^created = ".*"/created = "2000-01-01T00:00:00Z"/; s/^modified = ".*"/modified = "2000-01-01T00:00:00Z"/' "$f"
	done < <(find "$XDG_DATA_HOME/mem/store" -name '*.md' 2>/dev/null)
	"$MEM_BIN" reindex >/dev/null 2>&1
}

# git_gated <args...> -- run git with the workflow hooks installed.
git_gated() {
	git -c core.hooksPath="$HOOKS" "$@"
}

# write_exec <path> <<'EOF' ... EOF -- write a file from stdin and chmod +x.
write_exec() {
	mkdir -p "$(dirname -- "$1")"
	cat >"$1"
	chmod +x "$1"
}
