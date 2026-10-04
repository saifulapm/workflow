#!/usr/bin/env bash
# A landed milestone with a Show line is walked before it is ticked: serve
# starts one dogfood session in the checkout whose brief carries the Show
# path cut into numbered steps, the verify page's sections, the playbook for
# the milestone's Surface and the project's dev key. The stage reads dogfood,
# the milestone stays open, and nothing else starts while the walk stands.
# Once the session ends serve reads its report: a pass ticks the milestone,
# a failed step with its finding holds it with `findings 1`, and a session
# that never reports is skipped, walked again on the next tick, and pauses
# the project on its third strike.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
unset WORKFLOW_STATUS_FILE
mkdir -p "$XDG_CONFIG_HOME/qshell"
printf 'here\n' >"$XDG_CONFIG_HOME/qshell/machine"

# The products, one server: the clean page at the root, and under bad/ the
# same page with a defect seeded in its heading.
mkdir -p "$T_TMP/site/bad"
printf '<h1>Hello</h1>\n<footer>2026</footer>\n' >"$T_TMP/site/index.html"
printf '<h1>Helo</h1>\n<footer>2026</footer>\n' >"$T_TMP/site/bad/index.html"
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

# jev judges the page the session last fetched: the claim's last word is on
# it or it is not, with jev's exit codes, 0 true and 2 not true.
write_exec "$T_TMP/bin/jev" <<'JEV'
#!/bin/sh
[ "$1" = check ] || exit 1
word=${2##* }
if grep -q -- "$word" "$WF_TMP/$MEM_PROJECT.html" 2>/dev/null; then
	echo yes
	exit 0
fi
echo no
exit 2
JEV

# The fake session: every start is logged by what it is and whose. A walk
# fetches the product from the dev key, judges it with jev and reports, a
# defect filed as a finding with the page as its capture; app's walk first
# keeps its brief, environment and directory and waits to be let go, and
# mute's never reports. A lead does nothing; a task commits its file.
write_exec "$T_TMP/fake-worker.sh" <<'FAKE'
#!/bin/sh
task=$1; status=$3; brief=$4
done_json() { printf '{"is_error":false,"result":"ok"}\n'; }
goal=$(sed -n '/^## GOAL/,/^## SCOPE/p' "$brief")
case "$goal" in
*'Walk the Show path'*)
	printf 'dogfood %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	case "$MEM_PROJECT" in
	mute)
		done_json
		exit 0
		;;
	app)
		cp "$brief" "$WF_TMP/walk.md"
		env >"$WF_TMP/walk.env"
		pwd >"$WF_TMP/walk.pwd"
		for _ in $(seq 600); do [ -e "$WF_TMP/release" ] && break; sleep 0.2; done
		;;
	esac
	curl -s "$(sed -n 's/^dev: //p' "$brief")" >"$WF_TMP/$MEM_PROJECT.html"
	if jev check "the heading reads Hello" >/dev/null; then
		workflow report ready pass
	else
		mem finding add --milestone m1 --step 2 --evidence "$WF_TMP/$MEM_PROJECT.html" \
			'the heading reads Helo' >/dev/null
		workflow report ready 'failed 2'
	fi
	done_json
	exit 0
	;;
*'of the lead skill'*)
	printf 'lead %s\n' "$MEM_PROJECT" >>"$WF_TMP/sessions.log"
	done_json
	exit 0
	;;
esac
# A task's worktree is <root>/<project>/<plan>/<task>.
printf 'task %s %s\n' "$task" "$(basename "$(dirname "$(dirname "$2")")")" >>"$WF_TMP/sessions.log"
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

# project <name> <dev>: a checkout whose roadmap's one milestone has a Show
# line and a plan of two tasks.
project() {
	new_repo "$1"
	mem_register
	"$MEM_BIN" project set verify true >/dev/null
	"$MEM_BIN" project set dev "$2" >/dev/null
	printf '# roadmap: %s-road\n\n- [ ] m1 The home page\n      Surface: web\n      Show: open the home page, the heading reads Hello; then the footer shows the year\n' "$1" |
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
}

project bad "${url}bad/"
project mute "$url"
project app "$url"
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

S="$XDG_STATE_HOME/workflow/serve/app"
runs() { "$MEM_BIN" --project "$1" log --type run 2>/dev/null; }
# walked <project>: wait for the line serve logs once it has read the walk.
walked() {
	for _ in $(seq 120); do
		runs "$1" | grep -q 'dogfood m1:' && return 0
		sleep 1
	done
	return 1
}

cd "$T_TMP" || exit 1
WORKFLOW_DEADLINE_MIN=0.5 workflow serve --tick 1 >"$T_TMP/serve.out" 2>&1 &
serve_pid=$!
trap 'st=$?; touch "$T_TMP/release"; kill "$serve_pid" "$site_pid" 2>/dev/null; wait "$serve_pid" "$site_pid" 2>/dev/null; (exit $st); t_done' EXIT

for _ in $(seq 120); do
	[ "$(cat "$S/stage" 2>/dev/null)" = dogfood ] && [ -e "$T_TMP/walk.md" ] && break
	sleep 1
done
# A few more ticks, for anything the walk should hold back to start.
sleep 4

## ------------------------------------------------------------ the session

is "$(grep -c '^dogfood app$' "$T_TMP/sessions.log" 2>/dev/null)" 1 'one dogfood session for the landed milestone'
is "$(sed -n '/^dogfood app$/,$p' "$T_TMP/sessions.log" | tail -n +2 | grep -c ' app$')" 0 'and nothing started after it'
is "$(grep -c 'serve app: started the run of' "$T_TMP/serve.out")" 1 'one run for the milestone'
is "$(cat "$S/stage" 2>/dev/null)" dogfood 'the stage reads dogfood'
[ -s "$S/leads" ] && notok 'no lead is recorded beside the walk' "$(cat "$S/leads")" || ok 'no lead is recorded beside the walk'
like "$(cat "$S/walk" 2>/dev/null)" '^m1 1 [0-9]+ [^ ]+ 1 2 3$' 'the walk file holds the slug, walk 1, its start, the session and the steps'
like "$("$MEM_BIN" --project app roadmap)" '^- \[ \] m1 ' 'the milestone stays open while it is walked'
is "$(cat "$T_TMP/walk.pwd" 2>/dev/null)" "$T_TMP/app" 'the session stands in the checkout'
like "$(cat "$T_TMP/walk.env" 2>/dev/null)" "WORKFLOW_STATUS_FILE=$S/m1.dogfood.status" 'and reports to the walk status file'
is "$(runs app | grep -c 'dogfood m1:')" 0 'a live walk is not read'

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

## ------------------------------------------------------- a clean product

touch "$T_TMP/release"
walked app || notok 'the clean walk is read' "$(tail -5 "$T_TMP/serve.out")"
like "$(cat "$T_TMP/app.html" 2>/dev/null)" '<h1>Hello</h1>' 'the dev key reaches the product'
like "$(runs app)" 'dogfood m1: pass' 'a clean product logs its pass'
for _ in $(seq 20); do "$MEM_BIN" --project app roadmap | grep -q '^- \[x\] m1 ' && break; sleep 0.5; done
like "$("$MEM_BIN" --project app roadmap)" '^- \[x\] m1 ' 'and ticks its milestone'
[ -e "$S/walk" ] && notok 'the walk file goes with the pass' "$(cat "$S/walk")" || ok 'the walk file goes with the pass'

## -------------------------------------------------------- a seeded defect

B="$XDG_STATE_HOME/workflow/serve/bad"
walked bad || notok 'the failed walk is read' "$(tail -5 "$T_TMP/serve.out")"
like "$(runs bad)" 'dogfood m1: findings 1' 'a seeded defect logs its open finding'
like "$("$MEM_BIN" --project bad roadmap)" '^- \[ \] m1 ' 'and leaves its milestone unticked'
found=$("$MEM_BIN" --project bad finding list --open --json)
like "$found" '"step":"2"' 'the finding stands on the failed step'
like "$found" '"file":"evidence/m1/[^"]+"' 'with its capture filed'
like "$(head -1 "$B/walk" 2>/dev/null)" '^m1 1 [0-9]+ [^ ]+ 1 2 3$' 'the walk file stays'
is "$(sed -n 2p "$B/walk" 2>/dev/null)" 'failed 2' 'with the outcome on its second line'
sleep 3
is "$(cat "$B/stage" 2>/dev/null)" waiting 'the stage holds waiting'
is "$(runs bad | grep -c 'dogfood m1:')" 1 'and the walk is read once'

## ------------------------------------------------------ a silent session

M="$XDG_STATE_HOME/workflow/serve/mute"
walked mute || notok 'the silent walk is read' "$(tail -5 "$T_TMP/serve.out")"
like "$(runs mute)" 'dogfood m1: skipped no report' 'a session that never reports is skipped'
like "$("$MEM_BIN" --project mute roadmap)" '^- \[ \] m1 ' 'and its milestone stays open'
for _ in $(seq 30); do [ "$(cat "$M/stage" 2>/dev/null)" = paused ] && break; sleep 1; done
is "$(runs mute | grep -c 'dogfood m1: skipped no report')" 3 'each skipped walk is walked again on the next tick'
is "$(grep -c '^dogfood mute$' "$T_TMP/sessions.log")" 3 'one session for each'
is "$(sed -n 2p "$M/walk" 2>/dev/null)" 'skipped no report' 'the walk file keeps the outcome'
is "$(sed -n 3p "$M/walk" 2>/dev/null)" 'strikes 3' 'and counts the strikes'
is "$(cat "$M/stage" 2>/dev/null)" paused 'the third strike pauses the project'
