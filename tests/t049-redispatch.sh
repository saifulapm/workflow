#!/usr/bin/env bash
# `workflow redispatch` -- ask a live run to dispatch a parked task again.
# The run holds the project lock for its whole life, so a parked task used to
# wait for the run to end before anyone could act on it, with the worker slot
# it freed sitting idle (friction #W0S44DE6). A marker file in the run dir is
# how the request reaches a run nothing else can talk to.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# t1 works until it is released, heartbeating so it never reads as stalled.
# t2 reports blocked and parks -- until the go flag appears, when it does the
# job properly.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = t1 ]; then
	while [ ! -f "$WF_TMP/release-t1" ]; do
		say progress
		sleep 0.5
	done
elif [ ! -f "$WF_TMP/go-t2" ]; then
	say 'blocked waiting on a decision'
	printf '{"is_error":false,"result":"blocked"}\n'
	exit 0
fi
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

cat >"$T_TMP/plan.md" <<'EOF'
# plan: live

- [ ] t1 The long one
      Files: app/t1.php
      Verify: true
- [ ] t2 The one that parks
      Files: app/t2.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/live"

## ------------------------------------------- nothing live yet: refused

run workflow redispatch t2
is "$RC" 1 'with no live run, redispatch says so instead of pretending'
like "$OUT" 'no live run' 'and names the situation'

## ----------------------------------------------- the live-run round trip

env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/plan.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!

for _ in $(seq 1 100); do
	[ "$(cat "$rundir/t2.state" 2>/dev/null)" = parked ] && break
	sleep 0.2
done
is "$(cat "$rundir/t2.state" 2>/dev/null)" parked 't2 parked while t1 still runs'

run workflow redispatch t2
is "$RC" 0 'redispatch reaches the live run'
like "$OUT" 'asked' 'and says the run was asked'

: >"$WF_TMP/go-t2"
for _ in $(seq 1 150); do
	[ "$(cat "$rundir/t2.state" 2>/dev/null)" = merged ] && break
	sleep 0.2
done
is "$(cat "$rundir/t2.state" 2>/dev/null)" merged \
	'the live run dispatched it again and it merged'
is "$(cat "$rundir/t2.dispatches")" 2 'as a second attempt, not a fresh task'

: >"$WF_TMP/release-t1"
wait "$runpid"
is "$?" 0 'the run ends with everything merged'
like "$(cat "$T_TMP/run.log")" 'dispatched again by request' \
	'and its report says why t2 went twice'
