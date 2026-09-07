#!/usr/bin/env bash
# Two ways a task's branch outlives the run that made it, and what tells
# them apart: resumable() reads FAILED plus commits, nothing else (ruling 4).
#
# A branch with commits and no FAILED state is a run that died before it
# could say anything about the task -- there is no verdict on it, so the
# next run must not build it again silently, and must not resume it either.
# It gets the leftover recipe: merge it or delete it by hand (friction
# #GPC1PVZJ).
#
# A branch with commits and a FAILED state -- a gate rejection, a crash,
# anything the worker or the gate has already passed judgment on -- resumes
# on its own branch instead. Refusing that one just to make a person clear
# it by hand was the cost this milestone was cut to stop paying.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

## =================================================== the orphan leftover ===

new_repo land
mem_register
"$MEM_BIN" project set verify true >/dev/null

# Only t2 is ever dispatched here -- t1's branch is manufactured below --
# but a two-task plan needs a worker for whichever task the run does start.
write_exec "$T_TMP/land-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/land-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: land

- [ ] t1 The one a dead run leaves behind
      Files: app/t1.php
      Verify: true
- [ ] t2 The dependent [after: t1]
      Files: app/t2.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/land/land"
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5

# No `workflow run` ever touches t1 here: this is what a coordinator dying
# between the worktree's creation and its first status write leaves behind --
# a branch with a commit and a run dir that has never heard of the task, which
# is not the same thing as a task that reached FAILED.
git branch land/t1
git worktree add -q "$T_TMP/orphan" land/t1
(
	cd "$T_TMP/orphan" || exit 1
	mkdir -p app
	printf 't1\n' >app/t1.php
	git add app/t1.php
	git -c core.hooksPath=/dev/null commit -qm 'Add the t1 service'
)
git worktree remove --force "$T_TMP/orphan"

run workflow run
is "$RC" 2 'the run refuses over the leftover branch'
like "$OUT" 'land/t1' 'and names it'
is "$(cat "$rundir/t1.state" 2>/dev/null)" '' 'no state was ever recorded for it -- not FAILED, not anything'

## ------------------------------------------------- the recipe, followed

git merge -q --no-ff -m 'Merge the orphaned branch by hand' land/t1
git branch -qD land/t1
is "$(git branch --list 'land/t1' | grep -c .)" 0 'the branch is gone once it is merged'

## --------------------------------------------------------- the run after

run workflow run
is "$RC" 0 'the run after the recipe goes green'
is "$(cat "$rundir/t1.state")" merged 't1 counts as landed rather than being built again'
like "$OUT" 'already on integration/land' 'and the run says where its work is'
is "$(cat "$rundir/t1.dispatches" 2>/dev/null)" '' 'no worker was ever sent for it'
is "$(cat "$rundir/t2.state")" merged 'the dependent behind it runs and merges'

run git cat-file -e 'integration/land:app/t2.php'
is "$RC" 0 "the dependent's work is on the integration branch"
like "$("$MEM_BIN" plan)" '\[x\] t1' 'and the landed task is ticked off in mem'

## ============================================ a gate rejection resumes ===

new_repo resume
mem_register
"$MEM_BIN" project set verify true >/dev/null

# Reaches outside its Files: on the first attempt; fixes it, in place on the
# same branch, once the flag that makes it misbehave is gone.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
if [ -f "$WF_TMP/misbehave-$task" ]; then
	printf 'stray\n' >app/stray.php
	git add -A
	commit "Add the $task service"
elif [ -f app/stray.php ]; then
	git rm -q app/stray.php
	git add "app/$task.php"
	git -c core.hooksPath=/dev/null commit --amend -qm "Add the $task service"
else
	git add "app/$task.php"
	commit "Add the $task service"
fi
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: resume

- [ ] t1 The one the gate refuses
      Files: app/t1.php
      Verify: true
- [ ] t2 The dependent [after: t1]
      Files: app/t2.php
      Verify: true
EOF

rundir2="$XDG_STATE_HOME/workflow/runs/resume/resume"
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5

: >"$T_TMP/misbehave-t1"
run workflow run
is "$RC" 1 'the gate rejects the ownership violation'
is "$(cat "$rundir2/t1.state")" failed 't1 is failed, not merely blocked'
is "$(git branch --list 'resume/t1' | grep -c .)" 1 'its branch is kept, holding the commit'
is "$(cat "$rundir2/t2.state")" blocked 't2 never starts behind a failed dependency'

rm -f "$T_TMP/misbehave-t1"
run workflow run
is "$RC" 0 'the second run resumes it instead of refusing'
unlike "$OUT" 'still here from an earlier run' 'it is never called a leftover'
like "$OUT" 't1: failed last run -- resumed on its branch' 'and the run says so'
is "$(cat "$rundir2/t1.state")" merged 't1 merges once its worker fixes the violation'
is "$(cat "$rundir2/t1.dispatches")" 2 'on the second dispatch of the same branch'
is "$(cat "$rundir2/t2.state")" merged 'and the dependent behind it runs too'
