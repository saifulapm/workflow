#!/usr/bin/env bash
# The merge gate's failure note names the checks that broke, not just that
# the suite went red. Both streams of the gate's verify run land in
# <task>.gate, and the two failing checks that project suite prints show up
# in the task's failure note, in the run log and in the brief a resumed
# worker reads.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

write_exec "$T_TMP/suite.sh" <<'FAKE'
#!/bin/sh
printf 'ok 1 - warm up\n'
printf 'not ok 12 - t3 merges\n'
printf 'ok 13 - fine\n'
printf 'not ok 14 - side ships\n' >&2
exit 1
FAKE
"$MEM_BIN" project set verify "$T_TMP/suite.sh" >/dev/null

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

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: gate

- [ ] t1 The one whose merge finds the project suite red
      Files: app/t1.php
      Verify: true
- [ ] side A second task, so the plan is worth a worker
      Files: app/side.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/gate"
brief="$XDG_CACHE_HOME/workflow/briefs/app/gate/t1.md"
export WORKFLOW_DEADLINE_MIN=0.5

run workflow run
is "$RC" 1 'the gate rejects the merge'
is "$(cat "$rundir/t1.state")" failed 't1 is failed'
like "$(cat "$rundir/t1.failed")" 'not ok 12 - t3 merges' 'the failure note names the first failing check'
like "$(cat "$rundir/t1.failed")" 'not ok 14 - side ships' 'and the second'
like "$(cat "$rundir/t1.failed")" '-- see .*t1\.gate$' 'and points at the gate output file'

like "$OUT" 'not ok 12 - t3 merges' 'the run log names the first failing check too'
like "$OUT" 'not ok 14 - side ships' 'and the second'

like "$(cat "$rundir/t1.gate")" 'not ok 12 - t3 merges' 'the gate file holds the stdout check'
like "$(cat "$rundir/t1.gate")" 'not ok 14 - side ships' 'and the stderr check, both streams captured'

is "$(git branch --list 'gate/t1' | grep -c .)" 1 'its branch is kept, holding the commit'

run workflow run
is "$RC" 1 'the second run resumes it and finds the suite still red'
is "$(cat "$rundir/t1.dispatches")" 2 'on a second dispatch of the same branch'
like "$(cat "$brief")" 'not ok 12 - t3 merges' 'the redispatched brief names the first failing check'
like "$(cat "$brief")" 'not ok 14 - side ships' 'and the second'
