#!/usr/bin/env bash
# The shipped files themselves: skills inside their budgets, adapters that
# mandate the one line without which they are ungated, and the command surface.
source "$(dirname -- "$0")/lib.sh"
t_init

## ------------------------------------------------------------- the skills

for s in route plan implement review; do
	f="$WF_ROOT/skills/$s/SKILL.md"
	truthy "$([ -f "$f" ] && echo 0 || echo 1)" "skills/$s/SKILL.md exists"
	like "$(head -1 "$f")" '^---$' "skills/$s starts with frontmatter"
	like "$(sed -n '2,4p' "$f")" "name: $s" "skills/$s names itself"
	like "$(sed -n '2,4p' "$f")" 'description: Use ' "skills/$s describes when to use it"
done

# Line ceilings from the spec: route 50, plan 100, review 80.
is "$(($(wc -l <"$WF_ROOT/skills/route/SKILL.md") <= 50))" 1 'route is within 50 lines'
is "$(($(wc -l <"$WF_ROOT/skills/plan/SKILL.md") <= 100))" 1 'plan is within 100 lines'
is "$(($(wc -l <"$WF_ROOT/skills/review/SKILL.md") <= 80))" 1 'review is within 80 lines'

# The byte budgets, checked the way the machine checks them.
run workflow doctor
unlike "$OUT" 'skill .*over the' 'every shipped skill is inside its byte budget'
like "$OUT" 'skill implement .*within budget' 'and doctor saw all four'

# Skills point at the subcommands rather than restating what they do.
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'workflow review-needed' 'route defers to review-needed'
like "$(cat "$WF_ROOT/skills/route/SKILL.md")" 'workflow verify' 'route defers to verify'
like "$(cat "$WF_ROOT/skills/plan/SKILL.md")" 'workflow run --plan-file' 'plan defers to the parser'
like "$(cat "$WF_ROOT/skills/implement/SKILL.md")" 'workflow verify' 'implement defers to verify'
like "$(cat "$WF_ROOT/skills/review/SKILL.md")" 'workflow review-needed' 'review defers to review-needed'

## ------------------------------------------------------------ the adapters

for a in codex pi; do
	f="$WF_ROOT/adapters/$a.md"
	truthy "$([ -f "$f" ] && echo 0 || echo 1)" "adapters/$a.md exists"
	like "$(cat "$f")" 'export WORKFLOW_AGENT=1' "adapters/$a mandates exporting WORKFLOW_AGENT"
	like "$(cat "$f")" 'workflow verify' "adapters/$a routes through the gate"
	like "$(cat "$f")" 'mem ask' "adapters/$a keeps the stop conditions"
done

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
