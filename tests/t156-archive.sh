#!/usr/bin/env bash
# Old files leave Cloudflare under `cf dev`: each file PUT to the store is
# listed by GET /store/files?project=<id> with its upload time, and `hub
# share archive <tarball>`, with its clock moved 90 days past a point between
# two uploads,
#
#   - deletes an old file the `mem snapshot` tarball holds, which GET then
#     answers 404;
#   - fetches into the replica an old file the tarball lacks and the replica
#     no longer holds, and leaves it in the store;
#   - leaves a younger file alone;
#
# and the hub web's evidence page shows the old file's row as archived.
#
# Needs what t145 needs (hubweb/node_modules, worker-build, the wasm target,
# openssl, python3) and mem's and the hub's release binaries, which
# tests/run.sh builds.

real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
HUB_BIN=${HUB_BIN:-$WF_ROOT/hub/target/release/hub}
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
	exit 1
fi
if [ ! -x "$HUB_BIN" ]; then
	notok "the hub binary is built" "missing $HUB_BIN"
	exit 1
fi

free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
now_ms() { date +%s%3N; }

# The Access team, as in t145: a key, its JWKS, a server for it and a JWT.
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
python3 - "$T_TMP/key.pem" "$aud" "$team" "$(date +%s)" >"$T_TMP/jwt.hdr" <<'PY'
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

get() { # get <url> <out> -- prints the status
	curl -s -o "$2" -w '%{http_code}' --max-time 60 -H "Authorization: Bearer $token" "$1"
}
sha() { sha256sum "$1" | cut -d' ' -f1; }

# This machine's store: one project with three evidence files.
export MEM_NOTIFY_CMD=true
PROJECT=01K0ARCH1VE000000000000000
store=$XDG_DATA_HOME/mem/store
dir=$store/projects/$PROJECT
mkdir -p "$dir/items"
cat >"$dir/project.toml" <<-TOML
	created = "2026-01-01T00:00:00Z"
	id = "$PROJECT"
	name = "arch"
TOML
evidence() { # evidence <name> <note> -- prints the new item's id
	head -c 200000 /dev/urandom >"$T_TMP/$1"
	mem --project arch evidence add --task t1 "$T_TMP/$1" --note "$2" --json |
		python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'
}
old_id=$(evidence old.png 'the old shot')
evidence lost.png 'the lost shot' >/dev/null
evidence young.png 'the young shot' >/dev/null
# The old shot's item was filed long ago, so its row is archived.
sed -i 's/^created = .*/created = "2026-01-01T00:00:00Z"/' "$dir/items/$old_id.md"

start=$(now_ms)
cd "$T_TMP" || exit 1
like "$(node "$WF_ROOT/tests/push-store.mjs" "ws://127.0.0.1:$port/store/share" "$token" \
	"$store" "$base" 2>&1)" '^pushed [1-9][0-9]*$' "the store's items and files reach the store"
sleep 1
mid=$(now_ms)
sleep 1
files=$base/store/files/projects/$PROJECT/evidence/t1
is "$(curl -s -o /dev/null -w '%{http_code}' -X PUT --data-binary "@$T_TMP/young.png" \
	-H "Authorization: Bearer $token" "$files/young.png")" 204 "the young shot is uploaded again"
end=$(now_ms)

is "$(get "$base/store/files?project=$PROJECT" "$T_TMP/list.json")" 200 "the project's files are listed"
is "$(python3 - "$T_TMP/list.json" "$start" "$mid" "$end" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
start, mid, end = map(int, sys.argv[2:5])
for row in rows:
    at = row["at"]
    when = "old" if isinstance(at, int) and start <= at < mid else \
        "young" if isinstance(at, int) and mid < at <= end else repr(at)
    print(row["path"].rsplit("/", 1)[1], when)
PY
)" "lost.png old
old.png old
young.png young" "the listing names each file with its upload time in ms"
is "$(get "$base/store/files?project=../x" /dev/null)" 400 "a listing of a project that is no id is 400"
is "$(curl -s -o /dev/null -w '%{http_code}' "$base/store/files?project=$PROJECT")" 401 \
	"a listing with no bearer is 401"

# The replica no longer holds the lost shot, so the snapshot lacks it too.
rm "$dir/evidence/t1/lost.png"
tarball=$(mem snapshot)
listing=$(tar -tzf "$tarball")
like "$listing" "projects/$PROJECT/evidence/t1/old.png" "the snapshot holds the old shot"
unlike "$listing" "lost.png" "the snapshot lacks the lost shot"

# 90 days on from between the two uploads.
python3 -c 'import datetime, sys
at = datetime.datetime.fromtimestamp(int(sys.argv[1]) / 1000, datetime.timezone.utc)
print((at + datetime.timedelta(days=90)).isoformat(timespec="milliseconds").replace("+00:00", "Z"))' \
	"$mid" >"$T_TMP/now"
out=$(HUB_SHARE_NOW_FILE=$T_TMP/now HUB_STORE_URL="ws://127.0.0.1:$port/store/share" \
	HUB_STORE_TOKEN=$token "$HUB_BIN" share archive "$tarball" 2>&1)
is "$?" 0 "hub share archive exits 0"
like "$out" '2 files older than 90 days; 1 left the store, 1 fetched into the replica, 0 failed' \
	"hub share archive counts what it did"

is "$(get "$files/old.png" /dev/null)" 404 "the old shot the snapshot holds has left the store"
is "$(curl -s -o /dev/null -w '%{http_code}' -X DELETE -H "Authorization: Bearer $token" \
	"$files/old.png")" 404 "deleting it again is 404"
is "$(sha "$dir/evidence/t1/lost.png")" "$(sha "$T_TMP/lost.png")" \
	"the lost shot is fetched back into the replica byte for byte"
is "$(get "$files/lost.png" /dev/null)" 200 "the lost shot stays in the store"
is "$(get "$files/young.png" "$T_TMP/young.back")" 200 "the young shot stays in the store"
is "$(sha "$T_TMP/young.back")" "$(sha "$T_TMP/young.png")" "the young shot reads back the same"

page=$(curl -s -H "@$T_TMP/jwt.hdr" "$base/p/arch/evidence")
like "$page" 'old\.png<div class="meta">archived · the old shot' "the old shot's evidence row says archived"
unlike "$page" 'archived · the young shot' "the young shot's row is not archived"
is "$(curl -s -o "$T_TMP/file.out" -w '%{http_code}' -H "@$T_TMP/jwt.hdr" "$base/p/arch/file/$old_id")" \
	404 "the old shot's link answers 404"
is "$(cat "$T_TMP/file.out")" archived "and says archived"
