#!/usr/bin/env bash
# `workflow run` says at its start what plan-check would: the eight-pattern
# warning that predicted a task blowing two windows sat in plan-check output
# while the run dispatched it without a word (friction #V0HFVWJK). Warnings
# are said and the run goes on; a refusal stops it before a dispatch.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# t1 works until it is released, heartbeating so it never reads as stalled.
# t2 reports blocked and fails -- until the go flag appears, when it does the
# job properly.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = t1 ]; then
	while [ ! -f "$WF_TMP/release-t1" ]; do
		say progress
		sleep 0.5
	done
elif [ ! -f "$WF_TMP/go-$task" ]; then
	say 'blocked waiting on a decision'
	printf '{"is_error":false,"result":"blocked"}\n'
	exit 0
fi
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/worker.sh"
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

# plan: live

- [ ] t1 The long one
      Files: app/t1.php

: >"$WF_TMP/release-t1"
: >"$WF_TMP/go-t2"
cat >"$T_TMP/wide.md" <<'EOF2'
# plan: wide

- [ ] t1 Small
      Files: app/t1.php
      Verify: true
- [ ] t2 Owns too much
      Files: app/t2.php app/a.php app/b.php app/c.php app/d.php app/e.php app/f.php app/g.php app/h.php
      Verify: true
EOF2
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/wide.md"
is "$RC" 0 'a warning does not stop the run'
like "$OUT" 'task t2: Files carries 9 patterns -- a task owning more than eight is two tasks' \
	'and the oversize warning is said where the orchestrator is looking'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/wide/t2.state")" merged 'the task still ran'

cat >"$T_TMP/deferred.md" <<'EOF2'
# plan: deferred

- [ ] t1 Small
      Files: app/t1.php
      Verify: true
- [ ] t2 Puts it off
      Files: app/t2.php
      Verify: true
      Done: TBD
EOF2
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/deferred.md"
is "$RC" 2 'a refusal stops the run'
like "$OUT" 'plan-check refuses this plan' 'and says so'
truthy "$([ ! -d "$XDG_STATE_HOME/workflow/runs/app/deferred" ] && echo 0 || echo 1)" 'with nothing dispatched'
