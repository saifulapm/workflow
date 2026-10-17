#!/usr/bin/env bash
# The bar's route file comes from the local hub alone: a real `hub local`
# writes $XDG_RUNTIME_DIR/workflow-route.json at start, `workflow route`
# changes it through the hub's socket, a peer's newer route rewrites it, and
# Dulali is no longer a route.
#
# Needs the hub's release binary.

source "$(dirname -- "$0")/lib.sh"
t_init

HUB_BIN=${HUB_BIN:-$WF_ROOT/hub/target/release/hub}
free_port() {
	python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
port=$(free_port)
file=$XDG_RUNTIME_DIR/workflow-route.json
export HUB_LOCAL_SOCKET=$XDG_STATE_HOME/hub/local.sock

HUB_PEER_TOKEN=peer-token HUB_PEERS= HUB_HOST=mini \
	"$HUB_BIN" local --port "$port" >>"$T_TMP/hub.log" 2>&1 &
hub=$!
trap 'kill $hub 2>/dev/null; t_done' EXIT
for _ in $(seq 1 100); do [ -S "$HUB_LOCAL_SOCKET" ] && [ -f "$file" ] && break; sleep 0.1; done

field() { python3 -c 'import json,sys; v=json.load(open(sys.argv[1])).get(sys.argv[2]); print("null" if v is None else v)' "$file" "$1"; }

is "$(field route)" phone "the hub writes the route file at start: phone"
is "$(field self)" "mini:$port" "the file names this hub as self"

is "$(workflow route device)" "route: device (mini:$port)" "workflow route device prints this machine"
is "$(field route)" device "the file follows to device"
is "$(field machine)" "$(field self)" "the device is this hub"

curl -s -o /dev/null -H 'Authorization: Bearer peer-token' -H 'Content-Type: application/json' \
	-d '{"route":"device","machine":"nuc","at":99999999999999}' "http://127.0.0.1:$port/route"
is "$(field machine)" nuc "a peer's newer route rewrites the file"
is "$(workflow route)" "route: device (nuc)" "workflow route reads the hub's route"

workflow route phone >/dev/null
is "$(field route)" phone "the file follows back to phone"

run workflow route dulali
is "$RC" 2 "dulali is not a route"
is "$(field route)" phone "a refused route leaves the file alone"
