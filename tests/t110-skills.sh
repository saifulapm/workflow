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
like "$cloud" 'never for builds, tests or' "orchestrate cloud: a local agent is never for builds, tests or code"
like "$cloud" 'a phone on adb, or iOS on the Mac' "orchestrate cloud: a local agent only for the screen, adb or iOS"
