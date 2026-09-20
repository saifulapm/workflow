#!/usr/bin/env bash
# `workflow docs <library> "<query>"`: the docs channel a worker had none of
# (m1-lessons ruling 8). It runs the Context7 CLI twice -- `library` for the
# first id, `docs` for the text -- and prints the text as it came; nothing
# matched, or the CLI failing, is exit 1 with the reason on stderr. A fake
# npx on PATH plays the CLI.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
write_exec "$T_TMP/bin/npx" <<'NPX'
#!/bin/sh
printf '%s\n' "$*" >>"$WF_TMP/npx.argv"
# npx -y ctx7@latest <verb> ...
shift 2
verb=$1
shift
case $verb in
library)
	case $1 in
	nothing) printf 'No libraries found.\n'; exit 0 ;;
	broken) echo 'Error: network is down' >&2; exit 1 ;;
	esac
	printf '1. Title: Pothos GraphQL\n   Context7-compatible library ID: /hayes/pothos\n   Description: a schema builder\n\n2. Title: Other\n   Context7-compatible library ID: /other/pothos\n'
	;;
docs)
	printf 'id=%s query=%s\n' "$1" "$2" >>"$WF_TMP/docs.calls"
	printf '## Relay plugin\n\nbuilder.node(...) takes a ref.\n'
	;;
esac
NPX

new_repo app
mem_register

run workflow docs pothos 'relay plugin node'
is "$RC" 0 'a library that matches prints its docs'
like "$OUT" 'builder\.node\(\.\.\.\) takes a ref\.' 'the text as the CLI gave it'
like "$OUT" 'workflow: docs: /hayes/pothos' 'naming the id it read, on stderr'
is "$(sed -n 1p "$WF_TMP/npx.argv")" '-y ctx7@latest library pothos relay plugin node' 'library first, with the query'
is "$(sed -n 2p "$WF_TMP/npx.argv")" '-y ctx7@latest docs /hayes/pothos relay plugin node' 'then docs on the first id, with the same query'

run workflow docs nothing 'anything'
is "$RC" 1 'no match is exit 1'
like "$OUT" "docs: no library matched 'nothing' -- try another name" 'saying so'
is "$(grep -c . "$WF_TMP/docs.calls")" 1 'and docs was not asked'

run workflow docs broken 'anything'
is "$RC" 1 'a CLI failure is exit 1'
like "$OUT" 'docs: Error: network is down' 'with its stderr'

run workflow docs pothos ''
is "$RC" 2 'an empty query is usage'

t_done
