#!/usr/bin/env bash
# hubweb's sharing socket under `cf dev`: GET /store/share with the bearer
# STORE_TOKEN upgrades to a WebSocket held by the store, which numbers each
# mem write, acks it to the machine that sent it, streams it to every other
# open socket, acks a resent id with its first seq, refuses a page write made
# from an old version, and replays every write after `since` on hello.
#
# Needs hubweb/node_modules (pnpm -C hubweb install --frozen-lockfile),
# worker-build, the wasm32-unknown-unknown target, python3 and node 22 or
# later, whose global WebSocket takes handshake headers.
#
# As in t140, cf dev runs in a copy of hubweb inside T_TMP so its state goes
# with the sandbox, and STORE_TOKEN reaches it through the environment.

# The real cache dir, read before t_init moves HOME: worker-build keeps
# wasm-bindgen, wasm-opt and esbuild there.
real_cache=${XDG_CACHE_HOME:-$HOME/.cache}
source "$(dirname -- "$0")/lib.sh"
t_init

HUBWEB=$WF_ROOT/hubweb
if [ ! -x "$HUBWEB/node_modules/.bin/cf" ]; then
	notok "hubweb/node_modules holds cf and wrangler" "run: pnpm -C hubweb install --frozen-lockfile"
	exit 1
fi

dev_pid=
stop_dev() {
	local status=$?
	[ -n "$dev_pid" ] && kill -TERM -- "-$dev_pid" 2>/dev/null && wait "$dev_pid" 2>/dev/null
	(exit "$status")
	t_done
}
trap stop_dev EXIT

token=test-store-token
app=$T_TMP/hubweb
t_hubweb_copy "$app"
t_cf_dev "$app" "$T_TMP/dev.log" STORE_TOKEN=$token
up=$?
dev_pid=$CF_DEV_PID
port=$CF_DEV_PORT
base=http://127.0.0.1:$port
if [ "$up" != 0 ]; then
	notok "cf dev answers on port $port" "$(tail -40 "$T_TMP/dev.log")"
	exit 1
fi

upgrade() { # upgrade [curl args...] -- prints the status of a WebSocket handshake
	curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$@" \
		-H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
		-H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' "$base/store/share"
}
is "$(upgrade)" 401 "/store/share without a bearer is refused"
is "$(upgrade -H "Authorization: Bearer wrong-$token")" 401 "/store/share with a wrong bearer is refused"

# The machines: A, B and C connect and say hello, A writes, B writes a new
# page A already made, then D joins from nothing. Each line it prints is
# `<name> <json>` for one fact the checks below read, with each object's keys
# sorted so the checks do not depend on the order the store sends them in.
cat >"$T_TMP/share.mjs" <<'JS'
const [url, token] = process.argv.slice(2);
const sorted = (_, v) =>
  v && typeof v === "object" && !Array.isArray(v)
    ? Object.fromEntries(Object.entries(v).sort(([x], [y]) => (x < y ? -1 : 1)))
    : v;
const say = (name, value) => console.log(name, JSON.stringify(value, sorted));

function open(machine, since) {
  const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } });
  const inbox = [];
  let wake = null;
  ws.onmessage = (e) => { inbox.push(JSON.parse(e.data)); wake?.(); };
  // The next message within `ms`, or null.
  const next = async (ms = 2000) => {
    const until = Date.now() + ms;
    while (!inbox.length && Date.now() < until) {
      await new Promise((r) => { wake = r; setTimeout(r, until - Date.now()); });
    }
    return inbox.shift() ?? null;
  };
  const opened = new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = () => fail(new Error(`${machine} did not connect`)); });
  const send = (m) => ws.send(JSON.stringify(m));
  return { ws, next, send, ready: opened.then(() => send({ t: "hello", machine, since })) };
}

const item = "projects/P1/items/01ITEM.md";
const page = "projects/P1/wiki/lessons.md";
const [a, b, c] = [open("mac", 0), open("nuc", 0), open("air", 0)];
await Promise.all([a.ready, b.ready, c.ready]);
say("hello", await Promise.all([a.next(), b.next(), c.next()]));

a.send({ t: "write", id: "01W1", path: item, base: null, body: "first item" });
say("ack", await a.next());
say("rec-b", await b.next());
say("rec-c", await c.next());
say("a-after", await a.next(500));

a.send({ t: "write", id: "01W1", path: item, base: null, body: "first item" });
say("resend", await a.next());
say("b-after-resend", await b.next(500));

a.send({ t: "write", id: "01P1", path: page, base: null, body: "lessons v1" });
say("page", await a.next());
await b.next();
b.send({ t: "write", id: "01P2", path: page, base: null, body: "lessons elsewhere" });
say("stale", await b.next());

const d = open("mini", 0);
await d.ready;
const replay = [];
for (let m = await d.next(); m; m = m.t === "ready" ? null : await d.next()) replay.push(m);
say("replay", replay.map((m) => m.t === "rec" ? [m.seq, m.id, m.body, m.machine] : [m.t, m.seq]));
for (const s of [a, b, c, d]) s.ws.close();
JS
ws_url=ws://127.0.0.1:$port/store/share
node "$T_TMP/share.mjs" "$ws_url" "$token" >"$T_TMP/share.out" 2>&1
fact() { sed -n "s/^$1 //p" "$T_TMP/share.out"; }

is "$(fact hello)" '[{"seq":0,"t":"ready"},{"seq":0,"t":"ready"},{"seq":0,"t":"ready"}]' \
	"three sockets with the bearer connect and hello an empty store"
is "$(fact ack)" '{"id":"01W1","seq":1,"t":"ack"}' "A's first write is acked as seq 1"
rec='{"body":"first item","id":"01W1","machine":"mac","path":"projects/P1/items/01ITEM.md","seq":1,"t":"rec"}'
is "$(fact rec-b)" "$rec" "B gets A's write as a rec within 2 s"
is "$(fact rec-c)" "$rec" "C gets A's write as a rec within 2 s"
is "$(fact a-after)" null "A does not get its own write back"
is "$(fact resend)" '{"id":"01W1","seq":1,"t":"ack"}' "a resent id is acked with its first seq"
is "$(fact b-after-resend)" null "B gets no second rec for the resend"
is "$(fact page)" '{"id":"01P1","seq":2,"t":"ack"}' "a new page is stored as seq 2"
is "$(fact stale)" \
	'{"head":{"body":"lessons v1","id":"01P1","path":"projects/P1/wiki/lessons.md","seq":2},"id":"01P2","t":"refused"}' \
	"a page write from an old version is refused with the head"
is "$(fact replay)" '[[1,"01W1","first item","mac"],[2,"01P1","lessons v1","mac"],["ready",2]]' \
	"a fourth socket's hello since 0 replays every rec in seq order, then ready"
unlike "$(grep -v '^[a-z-]* ' "$T_TMP/share.out")" . "the client printed only facts, no errors"

records='[{"project":"walkone","kind":"handoff","key":"walkone","body":"Next: share."}]'
like "$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer $token" \
	-H 'Content-Type: application/json' --data "$records" "$base/store/records")" '^2[0-9][0-9]$' \
	"PUT /store/records still stores records"
