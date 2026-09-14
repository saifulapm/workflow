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

: >"$WF_TMP/release-t1"
wait "$runpid"
is "$?" 0 'the run ends with all three merged'
like "$(cat "$T_TMP/grow.md")" '\[x\] t3' 'and ticks the added task off in the plan it came from'
