#!/bin/sh
# A fake amx. It adds each call's words to $S/record, one line a call with
# every word in brackets, and answers from what a real amx printed, kept in
# fixtures/amx as <case>.out, .err and .code. Its one agent is rec-t8a; its
# ls also lists rec-t8b, an agent of Saiful's own. While $S/result-times-out
# is there, result answers as amx does when its --timeout runs out.

FX=$(dirname "$0")/fixtures/amx
replay() {
  cat "$FX/$1.out"
  cat "$FX/$1.err" >&2
  exit "$(cat "$FX/$1.code")"
}

printf '[%s]' "$@" >> "$S/record"
echo >> "$S/record"
case $1 in
new | ls | status | logs | stop) replay "$1" ;;
result)
  if [ -f "$S/result-times-out" ]; then replay result-short; fi
  replay result
  ;;
# No transcript of these was recorded, and the hub reads neither's output.
send | answer) ;;
*)
  echo "fake amx: no transcript for $1" >&2
  exit 1
  ;;
esac
