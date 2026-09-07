#!/usr/bin/env bash
# Which worker a run dispatches onto: WORKFLOW_BACKEND first, then the backend
# key the project set in mem, then claude. The amx side runs through a fake
# WORKFLOW_AMX that records every call's argv, so the new/status/stop shapes
# are checked against what the backend really sends.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
argv="$AMX_DIR/argv"
: >"$argv"

# saw <line> <desc> -- one call the fake amx recorded, its argv joined by '|'.
saw() {
	if grep -qxF -- "$1" "$argv"; then ok "$2"; else notok "$2" "$(cat "$argv")"; fi
}

## ----------------------------------------------------------------- the fakes

# A stand-in for amx. It records every call, does on `new` what the worker it
# starts would have done, and answers `status` out of the phase it last wrote
# for that name.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"

verb=$1
shift
case $verb in
new)
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model) shift 2 ;;
		--no-worktree) shift ;;
		*) text=$1; shift ;;
		esac
	done
	printf 'working\n' >"$AMX_DIR/$name.state"
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	# A reading, not a task: the brief names an answer file rather than a
	# status file, and neither reader here writes a verdict. wall leaves its
	# last words in the conversation amx names for it; ceiling never gets a
	# conversation, so the only account of why is what this launch prints.
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	if [ -n "$answer" ]; then
		printf 'nothing useful\n' >"$answer"
		case $name in
		wf-wall-review-*)
			sess=11111111-2222-4333-8444-555555555555
			printf '%s\n' "$sess" >"$AMX_DIR/$name.session"
			slug=$(printf '%s' "$dir" | sed -E 's/[^A-Za-z0-9]/-/g')
			mkdir -p "$HOME/.claude/projects/$slug"
			printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"You've reached your Fable limit for this session.\"}]}}" \
				>"$HOME/.claude/projects/$slug/$sess.jsonl"
			;;
		*) echo 'amx: rate limit exceeded, please retry.' >&2 ;;
		esac
		printf 'done\n' >"$AMX_DIR/$name.state"
		exit 0
	fi
	# The status file to report into is in the brief, which is where a real
	# worker reads it too. Its name is the task's.
	status=$(grep -oE '[^ ]+\.status' "$brief" | head -1)
	task=$(basename "$status" .status)
	printf '%s progress\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	# The stalling task stays working and reports nothing more.
	[ "$task" = hang ] && exit 0
	cd "$dir" || exit 1
	mkdir -p app
	printf '%s\n' "$task" >"app/$task.php"
	git add "app/$task.php"
	git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
	printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
	printf 'done\n' >"$AMX_DIR/$name.state"
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	printf '{"id":"%s","state":"%s","last_event":0,"session":"%s"}\n' \
		"$1" "$(cat "$AMX_DIR/$1.state")" "$(cat "$AMX_DIR/$1.session" 2>/dev/null)"
	;;
stop)
	printf 'stopped\n' >"$AMX_DIR/$1.state"
	;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

# The claude backend's fake, dispatched through its own template seam. It
# names itself in a log so a run that was meant for amx cannot use it unseen.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
printf '%s\n' "$task" >>"$WF_TMP/claude.log"
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {session}'"'"' > {out} 2> {err} &'

## ------------------------------------------------ a project that chose amx

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
"$MEM_BIN" project set backend amx >/dev/null

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: amx-run

- [ ] t1 Add the pricing service
      Files: app/t1.php
      Verify: true
- [ ] hang Never finish
      Files: app/hang.php
      Verify: true
EOF

export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.05
run workflow run
is "$RC" 1 'the run reports the stalled task with exit 1'

rundir="$XDG_STATE_HOME/workflow/runs/app/amx-run"
is "$(cat "$rundir/t1.state")" merged 'the task its amx worker finished is merged'
is "$(cat "$T_TMP/claude.log" 2>/dev/null)" '' \
	'and no claude worker was started for a project that chose amx'

sess=$(cat "$rundir/t1.session")
like "$sess" '^wf-t1-[0-9a-z]{4}$' 'the handle the run records is the amx agent name'
saw "new|--name|$sess|--dir|$XDG_STATE_HOME/workflow/worktrees/app/amx-run/t1|--no-worktree|--model|opus|Read $XDG_CACHE_HOME/workflow/briefs/app/amx-run/t1.md and execute it exactly." \
	'the dispatch is amx new into the task worktree, with the brief as the task'
saw "status|$sess|--json" 'liveness and the ending are read off amx status --json'

is "$(cat "$rundir/hang.state")" failed 'the worker that never reported ready is failed'
is "$(cat "$rundir/hang.dispatches")" 2 'after exactly one redispatch'
saw "stop|$(cat "$rundir/hang.session")" 'a stalled worker is ended with amx stop'

## ---------------------------------------- a project that selected nothing

before=$(grep -c . "$argv")

new_repo plain
mem_register
"$MEM_BIN" project set verify true >/dev/null

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: default-run

- [ ] p1 Add the first service
      Files: app/p1.php
      Verify: true
- [ ] p2 Add the second service
      Files: app/p2.php
      Verify: true
EOF

export WORKFLOW_DEADLINE_MIN=0.5
run workflow run
is "$RC" 0 'a project with no backend key runs as it always did'
plaindir="$XDG_STATE_HOME/workflow/runs/plain/default-run"
is "$(cat "$plaindir/p1.state")" merged 'its first task merged through the claude backend'
is "$(cat "$plaindir/p2.state")" merged 'and so did its second'
is "$(grep -c . "$argv")" "$before" 'and amx was never asked for anything'

## ------------------------------------------ the environment beats the key

cd "$T_TMP/app" || exit 1
cat >"$T_TMP/env-wins.md" <<'PLAN'
# plan: env-wins

- [ ] e1 Add the third service
      Files: app/e1.php
      Verify: true
- [ ] e2 Add the fourth service
      Files: app/e2.php
      Verify: true
PLAN
WORKFLOW_BACKEND=claude run workflow run --plan-file "$T_TMP/env-wins.md"
is "$RC" 0 'WORKFLOW_BACKEND overrides the backend the project chose'
envdir="$XDG_STATE_HOME/workflow/runs/app/env-wins"
is "$(cat "$envdir/e1.state")" merged 'the named backend is what the tasks ran on'
is "$(grep -c . "$argv")" "$before" 'and the amx the project asked for was left alone'
like "$(cat "$T_TMP/claude.log")" 'e1' 'the claude backend took the work instead'

## ------------------------------------- a reader that hit a provider limit

# The reader at the merge gate is a worker like any other, so under amx it is
# an agent too. `amx status --json` names the conversation its pane is
# running, which is where a reading that ended on the provider's own line left
# its last words; a reading that never got a conversation has only what
# `amx new` printed on its way out. Either line fails the task on the first
# reading, since a second one meets the same wall.
unset WORKFLOW_REVIEW_MODEL
"$MEM_BIN" project set review-model fable >/dev/null

cat >"$T_TMP/limit.md" <<-'PLAN'
	# plan: limit

	- [ ] wall Add the wall service
	      Files: app/wall.php
	      Verify: true
	- [ ] ceiling Add the ceiling service
	      Files: app/ceiling.php
	      Verify: true
PLAN
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/limit.md"
is "$RC" 1 'a reader that hit a provider limit fails the run'
limitdir="$XDG_STATE_HOME/workflow/runs/app/limit"

is "$(cat "$limitdir/wall.state" 2>/dev/null)" failed 'the task whose reader hit the wall is failed'
like "$(cat "$limitdir/wall.failed")" "^the reader hit a provider limit: You've reached your Fable limit for this session\\. \\(session wf-wall-review-" \
	'the note names the line and the amx agent that read'
like "$(cat "$limitdir/wall.review-err")" "You've reached your Fable limit for this session\\." \
	'review-err carries the line off the conversation amx named'
is "$(cat "$limitdir/wall.review-tries")" 1 'and only one reading was tried'

is "$(cat "$limitdir/ceiling.state" 2>/dev/null)" failed 'a reading with no conversation of its own is failed too'
like "$(cat "$limitdir/ceiling.failed")" '^the reader hit a provider limit: amx: rate limit exceeded, please retry\.' \
	'on the line the launch printed'
like "$(cat "$limitdir/ceiling.review-err")" 'rate limit exceeded' 'which the dispatch put in review-err'
is "$(cat "$limitdir/ceiling.review-tries")" 1 'after one reading as well'
