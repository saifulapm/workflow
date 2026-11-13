#!/bin/sh
# Re-records the headless transcripts here from the real playwright-cli, so a
# CLI upgrade can refresh them. Tests never run this. Each case is recorded as
#   playwright-cli <args> >case.out 2>case.err; echo $? >case.code
# Check the diff after, since tests replay these files byte for byte.

dir=$(cd "$(dirname "$0")" && pwd)
s=m2brec

record() {
  case=$1
  shift
  playwright-cli "$@" >"$dir/$case.out" 2>"$dir/$case.err"
  echo $? >"$dir/$case.code"
}

playwright-cli list >/dev/null 2>&1
record list list

playwright-cli -s=m2blist open about:blank >/dev/null 2>&1
record list1 list
playwright-cli -s=m2blist close >/dev/null 2>&1

record detach_none -s=m2bnone detach

playwright-cli -s=$s open about:blank >/dev/null 2>&1
record result -s=$s run-code 'async page => { return -2 }'
record tabnew_bad -s=$s tab-new https://nope.invalid/
record sessclosed -s=$s run-code 'async page => { await page.context().browser().close(); return 1 }'
playwright-cli -s=$s close >/dev/null 2>&1
