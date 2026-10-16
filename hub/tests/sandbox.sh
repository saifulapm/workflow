#!/usr/bin/env bash
# The hub built from this checkout, serving a seeded throwaway store.
#
#   bash hub/tests/sandbox.sh                   # serve until killed
#   bash hub/tests/sandbox.sh shot / home.png   # one 390 px screenshot, then stop
#
# The first line on stdout is `sandbox http://127.0.0.1:<port>/`, the second
# `store <dir>`, the directory standing in for HOME. Everything else goes to
# stderr. HUB_SANDBOX_PORT picks the port; 0, the default, picks a free one.
# HUB_SANDBOX_NTFY_BASE=http://127.0.0.1:<port> sends the doorbell's pushes
# to a sink the walk listens on; by default they go nowhere.
#
# Four projects cover the hub's stages: alpha has a brief only, beta a draft
# roadmap waiting on approval, gamma an approved milestone being built, with
# evidence, findings, a wiki and rulings, and delta a roadmap with every
# milestone ticked.

set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/../.." && pwd)

# Taken before HOME moves: the browser and its config live under the caller's
# HOME, and the lock has to be the same file for every caller on the machine.
caller_home=$HOME
caller_config=${XDG_CONFIG_HOME:-$HOME/.config}
short_tmp=${XDG_RUNTIME_DIR:-/tmp}
lock=$short_tmp/hub-browser.lock

# One browser at a time on this machine: the suite and two evidence captures
# would otherwise run three, and three do not fit in its memory.
shot() {
	local path=$1 out=$2 url
	case $out in /*) ;; *) out=$PWD/$out ;; esac
	shot_tmp=$(mktemp -d "${TMPDIR:-/tmp}/hub-shot.XXXXXX")
	session=hub-sandbox-$$
	bash "$0" >"$shot_tmp/out" &
	child=$!
	# However shot ends, the browser, the sandbox and its output go with it.
	trap shot_stop EXIT
	url=""
	for _ in $(seq 1200); do
		url=$(sed -n '1s/^sandbox \(.*\)$/\1/p' "$shot_tmp/out")
		[ -n "$url" ] && break
		kill -0 "$child" 2>/dev/null || break
		sleep 0.5
	done
	if [ -z "$url" ]; then
		echo "sandbox.sh: the sandbox did not start" >&2
		return 1
	fi
	head -n 1 "$shot_tmp/out"
	exec 9>"$lock"
	flock 9
	# As the seeded paired device, so the page shows its controls.
	pw open "$url"
	pw cookie-set hub_device SANDBOXDEVICESANDBOXDEVICE --domain 127.0.0.1 --path /
	pw goto "${url%/}$path"
	pw resize 390 844
	pw screenshot --filename "$out"
	echo "shot $out"
}
# The daemon's socket lives under TMPDIR, and a socket path is capped at 108
# bytes, which a task's own TMPDIR can pass on its own.
pw() { HOME=$caller_home XDG_CONFIG_HOME=$caller_config TMPDIR=$short_tmp playwright-cli -s="$session" "$@" >&2; }
shot_stop() {
	pw close >/dev/null 2>&1 || true
	kill "$child" 2>/dev/null || true
	wait "$child" 2>/dev/null || true
	rm -rf "$shot_tmp"
}

trap 'exit 143' TERM
trap 'exit 130' INT

if [ "${1:-}" = shot ]; then
	[ $# -eq 3 ] || { echo "usage: sandbox.sh shot <path> <out.png>" >&2; exit 2; }
	shot "$2" "$3"
	exit
fi

# A shell inside a checkout or a worktree carries these for its own project;
# the seed would then write to that project rather than the sandbox's.
unset MEM_PROJECT GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

bin_of() {
	local crate=$1 target=${CARGO_TARGET_DIR:-$root/$1/target}
	(cd "$root/$crate" && CARGO_TARGET_DIR=$target cargo build --release) >&2
	printf '%s/release/%s\n' "$target" "$crate"
}
mem_bin=$(bin_of mem)
hub_bin=$(bin_of hub)
# The hub's Pause and Unpause run workflow, which must reach the sandbox's mem.
workflow_bin=$(bin_of workflow)

sb=$(mktemp -d "${TMPDIR:-/tmp}/hub-sandbox.XXXXXX")
hub_pid=""
stop() {
	if [ -n "$hub_pid" ]; then
		kill "$hub_pid" 2>/dev/null || true
		wait "$hub_pid" 2>/dev/null || true
	fi
	rm -rf "$sb"
}
trap stop EXIT

mkdir -p "$sb/bin"
ln -s "$mem_bin" "$sb/bin/mem"
ln -s "$hub_bin" "$sb/bin/hub"
ln -s "$workflow_bin" "$sb/bin/workflow"
export PATH="$sb/bin:$PATH"

# Toolchains keep their real homes; everything mem and hub write lands under
# the sandbox.
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export HOME="$sb/home"
export XDG_DATA_HOME="$HOME/.local/share" XDG_CONFIG_HOME="$HOME/.config"
export XDG_STATE_HOME="$HOME/.local/state" XDG_CACHE_HOME="$HOME/.cache"
export MEM_SYNC_CMD=true MEM_NOTIFY_CMD=true
export GIT_CONFIG_GLOBAL="$HOME/.gitconfig" GIT_CONFIG_NOSYSTEM=1
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME/hub" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
git config --global user.email sandbox@example.invalid
git config --global user.name 'Hub Sandbox'
git config --global init.defaultBranch main
git config --global commit.gpgsign false
# Port 9 is discard: a doorbell that rings goes nowhere, unless the walk
# names a sink of its own to count the pushes in.
printf 'ntfy_base = "%s"\n' "${HUB_SANDBOX_NTFY_BASE:-http://127.0.0.1:9}" >"$XDG_CONFIG_HOME/hub/config.toml"
# One paired device, so a walk can POST: the browser sets the cookie with
# `playwright-cli cookie-set hub_device SANDBOXDEVICESANDBOXDEVICE`.
mkdir -p "$XDG_STATE_HOME/hub"
printf 'SANDBOXDEVICESANDBOXDEVICE 2026-01-01T00:00:00Z sandbox\n' >"$XDG_STATE_HOME/hub/devices"
chmod 600 "$XDG_STATE_HOME/hub/devices"

source "$root/hub/tests/seed.sh"

cd "$sb"
hub --port "${HUB_SANDBOX_PORT:-0}" >"$sb/hub.out" 2>"$sb/hub.err" &
hub_pid=$!
port=""
for _ in $(seq 100); do
	port=$(sed -n 's/^hub listening on 127\.0\.0\.1:\([0-9]*\)$/\1/p' "$sb/hub.out")
	[ -n "$port" ] && break
	kill -0 "$hub_pid" 2>/dev/null || break
	sleep 0.1
done
if [ -z "$port" ]; then
	cat "$sb/hub.err" >&2
	echo "sandbox.sh: hub did not start" >&2
	exit 1
fi
printf 'sandbox http://127.0.0.1:%s/\nstore %s\n' "$port" "$sb"
wait "$hub_pid"
