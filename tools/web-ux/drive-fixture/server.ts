// Serves the real static production shell. Drive is an in-memory fake API;
// other routes reuse the shared Home/settings fixtures via a loopback proxy.
import { dirname, fromFileUrl, join } from "jsr:@std/path@1.1.4";
import { createDriveFixture } from "./api.ts";
import { cleanupPlan, workerCatalog, workerSnapshot } from "./workers.ts";
const port = Number(Deno.args[0]);
const buildRoot = Deno.args[1];
if (!Number.isInteger(port) || !buildRoot) {
  throw new Error("usage: server <port> <production-build-root>");
}
const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
const homePort = (listener.addr as Deno.NetAddr).port;
listener.close();
const home = new Deno.Command(Deno.execPath(), {
  args: [
    "run",
    "--allow-net",
    "--allow-read",
    join(root, "tools/web-ux/browser-tests/workspace_home_fixture_server.ts"),
    String(homePort),
    buildRoot,
  ],
  stdout: "null",
  stderr: "inherit",
}).spawn();
let stopped = false;
function cleanup() {
  if (!stopped) {
    stopped = true;
    try {
      home.kill("SIGTERM");
    } catch { /* already exited */ }
  }
}
Deno.addSignalListener("SIGTERM", () => {
  cleanup();
  Deno.exit(0);
});
Deno.addSignalListener("SIGINT", () => {
  cleanup();
  Deno.exit(0);
});
const fixture = createDriveFixture();
const deadline = Date.now() + 10000;
while (true) {
  try {
    const response = await fetch(`http://127.0.0.1:${homePort}/health`);
    await response.body?.cancel();
    if (response.ok) break;
  } catch { /* startup */ }
  if (Date.now() > deadline) {
    cleanup();
    throw new Error("shared Home fixture failed to start");
  }
  await new Promise((resolve) => setTimeout(resolve, 30));
}
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") {
    return Response.json({ authority: "fixture, not actual Backend", ready: true });
  }
  const drive = await fixture.handler(request);
  if (drive) return drive;
  const api = url.pathname.match(/^\/api\/w\/([^/]+)(\/.*)$/);
  if (api?.[2] === "/workers") {
    if (api[1] === "workers-error") {
      return Response.json({ error: "Worker fixture unavailable" }, { status: 503 });
    }
    return Response.json(workerCatalog(decodeURIComponent(api[1])));
  }
  if (api?.[2].match(/^\/runtimes\/[^/]+\/cleanup-plan$/)) {
    return Response.json(cleanupPlan(decodeURIComponent(api[1]), api[2].split("/")[2]));
  }
  // Home fixture keeps the production WebSocket subscription flow intact.
  if (api?.[2] === "/protocol/ws" && request.headers.get("upgrade") === "websocket") {
    const { socket, response } = Deno.upgradeWebSocket(request);
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data));
      if (frame?.message?.method !== "subscribe_events") return;
      socket.send(JSON.stringify({
        protocol_version: 2,
        frame: "response",
        message: {
          result: "subscribed",
          payload: {
            request_id: frame.message.params.request_id,
            subscription_id: "fixture-workers",
            selector: { topic: "workspace_workers" },
            snapshot: {
              topic: "workers",
              data: { workers: workerSnapshot(decodeURIComponent(api[1])) },
            },
          },
        },
      }));
    };
    return response;
  }
  const proxy = new URL(url.pathname + url.search, `http://127.0.0.1:${homePort}`);
  const response = await fetch(proxy, {
    method: request.method,
    headers: request.headers,
    body: request.method === "GET" || request.method === "HEAD"
      ? undefined
      : await request.arrayBuffer(),
    redirect: "manual",
  });
  if (api?.[2] === "/workspace") {
    const body = await response.json();
    body.display_name ??= `Drive ${decodeURIComponent(api[1])}`;
    return Response.json(body, { status: response.status });
  }
  return response;
});
