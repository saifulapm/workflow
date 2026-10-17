# shellcheck shell=bash
# Shared harness for the workflow tests. Sourced by every tests/t*.sh.
#
# No test framework: each script prints TAP-ish lines (`ok N - desc`) and a
# trailing plan (`1..N`); tests/run.sh tallies them. Every test runs against a
# throwaway HOME so nothing on the machine is read or written -- see t_init.

set -uo pipefail

TESTS_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
WF_ROOT=$(dirname -- "$TESTS_DIR")
MEM_BIN=${MEM_BIN:-$WF_ROOT/mem/target/release/mem}

# The program under test. The suite drives the CLI, so it validates whichever
# implementation this points at; the default is the crate's release binary.
WORKFLOW_BIN=${WORKFLOW_BIN:-$WF_ROOT/workflow/target/release/workflow}
case "$WORKFLOW_BIN" in
/*) ;;
*) WORKFLOW_BIN=$(cd -- "$(dirname -- "$WORKFLOW_BIN")" && pwd)/$(basename -- "$WORKFLOW_BIN") ;;
esac
export WORKFLOW_BIN

_checks=0
_failed=0

ok()    { _checks=$((_checks + 1)); printf 'ok %d - %s\n' "$_checks" "$1"; }
notok() {
	_checks=$((_checks + 1))
	_failed=$((_failed + 1))
	printf 'not ok %d - %s\n' "$_checks" "$1"
	[ $# -gt 1 ] && printf '%s\n' "$2" | sed 's/^/#   /'
	return 0
}

# is <got> <want> <desc>
is() {
	if [ "$1" = "$2" ]; then ok "$3"; else notok "$3" "got:  $1
want: $2"; fi
}

# isnt <got> <unwanted> <desc>
isnt() {
	if [ "$1" != "$2" ]; then ok "$3"; else notok "$3" "got the unwanted value: $1"; fi
}

# like <got> <extended-regex> <desc>
#
# A here-string, not a pipe from printf: grep -q quits on the first match, and
# under load a printf still writing behind it took SIGPIPE, which pipefail then
# read as the check failing. That was one in two hundred checks going red on a
# text that matched, only under load.
like() {
	if grep -Eq -- "$2" <<<"$1"; then ok "$3"; else notok "$3" "no match for /$2/ in:
$1"; fi
}

# unlike <got> <extended-regex> <desc>
unlike() {
	if grep -Eq -- "$2" <<<"$1"; then notok "$3" "unwanted match for /$2/ in:
$1"; else ok "$3"; fi
}

# truthy <status> <desc>
truthy() { is "$1" 0 "$2"; }

# Run a command, capturing combined output in OUT and the status in RC.
run() {
	OUT=$("$@" 2>&1)
	RC=$?
	return 0
}

# Same, but stdout only (stderr passed through to the log).
run_out() {
	OUT=$("$@" 2>/dev/null)
	RC=$?
	return 0
}

# t_cf_dev <app> <log> [VAR=value...] -- runs `cf dev` in <app>, a copy of
# hubweb, with the variables in its environment, and waits until it answers.
# Sets CF_DEV_PORT, and CF_DEV_PID, the leader of its process group (setsid),
# for the caller to kill. A port found free can be taken by another test's
# workerd before this one binds it, so a bind failure is tried again on a new
# port. Callers set real_cache and HUBWEB: worker-build keeps its tools in the
# real cache, and CARGO_TARGET_DIR reuses hubweb's compiled dependencies. cf
# and wrangler each send usage telemetry unless told not to; the sandbox HOME
# holds no Cloudflare login, so nothing else can reach the account.
#
# At most two cf dev servers run at once on the machine, whatever WF_JOBS
# says: seven at once overflowed the mini's memory. A test takes one of two
# slots (flock files in WF_CF_SLOTS) the first time it starts cf dev and
# holds it until it ends. The lock sits in a small watcher of the test's
# shell, never in a file descriptor the test's children inherit, so a
# server left behind cannot keep a slot; and the watcher is no child of the
# test, so a test's bare `wait` does not wait on it.
t_cf_dev() {
	local app=$1 log=$2 _try _ _slot _mark _me=$BASHPID
	shift 2
	mkdir -p "${WF_CF_SLOTS:=${TMPDIR:-/tmp}/wf-cf-slots-$(id -u)}"
	while [ -z "${_cf_slot:-}" ]; do
		for _slot in 1 2; do
			_mark=$WF_CF_SLOTS/$_slot.$_me
			rm -f "$_mark" "$_mark.busy"
			( (flock -n -o "$WF_CF_SLOTS/$_slot" sh -c \
				'touch "$1"; while kill -0 "$2" 2>/dev/null; do sleep 0.5; done' \
				_ "$_mark" "$_me" || touch "$_mark.busy") </dev/null >/dev/null 2>&1 &)
			while [ ! -e "$_mark" ] && [ ! -e "$_mark.busy" ]; do sleep 0.05; done
			rm -f "$_mark.busy"
			if [ -e "$_mark" ]; then
				rm -f "$_mark"
				_cf_slot=$_slot
				break
			fi
		done
		[ -n "${_cf_slot:-}" ] || sleep 0.2
	done
	for _try in 1 2 3; do
		CF_DEV_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
		(
			cd "$app" &&
				exec env "$@" XDG_CACHE_HOME="$real_cache" CARGO_TARGET_DIR="$HUBWEB/target" \
					CF_SEND_TELEMETRY=false WRANGLER_SEND_METRICS=false \
					setsid "$HUBWEB/node_modules/.bin/cf" dev --port "$CF_DEV_PORT"
		) </dev/null >"$log" 2>&1 &
		CF_DEV_PID=$!
		# A cold run compiles the crate to wasm first, so the wait is long.
		for _ in $(seq 1 600); do
			curl -s -o /dev/null "http://127.0.0.1:$CF_DEV_PORT/" 2>/dev/null && return 0
			kill -0 "$CF_DEV_PID" 2>/dev/null || break
			grep -q 'Address already in use' "$log" && break
			sleep 0.5
		done
		kill -TERM -- "-$CF_DEV_PID" 2>/dev/null
		wait "$CF_DEV_PID" 2>/dev/null
		grep -q 'Address already in use' "$log" || return 1
	done
	return 1
}

# t_hubweb_copy <dir> -- a copy of hubweb at <dir> that cf dev can run: its
# sources and config copied, node_modules linked, and hubpages (a path
# dependency, and the static assets) linked beside it.
t_hubweb_copy() {
	local app=$1
	mkdir -p "$app"
	cp -r "$HUBWEB"/{Cargo.toml,Cargo.lock,src,package.json,cloudflare.config.ts,wrangler.config.ts} "$app/"
	ln -s "$HUBWEB/node_modules" "$app/node_modules"
	[ -e "$(dirname -- "$app")/hubpages" ] || ln -s "$WF_ROOT/hubpages" "$(dirname -- "$app")/hubpages"
}

# t_access_team -- stands in for the Cloudflare Access team: an RSA key, its
# JWKS served at <team>/cdn-cgi/access/certs, and a minter. Sets team, aud,
# team_pid, now and valid (a fresh token for aud), and defines claims, mint
# and with_jwt. `other` is a second key, for forgeries.
t_access_team() {
	# The team: a key, its JWKS, and a server for it. `other` signs the forgery.
	openssl genrsa -out "$T_TMP/key.pem" 2048 2>/dev/null
	openssl genrsa -out "$T_TMP/other.pem" 2048 2>/dev/null
	mkdir -p "$T_TMP/team/cdn-cgi/access"
	openssl rsa -in "$T_TMP/key.pem" -noout -modulus | python3 -c '
	import base64, json, sys
	n = bytes.fromhex(sys.stdin.read().strip().split("=", 1)[1])
	n = base64.urlsafe_b64encode(n).rstrip(b"=").decode()
	print(json.dumps({"keys": [{"kty": "RSA", "alg": "RS256", "kid": "k1", "use": "sig", "n": n, "e": "AQAB"}]}))
	' >"$T_TMP/team/cdn-cgi/access/certs"
	team_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
	team=http://127.0.0.1:$team_port
	python3 -m http.server --bind 127.0.0.1 --directory "$T_TMP/team" "$team_port" \
		>"$T_TMP/team.log" 2>&1 &
	team_pid=$!
	
	aud=next-hub-aud
	cat >"$T_TMP/mint.py" <<-'PY'
	import base64, hashlib, hmac, json, subprocess, sys
	key, alg, kid, claims = sys.argv[1:5]
	def b64(raw):
	    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
	signed = b64(json.dumps({"alg": alg, "kid": kid, "typ": "JWT"}).encode()) + "." + b64(claims.encode())
	if alg == "RS256":
	    sig = subprocess.run(["openssl", "dgst", "-sha256", "-sign", key], input=signed.encode(),
	                         capture_output=True, check=True).stdout
	elif alg == "HS256":
	    sig = hmac.new(b"guessed", signed.encode(), hashlib.sha256).digest()
	else:
	    sig = b""
	print(signed + "." + b64(sig))
	PY
	now=$(date +%s)
	claims() { # claims <aud> <iss> <exp> -- one token's claims
		printf '{"aud":["%s"],"iss":"%s","exp":%s,"iat":%s,"email":"saiful@example.com"}' "$1" "$2" "$3" "$now"
	}
	mint() { # mint <key> <alg> <kid> <claims> -- prints the token
		python3 "$T_TMP/mint.py" "$T_TMP/$1.pem" "$2" "$3" "$4"
	}
	with_jwt() { printf 'Cf-Access-Jwt-Assertion: %s' "$1"; }
	valid=$(mint key RS256 k1 "$(claims "$aud" "$team" $((now + 600)))")
}

# Print the plan and exit with the right status. Registered as an EXIT trap by
# t_init so a test that dies mid-way still reports what it managed to run.
t_done() {
	local status=$?
	trap - EXIT
	printf '1..%d\n' "$_checks"
	[ "$status" -ne 0 ] && [ "$_failed" -eq 0 ] && printf '# aborted with status %s\n' "$status"
	[ -n "${T_TMP:-}" ] && [ -z "${WF_KEEP_TMP:-}" ] && rm -rf -- "$T_TMP"
	if [ "$_failed" -gt 0 ] || [ "$status" -ne 0 ]; then exit 1; fi
	exit 0
}

# A lone `bash tests/tNNN.sh` runs the release binaries as they are on disk,
# unlike tests/run.sh, which rebuilds them. Refuse one that is older than the
# newest source of its crate (or of a path dependency of it), and name the
# build. Only the default release path inside this tree is checked; a binary
# the caller points elsewhere is theirs.
# _stale_guard <bin> <crate>
_stale_guard() {
	local bin=$1 crate=$2 dirs=("$WF_ROOT/$2") dep newer d
	[ "$bin" = "$WF_ROOT/$crate/target/release/$crate" ] && [ -x "$bin" ] || return 0
	while IFS= read -r dep; do
		[ -d "$WF_ROOT/$crate/$dep/src" ] && dirs+=("$(cd -- "$WF_ROOT/$crate/$dep" && pwd)")
	done < <(sed -n 's/.*path *= *"\([^"]*\)".*/\1/p' "$WF_ROOT/$crate/Cargo.toml" 2>/dev/null)
	for d in "${dirs[@]}"; do
		newer=$(find "$d/src" "$d/Cargo.toml" "$d/Cargo.lock" -type f -newer "$bin" -print -quit 2>/dev/null)
		if [ -n "$newer" ]; then
			printf 'stale %s: %s is newer than it.\nbuild it first: (cd %s && cargo build --release)\n' \
				"$crate" "$newer" "$WF_ROOT/$crate" >&2
			exit 1
		fi
	done
}

# t_init -- a sandbox HOME, an isolated git and mem, workflow on PATH.
t_init() {
	_stale_guard "$MEM_BIN" mem
	_stale_guard "$WORKFLOW_BIN" workflow
	# Tests default HUB_BIN themselves, after t_init, so a script that names
	# it is a hub test.
	if [ -n "${HUB_BIN:-}" ] || grep -q HUB_BIN "$0" 2>/dev/null; then
		_stale_guard "${HUB_BIN:-$WF_ROOT/hub/target/release/hub}" hub
	fi
	T_TMP=$(mktemp -d "${TMPDIR:-/tmp}/wf-test.XXXXXX")
	trap t_done EXIT

	# Toolchains keep their real homes: the sandbox is about state workflow and
	# mem write, not about reinstalling rust inside every test.
	export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
	export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
	# mise too, and for a second reason: it reads a config.toml in any ancestor
	# of the cwd. With TMPDIR under the real HOME the scratch repos sit below
	# ~/.config/mise, and once HOME moves that file is no longer the global
	# config but an untrusted project one, so every shimmed node, npm and pnpm
	# refuses to start.
	export MISE_CONFIG_DIR="${MISE_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/mise}"
	export MISE_DATA_DIR="${MISE_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/mise}"

	export HOME="$T_TMP/home"
	export XDG_DATA_HOME="$HOME/.local/share"
	export XDG_CACHE_HOME="$HOME/.cache"
	export XDG_STATE_HOME="$HOME/.local/state"
	export XDG_CONFIG_HOME="$HOME/.config"
	mkdir -p "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$T_TMP/bin"

	export GIT_CONFIG_GLOBAL="$HOME/.gitconfig"
	export GIT_CONFIG_NOSYSTEM=1
	git config --global user.email tester@example.invalid
	git config --global user.name 'Workflow Tester'
	git config --global init.defaultBranch main
	git config --global commit.gpgsign false
	git config --global advice.detachedHead false

	ln -sf "$WORKFLOW_BIN" "$T_TMP/bin/workflow"
	[ -x "$MEM_BIN" ] && ln -sf "$MEM_BIN" "$T_TMP/bin/mem"
	export PATH="$T_TMP/bin:$PATH"
	export WORKFLOW_MEM="$MEM_BIN"
	export MEM_SYNC_CMD=true
	# amx is what `workflow go` starts agents with, and nothing in a sandbox may
	# reach the machine's: a test that wants one points WORKFLOW_AMX at its own
	# fake. A binary that is not there fails every amx call rather than
	# touching somebody's real agent.
	export WORKFLOW_AMX="$T_TMP/bin/no-amx"

	# PI_CODING_AGENT is pi's own marker, exported to every child; the hooks
	# read it beside WORKFLOW_AGENT (hook.rs agent_marked), so a suite run from
	# inside pi would see every "human" commit as an agent's.
	unset WORKFLOW_AGENT PI_CODING_AGENT WORKFLOW_ALLOW_PUSH
	unset MEM_PROJECT
	unset GIT_DIR GIT_INDEX_FILE GIT_PREFIX GIT_WORK_TREE
	# A session the hub starts carries the mini's local.env: its store and
	# peer keys would point every sandboxed hub and mem at the real store.
	for v in $(compgen -e | grep -E '^HUB_' | grep -vx HUB_BIN); do unset "$v"; done
	unset TS_AUTHKEY MEM_REMOTE

	HOOKS="$WF_ROOT/hooks"
	cd "$T_TMP" || exit 1
}

# new_repo <dir> -- an initialised repo with one commit, cd'd into.
new_repo() {
	mkdir -p "$T_TMP/$1"
	cd "$T_TMP/$1" || exit 1
	git init -q .
	printf 'seed\n' >README.md
	git add README.md
	git -c core.hooksPath=/dev/null commit -qm 'seed'
}

# mem_register -- make the cwd repo a checkout mem knows (a write verb registers).
mem_register() {
	"$MEM_BIN" log 'registered by the test harness' >/dev/null 2>&1
}

# age_mem_items -- backdate every item in the sandbox store so `--since` windows
# no longer cover them. mem reads created/modified from the frontmatter, so the
# rewrite has to go through a reindex.
age_mem_items() {
	local f
	while IFS= read -r f; do
		sed -i 's/^created = ".*"/created = "2000-01-01T00:00:00Z"/; s/^modified = ".*"/modified = "2000-01-01T00:00:00Z"/' "$f"
	done < <(find "$XDG_DATA_HOME/mem/store" -name '*.md' 2>/dev/null)
	"$MEM_BIN" reindex >/dev/null 2>&1
}

# git_gated <args...> -- run git with the workflow hooks installed.
git_gated() {
	git -c core.hooksPath="$HOOKS" "$@"
}

# write_exec <path> <<'EOF' ... EOF -- write a file from stdin and chmod +x.
write_exec() {
	mkdir -p "$(dirname -- "$1")"
	cat >"$1"
	chmod +x "$1"
}
