#!/usr/bin/env bash
# An integration branch holding merged work, a trunk that moved on since, and
# every task settled: `workflow accept` says to run the plan again to land it,
# and the run used to refuse with "git merge it by hand" (friction #V08D6BND).
# The run now merges the trunk into integration when the two merge clean,
# proves the result, and lands it.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register
export WF_TMP="$T_TMP"
"$MEM_BIN" project set verify "sh -c '[ -f \"$T_TMP/red\" ] && [ -f app/t2.php ] && exit 1; exit 0'" >/dev/null

write_exec "$T_TMP/worker.sh" <<'FAKE'
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
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'
export WORKFLOW_DEADLINE_MIN=0.5

"$MEM_BIN" plan --stdin >/dev/null <<'PLAN'
# plan: moved

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Verify: true
- [ ] t2 Add the t2 service
      Files: app/t2.php
      Verify: true
PLAN

: >"$T_TMP/red"
run workflow run
is "$RC" 1 'the first run stops short on t2'
rm -f "$T_TMP/red"

printf 'moved\n' >moved.txt
git add moved.txt
git -c core.hooksPath=/dev/null commit -qm 'Move the trunk on'

run workflow accept t2
is "$RC" 0 'accept lands t2 on integration'
like "$OUT" 'run the plan again to land it on the trunk' 'and says to run the plan again'

run workflow run
is "$RC" 0 'the run lands it'
like "$OUT" 'carries [0-9]+ commit\(s\) an earlier run merged and the trunk has moved on; merging the trunk into it' \
	'saying it merged the moved trunk in'
for f in app/t1.php app/t2.php moved.txt; do
	truthy "$([ -f "$f" ] && echo 0 || echo 1)" "the trunk has $f"
done
