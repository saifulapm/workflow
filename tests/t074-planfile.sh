#!/usr/bin/env bash
# ruling 4: a --plan-file inside the checkout is refused -- its ticks would
# land in the tree. The suite's own plan files sit in $T_TMP, beside the
# sandbox repos, never inside one.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

cat >plan-in-tree.md <<'PLAN'
# plan: in-tree

- [ ] t1 Add the t1 service
      Files: src/t1.php
      Verify: true
- [ ] t2 Add the t2 service
      Files: src/t2.php
      Verify: true
PLAN

run workflow run --plan-file plan-in-tree.md
is "$RC" 2 'a plan file under the sandbox repo is refused'
like "$OUT" 'plan-in-tree\.md is inside the checkout' 'and names the file and why'
like "$OUT" 'mem plan <slug> --set-file' 'and says how to store it instead'
like "$OUT" 'run from mem' 'and how to run it'

## ------------------ the same file, copied beside the checkout, is a real run

"$MEM_BIN" project set verify true >/dev/null

write_exec "$T_TMP/beside-worker.sh" <<'BFAKE'
#!/bin/sh
task=$1; status=$3
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p src
printf '<?php\n' >"src/$task.php"
git add "src/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
printf '%s ready merge-ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
BFAKE
export FAKE="$T_TMP/beside-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

cp plan-in-tree.md "$T_TMP/plan-beside.md"
rundir="$XDG_STATE_HOME/workflow/runs/app/in-tree"
run workflow run --plan-file "$T_TMP/plan-beside.md"
is "$RC" 0 'the same file, copied beside the checkout, drives a real run'
is "$(cat "$rundir/t1.state")" merged 't1 was dispatched, worked and merged'
is "$(cat "$rundir/t2.state")" merged 't2 was dispatched, worked and merged'
like "$(cat "$T_TMP/plan-beside.md")" '\[x\] t1' 'the ticks land back in the file the run was handed'
like "$(cat "$T_TMP/plan-beside.md")" '\[x\] t2' 'both tasks ticked there'
