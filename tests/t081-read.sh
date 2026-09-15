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
rm -f app/notes.txt
