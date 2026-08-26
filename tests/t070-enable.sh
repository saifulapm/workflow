#!/usr/bin/env bash
# workflow enable / disable: which projects see this repo's skills.
#
# Claude Code reads the skill list from its settings files, and a project's
# file outranks the user's. The gate is `disable --global` once; a project
# takes the skills back with `enable`. Nothing else scopes a skill to a
# project, so these two verbs are the whole switch.
source "$(dirname -- "$0")/lib.sh"
t_init

user="$HOME/.claude/settings.json"
skills='route plan implement orchestrate review mem unslop'

## ------------------------------------------------------- the gate, once

run workflow disable --global
is "$RC" 0 'disable --global exits 0'
for s in $skills; do
	is "$(jq -r ".skillOverrides.\"$s\"" "$user")" 'off' "the user file hides $s"
done
like "$OUT" 'are off here' 'and it says what it did'

## ----------------------------------------------- a project that opts in

new_repo app
run workflow enable
is "$RC" 0 'enable exits 0'
proj=$PWD/.claude/settings.json
truthy "$([ -f "$proj" ] && echo 0 || echo 1)" 'it writes the project settings file'
for s in $skills; do
	is "$(jq -r ".skillOverrides.\"$s\"" "$user")" 'off' "the user file still hides $s"
	is "$(jq -r ".skillOverrides.\"$s\"" "$proj")" 'on' "and the project turns $s back on"
done

# The gate is in place, so nothing needs saying about it.
unlike "$OUT" 'does not turn them off' 'with the gate in place it stays quiet'

run workflow enable
is "$RC" 0 'running it again exits 0'
like "$OUT" 'already says all of this' 'and says the file already said it'

# From a subdirectory it is still the project that is enabled, not the
# subdirectory: Claude Code reads .claude/ from the repo root down.
mkdir -p src/deep
cd src/deep || exit 1
run workflow enable
truthy "$([ ! -e "$PWD/.claude" ] && echo 0 || echo 1)" 'a subdirectory writes no settings of its own'
like "$OUT" 'already says all of this' 'it found the toplevel file'
cd "$T_TMP/app" || exit 1

## --------------------------------------------- what else is in the file

# A project's settings file is not this command's file. Whatever else it says
# survives, including other people's skill entries.
cat >"$proj" <<'EOF'
{
  "outputStyle": "Concise",
  "skillOverrides": { "deploy": "off" },
  "permissions": { "allow": ["Bash(ls:*)"] }
}
EOF
run workflow enable
is "$(jq -r '.outputStyle' "$proj")" 'Concise' 'an unrelated key survives'
is "$(jq -r '.permissions.allow[0]' "$proj")" 'Bash(ls:*)' 'and so does an unrelated block'
is "$(jq -r '.skillOverrides.deploy' "$proj")" 'off' "and another skill's entry"
is "$(jq -r '.skillOverrides.route' "$proj")" 'on' 'while route is turned on'

## ------------------------------------------------------- opting back out

run workflow disable
is "$RC" 0 'disable exits 0'
for s in $skills; do
	is "$(jq -r ".skillOverrides.\"$s\"" "$proj")" 'off' "the project now hides $s too"
done
is "$(jq -r '.outputStyle' "$proj")" 'Concise' 'and the rest of the file is still there'

## ------------------------------------------------------------- dry runs

before=$(cat "$proj")
run_out workflow enable --dry-run
is "$RC" 0 'a dry run exits 0'
is "$(printf '%s' "$OUT" | jq -r '.skillOverrides.route')" 'on' 'it prints what it would write'
is "$(cat "$proj")" "$before" 'and writes nothing'

## ------------------------------------- a machine with no gate says so

rm -f "$user"
run workflow enable
like "$OUT" 'does not turn them off' 'without the gate, enable says the file means nothing yet'
like "$OUT" 'workflow disable --global' 'and names the command that sets it up'

## ------------------------------------------------ a file it cannot use

printf 'not json at all\n' >"$proj"
run workflow enable
is "$RC" 1 'a settings file that is not json fails'
like "$OUT" 'fix it by hand' 'and says to fix it by hand'
is "$(cat "$proj")" 'not json at all' 'the file is left exactly as it was'

# The same for a skillOverrides that is not a map: refusing beats taking
# someone else's file with us.
printf '{"skillOverrides": "all"}\n' >"$proj"
run workflow enable
is "$RC" 1 'a skillOverrides that is not a map fails'
is "$(jq -r '.skillOverrides' "$proj")" 'all' 'and that file is untouched too'

## ---------------------------------------------- outside a repo entirely

mkdir -p "$T_TMP/loose"
cd "$T_TMP/loose" || exit 1
run workflow enable
is "$RC" 0 'outside a repo it still writes'
is "$(jq -r '.skillOverrides.mem' "$T_TMP/loose/.claude/settings.json")" 'on' 'in the working directory'
