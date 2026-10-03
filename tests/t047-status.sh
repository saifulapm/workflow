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
printf 'failed\n' >"$rundir/t2.state"
printf 'the suite is red once the change sits on integration\n' >"$rundir/t2.failed"
printf '2\n' >"$rundir/t2.dispatches"
printf '4000\n' >"$rundir/t2.context"
printf '2026-08-21T10:00:00Z blocked waiting on an answer\n' >"$rundir/t2.status"
printf 'pending\n' >"$rundir/t3.state"
printf 'waiting for a worker slot (2 of 2 running)\n' >"$rundir/t3.held"
printf 'dispatched\n' >"$rundir/t4.state"
printf '2026-08-21T10:05:00Z ready: done\n' >"$rundir/t4.status"

run workflow status
is "$RC" 0 'status exits 0 with runs to report'
like "$OUT" 'demo' 'the plan is named'
like "$OUT" 't1 +merged' 'a merged task shows its state'
like "$OUT" 't2 +failed +the suite is red' 'a failed task shows its reason'
like "$OUT" 't3 +pending +waiting for a worker slot \(2 of 2 running\)' 'a pending task says what holds it (friction #XR01M9H0)'
like "$OUT" 'carried 158k' 'the context a task carried is plan-sizing feedback'
unlike "$OUT" '\$' 'and no dollar figure is reported at all'
like "$OUT" 'last report: ready done' 'a status line punctuated ready: reports the state bare'

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
unlike "$OUT" '"reviews"' 'no task carries a reviews count'
like "$OUT" '"held": *"waiting for a worker slot \(2 of 2 running\)"' 'json carries what holds a pending task'
unlike "$OUT" '"spend"' 'and no spend field'
like "$OUT" '"live": *false' 'nobody holds the run lock'
unlike "$OUT" '"readings"|"fixes"' 'and no run carries readings or fixes'
like "$OUT" '"context": *162502' 'json sums context for the run, distinct from any one task'

# The serve fields, in a checkout serve has never touched: idle, no
# milestone, nothing parked, no runner, not paused.
is "$(printf '%s' "$OUT" | jq -r '.stage')" idle 'no serve state and no live run is stage idle'
is "$(printf '%s' "$OUT" | jq -c '.milestone')" null 'no roadmap is no milestone'
is "$(printf '%s' "$OUT" | jq -c '.parked')" '[]' 'nothing parked'
is "$(printf '%s' "$OUT" | jq -c '.findings')" 0 'no open findings'
is "$(printf '%s' "$OUT" | jq -c '.runner')" null 'no runner'
is "$(printf '%s' "$OUT" | jq -c '.paused')" false 'not paused'

# The same fields once serve, mem and a lead have each said something.
printf '# roadmap: road\n\n- [x] m0 The groundwork\n- [ ] demo The demo\n- [ ] m2 The rest\n' |
	"$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" project set runner here >/dev/null
"$MEM_BIN" project set paused 'here 2026-10-04' >/dev/null
"$MEM_BIN" finding add --milestone demo --step 1 'the cart total is off by one' >/dev/null
printf 'the owner decides the slot count\n' >"$rundir/t3.parked"
mkdir -p "$XDG_STATE_HOME/workflow/serve/app"
printf 'waiting\n' >"$XDG_STATE_HOME/workflow/serve/app/stage"
run workflow status --json
is "$(printf '%s' "$OUT" | jq -r '.stage')" waiting 'stage is the serve stage file'
is "$(printf '%s' "$OUT" | jq -cS '.milestone')" '{"m":3,"n":2,"slug":"demo"}' 'milestone is the first unticked one, n of m'
is "$(printf '%s' "$OUT" | jq -cS '.parked')" '[{"reason":"the owner decides the slot count","task":"t3"}]' 'a parked task with its reason'
is "$(printf '%s' "$OUT" | jq -c '.findings')" 1 'open findings counted'
is "$(printf '%s' "$OUT" | jq -r '.runner')" here 'runner is the project key'
is "$(printf '%s' "$OUT" | jq -c '.paused')" true 'paused is the key set'
run workflow status
like "$(printf '%s\n' "$OUT" | head -n 1)" '^stage: waiting · milestone demo \(2 of 3\) · runner here · 1 parked$' 'the human report opens with the serve line'
rm -f "$rundir/t3.parked" "$XDG_STATE_HOME/workflow/serve/app/stage"
"$MEM_BIN" project unset paused >/dev/null

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
