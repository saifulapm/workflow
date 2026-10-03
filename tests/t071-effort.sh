#!/usr/bin/env bash
# The reasoning dial. `mem project set effort <level>` reaches the worker as
# `--effort` on either seam; WORKFLOW_EFFORT takes one run, an empty one
# meaning no flag at all; and the run records the level beside `model`. The
# process seam hands the level over as its own placeholder; amx gets it as
# `--effort` on `amx new`, checked through a fake that records every argv.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# The fake worker logs the level it was handed -- `-` for none -- and does
# the least that lets the task merge.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; effort=$7
printf '%s %s\n' "$task" "${effort:--}" >>"$WF_TMP/effort.log"
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p app
# Named for its plan too: a finished plan lands on the checkout, and the next
# plan's task over the same file needs a change to commit.
printf '%s %s\n' "$(basename "$(dirname "$PWD")")" "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief} {model} {effort}'"'"' > {out} 2> {err} &'

# The amx stand-in: records argv, does the same work, and answers status.
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"
verb=$1
shift
case $verb in
new)
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model | --effort) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		--parent | --role) shift 2 ;;
		*) text=$1; shift ;;
		esac
	done
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	status=$(grep -oE '[^ ]+\.status' "$brief" | head -1)
	task=$(basename "$status" .status)
	cd "$dir" || exit 1
	mkdir -p app
	# Named for its plan too: a finished plan lands on the checkout, and the next
# plan's task over the same file needs a change to commit.
printf '%s %s\n' "$(basename "$(dirname "$PWD")")" "$task" >"app/$task.php"
	git add "app/$task.php"
	git -c core.hooksPath=/dev/null commit -qm "Add a service"
	printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	printf 'done\n' >"$AMX_DIR/$name.state"
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	printf '{"id":"%s","state":"%s","last_event":0,"session":""}\n' "$1" "$(cat "$AMX_DIR/$1.state")"
	;;
stop) printf 'stopped\n' >"$AMX_DIR/$1.state" ;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

# saw <line> <desc> -- one call the fake amx recorded, its argv joined by '|'.
saw() {
	if grep -qxF -- "$1" "$AMX_DIR/argv"; then ok "$2"; else notok "$2" "$(cat "$AMX_DIR/argv")"; fi
}

# handed <task> <level> <desc> -- what the process-seam fake logged for one dispatch.
handed() {
	is "$(sed -n "s/^$1 //p" "$WF_TMP/effort.log" | head -1)" "$2" "$3"
}

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

# Two tasks: a one-task plan is refused as not worth a worker.
plan() {
	cat >"$T_TMP/$1.md" <<-EOF
	# plan: $1

	- [ ] t1 Add the t1 service
	      Files: app/t1.php
	      Verify: true
	- [ ] t2 Add the t2 service
	      Files: app/t2.php
	      Verify: true
	EOF
}
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5

## ------------------------------------------------ nothing set: no flag

plan quiet
run workflow run --plan-file "$T_TMP/quiet.md"
is "$RC" 0 'a project with no dial runs as before'
handed t1 - 'the worker gets no level'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/quiet/effort")" '' 'the run records no worker level'

## ------------------------------------------------------ the project key

"$MEM_BIN" project set effort max >/dev/null
: >"$WF_TMP/effort.log"
plan keys
run workflow run --plan-file "$T_TMP/keys.md"
is "$RC" 0 'the run merges under the dial'
handed t1 max 'the worker is handed the effort key'
handed t2 max 'every worker is'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/keys/effort")" max 'the run records the worker level'

## ------------------------------------- the variable takes one run, empty off

: >"$WF_TMP/effort.log"
plan once
run env WORKFLOW_EFFORT= workflow run --plan-file "$T_TMP/once.md"
is "$RC" 0 'the run merges under the variable'
handed t1 - 'WORKFLOW_EFFORT= empty hands the worker no level, key or no key'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/once/effort")" '' 'and the run records what it ran with'

## ------------------------------------------------- the same keys under amx

unset WORKFLOW_WORKER_CMD
: >"$AMX_DIR/argv"
plan panes
run workflow run --plan-file "$T_TMP/panes.md"
is "$RC" 0 'the run merges under amx'
wt="$XDG_STATE_HOME/workflow/worktrees/app/panes"
sess=$(cat "$XDG_STATE_HOME/workflow/runs/app/panes/t1.session")
saw "new|--name|$sess|--dir|$wt/t1|--no-worktree|--role|worker|--model|opus|--effort|max|Read $XDG_CACHE_HOME/workflow/briefs/app/panes/t1.md and execute it exactly." \
	'amx new carries --effort for the worker'

## --------------------------- the start line names where each dial came from

# The record this plan wrote when it began beats the project key, on purpose
# (#7GVER0M5) -- so a `mem project set` between two runs of the same plan is
# ignored, and a run that never said which it kept left an orchestrator
# reading the run directory to find out why (frictions #D535K4EF,
# #G2R8CYFH). The second run of `panes` below runs after both keys changed,
# so its record is all it has.
"$MEM_BIN" project set model haiku >/dev/null
"$MEM_BIN" project set effort low >/dev/null
run workflow run --plan-file "$T_TMP/panes.md"
is "$RC" 0 'the plan is already merged, so the second run has nothing to do'
like "$OUT" "run panes: writing with opus \(the run's record\) at effort max \(the run's record\)" \
	'the run says what it writes with, and that it kept its own record over the changed keys'

## ------------------------------- and a flag rewrites that record

# Without this the only way to change a dial a run recorded was to edit the
# run directory by hand.
run workflow run --plan-file "$T_TMP/panes.md" --model glm
is "$RC" 0 'a run with the dials named goes ahead'
like "$OUT" 'run panes: model is now glm in this run.s record' 'it says what it rewrote'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/panes/model")" glm 'and the record carries it'
like "$OUT" "run panes: writing with glm \(the run's record\) at effort max \(the run's record\)" \
	'the start line reads back what the flags put there'
"$MEM_BIN" project unset model >/dev/null
"$MEM_BIN" project set effort max >/dev/null

## ----------------------------------------------- unset is the way back

"$MEM_BIN" project unset effort >/dev/null
: >"$AMX_DIR/argv"
plan back
run workflow run --plan-file "$T_TMP/back.md"
is "$RC" 0 'the run merges with the keys cleared'
is "$(grep -c -- '--effort' "$AMX_DIR/argv")" 0 'and no dispatch carries --effort'

## ------------------------------ a task's Effort line beats the run's dial

# For that task alone: the other one still goes out at the run's level.
cat >"$T_TMP/hard.md" <<'PLAN'
# plan: hard

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Effort: xhigh
      Verify: true
- [ ] t2 Add the t2 service
      Files: app/t2.php
      Verify: true
PLAN
: >"$AMX_DIR/argv"
run env WORKFLOW_EFFORT=high workflow run --plan-file "$T_TMP/hard.md"
is "$RC" 0 'the run merges with one task marked xhigh'
wt="$XDG_STATE_HOME/workflow/worktrees/app/hard"
sess=$(cat "$XDG_STATE_HOME/workflow/runs/app/hard/t1.session")
saw "new|--name|$sess|--dir|$wt/t1|--no-worktree|--role|worker|--model|opus|--effort|xhigh|Read $XDG_CACHE_HOME/workflow/briefs/app/hard/t1.md and execute it exactly." \
	'the task with an Effort line goes out at its own level'
sess=$(cat "$XDG_STATE_HOME/workflow/runs/app/hard/t2.session")
saw "new|--name|$sess|--dir|$wt/t2|--no-worktree|--role|worker|--model|opus|--effort|high|Read $XDG_CACHE_HOME/workflow/briefs/app/hard/t2.md and execute it exactly." \
	'and the task without one at the run level'
