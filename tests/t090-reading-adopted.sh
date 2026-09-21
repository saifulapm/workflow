#!/usr/bin/env bash
# A task left mid-reading by a coordinator that died -- a power cut, a
# SIGKILL -- is read again by the next run, and never judged as a worker.
# Its worker finished before the outage and the machine took every record
# of that session with it, so asking the backend what its last turn came to
# answers "nothing"; failing the task on that throws away a merge already
# applied to integration, with its diff still unread (ebdify m1's admin,
# 2026-09-21).
source "$(dirname -- "$0")/lib.sh"
t_init

unset WORKFLOW_REVIEW_MODEL
export WF_TMP="$T_TMP"

# The worker commits and reports ready. The reader hangs on its first
# reading of t1, so the run can be killed with the reading in flight, and
# ships on every reading after.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; status=$3; session=$4; brief=$5
case $task in
*-review)
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	printf '%s\n' "${task%-review}" >>"$WF_TMP/readings"
	if [ "${task%-review}" = t1 ] && [ ! -f "$WF_TMP/t1-read-once" ]; then
		: >"$WF_TMP/t1-read-once"
		sleep 300
	fi
	printf 'VERDICT: ship\n' >"$answer"
	exit 0
	;;
esac
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
"$MEM_BIN" project set review-model fable >/dev/null

cat >"$T_TMP/cut.md" <<'PLAN'
# plan: cut

- [ ] t1 The one whose reading the outage cut
      Files: app/t1.php
      Verify: true
- [ ] side A second task, so the plan is worth a worker
      Files: app/side.php
      Verify: true
PLAN
rundir="$XDG_STATE_HOME/workflow/runs/app/cut"
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_DEADLINE_MIN=10

workflow run --plan-file "$T_TMP/cut.md" >"$T_TMP/cut1.log" 2>&1 &
runpid=$!
for _ in $(seq 1 300); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = reviewing ] && break
	sleep 0.2
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" reviewing 't1 is reviewing when the power goes'

# The outage. The coordinator dies where it stands, the reader with it, and
# the machine takes the record of t1's finished worker too -- which is what
# the backend is asked about on the way back up.
kill -9 "$runpid" 2>/dev/null
wait "$runpid" 2>/dev/null
pkill -f 'sleep 300' 2>/dev/null
rm -f "$rundir/t1.json" "$rundir/t1.pid"
sleep 1
is "$(cat "$rundir/t1.state")" reviewing 'the run dir still says reviewing'
truthy "$([ -s "$rundir/t1.merging" ] && echo 0 || echo 1)" 'with the merge it had already applied on record'

## ------------------------------------------------- the run after the outage

run workflow run --plan-file "$T_TMP/cut.md"
is "$RC" 0 'the next run finishes the plan'
like "$OUT" 'task t1: left mid-reading by a run that is gone -- reading it again' 'saying it reads the merge again'
unlike "$OUT" 'task t1: failed' 'never failing it on the worker that finished before the outage'
is "$(cat "$rundir/t1.state")" merged 't1 merges on the second reading'
is "$(cat "$rundir/t1.dispatches")" 1 'on the one worker it ever had'
is "$(grep -c '^t1$' "$WF_TMP/readings")" 2 'after two readings'
is "$(cat "$rundir/side.state")" merged 'and the task beside it merged'

t_done
