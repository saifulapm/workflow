#!/usr/bin/env bash
# `workflow advise` -- a worker asks a stronger model at a decision point,
# without ending its turn. Dispatched like a reader, through the same
# template: a task id ending `-advice-<n>` is a consult, and the fake below
# answers the first and stays silent on the second, the way a session that
# ended without writing a verdict does.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; brief=$5
case $task in
*-advice-1)
	answer=$(sed -n 's/^    Answer file: //p' "$brief")
	cp "$brief" "$WF_TMP/advice-prompt-$(date +%s%N)"
	printf 'use the service\n' >"$answer"
	;;
*-advice-2)
	cp "$brief" "$WF_TMP/advice-prompt-$(date +%s%N)"
	# writes nothing: the session ends with no answer
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

"$MEM_BIN" plan --stdin >/dev/null <<'EOF'
# plan: live

## Spec

Ruling 1. The t1 service says what the advisor said.

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Verify: true
      Done: app/t1.php carries the fix
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/live"
mkdir -p "$rundir"

## -------------------------------------------------- nobody named to advise

run env WORKFLOW_TASK=live/t1 WORKFLOW_ADVISOR= WORKFLOW_REVIEW_MODEL= workflow advise 'which file owns rounding?'
is "$RC" 2 'with no advisor and no reader the consult is refused'
like "$OUT" 'mem project set advisor' 'naming the remedy'
[ -f "$rundir/t1.advised" ] && notok 'and no consult was counted' "$(cat "$rundir/t1.advised")" || ok 'and no consult was counted'

## ----------------------------------------------------------- the first ask

run env WORKFLOW_TASK=live/t1 WORKFLOW_ADVISOR=sage WORKFLOW_REVIEW_MODEL= workflow advise 'which file owns rounding?' --file app/t1.php
is "$RC" 0 'an answered consult exits 0'
like "$OUT" 'use the service' 'and prints the answer'
is "$(cat "$rundir/t1.advised")" 1 'the consult count is one'
like "$(cat "$rundir/t1.advice.1")" 'use the service' 'kept as <task>.advice.<n> in the run dir'
prompt=$(ls -t "$WF_TMP"/advice-prompt-* | head -1)
like "$(cat "$prompt")" '# Advice for t1' 'the prompt names the task'
like "$(cat "$prompt")" 'Ruling 1\. The t1 service' 'and carries the plan of record'
like "$(cat "$prompt")" 'Done: app/t1\.php carries the fix' 'and the task block'
like "$(cat "$prompt")" 'which file owns rounding\?' 'and the question'
like "$(cat "$prompt")" 'draft' 'and the named file, inlined'
like "$(cat "$prompt")" 'Answer file: '"$rundir"'/t1\.advice\.1' 'naming where the answer goes'
like "$("$MEM_BIN" log --type run --json)" 'task t1: advised \(1\) -- which file owns rounding\?' 'the log line names the consult and the question'

## --------------------------------------------------------------- the second, silent

run env WORKFLOW_TASK=live/t1 WORKFLOW_ADVISOR=sage WORKFLOW_REVIEW_MODEL= WORKFLOW_REVIEW_DEADLINE_MIN=0.05 workflow advise 'and this one?'
is "$RC" 1 'a session that ends with no answer exits 1'
is "$(cat "$rundir/t1.advised")" 2 'the consult count is two'
[ -f "$rundir/t1.advice.2" ] && notok 'and nothing was kept for it' "$(cat "$rundir/t1.advice.2")" || ok 'and nothing was kept for it'

## ------------------------------------------------------- capped at three

printf '3\n' >"$rundir/t1.advised"
run env WORKFLOW_TASK=live/t1 WORKFLOW_ADVISOR=sage WORKFLOW_REVIEW_MODEL= workflow advise 'a fourth?'
is "$RC" 2 'a fourth consult this attempt is refused'
like "$OUT" 'three consults this attempt; the fourth is a mem ask' 'and says so'
is "$(cat "$rundir/t1.advised")" 3 'the count is left at three, not bumped'
[ -f "$rundir/t1.advice.4" ] && notok 'nothing was dispatched for it' "$(cat "$rundir/t1.advice.4")" || ok 'nothing was dispatched for it'

## ------------------------------------------------------- outside a run

run env WORKFLOW_ADVISOR=sage WORKFLOW_REVIEW_MODEL= workflow advise 'is this cache safe?'
is "$RC" 2 'outside a run, --against is required'
like "$OUT" 'against.*required' 'and says so'

run env WORKFLOW_ADVISOR=sage WORKFLOW_REVIEW_MODEL= workflow advise 'is this cache safe?' --against 'the cache never outlives a request'
is "$RC" 1 'the process seam never answers outside a run either, so the call still ends without one'
