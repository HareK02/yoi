// 本番build/client/shellだけを検証する合成HTTP/WebAuthn境界。
// 実認証・署名・cookie安全性・policyの永続化は検証しない。本文/credentialは記録しない。
import { extname, join, resolve } from "@std/path";
import { setupWebAuthnProjection } from "../hosting-browser/setup-projection.ts";

const [port, build] = Deno.args;
if (!port || !build) throw new Error("Expected port and production build root");
const root = resolve(build);
const user = {
  user_id: "fixture-user",
  account_id: "fixture-account",
  handle: "alice",
  display_name: "Alice",
};
type State = {
  mode: string;
  signedIn: boolean;
  whoamiCalls: number;
  selfRegistrationEnabled: boolean;
};
const sessions = new Map<string, State>();
const mime: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".png": "image/png",
  ".ico": "image/x-icon",
};
const options = (login: boolean) => ({
  challenge_id: "synthetic-challenge-only",
  public_key: login ? { challenge: "dWk" } : {
    challenge: "dWk",
    rp: { name: "UI projection" },
    user: { id: "dWk", name: "alice", displayName: "Alice" },
    pubKeyCredParams: [{ type: "public-key", alg: -7 }],
  },
});

Deno.serve({ hostname: "127.0.0.1", port: Number(port) }, async (request) => {
  const url = new URL(request.url);
  const path = url.pathname;
  if (path === "/health") return new Response("ok");
  const selected = url.searchParams.get("fixture");
  let id = request.headers.get("cookie")?.match(/auth-fixture=([\w-]+)/)?.[1];
  // routeごとに独立。クライアントgoto後も同じ合成sessionをwhoamiへ投影する。
  if (selected || !id || !sessions.has(id)) {
    id = crypto.randomUUID();
    sessions.set(id, {
      mode: selected ?? "anonymous",
      signedIn: selected === "account" ||
        selected?.startsWith("addition-") === true,
      whoamiCalls: 0,
      selfRegistrationEnabled: selected !== "policy-disabled",
    });
  }
  const state = sessions.get(id)!;
  const { mode } = state;
  if (path === "/__ui_fixture/webauthn.js") {
    let extra = "";
    if (mode.endsWith("-cancel")) {
      extra =
        `for (const method of ['get','create']) Object.defineProperty(navigator.credentials,method,{configurable:true,value:async()=>{throw new DOMException('Passkey cancelled','NotAllowedError')}});`;
    } else if (mode.endsWith("-error")) {
      extra =
        `for (const method of ['get','create']) Object.defineProperty(navigator.credentials,method,{configurable:true,value:async()=>{throw new DOMException('Synthetic authenticator failure','UnknownError')}});`;
    } else if (mode.endsWith("-pending")) {
      extra =
        `for (const method of ['get','create']) { const original=navigator.credentials[method].bind(navigator.credentials); Object.defineProperty(navigator.credentials,method,{configurable:true,value:async()=>{ await new Promise(resolve=>{window.__authFixtureRelease=resolve}); return original(); }}); }`;
    }
    return new Response(setupWebAuthnProjection + extra, {
      headers: {
        "content-type": "text/javascript",
        "cache-control": "no-store",
      },
    });
  }
  if (path === "/api/auth/whoami") {
    state.whoamiCalls++;
    if (mode === "session-loading") {
      return await new Promise<Response>(() => {});
    }
    if (
      mode === "session-error" ||
      (mode === "session-retry" && state.whoamiCalls === 1)
    ) {
      return new Response(null, { status: 503 });
    }
    if (mode === "session-malformed") {
      return Response.json({ actor: { handle: "untrusted" } });
    }
    return Response.json({
      actor: state.signedIn
        ? { ...user, auth_method: "browser_session" }
        : null,
    });
  }
  if (path === "/api/auth/registration-policy") {
    if (mode === "policy-error") return new Response(null, { status: 503 });
    return Response.json({
      self_registration_enabled: state.selfRegistrationEnabled,
    });
  }
  if (path === "/api/hosting/access") return Response.json({ is_admin: false });
  if (path === "/api/workspaces") {
    // fallbackは現行Workspace選択まで。公開トップ/T760の挙動を新設しない。
    return state.signedIn
      ? Response.json([])
      : new Response(null, { status: 401 });
  }
  if (path === "/api/auth/logout") {
    state.signedIn = false;
    return Response.json({ status: "logged_out" });
  }
  if (path === "/api/auth/device-login/approve") {
    if (!state.signedIn) return new Response(null, { status: 401 });
    if (mode === "device-expired") return new Response(null, { status: 410 });
    return Response.json({ status: "approved", user });
  }
  const passkey = path.match(
    /^\/api\/auth\/passkeys\/(login|registration|addition)\/(options|complete)$/,
  );
  if (passkey) {
    const [, purpose, stage] = passkey;
    if (purpose === "addition" && !state.signedIn) {
      return new Response(null, { status: 401 });
    }
    if (purpose === "registration") {
      if (mode === "policy-stopped" && stage === "complete") {
        state.selfRegistrationEnabled = false;
      }
      if (!state.selfRegistrationEnabled) {
        return new Response(null, { status: 403 });
      }
    }
    if (mode.endsWith("-http-error") && stage === "options") {
      return new Response(null, { status: 503 });
    }
    if (mode === `${purpose}-expired` && stage === "complete") {
      if (purpose === "addition") state.signedIn = false;
      return new Response(null, { status: 401 });
    }
    if (stage === "options") return Response.json(options(purpose === "login"));
    state.signedIn = true;
    return Response.json({ user });
  }
  if (path === "/api/auth/activation/options") {
    return Response.json(options(false));
  }
  if (path === "/api/auth/activation/complete") {
    state.signedIn = true;
    return Response.json({ user });
  }
  if (path.startsWith("/api/")) return new Response(null, { status: 404 });
  let file: string;
  try {
    file = join(root, decodeURIComponent(path));
  } catch {
    return new Response(null, { status: 400 });
  }
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
  if (extension === ".html") {
    headers.set(
      "set-cookie",
      `auth-fixture=${id}; Path=/; SameSite=Strict; HttpOnly`,
    );
  }
  return new Response(bytes as Uint8Array<ArrayBuffer>, { headers });
});
