#!/usr/bin/env bash
# A reading past its deadline answers in place (m1-lessons ruling 5). The
# reader is told, in its own session, to write its answer file now with what
# it has, and given a grace; a verdict written in it is judged like any
# other, and a reader that writes nothing fails the task with the answer file
# carrying the ending -- never a second cold reading of the same prompt,
# which starts from nothing and takes as long. And the settled section
# carries rulings since the plan's first run, not this one.
source "$(dirname -- "$0")/lib.sh"
t_init

unset WORKFLOW_REVIEW_MODEL
export WF_TMP="$T_TMP"
export AMX_DIR="$T_TMP/amx"
mkdir -p "$AMX_DIR"
argv="$AMX_DIR/argv"
: >"$argv"

saw() {
	if grep -qF -- "$1" "$argv"; then ok "$2"; else notok "$2" "$(cat "$argv")"; fi
}
count() { grep -cF -- "$1" "$argv"; }
readers() { grep -cE "^sub\|--bg\|--json\|--name\|wf-$1-review-[0-9a-z]{4}\|" "$argv"; }

# Workers do their task on `new`. A reader idles with nothing written until
# `send` brings the answer-now line: the reader of `late` then ships, the
# reader of `mute` stays mute.
write_exec "$T_TMP/fake-amx" <<'AMX'
#!/bin/sh
(IFS='|'; printf '%s\n' "$*") >>"$AMX_DIR/argv"
verb=$1
shift
say() { printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "${2:-}" >>"$status"; }
commit() { git -c core.hooksPath=/dev/null commit -qm "$1" >/dev/null; }
case $verb in
new | sub)
	name= dir= text=
	while [ $# -gt 0 ]; do
		case $1 in
		--name) name=$2; shift 2 ;;
		--dir) dir=$2; shift 2 ;;
		--model | --effort | --parent | --role) shift 2 ;;
		--no-worktree | --bg | --json) shift ;;
		*) text=$1; shift ;;
		esac
	done
	[ "$verb" = sub ] && printf '{"id":"%s","parent":null,"phase":"starting","answer":null,"evidence":"hooks"}\n' "$name"
	brief=${text#Read }
	brief=${brief% and execute it exactly.}
	printf '%s\n' "$brief" >"$AMX_DIR/$name.brief"
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	if [ -n "$answer" ]; then
		printf '%s\n' "$answer" >"$AMX_DIR/$name.answer"
		cp "$brief" "$AMX_DIR/$name.prompt"
		printf 'working hooks\n' >"$AMX_DIR/$name.state"
		exit 0
	fi
	status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
	task=$(basename "$brief" .md)
	cd "$dir" || exit 1
	say started
	mkdir -p app
	printf 'final\n' >"app/$task.php"
	git add "app/$task.php"
	commit "Add the $task service"
	say ready
	printf 'idle hooks\n' >"$AMX_DIR/$name.state"
	;;
send)
	name=$1
	text=$2
	printf '%s\n' "$text" >"$AMX_DIR/$name.sent"
	case $name in
	wf-late-review-*)
		printf 'VERDICT: ship\n- [later] app/late.php:1 -- read in a hurry.\n' >"$(cat "$AMX_DIR/$name.answer")"
		printf 'idle hooks\n' >"$AMX_DIR/$name.state"
		;;
	esac
	;;
status)
	[ -f "$AMX_DIR/$1.state" ] || exit 1
	read -r state evidence <"$AMX_DIR/$1.state"
	printf '{"id":"%s","state":"%s","evidence":"%s","last_event":0,"session":"","last_words":"still reading"}\n' "$1" "$state" "$evidence"
	;;
stop) printf 'stopped record\n' >"$AMX_DIR/$1.state" ;;
esac
exit 0
AMX
export WORKFLOW_AMX="$T_TMP/fake-amx"

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
"$MEM_BIN" project set review-model fable >/dev/null

## --------------------------- a ruling from before this run, after the first

rundir="$XDG_STATE_HOME/workflow/runs/app/deadline"
mkdir -p "$rundir"
printf '%s\n' "$(($(date +%s) - 600))" >"$rundir/first-started"
"$MEM_BIN" save --kind ruling --type design 'ruling: late ships on a partial reading - the deadline is the budget - cost if wrong: a task read twice' >/dev/null

cat >"$T_TMP/deadline.md" <<'PLAN'
# plan: deadline

- [ ] late Add the late service
      Files: app/late.php
      Verify: true
- [ ] mute Add the mute service
      Files: app/mute.php
      Verify: true
PLAN
export WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 WORKFLOW_REVIEW_DEADLINE_MIN=0.05
run workflow run --plan-file "$T_TMP/deadline.md"
is "$RC" 1 'the run stops short over the mute reading'
line='Time is up: write your answer file now with what you have. Partial findings and a verdict beat none.'

## ------------------------------------------- told to answer, and it does

lsess=$(cat "$rundir/late.review-session")
is "$(cat "$rundir/late.state")" merged 'late merged on the answer it wrote when told'
saw "send|$lsess|$line" 'the reader was told in its own session'
is "$(readers late)" 1 'and was the one reader late had'
is "$(cat "$rundir/late.review-tries")" 1 'one reading counted'
like "$OUT" "task late: the review is past its 3 second deadline -- told to answer now, 3 s more \\(session $lsess\\)" 'the run says so, with the grace'
like "$OUT" 'task late: the reviewer says ship' 'and judged the late answer like any other'
like "$(cat "$AMX_DIR/$lsess.prompt")" 'A ruling saved since this plan'"'"'s first run:' 'the prompt carried the ruling from before this run'
like "$(cat "$AMX_DIR/$lsess.prompt")" 'late ships on a partial reading' 'by its body'

## ------------------------------------------ told to answer, and says nothing

msess=$(cat "$rundir/mute.review-session")
is "$(cat "$rundir/mute.state")" failed 'mute fails'
saw "send|$msess|$line" 'after being told'
is "$(readers mute)" 1 'and no second reading was started'
like "$(cat "$rundir/mute.failed")" '^the review ran past its 3 second deadline, was told to answer and wrote nothing in the 3 s after, and was stopped -- read ' 'the note says what happened, naming the file'
like "$(cat "$rundir/mute.review")" '^no answer: the review ran past its 3 second deadline' 'which exists and carries the ending'
like "$(cat "$rundir/mute.review")" 'still reading' 'and the reader'"'"'s last words'
saw "stop|$msess" 'the mute reader is stopped'
unlike "$OUT" 'one more reading' 'nothing was read cold again'

t_done
