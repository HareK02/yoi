// UI projection fixture ONLY: actual production assets, synthetic API boundary.
// No real sessions/accounts/DB or Backend authorization claims. Cookie is a
// public fixture selector, not a credential; it is never saved as auth state.
import { extname, join, resolve } from "@std/path";
import { setupProjectionResponse } from "./setup-projection.ts";
const [port, build] = Deno.args;
if (!port || !build) {
  throw new Error("usage: fixture-server.ts <port> <build-root>");
}
const buildRoot = resolve(build);
const mime: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".png": "image/png",
};
const policies = new Map<string, boolean>();
// 各fixture processはscenarioを逐次実行する。setupのBearer-only要求はcookieを
// 送らないので、秘密やheadersを読まず直前の合成page条件で応答を選ぶ。
let setupMode: string | null = null;
Deno.serve({ hostname: "127.0.0.1", port: Number(port) }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  const selected = url.searchParams.get("fixture");
  if (url.pathname === "/hosting/setup" && selected?.startsWith("setup-")) {
    setupMode = selected;
  }
  const mode = selected ??
    request.headers.get("cookie")?.match(/ui-fixture=([\w-]+)/)?.[1] ??
    (url.pathname.startsWith("/api/auth/passkeys/addition/")
      ? setupMode
      : null) ??
    "admin-on";
  if (selected) policies.delete(mode);
  const setup = setupProjectionResponse(url.pathname, mode, selected !== null);
  if (setup) return setup;
  const anonymous = mode.startsWith("anonymous") || mode.startsWith("account-");
  const ordinary = mode.startsWith("ordinary");
  if (url.pathname === "/api/auth/whoami") {
    return Response.json({
      session: !anonymous
        ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
        : { kind: "anonymous" },
      actor: anonymous ? null : {
        user_id: "fixture-user",
        account_id: "fixture-account",
        handle: "alice",
        display_name: "Alice",
        auth_method: "browser_session",
      },
    });
  }
  if (url.pathname === "/api/workspaces") return Response.json([]);
  if (url.pathname === "/api/hosting/access") {
    return Response.json({ is_admin: !ordinary && !anonymous });
  }
  const policy = () => ({
    self_registration_enabled: policies.get(mode) ?? !mode.includes("off"),
  });
  if (url.pathname === "/api/auth/registration-policy") {
    if (mode === "account-error") {
      return new Response("synthetic availability failure", { status: 503 });
    }
    if (mode === "account-loading") return new Promise<Response>(() => {});
    return Response.json(policy());
  }
  if (url.pathname === "/api/hosting/settings") {
    if (anonymous || ordinary) {
      return new Response("synthetic denied", {
        status: anonymous ? 401 : 403,
      });
    }
    if (mode === "admin-load-error") {
      return new Response("synthetic settings failure", { status: 503 });
    }
    if (
      mode === "admin-loading" ||
      (mode === "admin-saving" && request.method === "PUT")
    ) {
      return new Promise<Response>(() => {});
    }
    if (request.method === "PUT") {
      if (mode === "admin-save-error") {
        return new Response("synthetic unconfirmed save", { status: 503 });
      }
      const data = await request.json();
      if (typeof data.self_registration_enabled !== "boolean") {
        return new Response("invalid", { status: 400 });
      }
      policies.set(mode, data.self_registration_enabled);
    }
    return Response.json(policy());
  }
  if (url.pathname.startsWith("/api/")) {
    return new Response("unexpected fixture API", { status: 404 });
  }
  const path = join(buildRoot, decodeURIComponent(url.pathname));
  if (!path.startsWith(buildRoot + "/")) {
    return new Response("invalid path", { status: 400 });
  }
  let bytes: Uint8Array;
  let extension = extname(path);
  try {
    bytes = await Deno.readFile(path);
  } catch (error) {
    if (
      !(error instanceof Deno.errors.NotFound) &&
      !(error instanceof Deno.errors.IsADirectory)
    ) {
      throw error;
    }
    bytes = await Deno.readFile(join(buildRoot, "index.html"));
    extension = ".html";
  }
  // productionのmarkup/style/moduleを保ち、Setupだけbrowser API境界を代替する。
  // このbootstrapはnavigator.credentialsのみを置換し、formや表示を操作しない。
  if (extension === ".html" && mode.startsWith("setup-")) {
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
      `ui-fixture=${selected}; Path=/; SameSite=Strict`,
    );
  }
  return new Response(bytes as Uint8Array<ArrayBuffer>, { headers });
});
