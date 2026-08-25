#!/usr/bin/env bash
# A killed coordinator takes its workers with it (friction #RF50DJXQ, the
# half that waited on a signal crate). SIGTERM to `workflow run` stops every
# dispatched worker before the process ends; the tasks stay dispatched, so
# the next run in this checkout adopts and collects what they left.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"

# Workers that heartbeat forever: the run must be mid-wave when the signal
# lands, and nothing may look stalled.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
status=$3
while :; do
	printf '%s progress\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	sleep 0.5
done
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

cat >"$T_TMP/plan.md" <<'EOF'
# plan: shutdown

- [ ] t1 Works until told otherwise
      Files: app/t1.php
      Verify: true
- [ ] t2 So does this one
      Files: app/t2.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/shutdown"

env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=5 \
	workflow run --plan-file "$T_TMP/plan.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!

for _ in $(seq 1 100); do
	[ -s "$rundir/t1.pid" ] && [ -s "$rundir/t2.pid" ] && break
	sleep 0.2
done
w1=$(cat "$rundir/t1.pid" 2>/dev/null)
w2=$(cat "$rundir/t2.pid" 2>/dev/null)
isnt "$w1" '' 'both workers are running before the signal'

kill -TERM "$runpid"
for _ in $(seq 1 100); do
	kill -0 "$runpid" 2>/dev/null || break
	sleep 0.2
done
run kill -0 "$runpid"
isnt "$RC" 0 'the coordinator is gone'

for _ in $(seq 1 50); do
	kill -0 "$w1" 2>/dev/null || kill -0 "$w2" 2>/dev/null || break
	sleep 0.2
done
is "$(kill -0 "$w1" 2>/dev/null && echo alive || echo gone)" gone \
	'the first worker was stopped on the way down'
is "$(kill -0 "$w2" 2>/dev/null && echo alive || echo gone)" gone \
	'and so was the second'

like "$(cat "$T_TMP/run.log")" 'stopping' 'the run says it is stopping its workers'
is "$(cat "$rundir/t1.state")" dispatched \
	'the task stays dispatched, for the next run to adopt and collect'

kill "$w1" "$w2" 2>/dev/null
wait "$runpid" 2>/dev/null
exit 0
