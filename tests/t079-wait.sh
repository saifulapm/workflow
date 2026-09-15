#!/usr/bin/env bash
# `workflow wait`: the session that owns a run blocks on the run's events
# file instead of sleeping on a clock, and the exit code says what woke it --
# 2 a question, 1 a task failed for good, 4 a merge under --merges, 0 the
# run ended or none is live, 3 a timeout. A cursor keeps a line from being
# reported twice; a fix round the run handles itself is never an event.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# ask asks once and finishes on its second attempt; bad reports blocked
# with no question, which is failed for good; hold stays alive until
# released so the run is live for every wait below; the rest just merge.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
say started
case $task in
ask)
	if [ ! -f "$WF_TMP/asked" ]; then
		mem ask 'which word?' >"$WF_TMP/ask-id"
		: >"$WF_TMP/asked"
		say blocked "asked $(cat "$WF_TMP/ask-id")"
		printf '{"is_error":false,"result":"ok"}\n'
		exit 0
	fi
	;;
bad)
	say blocked 'cannot see how'
	printf '{"is_error":false,"result":"ok"}\n'
	exit 0
	;;
hold)
	while [ ! -f "$WF_TMP/release-hold" ]; do sleep 0.2; done
	;;
esac
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
commit "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

run workflow wait
is "$RC" 0 'with no run live, wait returns at once'
like "$OUT" 'no run is live here' 'and says so'

cat >"$T_TMP/plan.md" <<'PLAN'
# plan: waited

- [ ] hold Stay alive until released
      Files: app/hold.php
      Verify: true
- [ ] ask Add the ask service
      Files: app/ask.php
      Verify: true
- [ ] bad Add the bad service
      Files: app/bad.php
      Verify: true
- [ ] t1 Add the t1 service  [after: ask]
      Files: app/t1.php
      Verify: true
PLAN
rundir="$XDG_STATE_HOME/workflow/runs/app/waited"
env WORKFLOW_MAX_WORKERS=3 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/plan.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!
for _ in $(seq 1 100); do
	[ -f "$rundir/hold.state" ] && break
	sleep 0.1
done

## ------------------------------------------------ the question wakes it

run timeout 60 workflow wait
is "$RC" 2 'the first wait returns on the question'
qid=$(cat "$WF_TMP/ask-id")
like "$OUT" "^[0-9T:Z-]+ question ask -- asked $qid: which word\\?" 'naming the task and the question'
like "$OUT" "failed bad -- the worker's last report was 'blocked: cannot see how'" \
	'and the task that failed for good, which came in the same window'
unlike "$OUT" 'merged' 'a merge alone is not reported without --merges'
is "$(cat "$rundir/wait.cursor")" "$(wc -c <"$rundir/events")" 'the cursor stands at the end of what was read'

run timeout 3 workflow wait --timeout 1
is "$RC" 3 'nothing new: the next wait times out'
is "$OUT" '' 'and repeats nothing'

## ------------------------------------------------- merges, on request

"$MEM_BIN" answer "$qid" 'that one' >/dev/null
run timeout 60 workflow wait --merges
is "$RC" 4 'with --merges, the next merge wakes it'
like "$OUT" ' merged (ask|t1)$' 'naming the task'
unlike "$OUT" 'question' 'the question is not reported again'

## ------------------------------------------------------- the ending

: >"$WF_TMP/release-hold"
run timeout 60 workflow wait
is "$RC" 0 'the run ending wakes it with exit 0'
like "$OUT" ' ended 3 merged, 1 failed, 0 never started, 3 readings, 0 fix verdicts, 0 context$' \
	'with the tally, readings, fix verdicts and context'
wait "$runpid"
like "$(cat "$rundir/events")" '^[0-9T:Z-]+ question ask' 'the events file opens with the question'
is "$(grep -c ' merged ' "$rundir/events")" 3 'carries every merge'
is "$(grep -c ' failed ' "$rundir/events")" 1 'the one final failure'
is "$(grep -c ' ended ' "$rundir/events")" 1 'and the ending once'

run workflow wait
is "$RC" 0 'and with the run gone, wait is back to returning at once'

t_done
