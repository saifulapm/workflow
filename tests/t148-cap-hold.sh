#!/usr/bin/env bash
# A launch amx refuses at its machine-wide cap is a wait, not a failure: the
# task goes back to pending with no dispatch spent and is tried again once
# WORKFLOW_SPAWN_RETRY_S has passed. Any other refusal still fails the task.
source "$(dirname -- "$0")/lib.sh"
t_init

export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
argv="$AMX_DIR/argv"
: >"$argv"

# A stand-in for amx that refuses its first `$AMX_DIR/caps` launches with the
# line amx prints at max_total, then starts a worker that commits and reports
# ready. Every launch it is asked for is timed into `$AMX_DIR/tries`.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"

verb=$1
shift
case $verb in
new)
	case " $* " in *" --check "*) exit 0 ;; esac
	date +%s >>"$AMX_DIR/tries"
	if [ -f "$AMX_DIR/refuse" ]; then
		cat "$AMX_DIR/refuse" >&2
		exit 2
	fi
	if [ "$(wc -l <"$AMX_DIR/tries")" -le "$(cat "$AMX_DIR/caps" 2>/dev/null || echo 0)" ]; then
		echo 'amx: 2 agents are already running on this machine, and max_total is 2' >&2
		exit 2
	fi
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model | --parent | --role) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		*) text=$1; shift ;;
		esac
	done
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	status=$(grep -oE '[^ ]+\.status' "$brief" | head -1)
	task=$(basename "$status" .status)
	cd "$dir" || exit 1
	mkdir -p app
	printf '%s\n' "$task" >"app/$task.php"
	git add "app/$task.php"
	git -c core.hooksPath=/dev/null commit -qm "Add a service"
	printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	printf 'done\n' >"$AMX_DIR/$name.state"
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	printf '{"id":"%s","state":"%s","last_event":0,"session":"","question":null}\n' \
		"$1" "$(cat "$AMX_DIR/$1.state")"
	;;
stop)
	printf 'stopped\n' >"$AMX_DIR/$1.state"
	;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"
export WORKFLOW_SPAWN_RETRY_S=2 WORKFLOW_DEADLINE_MIN=0.5

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

## --------------------------------------------- two launches at the cap

printf '2\n' >"$AMX_DIR/caps"
cat >"$T_TMP/cap.md" <<'PLAN'
# plan: cap

- [ ] c1 Add the pricing service
      Files: app/c1.php
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/cap.md"
is "$RC" 0 'a run whose first two launches meet the cap still merges'
rundir="$XDG_STATE_HOME/workflow/runs/app/cap"
is "$(cat "$rundir/c1.state")" merged 'the task is merged'
is "$(cat "$rundir/c1.dispatches")" 1 'with one dispatch counted'
is "$(grep -c ' failed c1' "$rundir/events" 2>/dev/null)" 0 'and no failed event written'
is "$(grep -c '^new|--name|wf-c1-' "$argv")" 3 'after three launches'
like "$OUT" 'waiting for a machine slot: amx: 2 agents are already running on this machine, and max_total is 2' \
	'the run says what it waits on'
second=$(sed -n 2p "$AMX_DIR/tries") third=$(sed -n 3p "$AMX_DIR/tries")
gap=$((${third:-0} - ${second:-0}))
truthy "$([ "$gap" -ge 2 ] && echo 0 || echo 1)" 'the retry waited WORKFLOW_SPAWN_RETRY_S seconds'

## ----------------------------------------- a refusal with another line

: >"$AMX_DIR/tries"
printf 'amx: no tmux server to reach\n' >"$AMX_DIR/refuse"
cat >"$T_TMP/notmux.md" <<'PLAN'
# plan: notmux

- [ ] n1 Add the stock service
      Files: app/n1.php
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/notmux.md"
is "$RC" 1 'any other refusal fails the run'
nodir="$XDG_STATE_HOME/workflow/runs/app/notmux"
is "$(cat "$nodir/n1.state")" failed 'the task is failed'
is "$(cat "$nodir/n1.failed")" 'the launch was refused: amx: no tmux server to reach' 'on the line amx printed'
is "$(grep -c ' failed n1' "$nodir/events")" 1 'with a failed event'
