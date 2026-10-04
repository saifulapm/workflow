#!/usr/bin/env bash
# The commit gate and the merge gate run hygiene: pre-commit reads the staged
# diff after the suite, commit-msg reads the message, WORKFLOW_HYGIENE=skip
# clears a human commit and never an agent's, and the merge gate names the
# branch commit whose message failed.
source "$(dirname -- "$0")/lib.sh"
t_init
chmod +x "$HOOKS"/pre-commit "$HOOKS"/commit-msg 2>/dev/null

## ------------------------------------------------- in a run worktree

mkdir -p "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a" "$XDG_STATE_HOME/workflow/runs/proj/plan-a"
new_repo wt
mv "$T_TMP/wt" "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a/t1"
cd "$XDG_STATE_HOME/workflow/worktrees/proj/plan-a/t1" || exit 1
printf 'true\n' >"$XDG_STATE_HOME/workflow/runs/proj/plan-a/t1.verify"

mkdir -p src
printf 'fn main() {}\n// cached because ruling 4 says so\n' >src/cache.rs
git add src/cache.rs
run git_gated commit -m 'Cache the supplier lookup'
isnt "$RC" 0 'pre-commit: a line citing a numbered ruling is refused'
like "$OUT" '^src/cache\.rs:2: hard numbered ruling: // cached because ruling 4 says so' 'pre-commit: with its path and line'
is "$(git log --oneline | wc -l)" 1 'pre-commit: nothing was committed'

run env WORKFLOW_HYGIENE=skip git -c core.hooksPath="$HOOKS" commit -m 'Cache the supplier lookup'
isnt "$RC" 0 'skip: ignored in a run worktree, with no agent variable set'

printf 'fn main() {}\n// cached because the supplier allows twelve calls a minute\n' >src/cache.rs
git add src/cache.rs
subject='Cache the supplier lookup for five minutes so the importer stays under its limit'
is "${#subject}" 80 'the subject is 80 characters'
run git_gated commit -m "$subject"
isnt "$RC" 0 'commit-msg: an 80-character subject is refused'
like "$OUT" 'hard long subject: ' 'commit-msg: named as a long subject'

run git_gated commit -m 'Cache the supplier lookup'
is "$RC" 0 'the reason in place of the citation, under a short subject, goes through'

## ------------------------------------------- in a registered checkout

new_repo human
mem_register
"$MEM_BIN" project set verify true >/dev/null
printf 'fn main() {}\n// cached because ruling 4 says so\n' >cache.rs
git add cache.rs

run env -u WORKFLOW_AGENT -u PI_CODING_AGENT git -c core.hooksPath="$HOOKS" commit -m 'Cache the supplier lookup'
isnt "$RC" 0 'a human commit in a registered checkout is held to hygiene too'
like "$OUT" '^cache\.rs:2: hard numbered ruling: ' 'and told the line'

run env WORKFLOW_AGENT=1 WORKFLOW_HYGIENE=skip git -c core.hooksPath="$HOOKS" commit -m 'Cache the supplier lookup'
isnt "$RC" 0 'skip: ignored with WORKFLOW_AGENT set'
like "$OUT" 'hard numbered ruling' 'skip: and the finding still named'

run env -u WORKFLOW_AGENT -u PI_CODING_AGENT WORKFLOW_HYGIENE=skip git -c core.hooksPath="$HOOKS" commit -m "$subject"
is "$RC" 0 'skip: honoured without WORKFLOW_AGENT, for the diff and the message'
is "$(git log -1 --format=%s)" "$subject" 'skip: the commit landed'

## ----------------------------------------------------------- merge gate

cd "$T_TMP" || exit 1
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
if [ "$task" = t2 ]; then
	printf 'report\n' >report.txt
	git add report.txt
	git -c core.hooksPath=/dev/null commit -qm 'Add the supplier report'
	say ready
	printf '{"is_error":false,"result":"ok"}\n'
	exit 0
fi
printf 'one\n' >lookup.txt
git add lookup.txt
git -c core.hooksPath=/dev/null commit -qm 'Add the supplier lookup'
printf 'two\n' >>lookup.txt
git add lookup.txt
git -c core.hooksPath=/dev/null commit -qm 'Cache the supplier lookup

Cached because ruling 4 says so.'
printf 'three\n' >>lookup.txt
git add lookup.txt
git -c core.hooksPath=/dev/null commit -qm 'Log the supplier lookup'
say ready
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status}'"'"' > {out} 2> {err} &'
export WORKFLOW_DEADLINE_MIN=0.5

# The suite the gate runs says where its scratch space is.
write_exec "$T_TMP/suite" <<SUITE
#!/bin/sh
printf '%s\\n' "\$TMPDIR" >>"$T_TMP/suite-tmpdir"
SUITE

new_repo app
mem_register
"$MEM_BIN" project set verify "$T_TMP/suite" >/dev/null
"$MEM_BIN" plan --stdin >/dev/null <<'PLAN'
# plan: gate

- [ ] t1 Add the supplier lookup
      Files: lookup.txt
      Verify: true
- [ ] t2 Add the supplier report
      Files: report.txt
      Verify: true
PLAN

rundir="$XDG_STATE_HOME/workflow/runs/app/gate"
run workflow run
isnt "$RC" 0 'merge gate: the run does not merge the task'
is "$(cat "$rundir/t2.state")" merged 'merge gate: a sibling with clean messages merges'
like "$(cat "$rundir/t1.failed")" 'commit [0-9a-f]{7,} "Cache the supplier lookup" did not pass hygiene' \
	'merge gate: the failure names the commit whose message failed'
like "$OUT" 'hard numbered ruling: Cached because ruling 4 says so' 'merge gate: and the run output names the line'
is "$(git log --oneline integration/gate 2>/dev/null | grep -c 'supplier lookup')" 0 'merge gate: nothing landed on integration'
is "$(sort -u "$T_TMP/suite-tmpdir")" "$rundir/gate.tmp" \
	'merge gate: its suite keeps scratch under the run directory, not the shared /tmp'
is "$(ls -A "$rundir/gate.tmp" 2>/dev/null)" '' 'merge gate: and nothing is left in it'
