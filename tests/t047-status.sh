#!/usr/bin/env bash
# workflow status: the run dir read out loud, for the session that owns a run.
# It reads and never touches -- no dispatch, no cleanup, no state change.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

# A run dir as a stopped run leaves it: one merged, one parked with a reason,
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
EOF
printf 'abc123\n' >"$rundir/base_sha"
printf 't1\t0.25\nt2\t0.50\n' >"$rundir/costs.tsv"
printf 'merged\n' >"$rundir/t1.state"
printf 'deadbeef\n' >"$rundir/t1.merged"
printf 'parked\n' >"$rundir/t2.state"
printf 'the suite is red once the change sits on integration\n' >"$rundir/t2.parked"
printf '2\n' >"$rundir/t2.dispatches"
printf '2026-08-21T10:00:00Z blocked waiting on an answer\n' >"$rundir/t2.status"
printf 'pending\n' >"$rundir/t3.state"

run workflow status
is "$RC" 0 'status exits 0 with runs to report'
like "$OUT" 'demo' 'the plan is named'
like "$OUT" 't1 +merged' 'a merged task shows its state'
like "$OUT" 't2 +parked +the suite is red' 'a parked task shows its reason'
like "$OUT" 't3 +pending' 'a task that never started says so'
like "$OUT" '\$0.75' 'spend is totalled from costs.tsv'

run workflow status --json
is "$RC" 0 'status --json exits 0'
like "$OUT" '"plan": *"demo"' 'json names the plan'
like "$OUT" '"state": *"parked"' 'json carries task states'
like "$OUT" '"parked": *"the suite is red once the change sits on integration"' 'json carries the park reason'
like "$OUT" '"last_status": *"blocked waiting on an answer"' "json carries the worker's own last report"
like "$OUT" '"spend": *0.75' 'json totals spend'
like "$OUT" '"live": *false' 'nobody holds the run lock'

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
