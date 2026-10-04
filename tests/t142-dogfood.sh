#!/usr/bin/env bash
# A landed milestone with a Show line is walked before it is ticked: serve
# starts one dogfood session in the checkout whose brief carries the Show
# path cut into numbered steps, the verify page's sections, the playbook for
# the milestone's Surface and the project's dev key. The stage reads dogfood,
# the milestone stays open, and nothing else starts while the walk stands.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The product: one page on a port of its own.
mkdir -p "$T_TMP/site"
printf '<h1>Hello</h1>\n<footer>2026</footer>\n' >"$T_TMP/site/index.html"
python3 - "$T_TMP/site" "$T_TMP/port" 2>/dev/null <<'PY' &
import functools, http.server, os, sys
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1])
server = http.server.HTTPServer(("127.0.0.1", 0), handler)
with open(sys.argv[2] + ".tmp", "w") as f:
    f.write(str(server.server_address[1]))
os.rename(sys.argv[2] + ".tmp", sys.argv[2])
server.serve_forever()
PY
site_pid=$!
for _ in $(seq 50); do [ -s "$T_TMP/port" ] && break; sleep 0.1; done
url="http://127.0.0.1:$(cat "$T_TMP/port")/"

# The fake session: every start is logged by what it is. The walk keeps its
# brief, its environment and its directory, and fetches the product from the
# dev key; a lead does nothing; a task commits its file.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	printf 'dogfood\n' >>"$WF_TMP/sessions.log"
	cp "$brief" "$WF_TMP/walk.md"
	env >"$WF_TMP/walk.env"
	pwd >"$WF_TMP/walk.pwd"
	curl -s "$(sed -n 's/^dev: //p' "$brief")" >"$WF_TMP/product.html"
	done_json
	exit 0
	;;
*'of the lead skill'*)
	printf 'lead\n' >>"$WF_TMP/sessions.log"
	done_json
	exit 0
	;;
esac
printf 'task %s\n' "$task" >>"$WF_TMP/sessions.log"
say() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$status"; }
say started
mkdir -p app
printf '%s\n' "$task" >"app/$task.php"
git add "app/$task.php"
git -c core.hooksPath=/dev/null commit -qm "Add a service"
say ready
done_json
FAKE
export WORKFLOW_WORKER_CMD='cd {worktree} && WORKFLOW_AGENT=1 setsid sh -c '"'"'echo $$ > {pidfile}; exec sh "$WF_TMP/fake-worker.sh" {task} {worktree} {status} {brief}'"'"' > {out} 2> {err} &'

# The playbooks live in the workflow project, one section per surface.
new_repo workflow
mem_register
"$MEM_BIN" wiki dogfood-playbooks --stdin --note 'the per-surface playbooks' >/dev/null <<'EOF'
# Dogfood playbooks

## web

Drive the page with playwright-cli and judge each step with jev.

## cli

Run the binary in a tmux pane and capture it.
EOF

new_repo app
mem_register
"$MEM_BIN" project set verify true >/dev/null
"$MEM_BIN" project set dev "$url" >/dev/null
"$MEM_BIN" wiki verify --stdin --note 'how the app is launched and driven' >/dev/null <<'EOF'
# Verify

## Launch

Serve the site directory on a free port.

## Doctor

The port answers with the home page.

## Drive

Open the dev URL in a browser.

## Evidence

A screenshot of each page.

## Cleanup

Stop the server.

## History

Written while the app had one page.
EOF
printf '# roadmap: app-road\n\n- [ ] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello; then the footer shows the year\n' |
	"$MEM_BIN" roadmap --stdin >/dev/null
"$MEM_BIN" roadmap --status approved >/dev/null
"$MEM_BIN" plan m1 --stdin >/dev/null <<-EOF
# plan: m1

- [ ] a1 Add the a1 service
      Files: app/a1.php
      Verify: true
- [ ] a2 Add the a2 service
      Files: app/a2.php
      Verify: true
EOF

S="$XDG_STATE_HOME/workflow/serve/app"

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; kill "$serve_pid" "$site_pid" 2>/dev/null; wait "$serve_pid" "$site_pid" 2>/dev/null; (exit $st); t_done' EXIT

for _ in $(seq 120); do
	[ "$(cat "$S/stage" 2>/dev/null)" = dogfood ] && [ -e "$T_TMP/walk.md" ] && break
	sleep 1
done
# A few more ticks, for anything the walk should hold back to start.
sleep 4

## ------------------------------------------------------------ the session

is "$(grep -c '^dogfood$' "$T_TMP/sessions.log" 2>/dev/null)" 1 'one dogfood session for the landed milestone'
is "$(sed -n '/^dogfood$/,$p' "$T_TMP/sessions.log" | tail -n +2)" '' 'and nothing started after it'
is "$(grep -c 'started the run of' "$T_TMP/serve.out")" 1 'one run for the milestone'
is "$(cat "$S/stage" 2>/dev/null)" dogfood 'the stage reads dogfood'
[ -s "$S/leads" ] && notok 'no lead is recorded beside the walk' "$(cat "$S/leads")" || ok 'no lead is recorded beside the walk'
like "$(cat "$S/walk" 2>/dev/null)" '^m1 1 [0-9]+ [^ ]+ 1 2 3$' 'the walk file holds the slug, walk 1, its start, the session and the steps'
like "$("$MEM_BIN" --project app roadmap)" '^- \[ \] m1 ' 'the milestone stays open while it is walked'
is "$(cat "$T_TMP/walk.pwd" 2>/dev/null)" "$T_TMP/app" 'the session stands in the checkout'
like "$(cat "$T_TMP/walk.env" 2>/dev/null)" "WORKFLOW_STATUS_FILE=$S/m1.dogfood.status" 'and reports to the walk status file'
like "$(cat "$T_TMP/product.html" 2>/dev/null)" '<h1>Hello</h1>' 'the dev key reaches the product'

## -------------------------------------------------------------- the brief

brief=$(cat "$S/m1.dogfood.md" 2>/dev/null)
like "$brief" '1\. open the home page' 'the brief numbers the first step'
like "$brief" '2\. the heading reads Hello' 'the second'
like "$brief" '3\. the footer shows the year' 'and the third, its leading then dropped'
for s in Launch Doctor Drive Evidence Cleanup; do
	like "$brief" "#### $s" "it carries the verify page's $s section"
done
like "$brief" 'Serve the site directory on a free port' 'with its text'
unlike "$brief" 'Written while the app had one page' 'and no other section of the page'
like "$brief" 'Surface: web' 'it names the surface'
like "$brief" 'Drive the page with playwright-cli' 'with the playbook section for it'
unlike "$brief" 'Run the binary in a tmux pane' 'and no other surface'"'"'s'
like "$brief" "dev: $url" 'it carries the dev key'
