#!/usr/bin/env bash
# The evidence page's images on next-hub: GET /p/<project>/file/<id> reads the
# item's bytes from the project's Files store and sends them back with the
# image's type, a private hour of cache and nosniff. An unknown id, or an id
# of another project, is 404, and a request with no Access JWT is 401. Same
# harness as t140: the test stands in for the Access team and mints the JWT.
#
# Needs hubweb/node_modules (pnpm -C hubweb install --frozen-lockfile),
# worker-build, the wasm32-unknown-unknown target, openssl and python3.

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

# The team: a key, its JWKS, and a server for it.
openssl genrsa -out "$T_TMP/key.pem" 2048 2>/dev/null
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
now=$(date +%s)
python3 - "$T_TMP/key.pem" "$aud" "$team" "$now" >"$T_TMP/jwt.hdr" <<'PY'
import base64, json, subprocess, sys
key, aud, team, now = sys.argv[1:5]
def b64(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
claims = {"aud": [aud], "iss": team, "exp": int(now) + 600, "iat": int(now), "email": "saiful@example.com"}
signed = b64(json.dumps({"alg": "RS256", "kid": "k1", "typ": "JWT"}).encode()) + "." + b64(json.dumps(claims).encode())
sig = subprocess.run(["openssl", "dgst", "-sha256", "-sign", key], input=signed.encode(),
                     capture_output=True, check=True).stdout
print("Cf-Access-Jwt-Assertion: " + signed + "." + b64(sig))
PY

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

token=test-store-token
app=$T_TMP/hubweb
t_hubweb_copy "$app"
t_cf_dev "$app" "$T_TMP/dev.log" STORE_TOKEN=$token ACCESS_AUD="$aud" ACCESS_TEAM="$team"
up=$?
pids+=("$CF_DEV_PID")
port=$CF_DEV_PORT
base=http://127.0.0.1:$port
if [ "$up" != 0 ]; then
	notok "cf dev answers on port $port" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi

# The hub sandbox's projects, written by real mem and pushed to the store,
# with every project's evidence bytes in the Files store.
export MEM_NOTIFY_CMD=true
sb=$T_TMP/sb
mkdir -p "$sb"
# shellcheck source=../hub/tests/seed.sh
(source "$WF_ROOT/hub/tests/seed.sh") >/dev/null
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$port/store/share" "$token" \
	"$XDG_DATA_HOME/mem/store" "$base" 2>&1)" '^pushed [1-9][0-9]*$' \
	"the seeded store's files and evidence bytes reach the store"

# The first evidence item of gamma: its id and the file mem keeps for it.
listing=$(cd "$sb/gamma" && mem evidence list --project=gamma --json)
id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["items"][0]["id"])' <<<"$listing")
file=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["items"][0]["file"])' <<<"$listing")
seeded=$(ls "$XDG_DATA_HOME"/mem/store/projects/*/"$file")

fetch() { # fetch <path> [curl args...] -- prints the status; body in file.out, headers in file.hdr
	local path=$1
	shift
	curl -s -D "$T_TMP/file.hdr" -o "$T_TMP/file.out" -w '%{http_code}' "$@" "$base$path"
}
is "$(fetch "/p/gamma/file/$id" -H "@$T_TMP/jwt.hdr")" 200 "GET /p/gamma/file/<id> with a JWT answers 200"
is "$(sha256sum <"$T_TMP/file.out" | cut -d' ' -f1)" "$(sha256sum <"$seeded" | cut -d' ' -f1)" \
	"the response holds the seeded PNG's exact bytes"
hdr=$(tr -d '\r' <"$T_TMP/file.hdr")
like "$hdr" '[Cc]ontent-[Tt]ype: image/png' "the type is image/png"
like "$hdr" '[Cc]ache-[Cc]ontrol: private, max-age=3600' "the file is cached privately for an hour"
like "$hdr" '[Xx]-[Cc]ontent-[Tt]ype-[Oo]ptions: nosniff' "the file is sent with nosniff"

is "$(fetch "/p/gamma/file/01K0000000000000000000NONE" -H "@$T_TMP/jwt.hdr")" 404 "an unknown id is 404"
is "$(fetch "/p/beta/file/$id" -H "@$T_TMP/jwt.hdr")" 404 "an id of another project is 404"
is "$(fetch "/p/gamma/file/$id")" 401 "without a JWT the file is refused"
