#!/usr/bin/env bash
# The bg backend's liveness half: a session the agents list calls busy is
# alive; one that stalls past the deadline is ended with `claude stop`, not a
# signal; the redispatch mints a new session; and a second stall fails.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
mkdir -p "$T_TMP/agents"
# Dispatches mint their own session id — --bg ignores --session-id — announce
# it the way --bg does, and never do any work. The agents listing serves
# whatever the state files say, in the shape a session with no process left
# behind it has: an id, a session id and a state, and no status. Stop is
# recorded and leaves the session behind stopped, as `claude stop` does.
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
printf 'working' >"$WF_TMP/agents/$sid"
printf '%s\n' "$*" >>"$WF_TMP/claude-args"
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

cat >"$T_TMP/plan.md" <<'EOF'
# plan: bg-liveness

- [ ] t1 A task whose worker goes quiet
      Files: app/One.php
      Verify: true
- [ ] t2 A second task so the run is worth having
      Files: app/Two.php
      Verify: true
EOF

run env WORKFLOW_MAX_WORKERS=1 WORKFLOW_DEADLINE_MIN=0.05 \
	workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 1 'a worker that never reports fails the run'

rundir="$XDG_STATE_HOME/workflow/runs/app/bg-liveness"
is "$(cat "$rundir/t1.state")" failed 'the stalled task is failed'
is "$(cat "$rundir/t1.dispatches")" 2 'after exactly one redispatch'

dispatches=$(grep -c -- '--bg' "$T_TMP/claude-args")
is "$dispatches" 4 'both tasks dispatched --bg, once plus one redispatch each'
like "$(cat "$WF_TMP/claude-args")" 'model opus' 'workers get the strong model by default'
sessions=$(ls "$T_TMP/agents" | wc -l)
is "$sessions" 4 'and every dispatch got a session of its own'

stops=$(sort -u "$T_TMP/claude-stops" | grep -c '^stop a1b2c3')
is "$(($stops >= 1))" 1 'a stalled session is ended with claude stop, by its short id'
like "$(cat "$T_TMP/claude-stops")" 'stop a1b2c301' \
	'and the stop named the first session, which is the one that stalled'
# The handle the run kept is the one --bg announced, not the uuid it minted
# going in: a run holding the minted id can neither see a worker finish nor
# stop one.
is "$(cat "$rundir/t1.session")" 'a1b2c302-0000-4000-8000-000000000000' \
	'the redispatch recorded the session the new background agent actually got'
