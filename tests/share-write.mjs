// share-write.mjs <ws url> <bearer> <store path> <file> -- writes <file> to
// <store path> over the store's sharing socket, as a machine would, and
// prints `sent <ms since epoch>` once the store acks the write.
import { readFileSync } from "node:fs";

const [url, token, path, file] = process.argv.slice(2);
const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } });
ws.onerror = () => { console.error("share-write: could not connect"); process.exit(1); };
ws.onopen = () => ws.send(JSON.stringify({ t: "hello", machine: "writer", since: 0 }));
let sent = 0;
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.t === "ready") {
    sent = Date.now();
    ws.send(JSON.stringify({ t: "write", id: "01WRITE" + sent, path, base: null, body: readFileSync(file, "utf8") }));
  } else if (m.t === "ack") {
    console.log(`sent ${sent}`);
    process.exit(0);
  }
};
