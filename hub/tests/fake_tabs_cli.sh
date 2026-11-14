#!/bin/sh
# A fake playwright-cli over Chromium's tab strip, kept in $S/tabs, one tab a
# line, tab-separated: id, who opened it (a session, or saiful for one of his
# own), the session whose daemon marked it (0 for none), its URL, and the id
# of the tab that opened it (0 for none).
#
# Like the Playwright extension, a connection sees only the tabs in its
# group. A session that attaches with --extension makes a connection of its
# own; one that attaches to another session's name joins that one's, and
# sees every tab of it ($S/conn-<session> names the connection). Like
# playwright-core, a mark lives in the marking session's daemon, closing the
# current tab makes the one at the same place current, and closing a
# connection's last tab ends it and every session on it with "Session
# closed" and exit 1. Detaching the session that made a connection ends it
# for every session that joined. The hub's run-code is told apart by the
# text of its code.
#
# What the real CLI prints is kept as transcripts in fixtures/playwright-cli
# (<case>.out, .err and .code, recorded by record.sh). The fake replays them
# on the same streams with the same exit code, changing only the parts that
# vary: a session name, a URL, the code run and the value it returned.

FX=$(dirname "$0")/fixtures/playwright-cli
# Copies stdin, swapping the text $1 for $2 and $3 for $4 where each starts.
subst() {
  F1=$1 T1=$2 F2=$3 T2=$4 awk '
    function swap(line,   out) {
      while (line != "") {
        if (ENVIRON["F1"] != "" && index(line, ENVIRON["F1"]) == 1) {
          out = out ENVIRON["T1"]; line = substr(line, length(ENVIRON["F1"]) + 1)
        } else if (ENVIRON["F2"] != "" && index(line, ENVIRON["F2"]) == 1) {
          out = out ENVIRON["T2"]; line = substr(line, length(ENVIRON["F2"]) + 1)
        } else {
          out = out substr(line, 1, 1); line = substr(line, 2)
        }
      }
      return out
    }
    { print swap($0) }'
}
# Prints transcript $1 (stdout, then stderr) with the substitutions $2..$5
# and exits with the code it recorded.
replay() {
  c=$1
  shift
  subst "$@" < "$FX/$c.out"
  subst "$@" < "$FX/$c.err" >&2
  exit "$(cat "$FX/$c.code")"
}

# One call at a time, as one browser answers them, so two sessions' calls
# never write the strip over each other.
exec 9>> "$S/lock"
flock 9
printf '%s\n' "$*" >> "$S/record"
touch "$S/sessions"
if [ "$1" = list ]; then
  if [ -s "$S/sessions" ]; then
    head -n 1 "$FX/list1.out"
    while read -r s; do tail -n +2 "$FX/list1.out" | subst m2blist "$s"; done < "$S/sessions"
    exit "$(cat "$FX/list1.code")"
  fi
  replay list
fi
session=${1#-s=}
cmd=$2
shift 2
code=$1
cur=$(cat "$S/current-$session" 2>/dev/null)
conn=$(cat "$S/conn-$session" 2>/dev/null)

field() { awk -F'\t' -v id="$1" -v f="$2" '$1 == id { print $f }' "$S/tabs"; }
set_field() {
  awk -F'\t' -v OFS='\t' -v id="$1" -v f="$2" -v v="$3" '$1 == id { $f = v } 1' "$S/tabs" > "$S/t"
  mv "$S/t" "$S/tabs"
}
drop() {
  awk -F'\t' -v id="$1" '$1 != id' "$S/tabs" > "$S/t"
  mv "$S/t" "$S/tabs"
}
attached() { grep -qx "$1" "$S/sessions"; }
# The tabs of connection $1: those opened by a session on it, or dragged
# into its group.
of_conn() {
  for f in "$S"/conn-*; do
    [ -f "$f" ] && printf '%s\t%s\n' "${f##*/conn-}" "$(cat "$f")"
  done | awk -F'\t' -v c="$1" 'NR == FNR { conn[$1] = $2; next }
    { k = ($2 in conn) ? conn[$2] : $2; if (k == c) print $1 }' - "$S/tabs"
}
visible() { of_conn "$conn"; }
nth() { visible | sed -n "$(($1 + 1))p"; }
# A tab this session's daemon marked, or, when the hub's code asks for
# openers, one such a tab opened.
own() {
  t=$1
  while [ -n "$t" ] && [ "$t" != 0 ]; do
    [ "$(field "$t" 3)" = "$session" ] && return 0
    [ -n "$openers" ] || return 1
    t=$(field "$t" 5)
  done
  return 1
}
result() { replay result 'async page => { return -2 }' "$code" -2 "$1"; }
# The connection's tabs as playwright-core lists them, every page having an
# empty title.
tab_lines() {
  i=0
  for id in $(visible); do
    mark=
    [ "$id" = "$(cat "$S/current-$session")" ] && mark=" (current)"
    printf -- '- %s:%s [](%s)\n' "$i" "$mark" "$(field "$id" 4)"
    i=$((i + 1))
  done
}
tab_result() { echo '### Result'; tab_lines; }
# Ends every session on connection $1.
end_conn() {
  while read -r s; do
    [ "$(cat "$S/conn-$s" 2>/dev/null)" = "$1" ] || echo "$s"
  done < "$S/sessions" > "$S/t"
  mv "$S/t" "$S/sessions"
}
end_session() {
  end_conn "$conn"
  replay sessclosed
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

if [ "$cmd" != attach ] && [ "$cmd" != detach ] && ! attached "$session"; then
  replay notopen m2bjoin "$session"
fi

case $cmd in
attach)
  # A new daemon knows no mark of the session's last one.
  awk -F'\t' -v OFS='\t' -v s="$session" '$3 == s { $3 = 0 } 1' "$S/tabs" > "$S/t"
  mv "$S/t" "$S/tabs"
  rm -f "$S/seen-$session"
  if [ "$1" = "--extension=chromium" ]; then
    echo "$session" > "$S/conn-$session"
    conn=$session
    if [ -n "$PLAYWRIGHT_MCP_EXTENSION_TOKEN" ]; then
      new_tab about:blank
    else
      # With no token Saiful picks one of his own tabs on the connect page.
      his=$(awk -F'\t' '$2 == "saiful" { print $1; exit }' "$S/tabs")
      set_field "$his" 2 "$session"
      echo "$his" > "$S/current-$session"
    fi
    echo "$session" >> "$S/sessions"
    echo attached
  else
    if ! attached "$1"; then
      echo "[PlaywrightError: connect ENOENT $1] { log: [], code: 'ENOENT' }" >&2
      exit 1
    fi
    cat "$S/conn-$1" > "$S/conn-$session"
    conn=$(cat "$S/conn-$session")
    visible | head -n 1 > "$S/current-$session"
    echo "$session" >> "$S/sessions"
    replay join m2bjoin "$session" m2brec "$1"
  fi
  ;;
tab-new)
  if [ "$1" = about:blank ]; then
    # What the probe saw through the extension: the new page closes, and
    # playwright-core makes the connection's last tab current.
    visible | tail -n 1 > "$S/current-$session"
    printf '### Error\nTarget page, context or browser has been closed\n'
    exit 1
  fi
  new_tab "${1:-about:blank}"
  case $1 in *.invalid*) replay tabnew_bad https://nope.invalid/ "$1" ;; esac
  tab_result
  ;;
goto)
  set_field "$cur" 4 "$1"
  # playwright-core lists the open tabs once there is more than one.
  if [ "$(visible | wc -l)" -gt 1 ]; then
    echo '### Open tabs'
    tab_lines
  fi
  echo '### Page'
  echo "- Page URL: $1"
  ;;
tab-list) tab_result ;;
click)
  if [ "$1" = close-me ]; then
    # The page closes itself, and playwright-core makes the connection's
    # last tab current and reports that page.
    drop "$cur"
    visible | tail -n 1 > "$S/current-$session"
    echo '### Page'
    echo "- Page URL: $(field "$(cat "$S/current-$session")" 4)"
    echo '### Snapshot'
    echo '- [Snapshot](.playwright-cli/page-x.yml)'
  else
    echo "ran click $*"
  fi
  ;;
tab-select) nth "$1" > "$S/current-$session"; tab_result ;;
tab-close)
  if [ -n "$1" ]; then close_tab "$(nth "$1")"; else close_tab "$cur"; fi
  tab_result
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
  *'__hubSeen = true'*)
    visible > "$S/seen-$session"
    ;;
  *'!page.__hubSeen'*)
    grep -qx "$cur" "$S/seen-$session" 2>/dev/null || set_field "$cur" 3 "$session"
    ;;
  *'title()'*)
    json= i=0
    for id in $(visible); do
      if own "$id"; then
        c=false
        [ "$id" = "$cur" ] && c=true
        json="$json${json:+,}{\"i\":$i,\"current\":$c,\"title\":\"\",\"url\":\"$(field "$id" 4)\"}"
      fi
      i=$((i + 1))
    done
    result "[$json]"
    ;;
  *) echo "fake: unknown code: $1" >&2; exit 1 ;;
  esac
  ;;
detach)
  if attached "$session"; then
    grep -vx "$session" "$S/sessions" > "$S/t"
    mv "$S/t" "$S/sessions"
    # The session that made the connection takes it down with it.
    [ "$conn" = "$session" ] && end_conn "$session"
    replay detach_join m2bjoin "$session"
  else
    replay detach_none m2bnone "$session"
  fi
  ;;
*) echo "ran $cmd $*" ;;
esac
