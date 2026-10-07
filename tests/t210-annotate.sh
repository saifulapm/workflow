#!/usr/bin/env bash
# hub/assets/annotate.js, the comment layer a designed plan page gets: the
# script parses as a classic script, and its anchor, quote and unopened-count
# pieces pass their node tests.
source "$(dirname -- "$0")/lib.sh"
t_init

run node --check "$WF_ROOT/hub/assets/annotate.js"
if [ "$RC" -eq 0 ]; then ok 'annotate.js parses'; else notok 'annotate.js parses' "$OUT"; fi

run node --test "$WF_ROOT/hub/tests/annotate.test.mjs"
if [ "$RC" -eq 0 ]; then ok 'annotate.js node tests pass'; else notok 'annotate.js node tests pass' "$OUT"; fi
