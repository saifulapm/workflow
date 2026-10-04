#!/usr/bin/env bash
# Serve takes a walk asked for by hand: each tick it lists the orchestrator's
# pending questions across projects, and a request addressed to this machine
# for a project with a checkout here and nothing going is walked, whoever the
# runner is. The question is answered with the walk's words beside its run
# line. A request for another machine stays pending, a project with a live
# run waits for a later tick, and a head without the commit walks nothing:
# one finding on step 0 names the push and the answer is `failed 0`. A
# requested walk ticks nothing and claims nothing.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake session: every walk is logged by whose it is and keeps its brief.
# bad's walk fails step 2 and files its finding; any other walk passes.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
brief=$4
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	printf 'dogfood %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/$MEM_PROJECT.walk.md"
	if [ "$MEM_PROJECT" = bad ]; then
		mem finding add --milestone m1 --step 2 'the heading reads Helo' >/dev/null
		workflow report ready 'failed 2'
	else
		workflow report ready pass
	fi
	;;
*)
	printf 'other %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	;;
esac
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

# project <name>: a checkout whose roadmap is done, its first milestone
# landed with a Show line of three steps and its second never ticked.
project() {
	new_repo "$1"
	mem_register
	printf '# roadmap: %s-road\n\n- [x] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello; then the footer shows the year\n- [ ] m2 The cart\n' "$1" |
		"$MEM_BIN" roadmap --stdin >/dev/null
	"$MEM_BIN" roadmap --status done >/dev/null
}
# ask <text>: the request as the project's own question for the engine.
ask() { "$MEM_BIN" ask --for orchestrator -- "$1" | tr -d '#'; }

project app
app_head=$(git rev-parse HEAD)
app_road=$("$MEM_BIN" roadmap)
app_q=$(workflow dogfood 2>/dev/null | tr -d '#')

project bad
bad_q=$(ask "dogfood m1 at $(git rev-parse HEAD) on here steps 2")

project away
away_q=$(ask "dogfood m1 at $(git rev-parse HEAD) on mini")

project busy
busy_q=$(ask "dogfood m1 at $(git rev-parse HEAD) on here")
sleep 600 &
busy_pid=$!
mkdir -p "$XDG_STATE_HOME/workflow/serve/busy"
printf '%s\n' "$busy_pid" >"$XDG_STATE_HOME/workflow/serve/busy/child.pid"

project gone
gone_sha=1111111111111111111111111111111111111111
gone_q=$(ask "dogfood m1 at $gone_sha on here")

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
walks() { cat "$T_TMP/sessions.log" 2>/dev/null | grep -c "^dogfood $1\$"; }

cd "$T_TMP" || exit 1
workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" "$busy_pid" 2>/dev/null; wait "$serve_pid" "$busy_pid" 2>/dev/null; (exit $st); t_done' EXIT

## ------------------------------------------------------------ a clean walk

answered app "$app_q" || notok 'the request is answered' "$(tail -5 "$T_TMP/serve.out")"
is "$(answer app "$app_q")" pass 'a clean walk answers pass'
like "$(runs app)" 'dogfood m1: pass' 'beside its run line'
is "$(walks app)" 1 'one session walked it'
like "$(cat "$T_TMP/app.walk.md" 2>/dev/null)" '3\. the footer shows the year' 'every step of the Show path'
is "$("$MEM_BIN" --project app roadmap)" "$app_road" 'the roadmap is left as it was'
is "$("$MEM_BIN" --project app project current --json | jq -r '.runner // empty')" '' 'and the project unclaimed'

## ------------------------------------------------------- a failed step

answered bad "$bad_q" || notok 'the failed walk is answered' "$(tail -5 "$T_TMP/serve.out")"
is "$(answer bad "$bad_q")" 'failed 2' 'a failed step answers failed 2'
like "$(runs bad)" 'dogfood m1: findings 1' 'beside its run line'
brief=$(cat "$T_TMP/bad.walk.md" 2>/dev/null)
like "$brief" '2\. the heading reads Hello' 'the step asked for is walked'
unlike "$brief" '1\. open the home page' 'and no other'
is "$("$MEM_BIN" --project bad finding list --open --json | jq -r '[.items[] | .step] | join(" ")')" 2 'its finding stays open'
like "$("$MEM_BIN" --project bad roadmap)" '^- \[ \] m2 ' 'and nothing is ticked'

## --------------------------------------------------- a missing commit

answered gone "$gone_q" || notok 'the missing commit is answered' "$(tail -5 "$T_TMP/serve.out")"
is "$(answer gone "$gone_q")" 'failed 0' 'a head without the commit answers failed 0'
found=$("$MEM_BIN" --project gone finding list --open --json)
is "$(jq '.items | length' <<<"$found")" 1 'with one finding'
is "$(jq -r '.items[0].step' <<<"$found")" 0 'on step 0'
# mem cuts a long title short, so the text is read off the item's body.
body=$("$MEM_BIN" --project gone show "$(jq -r '.items[0].short_id' <<<"$found")" --json | jq -r '.items[0].body' | sed '/^$/d')
is "$body" "commit $gone_sha is not on here: push it there, the engine never pushes" 'naming the push'
is "$(walks gone)" 0 'and nothing is walked'

## ------------------------------------------ another machine, a live run

is "$(answer away "$away_q")" '' 'a request for another machine stays pending'
is "$(walks away)" 0 'and is not walked'
is "$(answer busy "$busy_q")" '' 'a project with a live run waits'
is "$(walks busy)" 0 'and is not walked'

kill "$busy_pid" 2>/dev/null
wait "$busy_pid" 2>/dev/null
rm -f "$XDG_STATE_HOME/workflow/serve/busy/child.pid"
answered busy "$busy_q" || notok 'the request is taken once the run is gone' "$(tail -5 "$T_TMP/serve.out")"
is "$(answer busy "$busy_q")" pass 'a later tick walks it once the run has gone'
sleep 2
is "$(answer away "$away_q")" '' 'the other machine'"'"'s request is still pending'
is "$(cat "$T_TMP/sessions.log" 2>/dev/null | grep -c '^other ')" 0 'no other session started'
