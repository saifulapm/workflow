#!/usr/bin/env bash
# A worker that reports ready with nothing committed is asserting the work is
# already in the tree it opened onto -- rebuilt by hand, or landed by an
# earlier plan. Refusing it as "committed nothing" failed the task and
# skipped every dependent (friction #B2D8SJKR); the run counts it as
# done-previously instead and moves on.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = t2 ]; then
	mkdir -p app
	printf 't2\n' >app/t2.php
	git add app/t2.php
	git -c core.hooksPath=/dev/null commit -qm 'Add the t2 service'
fi
# t1 looks around, finds its Done already satisfied, and commits nothing.
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
mkdir -p app
printf 'already here\n' >app/t1.php
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: landed

- [ ] t1 The service that already exists
      Files: app/t1.php
      Verify: true
- [ ] t2 The dependent [after: t1]
      Files: app/t2.php
      Verify: true
EOF

export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5
run workflow run
is "$RC" 0 'a satisfied task does not stop the run'

rundir="$XDG_STATE_HOME/workflow/runs/app/landed"
is "$(cat "$rundir/t1.state")" done-previously \
	'ready with nothing committed is counted as already done'
like "$OUT" 'already in the tree' 'and the run says so'
is "$(cat "$rundir/t2.state")" merged 'the dependent is dispatched, not skipped'

run git cat-file -e "integration/landed:app/t2.php"
is "$RC" 0 "the dependent's work is on the integration branch"

like "$("$MEM_BIN" plan)" '\[x\] t1' 'the satisfied task is ticked off in mem'
is "$(git branch --list 'landed/t1' | grep -c .)" 0 \
	'its empty branch is not left for the next run to refuse on'
