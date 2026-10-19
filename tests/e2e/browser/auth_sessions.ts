/// <reference lib="dom" />
// WorkerE2eEnvironment所有の実Backendへ全APIをproxyし、本番static shellを配信する。
// fixture-serverと同じSPA配信方式。製品APIのwhoami、認証、認可、Set-Cookieは置換しない。
// 固定expiry/短命cookieを作る明示fixtureだけが発行境界を代替し、実分類はBackendへ委ねる。
// navigator.credentialsのみRustのResidentAuthenticatorへ接続する（物理認証器ではない）。
import { assert, assertEquals } from "@std/assert";
import { extname, join, normalize } from "@std/path";
import { type Browser, type BrowserContext, chromium } from "playwright";

const [backend, origin, build, handle] = Deno.args;
if (!backend || !origin || !build || !handle) {
  throw new Error(
    "Expected isolated Backend URL, browser origin, production build, handle",
  );
}
await Deno.stat(join(build, "index.html")); // 暗黙buildや別asset rootへのfallbackはしない。
const mime: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
  ".ico": "image/x-icon",
};
const server = Deno.serve({
  hostname: "127.0.0.1",
  port: Number(new URL(origin).port),
  onListen() {},
}, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/__e2e/expired-session" && request.method === "POST") {
    if (request.headers.get("origin") !== origin) {
      return new Response(null, { status: 403 });
    }
    // Rustは環境専用DBの実登録userへ公開store APIで保存する。secretをbody/ログへ出さない。
    const fixture = await bridge("expired-session", {}) as {
      set_cookies: string[];
    };
    const headers = new Headers({ "cache-control": "no-store" });
    for (const cookie of fixture.set_cookies) {
      headers.append("set-cookie", cookie);
    }
    return new Response(null, { status: 204, headers });
  }
  if (
    url.pathname === "/__e2e/primary-cookie-present" && request.method === "GET"
  ) {
    // HTTPで受け取ったprimaryの有無だけを返す。値、marker、DB状態は公開しない。
    const present = request.headers.get("cookie")?.split(";").some((pair) =>
      pair.trim().split("=", 1)[0] === "yoi_workspace_session"
    ) ?? false;
    return Response.json(present, { headers: { "cache-control": "no-store" } });
  }
  if (url.pathname.startsWith("/api/")) {
    const headers = new Headers(request.headers);
    headers.delete("host");
    headers.delete("connection");
    // OriginとCookieを保持し、Set-Cookieを含む実応答をそのまま返す。
    return await fetch(new URL(url.pathname + url.search, backend), {
      method: request.method,
      headers,
      body: ["GET", "HEAD"].includes(request.method)
        ? undefined
        : await request.arrayBuffer(),
      redirect: "manual",
    });
  }
  const relative = normalize(url.pathname.replace(/^\/+/, "") || "index.html");
  if (relative.startsWith("..")) {
    return new Response("not found", { status: 404 });
  }
  let file = join(build, relative);
  try {
    if ((await Deno.stat(file)).isDirectory) file = join(file, "index.html");
    return new Response(await Deno.readFile(file), {
      headers: {
        "content-type": mime[extname(file)] ?? "application/octet-stream",
      },
    });
  } catch (error) {
    if (!(error instanceof Deno.errors.NotFound)) throw error;
    if (url.pathname.startsWith("/_app/")) {
      return new Response("not found", { status: 404 });
    }
    return new Response(await Deno.readFile(join(build, "index.html")), {
      headers: { "content-type": mime[".html"] },
    });
  }
});

// 逐次ceremony/fixtureだけを扱う短いNDJSON bridge。署名/cookieをartifactへ記録しない。
const input = Deno.stdin.readable.pipeThrough(new TextDecoderStream())
  .getReader();
let pending = "";
async function bridge(operation: string, options: unknown): Promise<unknown> {
  await Deno.stdout.write(
    new TextEncoder().encode(JSON.stringify({ operation, options }) + "\n"),
  );
  while (!pending.includes("\n")) {
    const { done, value } = await input.read();
    if (done) throw new Error("ResidentAuthenticator disconnected");
    pending += value;
    if (pending.length > 1024 * 1024) throw new Error("Oversized credential");
  }
  const newline = pending.indexOf("\n");
  const credential = JSON.parse(pending.slice(0, newline));
  pending = pending.slice(newline + 1);
  return credential;
}
let browser: Browser | undefined;
let context: BrowserContext | undefined;
const terminate = () => {
  void (async () => {
    await browser?.close();
    await server.shutdown();
    Deno.exit(143);
  })();
};
Deno.addSignalListener("SIGTERM", terminate);
try {
  browser = await chromium.launch({ headless: true, timeout: 15_000 });
  context = await browser.newContext();
  context.setDefaultTimeout(10_000);
  await context.route(
    "**/*",
    (route) =>
      new URL(route.request().url()).origin === origin
        ? route.continue()
        : route.abort(),
  );
  await context.exposeBinding(
    "residentCredential",
    (_source, operation: string, options: unknown) =>
      bridge(operation, options),
  );
  await context.addInitScript(() => {
    const encode = (value: unknown): unknown => {
      if (value instanceof ArrayBuffer) {
        return btoa(String.fromCharCode(...new Uint8Array(value)))
          .replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
      }
      if (Array.isArray(value)) return value.map(encode);
      if (value && typeof value === "object") {
        return Object.fromEntries(
          Object.entries(value).map(([key, item]) => [key, encode(item)]),
        );
      }
      return value;
    };
    const bytes = (value: string) => {
      const base64 = value.replaceAll("-", "+").replaceAll("_", "/");
      return Uint8Array.from(
        atob(base64),
        (character) => character.charCodeAt(0),
      ).buffer;
    };
    for (const operation of ["create", "get"] as const) {
      Object.defineProperty(navigator.credentials, operation, {
        configurable: true,
        value: async (
          options: CredentialCreationOptions & CredentialRequestOptions,
        ) => {
          const bridge = (window as unknown as {
            residentCredential: (
              operation: string,
              options: unknown,
            ) => Promise<{
              id: string;
              rawId: string;
              response: Record<string, string | string[] | null>;
              extensions?: unknown;
            }>;
          }).residentCredential;
          const wire = await bridge(operation, {
            publicKey: encode(options.publicKey),
          });
          const response = Object.fromEntries(
            Object.entries(wire.response).map((
              [key, value],
            ) => [key, typeof value === "string" ? bytes(value) : value]),
          );
          return {
            id: wire.id,
            rawId: bytes(wire.rawId),
            type: "public-key",
            getClientExtensionResults: () => wire.extensions ?? {},
            response: {
              ...response,
              getTransports: () => wire.response.transports ?? [],
            },
          };
        },
      });
    }
  });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const at = async (path: string) => {
    await page.waitForURL((url) =>
      url.origin === origin && url.pathname + url.search + url.hash === path
    );
    console.error(`navigation-ready: ${new URL(path, origin).pathname}`);
  };
  const whoami = async () => {
    const response = await page.request.get(`${origin}/api/auth/whoami`);
    assertEquals(response.status(), 200);
    return await response.json();
  };
  const account = "/account?view=passkeys&filter=a%2Fb#selected";
  const privateRequests: string[] = [];
  const observePrivate = (request: { url(): string }) => {
    const path = new URL(request.url()).pathname;
    if (/^\/api\/(workspaces|w\/|hosting\/)/.test(path)) {
      privateRequests.push(path);
    }
  };
  page.on("request", observePrivate);
  await page.goto(origin + account);
  await at("/");
  await page.getByRole("heading", { name: "Yoi", exact: true }).waitFor();
  assertEquals((await whoami()).session, { kind: "anonymous" });
  assertEquals(
    privateRequests,
    [],
    "anonymous gate must run before private loaders",
  );
  page.off("request", observePrivate);

  await page.goto(
    `${origin}/register?${new URLSearchParams({ returnTo: account })}`,
  );
  await page.getByLabel("Account ID (handle)", { exact: true }).fill(handle);
  await page.getByLabel("Display name", { exact: true }).fill(
    "Real browser E2E",
  );
  const registration = page.waitForResponse((response) =>
    new URL(response.url()).pathname ===
      "/api/auth/passkeys/registration/complete"
  );
  await page.getByRole("button", { name: "Register new account", exact: true })
    .click();
  assertEquals((await registration).status(), 200);
  await at(account);
  await page.getByRole("button", { name: "Log out", exact: true }).waitFor();
  const identity = await whoami();
  assertEquals(identity.actor?.handle, handle);
  assertEquals(identity.session.kind, "authenticated");
  assert(typeof identity.session.expires_at === "string");
  const cookies = await context.cookies(origin);
  const session = cookies.find((cookie) =>
    cookie.name === "yoi_workspace_session"
  );
  const marker = cookies.find((cookie) =>
    cookie.name === "yoi_workspace_session_expiry"
  );
  assert(session && marker, "Backend must issue both cookies");
  for (const cookie of [session, marker]) {
    assert(cookie.httpOnly);
    assertEquals(cookie.sameSite, "Lax");
  }
  const seconds = (Date.parse(identity.session.expires_at) / 1000) -
    Date.now() / 1000;
  assert(seconds > 14 * 86400 - 60 && seconds <= 14 * 86400);
  assert(
    Math.abs(session.expires - Date.parse(identity.session.expires_at) / 1000) <
      2,
  );
  assert(Math.abs(marker.expires - session.expires - 7 * 86400) < 2);
  await page.goto(origin + account);
  await at(account);
  await page.getByRole("button", { name: "Log out", exact: true }).waitFor();
  await page.reload();
  await at(account);
  await page.getByRole("button", { name: "Log out", exact: true }).waitFor();

  // 明示logoutは実Serverのrevoke + 両cookie削除を通る。
  await page.getByRole("button", { name: "Log out", exact: true }).click();
  await at("/");
  assertEquals((await whoami()).session, { kind: "anonymous" });
  const afterLogout = await context.cookies(origin);
  assert(
    !afterLogout.some((cookie) =>
      [session.name, marker.name].includes(cookie.name)
    ),
  );
  const revoked = await page.request.get(`${origin}/api/auth/whoami`, {
    headers: {
      cookie:
        `${session.name}=${session.value}; ${marker.name}=${marker.value}`,
    },
  });
  assertEquals((await revoked.json()).session, { kind: "anonymous" });
  await page.goto(origin + account);
  await at("/");

  // 実discoverable loginで、確認済みの認証成功後のみpathname/query/fragmentへ復帰する。
  await page.goto(
    `${origin}/login?${new URLSearchParams({ returnTo: account })}`,
  );
  await page.reload();
  const login = page.waitForResponse((response) =>
    new URL(response.url()).pathname === "/api/auth/passkeys/login/complete"
  );
  await page.getByRole("button", { name: "Log in with passkey", exact: true })
    .click();
  assertEquals((await login).status(), 200);
  await at(account);
  await page.getByRole("button", { name: "Log out", exact: true }).waitFor();
  assertEquals((await whoami()).actor?.handle, handle);

  // 14日実時間の前進ではない。固定expiry実store recordとMax-Age=2のHTTP cookieで、
  // 自然消失後のmarker→実whoami→製品loader/router→実署名loginの接続を検証する。
  const expiredTarget = "/dashboard?view=workspaces&filter=a%2Fb&ratio=100%25#selected%25";
  const catalogResponse = () =>
    page.waitForResponse((response) =>
      new URL(response.url()).pathname === "/api/workspaces" &&
      response.request().method() === "GET"
    );
  const initialCatalog = catalogResponse();
  await page.goto(origin + expiredTarget);
  assertEquals((await initialCatalog).status(), 200);
  await at(expiredTarget);
  await page.getByRole("heading", { name: "Dashboard", exact: true }).waitFor();
  const issuedStatus = await page.evaluate(async () => {
    const response = await fetch("/__e2e/expired-session", {
      method: "POST",
      cache: "no-store",
    });
    return response.status;
  });
  assertEquals(issuedStatus, 204);
  const initiallyPresent = await page.evaluate(async () =>
    await (await fetch("/__e2e/primary-cookie-present", { cache: "no-store" }))
      .json()
  );
  assertEquals(
    initiallyPresent,
    true,
    "short-lived primary must first reach Chromium",
  );
  await page.evaluate(async () => {
    const deadline = performance.now() + 10_000;
    do {
      const response = await fetch("/__e2e/primary-cookie-present", {
        cache: "no-store",
      });
      if (response.ok && await response.json() === false) return;
      await new Promise((resolve) => setTimeout(resolve, 100));
    } while (performance.now() < deadline);
    throw new Error("Primary session cookie did not naturally expire");
  });
  // 値をassertionへ渡さず、cookie jarにもprimaryが消えmarkerが残ることを確認する。
  const expiredCookieNames = (await context.cookies(origin)).map((cookie) =>
    cookie.name
  );
  assert(!expiredCookieNames.includes(session.name));
  assert(expiredCookieNames.includes(marker.name));
  assertEquals(await whoami(), {
    actor: null,
    session: { kind: "expired" },
  });
  privateRequests.length = 0;
  page.on("request", observePrivate);
  const expiredWhoami = page.waitForResponse((response) =>
    new URL(response.url()).pathname === "/api/auth/whoami"
  );
  await page.reload();
  const expiredResponse = await expiredWhoami;
  assertEquals(expiredResponse.status(), 200);
  // SPAがloginへ移った後の旧document bodyをCDPから読み直さない。
  // 分類本文は直前/復帰待ち後にも同じ実Backendから取得し、browser要求はstatusを観測する。
  const expiredLogin = `/login?${new URLSearchParams({
    returnTo: expiredTarget,
  })}`;
  await at(expiredLogin);
  await page.getByRole("button", { name: "Log in with passkey", exact: true })
    .waitFor();
  assertEquals(
    privateRequests,
    [],
    "expired reload must gate the real private catalog loader",
  );
  assertEquals(await whoami(), { actor: null, session: { kind: "expired" } });
  await page.reload();
  await at(expiredLogin);
  await page.getByRole("button", { name: "Log in with passkey", exact: true })
    .waitFor();
  assertEquals(
    privateRequests,
    [],
    "login reload must not lose or privately load returnTo",
  );
  page.off("request", observePrivate);
  const recoveredCatalog = catalogResponse();
  const relogin = page.waitForResponse((response) =>
    new URL(response.url()).pathname === "/api/auth/passkeys/login/complete"
  );
  await page.getByRole("button", { name: "Log in with passkey", exact: true })
    .click();
  assertEquals((await relogin).status(), 200);
  await at(expiredTarget);
  assertEquals((await recoveredCatalog).status(), 200);
  await page.getByRole("heading", { name: "Dashboard", exact: true }).waitFor();
  const recovered = await whoami();
  assertEquals(recovered.session.kind, "authenticated");
  assertEquals(recovered.actor?.user_id, identity.actor.user_id);
  assertEquals(recovered.actor?.handle, handle);
  await page.goto(origin + account);
  await at(account);
  await page.getByRole("button", { name: "Log out", exact: true }).waitFor();

  // fixtureでprimaryだけを除去。Server時計は進めていないため、markerのみでは
  // authenticatedにもexpiredにもならない。これを14日経過の成功とは数えない。
  await context.clearCookies({ name: session.name });
  const partial = await whoami();
  assertEquals(partial, { actor: null, session: { kind: "anonymous" } });
  await page.reload();
  await at("/");
  await context.clearCookies();
  await page.goto(origin + account);
  await at("/");
  assertEquals((await whoami()).session, { kind: "anonymous" });
  assertEquals(errors, []);
} finally {
  await context?.close();
  await browser?.close();
  await server.shutdown();
  Deno.removeSignalListener("SIGTERM", terminate);
}
await Deno.stdout.write(new TextEncoder().encode('{"operation":"done"}\n'));
// 親はstdout EOFで完了を確定する。stdin streamのprefetch取消を待つと相互待ちになる。
Deno.exit(0);
