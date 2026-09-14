#!/usr/bin/env bash
# A directory where the integration worktree should be, that git no longer
# knows: an unclean shutdown zero-truncates its .git file and drops the entry
# from .git/worktrees. That used to fail as "cannot fast-forward", pointing at
# a history that was fine (friction #S59909Y7). The run says what it is and
# how to mend it, and the mend works.
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
cat >"$T_TMP/plan.md" <<'EOF2'
# plan: corrupt

- [ ] t1 Lands once the worktree is sound
      Files: app/t1.php
      Verify: true
- [ ] t2 So does this
      Files: app/t2.php
      Verify: true
EOF2
wtroot="$XDG_STATE_HOME/workflow/worktrees/app/corrupt"
mkdir -p "$wtroot/_integration"
: >"$wtroot/_integration/.git"

run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 2 'the run stops before touching anything'
like "$OUT" 'integration worktree at .* is missing or corrupt' 'and says the worktree is the fault'
like "$OUT" 'git worktree prune' 'naming the mend'
like "$OUT" 'the branch integration/corrupt is intact' 'and that the branch is fine'
unlike "$OUT" 'cannot fast-forward' 'not the history'

rm -rf "$wtroot/_integration"
git worktree prune
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 0 'after the mend the same run goes through'
is "$(cat "$XDG_STATE_HOME/workflow/runs/app/corrupt/t2.state")" merged 'and merges'
