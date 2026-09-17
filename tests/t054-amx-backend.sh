#!/usr/bin/env bash
# The amx backend, which every run dispatches onto: a fake WORKFLOW_AMX
# records every call's argv, so the new/status/stop shapes are checked
# against what the backend really sends; a stalled agent is stopped and
# dispatched once more under a fresh name; a launch amx refuses fails the
# task on the line it printed; and a reader that hit a provider limit fails
# on the first reading.
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
new | sub)
	if [ -f "$AMX_DIR/refuse" ]; then
		cat "$AMX_DIR/refuse" >&2
		exit 2
	fi
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		--parent | --role) shift 2 ;;
		*) text=$1; shift ;;
		esac
	done
	[ "$verb" = sub ] && printf '{"id":"%s","parent":null,"phase":"starting","answer":null,"evidence":"hooks"}\n' "$name"
	printf 'working\n' >"$AMX_DIR/$name.state"
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	# A reading, not a task: the brief names an answer file rather than a
	# status file, and neither reader here writes a verdict. wall leaves its
	# last words in the conversation amx names for it; ceiling never gets a
	# conversation, so the only account of why is what this launch prints.
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	if [ -n "$answer" ]; then
		case $name in
		wf-trust-review-*)
			# Stopped at claude's folder-trust screen: no answer file at
			# all, nothing on stderr, and a pane waiting on a question
			# that only the screen shows.
			printf 'Quick safety check: Is this a project you created or one you trust?' >"$AMX_DIR/$name.question"
			printf 'waiting\n' >"$AMX_DIR/$name.state"
			exit 0
			;;
		esac
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
	question=null
	[ -f "$AMX_DIR/$1.question" ] && question=$(printf '{"text":"%s","options":[]}' "$(cat "$AMX_DIR/$1.question")")
	printf '{"id":"%s","state":"%s","last_event":0,"session":"%s","question":%s}\n' \
		"$1" "$(cat "$AMX_DIR/$1.state")" "$(cat "$AMX_DIR/$1.session" 2>/dev/null)" "$question"
	;;
stop)
	printf 'stopped\n' >"$AMX_DIR/$1.state"
	;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

## ------------------------------------------------------- a run under amx

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null

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

sess=$(cat "$rundir/t1.session")
like "$sess" '^wf-t1-[0-9a-z]{4}$' 'the handle the run records is the amx agent name'
saw "new|--name|$sess|--dir|$XDG_STATE_HOME/workflow/worktrees/app/amx-run/t1|--no-worktree|--role|worker|--model|opus|Read $XDG_CACHE_HOME/workflow/briefs/app/amx-run/t1.md and execute it exactly." \
	'the dispatch is amx new into the task worktree, with the brief as the task'
rsess=$(cat "$rundir/t1.review-session")
like "$(grep "^sub|--bg|--json|--name|$rsess|" "$argv")" \
	"|--parent|$sess|--dir|$XDG_STATE_HOME/workflow/worktrees/app/amx-run/_integration|--no-worktree|--role|reader|" \
	'the reader is dispatched as a child of the worker, through amx sub --bg'
saw "status|$sess|--json" 'liveness and the ending are read off amx status --json'

is "$(cat "$rundir/hang.state")" failed 'the worker that never reported ready is failed'
is "$(cat "$rundir/hang.dispatches")" 2 'after exactly one redispatch'
saw "stop|$(cat "$rundir/hang.session")" 'a stalled worker is ended with amx stop'
is "$(grep -c '|--name|wf-hang-' "$argv")" 2 'and each dispatch of it ran under its own agent name'
first=$(grep '^new|--name|wf-hang-' "$argv" | head -1 | cut -d'|' -f3)
like "$(grep '^sub|--bg|--json|--name|wf-hang-' "$argv")" "|--parent|$first|--dir|" \
	'the redispatch is a child of the first session, through amx sub'
isnt "$first" "$(cat "$rundir/hang.session")" \
	'the second name is not the first'

## ------------------------------------------- a launch amx refuses

# `amx new` at its cap, or with no tmux to reach, exits non-zero and starts
# nothing. The task fails on the line amx printed rather than waiting out a
# stall deadline for a pane that never came up.
printf 'amx: 5 agents already running here, and max_agents is 5\n' >"$AMX_DIR/refuse"
cat >"$T_TMP/full.md" <<'PLAN'
# plan: full

- [ ] f1 Add the first service
      Files: app/f1.php
      Verify: true
- [ ] f2 Add the second service
      Files: app/f2.php
      Verify: true
PLAN
run workflow run --plan-file "$T_TMP/full.md"
is "$RC" 1 'a refused launch fails the run'
fulldir="$XDG_STATE_HOME/workflow/runs/app/full"
is "$(cat "$fulldir/f1.state")" failed 'the task whose launch was refused is failed'
is "$(cat "$fulldir/f1.failed")" 'the launch was refused: amx: 5 agents already running here, and max_agents is 5' \
	'on the line amx printed'
is "$(cat "$fulldir/f1.dispatches")" 1 'and not retried: the next launch meets the same cap'
rm -f "$AMX_DIR/refuse"

# The refused launch left a name amx never created in f1.session. The run
# that retries it names no parent -- amx would refuse one it has no record
# of, and every dispatch after -- and goes out as a plain `amx new`.
never=$(cat "$fulldir/f1.session")
run workflow run --plan-file "$T_TMP/full.md"
is "$RC" 0 'with the cap lifted the plan merges'
is "$(cat "$fulldir/f1.state")" merged 'the refused task included'
is "$(grep -c "|--parent|$never|" "$argv")" 0 'a session amx never created is nobody's parent'
like "$(grep '|--name|wf-f1-' "$argv" | tail -1)" '^new|--name|wf-f1-' 'so its retry is amx new'

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

## ------------------------------------- a reader stopped at a question

# A question drawn in front of the session -- claude's folder-trust screen --
# is on the screen and nowhere else: no hook reports it, no answer file is
# written, and amx reads the pane as waiting, which is an ending. The run's
# whole account of that used to be "no verdict", retried and failed; on
# 2026-09-16 it took an hour to find the screen behind it. amx status --json
# carries the question, so the failure names it.
cat >"$T_TMP/trust.md" <<-'PLAN'
	# plan: trust

	- [ ] trust Add the trust service
	      Files: app/trust.php
	      Verify: true
	- [ ] beside Add the beside service, so the plan is two tasks and runs
	      Files: app/beside.php
	      Verify: true
PLAN
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/trust.md"
is "$RC" 1 'a reader that never gets past a question fails the run'
trustdir="$XDG_STATE_HOME/workflow/runs/app/trust"
is "$(cat "$trustdir/trust.state" 2>/dev/null)" failed 'the task whose readers sat at the screen is failed'
like "$(cat "$trustdir/trust.failed")" '^the review ended with no verdict; the reader stopped at a question: "Quick safety check: Is this a project you created or one you trust\?" -- read ' \
	'and the note names the question the reader was sitting on'
like "$OUT" 'task trust: the review ended with no verdict; the reader stopped at a question: "Quick safety check: .* -- one more reading' \
	'and so did the line that sent the second reading'
