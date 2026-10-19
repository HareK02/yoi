// production shellを既存static fixtureから配信し、Account表示用の境界だけを置換する。
// 実account/session/credentialは作成せず、queryで撮影条件を固定する。
const port = Number(Deno.args[0]);
const upstream = Deno.args[1];
if (!Number.isInteger(port) || !upstream) {
  throw new Error("usage: server <port> <upstream>");
}
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  if (url.pathname === "/api/auth/whoami") {
    return Response.json({
      session:
        request.headers.get("cookie")?.includes("fixture-actor=authenticated")
          ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
          : { kind: "anonymous" },
      actor:
        request.headers.get("cookie")?.includes("fixture-actor=authenticated")
          ? {
            user_id: "fixture-user",
            account_id: "fixture-account",
            handle: "alice",
            display_name: "Alice",
            auth_method: "browser_session",
          }
          : null,
    });
  }
  if (url.pathname === "/api/workspaces") return Response.json([]);
  if (url.pathname.startsWith("/api/")) {
    return new Response("unexpected fixture API", { status: 404 });
  }
  const response = await fetch(new URL(url.pathname + url.search, upstream));
  const headers = new Headers(response.headers);
  if (url.pathname === "/account") {
    headers.set(
      "set-cookie",
      `fixture-actor=${
        url.searchParams.get("actor") === "authenticated"
          ? "authenticated"
          : "anonymous"
      }; Path=/; SameSite=Strict`,
    );
  }
  return new Response(response.body, { status: response.status, headers });
});
