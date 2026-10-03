#!/usr/bin/env bash
# A worker sent back into its own session. Under amx a worker whose turn
# ended is still there, idle in its pane with everything it read and wrote,
# so an answered question goes to it with `amx send` and a rewritten brief
# rather than to a fresh session re-reading it all, whether or not a worker
# slot is free. The attempt count does not move.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
argv="$AMX_DIR/argv"
: >"$argv"

saw() {
	if grep -qF -- "$1" "$argv"; then ok "$2"; else notok "$2" "$(cat "$argv")"; fi
}
# A first dispatch and every fresh session after one are `amx new` at depth
# zero (e65c800).
workers() { grep -cE "^new\|--name\|wf-$1-[0-9a-z]{4}\|" "$argv"; }

# The fake worker does its task on `new`. On `send` the worker named does
# what the rewritten brief asks: the task itself, for an answered question.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"
verb=$1
shift
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1" >/dev/null; }
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
	printf '%s\n' "$dir" >"$AMX_DIR/$name.dir"
	printf '%s\n' "$brief" >"$AMX_DIR/$name.brief"
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	task=$(basename "$brief" .md)
	cd "$dir" || exit 1
	say started
	case $task in
	busy)
		# Holds the one slot until the asker has had its answer sent in.
		printf 'working hooks\n' >"$AMX_DIR/$name.state"
		(
			while [ ! -f "$WF_TMP/asker2-sent" ]; do sleep 0.1; done
			mkdir -p app
			printf 'busy\n' >app/busy.php
			git add app/busy.php
			commit 'Add the busy service'
			say ready
			printf 'idle hooks\n' >"$AMX_DIR/$name.state"
		) >/dev/null 2>&1 &
		exit 0
		;;
	asker | asker2)
		if [ ! -f "$WF_TMP/asked" ]; then
			mem ask 'is draft the right word?' >"$WF_TMP/ask-id"
			: >"$WF_TMP/asked"
			say blocked "asked $(cat "$WF_TMP/ask-id")"
			printf 'idle hooks\n' >"$AMX_DIR/$name.state"
			exit 0
		fi
		;;
	esac
	mkdir -p app
	printf 'draft\n' >"app/$task.php"
	git add "app/$task.php"
	commit "Add a service"
	say ready
	printf 'idle hooks\n' >"$AMX_DIR/$name.state"
	;;
send)
	name=$1
	text=$2
	dir=$(cat "$AMX_DIR/$name.dir")
	brief=$(printf '%s' "$text" | sed -n 's/^Read \(.*\) again: .*/\1/p')
	cp "$brief" "$AMX_DIR/$name.sent-brief"
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	task=$(basename "$brief" .md)
	cd "$dir" || exit 1
	say started
	sed -n '/The orchestrator answered:/p' "$brief" >"$WF_TMP/$task.answer"
	mkdir -p app
	printf 'final\n' >"app/$task.php"
	git add "app/$task.php"
	commit "Add a service"
	say ready
	: >"$WF_TMP/$task-sent"
	printf 'idle hooks\n' >"$AMX_DIR/$name.state"
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	read -r state evidence <"$AMX_DIR/$1.state"
	printf '{"id":"%s","state":"%s","evidence":"%s","last_event":0,"session":""}\n' "$1" "$state" "$evidence"
	;;
stop) printf 'stopped record\n' >"$AMX_DIR/$1.state" ;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5

## ------------------------------------------ an answer goes to the worker

cat >"$T_TMP/ask.md" <<'PLAN'
# plan: ask

- [ ] asker Add the asker service
      Files: app/asker.php
      Verify: true
- [ ] other Add the other service
      Files: app/other.php
      Verify: true
PLAN
workflow run --plan-file "$T_TMP/ask.md" >"$T_TMP/ask.log" 2>&1 &
runpid=$!
# Thirty seconds, not ten: the deadline exported above makes the poll three
# seconds wide, and three polls is no margin at all beside a loaded machine
# (friction #CK3VG63H).
for _ in $(seq 1 300); do
	[ -s "$WF_TMP/ask-id" ] && grep -q 'waiting on' "$T_TMP/ask.log" && break
	sleep 0.1
done
qid=$(cat "$WF_TMP/ask-id")
askdir="$XDG_STATE_HOME/workflow/runs/app/ask"
asess=$(cat "$askdir/asker.session")
is "$(cat "$askdir/asker.state")" failed 'the asker waits, failed on its question'
"$MEM_BIN" answer "$qid" 'final is the word' >/dev/null
wait "$runpid"
is "$?" 0 'the run merges everything once the answer is in'
is "$(cat "$askdir/asker.state")" merged 'the asker merged'
is "$(workers asker)" 1 'in the one session it had'
saw "send|$asess|Read" 'the answer was sent into it'
like "$(cat "$WF_TMP/asker.answer")" 'The orchestrator answered: final is the word' \
	'and the brief it was pointed at carries the answer'
like "$(cat "$T_TMP/ask.log")" "task asker: sent back to its worker \\(session $asess, continuation 1\\)" 'the run says so'
is "$(cat "$askdir/asker.dispatches")" 1 'one attempt'

## ------------------------- an answer goes in while every slot is taken

# The asker's own session takes its answer whatever the cap says; holding it
# for a free slot left a worker idle on its question for as long as its
# siblings ran (frictions #FCZBJ0ZZ, #E9KPGCWQ). Here the one slot is held
# by a task that finishes only once the asker has had its answer.
rm -f "$WF_TMP/asked" "$WF_TMP/ask-id"
cat >"$T_TMP/capped.md" <<'PLAN'
# plan: capped

- [ ] asker2 Add the asker2 service
      Files: app/asker2.php
      Verify: true
- [ ] busy Add the busy service
      Files: app/busy.php
      Verify: true
PLAN
WORKFLOW_MAX_WORKERS=1 workflow run --plan-file "$T_TMP/capped.md" >"$T_TMP/capped.log" 2>&1 &
runpid=$!
for _ in $(seq 1 300); do
	[ -s "$WF_TMP/ask-id" ] && grep -q 'waiting on' "$T_TMP/capped.log" && break
	sleep 0.1
done
"$MEM_BIN" answer "$(cat "$WF_TMP/ask-id")" 'final is the word' >/dev/null
wait "$runpid"
is "$?" 0 'the run merges both with the one slot taken when the answer came'
like "$(cat "$T_TMP/capped.log")" 'task asker2: sent back to its worker' 'the answer went into the waiting session'
like "$(cat "$T_TMP/capped.log")" 'task asker2: stopped on its question -- asked #' 'and its stop was said as a question, not a failure'

t_done
