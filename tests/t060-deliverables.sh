#!/usr/bin/env bash
# The shipped files themselves: skills inside their budgets, hooks that
# mandate the one line without which they are ungated, and the command surface.
source "$(dirname -- "$0")/lib.sh"
t_init

## ------------------------------------------------------------- the skills

for s in route plan implement review orchestrate roadmap; do
	f="$WF_ROOT/skills/$s/SKILL.md"
	truthy "$([ -f "$f" ] && echo 0 || echo 1)" "skills/$s/SKILL.md exists"
	like "$(head -1 "$f")" '^---$' "skills/$s starts with frontmatter"
	like "$(sed -n '2,4p' "$f")" "name: $s" "skills/$s names itself"
	like "$(sed -n '2,4p' "$f")" 'description: Use ' "skills/$s describes when to use it"
done

# Line ceilings from the spec: route 52, plan 100, review 80, orchestrate 100,
# roadmap 100.
is "$(($(wc -l <"$WF_ROOT/skills/route/SKILL.md") <= 52))" 1 'route is within 52 lines'
is "$(($(wc -l <"$WF_ROOT/skills/plan/SKILL.md") <= 100))" 1 'plan is within 100 lines'
is "$(($(wc -l <"$WF_ROOT/skills/review/SKILL.md") <= 80))" 1 'review is within 80 lines'
is "$(($(wc -l <"$WF_ROOT/skills/orchestrate/SKILL.md") <= 100))" 1 'orchestrate is within 100 lines'
is "$(($(wc -l <"$WF_ROOT/skills/roadmap/SKILL.md") <= 100))" 1 'roadmap is within 100 lines'

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
# An orchestrator runs in the background with nobody to ask in, so the
# escalation has to name the channel it goes out on, not just say to ask.
like "$orchestrate_skill" 'ask Saiful fresh with `mem ask`' \
	'orchestrate escalates through the question channel'
# The binary decides mechanics; the session decides judgment. A skill that
# edits project code has crossed the line the layer exists to draw.
like "$orchestrate_skill" 'ever edit project code' 'orchestrate forbids touching the code'
# A merge landed on a leftover scaffold branch once. Nothing in the binary can
# catch that: the merge is a human hand on a checkout it did not choose.
like "$orchestrate_skill" 'git branch --show-current' \
	'orchestrate checks which branch the checkout is on before merging'
like "$orchestrate_skill" 'Leave the checkout on main' \
	'and puts it back on main when the run ends'

# A roadmap is cut in one sitting and executed over many sessions, so the
# skill has to carry both halves: the verbs that store and check it, and what
# a session picking up the next milestone does.
roadmap_skill=$(cat "$WF_ROOT/skills/roadmap/SKILL.md")
like "$roadmap_skill" 'mem roadmap --stdin' 'roadmap stores the milestones'
like "$roadmap_skill" 'mem plan <slug> --set-file' 'and one plan per milestone beside them'
like "$roadmap_skill" 'workflow plan-check "\$d/roadmap\.md"' 'roadmap checks the whole thing at once'
like "$roadmap_skill" 'mem roadmap +#' 'a later session reads which milestone is next'
like "$roadmap_skill" 'mem plan --from <slug>' 'and makes its plan the plan of record'
# `--from` writes the plan of record into mem's store, and nothing puts a copy
# in the checkout. Checking a bare plan.md there reads whatever an older
# session left lying around, and the milestone runs unchecked.
like "$roadmap_skill" 'plan-check <\(mem plan\)' 'then checks that plan against the tree it will run in'
unlike "$roadmap_skill" 'plan-check plan\.md' 'and not a plan.md nothing wrote'
like "$roadmap_skill" 'One milestone' 'a session takes one milestone and no more'
# A plan cut weeks ago meets a tree that has moved. The small difference is
# the orchestrator's to absorb; the one that changes what is being built is
# not, and a session that guesses builds the wrong milestone.
like "$roadmap_skill" 'mem save --kind ruling' 'drift the orchestrator absorbs is a ruling'
like "$roadmap_skill" 'mem ask' 'and drift that changes the scope goes back to planning'

# A skill nothing points at is one nobody loads: each lane that can find
# itself holding a roadmap says so where that lane is decided.
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'roadmap' 'route sends work bigger than one plan to roadmap'
like "$plan_skill" 'roadmap' 'plan says when a plan is a roadmap instead'
like "$orchestrate_skill" 'roadmap' 'orchestrate knows a run can be one milestone of one'
like "$(cat "$WF_ROOT/skills/mem/SKILL.md")" 'mem roadmap' 'mem names the verb that reads the milestones'

## ------------------------------------------------------- the fix-round loop

# The gate runs the project's whole suite on integration, not just a task's
# own Verify, so a task that breaks a test outside its Verify has to fix it
# and claim it in Files -- the old wording pointed at the wrong command.
like "$plan_skill" 'No window may be red\.' 'plan says a task fixes any test it breaks, not only its own'
unlike "$plan_skill" 'workflow verify. is what the gate' 'and drops the stale pointer to the wrong command'
like "$plan_skill" 'Several one-line edits of one kind across files are one task' \
	'plan says repeated one-line edits are one task, not one per file'
implement_skill=$(cat "$WF_ROOT/skills/implement/SKILL.md")
like "$implement_skill" 'a literal from the spec' \
	'implement warns against a test that recomputes what the code computes'
# Two fix rounds go by themselves; the third verdict is the orchestrator's,
# who reads the third review and accepts or redispatches, and never sleeps
# on a clock while the run works.
like "$orchestrate_skill" 'rounds go by themselves' \
	'orchestrate knows two fix rounds go by themselves'
like "$orchestrate_skill" 'its third reading is yours' \
	'and a third is the orchestrator to read'
like "$orchestrate_skill" '<task>\.review\.3' 'naming the third review file to read'
like "$orchestrate_skill" 'accept <task>' 'and the accept verb as the way out'
like "$orchestrate_skill" '`\[later\]` finding in' 'a later finding is never ruled in'
like "$orchestrate_skill" 'Never sleep on a clock' 'orchestrate never sleeps on a clock'
like "$orchestrate_skill" 'workflow wait' 'it waits on the run instead'
unlike "$orchestrate_skill" 'every few minutes read' 'and drops the polling wording'

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

# The README spells the skill list out twice by hand -- what skills/ holds,
# and what enable and disable write. A skill missing from either list is one a
# reader has no way to learn is there.
holds=$(grep -A1 'session-facing instructions' "$WF_ROOT/README.md")
writes=$(grep -A1 'write one key' "$WF_ROOT/README.md")
for s in route plan roadmap implement orchestrate review mem unslop; do
	like "$holds" "$s" "the README counts $s among the skills it ships"
	like "$writes" "$s" "and among the ones enable and disable write"
done

## ------------------------------------------------------------ the adapters

# Removed 2026-08-31: other runtimes wire themselves (read the skills, export
# WORKFLOW_AGENT=1); the repo ships no per-runtime instruction files.
truthy "$([ ! -d "$WF_ROOT/adapters" ] && echo 0 || echo 1)" \
	'the adapters directory stays gone'

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
