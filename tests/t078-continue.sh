#!/usr/bin/env bash
# A worker sent back into its own session. Under amx a worker whose turn
# ended is still there, idle in its pane with everything it read and wrote,
# so the first fix round and an answered question go to it with `amx send`
# and a rewritten brief rather than to a fresh session re-reading it all.
# The attempt count does not move; the status file is emptied so the gate
# judges the new report; a send amx does not confirm stops the session and
# dispatches afresh, since a paste that may have landed cannot sit beside a
# second worker; and a session whose task settled is stopped.
source "$(dirname -- "$0")/lib.sh"
t_init

# The reader is part of this.
unset WORKFLOW_REVIEW_MODEL

export WF_TMP="$T_TMP"
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
argv="$AMX_DIR/argv"
: >"$argv"

saw() {
	if grep -qF -- "$1" "$argv"; then ok "$2"; else notok "$2" "$(cat "$argv")"; fi
}
count() { grep -cF -- "$1" "$argv"; }
# A first dispatch and every fresh session after one are `amx new` at depth
# zero (e65c800); only a reader goes out as `amx sub --bg` under its worker.
workers() { grep -cE "^new\|--name\|wf-$1-[0-9a-z]{4}\||^sub\|--bg\|--json\|--name\|wf-$1-[0-9a-z]{4}\|" "$argv"; }

# One fake plays worker and reader. A worker does its task on `new`; a
# reader writes fix on its first reading of a task and ship after that. On
# `send` the worker named does what the rewritten brief asks: a fix commit
# for a fix round, the task itself for an answered question. The worker of
# `refuse` is at a question as far as amx is concerned, so its send exits 2.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"
verb=$1
shift
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1" >/dev/null; }
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
	printf '%s\n' "$dir" >"$AMX_DIR/$name.dir"
	printf '%s\n' "$brief" >"$AMX_DIR/$name.brief"
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	if [ -n "$answer" ]; then
		task=$(basename "$answer" .review)
		n=$(grep -c . "$AMX_DIR/$task.readings" 2>/dev/null || echo 0)
		printf 'reading\n' >>"$AMX_DIR/$task.readings"
		if [ "$n" = 0 ]; then
			printf 'VERDICT: fix\n1. app/%s.php:1 -- says draft; the Done line wants the fix.\n' "$task" >"$answer"
		else
			printf 'VERDICT: ship\n' >"$answer"
		fi
		printf 'idle hooks\n' >"$AMX_DIR/$name.state"
		exit 0
	fi
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	task=$(basename "$brief" .md)
	cd "$dir" || exit 1
	say started
	case $task in
	asker)
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
	commit "Add the $task service"
	say ready
	printf 'idle hooks\n' >"$AMX_DIR/$name.state"
	;;
send)
	name=$1
	text=$2
	case $name in
	wf-refuse-*) exit 2 ;;
	esac
	dir=$(cat "$AMX_DIR/$name.dir")
	brief=$(printf '%s' "$text" | sed -n 's/^Read \(.*\) again: .*/\1/p')
	cp "$brief" "$AMX_DIR/$name.sent-brief"
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	task=$(basename "$brief" .md)
	cd "$dir" || exit 1
	say started
	if grep -q 'What the reader found' "$brief"; then
		printf 'fixed\n' >>"app/$task.php"
		git add "app/$task.php"
		commit 'Fix what the review found'
	else
		sed -n '/The orchestrator answered:/p' "$brief" >"$WF_TMP/$task.answer"
		mkdir -p app
		printf 'final\n' >"app/$task.php"
		git add "app/$task.php"
		commit "Add the $task service"
	fi
	say ready
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
"$MEM_BIN" project set review-model fable >/dev/null
base=$(git rev-parse HEAD)

## ------------------------------------------ the fix round goes to the worker

cat >"$T_TMP/fix.md" <<'PLAN'
# plan: fix

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Verify: true
      Done: app/t1.php ends with fixed
- [ ] refuse Add the refuse service
      Files: app/refuse.php
      Verify: true
      Done: app/refuse.php ends with fixed
PLAN
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run --plan-file "$T_TMP/fix.md"
is "$RC" 0 'both tasks merge after a fix round each'
rundir="$XDG_STATE_HOME/workflow/runs/app/fix"
brief="$XDG_CACHE_HOME/workflow/briefs/app/fix/t1.md"

sess=$(cat "$rundir/t1.session")
is "$(cat "$rundir/t1.state")" merged 't1 merged'
is "$(cat "$rundir/t1.reviews")" 1 'after one fix verdict'
is "$(workers t1)" 1 'its worker was started once'
saw "send|$sess|Read $brief again: it now says what happened to your last report and what to do next. Do that, then report as it says." \
	'and sent back into that session, pointed at its brief again'
is "$(cat "$rundir/t1.dispatches")" 1 'the attempt count did not move'
is "$(cat "$rundir/t1.continued")" 1 'the continuation is counted on its own'
like "$(cat "$AMX_DIR/$sess.sent-brief")" 'What the reader found' \
	'the brief it was pointed at carries the findings'
like "$(cat "$AMX_DIR/$sess.sent-brief")" 'says draft; the Done line wants the fix' 'verbatim'
like "$(cat "$AMX_DIR/$sess.sent-brief")" 'already holds 1 commit' 'and says the branch already holds its commit'
like "$OUT" "task t1: sent back to its worker \\(session $sess, continuation 1\\)" 'the run says so'
is "$(git rev-list --count "$base..integration/fix" -- app/t1.php)" 2 'both commits landed: the draft and the fix'
saw "stop|$sess" 'the worker is stopped once its task merged'
is "$(count "stop|wf-t1-review-")" 2 'and each reader is stopped once its reading is judged'
is "$(grep -c . "$WF_TMP/asked" 2>/dev/null || echo 0)" 0 'nobody asked anything here'

## ---------------------------------------- a send amx refuses: fresh session

rsess=$(cat "$rundir/refuse.session")
is "$(cat "$rundir/refuse.state")" merged 'refuse merged too'
is "$(workers refuse)" 2 'after a fresh session, since the send was refused'
is "$(grep -cE '^sub\|--bg\|--json\|--name\|wf-refuse-[0-9a-z]{4}\|' "$argv")" 0 'the fresh session is top-level, not a child of the one the send was refused on'
is "$(grep -cE '^new\|--name\|wf-refuse-[0-9a-z]{4}\|' "$argv")" 2 'both of its sessions went out as amx new at depth 0'
is "$(cat "$rundir/refuse.dispatches")" 2 'counted as a second attempt'
is "$(cat "$rundir/refuse.continued" 2>/dev/null)" '' 'and not as a continuation'
first=$(grep -E '^new\|--name\|wf-refuse-[0-9a-z]{4}\|' "$argv" | head -1 | cut -d'|' -f3)
saw "stop|$first" 'the session the send was refused on is stopped before the fresh one starts'
like "$OUT" 'task refuse: dispatched again with the findings on the next free slot' 'the run says which way it went'

## -------------------------------------- an answer goes to the worker too

cat >"$T_TMP/ask.md" <<'PLAN'
# plan: ask

- [ ] asker Add the asker service
      Files: app/asker.php
      Verify: true
- [ ] other Add the other service
      Files: app/other.php
      Verify: true
PLAN
"$MEM_BIN" project set review-model none >/dev/null
workflow run --plan-file "$T_TMP/ask.md" >"$T_TMP/ask.log" 2>&1 &
runpid=$!
for _ in $(seq 1 100); do
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

t_done
