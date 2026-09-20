#!/usr/bin/env bash
# workflow status: the run dir read out loud, for the session that owns a run.
# It reads and never touches -- no dispatch, no cleanup, no state change.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

# A run dir as a stopped run leaves it: one merged, one failed with a reason,
# one never started.
rundir="$XDG_STATE_HOME/workflow/runs/app/demo"
mkdir -p "$rundir"
cat >"$rundir/plan.md" <<'EOF'
# plan: demo

- [ ] t1 First service
      Files: app/t1.php
      Verify: true
- [ ] t2 Second service
      Files: app/t2.php
      Verify: true
- [ ] t3 Third service
      Files: app/t3.php
      Verify: true
- [ ] t4 Fourth service
      Files: app/t4.php
      Verify: true
EOF
printf 'abc123\n' >"$rundir/base_sha"
printf 'merged\n' >"$rundir/t1.state"
printf 'deadbeef\n' >"$rundir/t1.merged"
printf '158502\n' >"$rundir/t1.context"
printf '2\n' >"$rundir/t1.reviews"
printf 'failed\n' >"$rundir/t2.state"
printf 'the suite is red once the change sits on integration\n' >"$rundir/t2.failed"
printf '2\n' >"$rundir/t2.dispatches"
printf '4000\n' >"$rundir/t2.context"
printf '2026-08-21T10:00:00Z blocked waiting on an answer\n' >"$rundir/t2.status"
printf 'pending\n' >"$rundir/t3.state"
printf 'dispatched\n' >"$rundir/t4.state"
printf '2026-08-21T10:05:00Z ready: done\n' >"$rundir/t4.status"

run workflow status
is "$RC" 0 'status exits 0 with runs to report'
like "$OUT" 'demo' 'the plan is named'
like "$OUT" 't1 +merged' 'a merged task shows its state'
like "$OUT" 't2 +failed +the suite is red' 'a failed task shows its reason'
like "$OUT" 't3 +pending' 'a task that never started says so'
like "$OUT" 'carried 158k' 'the context a task carried is plan-sizing feedback'
unlike "$OUT" '\$' 'and no dollar figure is reported at all'
like "$OUT" 'last report: ready done' 'a status line punctuated ready: reports the state bare'
like "$OUT" 't1 +merged +2 ' 'the fix column shows the reviews a task carried'

# A task that merged after a refused first attempt keeps its .failed text on
# disk; status shows the report, not the stale reason.
printf 'merged\n' >"$rundir/t1.state"
printf 'wrote outside its Files: patterns -- M|pnpm-lock.yaml\n' >"$rundir/t1.failed"
run workflow status
unlike "$OUT" 't1 +merged +.*wrote outside its Files' 'a merged task does not show the reason its first attempt failed for'
like "$OUT" 't2 +failed +the suite is red' 'a failed task still does'
printf '%s\n' "$(($(date +%s) - 720))" >"$rundir/t2.dispatched_at"
run workflow status --brief
is "$RC" 0 'status --brief exits 0'
like "$OUT" '^  t1 +merged +-$' 'brief: one line per task, state and age, a task never dispatched shows -'
like "$OUT" '^  t2 +failed +12m$' 'and minutes since the last dispatch'
unlike "$OUT" 'the suite is red|last report|carried' 'with no reason, report or context'

run workflow status --json
is "$RC" 0 'status --json exits 0'
like "$OUT" '"plan": *"demo"' 'json names the plan'
like "$OUT" '"state": *"failed"' 'json carries task states'
like "$OUT" '"failed": *"the suite is red once the change sits on integration"' 'json carries the failure reason'
like "$OUT" '"last_status": *"blocked waiting on an answer"' "json carries the worker's own last report"
like "$OUT" '"last_status": *"ready done"' 'json reports ready for a status line punctuated ready:'
like "$OUT" '"context": *158502' 'json carries the context a task carried'
like "$OUT" '"reviews": *2' 'json carries the fix verdicts a task carried'
unlike "$OUT" '"spend"' 'and no spend field'
like "$OUT" '"live": *false' 'nobody holds the run lock'
like "$OUT" '"readings": *3' 'json sums readings for the run: 2 fix verdicts plus 1 task read to a merge'
like "$OUT" '"fixes": *2' 'json sums the fix verdicts for the run'
like "$OUT" '"context": *162502' 'json sums context for the run, distinct from any one task'

# A held lock is a live orchestrator.
if command -v flock >/dev/null 2>&1; then
	flock "$rundir/lock" sleep 3 &
	locker=$!
	sleep 0.3
	run workflow status --json
	like "$OUT" '"live": *true' 'a held lock reports live'
	kill "$locker" 2>/dev/null
	wait "$locker" 2>/dev/null
fi

# Outside any checkout there is no project to report on.
cd "$T_TMP"
run workflow status
is "$RC" 2 'status outside a checkout exits 2'

# The help names it.
run workflow help
like "$OUT" '  status' 'help lists status'
