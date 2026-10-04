#!/usr/bin/env bash
# A task with a Show line merges only on evidence filed since its dispatch:
# the first refusal sends it out once more with the command in its brief, a
# second fails it, and an item filed before the dispatch never counts. A task
# with no Show line merges as it always did.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
tries=$(($(cat "$WF_TMP/$task.tries" 2>/dev/null || echo 0) + 1))
printf '%s\n' "$tries" >"$WF_TMP/$task.tries"
say started
if [ ! -f "$task.txt" ]; then
	printf '%s\n' "$task" >"$task.txt"
	git add "$task.txt"
	git -c core.hooksPath=/dev/null commit -qm "Add $task"
fi
file=
case $task in
once) file=yes ;;
second) [ "$tries" -ge 2 ] && file=yes ;;
esac
if [ -n "$file" ]; then
	printf 'transcript of %s, attempt %s\n' "$task" "$tries" >"$WF_TMP/$task-$tries.txt"
	mem evidence add --task "$task" "$WF_TMP/$task-$tries.txt" --note "attempt $tries" >/dev/null
fi
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'
export WORKFLOW_DEADLINE_MIN=0.5

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
"$MEM_BIN" plan --stdin >/dev/null <<'PLAN'
# plan: shown

- [ ] never Never files its evidence
      Files: never.txt
      Verify: true
      Show: the page it changed
- [ ] second Files its evidence when asked again
      Files: second.txt
      Verify: true
      Show: the page it changed
- [ ] once Files its evidence at once
      Files: once.txt
      Verify: true
      Show: the page it changed
- [ ] before Has only evidence filed before it went out
      Files: before.txt
      Verify: true
      Show: the page it changed
- [ ] plain Has no Show line
      Files: plain.txt
      Verify: true
PLAN

# Left by an earlier attempt, before this run dispatched anything.
printf 'an old capture\n' >"$T_TMP/old.txt"
"$MEM_BIN" evidence add --task before "$T_TMP/old.txt" --note 'an old capture' >/dev/null

rundir="$XDG_STATE_HOME/workflow/runs/app/shown"
run workflow run
isnt "$RC" 0 'the run ends with tasks failed'

refusal='no Show evidence since its dispatch -- capture it and file it: mem evidence add --task never <file> --note "<what it shows>"'
is "$(cat "$rundir/never.state")" failed 'no evidence twice: failed'
is "$(cat "$rundir/never.dispatches")" 2 'after one more try'
is "$(cat "$rundir/never.failed")" "$refusal" 'the failure note is the refusal, with the command'
like "$OUT" "task never: failed -- no Show evidence since its dispatch .*mem evidence add --task never" \
	'and the run says so'
is "$([ -f "$rundir/never.show_asked" ] && echo yes)" yes 'the first refusal is marked as asked'
is "$(grep -c 'failed never' "$rundir/events")" 1 'one failed event, for the second refusal only'
like "$(cat "$XDG_CACHE_HOME"/workflow/briefs/app/shown/never.md 2>/dev/null)" \
	'no Show evidence since its dispatch' 'the second brief carries the refusal as the attempt before'

is "$(cat "$rundir/second.state")" merged 'evidence on the second attempt: merged'
is "$(cat "$rundir/second.dispatches")" 2 'on its second dispatch'

is "$(cat "$rundir/once.state")" merged 'evidence at once: merged'
is "$(cat "$rundir/once.dispatches")" 1 'on its first dispatch'

is "$(cat "$rundir/before.state")" failed 'evidence from before the dispatch only: failed'
is "$(wc -l <"$rundir/before.evidence_seen")" 1 'the old item was seen at dispatch'
like "$(cat "$rundir/before.failed")" 'mem evidence add --task before' 'with the command in its note'

is "$(cat "$rundir/plain.state")" merged 'no Show line: merged as before'
is "$(cat "$rundir/plain.dispatches")" 1 'at once'
is "$([ -e "$rundir/plain.evidence_seen" ] || echo none)" none 'and nothing was counted for it'
