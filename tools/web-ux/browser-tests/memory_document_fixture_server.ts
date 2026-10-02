import { extname, join, normalize } from "jsr:@std/path@1.1.4";

const port = Number(Deno.args[0]);
const buildRoot = Deno.args[1];
const sectionCount = Deno.args[2] === undefined ? 90 : Number(Deno.args[2]);
if (
  !Number.isInteger(port) || !buildRoot || !Number.isInteger(sectionCount) || sectionCount < 3 ||
  sectionCount > 100
) {
  throw new Error("usage: server.ts <port> <build-root> [section-count: 3..100]");
}

const workspaceId = "memory-review";
const paragraphs = Array.from(
  { length: sectionCount },
  (_, index) =>
    `## Section ${
      index + 1
    }\n\nThis is representative Memory content with **important context**, an identifier \`memory-item-${
      index + 1
    }\`, and a long URL https://example.com/${"long-segment-".repeat(12)}${
      index + 1
    }.\n\n> A durable note for section ${index + 1}.\n\n- first detail\n- second detail`,
).join("\n\n");
const bodyMd =
  `# Workspace Memory\n\nThis document is read-only.\n\n| Very wide heading | Another heading | Decision |\n| --- | --- | --- |\n| ${
    "wide-value-".repeat(16)
  } | stable | keep scrolling local |\n\n##### Deep document heading\n\n###### Deepest document heading\n\n\`\`\`rust\nfn example() { println!("${
    "wide-code-".repeat(18)
  }"); }\n\`\`\`\n\n[Safe link](https://example.com) [Unsafe link](javascript:alert(1))\n\n<img src=x onerror=alert(1)>\n\n${paragraphs}\n\n## T-661 UNIQUE END MARKER`;

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });

const mime: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
};

async function staticResponse(pathname: string): Promise<Response> {
  const relative = pathname === "/" ? "index.html" : pathname.replace(/^\/+/, "");
  const normalized = normalize(relative);
  if (normalized.startsWith("..")) return new Response("not found", { status: 404 });
  let filePath = join(buildRoot, normalized);
  try {
    const stat = await Deno.stat(filePath);
    if (stat.isDirectory) filePath = join(filePath, "index.html");
    return new Response(await Deno.readFile(filePath), {
      headers: { "content-type": mime[extname(filePath)] ?? "application/octet-stream" },
    });
  } catch {
    return new Response(await Deno.readFile(join(buildRoot, "index.html")), {
      headers: { "content-type": "text/html; charset=utf-8" },
    });
  }
}

Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  if (url.pathname === "/api/workspaces") {
    return json([{
      workspace_id: workspaceId,
      owner_account_id: "owner",
      display_name: "Memory Review",
      state: "active",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-02T00:00:00Z",
    }]);
  }
  if (url.pathname === `/api/w/${workspaceId}/workspace`) {
    return json({
      workspace_id: workspaceId,
      display_name: "Memory Review",
      record_authority: "fixture",
      schema_version: 1,
      auth: {
        Passkey: {
          rp_id: "127.0.0.1",
          origin: `http://127.0.0.1:${port}`,
          public_base_url: `http://127.0.0.1:${port}`,
          cookie_name: "fixture",
        },
      },
      permissions: {
        manage_repositories: false,
        manage_secrets: false,
        manage_runtimes: false,
        delete_workspace: false,
      },
      extension_points: {
        store: "fixture",
        event_stream: { status: "ready", note: "fixture", diagnostics: [] },
        host_worker_bridge: { status: "ready", note: "fixture", diagnostics: [] },
        companion_console: { status: "ready", note: "fixture", diagnostics: [] },
      },
    });
  }
  if (url.pathname === `/api/w/${workspaceId}/repositories`) {
    return json({ workspace_id: workspaceId, items: [], source: "fixture", diagnostics: [] });
  }
  if (url.pathname === `/api/w/${workspaceId}/working-directories`) {
    return json({ workspace_id: workspaceId, items: [], diagnostics: [] });
  }
  if (url.pathname === `/api/w/${workspaceId}/memory`) {
    return json({
      body_md: bodyMd,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-02T03:04:05Z",
      bytes: new TextEncoder().encode(bodyMd).byteLength,
      record_source: "sqlite_workspace_authority.memory_document",
    });
  }
  if (
    url.pathname === `/api/w/${workspaceId}/protocol/ws` &&
    request.headers.get("upgrade") === "websocket"
  ) {
    const { socket, response } = Deno.upgradeWebSocket(request);
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data));
      if (frame?.message?.method !== "subscribe_events") return;
      const requestId = frame.message.params.request_id;
      socket.send(JSON.stringify({
        protocol_version: 2,
        frame: "response",
        message: {
          result: "subscribed",
          payload: {
            request_id: requestId,
            subscription_id: "fixture-workers",
            selector: { topic: "workspace_workers" },
            snapshot: { topic: "workers", data: { workers: [] } },
          },
        },
      }));
    };
    return response;
  }
  return await staticResponse(url.pathname);
});
