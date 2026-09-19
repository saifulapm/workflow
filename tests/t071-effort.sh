#!/usr/bin/env bash
# The reasoning dial. `mem project set effort <level>` and `set review-effort
# <level>` reach the worker and the reader as `--effort` on either seam
# WORKFLOW_EFFORT and WORKFLOW_REVIEW_EFFORT take one run,
# an empty one meaning no flag at all; and the run records both levels beside
# `model` and `review-model`. The process seam hands the level over as its
# own placeholder; amx gets it as `--effort` on `amx new`, checked through a
# fake that records every argv.
source "$(dirname -- "$0")/lib.sh"
t_init

# The reader is part of this: its dial is the second key.
unset WORKFLOW_REVIEW_MODEL

export WF_TMP="$T_TMP"

# One fake plays worker and reader. Either logs the level it was handed --
# `-` for none -- and does the least that lets the task merge.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$5; effort=$7
printf '%s %s\n' "$task" "${effort:--}" >>"$WF_TMP/effort.log"
case $task in
*-review)
	printf 'VERDICT: ship\n' >"$(sed -n 's/^    Answer file: //p' "$brief")"
	exit 0
	;;
esac
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p app
# Named for its plan too: a finished plan lands on the checkout, and the next
# plan's task over the same file needs a change to commit.
printf '%s %s\n' "$(basename "$(dirname "$PWD")")" "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
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
new | sub)
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
	[ "$verb" = sub ] && printf '{"id":"%s","parent":null,"phase":"starting","answer":null,"evidence":"hooks"}\n' "$name"
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	if [ -n "$answer" ]; then
		printf 'VERDICT: ship\n' >"$answer"
		printf 'done\n' >"$AMX_DIR/$name.state"
		exit 0
	fi
	status=$(grep -oE '[^ ]+\.status' "$brief" | head -1)
	task=$(basename "$status" .status)
	cd "$dir" || exit 1
	mkdir -p app
	# Named for its plan too: a finished plan lands on the checkout, and the next
# plan's task over the same file needs a change to commit.
printf '%s %s\n' "$(basename "$(dirname "$PWD")")" "$task" >"app/$task.php"
	git add "app/$task.php"
	git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
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
"$MEM_BIN" project set review-model fable >/dev/null

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
handed t1-review - 'and neither does the reader'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/quiet/effort")" '' 'the run records no worker level'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/quiet/review-effort")" '' 'and no reader level'

## ------------------------------------------- the project keys, both dials

"$MEM_BIN" project set effort max >/dev/null
"$MEM_BIN" project set review-effort high >/dev/null
: >"$WF_TMP/effort.log"
plan keys
run workflow run --plan-file "$T_TMP/keys.md"
is "$RC" 0 'the run merges under both keys'
handed t1 max 'the worker is handed the effort key'
handed t2 max 'every worker is'
handed t1-review high 'the reader is handed review-effort'
handed t2-review high 'every reading is'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/keys/effort")" max 'the run records the worker level'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/keys/review-effort")" high 'and the reader level'

## ---------------------------------- the variables take one run, empty off

: >"$WF_TMP/effort.log"
plan once
run env WORKFLOW_EFFORT= WORKFLOW_REVIEW_EFFORT=low workflow run --plan-file "$T_TMP/once.md"
is "$RC" 0 'the run merges under the variables'
handed t1 - 'WORKFLOW_EFFORT= empty hands the worker no level, key or no key'
handed t1-review low 'WORKFLOW_REVIEW_EFFORT overrides the reader key for this run'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/once/effort")" '' 'and the run records what it ran with'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/once/review-effort")" low 'for both dials'

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
rsess=$(cat "$XDG_STATE_HOME/workflow/runs/app/panes/t1.review-session")
saw "sub|--bg|--json|--name|$rsess|--parent|$sess|--dir|$wt/_integration|--no-worktree|--role|reader|--model|fable|--effort|high|Read $XDG_STATE_HOME/workflow/runs/app/panes/t1.review-prompt and execute it exactly." \
	'and --effort for the reader'

## --------------------------- the start line names where each dial came from

# The record this plan wrote when it began beats the project key, on purpose
# (#7GVER0M5) -- so a `mem project set` between two runs of the same plan is
# ignored, and a run that never said which it kept left an orchestrator
# reading the run directory to find out why (frictions #D535K4EF,
# #G2R8CYFH). The second run of `panes` below runs after both keys changed.
"$MEM_BIN" project set model haiku >/dev/null
"$MEM_BIN" project set effort low >/dev/null
run workflow run --plan-file "$T_TMP/panes.md"
is "$RC" 0 'the plan is already merged, so the second run has nothing to do'
like "$OUT" "run panes: writing with opus \(the run's record\) at effort max \(the run's record\), reading with fable \(the run's record\) at effort high \(the run's record\)" \
	'the run says what it writes and reads with, and that it kept its own record over the changed keys'

## ------------------------------- and a flag rewrites that record

# Without this the only way to change a dial a run recorded was to edit the
# run directory by hand.
run workflow run --plan-file "$T_TMP/panes.md" --model glm --review-effort low
is "$RC" 0 'a run with the dials named goes ahead'
like "$OUT" 'run panes: model is now glm in this run.s record' 'it says what it rewrote'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/panes/model")" glm 'and the record carries it'
like "$OUT" "run panes: writing with glm \(the run's record\) at effort max \(the run's record\), reading with fable \(the run's record\) at effort low \(the run's record\)" \
	'the start line reads back what the flags put there'
"$MEM_BIN" project unset model >/dev/null
"$MEM_BIN" project set effort max >/dev/null

## ----------------------------------------------- unset is the way back

"$MEM_BIN" project unset effort >/dev/null
"$MEM_BIN" project unset review-effort >/dev/null
: >"$AMX_DIR/argv"
plan back
run workflow run --plan-file "$T_TMP/back.md"
is "$RC" 0 'the run merges with the keys cleared'
is "$(grep -c -- '--effort' "$AMX_DIR/argv")" 0 'and no dispatch carries --effort'
