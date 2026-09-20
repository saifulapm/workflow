#!/usr/bin/env bash
# A worker's question is the orchestrator's: tagged with the task, kept off
# the human listing, answered from the checkout, and carried into the
# worker's next attempt -- which the live run dispatches by itself the moment
# the answer lands. The plan of record is read at dispatch and at the gate,
# so an answer that widens a Files line is the whole correction. A run that
# stops short writes a report to the log; it asks nobody anything.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; status=$3; session=$4; brief=$5
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
attempt=$(grep -c . "$WF_TMP/$task.attempts" 2>/dev/null || echo 0)
attempt=$((attempt + 1))
printf 'attempt %s\n' "$attempt" >>"$WF_TMP/$task.attempts"

say started
case "$task" in
ask)
	if [ "$attempt" = 1 ]; then
		printf '%s\n' "${WORKFLOW_TASK:-unset}" >"$WF_TMP/ask-task"
		# Asked from the worktree, with nothing said about who it is for.
		mem ask 'may I add the fixture the Done line implies?' >"$WF_TMP/ask-id"
		say blocked "asked $(cat "$WF_TMP/ask-id")"
		done_json
		exit 0
	fi
	sed -n '/The orchestrator answered:/p' "$brief" >"$WF_TMP/ask-answer"
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Ask.php
	git add app/Services/Ask.php
	commit 'Add the ask service'
	say ready 'merge-ready'
	done_json
	;;
widen)
	if [ "$attempt" = 1 ]; then
		mem ask 'the change needs app/Services/Wide2.php, which Files omits' >"$WF_TMP/widen-id"
		say blocked "asked $(cat "$WF_TMP/widen-id")"
		done_json
		exit 0
	fi
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Widen.php
	printf '<?php\n' >app/Services/Wide2.php
	git add app/Services/Widen.php app/Services/Wide2.php
	commit 'Add the widened service'
	say ready 'merge-ready'
	done_json
	;;
moot)
	# Asks, then finds its own way and finishes anyway.
	mem ask 'is the fixture basket the right one?' >"$WF_TMP/moot-id"
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Moot.php
	git add app/Services/Moot.php
	commit 'Add the moot service'
	say ready 'merge-ready'
	done_json
	;;
stuck)
	say blocked 'waiting on a sibling'
	done_json
	;;
t1)
	mkdir -p app/Services
	printf '<?php\n' >app/Services/T1.php
	git add app/Services/T1.php
	commit 'Add the t1 service'
	say ready 'merge-ready'
	done_json
	;;
hold)
	if [ "$attempt" = 1 ]; then
		mem ask 'does an unanswered question hold its wave open?' >"$WF_TMP/hold-id"
		say blocked "asked $(cat "$WF_TMP/hold-id")"
		done_json
		exit 0
	fi
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Hold.php
	git add app/Services/Hold.php
	commit 'Add the hold service'
	say ready 'merge-ready'
	done_json
	;;
afterhold)
	mkdir -p app/Services
	printf '<?php\n' >app/Services/AfterHold.php
	git add app/Services/AfterHold.php
	commit 'Add the afterhold service'
	say ready 'merge-ready'
	done_json
	;;
pause)
	mem ask 'does a signal reach a task with a question still open?' >"$WF_TMP/pause-id"
	say blocked "asked $(cat "$WF_TMP/pause-id")"
	done_json
	;;
twice)
	if [ "$attempt" = 1 ]; then
		mem ask 'first question: does the wave wait on this one?' >"$WF_TMP/twice-id-1"
		say blocked "asked $(cat "$WF_TMP/twice-id-1")"
		done_json
		exit 0
	fi
	if [ "$attempt" = 2 ]; then
		mem ask 'second question: does the wave still wait, and say so?' >"$WF_TMP/twice-id-2"
		say blocked "asked $(cat "$WF_TMP/twice-id-2")"
		done_json
		exit 0
	fi
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Twice.php
	git add app/Services/Twice.php
	commit 'Add the twice service'
	say ready 'merge-ready'
	done_json
	;;
aftertwice)
	mkdir -p app/Services
	printf '<?php\n' >app/Services/AfterTwice.php
	git add app/Services/AfterTwice.php
	commit 'Add the aftertwice service'
	say ready 'merge-ready'
	done_json
	;;
cite)
	if [ "$attempt" = 1 ]; then
		# The blocked line quotes a ruling id before it names the question:
		# both are eight characters, only the second is answerable.
		mem ask 'the ruling leaves two readings open; which one?' >"$WF_TMP/cite-id"
		say blocked "ruling #RUL1NGID stands; asked $(cat "$WF_TMP/cite-id")"
		done_json
		exit 0
	fi
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Cite.php
	git add app/Services/Cite.php
	commit 'Add the cite service'
	say ready 'merge-ready'
	done_json
	;;
quiet)
	if [ "$attempt" = 1 ]; then
		# Ends its turn on the ask itself, with work on the branch and no
		# blocked line: the question is mem's to name.
		mkdir -p app/Services
		printf '<?php\n' >app/Services/Quiet.php
		git add app/Services/Quiet.php
		commit 'Start the quiet service'
		say progress 'half way'
		mem ask 'may I finish this the way the Done line reads?' >"$WF_TMP/quiet-id"
		done_json
		exit 0
	fi
	mkdir -p app/Services
	printf '<?php\n// finished\n' >app/Services/Quiet.php
	git add app/Services/Quiet.php
	commit 'Finish the quiet service'
	say ready 'merge-ready'
	done_json
	;;
reask)
	if [ "$attempt" = 1 ]; then
		mem ask 'reask: which spelling of the handle?' >"$WF_TMP/reask-id"
		say blocked "asked $(cat "$WF_TMP/reask-id")"
		done_json
		exit 0
	fi
	if [ "$attempt" = 2 ]; then
		# Blocks again naming the id already answered: the answer goes back
		# in, and nobody is woken over a settled question.
		say blocked "still asked $(cat "$WF_TMP/reask-id")"
		done_json
		exit 0
	fi
	mkdir -p app/Services
	printf '<?php\n' >app/Services/Reask.php
	git add app/Services/Reask.php
	commit 'Add the reask service'
	# The ids it acted on, cited as provenance in a report that is not blocked.
	say progress "acting on $(cat "$WF_TMP/reask-id")"
	say ready "done per $(cat "$WF_TMP/reask-id")"
	done_json
	;;
afterreask)
	mkdir -p app/Services
	printf '<?php\n' >app/Services/AfterReask.php
	git add app/Services/AfterReask.php
	commit 'Add the afterreask service'
	say ready 'merge-ready'
	done_json
	;;
ghost)
	# Never asks; the id in the note is a friction id, not a question mem
	# will ever list for this task.
	say blocked 'cannot proceed, see friction #GH0STGH0'
	done_json
	;;
afterghost)
	mkdir -p app/Services
	printf '<?php\n' >app/Services/AfterGhost.php
	git add app/Services/AfterGhost.php
	commit 'Add the afterghost service'
	say ready 'merge-ready'
	done_json
	;;
esac
FAKE

export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief}'"'"' > {out} 2> {err} &'

new_repo app
export WF_MAIN="$PWD"
mem_register
mkdir -p app/Services
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: ask-check

- [ ] ask Stop on a question and act on the answer
      Files: app/Services/Ask.php
      Verify: true
- [ ] widen Need a file the Files line omits
      Files: app/Services/Widen.php
      Verify: true
- [ ] moot Ask and finish anyway
      Files: app/Services/Moot.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/ask-check"

# The orchestrator, played by a loop: answer each worker question as it
# appears, and widen the plan of record for the one that needs a file.
(
	answered_ask=""
	answered_widen=""
	while [ ! -f "$T_TMP/run-done" ]; do
		if [ -z "$answered_ask" ] && [ -s "$WF_TMP/ask-id" ]; then
			id=$(sed 's/^#//' "$WF_TMP/ask-id")
			"$MEM_BIN" answer "$id" 'yes, go ahead: the fixture is yours to add' >/dev/null 2>&1 &&
				answered_ask=1
		fi
		if [ -z "$answered_widen" ] && [ -s "$WF_TMP/widen-id" ]; then
			"$MEM_BIN" plan --stdin >/dev/null <<'PLAN'
# plan: ask-check

- [ ] ask Stop on a question and act on the answer
      Files: app/Services/Ask.php
      Verify: true
- [ ] widen Need a file the Files line omits
      Files: app/Services/Widen.php app/Services/Wide2.php
      Verify: true
- [ ] moot Ask and finish anyway
      Files: app/Services/Moot.php
      Verify: true
PLAN
			id=$(sed 's/^#//' "$WF_TMP/widen-id")
			"$MEM_BIN" answer "$id" 'widened: the plan now lists Wide2.php on your Files line' >/dev/null 2>&1 &&
				answered_widen=1
		fi
		sleep 0.3
	done
) &
answerer=$!

export WORKFLOW_MAX_WORKERS=3 WORKFLOW_DEADLINE_MIN=0.2
run workflow run
run_rc=$RC
run_out=$OUT
touch "$T_TMP/run-done"
wait "$answerer" 2>/dev/null

is "$run_rc" 0 'every task merged in one run, questions and all'

## --------------------------------------------------- the worker's question

is "$(cat "$WF_TMP/ask-task" 2>/dev/null)" ask-check/ask \
	'WORKFLOW_TASK names the task in the worker environment'
run_out "$MEM_BIN" questions --for orchestrator --json
ask_row=$(printf '%s' "$OUT" | python3 -c '
import json,sys
for q in json.load(sys.stdin)["questions"]:
    if q["task"] == "ask-check/ask": print(q["audience"], q["answered"], q["answer"])')
is "$ask_row" 'orchestrator True yes, go ahead: the fixture is yours to add' \
	'a question asked from the worktree is the orchestrator'"'"'s, tagged with its task, and carries its answer'
run_out "$MEM_BIN" questions --pending --all-projects --for human --json
is "$(printf '%s' "$OUT" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["questions"]))')" 0 \
	'nothing here was ever a person'"'"'s to answer'

## ------------------------------------------------ the answer reaches the worker

is "$(cat "$rundir/ask.dispatches")" 2 'ask went again after its answer landed, inside the same run'
like "$run_out" 'task ask: #[A-Z0-9]+ was answered -- dispatched again with the answer' \
	'the run says it dispatched on the answer'
like "$(cat "$WF_TMP/ask-answer")" \
	'The orchestrator answered: yes, go ahead: the fixture is yours to add' \
	'the second attempt read the answer in its brief'
is "$(cat "$rundir/ask.state")" merged 'and merged'
like "$(cat "$rundir/ask.failed" 2>/dev/null; "$MEM_BIN" log --limit 40)" \
	'failed ask -- asked #[A-Z0-9]+: may I add the fixture' \
	'while it waited, the failure note named the question'

## ------------------------------------------ the plan of record, read live

is "$(cat "$rundir/widen.state")" merged 'widen merged once the plan of record carried the second file'
truthy "$(git -C "$WF_MAIN" cat-file -e integration/ask-check:app/Services/Wide2.php 2>/dev/null && echo 0 || echo 1)" \
	'the file the answer added to Files is on the integration branch'

## ------------------------------------------------------- a moot question

is "$(cat "$rundir/moot.state")" merged 'moot merged on its own'
run_out "$MEM_BIN" questions --for orchestrator --json
moot_row=$(printf '%s' "$OUT" | python3 -c '
import json,sys
for q in json.load(sys.stdin)["questions"]:
    if q["task"] == "ask-check/moot": print(q["answer"])')
like "$moot_row" '^moot: moot merged' 'the run answered the question its task no longer needs'

## ------------------------------------------ stopping short is a report, not a question

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: report-check

- [ ] t1 Add a service
      Files: app/Services/T1.php
      Verify: true
- [ ] stuck Report blocked and stop
      Files: app/Services/Stuck.php
      Verify: true
EOF
run workflow run
is "$RC" 1 'a task that blocked without asking fails the run'
like "$OUT" 'Plan report-check stopped short: 1 of 2 merged, 1 failed' 'the report is on stderr'
unlike "$OUT" 'What should happen' 'and it asks nothing'
run_out "$MEM_BIN" questions --pending --all-projects --json
is "$(printf '%s' "$OUT" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["questions"]))')" 0 \
	'no question was raised for the stop'
run_out "$MEM_BIN" log --limit 5
like "$OUT" 'Plan report-check stopped short' 'the report went to the log instead'

## ------------------------------------------ a wave waits on a question

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: hold-check

- [ ] hold Ask and hold its wave open, alone
      Files: app/Services/Hold.php
      Verify: true
- [ ] afterhold Comes after the wave that holds [after: hold]
      Files: app/Services/AfterHold.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/hold-check"
workflow run >"$T_TMP/hold.log" 2>&1 &
runpid=$!

for _ in $(seq 1 100); do
	[ "$(cat "$rundir/hold.state" 2>/dev/null)" = failed ] && break
	sleep 0.2
done
is "$(cat "$rundir/hold.state" 2>/dev/null)" failed 'hold stopped on its question, alone in its wave'

# More than three polls' worth of waiting, unanswered.
sleep 4
is "$(cat "$rundir/afterhold.state" 2>/dev/null)" pending \
	'the wave after it has not opened while the question sits unanswered'
is "$(grep -c 'waiting on #' "$T_TMP/hold.log")" 1 \
	'the run says the wave stays open once, not on every poll'

id=$(sed 's/^#//' "$WF_TMP/hold-id")
"$MEM_BIN" answer "$id" 'yes: an open question holds the wave open' >/dev/null 2>&1

wait "$runpid"
is "$?" 0 'the run finishes once the answer lands'
is "$(cat "$rundir/hold.state")" merged 'hold merged on its second attempt'
is "$(cat "$rundir/afterhold.state")" merged 'and the wave after it went on to merge too'

## -------------------------------- stopping short while waiting ends cleanly

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: pause-check

- [ ] pause Ask and wait for a signal, not an answer
      Files: app/Services/Pause.php
      Verify: true
- [ ] afterpause Comes after the wave that pauses [after: pause]
      Files: app/Services/AfterPause.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/pause-check"
workflow run >"$T_TMP/pause.log" 2>&1 &
runpid=$!

for _ in $(seq 1 100); do
	[ "$(cat "$rundir/pause.state" 2>/dev/null)" = failed ] && break
	sleep 0.2
done
is "$(cat "$rundir/pause.state" 2>/dev/null)" failed 'pause stopped on its question, alone in its wave'

kill -TERM "$runpid"
for _ in $(seq 1 100); do
	kill -0 "$runpid" 2>/dev/null || break
	sleep 0.2
done
run kill -0 "$runpid"
isnt "$RC" 0 'the coordinator is gone'
wait "$runpid" 2>/dev/null
is "$?" 1 'a stop while only waiting on an answer ends the run cleanly, not hung'

## ------------------------ an answered id in a later report is not a new ask

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: reask-check

- [ ] reask Ask once, cite the id ever after
      Files: app/Services/Reask.php
      Verify: true
- [ ] afterreask Comes after the one that cites [after: reask]
      Files: app/Services/AfterReask.php
      Verify: true
EOF
rundir="$XDG_STATE_HOME/workflow/runs/app/reask-check"
workflow run >"$T_TMP/reask.log" 2>&1 &
runpid=$!
for _ in $(seq 1 100); do
	[ -s "$WF_TMP/reask-id" ] && grep -q 'waiting on' "$T_TMP/reask.log" 2>/dev/null && break
	sleep 0.2
done
reask_id=$(sed 's/^#//' "$WF_TMP/reask-id")
"$MEM_BIN" answer "$reask_id" 'the lower-case one' >/dev/null 2>&1
wait "$runpid"
is "$?" 0 'the run merges once the one answer is in'
is "$(cat "$rundir/reask.state")" merged 'reask merged'
is "$(cat "$rundir/reask.dispatches")" 3 'on its third attempt: ask, block again on the same id, work'
is "$(grep -c ' question reask ' "$rundir/events")" 1 'one question event, for the one time it was open'
like "$(cat "$T_TMP/reask.log")" "task reask: re-asked #$reask_id: .* -- already answered, the answer goes back in" \
	'the second block on the answered id is said for what it is, and wakes nobody'
unlike "$(cat "$T_TMP/reask.log")" 'stopped short' 'and the run went on by itself'

## -------------------------------- a second question is named too, not just the first

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: twice-check

- [ ] twice Ask, get answered, then ask something else
      Files: app/Services/Twice.php
      Verify: true
- [ ] aftertwice Comes after the wave that asks twice [after: twice]
      Files: app/Services/AfterTwice.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/twice-check"
workflow run >"$T_TMP/twice.log" 2>&1 &
runpid=$!

for _ in $(seq 1 100); do
	[ -s "$WF_TMP/twice-id-1" ] && break
	sleep 0.2
done
id1=$(sed 's/^#//' "$WF_TMP/twice-id-1")
for _ in $(seq 1 100); do
	grep -q "waiting on #$id1" "$T_TMP/twice.log" 2>/dev/null && break
	sleep 0.2
done
is "$(grep -c "waiting on #$id1" "$T_TMP/twice.log")" 1 'the first question is named once'

"$MEM_BIN" answer "$id1" 'yes: named, and dispatched again' >/dev/null 2>&1

for _ in $(seq 1 100); do
	[ -s "$WF_TMP/twice-id-2" ] && break
	sleep 0.2
done
id2=$(sed 's/^#//' "$WF_TMP/twice-id-2")
for _ in $(seq 1 100); do
	grep -q "waiting on #$id2" "$T_TMP/twice.log" 2>/dev/null && break
	sleep 0.2
done
is "$(grep -c "waiting on #$id2" "$T_TMP/twice.log")" 1 \
	'the second question is named too, not swallowed by the first one'"'"'s guard'

"$MEM_BIN" answer "$id2" 'yes: the second answer lands too' >/dev/null 2>&1

wait "$runpid"
is "$?" 0 'the run finishes once both answers land'
is "$(cat "$rundir/twice.state")" merged 'twice merged on its third attempt'
is "$(cat "$rundir/aftertwice.state")" merged 'and the wave after it went on to merge too'

## ------------------- the question in the note, and a turn that ends on one

# Two ids of the same shape in one blocked line: the run keys on the one mem
# lists as a question for the task, not on whichever was written first
# (#SYHHNK5T). And a worker that ends its turn on `mem ask` with no blocked
# line at all has asked all the same (#NNVWGXZ4).
"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: asknote-check

- [ ] cite Blocked with a ruling id quoted beside the question
      Files: app/Services/Cite.php
      Verify: true
- [ ] quiet Ends its turn on the ask, with no blocked line
      Files: app/Services/Quiet.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/asknote-check"
workflow run >"$T_TMP/asknote.log" 2>&1 &
runpid=$!

# Answered only once the run has recorded what it is waiting on: an answer
# that lands first would settle the question before the note is read.
for _ in $(seq 1 150); do
	[ "$(cat "$rundir/cite.state" 2>/dev/null)" = failed ] &&
		[ "$(cat "$rundir/quiet.state" 2>/dev/null)" = failed ] && break
	sleep 0.2
done
cite_id=$(sed 's/^#//' "$WF_TMP/cite-id")
quiet_id=$(sed 's/^#//' "$WF_TMP/quiet-id")
like "$(cat "$rundir/cite.failed" 2>/dev/null)" "^asked #$cite_id" \
	'the question mem lists is what cite waits on, not the ruling id its note quoted first'
unlike "$(cat "$rundir/cite.failed" 2>/dev/null)" 'asked #RUL1NGID' \
	'the ruling id is never taken for a question'
like "$(cat "$rundir/quiet.failed" 2>/dev/null)" "^asked #$quiet_id" \
	'a turn that ended on mem ask without a blocked line is a question too'

"$MEM_BIN" answer "$cite_id" 'the first reading is the one' >/dev/null 2>&1
"$MEM_BIN" answer "$quiet_id" 'yes: finish it as the Done line reads' >/dev/null 2>&1

wait "$runpid"
is "$?" 0 'the run finishes once both of these answers land too'
is "$(cat "$rundir/cite.state")" merged 'cite merged on its second attempt'
is "$(cat "$rundir/quiet.state")" merged \
	'and the answer continued quiet rather than a fresh worker starting over'

## ------------------------------ an id mem never lists does not wait forever

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: ghost-check

- [ ] ghost Blocks on a friction id, never a question
      Files: app/Services/Ghost.php
      Verify: true
- [ ] afterghost Comes after the wave that never resolves [after: ghost]
      Files: app/Services/AfterGhost.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/ghost-check"
run workflow run
is "$RC" 1 'a question mem never lists does not hold the wave open forever'
like "$OUT" 'Plan ghost-check stopped short: 0 of 2 merged, 1 failed, 1 never started' \
	'the run reports the stop instead of hanging'
is "$(cat "$rundir/ghost.state")" failed 'ghost stays failed, its id never answerable'
is "$(cat "$rundir/afterghost.state" 2>/dev/null)" blocked 'the wave after it never opens'
like "$OUT" 'task ghost: #GH0STGH0 is not a question mem lists for this task -- the run ends; answer it and run again' \
	'the run says the id is unanswerable and what to do about it'
unlike "$OUT" 'waiting on #GH0STGH0' \
	'and never promises to stay open for an id mem has not listed once'
