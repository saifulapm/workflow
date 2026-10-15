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

# A question only Saiful may answer says so, so a route that lets another
# answerer take a person's questions never takes these.
skill() { cat "$WF_ROOT/skills/$1/SKILL.md"; }
like "$(skill plan | grep 'Review the <name> roadmap')" 'mem ask --for saiful' \
	"plan: the roadmap review is Saiful's"
like "$(skill grill | grep 'mem ask')" 'mem ask --for saiful' "grill: its questions are Saiful's"
like "$(skill orchestrate | sed -n '/^Saiful.s, asked without waiting/,/^These are/p')" \
	'mem ask --for saiful' "orchestrate: push, spend, secrets and taste are Saiful's"
