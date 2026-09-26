#!/usr/bin/env bash
# `workflow read` starts the gate's own reader over a working tree, the way
# the one-shot lane sends a diff through it before a commit: no run, no
# worktree of its own, one prompt and one verdict file. The fake reader below
# plays the part WORKFLOW_WORKER_CMD lets t057's play for a worker: it reads
# what it was handed and answers ship, fix or nothing at all.
source "$(dirname -- "$0")/lib.sh"
t_init

# t_init exports an empty WORKFLOW_REVIEW_MODEL so `workflow run` tests never
# get refused for lack of a reader; naming one is what this file is about.
unset WORKFLOW_REVIEW_MODEL

export WF_TMP="$T_TMP"
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; wt=$2; session=$4; brief=$5
# The conversation file a session under this worktree would write.
transcript() {
	slug=$(printf '%s' "$wt" | sed -E 's/[^A-Za-z0-9]/-/g')
	mkdir -p "$HOME/.claude/projects/$slug"
	printf '%s\n' "$HOME/.claude/projects/$slug/$session.jsonl"
}
case $task in
read-review)
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	cp "$brief" "$WF_TMP/read-prompt-$(date +%s%N)"
	if grep -q '^\+good$' "$brief"; then
		printf 'VERDICT: ship\nThe diff is clean.\n' >"$answer"
	elif grep -q '^\+bad$' "$brief"; then
		printf 'VERDICT: fix\n- [blocks] app/bad.php:1 -- never good enough.\n' >"$answer"
	elif grep -q '^\+slow$' "$brief"; then
		# Still reading when the deadline stops it: no verdict, and its
		# own last words in the transcript its session names.
		cat >"$(transcript)" <<-'LINE'
		{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Reading app/t1.php against the requirement..."}]}}
		LINE
		sleep 5
	elif grep -q '^\+silent$' "$brief"; then
		# A reader that never reaches visible text: the turn ends on the
		# completion budget it spent reasoning, and the transcript is
		# the only place that says so.
		cat >"$(transcript)" <<-'LINE'
		{"type":"assistant","message":{"role":"assistant","stop_reason":"max_tokens","usage":{"output_tokens":32000},"content":[]}}
		LINE
		sleep 5
	fi
	# a diff holding neither writes nothing: the reading ends with no verdict.
	;;
esac
exit 0
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief} {model}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
mkdir -p app
printf 'draft\n' >app/t1.php
git add app/t1.php
git -c core.hooksPath=/dev/null commit -qm 'Add the t1 service'

## -------------------------------------------------- nobody named to read it

# Exit 4, not a usage error: nobody named is a verdict of its own, which
# the skills answer by committing without a reading and saying so.
run_out env WORKFLOW_REVIEW_MODEL= workflow read
is "$RC" 4 'with no reader named the read exits 4'
is "$OUT" 'read: verdict none -- nobody is named to read this diff' 'and says so on stdout'
run env WORKFLOW_REVIEW_MODEL= workflow read
like "$OUT" 'mem project set review-model' 'naming the remedy'

"$MEM_BIN" project set review-model fable >/dev/null

## ------------------------------------------------------------- a clean diff

# newest <name> -- that file in the newest read directory, which is where a
# caller that cannot see the exit code reads what came back.
newest() { ls -t "$XDG_STATE_HOME"/workflow/runs/app/_read/*/"$1" | head -1; }

printf 'good\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 0 'a diff the reader ships exits 0'
like "$OUT" 'VERDICT: ship' 'and prints the verdict file'
like "$OUT" '^read: verdict ship$' 'with the verdict as its own last line on stdout'
is "$(cat "$(newest read.verdict)")" ship 'and in read.verdict, where an exit code need not reach'

## --------------------------------------------------------------- a bad diff

printf 'bad\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 1 'a diff the reader wants fixed exits 1'
like "$OUT" '\[blocks\] app/bad.php:1' 'and prints the findings'
like "$OUT" '^read: verdict fix$' 'and says fix on stdout'
is "$(cat "$(newest read.verdict)")" fix 'and in read.verdict'

## ------------------------------------------------------- no verdict at all

printf 'quiet\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.05 workflow read
is "$RC" 3 'a reading that ends with no verdict exits 3'
like "$OUT" 'no verdict -- read .*\.review$' 'naming the answer file'
like "$OUT" '^read: verdict none$' 'and says so on stdout, which a relay does not eat'
like "$(cat "$(newest read.verdict)")" '^none: ' 'read.verdict carries the word and the reason'

## ------------------------------- a reading that ends with nothing to show for it

# A reader stopped at its deadline said only that it had been stopped: no
# verdict, no partial answer, and nothing about why, so a flake and a prompt
# that kills every session read the same. Its own last words are kept beside
# the answer file now, and the stderr line names that file.
printf 'slow\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.05 workflow read
is "$RC" 3 'a reader still going at its deadline gives no verdict'
like "$OUT" 'ran past its 3 second deadline' 'and the line says so'
like "$OUT" ' -- and .*read\.review-err' 'naming the file its own words are in'
like "$(cat "$(newest read.review-err)")" 'Reading app/t1.php against the requirement' \
	'which holds what the reader last said'

# A reader that spends its whole completion budget before it says anything
# visible is indistinguishable from a wedged provider from outside. The
# transcript's own stop_reason and visible-token count are on the line.
printf 'silent\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.05 workflow read
is "$RC" 3 'a reader that never reached visible text gives no verdict either'
like "$OUT" 'stop_reason=max_tokens, 32000 output tokens, no visible text' \
	'and the line names how the turn stopped and what it spent'

## ----------------------------------------- the prompt carries what it reads

git checkout -q -- app/t1.php
printf 'a scratch note about the widget\n' >app/notes.txt
printf 'good\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read --against 'the widget adds up cleanly'
is "$RC" 0 'still ships with an untracked file beside the diff'
prompt=$(ls -t "$WF_TMP"/read-prompt-* | head -1)
like "$(cat "$prompt")" 'Untracked files' 'the untracked heading is there'
like "$(cat "$prompt")" 'a scratch note about the widget' 'and its contents are inlined'
like "$(cat "$prompt")" 'Done: the widget adds up cleanly' 'and the --against text is the requirement'
plan_section=$(sed -n '/^## The plan of record$/,/^## The task$/p' "$prompt")
like "$plan_section" 'the widget adds up cleanly' 'the requirement is the plan of record too'
rm -f app/notes.txt

## ------------------------------------ a path git quotes, and harness scratch

printf 'good\n' >app/t1.php
printf 'a note about spacing\n' >'app/plain note.txt'
mkdir -p app/.claude/agent-memory
printf '{"permissions": {}}\n' >app/.claude/settings.local.json
printf 'what a sub-agent remembered\n' >app/.claude/agent-memory/reviewer.md
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 0 'an untracked path git would quote still ships'
prompt=$(ls -t "$WF_TMP"/read-prompt-* | head -1)
like "$(cat "$prompt")" '^app/plain note.txt:$' 'the path is listed unquoted'
like "$(cat "$prompt")" 'a note about spacing' 'and its contents are inlined'
unlike "$(cat "$prompt")" 'settings.local.json|what a sub-agent remembered' \
	'the harness own scratch under .claude/ is not part of the change'
unlike "$OUT" 'left the working tree changed' 'and scratch does not trip the tree check'
rm -f 'app/plain note.txt'
rm -rf app/.claude

## -------------------------------------- bytes that are not lines of a source

printf 'good\n' >app/t1.php
printf '\211PNG\r\n\032\n\000\377\376' >app/logo.png
printf 'a doc\n```\nfenced\n```\ntail line\n' >app/doc.md
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 0 'a binary untracked file still ships'
prompt=$(ls -t "$WF_TMP"/read-prompt-* | head -1)
like "$(cat "$prompt")" 'binary, [0-9]+ bytes' 'binary bytes are named, not inlined'
like "$(cat "$prompt")" '^````$' 'a file holding a fence is wrapped in a longer one'
like "$(cat "$prompt")" '^tail line$' 'so its last line is still content'
rm -f app/logo.png app/doc.md

## -------------------------------- a git command that failed is not a clean diff

git checkout -q -- app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read --range no-such-ref..also-none
is "$RC" 2 'a range git cannot resolve is refused as usage, not as a missing verdict'
like "$OUT" 'read: fatal' 'naming what git said'

printf 'good\n' >app/t1.php
git add app/t1.php
git checkout -q --orphan fresh
printf 'a scratch note\n' >app/notes.txt
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 2 'an unborn HEAD is refused, not read as untracked files alone'
like "$OUT" 'read: fatal' 'naming what git said about HEAD'
rm -f app/notes.txt
