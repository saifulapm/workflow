#!/usr/bin/env bash
# workflow park, pause and resume against a live serve. The question lead
# parks the task whose question it cannot answer; serve starts no second lead
# for it and drops the label once the question is answered. A pause stops the
# project's run once and starts nothing; resume lets serve start it again.
# A walk that ends while its project is paused is read: a pass ticks the
# milestone, a failure waits in the walk file, and nothing starts until resume.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The fake worker plays the lead when its brief's GOAL names a pickup, a
# question or a failure, and a worker otherwise. The question lead parks the
# task. cart lands a commit; ask asks until its brief carries the answer, then
# ends without a word. A walk waits for its go file, then walked passes and
# walkfail files a finding on step 2 and fails it.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	printf 'dogfood %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	while [ ! -e "$WF_TMP/go-$MEM_PROJECT" ]; do sleep 1; done
	if [ "$MEM_PROJECT" = walkfail ]; then
		mem finding add --milestone m1 --step 2 'the heading reads Helo' >/dev/null
		workflow report ready 'failed 2'
	else
		workflow report ready pass
	fi
	done_json
	exit 0
	;;
*'Turn the open findings into fix tasks'*)
	printf 'lead findings %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	done_json
	exit 0
	;;
*'Pick up the plan'*)
	done_json
	exit 0
	;;
*'Answer question #'*)
	printf 'question\n' >>"$WF_TMP/leads.log"
	workflow park ask 'the owner decides where the ask service goes' >"$WF_TMP/park.out" 2>&1
	printf '%s\n' "$?" >"$WF_TMP/park.rc"
	done_json
	exit 0
	;;
*'second failure'*)
	printf 'failure\n' >>"$WF_TMP/leads.log"
	done_json
	exit 0
	;;
esac
printf 'attempt\n' >>"$WF_TMP/$task.attempts"
if [ "$task" = cart ]; then
	say started
	mkdir -p app
	printf 'cart\n' >app/cart.php
	git add app/cart.php
	git -c core.hooksPath=/dev/null commit -qm 'Add the cart'
	say ready
	done_json
	exit 0
fi
if grep -q 'next to the cart' "$brief"; then
	done_json
	exit 0
fi
say started
mem ask 'where does the ask service go?' >"$WF_TMP/ask-id"
say blocked "asked $(cat "$WF_TMP/ask-id")"
done_json
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf '# roadmap: app-road\n\n- [ ] m1 The first milestone\n' | "$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status approved >/dev/null
"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
# plan: m1

- [ ] cart Add the cart
      Files: app/cart.php
      Verify: true
- [ ] ask Add the ask service
      Files: app/ask.php
      Verify: true
EOF

# walker <name>: a checkout whose one milestone has a two-step Show line and
# a plan of the one task cart.
walker() {
	new_repo "$1"
	mem_register
	"$MEM_BIN" project set verify true >/dev/null
	printf '# roadmap: %s-road\n\n- [ ] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello\n' "$1" |
		"$MEM_BIN" roadmap --stdin >/dev/null
	"$MEM_BIN" roadmap --status approved >/dev/null
	"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
	# plan: m1

	- [ ] cart Add the cart
	      Files: app/cart.php
	      Verify: true
	EOF
}
walker walked
walker walkfail

S="$XDG_STATE_HOME/workflow/serve/app"
R="$XDG_STATE_HOME/workflow/runs/app/m1"
APP="$T_TMP/app"
leads() { grep -c "^$1" "$T_TMP/leads.log" 2>/dev/null; }
stage() { cat "$S/stage" 2>/dev/null; }
child_alive() { [ -s "$S/child.pid" ] && kill -0 "$(cat "$S/child.pid")" 2>/dev/null; }
json() { (cd "$APP" && workflow status --json) | jq -cS "$1"; }

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; (exit $st); t_done' EXIT

## ------------------------------------------------------------------ park

for _ in $(seq 90); do
	[ -e "$R/ask.parked" ] && break
	sleep 1
done
is "$(cat "$T_TMP/park.rc" 2>/dev/null)" 0 'park exits 0 with a live run'
is "$(cat "$R/ask.parked" 2>/dev/null)" 'the owner decides where the ask service goes' 'park writes the reason beside the task'
like "$("$MEM_BIN" --project app log --type run --limit 20 2>/dev/null)" 'park ask: the owner decides' 'and logs a run line'
is "$(json '.parked')" '[{"reason":"the owner decides where the ask service goes","task":"ask"}]' 'status --json lists the parked task with its reason'
is "$(json '.stage')" '"execution"' 'while the run is live'
is "$(json '.runner')" '"here"' 'status --json names the runner'

## ----------------------------------------------------------------- pause

cd "$APP" || exit 1
run workflow pause
is "$RC" 0 'pause exits 0'
like "$("$MEM_BIN" project current --json | jq -r '.paused')" '^here [0-9]{4}-[0-9]{2}-[0-9]{2}$' 'pause writes the paused key with machine and date'
for _ in $(seq 60); do
	[ "$(stage)" = paused ] && ! child_alive && break
	sleep 1
done
is "$(stage)" paused 'serve says paused'
child_alive && notok 'the run was stopped' "$(cat "$S/child.pid")" || ok 'the run was stopped'
like "$(cat "$R/events" 2>/dev/null)" 'ended stopped by signal' 'by a signal, leaving its tasks for adoption'
sleep 3
is "$(grep -c 'told its run to stop' "$T_TMP/serve.out")" 1 'serve sent the stop once'
[ -s "$S/child.pid" ] && notok 'nothing starts while paused' "$(cat "$S/child.pid")" || ok 'nothing starts while paused'
is "$(json '.paused')" true 'status --json says paused'
is "$(json '.stage')" '"paused"' 'and the stage is paused'

## ---------------------------------------------------------------- resume

cd "$T_TMP" || exit 1
run workflow resume app
is "$RC" 0 'resume names a project from outside its checkout'
is "$("$MEM_BIN" --project app project current --json | jq -r '.paused')" null 'resume clears the paused key'
for _ in $(seq 60); do
	[ "$(stage)" = execution ] && child_alive && break
	sleep 1
done
is "$(stage)" execution 'serve starts the run again'
is "$(json '.paused')" false 'status --json says not paused'
is "$(leads question)" 1 'serve started no second lead for the parked task'

## ---------------------------------------------------------------- answer

ids=$("$MEM_BIN" --project app questions --for orchestrator --json | jq -r '.questions[] | select(.task == "m1/ask" and .answer == null) | .short_id')
for id in $ids; do
	"$MEM_BIN" --project app answer "$id" 'put it next to the cart' >/dev/null
done
for _ in $(seq 120); do
	[ "$(stage)" = waiting ] && break
	sleep 1
done
[ -e "$R/ask.parked" ] && notok 'an answered question unparks the task' || ok 'an answered question unparks the task'
is "$(json '.stage')" '"waiting"' 'status --json says waiting once the run stops short'
is "$(json '.parked')" '[]' 'with nothing parked'

## ----------------------------------------------------------- no live run

cd "$APP" || exit 1
run workflow park ask 'too late'
is "$RC" 2 'park with no live run exits 2'
like "$OUT" 'no live run' 'and says why'
[ -e "$R/ask.parked" ] && notok 'and writes nothing' || ok 'and writes nothing'

## ------------------------------------------------- a walk read while paused

cd "$T_TMP" || exit 1
serve_dir() { printf '%s/workflow/serve/%s' "$XDG_STATE_HOME" "$1"; }
stage_of() { cat "$(serve_dir "$1")/stage" 2>/dev/null; }
sessions() { grep -c "^$1 $2$" "$T_TMP/sessions.log" 2>/dev/null; }
road() { "$MEM_BIN" --project "$1" roadmap; }
until_() { for _ in $(seq "$1"); do eval "$2" && return 0; sleep 1; done; return 1; }
# paused_walk <name>: pause the project while its walk session goes, then let
# the session report.
paused_walk() {
	local n=$1
	until_ 120 '[ -e "$(serve_dir "$n")/walk" ] && [ "$(sessions dogfood "$n")" = 1 ]' ||
		notok "$n walks" "$(tail -15 "$T_TMP/serve.out")"
	workflow pause "$n" >/dev/null 2>&1
	until_ 30 '[ "$(stage_of "$n")" = paused ]'
	is "$(stage_of "$n")" paused "$n is paused while its walk goes"
	touch "$T_TMP/go-$n"
}

W=$(serve_dir walked)
paused_walk walked
until_ 60 'road walked | grep -q "^- \[x\] m1 "'
like "$(road walked)" '^- \[x\] m1 ' 'a walk that passes while paused ticks the milestone'
[ -e "$W/walk" ] && notok 'the walk file goes with the pass' "$(cat "$W/walk")" || ok 'the walk file goes with the pass'
sleep 3
is "$(stage_of walked)" paused 'the stage stays paused'
is "$(sessions dogfood walked)" 1 'and no second walk starts'

F=$(serve_dir walkfail)
paused_walk walkfail
until_ 60 '[ "$(sed -n 2p "$F/walk" 2>/dev/null)" = "failed 2" ]'
is "$(sed -n 2p "$F/walk" 2>/dev/null)" 'failed 2' 'a walk that fails while paused writes its outcome'
is "$(sed -n 3p "$F/walk" 2>/dev/null)" 'strikes 1' 'and its strike'
sleep 3
is "$(stage_of walkfail)" paused 'the stage stays paused'
is "$(sessions 'lead findings' walkfail)" 0 'no findings lead starts while paused'
like "$(road walkfail)" '^- \[ \] m1 ' 'the milestone stays open'
workflow resume walkfail >/dev/null 2>&1
until_ 60 '[ "$(sessions "lead findings" walkfail)" = 1 ]'
is "$(sessions 'lead findings' walkfail)" 1 'the findings lead goes out after resume'
