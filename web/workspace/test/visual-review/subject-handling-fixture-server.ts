// 共有Ticket fixtureの古い認証・担当projectionだけをAPI境界で更新する。
// 製品shell/CSS・認可処理には手を加えず、実Backend/Worker実行の証拠にはしない。
const port = Number(Deno.args[0]);
const upstream = Deno.args[1];
const baseline = Deno.args.includes("--baseline");
if (!Number.isInteger(port) || !upstream) {
  throw new Error("usage: server <port> <upstream> [--baseline]");
}
const subject = (id: string, state = "active") => ({
  id,
  role: "Research",
  state,
  behavior_md: "",
  memory_fingerprint: "fixture",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  current_worker: null,
});
function project(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(project);
  if (value && typeof value === "object") {
    const item = value as Record<string, unknown>;
    if (
      item.kind === "workspace_agent" &&
      item.agent_key === "workspace-orchestrator"
    ) return { kind: "subject", subject_id: "subject-a" };
    return Object.fromEntries(
      Object.entries(item).map(([key, value]) => [key, project(value)]),
    );
  }
  return value;
}
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (
    url.pathname.endsWith("/protocol/ws") &&
    request.headers.get("upgrade") === "websocket"
  ) {
    const { socket, response } = Deno.upgradeWebSocket(request);
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data));
      if (frame?.message?.method === "subscribe_events") {
        socket.send(JSON.stringify({
          protocol_version: 2,
          frame: "response",
          message: {
            result: "subscribed",
            payload: {
              request_id: frame.message.params.request_id,
              subscription_id: "fixture-workers",
              selector: frame.message.params.selector,
              snapshot: { topic: "workers", data: { workers: [] } },
            },
          },
        }));
      }
    };
    return response;
  }
  if (url.pathname === "/api/auth/whoami") {
    return Response.json({
      authentication_reference: "fixture-authentication",
      session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
      actor: {
        user_id: "fixture-user",
        account_id: "fixture-member",
        handle: "mika",
        display_name: "Mika",
        auth_method: "browser_session",
      },
    });
  }
  if (url.pathname === "/api/hosting/access") {
    return Response.json({ is_admin: false });
  }
  if (url.pathname.endsWith("/subjektiv/subjects")) {
    if (url.pathname.includes("tickets-denied")) {
      return Response.json({ message: "Subject listing denied" }, {
        status: 403,
      });
    }
    if (url.pathname.includes("tickets-empty")) {
      return Response.json({
        limit: 100,
        items: [],
        has_more: false,
        next_cursor: null,
      });
    }
    const more = url.searchParams.has("cursor");
    return Response.json({
      limit: 100,
      items: more
        ? [
          subject("subject-a"),
          subject("subject-b"),
          subject("subject-retired", "retired"),
        ]
        : [subject("subject-a")],
      has_more: !more,
      next_cursor: more ? null : "next-subject",
    });
  }
  if (url.pathname.endsWith("/tickets/browse")) {
    const input = await request.json();
    const queryUrl = new URL(
      url.pathname.replace(/\/browse$/, "/query"),
      upstream,
    );
    const response = await fetch(queryUrl, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(input),
    });
    if (!response.ok) return response;
    const data = await response.json();
    data.items = data.items.map((item: Record<string, unknown>) =>
      Object.fromEntries(
        [
          "id",
          "resource_key",
          "title",
          "state",
          "updated_at",
          "tags",
          "current_closure",
          "assignment_tracking",
        ].map((key) => [key, item[key]]),
      )
    );
    return Response.json(baseline ? data : project(data));
  }
  const response = await fetch(new URL(url.pathname + url.search, upstream), {
    method: request.method,
    headers: request.headers,
    body: request.method === "GET" || request.method === "HEAD"
      ? undefined
      : await request.arrayBuffer(),
    redirect: "manual",
  });
  if (url.pathname.endsWith("/workspace") && response.ok) {
    const value = await response.json();
    value.permissions.delete_subjects = false;
    return Response.json(value);
  }
  if (
    !baseline && !url.pathname.includes("tickets-legacy") &&
    /(?:\/api\/w|\/workspaces)\/[^/]+\/tickets/.test(url.pathname) &&
    response.headers.get("content-type")?.includes("application/json")
  ) {
    return Response.json(project(await response.json()), {
      status: response.status,
    });
  }
  return response;
});
