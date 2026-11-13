#!/bin/sh
# A fake playwright-cli over Chromium's tab strip, kept in $S/tabs, one tab a
# line, tab-separated: id, the session whose group holds it (saiful for
# none), 1 when the hub marked it, its URL, and the id of the tab that opened
# it (0 for none). Like the Playwright extension, a session sees only the
# tabs in its group. Like playwright-core, closing the current tab makes the
# one at the same place current, and closing a session's last tab ends it
# with "Session closed" and exit 1. The hub's run-code is told apart by the
# text of its code.

printf '%s\n' "$*" >> "$S/record"
if [ "$1" = list ]; then
  echo '### Browsers'
  touch "$S/sessions"
  while read -r s; do echo "- $s:"; done < "$S/sessions"
  exit 0
fi
session=${1#-s=}
cmd=$2
shift 2
cur=$(cat "$S/current-$session" 2>/dev/null)

field() { awk -F'\t' -v id="$1" -v f="$2" '$1 == id { print $f }' "$S/tabs"; }
set_field() {
  awk -F'\t' -v OFS='\t' -v id="$1" -v f="$2" -v v="$3" '$1 == id { $f = v } 1' "$S/tabs" > "$S/t"
  mv "$S/t" "$S/tabs"
}
drop() {
  awk -F'\t' -v id="$1" '$1 != id' "$S/tabs" > "$S/t"
  mv "$S/t" "$S/tabs"
}
visible() { awk -F'\t' -v s="$session" '$2 == s { print $1 }' "$S/tabs"; }
nth() { visible | sed -n "$(($1 + 1))p"; }
# A tab the hub marked, or, when the hub's code asks for openers, one a
# marked tab opened.
own() {
  t=$1
  while [ -n "$t" ] && [ "$t" != 0 ]; do
    [ "$(field "$t" 3)" = 1 ] && return 0
    [ -n "$openers" ] || return 1
    t=$(field "$t" 5)
  done
  return 1
}
result() { echo '### Result'; echo "$1"; }
end_session() {
  grep -vx "$session" "$S/sessions" > "$S/t"
  mv "$S/t" "$S/sessions"
  echo 'Session closed' >&2
  exit 1
}
new_tab() {
  id=$(cat "$S/next")
  echo $((id + 1)) > "$S/next"
  printf '%s\t%s\t0\t%s\t0\n' "$id" "$session" "$1" >> "$S/tabs"
  echo "$id" > "$S/current-$session"
}
close_tab() {
  pos=$(visible | grep -nx "$1" | cut -d: -f1)
  drop "$1"
  n=$(visible | wc -l)
  [ "$n" -eq 0 ] && end_session
  if [ "$1" = "$cur" ]; then
    [ "$pos" -gt "$n" ] && pos=$n
    visible | sed -n "${pos}p" > "$S/current-$session"
  fi
}

case $cmd in
attach)
  echo "$session" >> "$S/sessions"
  if [ -n "$PLAYWRIGHT_MCP_EXTENSION_TOKEN" ]; then
    new_tab about:blank
  else
    # With no token Saiful picks one of his own tabs on the connect page.
    his=$(awk -F'\t' '$2 == "saiful" { print $1; exit }' "$S/tabs")
    set_field "$his" 2 "$session"
    echo "$his" > "$S/current-$session"
  fi
  echo attached
  ;;
tab-new)
  new_tab "${1:-about:blank}"
  case $1 in *.invalid*) echo "Error: net::ERR_NAME_NOT_RESOLVED at $1" >&2; exit 1 ;; esac
  echo opened
  ;;
goto) set_field "$cur" 4 "$1"; echo "- Page URL: $1" ;;
tab-select) nth "$1" > "$S/current-$session"; echo selected ;;
tab-close)
  if [ -n "$1" ]; then close_tab "$(nth "$1")"; else close_tab "$cur"; fi
  echo closed
  ;;
run-code)
  case $1 in *'opener()'*) openers=1 ;; *) openers= ;; esac
  case $1 in
  *'.close()'*)
    if [ -f "$S/close-fails" ]; then
      echo 'Error: a modal dialog is open' >&2
      exit 1
    fi
    mine=
    for id in $(visible); do own "$id" && mine="$mine $id"; done
    for id in $mine; do drop "$id"; done
    [ -z "$(visible)" ] && end_session
    result "$(echo $mine | wc -w)"
    ;;
  *'return -2'*)
    if own "$cur"; then result -1; exit 0; fi
    i=0
    for id in $(visible); do
      if own "$id"; then result "$i"; exit 0; fi
      i=$((i + 1))
    done
    result -2
    ;;
  *'pages()['*)
    i=$(printf '%s' "$1" | sed 's/.*pages()\[\([0-9]*\)\].*/\1/')
    id=$(nth "$i")
    if [ -n "$id" ] && own "$id"; then result true; else result false; fi
    ;;
  *'own(page)'*) if own "$cur"; then result true; else result false; fi ;;
  *'__hubTab = true'*) set_field "$cur" 3 1 ;;
  *) echo "fake: unknown code: $1" >&2; exit 1 ;;
  esac
  ;;
detach)
  if grep -qx "$session" "$S/sessions"; then
    grep -vx "$session" "$S/sessions" > "$S/t"
    mv "$S/t" "$S/sessions"
    echo detached
  else
    echo "Browser '$session' is not attached."
  fi
  ;;
*) echo "ran $cmd $*" ;;
esac
