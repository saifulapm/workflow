#!/usr/bin/env bash
# `git worktree add` leaves a submodule's directory empty, so a run's worktrees
# came up without the engine a project keeps as one, and a gate that builds
# against it was red before anything was dispatched (friction #H8QF4ES3).
# Every run worktree now checks its submodules out from the main checkout's,
# with no network: here the submodule's upstream is gone before the run starts.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ -f engine/core.txt ]; then echo files; else echo empty; fi >"$WF_TMP/$task.engine"
mkdir -p src
printf '%s\n' "$task" >"src/$task.js"
git add "src/$task.js"
git -c core.hooksPath=/dev/null commit -qm "Add $task"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

new_repo engine-upstream
printf 'core\n' >core.txt
git add core.txt
git -c core.hooksPath=/dev/null commit -qm 'core'

new_repo app
mem_register
# Named apart from its path, so the url override has to use the name.
git -c protocol.file.allow=always submodule add -q --name eng "$T_TMP/engine-upstream" engine
git -c core.hooksPath=/dev/null commit -qm 'engine submodule'
# The gate records where it ran and what it found there, since the
# integration worktree is gone once the run ends.
"$MEM_BIN" project set verify 'if [ -f engine/core.txt ]; then r=files; else r=empty; fi; echo "$(basename "$PWD") $r" >>"$WF_TMP/gate.log"; [ $r = files ]' >/dev/null
rm -rf "$T_TMP/engine-upstream"

"$MEM_BIN" plan --stdin >/dev/null <<'EOF2'
# plan: subs

- [ ] t1 First
      Files: src/t1.js
      Verify: true
- [ ] t2 Second [after: t1]
      Files: src/t2.js
      Verify: true
EOF2

export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run
is "$RC" 0 'the run merges both tasks through a gate that needs the engine'

rundir="$XDG_STATE_HOME/workflow/runs/app/subs"
for t in t1 t2; do
	is "$(cat "$rundir/$t.state")" merged "$t merged"
	is "$(cat "$WF_TMP/$t.engine")" files "$t found the submodule's files in its worktree"
done
is "$(grep -c . "$WF_TMP/gate.log")" "$(grep -c '^_integration files$' "$WF_TMP/gate.log")" 'every gate ran in the integration worktree with the submodule checked out'
is "$(grep -c '^_integration files$' "$WF_TMP/gate.log")" 3 'the gate ran on the base and once for each merge, the last after t1 had landed'
