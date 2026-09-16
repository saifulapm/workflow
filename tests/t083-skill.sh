#!/usr/bin/env bash
# workflow skill: the skills this binary carries, served instead of installed.
#
# A skill used to be a file on disk that every harness discovered for itself,
# which needed a settings file per harness to gate and a doctor check to keep
# in step with the binary. Text served from include_str! cannot disagree with
# the binary serving it, and mem's digest names it where mem knows the project.
source "$(dirname -- "$0")/lib.sh"
t_init

owned='route plan roadmap implement orchestrate review unslop'

## ------------------------------------------------------------- the listing

run workflow skill
is "$RC" 0 'workflow skill exits 0'
for s in $owned; do
	like "$OUT" "^$s — " "it lists $s with its description"
done

# mem's skill is mem's to serve: one skill, one binary that owns it.
unlike "$OUT" '^mem — ' 'it does not list the skill mem owns'

is "$(printf '%s\n' "$OUT" | wc -l)" '7' 'one line per skill and nothing else'
is "$(printf '%s\n' "$OUT" | sort | md5sum)" "$(printf '%s\n' "$OUT" | md5sum)" \
	'in name order'

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
