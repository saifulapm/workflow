#!/usr/bin/env bash
# Every worktree's node_modules is a symlink to the checkout's, so the one task
# that changes the lockfile could not install: pnpm refuses a node_modules that
# resolves outside the worktree, and a worker that forced it would be rewriting
# what its siblings read (friction #A0WC5ABM). The task whose Files claim the
# manifest or the lockfile gets a directory of its own at dispatch, and so does
# a task dispatched after the lockfile changed on integration -- the shared
# directory does not carry what a sibling added until someone installs there.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# pnpm as the run sees it: where it ran and what it was asked, then a
# directory that is nobody's symlink.
write_exec "$T_TMP/bin/pnpm" <<'FAKE'
#!/bin/sh
printf '%s %s\n' "$(pwd)" "$*" >>"$WF_TMP/pnpm.log"
mkdir -p node_modules
: >node_modules/.own
FAKE

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ -L node_modules ]; then echo link; elif [ -d node_modules ]; then echo own; else echo none; fi >"$WF_TMP/$task.deps"
mkdir -p src
printf '%s\n' "$task" >"src/$task.js"
git add "src/$task.js"
if [ "$task" = t1 ]; then
	printf '  bullmq: 5.0.0\n' >>pnpm-lock.yaml
	git add pnpm-lock.yaml
fi
git -c core.hooksPath=/dev/null commit -qm "Add $task"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf '{"name":"app"}\n' >package.json
printf 'lockfileVersion: 9\n' >pnpm-lock.yaml
printf 'node_modules\n' >.gitignore
mkdir -p node_modules
: >node_modules/.shared
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF2'
# plan: deps

- [ ] t1 Add the queue library
      Files: package.json pnpm-lock.yaml src/t1.js
      Verify: true
- [ ] t3 Something beside it
      Files: src/t3.js
      Verify: true
- [ ] t2 Use the queue [after: t1]
      Files: src/t2.js
      Verify: true
EOF2

export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run
is "$RC" 0 'the run merges all three'

rundir="$XDG_STATE_HOME/workflow/runs/app/deps"
for t in t1 t2 t3; do
	is "$(cat "$rundir/$t.state")" merged "$t merged"
done
is "$(cat "$WF_TMP/t3.deps")" link 'a task that leaves the lockfile alone shares the checkout node_modules'
is "$(cat "$WF_TMP/t1.deps")" own 'the task whose Files claim the lockfile has a node_modules of its own'
is "$(cat "$WF_TMP/t2.deps")" own 'and so does the task dispatched after the lockfile changed on integration'
wt="$XDG_STATE_HOME/workflow/worktrees/app/deps"
like "$(cat "$WF_TMP/pnpm.log")" "^$wt/t1 install --frozen-lockfile\$" 'pnpm installed into the t1 worktree'
like "$(cat "$WF_TMP/pnpm.log")" "^$wt/t2 install --frozen-lockfile\$" 'and into the t2 worktree'
is "$(grep -c "^$wt/t3 " "$WF_TMP/pnpm.log")" 0 'and never into t3'
[ -e "$T_TMP/app/node_modules/.own" ] && notok 'the checkout node_modules is untouched' 'pnpm ran in the checkout' || ok 'the checkout node_modules is untouched'
