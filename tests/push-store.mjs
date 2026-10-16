// push-store.mjs <ws url> <bearer> <store root> [http base] -- sends every text file of
// a mem store's projects to a hubweb store over its sharing socket, as one
// machine would, and waits for each write's ack. Evidence and raw files are
// bytes, which travel apart from the socket, so they stay behind, unless a
// fourth argument gives the store's http base: then each is PUT to its
// /store/files path with the same bearer.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const [url, token, root, httpBase] = process.argv.slice(2);
const files = [];
const blobs = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    const rel = relative(root, path);
    if (name.startsWith(".")) continue;
    const bytes = /\/(evidence|raw)(\/|$)/.test(rel);
    if (statSync(path).isDirectory()) walk(path);
    else (bytes ? blobs : files).push(rel);
  }
};
walk(join(root, "projects"));

const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } });
const acked = new Set();
let ready;
const done = new Promise((ok) => (ready = ok));
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.t === "ready") {
    files.forEach((path, n) => {
      const id = "01PUSH" + String(n).padStart(20, "0");
      ws.send(JSON.stringify({ t: "write", id, path, base: null, body: readFileSync(join(root, path), "utf8") }));
    });
    if (!files.length) ready();
  } else if (m.t === "ack") {
    acked.add(m.id);
    if (acked.size === files.length) ready();
  } else if (m.t !== "rec" && m.t !== "route") {
    console.error("push-store:", e.data);
  }
};
ws.onopen = () => ws.send(JSON.stringify({ t: "hello", machine: "seed", since: 0 }));
ws.onerror = () => { console.error("push-store: could not connect"); process.exit(1); };
await done;
if (httpBase) {
  for (const rel of blobs) {
    const res = await fetch(`${httpBase}/store/files/${rel.split("/").map(encodeURIComponent).join("/")}`, {
      method: "PUT",
      headers: { Authorization: `Bearer ${token}` },
      body: readFileSync(join(root, rel)),
    });
    if (!res.ok) { console.error("push-store: PUT", rel, res.status); process.exit(1); }
  }
}
console.log(`pushed ${files.length}`);
ws.close();
