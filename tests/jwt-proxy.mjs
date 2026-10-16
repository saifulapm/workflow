// jwt-proxy.mjs <cf dev port> <jwt> -- listens on a free port and forwards
// every request and every WebSocket upgrade to 127.0.0.1:<cf dev port> with
// Cloudflare Access's Cf-Access-Jwt-Assertion added. A browser cannot put a
// header on its own WebSocket, so a test points the browser here instead.
// Prints `proxy <port>` once it listens.
import http from "node:http";
import net from "node:net";

const [target, jwt] = process.argv.slice(2);

const server = http.createServer((req, res) => {
  const up = http.request(
    { host: "127.0.0.1", port: target, path: req.url, method: req.method,
      headers: { ...req.headers, host: `127.0.0.1:${target}`, "cf-access-jwt-assertion": jwt } },
    (back) => { res.writeHead(back.statusCode, back.headers); back.pipe(res); },
  );
  up.on("error", () => { res.writeHead(502); res.end(); });
  req.pipe(up);
});

server.on("upgrade", (req, client, head) => {
  const up = net.connect(Number(target), "127.0.0.1", () => {
    const lines = [`${req.method} ${req.url} HTTP/1.1`];
    for (let i = 0; i < req.rawHeaders.length; i += 2) {
      const name = req.rawHeaders[i];
      if (/^(host|cf-access-jwt-assertion)$/i.test(name)) continue;
      lines.push(`${name}: ${req.rawHeaders[i + 1]}`);
    }
    lines.push(`Host: 127.0.0.1:${target}`, `Cf-Access-Jwt-Assertion: ${jwt}`);
    up.write(lines.join("\r\n") + "\r\n\r\n");
    if (head.length) up.write(head);
    up.pipe(client);
    client.pipe(up);
  });
  up.on("error", () => client.destroy());
  client.on("error", () => up.destroy());
});

server.listen(0, "127.0.0.1", () => console.log(`proxy ${server.address().port}`));
