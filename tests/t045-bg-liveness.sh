#!/usr/bin/env bash
# The bg backend's liveness half: a session the agents list calls busy is
# alive; one that stalls past the deadline is ended with `claude stop`, not a
# signal; the redispatch mints a new session; and a second stall parks.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
mkdir -p "$T_TMP/agents"
# Dispatches record their session as busy and never do any work; the agents
# listing serves whatever the state files say; stop is recorded and flips the
# session idle, exactly as `claude stop` leaves a resident session behind.
write_exec "$T_TMP/bin/claude" <<'CLAUDE'
#!/bin/sh
case "$1" in
agents)
	out='['; sep=''
	for f in "$WF_TMP/agents"/*; do
		[ -f "$f" ] || continue
		sid=$(basename "$f")
		out="$out$sep{\"pid\":1,\"id\":\"id-$sid\",\"kind\":\"background\",\"sessionId\":\"$sid\",\"status\":\"$(cat "$f")\"}"
		sep=','
	done
	printf '%s]\n' "$out"
	exit 0 ;;
stop)
	printf 'stop %s\n' "$2" >>"$WF_TMP/claude-stops"
	for f in "$WF_TMP/agents"/*; do
		[ -f "$f" ] || continue
		[ "id-$(basename "$f")" = "$2" ] && printf 'idle' >"$f"
	done
	exit 0 ;;
esac
prev=''
for a in "$@"; do
	[ "$prev" = "--session-id" ] && sid=$a
	prev=$a
done
[ -n "$sid" ] && printf 'busy' >"$WF_TMP/agents/$sid"
printf '%s\n' "$*" >>"$WF_TMP/claude-args"
printf 'backgrounded · fake\n'
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
is "$RC" 1 'a worker that never reports parks the run'

rundir="$XDG_STATE_HOME/workflow/runs/app/bg-liveness"
is "$(cat "$rundir/t1.state")" parked 'the stalled task is parked'
is "$(cat "$rundir/t1.dispatches")" 2 'after exactly one redispatch'

dispatches=$(grep -c -- '--bg' "$T_TMP/claude-args")
is "$dispatches" 4 'both tasks dispatched --bg, once plus one redispatch each'
sessions=$(grep -oE -- '--session-id [0-9a-f-]{36}' "$T_TMP/claude-args" | sort -u | wc -l)
is "$sessions" 4 'and every dispatch minted its own session id'

stops=$(sort -u "$T_TMP/claude-stops" | grep -c '^stop id-')
is "$(($stops >= 1))" 1 'a stalled session is ended with claude stop, by its short id'
first_sid=$(grep -oE -- '--session-id [0-9a-f-]{36}' "$T_TMP/claude-args" | head -1 | awk '{print $2}')
like "$(cat "$T_TMP/claude-stops")" "stop id-$first_sid" \
	'and the stop named the session the agents list maps to that id'
