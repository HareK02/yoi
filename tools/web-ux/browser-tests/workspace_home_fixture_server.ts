import { extname, join, normalize } from "jsr:@std/path@1.1.4";

const port = Number(Deno.args[0]);
const buildRoot = Deno.args[1];
if (!Number.isInteger(port) || !buildRoot) throw new Error("usage: server <port> <build-root>");
const names: Record<string, string> = {
  "home-owner": "Workspace Home Review",
  "home-member": "Shared Workspace",
  "home-long":
    "Workspace with a long name — international documentation and distributed development",
};
const json = (value: unknown, status = 200) => Response.json(value, { status });
const mime: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
};
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  if (url.pathname === "/api/workspaces") {
    return json(
      Object.entries(names).map(([workspace_id, display_name]) => ({
        workspace_id,
        display_name,
        owner_account_id: "fixture-owner",
        state: "active",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-02T00:00:00Z",
      })),
    );
  }
  const match = url.pathname.match(/^\/api\/w\/([^/]+)(\/.*)$/);
  if (match) {
    const [, workspaceId, path] = match;
    const owner = workspaceId !== "home-member";
    if (path === "/workspace") {
      return json({
        workspace_id: workspaceId,
        display_name: names[workspaceId],
        record_authority: "fixture",
        schema_version: 1,
        auth: {
          Passkey: {
            rp_id: "127.0.0.1",
            origin: url.origin,
            public_base_url: url.origin,
            cookie_name: "fixture",
          },
        },
        permissions: {
          manage_repositories: owner,
          manage_secrets: owner,
          manage_runtimes: owner,
          delete_workspace: owner,
        },
        extension_points: {
          store: "fixture",
          event_stream: { status: "ready", note: "fixture", diagnostics: [] },
          host_worker_bridge: { status: "ready", note: "fixture", diagnostics: [] },
          companion_console: { status: "ready", note: "fixture", diagnostics: [] },
        },
      });
    }
    if (path === "/repositories") {
      return json({ workspace_id: workspaceId, items: [], source: "fixture", diagnostics: [] });
    }
    if (path === "/working-directories") {
      return json({ workspace_id: workspaceId, items: [], diagnostics: [] });
    }
    // Retained only for before-change captures. The redesigned Home must not request Hosts.
    if (path === "/hosts") {
      return json({
        workspace_id: workspaceId,
        limit: 100,
        source: "fixture",
        diagnostics: [],
        items: workspaceId === "home-member" ? [] : [{
          runtime_id: "fixture-runtime",
          host_id: "fixture-host",
          label: "Development host",
          kind: "local",
          status: "available",
          observed_at: "2026-01-02T00:00:00Z",
          last_seen_at: null,
          os: "linux",
          arch: "x86_64",
          diagnostics: [],
        }],
      });
    }
    if (path === "/memory") {
      return json({
        body_md: "# Home navigation destination",
        bytes: 29,
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-02T00:00:00Z",
        record_source: "fixture",
      });
    }
    if (path === "/protocol/ws" && request.headers.get("upgrade") === "websocket") {
      const { socket, response } = Deno.upgradeWebSocket(request);
      socket.onmessage = (event) => {
        const frame = JSON.parse(String(event.data));
        if (frame?.message?.method !== "subscribe_events") return;
        socket.send(JSON.stringify({
          protocol_version: 1,
          frame: "response",
          message: {
            result: "subscribed",
            payload: {
              request_id: frame.message.params.request_id,
              subscription_id: "fixture-workers",
              selector: { topic: "workspace_workers" },
              snapshot_revision: 1,
              snapshot: { topic: "workers", data: { workers: [] } },
            },
          },
        }));
      };
      return response;
    }
    return json({ error: "Unexpected fixture API request" }, 404);
  }
  const relative = normalize(url.pathname.replace(/^\/+/, "") || "index.html");
  if (relative.startsWith("..")) return new Response("not found", { status: 404 });
  let file = join(buildRoot, relative);
  try {
    if ((await Deno.stat(file)).isDirectory) file = join(file, "index.html");
    return new Response(await Deno.readFile(file), {
      headers: { "content-type": mime[extname(file)] ?? "application/octet-stream" },
    });
  } catch {
    return new Response(await Deno.readFile(join(buildRoot, "index.html")), {
      headers: { "content-type": mime[".html"] },
    });
  }
});
