#!/usr/bin/env bash
# hubweb under `cf dev`: every path but the store write wants Cloudflare
# Access's JWT, signed RS256 by a key in the team's JWKS, for one of the
# Access apps' auds, inside its lifetime. The store takes records only with
# the bearer STORE_TOKEN, GET / renders one card per project record with its
# stage, its milestone and a Server-Timing header for the page build, and
# POST /mcp serves the handoff tool from the store.
#
# Needs hubweb/node_modules (pnpm -C hubweb install --frozen-lockfile),
# worker-build, the wasm32-unknown-unknown target, openssl and python3.
#
# cf dev hands the Worker to wrangler's cf delegate, which takes --port and
# nothing for state or secrets: state goes to .wrangler/state beside
# cloudflare.config.ts, and secrets come from the environment. So the test runs
# cf dev in a copy of hubweb inside T_TMP, which t_done removes with the state,
# and passes STORE_TOKEN and the Access settings in cf dev's environment,
# where ps does not show them.
#
# The test stands in for the Access team: it makes an RSA key, serves its
# JWKS at <team>/cdn-cgi/access/certs from a local HTTP server, and mints
# the tokens itself.

# The real cache dir, read before t_init moves HOME: worker-build keeps
# wasm-bindgen, wasm-opt and esbuild there, and would download them into a
# sandboxed one on every run.
real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
	exit 1
fi

free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

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
team_port=$(free_port)
team=http://127.0.0.1:$team_port
python3 -m http.server --bind 127.0.0.1 --directory "$T_TMP/team" "$team_port" \
	>"$T_TMP/team.log" 2>&1 &
team_pid=$!

aud=next-hub-aud
cat >"$T_TMP/mint.py" <<'PY'
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

pids=()
stop_all() {
	local status=$?
	for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
	kill "$team_pid" 2>/dev/null
	for pid in "${pids[@]}" "$team_pid"; do wait "$pid" 2>/dev/null; done
	(exit "$status")
	t_done
}
trap stop_all EXIT

start_dev() { # start_dev <name> [VAR=value...] -- cf dev on a copy of hubweb; sets CF_DEV_PORT
	local name=$1
	shift
	local app=$T_TMP/$name
	mkdir -p "$app"
	cp -r "$HUBWEB"/{Cargo.toml,Cargo.lock,src,package.json,cloudflare.config.ts,wrangler.config.ts} "$app/"
	ln -s "$HUBWEB/node_modules" "$app/node_modules"
	t_cf_dev "$app" "$T_TMP/$name.log" "$@"
	local up=$?
	pids+=("$CF_DEV_PID")
	if [ "$up" != 0 ]; then
		notok "cf dev answers on port $CF_DEV_PORT" "$(tail -40 "$T_TMP/$name.log")"
		exit 1
	fi
}

token=test-store-token
printf 'Authorization: Bearer %s\n' "$token" >"$T_TMP/auth.hdr"
printf 'Authorization: Bearer wrong-%s\n' "$token" >"$T_TMP/wrong.hdr"
printf '%s\n' "$(with_jwt "$valid")" >"$T_TMP/jwt.hdr"

# Two auds: next-hub and next-mem are two Access apps on one Worker.
start_dev hubweb STORE_TOKEN=$token ACCESS_AUD="next-mem-aud, $aud" ACCESS_TEAM="$team"
port=$CF_DEV_PORT
base=http://127.0.0.1:$port

put() { # put <body> [curl args...] -- prints the status code
	local body=$1
	shift
	curl -s -o /dev/null -w '%{http_code}' -X PUT "$@" \
		-H 'Content-Type: application/json' --data "$body" "$base/store/records"
}

walkone='{"name":"walkone","roadmap_status":"approved","milestone":"m1-x","plan_slug":"m1-x","plan_ticked":1,"plan_total":3,"milestones_done":0,"milestones_total":2}'
records='[{"project":"walkone","kind":"project","key":"walkone","body":'$walkone'},{"project":"walkone","kind":"handoff","key":"walkone","body":"Next: build the store."}]'

is "$(put "$records")" 401 "PUT /store/records without a token is refused"
is "$(put "$records" -H "@$T_TMP/wrong.hdr")" 401 "PUT /store/records with a wrong token is refused"
like "$(put "$records" -H "@$T_TMP/auth.hdr")" '^2[0-9][0-9]$' \
	"PUT /store/records with the token and no JWT stores two records"

walkdone='{"name":"walkdone","roadmap_status":"approved","milestone":"m2-y","plan_slug":"m2-y","plan_ticked":2,"plan_total":4,"milestones_done":2,"milestones_total":2}'
done_records='[{"project":"walkdone","kind":"project","key":"walkdone","body":'$walkdone'}]'
like "$(put "$done_records" -H "@$T_TMP/auth.hdr")" '^2[0-9][0-9]$' "PUT /store/records stores a finished project"

get_home() { # get_home [curl args...] -- prints the status code of GET /
	curl -s -o /dev/null -w '%{http_code}' "$@" "$base/"
}
fresh=$((now + 600))
is "$(get_home)" 401 "GET / without a JWT is refused"
is "$(get_home -H "$(with_jwt not-a-jwt)")" 401 "GET / with a malformed JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint key HS256 k1 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with an HS256 JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint key none k1 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with an unsigned (alg none) JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint other RS256 k1 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with a JWT signed by another key is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k1 "$(claims wrong-aud "$team" $fresh)")")")" 401 \
	"GET / with another app's aud is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k1 "$(claims "$aud" http://127.0.0.1:1 $fresh)")")")" 401 \
	"GET / with another team's iss is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k1 "$(claims "$aud" "$team" $((now - 60)))")")")" 401 \
	"GET / with an expired JWT is refused"
is "$(get_home -H "$(with_jwt "$(mint key RS256 k2 "$(claims "$aud" "$team" $fresh)")")")" 401 \
	"GET / with a JWT naming an unknown kid is refused"

home=$(curl -s -H "@$T_TMP/jwt.hdr" -D "$T_TMP/home.hdr" -o "$T_TMP/home.html" -w '%{http_code}' "$base/")
is "$home" 200 "GET / with a valid JWT answers 200"

# One line per card, so each check reads the card of the project it names.
card() { tr -d '\n' <"$T_TMP/home.html" | sed 's/<article/\n<article/g' | grep -F ">$1<"; }
one=$(card walkone)
like "$one" 'class="pill">execution<' "walkone's card shows the stage execution"
like "$one" 'm1-x' "walkone's card shows its milestone"
first=1
like "$one" "milestone $first of 2" "walkone's card shows milestone $first of 2"
like "$(card walkdone)" 'class="pill ok">done<' "a project with every milestone ticked shows done"

like "$(tr -d '\r' <"$T_TMP/home.hdr")" '^[Ss]erver-[Tt]iming: build;dur=[0-9.]+' \
	"GET / carries a Server-Timing build duration"

mcp() { # mcp <path> <json-rpc message> [curl args...] -- prints the reply body
	local path=$1 message=$2
	shift 2
	curl -s -X POST "$@" -H 'Content-Type: application/json' \
		-H 'Accept: application/json, text/event-stream' --data "$message" "$base$path"
}
field() { # field <python expression on r> -- reads the JSON reply on stdin
	python3 -c "import json, sys; r = json.load(sys.stdin); print($1)" 2>/dev/null
}
handoff_call='{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"handoff","arguments":{"project":"walkone"}}}'

is "$(mcp /mcp "$handoff_call" -o /dev/null -w '%{http_code}')" 401 "POST /mcp without a JWT is refused"
init=$(mcp /mcp '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' -H "@$T_TMP/jwt.hdr")
is "$(field 'r["result"]["serverInfo"]["name"], r["result"]["protocolVersion"]' <<<"$init")" "mem 2025-06-18" \
	"initialize names the server and the protocol version asked for"
is "$(mcp /mcp '{"jsonrpc":"2.0","method":"notifications/initialized"}' -H "@$T_TMP/jwt.hdr" \
	-o /dev/null -w '%{http_code}')" 202 "notifications/initialized is accepted with no answer"
list=$(mcp /mcp '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' -H "@$T_TMP/jwt.hdr")
is "$(field '[t["name"] for t in r["result"]["tools"]], r["result"]["tools"][0]["inputSchema"]["required"]' <<<"$list")" \
	"['handoff'] ['project']" "tools/list lists handoff, which needs a project"
is "$(mcp /mcp "$handoff_call" -H "@$T_TMP/jwt.hdr" | field 'r["result"]["content"][0]["text"], r["result"]["isError"]')" \
	"Next: build the store. False" "tools/call handoff answers walkone's stored handoff"
is "$(mcp / "$handoff_call" -H "@$T_TMP/jwt.hdr" | field 'r["result"]["content"][0]["text"]')" \
	"Next: build the store." "POST / serves MCP too"

# No Access settings: every guarded path is refused, the store write is not.
start_dev bare STORE_TOKEN=$token
bare_port=$CF_DEV_PORT
is "$(curl -s -o /dev/null -w '%{http_code}' -H "@$T_TMP/jwt.hdr" "http://127.0.0.1:$bare_port/")" 401 \
	"GET / without ACCESS_AUD and ACCESS_TEAM is refused, even with a JWT"
like "$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "@$T_TMP/auth.hdr" \
	-H 'Content-Type: application/json' --data "$done_records" \
	"http://127.0.0.1:$bare_port/store/records")" '^2[0-9][0-9]$' \
	"PUT /store/records works without the Access settings"
