#!/usr/bin/env bash
# A failed task with commits on its branch keeps its worktree, its branch and
# its cargo dir; the next run resumes it there instead of refusing or
# rebuilding it fresh (ruling 4, resumable() = FAILED && commits > 0).
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# Reaches outside its Files: on the first attempt; fixes it, in place on the
# same branch, once the flag that makes it misbehave is gone -- the way a
# worker reading its own rejection in the next brief would.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1"; }
say started
printf '%s\n' "${CARGO_TARGET_DIR:-unset}" >"$WF_TMP/$task-target"
mkdir -p app
if [ -f "$WF_TMP/misbehave-$task" ]; then
	printf 'stray\n' >app/stray.php
	git add -A
	commit "Add the $task service"
else
	if [ -f app/stray.php ]; then
		git rm -q app/stray.php
		commit 'Undo the earlier write outside Files'
	fi
	mkdir -p app
	printf '%s\n' "$task" >"app/$task.php"
	git add "app/$task.php"
	commit "Add the $task service"
fi
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

## ==================================================== the ordinary resume ===

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: resume

- [ ] t1 The one that fails first
      Files: app/t1.php
      Verify: true
- [ ] t2 The dependent [after: t1]
      Files: app/t2.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/resume"
wtroot="$XDG_STATE_HOME/workflow/worktrees/app/resume"
cargo_root="$XDG_STATE_HOME/workflow/cargo/app/resume"
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5

: >"$T_TMP/misbehave-t1"
run workflow run
is "$RC" 1 'the run fails the committing worker'
is "$(cat "$rundir/t1.state")" failed 't1 failed at the gate'
is "$(cat "$rundir/t2.state")" blocked 't2 never starts behind a failed dependency'
is "$(git rev-list --count 'main..resume/t1')" 1 "its branch holds the commit it made"

[ -d "$wtroot/t1" ] && kept=yes || kept=no
is "$kept" yes 'cleanup keeps its worktree for resume'
[ -d "$cargo_root/t1" ] && cargokept=yes || cargokept=no
is "$cargokept" yes 'cleanup keeps its cargo dir too'
[ -d "$cargo_root/t2" ] && cargogone=no || cargogone=yes
is "$cargogone" yes 'a task that never ran keeps no cargo dir behind'

s1=$(git rev-parse resume/t1)
session1=$(cat "$rundir/t1.session")

## ------------------------------------------------------- the resumed run

rm -f "$T_TMP/misbehave-t1"
run workflow run
is "$RC" 0 'the resumed run goes green'
like "$OUT" 't1: failed last run -- resumed on its branch' 'preflight names the resume'
unlike "$OUT" 'still here from an earlier run' 'and never calls it a leftover'
is "$(cat "$rundir/t1.state")" merged 't1 merges on the second try'
is "$(cat "$rundir/t1.dispatches")" 2 'dispatched twice in all'
isnt "$(cat "$rundir/t1.session")" "$session1" 'the resumed dispatch minted a new session'
is "$(cat "$rundir/t2.state")" merged 'the dependent behind it runs and merges too'

run git merge-base --is-ancestor "$s1" integration/resume
is "$RC" 0 "the first attempt's own commit is still in the history -- the same branch, not a fresh one"
run git cat-file -e 'integration/resume:app/t1.php'
is "$RC" 0 "t1's work is on the integration branch"

[ -d "$wtroot" ] && left=yes || left=no
is "$left" no 'a finished run leaves no worktree behind'
[ -d "$cargo_root" ] && cargoleft=yes || cargoleft=no
is "$cargoleft" no 'and no cargo dir either'

## ============================== resumed after a hand-removed worktree ===

new_repo hand
mem_register
"$MEM_BIN" project set verify true >/dev/null

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: prune

- [ ] t1 The one that fails first
      Files: app/t1.php
      Verify: true
- [ ] t2 The dependent [after: t1]
      Files: app/t2.php
      Verify: true
EOF

prunerun="$XDG_STATE_HOME/workflow/runs/hand/prune"
prunewt="$XDG_STATE_HOME/workflow/worktrees/hand/prune"

: >"$T_TMP/misbehave-t1"
run workflow run
is "$RC" 1 'pass 1: the committing worker fails again'
is "$(cat "$prunerun/t1.state")" failed 'pass 1: t1 failed'

# A worktree removed by hand, not by git: the registration in .git/worktrees
# outlives the directory, and setup must not choke on either half of that.
rm -rf "$prunewt/t1"
like "$(git worktree list)" 'prune/t1' \
	'the worktree is still registered even with its directory gone'

rm -f "$T_TMP/misbehave-t1"
run workflow run
is "$RC" 0 'pass 2: setup prunes the stale registration and resumes on the same branch'
is "$(cat "$prunerun/t1.state")" merged 'pass 2: t1 merges'
is "$(cat "$prunerun/t1.dispatches")" 2 'pass 2: on its second dispatch'
is "$(cat "$prunerun/t2.state")" merged 'pass 2: the dependent runs and merges too'
