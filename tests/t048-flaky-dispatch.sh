#!/usr/bin/env bash
# A worker that dies leaving nothing -- no status line, no commit, no result --
# has said nothing about the task, only about the dispatch: a transient API
# error on the first turn looks exactly like this. It gets one retry in its
# wave before anything is failed (friction #195SW7VX), the same allowance a
# silent stall already had.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# Dies with nothing written on its first dispatch, does the job on its second.
write_exec "$T_TMP/flaky-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
if [ ! -f "$WF_TMP/tried-$task" ]; then
	: >"$WF_TMP/tried-$task"
	exit 1
fi
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/flaky-worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
base=$(git rev-parse HEAD)

cat >"$T_TMP/plan.md" <<'EOF'
# plan: flaky

- [ ] t1 First service
      Files: app/t1.php
      Verify: true
- [ ] t2 Second service
      Files: app/t2.php
      Verify: true
EOF

run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.2 \
	workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'both workers died once with nothing written, and the run still completes'
rundir="$XDG_STATE_HOME/workflow/runs/app/flaky"
is "$(cat "$rundir/t1.state")" merged 't1 merged on its second dispatch'
is "$(cat "$rundir/t2.state")" merged 'and so did t2'
is "$(cat "$rundir/t1.dispatches")" 2 'after exactly one retry'
like "$OUT" 'died leaving nothing' 'and the run says why it tried again'
is "$(git rev-list --count "$base..integration/flaky")" 2 'both commits are on integration'

# The retried attempt's brief says what became of the first one.
brief=$(cat "$XDG_CACHE_HOME/workflow/briefs/app/flaky/t1.md")
like "$brief" 'This is attempt 2' 'the retry brief says which attempt this is'
like "$brief" 'died before writing anything' 'and what happened to the one before it'

## ---------------------------------------- the retry is one, never a loop

# A dispatch that never produces anything fails on its second silence, with
# the reason the first attempt would have given.
rm -f "$T_TMP/tried-t1" "$T_TMP/tried-t2"
new_repo hopeless
mem_register
printf '{"name":"acme/h"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
cat >"$T_TMP/hopeless.md" <<'EOF'
# plan: hopeless

- [ ] t1 Never happens
      Files: app/t1.php
      Verify: true
- [ ] t2 Nor this
      Files: app/t2.php
      Verify: true
EOF
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.2 \
	WORKFLOW_WORKER_CMD='true' workflow run --plan-file "$T_TMP/hopeless.md"
is "$RC" 1 'a dispatch that never produces anything still ends failed'
hdir="$XDG_STATE_HOME/workflow/runs/hopeless/hopeless"
is "$(cat "$hdir/t1.state")" failed 'failed, not retried forever'
is "$(cat "$hdir/t1.dispatches")" 2 'after the one retry it is allowed'
is "$(cat "$hdir/t1.failed")" 'dispatch race: worker never wrote its pidfile' \
	'and the reason still names the dispatch, not a worker that never ran'
