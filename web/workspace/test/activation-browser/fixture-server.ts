// 本番buildを配信するUI投影fixture。実際のcode検証、認証、DB、cookie発行ではない。
// request本文/credential/secretを記録しない。fixture cookieは公開状態名だけ。
import { extname, join, resolve } from "@std/path";
import { setupWebAuthnProjection } from "../hosting-browser/setup-projection.ts";
const [port, build] = Deno.args;
if (!port || !build) throw new Error("Expected port and build root");
const root = resolve(build);
const mime: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".png": "image/png",
};
const user = {
  user_id: "fixture-user",
  account_id: "fixture-account",
  handle: "alice",
  display_name: "Alice",
};
Deno.serve({ hostname: "127.0.0.1", port: Number(port) }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  const selected = url.searchParams.get("fixture");
  const mode = selected ??
    request.headers.get("cookie")?.match(/activation-fixture=([\w-]+)/)?.[1] ??
    "admin";
  const path = url.pathname;
  if (path === "/__ui_fixture/webauthn.js") {
    const source = mode === "activation-pending"
      ? `Object.defineProperty(navigator.credentials,'create',{configurable:true,value:async()=>new Promise(()=>{})});`
      : mode === "activation-cancel"
      ? `Object.defineProperty(navigator.credentials,'create',{configurable:true,value:async()=>{throw new DOMException('Cancelled','NotAllowedError')}});`
      : setupWebAuthnProjection;
    return new Response(source, {
      headers: { "content-type": "text/javascript" },
    });
  }
  if (path === "/api/workspaces") return new Response(null, { status: 503 });
  if (path === "/api/auth/whoami") {
    return Response.json({
      authentication_reference: mode !== "anonymous" ? "fixture-authentication" : null,
      session: mode !== "anonymous"
        ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
        : { kind: "anonymous" },
      actor: mode === "anonymous"
        ? null
        : { ...user, auth_method: "browser_session" },
    });
  }
  if (path === "/api/hosting/access") {
    if (mode === "loading") return await new Promise<Response>(() => {});
    if (mode === "access-error") return new Response(null, { status: 503 });
    return Response.json({
      is_admin: mode !== "ordinary" && mode !== "anonymous",
    });
  }
  if (path === "/api/hosting/settings") {
    return Response.json({ self_registration_enabled: false });
  }
  if (
    path === "/api/hosting/accounts" ||
    path === "/api/hosting/accounts/activation-code"
  ) {
    if (mode === "issue-error") return new Response(null, { status: 409 });
    // codeは暗号素材でない固定の投影値。captureは.secretを隠しsnapshotからも除外する。
    return Response.json({
      user,
      code: "synthetic-output-only",
      expires_at: new Date(Date.now() + 900000).toISOString(),
    });
  }
  if (path === "/api/auth/activation/options") {
    if (mode === "activation-reject") {
      return new Response(null, { status: 403 });
    }
    return Response.json({
      challenge_id: "ui-projection-only",
      public_key: {
        challenge: "dWk",
        rp: { name: "UI fixture" },
        user: { id: "dWk", name: "alice", displayName: "Alice" },
        pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      },
    });
  }
  if (path === "/api/auth/activation/complete") {
    if (mode === "activation-unknown") {
      return new Response(null, { status: 503 });
    }
    return Response.json({ user, authentication_reference: "fixture-authentication" });
  }
  if (path.startsWith("/api/")) return new Response(null, { status: 404 });
  const file = join(root, decodeURIComponent(path));
  if (!file.startsWith(root + "/")) return new Response(null, { status: 400 });
  let bytes: Uint8Array;
  let extension = extname(file);
  try {
    bytes = await Deno.readFile(file);
  } catch (error) {
    if (
      !(error instanceof Deno.errors.NotFound) &&
      !(error instanceof Deno.errors.IsADirectory)
    ) throw error;
    bytes = await Deno.readFile(join(root, "index.html"));
    extension = ".html";
  }
  if (extension === ".html") {
    bytes = new TextEncoder().encode(
      new TextDecoder().decode(bytes).replace(
        "<head>",
        '<head><script src="/__ui_fixture/webauthn.js"></script>',
      ),
    );
  }
  const headers = new Headers({
    "content-type": mime[extension] ?? "application/octet-stream",
    "cache-control": "no-store",
  });
  if (selected) {
    headers.set(
      "set-cookie",
      `activation-fixture=${selected}; Path=/; SameSite=Strict`,
    );
  }
  return new Response(bytes as Uint8Array<ArrayBuffer>, { headers });
});
