#!/usr/bin/env bash
# Every skill and agent the workflow ships keeps its description under 200
# characters. A harness carries each description on every turn, and a long
# one tends to restate the body's steps, which an agent then follows instead
# of the body.
source "$(dirname -- "$0")/lib.sh"
t_init

for file in "$WF_ROOT"/skills/*/SKILL.md "$WF_ROOT"/agents/*.md; do
	name=${file#"$WF_ROOT"/}
	description=$(sed -n 's/^description: //p' "$file" | head -1)
	isnt "${#description}" 0 "$name: has a description"
	if [ "${#description}" -lt 200 ]; then length=short; else length="${#description} characters"; fi
	is "$length" short "$name: its description is under 200 characters"
done

# After a compaction, Claude Code attaches each skill again cut at 20,000
# characters, so a rule past that point is lost for the rest of the session.
for file in "$WF_ROOT"/skills/*/SKILL.md; do
	size=$(wc -c < "$file")
	if [ "$size" -lt 20000 ]; then fits=yes; else fits="$size bytes"; fi
	is "$fits" yes "${file#"$WF_ROOT"/}: under 20,000 bytes"
done

# A cloud session reads its rules from the file the cloud prompt names, so
# that file ships in the orchestrate skill's folder.
named=$(grep -o 'the [^ ]* file in its folder' "$WF_ROOT/workflow/src/cloud-prompt.txt" | cut -d' ' -f2)
isnt "$named" "" "cloud prompt: names the orchestrate file it follows"
if [ -f "$WF_ROOT/skills/orchestrate/$named" ]; then shipped=yes; else shipped=missing; fi
is "$shipped" yes "cloud prompt: skills/orchestrate/$named ships"

# A question only Saiful may answer says so, so a route that lets another
# answerer take a person's questions never takes these.
skill() { cat "$WF_ROOT/skills/$1/SKILL.md"; }
like "$(skill plan | grep 'Review the <name> roadmap')" 'mem ask --for saiful' \
	"plan: the roadmap review is Saiful's"
like "$(skill grill | grep 'mem ask')" 'mem ask --for saiful' "grill: its questions are Saiful's"
# A fixed summary slug overwrote the 2026-10-15 grill's summary on 2026-11-18.
like "$(skill grill | sed -n '/^## 6\./,/^## 7\./p')" 'grilling-summary-<topic>' \
	"grill: each grill's summary page is named after its topic"
like "$(skill grill | sed -n '/^## 7\./,$p')" 'spec-<topic>' \
	"grill: a later grill's spec is named after its topic"
like "$(skill grill | sed -n '/^## 1\./,/^## 2\./p')" 'research-summary-<topic>' \
	"grill: a later grill's research summary is named after its topic"
like "$(skill plan | sed -n '/^## 1\./,/^## 2\./p')" 'spec-<topic>' \
	"plan: it reads the spec page the grill named"
like "$(skill orchestrate | sed -n '/^Saiful.s, asked without waiting/,/^These are/p')" \
	'mem ask --for saiful' "orchestrate: push, spend, secrets and taste are Saiful's"

# Agents guessed these mem commands 100 times in two weeks
# (audit-mem-2026-11-05); the mem skill names the command to use for each.
for guess in 'mem finding show' 'mem plan show' 'mem list' 'mem show handoff' \
	'mem handoff --show' 'mem questions --all'; do
	like "$(skill mem | grep -F "| \`$guess")" '| `mem ' "mem: names the command for $guess"
done

# 607 ideas were written in two weeks and about 26 read: planning reads them.
like "$(skill plan | sed -n '/^## 1. Read/,/^## 2/p')" 'mem search --kind idea' \
	"plan: the Read step lists the project's ideas"

# A Claude Project's milestone runs in the cloud from start to landing: a
# task for Saiful's hands is a question with done and hold, never a park for
# a device. A device still takes up an old local-only handoff from the
# reviewed, walked work it names.
rules=$(skill plan | sed -n '/^## 4/,/^## 5/p')
like "$rules" 'Runs on:' "plan: a Claude Project's plan says where it runs"
like "$rules" 'options are `done` and `hold`' "plan: a task for his hands is a question"
like "$(cat "$WF_ROOT/skills/orchestrate/cloud.md")" 'Never park for a device' \
	"orchestrate cloud: it never parks for a device"
like "$(skill orchestrate | sed -n '/half done/,/^$/p')" 'review and walk done at' \
	"orchestrate: a device leg starts from the reviewed, walked work"

# A plan's acceptance line that already held on the base built nothing, twice
# (workflow m2b-clean-commits and m3-ask-not-park): rule 10 drops such a line.
like "$(skill plan | sed -n '/^10\. /,/^Store each plan/p')" 'already passes on the base names nothing' \
	"plan: rule 10 drops an acceptance line the base already passes"

# A plan promised a number counted in the code, not on production, three
# times (lareys-trendsi m7, m15 and m21): rule 11 has each number name where
# it was counted.
like "$(skill plan | sed -n '/^11\. /,/^Store each plan/p')" 'names where it was counted' \
	"plan: rule 11 has each number a plan promises name where it was counted"

# Nothing showed whether a thread's hooks ran (retro-fixes, #R5JGX4HC): the
# kit's SessionStart writes `hooks ran: <session> in <project>`, and a
# thread's first step looks for its own line, filing a finding when it is
# missing, never a question.
first=$(awk '/^- /{n++} n==1' "$WF_ROOT/skills/orchestrate/cloud.md")
like "$first" 'mem log --project workflow' "orchestrate cloud: the first step reads workflow's log"
like "$first" '--limit 200 [|] grep -qF' "orchestrate cloud: it reads past the default 20 entries"
like "$first" "timeout 60 sh -c 'until mem log" "orchestrate cloud: it waits a minute for the line the hook writes in the background"
like "$first" 'hooks ran: [$]CLAUDE_CODE_REMOTE_SESSION_ID ' "orchestrate cloud: it looks for its own session's hooks line"
like "$first" 'mem finding add --project workflow --milestone hooks-in-threads --step 0 "hooks did not run in thread [$]CLAUDE_CODE_REMOTE_SESSION_ID"' \
	"orchestrate cloud: a missing line files a hooks-in-threads finding"
unlike "$first" 'mem ask' "orchestrate cloud: a missing hooks line asks nothing"

# ebdfy-ui m27's thread sat 82 minutes on a step the safety check blocked,
# with no handoff: it asks naming the command, and parks before its turn ends.
blocked=$(sed -n '/safety check blocks a step/,/^- /p' "$WF_ROOT/skills/orchestrate/cloud.md")
like "$blocked" 'naming the exact command' "orchestrate cloud: a blocked step is asked naming its command"
like "$blocked" 'parked: blocked: <step> [(]#<id>[)]' "orchestrate cloud: a turn that ends on a blocked step parks first"

# His hub types his answer into the thread, since the safety check never
# reads what wait-answer prints (g2-answer-typed): the thread takes it as his.
relayed=$(sed -n '/starts `Saiful answered your question`/,/^- /p' "$WF_ROOT/skills/orchestrate/cloud.md")
like "$relayed" 'is his word' "orchestrate cloud: an answer his hub typed is his word"
like "$relayed" 'as on a line he typed' "orchestrate cloud: it is acted on as a line he typed"
like "$relayed" 'once, not once for each' "orchestrate cloud: the step an answer clears runs once"

# A thread asks before each of the six destructive kinds (#NHEQRXY1), files a
# held line with its exact text and sends it again with the yes, and starts a
# local agent only for what the cloud cannot do (m4-device-tmux).
cloud=$(cat "$WF_ROOT/skills/orchestrate/cloud.md")
for kind in 'a delete or an overwrite of something that exists' 'making something public' \
	"a change to a token or to a person's" 'anything that costs money' \
	'a forced push or a moved' 'a command whose effect you cannot state'; do
	like "$cloud" "$kind" "orchestrate cloud: asks before $kind"
done
like "$(sed -n '/^- Before any of these six/,/^- /p' "$WF_ROOT/skills/orchestrate/cloud.md")" \
	'mem ask --for saiful' "orchestrate cloud: the six kinds are asked with mem ask"
held=$(sed -n '/prints `ask: <line>`/,/^- /p' "$WF_ROOT/skills/orchestrate/cloud.md")
like "$held" 'exact line' "orchestrate cloud: a held line is asked with its exact text"
like "$held" 'device tmux send --yes <id>' "orchestrate cloud: a yes sends the held line again"
like "$held" 'tried once more, then' "orchestrate cloud: another failure is tried once, then asked"
like "$held" 'Only a yes he gives on the' "orchestrate cloud: a yes typed in the thread does not release a held line"
like "$held" 'asked: #<id>' "orchestrate cloud: the device asks a held line's question and names it"
unlike "$held" 'mem ask' "orchestrate cloud: a thread never asks a held line's question itself"
wall=$(sed -n '/^- The tmux session runs behind a wall/,/^- /p' "$WF_ROOT/skills/orchestrate/cloud.md")
like "$wall" 'send it again with `device tmux send --no-wall' "orchestrate cloud: a line the wall cannot serve is asked with --no-wall"
like "$wall" 'Never work around the wall' "orchestrate cloud: never work around the wall"
like "$wall" 'the wall cannot start on <machine>' "orchestrate cloud: a wall that cannot start is a question"
like "$cloud" 'never for builds, tests or' "orchestrate cloud: a local agent is never for builds, tests or code"
like "$cloud" 'a phone on adb, or iOS on the Mac' "orchestrate cloud: a local agent only for the screen, adb or iOS"

# device run is gone (m5-device-deploy): the device's programs and an app's
# staging deploy go through device tmux, in the deploy worktree, and a
# locked pass is asked of Saiful.
dogfood=$(skill dogfood)
for page in "$dogfood" "$cloud" "$(skill plan)"; do
	unlike "$page" '`device run[` ]' "skills: no page names device run"
done
for tool in adb podman gh cf; do
	like "$(sed -n '/^- Terminal work on the device/,/^- /p' "$WF_ROOT/skills/orchestrate/cloud.md")" "$tool" \
		"orchestrate cloud: $tool goes through device tmux"
	like "$dogfood" "$tool" "dogfood: $tool goes through device tmux"
done
like "$dogfood" "device tmux send 'workflow shopify deploy <app>'" \
	"dogfood: an app's staging deploy is typed into device tmux"
like "$dogfood" 'device browser goto <url>' "dogfood: the walk opens the walk line's url"
like "$dogfood" 'end with `playwright-cli -s=<name> detach`' "dogfood: a walk in his Chromium ends by detaching"
like "$(sed -n '/api.presence/,/^## /p' <<<"$dogfood")" 'workflow walk-account' \
	"dogfood: the presence check reads the claude.ai account before a step there"
like "$(skill orchestrate | sed -n '/^## 6/,/^## 7/p')" 'run `workflow walk-account` and name the account it prints in both' \
	"orchestrate: a walk's question and brief name the account its Chromium holds"
like "$cloud" "device tmux send 'workflow shopify deploy <app>'" \
	"orchestrate cloud: an app's staging deploy is typed into device tmux"
like "$cloud" '~/.cache/workflow-cloud/<repo>' "orchestrate cloud: tmux works in the deploy worktree"
like "$cloud" 'device with-secrets <project> -- <command>' "orchestrate cloud: a theme's secrets come through with-secrets"
like "$(sed -n '/pass is locked on/,/^- /p' "$WF_ROOT/skills/orchestrate/cloud.md")" 'unlock' \
	"orchestrate cloud: a locked pass is asked of Saiful"
like "$cloud" 'after a later push run `device' "orchestrate cloud: a later push starts a new tmux session"
# r3-walk-line: land refuses code its walk did not see with `land: refused:`,
# which a thread clears itself, never by asking Saiful.
prompt=$(tr '\n' ' ' < "$WF_ROOT/workflow/src/cloud-prompt.txt")
refusal=$(sed -n '/^  - A line starting `land: refused:`/,/^  - /p' "$WF_ROOT/skills/orchestrate/cloud.md" | tr '\n' ' ' | tr -s ' ')
for text in prompt refusal; do
	like "${!text}" 'land: refused:' "$text: names land's refusal"
	like "${!text}" 'walked <milestone> at <sha>' "$text: a refusal is cleared by a re-walk's line"
	like "${!text}" 'tests-only after <sha>: <why>' "$text: or by the tests-only line"
	like "${!text}" 'never a question for Saiful' "$text: a refusal is never asked"
done
# r3-walk-line: land reads the walk line, so a passing walk ends with it.
like "$(sed -n '/^## 6\./,$p' <<<"$dogfood")" 'mem log "walked <milestone> at <sha>"' \
	"dogfood: a passing walk ends with its walk line"
six=$(skill orchestrate | sed -n '/^## 6/,/^## 7/p')
like "$six" 'no walk: <why>' "orchestrate: a milestone with nothing to open logs no walk"
like "$six" 'walked <milestone> at <sha>' "orchestrate: a walk it runs itself ends with the walk line"
