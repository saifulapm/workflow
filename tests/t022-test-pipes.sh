#!/usr/bin/env bash
# No test pipes a command into grep -q. lib.sh sets pipefail, and grep -q
# quits on its first match: a writer still writing behind it takes SIGPIPE,
# and pipefail reads the match as a miss. Under load that turned t153's
# "listed within 2 s" red on a write its own wait loop had just seen. A
# check reads its text from a here-string instead, as like() does.
source "$(dirname -- "$0")/lib.sh"
t_init

piped=$(grep -nE '(^|[^|])\|[[:space:]]*grep[[:space:]]+-[[:alpha:]]*q' "$WF_ROOT"/tests/*.sh)
is "$piped" "" "no test pipes a command into grep -q"
