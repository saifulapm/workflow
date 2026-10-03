#!/usr/bin/env bash
# A pending task says what holds it. With one worker slot and two tasks that
# could run at once, the second waits for the slot and its .held says so;
# `workflow status` shows it beside the state (friction #XR01M9H0).
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

export WF_TMP="$T_TMP"
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = t1 ]; then
	# Hold the slot until the test has read what t2 is waiting on.
	i=0
	while [ ! -f "$WF_TMP/go" ] && [ $i -lt 200 ]; do sleep 0.1; i=$((i + 1)); done
fi
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'
export WORKFLOW_MAX_WORKERS=1 WORKFLOW_DEADLINE_MIN=0.5

"$MEM_BIN" plan --stdin >/dev/null <<'PLAN'
# plan: held

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Verify: true
- [ ] t2 Add the t2 service
      Files: app/t2.php
      Verify: true
- [ ] t3 Add the t3 service [after: t2]
      Files: app/t3.php
      Verify: true
PLAN

rundir="$XDG_STATE_HOME/workflow/runs/app/held"
workflow run >"$T_TMP/run.out" 2>&1 &
pid=$!
i=0
while [ ! -s "$rundir/t2.held" ] && [ $i -lt 150 ]; do sleep 0.1; i=$((i + 1)); done
is "$(cat "$rundir/t2.held" 2>/dev/null)" 'waiting for a worker slot (1 of 1 running)' 'a ready task held by the cap says so'
is "$(cat "$rundir/t3.held" 2>/dev/null)" 'waiting on t2 to merge' 'and one behind a dependency names it'
run workflow status
like "$OUT" 't2 +pending +waiting for a worker slot' 'status shows what holds it'
touch "$T_TMP/go"
wait "$pid"
is "$?" 0 'the run merges all three'
truthy "$([ ! -e "$rundir/t2.held" ] && echo 0 || echo 1)" 'and the hold is gone once the task went'
