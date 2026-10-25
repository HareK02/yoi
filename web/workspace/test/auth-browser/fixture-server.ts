// 本番build/client/shellだけを検証する合成HTTP/WebAuthn境界。
// 実認証・署名・製品cookie設計・policyの永続化は検証しない。本文/credentialは記録しない。
// cookie-sessionは実HTTPのMax-AgeをChromiumが失効させる境界だけを検証する。Backendの判定は合成。
import { extname, join, resolve } from "@std/path";
import { setupWebAuthnProjection } from "../hosting-browser/setup-projection.ts";

import {
  createdWorkspace,
  creationResponse,
} from "../../src/lib/workspace/api/workspace-catalog.test-fixtures.ts";

const [port, build, workspaceFixture] = Deno.args;
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
  expired: boolean;
  expiresAt: string | null;
  cookieSession: boolean;
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
const options = (login: boolean, legacy = false) => ({
  challenge_id: "synthetic-challenge-only",
  public_key: login
    ? {
      challenge: "dWk",
      allowCredentials: legacy ? [{ type: "public-key", id: "dWk" }] : [],
      userVerification: "required",
    }
    : {
      challenge: "dWk",
      rp: { name: "UI projection" },
      user: { id: "dWk", name: "alice", displayName: "Alice" },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      authenticatorSelection: {
        residentKey: "required",
        requireResidentKey: true,
        userVerification: "required",
      },
    },
});

Deno.serve({ hostname: "127.0.0.1", port: Number(port) }, async (request) => {
  const url = new URL(request.url);
  const path = url.pathname;
  if (path === "/health") return new Response("ok");
  const selected = url.searchParams.get("fixture");
  let id = request.headers.get("cookie")?.match(/auth-fixture=([\w-]+)/)?.[1];
  // routeごとに独立。クライアントgoto後も同じ合成sessionをwhoamiへ投影する。
  if (
    (selected && sessions.get(id ?? "")?.mode !== selected) || !id ||
    !sessions.has(id)
  ) {
    id = crypto.randomUUID();
    sessions.set(id, {
      mode: selected ?? "anonymous",
      signedIn: selected === "account" ||
        selected?.startsWith("workspace-") === true || selected === "admin" ||
        selected?.startsWith("creation-") === true ||
        selected?.startsWith("addition-") === true,
      whoamiCalls: 0,
      selfRegistrationEnabled: selected !== "policy-disabled",
      expired: selected?.startsWith("session-expired") === true,
      expiresAt: null,
      cookieSession: false,
    });
  }
  const state = sessions.get(id)!;
  const { mode } = state;
  const sessionCookiePresent = request.headers.get("cookie")?.split(/;\s*/)
    .includes("auth-fixture-session=active") === true;
  if (state.cookieSession && !sessionCookiePresent) {
    state.signedIn = false;
    state.expired = true;
  }
  // 制御は同一browser contextの合成fixtureに限定。認証・cookieをJSで偽造しない。
  if (path === "/__ui_fixture/session") {
    if (request.method === "POST") {
      const { operation } = await request.json();
      if (operation === "arm-cookie") {
        state.signedIn = true;
        state.expired = false;
        state.cookieSession = true;
        state.expiresAt = new Date(Date.now() + 3000).toISOString();
        return Response.json({ expires_at: state.expiresAt }, {
          headers: {
            "set-cookie":
              "auth-fixture-session=active; Max-Age=3; Path=/; HttpOnly; SameSite=Strict",
          },
        });
      }
      if (operation === "expire" || operation === "anonymous") {
        state.signedIn = false;
        state.expired = operation === "expire";
        state.cookieSession = false;
        state.expiresAt = null;
      } else return new Response(null, { status: 400 });
    }
    return Response.json({
      session_cookie_present: sessionCookiePresent,
      whoami_calls: state.whoamiCalls,
      expired: state.expired,
    });
  }
  if (path === "/__ui_fixture/webauthn.js") {
    let extra = "";
    if (mode.endsWith("-cancel-once")) {
      extra =
        `for (const method of ['get','create']) { const original=navigator.credentials[method].bind(navigator.credentials); let first=true; Object.defineProperty(navigator.credentials,method,{configurable:true,value:async(...args)=>{if(first){first=false;throw new DOMException('Passkey cancelled','NotAllowedError')}return original(...args)}}); }`;
    } else if (mode.endsWith("-cancel")) {
      extra =
        `for (const method of ['get','create']) Object.defineProperty(navigator.credentials,method,{configurable:true,value:async()=>{throw new DOMException('Passkey cancelled','NotAllowedError')}});`;
    } else if (mode.endsWith("-unsupported")) {
      extra =
        `for (const method of ['get','create']) Object.defineProperty(navigator.credentials,method,{configurable:true,value:async()=>{throw new DOMException('Synthetic unsupported key','NotSupportedError')}});`;
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
      return Response.json({
        authentication_reference: state.signedIn
          ? "fixture-authentication"
          : null,
        actor: { handle: "untrusted" },
      });
    }
    return Response.json({
      authentication_reference: state.signedIn
        ? "fixture-authentication"
        : null,
      actor: state.signedIn
        ? { ...user, auth_method: "browser_session" }
        : null,
      session: state.signedIn
        ? {
          kind: "authenticated",
          expires_at: state.expiresAt ??
            "2099-01-01T00:00:00Z",
        }
        : { kind: state.expired ? "expired" : "anonymous" },
    });
  }
  if (path === "/api/auth/passkeys") {
    return Response.json({
      user,
      keys: [{
        key_reference: "synthetic-safe-key-reference",
        created_at: "2026-01-01T00:00:00Z",
        last_used_at: null,
        normal_login_verified_at: null,
      }],
    });
  }
  if (path === "/api/auth/passkeys/addition/result") {
    return Response.json({ status: "unknown" });
  }
  if (path === "/api/auth/registration-policy") {
    if (mode === "policy-loading") return await new Promise<Response>(() => {});
    if (mode === "policy-error") return new Response(null, { status: 503 });
    return Response.json({
      self_registration_enabled: state.selfRegistrationEnabled,
    });
  }
  if (path === "/api/hosting/access") {
    return Response.json({ is_admin: mode === "admin" });
  }
  if (path === "/api/workspaces") {
    if (!state.signedIn || mode === "workspace-unauthorized") {
      return new Response(null, { status: 401 });
    }
    if (mode === "workspace-error") return new Response(null, { status: 503 });
    if (mode === "workspace-loading") {
      return await new Promise<Response>(() => {});
    }
    if (request.method === "POST") {
      if (mode === "creation-denied") {
        return new Response(null, { status: 403 });
      }
      if (mode === "creation-error") return new Response(null, { status: 503 });
      return Response.json(creationResponse());
    }
    const ids = mode === "workspace-one"
      ? ["home-empty"]
      : mode === "workspace-many"
      ? ["home-empty", "home-member", "home-long"]
      : [];
    return Response.json(ids.map((workspace_id, i) => ({
      ...createdWorkspace,
      workspace_id,
      display_name: i === 0
        ? "New Workspace"
        : i === 1
        ? "Shared Workspace"
        : "Workspace with a long name — international documentation and distributed development",
    })));
  }
  if (path === "/api/auth/logout") {
    state.signedIn = false;
    state.expired = false;
    state.cookieSession = false;
    state.expiresAt = null;
    return Response.json({ status: "logged_out" }, {
      headers: {
        "set-cookie":
          "auth-fixture-session=; Max-Age=0; Path=/; HttpOnly; SameSite=Strict",
      },
    });
  }
  if (path === "/api/auth/device-login/approve") {
    if (!state.signedIn) return new Response(null, { status: 401 });
    if (mode === "device-expired") {
      return Response.json({
        message: "expired_device_login: device login code expired",
      }, { status: 400 });
    }
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
      // challengeの401だけではbrowser sessionの失効を主張しない。
      return new Response(null, { status: 401 });
    }
    if (stage === "options") {
      const body = await request.json();
      return Response.json(
        options(purpose === "login", typeof body.handle === "string"),
      );
    }
    state.signedIn = true;
    state.expired = false;
    state.cookieSession = false;
    state.expiresAt = null;
    return Response.json({
      user,
      authentication_reference: "fixture-authentication",
    });
  }
  if (path === "/api/auth/activation/options") {
    return Response.json(options(false));
  }
  if (path === "/api/auth/activation/complete") {
    state.signedIn = true;
    state.expired = false;
    state.cookieSession = false;
    state.expiresAt = null;
    return Response.json({
      user,
      authentication_reference: "fixture-authentication",
    });
  }
  // Workspace画面の既存fixtureを再利用する。認証/登録/作成APIの代わりにはしない。
  if (workspaceFixture && path.startsWith("/api/w/")) {
    const target = new URL(path + url.search, workspaceFixture);
    if (request.headers.get("upgrade") === "websocket") {
      const { socket, response } = Deno.upgradeWebSocket(request);
      target.protocol = "ws:";
      const peer = new WebSocket(target);
      const pending: string[] = [];
      socket.onmessage = (event) =>
        peer.readyState === WebSocket.OPEN
          ? peer.send(event.data)
          : pending.push(String(event.data));
      peer.onopen = () => {
        for (const frame of pending) peer.send(frame);
      };
      peer.onmessage = (event) => {
        if (socket.readyState === WebSocket.OPEN) socket.send(event.data);
      };
      socket.onclose = () => peer.close();
      peer.onclose = () => socket.close();
      return response;
    }
    return await fetch(new Request(target, request));
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
