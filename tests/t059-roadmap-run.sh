#!/usr/bin/env bash
# The roadmap at run time. A roadmap is milestones, not work a worker can be
# handed, so `workflow run` refuses one and says which verb turns a milestone
# into the plan of record.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# The fake worker: one commit per task, except sulk, which reports blocked.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = sulk ]; then
	say 'blocked I would rather not'
	printf '{"is_error":false,"result":"ok"}\n'
	exit 0
fi
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE

export FAKE="$T_TMP/fake-worker.sh"
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

"$MEM_BIN" roadmap --stdin >/dev/null <<'EOF'
# roadmap: app-2026

- [ ] filed-milestone The one whose plan is read off a file
- [ ] sulky-milestone The one whose worker will not
- [ ] done-milestone The one that finishes
EOF

# plan <slug> <task-two> -- a two-task plan in $T_TMP/<slug>.md.
plan() {
	cat >"$T_TMP/$1.md" <<-EOF
	# plan: $1

	- [ ] t1 Add the t1 service
	      Files: app/t1.php
	      Verify: true
	- [ ] $2 Add the $2 service
	      Files: app/$2.php
	      Verify: true
	EOF
}

## ------------------------------------------- a roadmap is not run, it is cut

cat >"$T_TMP/roadmap.md" <<'EOF'
# roadmap: app-2026

- [ ] filed-milestone The one whose plan is read off a file
- [ ] done-milestone The one that finishes
EOF
run workflow run --plan-file "$T_TMP/roadmap.md"
is "$RC" 2 'a roadmap handed to run is refused'
like "$OUT" "'app-2026' is a roadmap" 'named, so the reader knows which document'
like "$OUT" 'milestones rather than work a worker can take' 'and told why'
like "$OUT" 'mem plan --from <slug>' 'with the verb that turns one into the plan of record'
is "$(git worktree list | grep -c .)" 1 'and no worktree was made'
[ -d "$XDG_STATE_HOME/workflow/worktrees/app/app-2026" ] && notok 'nor a worker dispatched' 'the run set up' || ok 'nor a worker dispatched'

# The same document filed as the plan of record: mem stores what it is given,
# so this is the mistake to catch on the way in rather than at the first
# milestone dispatched as a task.
"$MEM_BIN" plan --set-file "$T_TMP/roadmap.md" >/dev/null
run workflow run
is "$RC" 2 'a roadmap filed as the plan of record is refused the same way'
like "$OUT" 'mem plan --from <slug>' 'with the same remedy'
