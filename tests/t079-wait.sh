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
hold | hold2)
	while [ ! -f "$WF_TMP/release-$task" ]; do sleep 0.2; done
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
like "$OUT" ' ended 3 merged, 1 failed, 0 never started, 0 readings, 0 fix verdicts, 0 context$' \
	'with the tally, and no reader means no readings or fix verdicts'
wait "$runpid"
like "$(cat "$rundir/events")" '^[0-9T:Z-]+ question ask' 'the events file opens with the question'
is "$(grep -c ' merged ' "$rundir/events")" 3 'carries every merge'
is "$(grep -c ' failed ' "$rundir/events")" 1 'the one final failure'
is "$(grep -c ' ended ' "$rundir/events")" 1 'and the ending once'

run workflow wait
is "$RC" 0 'and with the run gone, wait is back to returning at once'

## ------------------------------------- a new run starts the cursor afresh

# The run dir is keyed by the plan, so a second run appends to the first
# run's events file -- and its first wait used to return at once on the last
# run's 'ended' or 'failed' line, while the live run was still dispatching
# (frictions #RFKBV9GY, #Y1Q0H852, #WBMMFJ3Y, #9TJ759K3). The cursor is
# stamped past what is already there when the run takes the lock.
cat >"$T_TMP/plan2.md" <<'PLAN'
# plan: waited

- [x] hold Stay alive until released
      Files: app/hold.php
      Verify: true
- [x] ask Add the ask service
      Files: app/ask.php
      Verify: true
- [x] t1 Add the t1 service  [after: ask]
      Files: app/t1.php
      Verify: true
- [ ] hold2 Stay alive until released too
      Files: app/hold2.php
      Verify: true
PLAN
env WORKFLOW_MAX_WORKERS=3 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/plan2.md" >"$T_TMP/run2.log" 2>&1 &
run2pid=$!
for _ in $(seq 1 300); do
	[ "$(cat "$rundir/hold2.state" 2>/dev/null)" = dispatched ] && break
	sleep 0.2
done
is "$(cat "$rundir/hold2.state" 2>/dev/null)" dispatched 'the second run is live with a worker out'

run timeout 30 workflow wait --timeout 1
is "$RC" 3 'the first wait of a second run blocks rather than replaying'
is "$OUT" '' 'and the run before it is behind the cursor, ending and all'

: >"$WF_TMP/release-hold2"
wait "$run2pid"
is "$(grep -c ' ended ' "$rundir/events")" 2 'both runs wrote their ending to the one events file'

## --------------------------------------- a run still inside the trunk gate

# `setup` writes `started` before the trunk gate and `plan.md` only once the
# gate is green, so for the whole of that suite the held lock was invisible
# here and wait said no run was live (friction #KBPVJF24).
if command -v flock >/dev/null 2>&1; then
	gating="$XDG_STATE_HOME/workflow/runs/app/gating"
	mkdir -p "$gating"
	printf '%s\n' "$(date +%s)" >"$gating/started"
	flock "$gating/lock" sleep 30 &
	locker=$!
	# Held when this can no longer take it, however loaded the machine is.
	for _ in $(seq 1 300); do
		flock -n "$gating/lock" true 2>/dev/null || break
		sleep 0.1
	done
	run timeout 30 workflow wait --timeout 1
	is "$RC" 3 'a run in the trunk gate, before plan.md, is waited on by its lock alone'
	unlike "$OUT" 'no run is live here' 'not called nothing at all'
	kill "$locker" 2>/dev/null
	wait "$locker" 2>/dev/null || true
fi

t_done
