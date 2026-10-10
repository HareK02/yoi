import { extname, join, normalize } from "jsr:@std/path@1.1.4";

// Synthetic authority, real production shell and protocol transport. No UI overrides.
const port = Number(Deno.args[0]);
const buildRoot = Deno.args[1] ?? Deno.env.get("WEB_UX_BUILD_ROOT");
if (!Number.isInteger(port) || !buildRoot) {
  throw new Error("usage: console_live_fixture_server.ts <port> <build-root>");
}
const workspaceId = "console-live-review";
const runtimeId = "runtime-a";
const workerId = "worker-a";
const workerPath = `/api/w/${workspaceId}/runtimes/${runtimeId}/workers/${workerId}`;
const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });
const invocations:
  import("../../../web/workspace/src/lib/generated/protocol.ts").FeatureInvocationDescriptor[] = [
    {
      identity: "fixture.review",
      name: "review",
      aliases: ["rv"],
      display_name: "Review",
      description: "Review a synthetic document without running a real Worker.",
      syntax: "parenthesized",
      arguments: [
        {
          name: "mode",
          position: 0,
          required: true,
          value_type: { kind: "enum", values: ["brief", "thorough"] },
          completion: { kind: "static", values: ["brief", "thorough"] },
          description: "Review detail",
        },
        {
          name: "count",
          position: 1,
          required: false,
          value_type: { kind: "integer" },
          completion: { kind: "none" },
          description: "Number of examples",
        },
        {
          name: "enabled",
          position: 2,
          required: false,
          value_type: { kind: "boolean" },
          completion: { kind: "static", values: ["true", "false"] },
          description: "Enable review",
        },
        {
          name: "path",
          position: 3,
          required: false,
          value_type: { kind: "worker_file" },
          completion: { kind: "worker_file" },
          description: "Worker document",
        },
      ],
    },
    {
      identity: "fixture.attach",
      name: "attach",
      aliases: [],
      display_name: "Attach file",
      description: "Stage a client file using the attachment adapter.",
      syntax: "parenthesized",
      arguments: [{
        name: "file",
        position: 0,
        required: true,
        value_type: { kind: "client_file" },
        completion: { kind: "client_file" },
      }],
      client_adapter: "attachment",
    },
  ];
const entries = [
  {
    kind: "message",
    entry_id: "user-1",
    timestamp: 1,
    provenance: "human_input",
    role: "user",
    content: [{ kind: "text", text: "Review the release notes before sending." }],
  },
  {
    kind: "message",
    entry_id: "assistant-1",
    timestamp: 2,
    provenance: "model_output",
    role: "assistant",
    content: [{
      kind: "text",
      text:
        "Ready. Choose a Feature and inspect its arguments, or attach a document. Nothing runs until Submit.",
    }],
  },
];
const session = {
  pending_submissions: { notification_count: 0, head_id: null, submissions: [] },
  entries,
};
const state = { last_command_id: 0, state: { kind: "idle" } };
const worker = {
  worker_id: workerId,
  runtime_id: runtimeId,
  resource_key: "W-901",
  display_name: "Console Live Fixture",
  label: "Console Live Fixture",
  availability: "observed",
  state: "idle",
  worker_state: state,
  retention_state: "active",
  host_id: "fixture-host",
  profile: "default",
  tags: [],
  diagnostics: [],
  workspace: { workspace_id: workspaceId, visibility: "workspace", identity: workspaceId },
  implementation: { kind: "builtin", display_hint: "fixture" },
  workdir_attachments: [],
};
const snapshot = {
  event: "snapshot",
  data: {
    session,
    state,
    greeting: {
      worker_name: worker.label,
      cwd: "/synthetic/workdir",
      provider: "fixture",
      model: "synthetic",
      scope_summary: "synthetic read-only files",
      tools: [],
      context_window: 200000,
      context_tokens: 2400,
    },
  },
};
const methods: unknown[] = [];
const requests: { method: string; path: string }[] = [];
const events: unknown[] = [];
let submitMode: "accept" | "reject" | "hold" = "accept";
let uploadMode: "normal" | "fail" | "hold" = "normal";
const pendingAcceptances: (() => void)[] = [];
const cancelledUploads = new Set<string>();
const heldUploads = new Map<string, (response: Response) => void>();
const releaseHeldUploads = new Map<string, () => void>();
const grants = new Map<string, { file_name: string; media_type: string }>();
const uploads: string[] = [];
const uploadAttempts: string[] = [];
const deletes: string[] = [];
let historyRequests = 0;
let nextSubscription = 0;
const mime: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
  ".wasm": "application/wasm",
};
async function staticResponse(pathname: string): Promise<Response> {
  const relative = normalize(pathname.replace(/^\/+/, "") || "index.html");
  if (relative.startsWith("..")) return new Response("not found", { status: 404 });
  let path = join(buildRoot, relative);
  try {
    if ((await Deno.stat(path)).isDirectory) path = join(path, "index.html");
    return new Response(await Deno.readFile(path), {
      headers: { "content-type": mime[extname(path)] ?? "application/octet-stream" },
    });
  } catch {
    return new Response(await Deno.readFile(join(buildRoot, "index.html")), {
      headers: { "content-type": mime[".html"] },
    });
  }
}
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  if (url.pathname === "/fixture-state") {
    return json({
      methods,
      events,
      requests,
      uploads,
      upload_attempts: uploadAttempts,
      deletes,
      held_uploads: [...heldUploads.keys()],
      history_requests: historyRequests,
    });
  }
  if (url.pathname === "/fixture-control" && request.method === "POST") {
    const control = await request.json();
    if (["accept", "reject", "hold"].includes(control.submit)) submitMode = control.submit;
    if (["normal", "fail", "hold"].includes(control.upload)) uploadMode = control.upload;
    if (control.accept_pending) { for (const accept of pendingAcceptances.splice(0)) accept(); }
    if (control.release_uploads) {
      for (const release of [...releaseHeldUploads.values()]) release();
    }
    return json({ submitMode, uploadMode });
  }
  if (url.pathname.startsWith("/api/")) {
    requests.push({ method: request.method, path: url.pathname });
  }
  if (url.pathname === "/api/workspaces") {
    return json([{
      workspace_id: workspaceId,
      owner_account_id: "owner",
      display_name: "Console Live Review",
      state: "active",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-02T00:00:00Z",
    }]);
  }
  if (url.pathname === `/api/w/${workspaceId}/workspace`) {
    return json({
      workspace_id: workspaceId,
      display_name: "Console Live Review",
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
  if (url.pathname === `/api/w/${workspaceId}/workers/W-901` || url.pathname === workerPath) {
    return json(worker);
  }
  if (url.pathname === `${workerPath}/session`) return json({ availability: "live_protocol" });
  if (url.pathname === `${workerPath}/session/history`) {
    historyRequests++;
    return json({
      availability: "page",
      page: {
        session_id: "session-a",
        lineage_id: "lineage-a",
        compact_ancestor_lineage_ids: [],
        turns: [{ turn_id: "user-1", entries }],
        next_cursor: null,
        has_more: false,
      },
    });
  }
  if (url.pathname === `${workerPath}/attachment-upload-grants` && request.method === "POST") {
    const id = url.searchParams.get("upload_id")!;
    grants.set(id, {
      file_name: url.searchParams.get("file_name")!,
      media_type: url.searchParams.get("media_type")!,
    });
    return json({ upload_id: id, expires_at_ms: Date.now() + 60000 });
  }
  if (url.pathname.startsWith(`${workerPath}/attachment-uploads/`)) {
    const id = url.pathname.split("/").at(-1)!;
    if (request.method === "DELETE") {
      deletes.push(id);
      grants.delete(id);
      cancelledUploads.add(id);
      heldUploads.get(id)?.(new Response(null, { status: 204 }));
      heldUploads.delete(id);
      releaseHeldUploads.delete(id);
      return new Response(null, { status: 204 });
    }
    const grant = grants.get(id);
    if (!grant || request.method !== "PUT") return json({ error: "unknown_upload" }, 404);
    uploadAttempts.push(id);
    if (uploadMode === "fail") {
      await request.body?.cancel();
      return json({ error: "synthetic_upload_failure" }, 503);
    }
    const bytes = await request.arrayBuffer();
    const hash = Array.from(
      new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("");
    const complete = () => {
      uploads.push(id);
      return json({
        file: {
          artifact_id: id,
          ...grant,
          created_at_ms: Date.now(),
          availability: "available",
          byte_len: bytes.byteLength,
          sha256: hash,
        },
      });
    };
    if (uploadMode === "hold") {
      if (cancelledUploads.has(id)) return new Response(null, { status: 204 });
      return await new Promise<Response>((resolve) => {
        heldUploads.set(id, resolve);
        releaseHeldUploads.set(id, () => {
          heldUploads.delete(id);
          releaseHeldUploads.delete(id);
          resolve(complete());
        });
      });
    }
    return complete();
  }
  if (url.pathname.startsWith(`${workerPath}/attachments/`) && request.method === "DELETE") {
    deletes.push(url.pathname.split("/").at(-1)!);
    return new Response(null, { status: 204 });
  }
  if (
    url.pathname === `/api/w/${workspaceId}/protocol/ws` &&
    request.headers.get("upgrade") === "websocket"
  ) {
    const { socket, response } = Deno.upgradeWebSocket(request);
    const subscriptions = new Map<string, string>();
    const send = (frame: unknown) => socket.send(JSON.stringify(frame));
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data));
      if (frame.frame === "request" && frame.message.method === "subscribe_events") {
        const { request_id, selector } = frame.message.params;
        const subscription_id = `fixture-${++nextSubscription}`;
        subscriptions.set(subscription_id, selector.topic);
        send({
          protocol_version: 2,
          frame: "response",
          message: {
            result: "subscribed",
            payload: {
              request_id,
              subscription_id,
              selector,
              snapshot: selector.topic === "worker_protocol"
                ? { topic: "worker_protocol", data: { worker_id: workerId, events: [snapshot] } }
                : { topic: "workers", data: { workers: [] } },
            },
          },
        });
      } else if (frame.frame === "worker_protocol") {
        const method = frame.message.method;
        methods.push(method);
        const subscription_id = frame.message.subscription_id;
        if (subscriptions.get(subscription_id) !== "worker_protocol") return;
        if (method.method === "list_completions") {
          const { kind, prefix, request_id, context } = method.params;
          let candidates: unknown[] = [];
          if (kind === "feature") {
            candidates = invocations.filter((descriptor) =>
              descriptor.name.startsWith(prefix) ||
              descriptor.aliases.some((alias) => alias.startsWith(prefix))
            ).map((invocation) => ({
              value: invocation.name,
              is_dir: false,
              description: invocation.description,
              invocation,
            }));
          } else if (kind === "file") {
            candidates = ["docs/", "docs/release-notes.md"].filter((value) =>
              value.startsWith(prefix)
            ).map((value) => ({ value, is_dir: value.endsWith("/") }));
          } else if (context?.invocation === "fixture.review") {
            const descriptor = invocations[0].arguments.find((argument) =>
              argument.name === context.argument
            );
            const values = !context.argument
              ? invocations[0].arguments.map((argument) => argument.name + "=")
              : descriptor?.completion.kind === "static"
              ? descriptor.completion.values ?? []
              : context.argument === "path"
              ? ["docs/release-notes.md", "docs/long-release-notes-for-responsive-validation.md"]
              : [];
            candidates = values.filter((value) => value.startsWith(prefix)).map((value) => ({
              value,
              is_dir: false,
            }));
          }
          send({
            protocol_version: 2,
            frame: "event",
            message: {
              event: "event",
              data: {
                subscription_id,
                payload: {
                  event: "worker_protocol",
                  data: {
                    worker_id: workerId,
                    event: {
                      event: "completions",
                      data: {
                        kind,
                        prefix,
                        ...(request_id ? { request_id } : {}),
                        ...(context ? { context } : {}),
                        entries: candidates,
                      },
                    },
                  },
                },
              },
            },
          });
        } else if (method.method === "submit") {
          const emit = (event: unknown) => {
            events.push(event);
            if (socket.readyState !== WebSocket.OPEN) return;
            send({
              protocol_version: 2,
              frame: "event",
              message: {
                event: "event",
                data: {
                  subscription_id,
                  payload: { event: "worker_protocol", data: { worker_id: workerId, event } },
                },
              },
            });
          };
          const submission_request_id = method.params.submission_request_id;
          const accept = () =>
            emit({
              event: "submission_accepted",
              data: {
                submission_request_id,
                submission_id: `accepted-${submission_request_id}`,
                disposition: "started",
              },
            });
          if (submitMode === "reject") {
            emit({
              event: "submission_rejected",
              data: {
                submission_request_id,
                message: "Synthetic admission rejection; draft was not accepted.",
              },
            });
          } else if (submitMode === "hold") pendingAcceptances.push(accept);
          else accept();
        }
        // Admission responses are synthetic authority only. Never execute a Feature or Worker.
      }
    };
    return response;
  }
  if (url.pathname.startsWith("/api/")) {
    return json({ error: "unhandled_fixture_request", path: url.pathname }, 404);
  }
  return await staticResponse(url.pathname);
});
