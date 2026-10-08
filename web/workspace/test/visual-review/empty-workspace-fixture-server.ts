// Extends the existing Web UX fixture at the API boundary, never the production shell/CSS.
// Both loopback servers are owned and cleaned up by the Web UX capture harness.
const port = Number(Deno.args[0]);
const upstream = Deno.args[1];
if (!Number.isInteger(port) || !upstream) {
  throw new Error("usage: server <port> <upstream-url>");
}

import { emptyLaunchOptions } from "../../src/lib/workspace/sidebar/worker-launch.test-fixtures.ts";
import { dashboardFixture } from "../../src/lib/workspace/home/dashboard.test-fixtures.ts";

Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/api/workspaces" && request.method === "POST") {
    return Response.json({
      message:
        "Workspace display name was rejected by Backend validation. Choose another display name and retry; no Workspace was created.",
    }, { status: 400 });
  }
  const match = url.pathname.match(/^\/api\/w\/([^/]+)(\/.*)$/);
  if (match) {
    const [, id, path] = match;
    // The shared catalog includes a deliberate failure persona, which can be hover-preloaded.
    // This scenario tests zero repositories, not that separate failure contract.
    if (id === "home-error" && path === "/merge-requests") {
      return Response.json(
        dashboardFixture(path, url.searchParams, "home-empty"),
      );
    }
    if (path === "/workers") {
      return Response.json({
        workspace_id: id,
        limit: 100,
        items: [],
        source: "fixture",
        diagnostics: [],
      });
    }
    if (path === "/workers/launch-options") {
      return Response.json(emptyLaunchOptions(id));
    }
  }
  // Forward HTTP and bridge the existing workspace subscription fixture.
  const target = new URL(url.pathname + url.search, upstream);
  if (request.headers.get("upgrade") === "websocket") {
    const { socket, response } = Deno.upgradeWebSocket(request);
    target.protocol = "ws:";
    const peer = new WebSocket(target);
    const pending: string[] = [];
    socket.onmessage = (event) =>
      peer.readyState === WebSocket.OPEN
        ? peer.send(event.data)
        : pending.push(String(event.data));
    peer.onopen = () => {
      for (const frame of pending) peer.send(frame);
    };
    peer.onmessage = (event) => {
      if (socket.readyState === WebSocket.OPEN) socket.send(event.data);
    };
    socket.onclose = () => peer.close();
    peer.onclose = () => socket.close();
    return response;
  }
  return await fetch(new Request(target, request));
});
