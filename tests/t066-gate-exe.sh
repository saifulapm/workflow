#!/usr/bin/env bash
# The gate spawns this same binary for `verify --gate`. Read at that moment,
# current_exe answers `<path> (deleted)` once a `cargo install` has replaced
# the file under a live run, and the gate died with a bare ENOENT -- one per
# live run, at the first gate after the reinstall (frictions #PXSZ47M3,
# #TAXTZMSW). The run takes the path once when it is built, so the gate
# runs whatever stands at that path now.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# The worker stands in for the reinstall: the binary the run was started
# from is replaced by a copy of itself -- new inode, same path -- before it
# reports ready and the gate runs.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
if [ "$task" = t1 ]; then
	cp "$WORKFLOW_BIN" "$WORKFLOW_BIN.new" && mv "$WORKFLOW_BIN.new" "$WORKFLOW_BIN"
fi
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf 'app\n' >README.md
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: reinstalled

- [ ] t1 The service
      Files: app/t1.php
      Verify: true
- [ ] t2 The dependent [after: t1]
      Files: app/t2.php
      Verify: true
EOF

export WORKFLOW_MAX_WORKERS=1 WORKFLOW_DEADLINE_MIN=0.5
run workflow run
is "$RC" 0 'a binary replaced under the run does not stop it'

rundir="$XDG_STATE_HOME/workflow/runs/app/reinstalled"
is "$(cat "$rundir/t1.state")" merged 'the gate ran on the binary now at that path'
is "$(cat "$rundir/t2.state")" merged 'and the dependent went on to merge'
unlike "$OUT" 'could not run' 'nothing about a spawn that failed'
