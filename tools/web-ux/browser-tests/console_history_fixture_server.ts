import { extname, join, normalize } from "jsr:@std/path@1.1.4";

const port = Number(Deno.args[0]);
const buildRoot = Deno.args[1];
if (!Number.isInteger(port) || !buildRoot) {
  throw new Error("usage: console_history_fixture_server.ts <port> <build-root>");
}

const workspaceId = "console-history-review";
const runtimeId = "runtime-a";
const workerId = "worker-a";
const workerResourceKey = "W-900";
let historyRequests = 0;

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });

const worker = {
  worker_id: workerId,
  runtime_id: runtimeId,
  resource_key: workerResourceKey,
  display_name: "Console Fixture",
  label: "Console Fixture",
  availability: "observed",
  state: "stopped",
  retention_state: "retained",
  host_id: "fixture-host",
  profile: "default",
  tags: [],
  diagnostics: [],
  workspace: {
    workspace_id: workspaceId,
    visibility: "workspace",
    identity: workspaceId,
  },
  implementation: { kind: "builtin", display_hint: "fixture" },
  workdir_attachments: [],
};

function historyEntry(index: number, role: "user" | "assistant") {
  const user = role === "user";
  return {
    entry_id: `${role}-${index}`,
    timestamp: index,
    provenance: user ? "human_input" : "model_output",
    kind: "message",
    role,
    content: [{
      kind: "text",
      text: user
        ? `question ${index}`
        : `done ${index}\ndetail ${index}.1\ndetail ${index}.2\ndetail ${index}.3`,
    }],
  };
}

function historyTurnEntries(index: number) {
  const tools = ["Bash", "Grep"].flatMap((name) => {
    const callId = `${name}-${index}`;
    return [{
      kind: "tool_call", entry_id: `call-${callId}`, timestamp: index,
      provenance: "model_output", call_id: callId, name, arguments: "{}",
    }, {
      kind: "tool_result", entry_id: `result-${callId}`, timestamp: index,
      provenance: "tool_output", call_id: callId, summary: "done", content: "done", is_error: false,
    }];
  });
  return [historyEntry(index, "user"), ...tools, historyEntry(index, "assistant")];
}

function historyPage(indices: number[], cursor: string | null) {
  return {
    availability: "page",
    page: {
      session_id: "session-a",
      lineage_id: "lineage-a",
      compact_ancestor_lineage_ids: [],
      turns: indices.map((index) => ({
        turn_id: `user-${index}`,
        entries: historyTurnEntries(index),
      })),
      next_cursor: cursor,
      has_more: cursor !== null,
    },
  };
}

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
  if (url.pathname === "/fixture-state") return json({ history_requests: historyRequests });
  if (url.pathname === "/api/workspaces") {
    return json([{
      workspace_id: workspaceId,
      owner_account_id: "owner",
      display_name: "Console History Review",
      state: "active",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-02T00:00:00Z",
    }]);
  }
  if (url.pathname === `/api/w/${workspaceId}/workspace`) {
    return json({
      workspace_id: workspaceId,
      display_name: "Console History Review",
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
  if (url.pathname === `/api/w/${workspaceId}/workers`) {
    return json({ workspace_id: workspaceId, limit: 200, items: [worker], source: "fixture", diagnostics: [] });
  }
  if (
    url.pathname === `/api/w/${workspaceId}/workers/${workerResourceKey}` ||
    url.pathname === `/api/w/${workspaceId}/runtimes/${runtimeId}/workers/${workerId}`
  ) {
    return json(worker);
  }
  if (
    url.pathname ===
      `/api/w/${workspaceId}/runtimes/${runtimeId}/workers/${workerId}/session`
  ) {
    const pendingMode = new URL(request.headers.get("referer") ?? request.url).searchParams.get("pending");
    const hasQueue = pendingMode === "both" || pendingMode === "queue";
    const hasNotifications = pendingMode === "both" || pendingMode === "notifications" || pendingMode === "legacy";
    const submissions = hasQueue ? [
      { submission_id: "queued-1", preview: "First queued input", accepted_at_ms: 1, segment_count: 1, byte_len: 18 },
      { submission_id: "queued-2", preview: "Long queued input — " + "preview text ".repeat(30), accepted_at_ms: 2, segment_count: 1, byte_len: 500 },
    ] : [];
    const taskEntries = pendingMode ? [{
      kind: "tool_call", entry_id: "fixture-task", timestamp: 13, provenance: "model_output",
      call_id: "fixture-task", name: "TaskCreate",
      arguments: JSON.stringify({ subject: "Inspect pending input layout", description: "Synthetic visual fixture" }),
    }] : [];
    return json({
      availability: "retained_snapshot",
      identity: { session_id: "session-a", segment_id: "segment-a", entry_count: 36 },
      snapshot: {
        pending_submissions: {

          notification_count: hasNotifications ? 2 : 0,
          ...(hasNotifications && pendingMode !== "legacy" ? { notification_previews: ["First notification", "Long notification — " + "notification text ".repeat(30)] } : {}),
          head_id: submissions[0]?.submission_id ?? null,
          submissions,
        },
        entries: [...[7, 8, 9, 10, 11, 12].flatMap(historyTurnEntries), ...taskEntries],
      },
    });
  }
  if (
    url.pathname ===
      `/api/w/${workspaceId}/runtimes/${runtimeId}/workers/${workerId}/session/history`
  ) {
    historyRequests += 1;
    switch (url.searchParams.get("cursor")) {
      case null:
        return json(historyPage([8, 9, 10, 11, 12], "older-8"));
      case "older-8":
        return json(historyPage([3, 4, 5, 6, 7], "older-3"));
      case "older-3":
        return json(historyPage([1, 2], null));
      default:
        return json({ availability: "unavailable", reason: "invalid_cursor", message: "invalid" });
    }
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
            selector: frame.message.params.selector,
            snapshot: { topic: "workers", data: { workers: [] } },
          },
        },
      }));
    };
    return response;
  }
  return await staticResponse(url.pathname);
});
