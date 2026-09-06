#!/usr/bin/env bash
# A worker the usage limit pauses without ending: the agents listing carries
# it idle rather than gone, so it is seen but not alive, and it says nothing
# past `started` in its own status file. Neither dead nor working, it is held
# to the stall deadline like a live worker, not collected the instant `alive`
# goes false (friction #17SPEY7R).
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
mkdir -p "$T_TMP/agents"
# A dispatch mints its own session, the way --bg does, then goes idle at
# once -- paused, not working, but the listing still carries it -- and
# writes `started` to its own status file before it goes quiet. A real
# worker learns its status path from the brief; this stub reads it back off
# its cwd, which the template already puts it in.
write_exec "$T_TMP/bin/claude" <<'CLAUDE'
#!/bin/sh
case "$1" in
agents)
	out='['; sep=''
	for f in "$WF_TMP/agents"/*; do
		[ -f "$f" ] || continue
		sid=$(basename "$f")
		out="$out$sep{\"id\":\"${sid%%-*}\",\"cwd\":\"\",\"kind\":\"background\",\"sessionId\":\"$sid\",\"state\":\"$(cat "$f")\",\"startedAt\":1}"
		sep=','
	done
	printf '%s]\n' "$out"
	exit 0 ;;
stop)
	printf 'stop %s\n' "$2" >>"$WF_TMP/claude-stops"
	for f in "$WF_TMP/agents"/*; do
		[ -f "$f" ] || continue
		sid=$(basename "$f")
		[ "${sid%%-*}" = "$2" ] && printf 'stopped' >"$f"
	done
	exit 0 ;;
esac
n=$(cat "$WF_TMP/seq" 2>/dev/null || echo 0)
n=$((n + 1)); printf '%s' "$n" >"$WF_TMP/seq"
short=$(printf 'a1b2c3%02x' "$n")
sid="$short-0000-4000-8000-000000000000"
printf 'idle' >"$WF_TMP/agents/$sid"
task=$(basename "$PWD")
rundir=$(dirname "$PWD" | sed 's#/worktrees/#/runs/#')
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$rundir/$task.status"
printf 'backgrounded · %s\n' "$short"
CLAUDE

is "$(command -v claude)" "$T_TMP/bin/claude" 'the stub is the claude on PATH'
[ "$(command -v claude)" = "$T_TMP/bin/claude" ] || exit 1

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
repo=$PWD

## ------------------------------------------------ reap_pass holds the line

cat >"$T_TMP/plan.md" <<'EOF'
# plan: paused-worker

- [ ] t1 A worker that pauses without a final word
      Files: app/One.php
      Verify: true
- [ ] t2 A second task so the run is worth having
      Files: app/Two.php
      Verify: true
EOF
rundir="$XDG_STATE_HOME/workflow/runs/app/paused-worker"

env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.05 \
	workflow run --plan-file "$T_TMP/plan.md" >"$T_TMP/run.log" 2>&1 &
runpid=$!

for _ in $(seq 1 50); do
	[ "$(cat "$rundir/t1.state" 2>/dev/null)" = dispatched ] && break
	sleep 0.05
done
is "$(cat "$rundir/t1.state" 2>/dev/null)" dispatched 'the worker is dispatched'

# Two polls' worth of waiting, well short of the stall deadline: a paused
# worker used to be collected on the very next pass.
sleep 0.7
is "$(cat "$rundir/t1.state" 2>/dev/null)" dispatched \
	'paused -- seen, not alive, nothing past "started" -- is not collected before the deadline'

wait "$runpid"
is "$?" 1 'the run fails once the deadline is spent'
is "$(cat "$rundir/t1.state")" failed 'and the task is failed, past the deadline'
is "$(cat "$rundir/t1.dispatches")" 2 'after exactly one redispatch, same as a stalled worker'
like "$(cat "$rundir/t1.failed")" 'stalled with no sign of life' \
	'failed as a stall, never as a worker that reported and erred'
stops=$(sort -u "$T_TMP/claude-stops" | grep -c '^stop a1b2c3')
is "$(($stops >= 1))" 1 'a paused session is ended with claude stop, the same way a stalled one is'

## --------------------------------------- adopt_stale keeps it as adopted

cat >"$T_TMP/orphan-plan.md" <<'EOF'
# plan: paused-orphan

- [ ] t1 A worker paused by a run that is gone
      Files: app/One.php
      Verify: true
- [ ] t2 A second task so the run is worth having
      Files: app/Two.php
      Verify: true
EOF
orundir="$XDG_STATE_HOME/workflow/runs/app/paused-orphan"
owtroot="$XDG_STATE_HOME/workflow/worktrees/app/paused-orphan"
mkdir -p "$orundir"
printf '%s\n' "$base" >"$orundir/base_sha"
printf 'dispatched\n' >"$orundir/t1.state"
printf '1\n' >"$orundir/t1.dispatches"
printf '%s\n' "$(date -u +%s)" >"$orundir/t1.dispatched_at"
git -C "$repo" worktree add -q -b paused-orphan/t1 "$owtroot/t1" "$base"
git -C "$repo" worktree add -q -b paused-orphan/t2 "$owtroot/t2" "$base"
# A session the listing carries as idle, and only "started" said: paused,
# not dead, from a run that no longer exists to watch it.
osid='b2c3d400-0000-4000-8000-000000000000'
printf 'idle' >"$T_TMP/agents/$osid"
printf '%s\n' "$osid" >"$orundir/t1.session"
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$orundir/t1.status"

run env WORKFLOW_DEADLINE_MIN=0.05 workflow run --plan-file "$T_TMP/orphan-plan.md"
like "$OUT" 'task t1: still working, from a run that is gone -- adopted' \
	'a paused task from a dead run is adopted like a live one, not collected at once'
is "$(cat "$orundir/t1.state")" failed 'the deadline still ends it eventually'
like "$(cat "$orundir/t1.failed")" 'stalled with no sign of life' \
	'failed as a stall, not mis-read as a worker that ran and erred'

t_done
