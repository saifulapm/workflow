#!/usr/bin/env bash
# The shipped files themselves: skills inside their budgets, adapters that
# mandate the one line without which they are ungated, and the command surface.
source "$(dirname -- "$0")/lib.sh"
t_init

## ------------------------------------------------------------- the skills

for s in route plan implement review orchestrate; do
	f="$WF_ROOT/skills/$s/SKILL.md"
	truthy "$([ -f "$f" ] && echo 0 || echo 1)" "skills/$s/SKILL.md exists"
	like "$(head -1 "$f")" '^---$' "skills/$s starts with frontmatter"
	like "$(sed -n '2,4p' "$f")" "name: $s" "skills/$s names itself"
	like "$(sed -n '2,4p' "$f")" 'description: Use ' "skills/$s describes when to use it"
done

# Line ceilings from the spec: route 50, plan 100, review 80, orchestrate 100.
is "$(($(wc -l <"$WF_ROOT/skills/route/SKILL.md") <= 50))" 1 'route is within 50 lines'
is "$(($(wc -l <"$WF_ROOT/skills/plan/SKILL.md") <= 100))" 1 'plan is within 100 lines'
is "$(($(wc -l <"$WF_ROOT/skills/review/SKILL.md") <= 80))" 1 'review is within 80 lines'
is "$(($(wc -l <"$WF_ROOT/skills/orchestrate/SKILL.md") <= 100))" 1 'orchestrate is within 100 lines'

# The byte budgets, checked the way the machine checks them.
run workflow doctor
unlike "$OUT" 'skill .*over the' 'every shipped skill is inside its byte budget'
like "$OUT" 'skill implement .*within budget' 'and doctor saw all four'

# Skills point at the subcommands rather than restating what they do.
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'workflow review-needed' 'route defers to review-needed'
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'workflow verify' 'route defers to verify'
plan_skill=$(cat "$WF_ROOT/skills/plan/SKILL.md")
like "$plan_skill" 'workflow plan-check' 'plan defers to the parser'
# The check and the run are different acts, and sending a planner to the run
# is how an unapproved plan once dispatched live workers.
unlike "$plan_skill" 'workflow run --plan-file plan' 'and not to the orchestrator'
like "$(cat "$WF_ROOT/skills/implement/SKILL.md")" 'workflow verify' 'implement defers to verify'
like "$(cat "$WF_ROOT/skills/review/SKILL.md")" 'workflow review-needed' 'review defers to review-needed'
orchestrate_skill=$(cat "$WF_ROOT/skills/orchestrate/SKILL.md")
like "$orchestrate_skill" 'workflow status --json' 'orchestrate polls status'
like "$orchestrate_skill" 'workflow plan-check' 'orchestrate checks the plan before running it'
like "$orchestrate_skill" 'mem save --kind ruling' 'orchestrate records its decisions as rulings'
like "$orchestrate_skill" 'mem ask' 'orchestrate escalates through the question channel'
# The binary decides mechanics; the session decides judgment. A skill that
# edits project code has crossed the line the layer exists to draw.
like "$orchestrate_skill" 'ever edit project code' 'orchestrate forbids touching the code'
# A merge landed on a leftover scaffold branch once. Nothing in the binary can
# catch that: the merge is a human hand on a checkout it did not choose.
like "$orchestrate_skill" 'git branch --show-current' \
	'orchestrate checks which branch the checkout is on before merging'
like "$orchestrate_skill" 'Leave the checkout on main' \
	'and puts it back on main when the run ends'

## ---------------------------------------------------------------- the wiki

# A page is read before a subsystem is touched and rewritten after it changes,
# so every skill on that path has to name the verb. A wiki nobody is told about
# is a wiki nobody writes.
mem_skill=$(cat "$WF_ROOT/skills/mem/SKILL.md")
like "$mem_skill" 'mem wiki' 'mem names the wiki'
like "$mem_skill" 'mem wiki <slug> --stdin --note' 'mem shows how a page is written'
# The index is a page like any other, and no verb writes it: whoever adds a
# page adds its line, or doctor is left to report the drift.
like "$mem_skill" 'index' 'mem says the index is maintained by hand'
# Deleting a page does not stick: bisync brings it back (gotcha #PK0TGG25).
# The store's answer is to archive in place, and only the skill can say so.
like "$mem_skill" 'stub' 'mem teaches the stub instead of a delete that will not hold'
like "$plan_skill" 'mem wiki' 'plan reads the pages before it cuts tasks'
implement_skill=$(cat "$WF_ROOT/skills/implement/SKILL.md")
like "$implement_skill" 'mem wiki .*--stdin --note' 'implement rewrites the page it touched'
# Nothing refuses an oversized or unlinked page: doctor reports it and a batch
# review is where someone acts on the report.
like "$orchestrate_skill" 'mem doctor' 'orchestrate lints the wiki in a batch review'
like "$orchestrate_skill" 'compact' 'and compacts the pages that have outgrown themselves'
like "$(cat "$WF_ROOT/README.md")" 'mem wiki' 'the README puts the wiki among the reads'

## ------------------------------------------------------------ the adapters

# codex only: pi reads the skills natively, so its adapter retired
# (2026-08-31) in favour of pi's own settings and a workflow-gate extension.
for a in codex; do
	f="$WF_ROOT/adapters/$a.md"
	truthy "$([ -f "$f" ] && echo 0 || echo 1)" "adapters/$a.md exists"
	like "$(cat "$f")" 'export WORKFLOW_AGENT=1' "adapters/$a mandates exporting WORKFLOW_AGENT"
	like "$(cat "$f")" 'workflow verify' "adapters/$a routes through the gate"
	like "$(cat "$f")" 'mem ask' "adapters/$a keeps the stop conditions"
done
truthy "$([ ! -f "$WF_ROOT/adapters/pi.md" ] && echo 0 || echo 1)" \
	'the retired pi adapter stays gone'

## ------------------------------------------------------------ the surface

run workflow help
is "$RC" 0 'workflow help exits 0'
for sub in verify lint-msg review-needed run reap doctor; do
	like "$OUT" "  $sub" "help lists $sub"
done
like "$OUT" '0 green .*1 failed .*2 no verifier .*3 test removal' "help states verify's exit contract"

run workflow nonsense
is "$RC" 2 'an unknown command exits 2'
like "$OUT" 'unknown command: nonsense' 'and says which command it does not know'
run workflow
is "$RC" 2 'no command at all exits 2'

# A mistyped option is not a mistyped command: sending the reader to check the
# command name blames the one thing that was right.
for sub in verify lint-msg review-needed run reap doctor; do
	run workflow "$sub" --frob
	unlike "$OUT" "unknown command: $sub" "a bad option does not make $sub an unknown command"
	like "$OUT" '\-\-frob' "and $sub names the option instead"
done

# A reader that closes the pipe -- `workflow status | head` -- is the reader's
# business. Rust ignores SIGPIPE, so an unfixed binary panics and exits 101
# (friction #ECTJYVXX).
# Enough patterns to overflow the pipe buffer, so the write cannot quietly
# succeed after head has gone.
big=$(awk 'BEGIN { for (i = 0; i < 9000; i++) printf "p%d ", i }')
run bash -c 'workflow split-patterns "$1" 2>&1 | head -2; exit "${PIPESTATUS[0]}"' _ "$big"
isnt "$RC" 101 'a closed pipe does not panic the binary'
unlike "$OUT" 'panicked' 'and no stack trace reaches the terminal'
