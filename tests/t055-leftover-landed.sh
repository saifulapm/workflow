#!/usr/bin/env bash
# The leftover-branch recipe, followed to the letter: merge the branch onto the
# trunk, delete it, run again. The task that wrote it never reached the merge
# gate, so no task ref records the work, and the next run used to dispatch a
# worker to build what was already on the trunk (friction #GPC1PVZJ). The tip
# the refusing preflight remembers is what tells the run otherwise.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# t1 reaches outside its Files, so the gate refuses it and its branch survives
# the run. t2 waits on it and does ordinary work.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
if [ "$task" = t1 ]; then
	printf 'stray\n' >app/stray.php
fi
git add -A
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: land

- [ ] t1 The one the gate refuses
      Files: app/t1.php
      Verify: true
- [ ] t2 The dependent [after: t1]
      Files: app/t2.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/land"
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5

## ------------------------------------------------- the run that leaves it

run workflow run
is "$RC" 1 'the run reports the refused task'
is "$(cat "$rundir/t1.state")" failed 't1 is refused at the gate'
is "$(git branch --list 'land/t1' | grep -c .)" 1 'and its branch is kept'

## ------------------------------------------- the run that prints the recipe

run workflow run
is "$RC" 2 'the next run refuses over the leftover branch'
like "$OUT" 'land/t1' 'and names it'

## ------------------------------------------------- the recipe, followed

git merge -q --no-ff -m 'Merge the refused branch by hand' land/t1
git branch -qD land/t1
is "$(git branch --list 'land/t1' | grep -c .)" 0 'the branch is gone once it is merged'

## --------------------------------------------------------- the run after

run workflow run
is "$RC" 0 'the run after the recipe goes green'
is "$(cat "$rundir/t1.state")" merged 't1 counts as landed rather than being built again'
like "$OUT" 'already on integration/land' 'and the run says where its work is'
is "$(cat "$rundir/t1.dispatches")" 1 'no second worker was sent to rebuild it'
is "$(cat "$rundir/t2.state")" merged 'the dependent behind it runs and merges'

run git cat-file -e 'integration/land:app/t2.php'
is "$RC" 0 "the dependent's work is on the integration branch"
like "$("$MEM_BIN" plan)" '\[x\] t1' 'and the landed task is ticked off in mem'
