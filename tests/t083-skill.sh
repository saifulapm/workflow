#!/usr/bin/env bash
# workflow skill: the skills this binary carries, read out of the binary.
#
# doctor --fix writes the same text to disk for the harnesses that list skills
# from files; this verb is what mem's digest and doctor itself read, so the
# list here is the list everywhere.
source "$(dirname -- "$0")/lib.sh"
t_init

owned='dogfood fix garden grill lead plan research review route work'

## ------------------------------------------------------------- the listing

run workflow skill
is "$RC" 0 'workflow skill exits 0'
is "$(printf '%s\n' "$OUT" | sed 's/ — .*//' | tr '\n' ' ')" "$owned " \
	'it lists the ten skills in name order'
is "$(printf '%s\n' "$OUT" | wc -l)" '10' 'one line per skill and nothing else'

# mem's skill is mem's to serve: one skill, one binary that owns it.
unlike "$OUT" '^mem — ' 'it does not list the skill mem owns'

# The description is what a harness shows on every turn, so each skill
# carries one, on one line, short.
for s in $owned; do
	run workflow skill "$s"
	is "$RC" 0 "workflow skill $s exits 0"
	desc=$(printf '%s\n' "$OUT" | grep '^description:')
	is "$(printf '%s\n' "$desc" | grep -c .)" '1' "$s has one description line"
	truthy "$([ -n "$desc" ] && [ "${#desc}" -lt 200 ] && echo 0 || echo 1)" \
		"$s describes itself in under 200 characters"
done

## ----------------------------------------------------------- one skill whole

run workflow skill route
is "$RC" 0 'workflow skill route exits 0'
like "$OUT" 'name: route' 'it prints the frontmatter'
like "$OUT" 'Two lanes' 'and the body'
truthy "$([ "$(printf '%s' "$OUT" | wc -c)" -gt 1000 ] && echo 0 || echo 1)" \
	'the whole file, not the description'

## -------------------------------------------------------------- a bad name

run workflow skill nosuchskill
is "$RC" 1 'an unknown name exits 1'
like "$OUT" 'nosuchskill' 'and names what it could not find'

# mem is a name workflow knows about and does not own, so it says so rather
# than pretending the skill does not exist.
run workflow skill mem
is "$RC" 1 'the skill mem owns is not workflow_s to serve'
like "$OUT" 'mem skill mem' 'and it says who to ask'

## ------------------------------------------------ it touches nothing at all

# Ruling 5: mem's digest shells out to `workflow skill`, so if this ever called
# mem back the two binaries would recurse. No store, no git, no mem.
cd "$T_TMP" || exit 1
run env WORKFLOW_MEM=/nonexistent/mem workflow skill
is "$RC" 0 'it runs with no mem on the path'
like "$OUT" '^route — ' 'and still lists the skills'

run env HOME=/nonexistent workflow skill route
is "$RC" 0 'and with no home directory to read'

t_done
