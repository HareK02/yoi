// 本番Web buildと合成HTTP境界を接続する。実認証・削除の永続性はBackend suiteの責務。
import { join, resolve } from "jsr:@std/path@1.1.4";
const [portText, build, mode] = Deno.args;
const port = Number(portText);
const root = resolve(import.meta.dirname!, "../../../..");
const upstreamPort = port + 1;
const upstream = new Deno.Command(Deno.execPath(), {
  args: [
    "run",
    "-A",
    join(root, "tools/web-ux/browser-tests/memory_document_fixture_server.ts"),
    String(upstreamPort),
    resolve(build),
    "3",
  ],
  stdout: "null",
  stderr: "inherit",
}).spawn();
const stop = () => {
  try {
    upstream.kill();
  } catch { /* already stopped */ }
};
Deno.addSignalListener("SIGTERM", () => {
  stop();
  Deno.exit();
});
Deno.addSignalListener("SIGINT", () => {
  stop();
  Deno.exit();
});
const attempts = new Map<string, string>();
const hidden = new Set<string>();
const user = {
  user_id: "fixture-user",
  account_id: "owner",
  handle: "owner",
  display_name: "Memory owner",
};
const apiRoot = "/api/w/memory-review";
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") {
    try {
      return await fetch(`http://127.0.0.1:${upstreamPort}/health`);
    } catch {
      return new Response(null, { status: 503 });
    }
  }
  if (url.pathname === "/api/auth/whoami") {
    return Response.json({
      authentication_reference: "subject-delete-fixture",
      actor: { ...user, auth_method: "browser_session" },
      session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
    });
  }
  const sessionId =
    request.headers.get("cookie")?.match(/delete-review-session=([^;]+)/)
      ?.[1] ?? crypto.randomUUID();
  const reader =
    request.headers.get("cookie")?.includes("subject-delete-persona=reader") ??
      false;
  const match = url.pathname.match(/\/subjektiv\/subjects\/([^/]+)(?:\/|$)/);
  const id = match ? decodeURIComponent(match[1]) : null;
  const key = JSON.stringify([sessionId, id]);
  if (request.method === "DELETE" && id) {
    if (reader) {
      return Response.json({ error: "forbidden", message: "Owner required" }, {
        status: 403,
      });
    }
    const body = await request.json();
    const prior = attempts.get(key);
    if (prior && prior !== body.operation_id) {
      return Response.json(
        { error: "conflict", message: "Changed operation" },
        { status: 409 },
      );
    }
    attempts.set(key, body.operation_id);
    hidden.add(key);
    if (!prior && id === "unknown-subject") {
      return Response.json({
        error: "internal",
        message: "Response lost after fence",
      }, { status: 500 });
    }
    return Response.json({
      subject_id: id,
      operation_id: body.operation_id,
      completed: !!prior,
    });
  }
  if (id && request.method === "GET" && hidden.has(key)) {
    return Response.json(
      { error: "not_found", message: "Subject unavailable" },
      { status: 404 },
    );
  }
  const mapped = id &&
    ["pending-subject", "unknown-subject", "legacy-subject"].includes(id);
  const target = new URL(request.url);
  target.port = String(upstreamPort);
  if (mapped) {
    target.pathname = target.pathname.replace(
      encodeURIComponent(id!),
      "empty-subject",
    );
  }
  let response = await fetch(new Request(target, request));
  if (url.pathname === `${apiRoot}/workspace` || mapped) {
    const data = await response.json();
    if (url.pathname === `${apiRoot}/workspace` && mode !== "before") {
      data.permissions.delete_subjects = !reader;
    }
    if (mapped) {
      if ("id" in data) {
        data.id = id;
        data.role = `Deletion review · ${id}`;
        if (id === "legacy-subject") data.state = "retired";
      }
      if ("subject_id" in data) data.subject_id = id;
    }
    response = Response.json(data, { status: response.status });
  }
  response = new Response(response.body, {
    status: response.status,
    headers: response.headers,
  });
  const persona = url.searchParams.get("persona");
  if (persona) {
    response.headers.set(
      "set-cookie",
      `subject-delete-persona=${persona}; Path=/; HttpOnly; SameSite=Lax`,
    );
  }
  response.headers.append(
    "set-cookie",
    `delete-review-session=${sessionId}; Path=/; HttpOnly; SameSite=Lax`,
  );
  return response;
});
