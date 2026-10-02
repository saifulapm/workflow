#!/usr/bin/env bash
# `mem project set hygiene-exempt "<globs>"`: the paths a project keeps out of
# the hygiene check, matched like Files patterns. mem stores the globs as
# given and hands them to whoever checks; `project current` carries them in
# both forms, `unset` takes them off, and an empty value is refused with a
# hint that shows a glob.
source "$(dirname -- "$0")/lib.sh"
t_init

new_repo app
mem_register

run "$MEM_BIN" project current --json
unlike "$OUT" 'hygiene_exempt' 'a project with no key carries none'

run "$MEM_BIN" project set hygiene-exempt 'tests/** docs/*.md'
is "$RC" 0 'set takes the globs'

run_out "$MEM_BIN" project current --json
is "$(jq -r .hygiene_exempt <<<"$OUT")" 'tests/** docs/*.md' 'the JSON carries them as one string'
run_out "$MEM_BIN" project current
like "$OUT" '^hygiene-exempt  tests/\*\* docs/\*\.md$' 'and the text form names them'

run "$MEM_BIN" project set hygiene-exempt '  '
is "$RC" 2 'an empty value is a usage error'
like "$OUT" 'mem project set hygiene-exempt "[^"]*\*[^"]*"' 'with a glob to copy'
run_out "$MEM_BIN" project current --json
is "$(jq -r .hygiene_exempt <<<"$OUT")" 'tests/** docs/*.md' 'and the stored globs stay'

run "$MEM_BIN" project unset hygiene-exempt
is "$RC" 0 'unset takes them off'
run_out "$MEM_BIN" project current --json
unlike "$OUT" 'hygiene_exempt' 'and the key is gone'
