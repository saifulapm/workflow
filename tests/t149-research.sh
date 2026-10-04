#!/usr/bin/env bash
# Serve starts research asked for by hand: each tick it lists the
# orchestrator's pending questions across projects, and a `research on
# <machine>` request addressed to this machine for a project with a checkout
# here and nothing going starts one research session, whatever the roadmap
# says. Once the session has ended the question is answered `done` beside a
# run line. A round's brief asks for what changed since the last roadmap. A
# request for another machine stays pending, and a project with a live run
# waits for a later tick.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake session: every research session is logged by whose it is and
# keeps its brief, then ends.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
brief=$4
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'workflow skill research'*)
	printf 'research %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/$MEM_PROJECT.research.md"
	;;
*)
	printf 'other %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	;;
esac
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

# project <name>: a checkout mem knows, with no roadmap yet.
project() {
	new_repo "$1"
	mem_register
}
# ask <text>: the request as the project's own question for the engine.
ask() { "$MEM_BIN" ask --for orchestrator -- "$1" | tr -d '#'; }

project app
app_q=$(ask "research on here")

# A round comes after a roadmap, here one marked done.
project old
printf '# roadmap: old-road\n\n- [x] m1 The home page\n' | "$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status done >/dev/null
old_q=$(ask "research round on here")

project away
away_q=$(ask "research on mini")

project busy
busy_q=$(ask "research on here")
sleep 600 &
busy_pid=$!
mkdir -p "$XDG_STATE_HOME/workflow/serve/busy"
printf '%s\n' "$busy_pid" >"$XDG_STATE_HOME/workflow/serve/busy/child.pid"

# answer <project> <question>: its answer once there is one, else nothing.
answer() {
	"$MEM_BIN" --project "$1" questions --for orchestrator --json |
		jq -r --arg id "$2" '.questions[] | select(.short_id == $id) | .answer // empty'
}
# answered <project> <question>: wait for the answer.
answered() {
	for _ in $(seq 60); do
		[ -n "$(answer "$1" "$2")" ] && return 0
		sleep 1
	done
	return 1
}
runs() { "$MEM_BIN" --project "$1" log --type run 2>/dev/null; }
sessions() { cat "$T_TMP/sessions.log" 2>/dev/null | grep -c "^research $1\$"; }

cd "$T_TMP" || exit 1
workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" "$busy_pid" 2>/dev/null; wait "$serve_pid" "$busy_pid" 2>/dev/null; (exit $st); t_done' EXIT

## ------------------------------------------------------- a request answered

answered app "$app_q" || notok 'the request is answered' "$(tail -5 "$T_TMP/serve.out")"
is "$(answer app "$app_q")" done 'an ended session answers done'
like "$(runs app)" 'research app: done' 'beside its run line'
is "$(sessions app)" 1 'one research session ran'
brief=$(cat "$T_TMP/app.research.md" 2>/dev/null)
like "$brief" '`workflow skill research`' 'its brief names the skill'
is "$(grep -c '^## ' <<<"$brief")" 9 'under the nine headings'
unlike "$brief" 'since the last roadmap' 'and asks for no round'
is "$([ -e "$XDG_STATE_HOME/workflow/serve/app/research" ] && echo there)" '' 'the research file is gone'

## ------------------------------------------------------------ a round

answered old "$old_q" || notok 'the round is answered' "$(tail -5 "$T_TMP/serve.out")"
is "$(answer old "$old_q")" done 'a round answers done'
like "$(cat "$T_TMP/old.research.md" 2>/dev/null)" 'what changed since the last roadmap' 'its brief asks what changed'

## ------------------------------------------ another machine, a live run

is "$(answer away "$away_q")" '' 'a request for another machine stays pending'
is "$(sessions away)" 0 'and starts nothing'
is "$(answer busy "$busy_q")" '' 'a project with a live run waits'
is "$(sessions busy)" 0 'and starts nothing'

kill "$busy_pid" 2>/dev/null
wait "$busy_pid" 2>/dev/null
rm -f "$XDG_STATE_HOME/workflow/serve/busy/child.pid"
answered busy "$busy_q" || notok 'the request is taken once the run is gone' "$(tail -5 "$T_TMP/serve.out")"
is "$(answer busy "$busy_q")" done 'a later tick starts it once the run has gone'
sleep 2
is "$(answer away "$away_q")" '' 'the other machine'"'"'s request is still pending'
is "$(cat "$T_TMP/sessions.log" 2>/dev/null | grep -c '^other ')" 0 'no other session started'
