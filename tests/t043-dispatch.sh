#!/usr/bin/env bash
# The default dispatch template, with a stub standing in for `claude`. This is
# the one place the shipped template itself runs: the credential scrub of spec
# §1, the flags, the fresh session uuid, and the brief inside its byte budget.
source "$(dirname -- "$0")/lib.sh"
t_init

export WF_TMP="$T_TMP"
# The stub speaks the whole surface the backend drives: a --bg dispatch, the
# agents listing (every recorded session, already idle — the stub's work is
# done the moment it returns), and stop. Env and args are recorded only for
# the dispatch: the agents calls come from the orchestrator's own unscrubbed
# environment and must not overwrite the worker's.
write_exec "$T_TMP/bin/claude" <<'CLAUDE'
#!/bin/sh
case "$1" in
agents)
	out='['; sep=''
	if [ -f "$WF_TMP/sessions" ]; then
		while IFS= read -r sid; do
			out="$out$sep{\"pid\":1,\"id\":\"id-$sid\",\"kind\":\"background\",\"sessionId\":\"$sid\",\"status\":\"idle\"}"
			sep=','
		done <"$WF_TMP/sessions"
	fi
	printf '%s]\n' "$out"
	exit 0 ;;
stop) exit 0 ;;
esac
env >"$WF_TMP/claude-env"
printf '%s\n' "$*" >>"$WF_TMP/claude-args"
prev=''
for a in "$@"; do
	[ "$prev" = "--session-id" ] && printf '%s\n' "$a" >>"$WF_TMP/sessions"
	prev=$a
done
printf '{"is_error":false,"total_cost_usd":0.01,"result":"ok"}\n'
CLAUDE

# Nothing here may reach the real binary: the stub has to be the claude that
# the template's PATH resolves, or this test does not run at all.
is "$(command -v claude)" "$T_TMP/bin/claude" 'the stub is the claude on PATH'
[ "$(command -v claude)" = "$T_TMP/bin/claude" ] || exit 1

new_repo app
mem_register
printf '{"name":"acme/app"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'

cat >"$T_TMP/plan.md" <<'EOF'
# plan: dispatch-check

- [ ] t1 Extract cart pricing into a service
      Files: app/Services/Cart*.php tests/Unit/Cart*
      Verify: bin/php artisan test --filter=Cart
      Done: cart totals identical for the fixture basket
- [ ] t2 A second task so the run is worth having
      Files: app/Services/Other*.php
      Verify: true
EOF

# The interactive environment carries live tokens. These stand in for them --
# including the ones with a digit in the name, which the glob-shaped contract of
# spec §1 covers and a letters-only character class quietly did not.
export GITHUB_API_KEY=live-token-1 GH_TOKEN=live-token-2 AWS_ACCESS_KEY_ID=live-token-3
export STRIPE_SECRET=live-token-4 SOME_APP_TOKEN=live-token-5 A_PRIVATE_KEY=live-token-6
export AWS_S3_SECRET=live-token-7 OAUTH2_TOKEN=live-token-8 GH2_TOKEN=live-token-9
export S3_SECRET=live-token-10
export ORDINARY_SETTING=keep-me

run env WORKFLOW_MAX_WORKERS=1 WORKFLOW_DEADLINE_MIN=0.2 WORKFLOW_MODEL=haiku \
	WORKFLOW_ALLOW_PUSH=1 WORKFLOW_HOOK_SEEN=/some/repo/.git \
	WORKFLOW_WORKER_BUDGET=3 WORKFLOW_MAX_TURNS=7 workflow run --plan-file "$T_TMP/plan.md"
is "$RC" 1 'the stub reports no ready state, so both tasks park'

## ------------------------------------------------------- the scrubbed env

envdump=$(cat "$T_TMP/claude-env")
unlike "$envdump" '^GITHUB_API_KEY=' 'GITHUB_API_KEY never reaches the worker'
unlike "$envdump" '^GH_TOKEN=' 'nor does a GH_ variable'
unlike "$envdump" '^AWS_ACCESS_KEY_ID=' 'nor an AWS_ one'
unlike "$envdump" '^STRIPE_SECRET=' 'nor a STRIPE_ one'
unlike "$envdump" '^SOME_APP_TOKEN=' 'nor anything ending in _TOKEN'
unlike "$envdump" '^A_PRIVATE_KEY=' 'nor anything ending in _KEY'
unlike "$envdump" '^AWS_S3_SECRET=' 'nor an AWS_ name with a digit in it'
unlike "$envdump" '^OAUTH2_TOKEN=' 'nor a _TOKEN name with a digit in it'
unlike "$envdump" '^GH2_TOKEN=' 'nor a GH-prefixed name with a digit in it'
unlike "$envdump" '^S3_SECRET=' 'nor a _SECRET name that starts with a digit-bearing word'
unlike "$envdump" 'live-token' 'no token value survives anywhere in the environment'
like "$envdump" '^ORDINARY_SETTING=keep-me' 'and an ordinary variable is left alone'
like "$envdump" '^WORKFLOW_AGENT=1' 'WORKFLOW_AGENT reaches the process that was exec-ed'
unlike "$envdump" '^WORKFLOW_ALLOW_PUSH=' \
	'an orchestrator run under the push valve does not hand it to its workers'
unlike "$envdump" '^WORKFLOW_HOOK_SEEN=' \
	'nor the depth guard, which would tell the first commit the gate had run'

## ------------------------------------------------------------- the flags

args=$(cat "$T_TMP/claude-args")
like "$args" '\-\-bg' 'workers are background sessions, visible in the agents view'
unlike "$args" '\-p \-\-output-format' 'and never print mode'
like "$args" '--dangerously-skip-permissions' 'permissions are skipped in the worktree'
like "$args" '--max-budget-usd 3' 'the worker budget knob reaches the command line'
like "$args" '--model haiku' 'and the model'
like "$args" '--session-id [0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}' \
	'the session id is a uuid, which is what --session-id insists on'
like "$args" 'Read .*/t[12]\.md and execute it exactly' 'and the prompt points at the brief'

rundir="$XDG_STATE_HOME/workflow/runs/app/dispatch-check"
s1=$(cat "$rundir/t1.session")
s2=$(cat "$rundir/t2.session")
isnt "$s1" "$s2" 'each dispatch mints its own session id'
is "$(cut -f2 "$rundir/costs.tsv" | sort -u)" '0.01' "both workers' costs landed in the ledger"

## ------------------------------------------------------------- the brief

brief="$XDG_CACHE_HOME/workflow/briefs/app/dispatch-check/t1.md"
truthy "$([ -f "$brief" ] && echo 0 || echo 1)" 'the brief was written where the template points'
is "$(($(wc -c <"$brief") <= 2000))" 1 'the brief is inside its 2000 byte budget'
body=$(cat "$brief")
like "$body" 'Extract cart pricing into a service' 'the brief carries the objective'
like "$body" 'Files: app/Services/Cart\*\.php tests/Unit/Cart\*' 'and the task block verbatim'
like "$body" 'Verify: bin/php artisan test --filter=Cart' 'including the evidence command'
like "$body" 'Done: cart totals identical' 'and the done condition'
like "$body" 'Never leave it' 'the worktree boundary'
like "$body" 'never .git add -A.' 'the staging rule'
like "$body" 'mem ask' 'the stop protocol'
like "$body" 'started, progress, ready, blocked' 'and the reporting states'
like "$body" "$rundir/t1.status" 'the brief names the status file by path'

## ------------------------------------------- a project whose path has a space

# The same shipped template, end to end, in a project called "my project". Every
# placeholder it substitutes -- the worktree, the brief, the pidfile, the two
# redirections -- carries that space, so this is where the quoting is decided.
write_exec "$T_TMP/bin/claude" <<'CLAUDE'
#!/bin/sh
# A worker that does the job: it reads the brief it was pointed at, writes the
# one file the task owns, commits it and reports ready. Same agents/stop
# surface as the first stub — the work happens during the dispatch call, so
# every listed session is already idle.
case "$1" in
agents)
	out='['; sep=''
	if [ -f "$WF_TMP/sessions" ]; then
		while IFS= read -r sid; do
			out="$out$sep{\"pid\":1,\"id\":\"id-$sid\",\"kind\":\"background\",\"sessionId\":\"$sid\",\"status\":\"idle\"}"
			sep=','
		done <"$WF_TMP/sessions"
	fi
	printf '%s]\n' "$out"
	exit 0 ;;
stop) exit 0 ;;
esac
prev=''
for a in "$@"; do
	[ "$prev" = "--session-id" ] && printf '%s\n' "$a" >>"$WF_TMP/sessions"
	prev=$a
done
for a in "$@"; do prompt=$a; done
brief=$(printf '%s' "$prompt" | sed -n 's/^Read \(.*\) and execute it exactly\.$/\1/p')
[ -r "$brief" ] || { printf '{"is_error":true,"result":"no brief"}\n'; exit 0; }
task=$(basename "$brief" .md)
status=$(sed -n 's/^Append one line per state change to \(.*\):$/\1/p' "$brief")
file=$(sed -n 's/^ *Files: *//p' "$brief" | head -1)
printf '%s started\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
mkdir -p "$(dirname "$file")"
printf '%s\n' "$task" >"$file"
git add "$file"
git -c core.hooksPath=/dev/null commit -qm "Add the $task file"
printf '%s ready merge-ready\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$status"
printf '{"is_error":false,"total_cost_usd":0.02,"result":"ok"}\n'
CLAUDE

new_repo 'my project'
spaced=$PWD
mem_register
printf '{"name":"acme/spaced"}\n' >composer.json
printf '#!/bin/sh\nexit 0\n' >artisan
chmod +x artisan
write_exec bin/php <<-'EOF'
	#!/bin/sh
	exit 0
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm 'project files'
spaced_base=$(git rev-parse HEAD)

cat >"$T_TMP/spaced-plan.md" <<'EOF'
# plan: spaced

- [ ] t1 Write the first file
      Files: app/One.php
      Verify: true
- [ ] t2 Write the second file
      Files: app/Two.php
      Verify: true
EOF

is "$(printf '%s' "$spaced" | grep -c ' ')" 1 'the project really is at a path with a space in it'
run env WORKFLOW_MAX_WORKERS=2 WORKFLOW_DEADLINE_MIN=0.5 \
	workflow run --plan-file "$T_TMP/spaced-plan.md"
is "$RC" 0 'a project path with a space dispatches and the run completes'
srun="$XDG_STATE_HOME/workflow/runs/my project/spaced"
is "$(cat "$srun/t1.state")" merged 'spaced path: the first task merged'
is "$(cat "$srun/t2.state")" merged 'spaced path: and so did the second'
is "$(git rev-list --count "$spaced_base..integration/spaced")" 2 \
	'spaced path: both commits are on the integration branch'
is "$(wc -l <"$srun/t1.err")" 0 'spaced path: the worker wrote nothing to stderr'
