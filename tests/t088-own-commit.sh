#!/usr/bin/env bash
# The Files line is enforced at the commit (m1-lessons ruling 6). The run
# writes the task's Files beside its Verify in the run dir, and the pre-commit
# hook in a run worktree refuses a commit that stages a path outside them,
# with the gate's wording and the mem ask line -- while the worker still has
# its session and the stop-and-ask rule in front of it. Two workers on
# ebdify m1 saw the lockfile outside their Files, noted it, shipped, and were
# failed three seconds after their sessions ended.
source "$(dirname -- "$0")/lib.sh"
t_init
chmod +x "$HOOKS"/pre-commit 2>/dev/null

## ------------------------------------------ the hook holds the commit to Files

mkdir -p "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a" "$XDG_STATE_HOME/workflow/runs/proj/plan-a"
new_repo wt
mkdir -p app
printf 'seed\n' >app/seed.php
git add app/seed.php
git -c core.hooksPath=/dev/null commit -qm 'seed the app'
mv "$T_TMP/wt" "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a/t1"
cd "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a/t1" || exit 1
rundir="$XDG_STATE_HOME/workflow/runs/proj/plan-a"
printf 'true\n' >"$rundir/t1.verify"
printf 'app/** tests/CartTest.php\n' >"$rundir/t1.files"

printf 'change\n' >app/cart.php
git add app/cart.php
run git_gated commit -m 'Add the cart'
is "$RC" 0 'a commit inside the Files line goes through'

printf 'lockfile\n' >pnpm-lock.yaml
printf 'more\n' >>app/cart.php
git add pnpm-lock.yaml app/cart.php
run git_gated commit -m 'Add a dependency'
isnt "$RC" 0 'a commit staging a path outside it is refused'
like "$OUT" 'commit refused: staged outside this task'"'"'s Files: patterns -- A\|pnpm-lock.yaml' 'naming the record, in the gate'"'"'s words'
unlike "$OUT" 'app/cart.php' 'and only the record outside'
like "$OUT" 'a file the toolchain rewrote is still yours: `mem ask` if Files must widen' 'with the way out'
unlike "$OUT" 'verify task' 'before the Verify line was run'
is "$(git log --oneline | wc -l)" 3 'nothing was committed'

git reset -q pnpm-lock.yaml
run git_gated commit -m 'Add more cart'
is "$RC" 0 'unstaged, the commit goes through'

# The last pattern on the line counts like every other. The hook reads the
# line out of a file written with a newline on the end, and a splitter that
# broke on spaces alone left that newline on the last pattern, so the one
# file it named was refused (ebdify m1's admin, 2026-09-21).
mkdir -p tests
printf 'test\n' >tests/CartTest.php
git add tests/CartTest.php
run git_gated commit -m 'Add the cart test'
is "$RC" 0 'a file named by the last pattern on the line commits'
unlike "$OUT" 'commit refused' 'and is never refused for being last'

# No Files on record: the hook holds the commit to nothing (an older run).
rm "$rundir/t1.files"
git add pnpm-lock.yaml
run git_gated commit -m 'Add the dependency after all'
is "$RC" 0 'with no Files line on record the hook says nothing about ownership'

## ---------------------------------------------- the run writes the line

cd "$T_TMP" || exit 1
export WF_TMP="$T_TMP"
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; status=$3; session=$4; brief=$5
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
say started
cp "$brief" "$WF_TMP/$task.brief"
mkdir -p app
printf 'final\n' >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/fake-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf '{"name":"app"}\n' >package.json
printf 'lockfileVersion: 9\n' >pnpm-lock.yaml
git add package.json pnpm-lock.yaml
git -c core.hooksPath=/dev/null commit -qm 'the manifest and its lockfile'
cat >"$T_TMP/files.md" <<'PLAN'
# plan: files

- [ ] deps Add a dependency
      Files: package.json app/deps.php
      Verify: true
- [ ] other Add the other service
      Files: app/other.php
      Verify: true
PLAN
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run --plan-file "$T_TMP/files.md"
is "$RC" 0 'both merge'
rundir="$XDG_STATE_HOME/workflow/runs/app/files"
is "$(cat "$rundir/deps.files")" 'package.json app/deps.php' 'the task'"'"'s Files line is on record beside its Verify'
is "$(cat "$rundir/other.files")" 'app/other.php' 'for every task'
like "$(cat "$WF_TMP/deps.brief")" 'Adding a dependency rewrites `pnpm-lock.yaml`; your Files do not claim it: `mem ask` before staging it, never a note\.' 'a brief claiming the manifest names the lockfile and that Files omit it'
unlike "$(cat "$WF_TMP/other.brief")" 'Adding a dependency' 'a brief claiming no manifest says nothing of it'

t_done
