#!/usr/bin/env bash
# Every worker session the run ends leaves one cost line in mem's run log,
# `cost <plan> worker <task>: minutes=<n> ... model=<m>`, so what a milestone
# spent can be summed without anyone reading a transcript by hand. Under the
# process seam no transcript exists, so the token fields are left out rather
# than written as zero. A task dispatched twice ran two sessions and leaves
# two lines; one session read over twice still leaves one.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; status=$3; session=$4; brief=$5
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
attempt=$(grep -c . "$WF_TMP/$task.attempts" 2>/dev/null || echo 0)
attempt=$((attempt + 1))
printf 'attempt %s\n' "$attempt" >>"$WF_TMP/$task.attempts"
# The first attempt at `twice` dies leaving nothing, which earns the one more
# try a run gives: a second session for the same task.
if [ "$task" = twice ] && [ "$attempt" = 1 ]; then
	done_json
	exit 0
fi
say started
mkdir -p app
printf 'final\n' >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
say ready
done_json
FAKE

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

cat >"$T_TMP/cost.md" <<'PLAN'
# plan: cost-run

- [ ] once Add the once service
      Files: app/once.php
      Verify: true
- [ ] twice Add the twice service
      Files: app/twice.php
      Verify: true
PLAN
export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief}'"'"' > {out} 2> {err} &'
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run --plan-file "$T_TMP/cost.md"
is "$RC" 0 'both tasks merge'
rundir="$XDG_STATE_HOME/workflow/runs/app/cost-run"
is "$(cat "$rundir/twice.dispatches")" 2 'twice went out in two sessions'

runlog=$("$MEM_BIN" log --type run --limit 100 --json)
costs() { grep -oE "cost cost-run worker $1:[^\"]*" <<<"$runlog"; }

## ------------------------------------------------------ one line per session

is "$(costs once | grep -c .)" 1 'a task done in one session leaves one cost line'
is "$(costs twice | grep -c .)" 2 'a task dispatched twice leaves two'
is "$(grep -oE 'cost cost-run worker [a-z]+:' <<<"$runlog" | grep -c .)" 3 'and no session is costed twice'

## ------------------------------------------------------------ what it says

for task in once twice; do
	like "$(costs $task)" 'minutes=[0-9]+' "$task: the line carries its whole minutes"
	like "$(costs $task)" 'model=[^ ]+' "$task: and the model it ran on"
	unlike "$(costs $task)" ' in=| out=' "$task: and no token counts, with no transcript to read them off"
done

t_done
