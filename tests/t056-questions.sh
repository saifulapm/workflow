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
