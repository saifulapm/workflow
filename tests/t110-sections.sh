#!/usr/bin/env bash
# mem wiki addresses one section of a page: `<slug> --sections` lists them,
# `<slug>#<hslug>` reads one, and the same address with --stdin replaces only
# that section's bytes, which search then finds under the section's own row.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

printf '# Pricing\n\nThe cart totals in cents.\n\n## Rounding\n\nHalf up, once, at the end.\n\n## Refunds\n\nRefunds reverse the rounding.\n' |
	"$MEM_BIN" wiki pricing --stdin --note 'three sections' >/dev/null

run_out "$MEM_BIN" wiki pricing --sections
is "$RC" 0 'the sections list'
is "$OUT" 'top  38  top
rounding  41  Rounding
refunds  42  Refunds' 'one line per section, top first'

run_out "$MEM_BIN" wiki 'pricing#rounding'
is "$RC" 0 'one section reads'
is "$OUT" '## Rounding

Half up, once, at the end.' 'and prints only its own bytes'

run "$MEM_BIN" wiki 'pricing#taxes'
is "$RC" 1 'a section the page lacks is exit 1'
like "$OUT" 'top, rounding, refunds' "naming the page's sections"

run "$MEM_BIN" wiki 'pricing#rounding' --stdin --note 'round half to even' <<'TEXT'
## Rounding

Half to even with a quetzal tiebreak.
TEXT
is "$RC" 0 'one section is replaced'

page=$("$MEM_BIN" wiki pricing)
like "$page" 'quetzal tiebreak' 'the page carries the new text'
unlike "$page" 'Half up' 'and not the old'
like "$page" 'Refunds reverse the rounding' 'and the other sections are untouched'
like "$("$MEM_BIN" log)" 'wiki pricing#rounding: round half to even' 'the write is logged under its section'

run_out "$MEM_BIN" search quetzal
like "$OUT" '^wiki:pricing#rounding  ' 'search finds the new text in its section row'
