#!/usr/bin/env bash
# A task added to the plan of record while the run is live is dispatched by
# that run, in plan order, once its dependencies merge. The brief and the
# reader already read the plan live; the ready set read the parse from setup,
# so an added task sat unseen until the next run (friction #HJHM61GZ).
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


: >"$WF_TMP/go-t2"
: >"$WF_TMP/go-t3"
cat >"$T_TMP/grow.md" <<'EOF2'
# plan: grow

- [ ] t1 Runs until released
      Files: app/t1.php
      Verify: true
- [ ] t2 Along from the start
      Files: app/t2.php
      Verify: true
EOF2
rundir="$XDG_STATE_HOME/workflow/runs/app/grow"

env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/grow.md" >"$T_TMP/grow.log" 2>&1 &
runpid=$!

for _ in $(seq 1 150); do
	[ "$(cat "$rundir/t2.state" 2>/dev/null)" = merged ] && break
	sleep 0.2
done
is "$(cat "$rundir/t2.state" 2>/dev/null)" merged 't2 merged while t1 still runs'

## ------------------------- an amendment reaches the files a worker reads

# t1's Verify, amended while its worker holds the worktree. The pre-commit
# hook in that worktree reads <task>.verify, written at dispatch, and status,
# wait and reap read the run dir's copy of the plan, written at setup -- so
# four amendments used to reach neither and the worker had to rewrite its own
# Verify line for the hook to let its commit through (friction #TDCT9VD8).
sed -i '0,/^      Verify: true$/s//      Verify: test -f wanted/' "$T_TMP/grow.md"
for _ in $(seq 1 150); do
	[ "$(cat "$rundir/t1.verify" 2>/dev/null)" = 'test -f wanted' ] && break
	sleep 0.2
done
is "$(cat "$rundir/t1.verify")" 'test -f wanted' \
	"a live task's own verify command follows the plan of record"
is "$(cat "$rundir/plan.md")" "$(cat "$T_TMP/grow.md")" \
	"and the run dir's copy of the plan is the plan as it reads now"

cat >>"$T_TMP/grow.md" <<'EOF2'
- [ ] t3 Added while the run is live  [after: t2]
      Files: app/t3.php
      Verify: true
EOF2

for _ in $(seq 1 150); do
	[ "$(cat "$rundir/t3.state" 2>/dev/null)" = merged ] && break
	sleep 0.2
done
is "$(cat "$rundir/t3.state" 2>/dev/null)" merged 'the added task is dispatched and merged by the live run'
like "$(cat "$T_TMP/grow.log")" 'task t3: added to the plan while the run is live -- pending' 'and the log says it joined'

## ------------------------------ an amendment the grammar refuses is said

# The parser answers None for the whole document when one appended task is
# slightly off, so a Verify line left out voids the entire re-read and the
# run goes on with the copy it started with -- silently, and for the rest of
# the run (friction #3QY5J9BS). Said once per text: the orchestrator is
# editing, and every poll would be a wall.
: >"$WF_TMP/go-t4"
cat >>"$T_TMP/grow.md" <<'EOF2'
- [ ] t4 Appended without a Verify line
      Files: app/t4.php
EOF2

for _ in $(seq 1 150); do
	grep -q 'does not parse' "$T_TMP/grow.log" && break
	sleep 0.2
done
like "$(cat "$T_TMP/grow.log")" \
	'run grow: the plan of record does not parse \(task t4 has no Verify: line\); the run is still using the copy it started with' \
	'the run says the re-read was void, and what the parser stopped on'
sleep 1
is "$(grep -c 'does not parse' "$T_TMP/grow.log")" 1 'once for that text, not once a poll'
is "$(cat "$rundir/t4.state" 2>/dev/null)" '' 'and nothing is dispatched for a task the grammar refused'

# Repaired: the same plan parses, and the task joins the live run.
printf '      Verify: true\n' >>"$T_TMP/grow.md"
for _ in $(seq 1 150); do
	[ "$(cat "$rundir/t4.state" 2>/dev/null)" = merged ] && break
	sleep 0.2
done
is "$(cat "$rundir/t4.state" 2>/dev/null)" merged 'the repaired task is dispatched and merged by the live run'
is "$(grep -c 'does not parse' "$T_TMP/grow.log")" 1 'and the refusal is not said again'

: >"$WF_TMP/release-t1"
wait "$runpid"
is "$?" 0 'the run ends with all four merged'
like "$(cat "$T_TMP/grow.md")" '\[x\] t3' 'and ticks the added task off in the plan it came from'
