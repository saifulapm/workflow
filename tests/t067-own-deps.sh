#!/usr/bin/env bash
# Every worktree gets a node_modules of its own, installed from the lockfile
# when the worktree is made. A symlink to the checkout's used to stand there,
# and under pnpm 11 that fails every `pnpm run` in the tree before the script
# starts: the runner's deps check tries an install and refuses a node_modules
# that resolves outside the worktree (friction #EVAD8X1G). A task dispatched
# after the lockfile changed on integration is installed again at dispatch,
# since its directory does not carry what a sibling added (#A0WC5ABM).
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# pnpm as the run sees it: where it ran and what it was asked, then a
# directory that is nobody's symlink, carrying what pnpm itself leaves there:
# a copy of the lockfile it installed from at node_modules/.pnpm/lock.yaml.
write_exec "$T_TMP/bin/pnpm" <<'FAKE'
#!/bin/sh
printf '%s %s\n' "$(pwd)" "$*" >>"$WF_TMP/pnpm.log"
mkdir -p node_modules/.pnpm
cp pnpm-lock.yaml node_modules/.pnpm/lock.yaml
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
# The gate's suite passes only in a tree whose node_modules pnpm installed from
# the lockfile the tree carries: the integration worktree is furnished the same
# way a task's is, or the trunk reads as red before anything is dispatched
# (friction #XJ9TZ2PW), and it is installed again once a merge changes its
# lockfile, or the suite that decides the next merge runs on the old one.
"$MEM_BIN" project set verify 'cmp -s pnpm-lock.yaml node_modules/.pnpm/lock.yaml' >/dev/null
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
for t in t1 t2 t3; do
	is "$(cat "$WF_TMP/$t.deps")" own "$t has a node_modules of its own, no symlink"
done
wt="$XDG_STATE_HOME/workflow/worktrees/app/deps"
is "$(grep -c "^$wt/t1 install --frozen-lockfile\$" "$WF_TMP/pnpm.log")" 1 'pnpm installed into the t1 worktree once, when it was made'
is "$(grep -c "^$wt/t3 install --frozen-lockfile\$" "$WF_TMP/pnpm.log")" 1 'and into t3 once'
is "$(grep -c "^$wt/t2 install --frozen-lockfile\$" "$WF_TMP/pnpm.log")" 2 'and into t2 twice: when it was made, and again at dispatch after the lockfile changed on integration'
is "$(grep -c "^$wt/_integration install --frozen-lockfile\$" "$WF_TMP/pnpm.log")" 2 'and into the integration worktree twice: when it was made, and before the gate after t1 landed its lockfile there'
[ -e "$T_TMP/app/node_modules/.own" ] && notok 'the checkout node_modules is untouched' 'pnpm ran in the checkout' || ok 'the checkout node_modules is untouched'
