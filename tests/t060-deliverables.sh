#!/usr/bin/env bash
# The shipped files themselves: skills that name themselves, hooks that
# mandate the one line without which they are ungated, and the command surface.
source "$(dirname -- "$0")/lib.sh"
t_init

## ------------------------------------------------------------- the skills

for s in dogfood fix garden grill lead plan research review route work mem; do
	f="$WF_ROOT/skills/$s/SKILL.md"
	truthy "$([ -f "$f" ] && echo 0 || echo 1)" "skills/$s/SKILL.md exists"
	like "$(head -1 "$f")" '^---$' "skills/$s starts with frontmatter"
	like "$(sed -n '2,4p' "$f")" "name: $s" "skills/$s names itself"
	like "$(sed -n '2,4p' "$f")" 'description: Use ' "skills/$s describes when to use it"
	# The description rides along on every turn, so it stays one short line.
	desc=$(grep -m1 '^description:' "$f")
	truthy "$([ "${#desc}" -lt 200 ] && echo 0 || echo 1)" "skills/$s describes itself in under 200 characters"
done

# Skills point at the subcommands rather than restating what they do.
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'workflow review-needed' 'route defers to review-needed'
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'workflow verify' 'route defers to verify'
plan_skill=$(cat "$WF_ROOT/skills/plan/SKILL.md")
like "$plan_skill" 'workflow plan-check' 'plan defers to the parser'
# The check and the run are different acts, and sending a planner to the run
# is how an unapproved plan once dispatched live workers.
unlike "$plan_skill" 'workflow run --plan-file plan' 'and not to the orchestrator'
work_skill=$(cat "$WF_ROOT/skills/work/SKILL.md")
like "$work_skill" 'workflow verify' 'work defers to verify'
like "$work_skill" 'mem evidence add' 'work attaches the Show evidence'
like "$(cat "$WF_ROOT/skills/lead/SKILL.md")" 'mem answer' 'lead answers a worker through mem'

# The gate counts only evidence filed since the dispatch, keyed by the bare
# task id, so the worker has to know both and which captures count.
like "$work_skill" 'playwright-cli screenshot`, `tmux capture-pane -p`, or a transcript' \
	'work names the three captures'
like "$work_skill" 'the task id in the brief.s GOAL' 'and keys the evidence by the task id'
like "$work_skill" 'counts only evidence filed in this session' 'and says an older capture does not count'
# The engine cuts the steps and reads the walk's outcome off the last status
# line, so the session walks what the brief numbers and reports in its words.
dogfood_skill=$(cat "$WF_ROOT/skills/dogfood/SKILL.md")
like "$dogfood_skill" 'The brief numbers the steps' 'dogfood takes its steps from the brief'
like "$dogfood_skill" 'mem --project workflow wiki dogfood-playbooks#<name>' \
	'and reads a linked playbook section from the workflow project'
like "$dogfood_skill" 'no verify page' 'a missing verify page is written from what worked'
like "$dogfood_skill" 'only the steps the brief names' 'a later walk takes only the named steps'
like "$dogfood_skill" 'off the runner machine' 'a refused wiki write is skipped'
like "$dogfood_skill" 'workflow report ready "failed <n> <n>"' 'and the last act reports the failed steps'
# Inside a milestone the lead appends to the plan of record; in maintenance
# there is none, so it stores one plan the engine runs under the limit.
lead_skill=$(cat "$WF_ROOT/skills/lead/SKILL.md")
like "$lead_skill" 'mem plan fix-<id> --stdin' 'lead stores one fix plan in maintenance'
like "$lead_skill" 'three tasks or fewer' 'and names the limit it runs unasked under'
like "$lead_skill" '"blocking finding #<id>: ' 'and the owner prefix that holds the milestone'
like "$lead_skill" '"finding #<id>: ' 'and the one that lets it tick'
# The walk after the landing is what closes a finding, so a fix ends on
# its Show captured again rather than on a close of its own.
fix_skill=$(cat "$WF_ROOT/skills/fix/SKILL.md")
like "$fix_skill" 'its evidence file' 'fix opens from the finding and its evidence'
like "$fix_skill" 'the engine closes the finding' 'and leaves the close to the engine'

# The plan skill cuts the roadmap and every milestone plan in one sitting,
# so it carries the verbs that check and store both, and the one approval.
like "$plan_skill" 'walking skeleton' 'plan starts the roadmap with the walking skeleton'
like "$plan_skill" 'Show:' 'every milestone carries a Show path'
like "$plan_skill" 'Surface:' 'and a Surface the engine drives it on'
like "$plan_skill" 'workflow plan-check "\$d/roadmap\.md"' 'plan checks the roadmap and its plans at once'
like "$plan_skill" 'mem roadmap --set-file' 'plan stores the milestones'
like "$plan_skill" 'mem plan <slug> --set-file' 'and one plan per milestone beside them'
like "$plan_skill" 'mem plan <slug> --status draft' 'each stored as a draft'
like "$plan_skill" 'plan-summary' 'plan writes the summary page the owner reads'
like "$plan_skill" 'mem ask .*--options approve,changes --recommend approve' \
	'and asks for the one approval with a recommendation'
truthy "$([ "$(wc -c <"$WF_ROOT/skills/plan/SKILL.md")" -lt 9000 ] && echo 0 || echo 1)" \
	'the plan skill stays under 9,000 bytes'

# A review reads and reports; the fix is someone else's. It names no reader
# verb, since the reading is the session itself.
review_skill=$(cat "$WF_ROOT/skills/review/SKILL.md")
like "$review_skill" 'git diff' 'review reads the diff'
like "$review_skill" 'mem wiki <slug>#<section>' 'against the spec section it was named'
like "$review_skill" 'severity, location, evidence, suggestion' 'and files findings in one shape'
like "$review_skill" 'empty review' 'an empty review is an answer'
unlike "$review_skill" 'workflow read' 'review names no reader verb'
truthy "$([ "$(wc -c <"$WF_ROOT/skills/review/SKILL.md")" -lt 1500 ] && echo 0 || echo 1)" \
	'the review skill stays under 1,500 bytes'

# A skill nothing points at is one nobody loads: each lane that can find
# itself holding a roadmap says so where that lane is decided.
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'Hand over to `plan`' 'route hands the plan lane to plan'
unlike "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'roadmap' 'and plan alone, which cuts any roadmap'
like "$plan_skill" 'roadmap' 'plan says when a plan is a roadmap instead'
like "$(cat "$WF_ROOT/skills/mem/SKILL.md")" 'mem roadmap' 'mem names the verb that reads the milestones'

# A numbered decision is a number to cite, and a worker cites it in a
# comment no reader of the repository can follow. The plan gives its
# decisions as sentences with their reasons, and the reason is what lands.
like "$plan_skill" '`## Decisions` section of sentences, each with its reason' \
	'plan asks for decisions with their reasons'
like "$plan_skill" 'never numbered' 'and never numbered'
unlike "$plan_skill" 'numbered `## Rulings`' 'and drops the numbered rulings'
# The hygiene check is what a commit has to pass at the gate, so the
# one-shot and the task loop run it before committing.
for s in work route; do
	skill=$(cat "$WF_ROOT/skills/$s/SKILL.md")
	like "$skill" 'workflow hygiene --staged' "$s runs the hygiene check before a commit"
	unlike "$skill" 'workflow read' "and $s no longer sends the diff to a reader"
done

# A worker stuck on one error consults the advisor Claude Code offers, and
# a task merges on the gate's green suite, so no verb reads or advises.
unlike "$work_skill" 'workflow advise' 'work names no advise verb'
unlike "$(cat "$WF_ROOT/README.md")" 'workflow read' 'README.md names no reader verb'

## ------------------------------------------------------- the fix-round loop

# The gate runs the project's whole suite on integration, not just a task's
# own Verify, so a task that breaks a test outside its Verify has to fix it
# and claim it in Files -- the old wording pointed at the wrong command.
like "$plan_skill" 'the suite is green after every task' 'plan says a task fixes any test it breaks, not only its own'
unlike "$plan_skill" 'workflow verify. is what the gate' 'and drops the stale pointer to the wrong command'
like "$plan_skill" 'Several one-line edits of one kind across files are one task' \
	'plan says repeated one-line edits are one task, not one per file'

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
# A page is read and rewritten a section at a time, so a worker replaces the
# paragraph it falsified and not the page.
like "$mem_skill" 'wiki:<slug>#<section>' 'mem shows the section rows search prints'
like "$mem_skill" 'mem wiki <slug>#<section> ' 'mem shows how one section is read'
like "$mem_skill" 'mem wiki <slug>#<section> --stdin --note' 'and how one section is replaced'
like "$mem_skill" 'mem wiki <slug> --sections' 'and how a page lists its sections'
like "$mem_skill" 'mem wiki lint' 'mem names the wiki lint'
for verb in 'mem decide' 'mem evidence add' 'mem finding add' 'mem raw add' 'mem brief' 'mem idea'; do
	like "$mem_skill" "$verb " "mem teaches $verb"
done
# `mem skill mem` prints the whole file, so it has to stay small.
truthy "$([ "$(wc -c <"$WF_ROOT/skills/mem/SKILL.md")" -lt 6000 ] && echo 0 || echo 1)" \
	'the mem skill stays under 6,000 bytes'
like "$(cat "$WF_ROOT/README.md")" 'mem wiki' 'the README puts the wiki among the reads'

# The README spells out what skills/ holds. The other list is not written down
# any more: `workflow skill` is the listing, read off the binary, and mem
# serves its own. A skill missing from either is one a reader cannot learn of.
holds=$(grep -A1 'session-facing instructions' "$WF_ROOT/README.md")
served=$(workflow skill)
for s in dogfood fix garden grill lead plan research review route work mem; do
	like "$holds" "$s" "the README counts $s among the skills it ships"
done
for s in dogfood fix garden grill lead plan research review route work; do
	like "$served" "^$s — " "and \`workflow skill\` serves $s with its description"
done
# mem's is mem's to serve, so workflow's listing must not claim it.
unlike "$served" '^mem — ' 'and leaves the mem skill to mem'

# The README explains the hygiene gate a commit has to pass, and every knob
# on it, because a refusal names the rule and the README is where the rule
# is read.
gate=$(sed -n '/^## What the gate refuses/,/^## /p' "$WF_ROOT/README.md")
like "$gate" 'hard tier' 'the README names the hard tier of hygiene'
like "$gate" 'soft tier' 'and the soft tier'
like "$gate" 'WORKFLOW_HYGIENE=skip' 'and the override for a human commit'
like "$gate" 'WORKFLOW_AGENT' 'and when the gate ignores that override'
like "$gate" 'mem project set hygiene-exempt' 'and the per-project exemption'
like "$gate" 'core\.excludesFile' 'and the global ignore list'
like "$gate" 'workflow doctor' 'and that doctor checks the list'
like "$gate" 'sentences' 'and that plans carry decisions as sentences'
like "$gate" 'workflow hygiene' 'and the verb that runs the check'

# Serve ticks a milestone only after its Show path walk, and a Show task
# merges only on evidence filed since its dispatch, so the README says how
# a walk is asked for, what serve is doing meanwhile, and how evidence is filed.
serve=$(sed -n '/^## Serve/,/^## Reading/p' "$WF_ROOT/README.md")
like "$serve" 'workflow dogfood \[<project>\] \[--milestone <slug>\]' 'the README names the request verb'
like "$serve" 'dogfood-machine' 'and the key that says where the walk runs'
like "$serve" 'needs-plan' 'and the stage of a milestone with no stored plan'
like "$serve" 'paused, dogfood' 'and the stage of a walk'
like "$(cat "$WF_ROOT/README.md")" 'mem evidence add --task <id>' 'the README says how Show evidence is filed'
# A verb the README names has to be one the binary answers to.
for sub in $(grep -oE '`workflow [a-z][a-z-]*|^ +workflow [a-z][a-z-]*' "$WF_ROOT/README.md" |
	sed -E 's/^[` ]*workflow //' | sort -u); do
	run workflow "$sub" --frob
	unlike "$OUT" "unknown command: $sub" "the README's workflow $sub is a real command"
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
