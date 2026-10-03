#!/usr/bin/env bash
# A Read item shaped `wiki:<slug>#<section>` reaches `mem wiki -- <slug>#<section>`
# at dispatch, so the brief carries that section's bytes alone under a heading
# keeping the name as written; a bare `wiki:<slug>` still carries the whole
# page, and a section mem refuses says either half may be wrong.
source "$(dirname -- "$0")/lib.sh"
t_init

unset WORKFLOW_REVIEW_MODEL
export WF_TMP="$T_TMP"

# One fake plays worker and reader; the assertions read the briefs the run
# wrote.
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
printf '%s %s\n' "$(basename "$(dirname "$PWD")")" "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
printf '%s ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"result":"ok"}\n'
FAKE
export FAKE="$T_TMP/worker.sh"
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$FAKE" {task} {worktree} {status} {session} {brief} {model}'"'"' > {out} 2> {err} &'

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
export WORKFLOW_REVIEW_MODEL=fable

printf '# Pricing\n\nThe cart totals in cents.\n\n## Rounding\n\nHalf up, once, at the end.\n\n## Refunds\n\nRefunds reverse the rounding.\n' |
	"$MEM_BIN" wiki pricing --stdin --note 'three sections' >/dev/null

## ------------------------------------------------- one section, and a bad one

cat >"$T_TMP/section.md" <<-EOF
# plan: section

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Read: wiki:pricing#rounding wiki:pricing#taxes
      Verify: true
- [ ] t2 Add the t2 service
      Files: app/t2.php
      Verify: true
EOF

run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/section.md"
is "$RC" 0 'a task naming a section, present or absent, still merges'

body=$(cat "$XDG_CACHE_HOME/workflow/briefs/app/section/t1.md")
like "$body" '### wiki:pricing#rounding' 'the heading keeps the name as written'
like "$body" 'Half up, once, at the end\.' "with the section's text inlined"
unlike "$body" 'The cart totals in cents' 'and not the top of the page'
unlike "$body" 'Refunds reverse the rounding' 'nor the sibling section'
like "$body" '### wiki:pricing#taxes' 'the section the page lacks is named too'
like "$body" 'This project has no such page or section\.' 'saying either half may be wrong'

## --------------------------------------------------------- the whole page

cat >"$T_TMP/page.md" <<-EOF
# plan: page

- [ ] t1 Add the t1 service
      Files: app/t1.php
      Read: wiki:pricing
      Verify: true
- [ ] t2 Add the t2 service
      Files: app/t2.php
      Verify: true
EOF

run env WORKFLOW_DEADLINE_MIN=0.5 workflow run --plan-file "$T_TMP/page.md"
is "$RC" 0 'a task naming the page merges'

page=$(cat "$XDG_CACHE_HOME/workflow/briefs/app/page/t1.md")
like "$page" '### wiki:pricing' 'the bare slug heads the page'
like "$page" 'The cart totals in cents\.' 'which carries the top'
like "$page" 'Half up, once, at the end\.' 'and the first section'
like "$page" 'Refunds reverse the rounding\.' 'and the second'
