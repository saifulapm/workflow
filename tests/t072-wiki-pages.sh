#!/usr/bin/env bash
# Wiki pages in the brief and the reader's prompt (rulings 1 and 2 of
# m1-wiki-first). A `Read:` item shaped `wiki:<slug>` is read live off `mem
# wiki -- <slug>` at dispatch and at the gate, and inlined verbatim under
# '## Pages the plan names' in both the worker's brief and the reader's
# prompt. An absent page says so rather than refusing the dispatch; past
# PAGES_CAP bytes of page text the rest are named as a `mem wiki` command
# instead of inlined, and the run warns once.
source "$(dirname -- "$0")/lib.sh"
t_init

unset WORKFLOW_REVIEW_MODEL
export WF_TMP="$T_TMP"

# One fake plays worker and reader: it logs nothing of its own, since the
# assertions read the brief and the review-prompt the run wrote.
write_exec "$T_TMP/worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$5
case $task in
*-review)
	printf 'VERDICT: ship\n' >"$(sed -n 's/^    Answer file: //p' "$brief")"
	exit 0
	;;
esac
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add the $task service"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief} {model}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
"$MEM_BIN" project set review-model fable >/dev/null

## --------------------------------------- an existing page and an absent one

printf 'The run drives waves and merges tasks.\n' | "$MEM_BIN" wiki run --stdin --note seed >/dev/null

cat >"$T_TMP/wiki.md" <<-EOF
# plan: wiki

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Read: wiki:run wiki:missing
      Verify: true
- [ ] t2 Add the t2 service
      Files: app/t2.php
      Verify: true
EOF

rundir="$XDG_STATE_HOME/workflow/runs/app/wiki"
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/wiki.md"
is "$RC" 0 'a task naming a wiki page, present or absent, still merges'
is "$(cat "$rundir/t1.state")" merged 't1 merged'

brief="$XDG_CACHE_HOME/workflow/briefs/app/wiki/t1.md"
body=$(cat "$brief")
like "$body" '## Pages the plan names' 'the brief carries the pages heading'
like "$body" '### wiki:run' 'naming the page it read'
like "$body" 'The run drives waves and merges tasks\.' 'with its text inlined verbatim'
like "$body" '### wiki:missing' 'and the absent page is named too'
like "$body" 'This project has no such page' 'saying so rather than refusing the dispatch'

review="$rundir/t1.review-prompt"
rbody=$(cat "$review")
like "$rbody" '## Pages the plan names' 'the reviewer sees the same heading'
like "$rbody" 'The run drives waves and merges tasks\.' 'and the same page text'
like "$rbody" '### wiki:missing' 'and the same absent page'

# A task naming no pages carries no heading at all.
brief2="$XDG_CACHE_HOME/workflow/briefs/app/wiki/t2.md"
unlike "$(cat "$brief2")" 'Pages the plan names' 't2 named no pages, so no heading'

## -------------------------------------------- pages past the byte cap

big=$(head -c 24001 /dev/zero | tr '\0' 'x')
printf '%s' "$big" | "$MEM_BIN" wiki big --stdin --note seed >/dev/null
printf 'a small page\n' | "$MEM_BIN" wiki small --stdin --note seed >/dev/null

cat >"$T_TMP/cap.md" <<-EOF
# plan: cap

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Read: wiki:big wiki:small
      Verify: true
- [ ] t2 Add the t2 service
      Files: app/t2.php
      Verify: true
EOF

caprun="$XDG_STATE_HOME/workflow/runs/app/cap"
run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/cap.md"
is "$RC" 0 'a plan whose pages pass the cap still merges'
is "$(cat "$caprun/t1.state")" merged 't1 merged'

capbrief=$(cat "$XDG_CACHE_HOME/workflow/briefs/app/cap/t1.md")
like "$capbrief" "$big" 'the page under the cap is inlined whole'
unlike "$capbrief" 'a small page' 'the page past the cap is not inlined'
like "$capbrief" '`mem wiki -- small`' 'and is named as a command to fetch it instead'
like "$OUT" 'its pages are past the 24000 byte cap' 'the run warns once about the cap'
