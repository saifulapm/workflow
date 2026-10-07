#!/usr/bin/env bash
# The hub built from this checkout, serving a seeded throwaway store.
#
#   bash hub/tests/sandbox.sh                   # serve until killed
#   bash hub/tests/sandbox.sh shot / home.png   # one 390 px screenshot, then stop
#
# The first line on stdout is `sandbox http://127.0.0.1:<port>/`, the second
# `store <dir>`, the directory standing in for HOME. Everything else goes to
# stderr. HUB_SANDBOX_PORT picks the port; 0, the default, picks a free one.
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
	pw open "${url%/}$path"
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

# A session started under the workflow carries these for its own task; the
# seed would then write to that project, and its questions would be asked of
# the orchestrator rather than the person the hub shows them to.
unset MEM_PROJECT GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

bin_of() {
	local crate=$1 target=${CARGO_TARGET_DIR:-$root/$1/target}
	(cd "$root/$crate" && CARGO_TARGET_DIR=$target cargo build --release) >&2
	printf '%s/release/%s\n' "$target" "$crate"
}
mem_bin=$(bin_of mem)
hub_bin=$(bin_of hub)

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
# Port 9 is discard: a doorbell that rings goes nowhere.
printf 'ntfy_base = "http://127.0.0.1:9"\n' >"$XDG_CONFIG_HOME/hub/config.toml"
# One paired device, so a walk can POST: the browser sets the cookie with
# `playwright-cli cookie-set hub_device SANDBOXDEVICESANDBOXDEVICE`.
mkdir -p "$XDG_STATE_HOME/hub"
printf 'SANDBOXDEVICESANDBOXDEVICE 2026-01-01T00:00:00Z sandbox\n' >"$XDG_STATE_HOME/hub/devices"
chmod 600 "$XDG_STATE_HOME/hub/devices"

# project <name>: a checkout with one commit, cd'd into. mem registers it on
# its first write.
project() {
	mkdir -p "$sb/$1"
	cd "$sb/$1"
	git init -q .
	printf '# %s\n' "$1" >README.md
	git add README.md
	git -c core.hooksPath=/dev/null commit -qm 'Start the project'
}
# page <slug> <title>: a wiki page of three sections.
page() {
	printf '# %s\n\n## What\n\nWhat %s says, in a line.\n\n## Why\n\nThe reason it is so.\n\n## Open\n\nNothing open.\n' "$2" "$2" |
		mem wiki "$1" --stdin --note "write the $1 page" >/dev/null
}

project alpha
mem brief --set 'A shared shopping list two phones keep in step.' >/dev/null

project beta
mem brief --set 'A recipe box that scales a dish to the people eating it.' >/dev/null
mem roadmap --stdin >/dev/null <<'ROAD'
# roadmap: beta

- [ ] b1-box Recipes are stored and listed
      Show: a recipe added on the phone is listed on the laptop
- [ ] b2-scale A recipe scales to any number of people  [after: b1-box]
      Show: a recipe for four, set to six, lists half again of every amount
ROAD
mem roadmap --status draft >/dev/null
# b1-box's plan is a designed page, which the hub opens in a frame with its
# comment layer and one answered comment pinned on its first claim's heading;
# b2-scale's is markdown, which it renders as it always has.
mem plan b1-box --set-file "$root/skills/plan/example.html" >/dev/null
mem ask --for orchestrator --about 'plan:b1-box#claim-1/1@20,50' \
	'Does the list show the newest recipe first?' >/dev/null
mem answer "$(mem questions --about 'plan:b1-box#' | awk '{print substr($1, 2)}')" \
	'Yes: newest first, and the plan now says so.' >/dev/null
mem plan b2-scale --stdin >/dev/null <<'PLAN'
# plan: b2-scale

- [ ] b2-t1 The first change
PLAN
# Asked the way the plan skill asks it.
mem ask --for human --options approve,changes --recommend approve 'Review the beta roadmap and its plan pages' >/dev/null

project gamma
mem brief --set 'A habit tracker that nags once and then lets go.' >/dev/null
for slug in index spec research-summary; do page "$slug" "gamma $slug"; done
# A word no other item carries, so a search for it has one hit, a section.
{ mem wiki spec; printf '\n## Quiet hours\n\nNo nag goes out between ten at night and seven in the morning.\n'; } |
	mem wiki spec --stdin --note 'add the quiet hours' >/dev/null
mem roadmap --stdin >/dev/null <<'ROAD'
# roadmap: gamma

- [x] g1-log Habits are logged from the phone
      Show: a habit ticked on the phone shows on the week view
- [ ] g2-nag A missed habit nags once
      Show: a habit missed by nine is nagged at nine and never again that day
ROAD
mem roadmap --status approved >/dev/null
mem plan --stdin >/dev/null <<'PLAN'
# plan: g2-nag

- [x] g2-t1 Store when a habit is due
- [ ] g2-t2 Send the nag
- [ ] g2-t3 Stop after one nag a day
PLAN
mem ask --for human --options 'nine,eight,ten' --recommend nine 'Which hour does the nag go out?' >/dev/null
# An answer of one word longer than a phone's line, so the questions page
# shows how an unbroken answer fits.
named=$(mem ask --for human 'What is the streak called?' | sed -n 's/^#\([^ ]*\).*/\1/p')
mem answer "$named" the_streak_kept_every_single_day_from_the_first_tick_to_the_last_without_a_break >/dev/null
# A week of seven days with one ticked, small enough to sit here as text,
# so the gallery has an image a screenshot can show without a browser.
base64 -d >"$sb/week.png" <<<'iVBORw0KGgoAAAANSUhEUgAAAKAAAAB4CAIAAAD6wG44AAABJElEQVR42u3RsQ0AIAgAQWdxJ5d0JisrS0dwCCwIuQ81CVy7Kl3zAsACLMACLMACLMCAVQl47ROcX0v6HMFJdU6GJYABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwEmBVTjAgAVYgAVYgAVYgAGrTA9RzR3lkHFmKgAAAABJRU5ErkJggg=='
mem evidence add --task g2-t1 "$sb/week.png" --note 'the week view with one habit ticked' >/dev/null
# A plain black screenshot as wide as a laptop's, 1280 by 720, so the
# gallery shows how it fits a phone.
base64 -d >"$sb/wide.png" <<<'iVBORw0KGgoAAAANSUhEUgAABQAAAALQAQAAAADnBuD7AAAAh0lEQVR42u3BMQEAAADCoPVPbQlPoAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB4GsTfAAGc95RKAAAAAElFTkSuQmCC'
mem evidence add --task g2-t1 "$sb/wide.png" --note 'the week view on a laptop screen' >/dev/null
printf 'ticked at 00:00, listed under yesterday\n' >"$sb/midnight.txt"
printf 'the nag fires twice at nine\n' >"$sb/twice.txt"
mem finding add --milestone g1-log --step 1 --evidence "$sb/midnight.txt" 'a habit ticked at midnight lands on the wrong day' >/dev/null
mem finding add --milestone g2-nag --step 2 --evidence "$sb/twice.txt" 'the nag fires twice at nine' >/dev/null
fixed=$(mem finding list | sed -n 's/^#\([^ ]*\) .*wrong day.*/\1/p')
mem finding close --by "$(git rev-parse --short HEAD)" "$fixed" >/dev/null
mem decide --by saiful 'Nag by notification, not by email' >/dev/null
mem decide --by saiful --replaces 'Nag by notification, not by email' 'Nag by notification, once a day at most' >/dev/null

project delta
mem brief --set 'A command line that renames photos by the day they were taken.' >/dev/null
mem roadmap --stdin >/dev/null <<'ROAD'
# roadmap: delta

- [x] d1-rename Photos are renamed by date
      Show: a folder of camera names becomes a folder of dates
ROAD
mem roadmap --status approved >/dev/null
mem idea 'Read the date from a video file too' >/dev/null
mem handoff --set 'Nothing in flight; the last fix landed.' >/dev/null

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
