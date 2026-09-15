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
task=$1; brief=$5
case $task in
read-review)
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	cp "$brief" "$WF_TMP/read-prompt-$(date +%s%N)"
	if grep -q '^\+good$' "$brief"; then
		printf 'VERDICT: ship\nThe diff is clean.\n' >"$answer"
	elif grep -q '^\+bad$' "$brief"; then
		printf 'VERDICT: fix\n- [blocks] app/bad.php:1 -- never good enough.\n' >"$answer"
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

run env WORKFLOW_REVIEW_MODEL= workflow read
is "$RC" 2 'with no reader named the read is refused'
like "$OUT" 'mem project set review-model' 'naming the remedy'

"$MEM_BIN" project set review-model fable >/dev/null

## ------------------------------------------------------------- a clean diff

printf 'good\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 0 'a diff the reader ships exits 0'
like "$OUT" 'VERDICT: ship' 'and prints the verdict file'

## --------------------------------------------------------------- a bad diff

printf 'bad\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 1 'a diff the reader wants fixed exits 1'
like "$OUT" '\[blocks\] app/bad.php:1' 'and prints the findings'

## ------------------------------------------------------- no verdict at all

printf 'quiet\n' >app/t1.php
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.05 workflow read
is "$RC" 3 'a reading that ends with no verdict exits 3'
like "$OUT" 'no verdict -- read .*\.review$' 'naming the answer file'

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
is "$RC" 3 'a range git cannot resolve is refused'
like "$OUT" 'read: fatal' 'naming what git said'

printf 'good\n' >app/t1.php
git add app/t1.php
git checkout -q --orphan fresh
printf 'a scratch note\n' >app/notes.txt
run env WORKFLOW_REVIEW_DEADLINE_MIN=0.5 workflow read
is "$RC" 3 'an unborn HEAD is refused, not read as untracked files alone'
like "$OUT" 'read: fatal' 'naming what git said about HEAD'
rm -f app/notes.txt
