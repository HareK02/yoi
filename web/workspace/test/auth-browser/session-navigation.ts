/// <reference lib="dom" />
// auth-shellの本番build/HTTP/WebAuthn fixtureと隔離contextを再利用する。
// Max-Ageは本物のHTTP cookieをbrowserが消すことを観測するが、whoami分類は合成。
// 実Serverのsession/marker/revoke判定の証明ではない。
import { assert, assertEquals } from "@std/assert";
import type { Browser, Page } from "playwright";

type Harness = {
  page: Page;
  mutations: { path: string; keys: string[] }[];
  go: (path: string, mode?: string, unavailableRoot?: boolean) => Promise<void>;
};
type Isolated = (
  browser: Browser,
  body: (harness: Harness) => Promise<void>,
) => Promise<void>;

export async function sessionNavigation(
  t: Deno.TestContext,
  browser: Browser,
  isolated: Isolated,
  origin: string,
) {
  const step = (name: string, body: (harness: Harness) => Promise<void>) =>
    t.step(name, () => isolated(browser, body));
  const login = (page: Page) =>
    page.getByRole("button", { name: "Log in with passkey", exact: true })
      .click();
  const at = (page: Page, target: string) =>
    page.waitForURL((url) =>
      url.origin === origin && url.pathname + url.search + url.hash === target
    );
  const loginTarget = (target: string) =>
    `/login?${new URLSearchParams({ returnTo: target })}`;
  const control = async (page: Page, operation: string) => {
    const response = await page.request.post(`${origin}/__ui_fixture/session`, {
      data: { operation },
    });
    assertEquals(response.status(), 200);
    return response;
  };
  const focus = (page: Page) =>
    page.evaluate(() => window.dispatchEvent(new Event("focus")));
  const noPrivateRequests = (page: Page) => {
    const paths: string[] = [];
    page.on("request", (request) => {
      const path = new URL(request.url()).pathname;
      if (/^\/api\/(workspaces|w\/|hosting\/)/.test(path)) paths.push(path);
    });
    return paths;
  };
  for (
    const target of [
      "/w/team/tickets?filter=100%25#entry",
      "/w/team/tickets/%e6%9c%aa?filter=100%25#part%25",
    ]
  ) {
    await step(
      `expiry retains encoded URL data ${target}`,
      async ({ page, go }) => {
        const privateRequests = noPrivateRequests(page);
        await go(target, "session-expired");
        await at(page, loginTarget(target));
        await page.getByRole("button", {
          name: "Log in with passkey",
          exact: true,
        }).waitFor();
        await page.reload();
        await at(page, loginTarget(target));
        assertEquals(privateRequests, []);
      },
    );
  }
  await step(
    "successful login retains percent query and fragment data",
    async ({ page, go }) => {
      const target = "/account?filter=100%25#part%25";
      await go(target, "session-expired");
      await at(page, loginTarget(target));
      await login(page);
      await at(page, target);
      await page.getByRole("button", { name: "Log out", exact: true })
        .waitFor();
    },
  );
  const protectedRoutes = [
    "/dashboard",
    "/account",
    "/hosting/settings",
    "/w/private/tickets",
  ];
  for (const path of protectedRoutes) {
    await step(
      `fresh anonymous ${path} goes to public root before private loaders`,
      async ({ page, go, mutations }) => {
        const privateRequests = noPrivateRequests(page);
        await go(`${path}?view=private#selected`);
        await at(page, "/");
        await page.getByRole("heading", { name: "Yoi", exact: true }).waitFor();
        assertEquals(await page.locator(".app-shell").count(), 0);
        assertEquals(privateRequests, []);
        assertEquals(mutations, []);
      },
    );
    await step(
      `expired ${path} preserves pathname query and fragment before private loaders`,
      async ({ page, go }) => {
        const target = `${path}?filter=a%2Fb&view=thread#selected`;
        const privateRequests = noPrivateRequests(page);
        await go(target, "session-expired");
        await at(page, loginTarget(target));
        await page.getByRole("button", {
          name: "Log in with passkey",
          exact: true,
        }).waitFor();
        assertEquals(privateRequests, []);
      },
    );
  }
  for (
    const [path, selector] of [
      ["/", "h1"],
      ["/login", "button[type=submit]"],
      ["/register", "#registration-handle"],
      ["/activate", "#activation-handle"],
      ["/login/device?user_code=ABCD-EFGH", 'a[href^="/login?"]'],
      ["/hosting/setup", "#host-setup-token"],
    ]
  ) {
    await step(
      `anonymous public entry ${path} does not redirect or load private data`,
      async ({ page, go }) => {
        const privateRequests = noPrivateRequests(page);
        await go(path);
        // 固有controlのrenderまで待ち、単なるdocument load成功で済ませない。
        if (path === "/register") {
          await page.getByLabel("Display name", { exact: true }).waitFor();
        } else await page.locator(selector).first().waitFor();
        await at(page, path);
        assertEquals(privateRequests, []);
      },
    );
  }
  await step(
    "expired login reload cancellation and retry keep the complete original URL",
    async ({ page, go, mutations }) => {
      const target = "/account?tab=keys&filter=a%2Fb#passkeys";
      await go(target, "session-expired-cancel-once");
      await at(page, loginTarget(target));
      await page.reload();
      await login(page);
      await page.getByRole("alert").filter({ hasText: /cancelled|timed out/i })
        .waitFor();
      await at(page, loginTarget(target));
      assertEquals(
        mutations.filter((m) => m.path.endsWith("/login/complete")).length,
        0,
      );
      await login(page);
      await at(page, target);
      await page.getByRole("button", {
        name: "Add passkey to alice",
        exact: true,
      }).waitFor();
      await page.reload();
      await at(page, target);
      await page.getByRole("button", {
        name: "Add passkey to alice",
        exact: true,
      }).waitFor();
    },
  );
  for (
    const mode of [
      "session-error",
      "session-malformed",
      "session-loading",
      "network-error",
    ]
  ) {
    await step(
      `protected ${mode} stays unknown without redirect or private loaders`,
      async ({ page, go }) => {
        if (mode === "network-error") {
          await page.route(
            "**/api/auth/whoami",
            (route) => route.abort("failed"),
          );
        }
        const privateRequests = noPrivateRequests(page);
        const target = "/account?tab=keys#passkeys";
        await go(target, mode);
        if (mode === "session-loading") {
          // 実要求がpendingであることを観測する。loadingの文言はloaderと画面で異なる。
          await page.waitForLoadState("domcontentloaded");
          assertEquals(
            await page.getByRole("button", {
              name: "Add passkey to alice",
              exact: true,
            }).count(),
            0,
          );
        } else await page.getByText(/Login status is unknown/i).waitFor();
        await at(page, target);
        assertEquals(privateRequests, []);
      },
    );
  }
  await step(
    "protected unknown can reload and retry without losing query or fragment",
    async ({ page, go }) => {
      const target = "/account?tab=keys#passkeys";
      let failed = true;
      await page.route(
        "**/api/auth/whoami",
        (route) => failed ? route.fulfill({ status: 503 }) : route.continue(),
      );
      await go(target, "account", true);
      await page.getByText(/Login status is unknown/i).waitFor();
      await at(page, target);
      failed = false;
      await page.reload();
      await page.getByRole("button", {
        name: "Add passkey to alice",
        exact: true,
      }).waitFor();
      await at(page, target);
    },
  );
  for (const active of [false, true]) {
    await step(
      `real HTTP Max-Age cookie disappears and ${
        active ? "next page access after expiry" : "reload"
      } returns through login`,
      async ({ page, go }) => {
        const target = "/account?tab=keys#after-cookie-expiry";
        await go(target, "account");
        await page.getByRole("button", {
          name: "Add passkey to alice",
          exact: true,
        }).waitFor();
        const armed = await control(page, "arm-cookie");
        assert(armed.headers()["set-cookie"].includes("Max-Age=3"));
        const cookie = (await page.context().cookies()).find((c) =>
          c.name === "auth-fixture-session"
        );
        assert(cookie?.httpOnly);
        assert(cookie.expires > Date.now() / 1000);
        if (active) {
          // 明示的な操作で期限を取得する。期限の経過だけでは再確認しない。
          const refreshed = page.waitForResponse((r) =>
            new URL(r.url()).pathname === "/api/auth/whoami"
          );
          await page.getByRole("button", {
            name: "Refresh session",
            exact: true,
          }).click();
          const wire = await (await refreshed).json();
          assertEquals(wire.session.kind, "authenticated");
          assertEquals(
            wire.session.expires_at,
            (await armed.json()).expires_at,
          );
        }
        // 固定sleepではなくHTTP要求にcookieが付かなくなる事実で同期する。
        await page.evaluate(async () => {
          const deadline = performance.now() + 10_000;
          do {
            const observation = await (await fetch("/__ui_fixture/session", {
              cache: "no-store",
            })).json();
            if (observation.session_cookie_present === false) return;
            await new Promise((resolve) => setTimeout(resolve, 100));
          } while (performance.now() < deadline);
          throw new Error("Primary session cookie did not naturally expire");
        });
        assertEquals(
          (await page.context().cookies()).some((c) =>
            c.name === "auth-fixture-session"
          ),
          false,
        );
        // 長寿命の識別補助は残る。全消去とは異なる条件を作ったことも確認する。
        assert(
          (await page.context().cookies()).some((c) =>
            c.name === "auth-fixture"
          ),
        );
        if (active) {
          await at(page, target);
          assertEquals(await page.getByText("Loading session…").count(), 0);
          await page.getByRole("link", { name: "Dashboard / Workspaces" })
            .click();
          await at(page, loginTarget("/dashboard"));
          await login(page);
          await at(page, "/dashboard");
          return;
        }
        await page.reload();
        await at(page, loginTarget(target));
        await page.reload();
        await login(page);
        await at(page, target);
        await page.getByRole("button", {
          name: "Add passkey to alice",
          exact: true,
        }).waitFor();
      },
    );
  }
  await step(
    "concurrent API 401 leaves the page alone; the next page access detects expiry",
    async ({ page, go }) => {
      const target = "/w/private/tickets?view=thread#concurrent-expiry";
      let release!: () => void;
      const held = new Promise<void>((resolve) => release = resolve);
      const statuses: number[] = [];
      // 本番Workspace loaderの並列requestを止める。window.fetchの呼出しだけで
      // 製品API clientの401再照会を通したことにはしない。
      await page.route("**/api/w/private/**", async (route) => {
        await held;
        statuses.push(401);
        await route.fulfill({ status: 401 });
      });
      const requests = Promise.all([
        page.waitForRequest((r) =>
          new URL(r.url()).pathname === "/api/w/private/workspace"
        ),
        page.waitForRequest((r) =>
          new URL(r.url()).pathname === "/api/w/private/repositories"
        ),
      ]);
      try {
        await go(target, "account");
        await requests;
        await control(page, "expire");
        release();
        // API failure stays on this page; only the next page access checks whoami.
        await page.getByRole("alert").filter({
          hasText: /request failed with HTTP 401/,
        }).waitFor();
        assert(statuses.length >= 2);
        await at(page, target);
        await focus(page);
        assertEquals(await page.getByText("Loading session…").count(), 0);
        await page.reload();
        await at(page, loginTarget(target));
        await page.unroute("**/api/w/private/**");
        await login(page);
        await at(page, target);
      } finally {
        release();
      }
    },
  );
  await step(
    "delayed pre-logout authenticated response cannot restore private UI or an old return URL",
    async ({ page, go }) => {
      await go("/account?tab=keys#stale", "account");
      await page.getByRole("button", { name: "Log out", exact: true })
        .waitFor();
      let started!: () => void;
      let failed!: (error: unknown) => void;
      let release!: () => void;
      const requested = new Promise<void>((resolve, reject) => {
        started = resolve;
        failed = reject;
      });
      const held = new Promise<void>((resolve) => release = resolve);
      let first = true;
      await page.route("**/api/auth/whoami", async (route) => {
        if (!first) return route.continue();
        first = false;
        try {
          const old = await route.fetch({ timeout: 5000 });
          started();
          await held;
          await route.fulfill({
            response: old,
            headers: { ...old.headers(), "x-auth-fixture-held": "true" },
          });
        } catch (error) {
          failed(error);
        }
      });
      try {
        const observed = page.waitForRequest((r) =>
          new URL(r.url()).pathname === "/api/auth/whoami"
        );
        await page.getByRole("button", { name: "Refresh session", exact: true })
          .click();
        await observed;
        await requested;
        // 他tabの本物の製品logoutでgeneration/storage eventも通す。
        const other = await page.context().newPage();
        try {
          await other.goto(`${origin}/account`);
          await other.getByRole("button", { name: "Log out", exact: true })
            .click();
          await at(other, "/");
          const delivered = page.waitForResponse((r) =>
            r.headers()["x-auth-fixture-held"] === "true"
          );
          release();
          await (await delivered).finished();
          // 古い応答を実際に製品へ届け、readSession/renderのtask境界まで進める。
          await page.evaluate(() =>
            new Promise<void>((resolve) =>
              requestAnimationFrame(() =>
                requestAnimationFrame(() => resolve())
              )
            )
          );
          await at(page, "/");
          assertEquals(new URL(page.url()).searchParams.has("returnTo"), false);
          assertEquals(
            await page.getByRole("button", {
              name: "Add passkey to alice",
              exact: true,
            }).count(),
            0,
          );
          await page.goto(`${origin}/account`);
          await at(page, "/");
        } finally {
          await other.close();
        }
      } finally {
        release();
      }
    },
  );
  await step(
    "challenge 401 leaves an authenticated account in place but requires result recovery before another addition",
    async ({ page, go }) => {
      const target = "/account?tab=keys#challenge";
      await go(target, "addition-expired");
      await page.getByRole("button", {
        name: "Add passkey to alice",
        exact: true,
      }).click();
      await page.getByRole("alert").waitFor();
      await at(page, target);
      assertEquals(
        (await (await page.request.get(`${origin}/api/auth/whoami`)).json())
          .session.kind,
        "authenticated",
      );
      assertEquals(
        await page.getByRole("button", {
          name: "Add passkey to alice",
          exact: true,
        }).isEnabled(),
        false,
      );
      await page.getByRole("button", {
        name: "Check addition result",
        exact: true,
      }).click();
      await page.getByRole("status").filter({
        hasText: "addition result is still unknown",
      }).waitFor();
      assertEquals(
        await page.getByRole("button", {
          name: "Add passkey to alice",
          exact: true,
        }).isEnabled(),
        false,
      );
    },
  );
  await step(
    "explicit logout and complete browser storage removal classify as anonymous, not expired",
    async ({ page, go }) => {
      await go("/account", "account");
      await control(page, "arm-cookie");
      await page.getByRole("button", { name: "Log out", exact: true }).click();
      await at(page, "/");
      const wire = await (await page.request.get(`${origin}/api/auth/whoami`))
        .json();
      assertEquals(wire, {
        actor: null,
        authentication_reference: null,
        session: { kind: "anonymous" },
      });
      await page.evaluate(() => {
        localStorage.clear();
        sessionStorage.clear();
      });
      await page.context().clearCookies();
      await page.goto(`${origin}/account?tab=keys#cleared`);
      await at(page, "/");
      assertEquals(new URL(page.url()).searchParams.has("returnTo"), false);
    },
  );
  for (const event of ["focus", "pageshow", "visibilitychange"] as const) {
    await step(
      `${event} leaves the page visible; reload detects expiry`,
      async ({ page, go }) => {
        const target = "/account?tab=keys#lifecycle";
        await go(target, "account");
        await page.getByRole("button", {
          name: "Add passkey to alice",
          exact: true,
        }).waitFor();
        await control(page, "expire");
        let checks = 0;
        page.on("request", (request) => {
          if (new URL(request.url()).pathname === "/api/auth/whoami") checks++;
        });
        await page.evaluate((event) => {
          if (event === "visibilitychange") {
            if (document.hidden) {
              throw new Error("Expected a visible test page");
            }
            document.dispatchEvent(new Event(event));
          } else window.dispatchEvent(new Event(event));
        }, event);
        await page.evaluate(() =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
          )
        );
        assertEquals(checks, 0);
        await at(page, target);
        assertEquals(await page.getByText("Loading session…").count(), 0);
        await page.reload();
        await at(page, loginTarget(target));
      },
    );
  }
  await step(
    "forget expired session clears returnTo and fragment before a later login",
    async ({ page, go, mutations }) => {
      await go("/account?tab=keys#forgotten", "session-expired");
      await page.getByRole("button", {
        name: "Forget this session",
        exact: true,
      }).click();
      await at(page, "/login");
      await page.getByRole("button", {
        name: "Log in with passkey",
        exact: true,
      }).waitFor();
      assertEquals(
        (await (await page.request.get(`${origin}/api/auth/whoami`)).json())
          .session.kind,
        "anonymous",
      );
      assertEquals(
        mutations.filter((m) => m.path === "/api/auth/logout").length,
        1,
      );
      await login(page);
      await at(page, "/dashboard");
    },
  );
  await step(
    "forget session is allowed on unknown login and never claims authenticated success",
    async ({ page, go, mutations }) => {
      await go(
        "/login?returnTo=%2Faccount%3Ftab%3Dkeys%23forgotten#entry",
        "session-error",
      );
      await page.getByText(/Login status is unknown/i).waitFor();
      await page.getByRole("button", {
        name: "Forget this session",
        exact: true,
      }).click();
      await at(page, "/login");
      await page.getByText(/Login status is unknown/i).waitFor();
      assertEquals(
        mutations.filter((m) => m.path === "/api/auth/logout").length,
        1,
      );
      assertEquals(
        await page.getByRole("link", { name: /^Continue to/ }).count(),
        0,
      );
    },
  );
  await step(
    "other-tab logout clears a pending expired login return URL",
    async ({ page, go }) => {
      const target = "/account?tab=keys#other-tab-logout";
      await go(target, "session-expired");
      await at(page, loginTarget(target));
      const other = await page.context().newPage();
      try {
        await other.goto(`${origin}${loginTarget(target)}`);
        await other.getByRole("button", {
          name: "Forget this session",
          exact: true,
        }).click();
        await at(other, "/login");
        await at(page, "/login");
        await login(page);
        await at(page, "/dashboard");
      } finally {
        await other.close();
      }
    },
  );
  for (const status of [401, 403, 404]) {
    await step(
      `post-login resource ${status} remains a resource failure, never a relogin loop`,
      async ({ page, go }) => {
        const target = "/dashboard?selected=missing#resource";
        let whoamiCalls = 0;
        page.on("request", (request) => {
          if (new URL(request.url()).pathname === "/api/auth/whoami") {
            whoamiCalls++;
          }
        });
        await page.route(
          "**/api/workspaces?*",
          (route) => route.fulfill({ status }),
        );
        await go(target, "session-expired");
        await at(page, loginTarget(target));
        await login(page);
        await at(page, target);
        await page.getByText(/Workspaces could not be loaded/).waitFor();
        const before = whoamiCalls;
        await page.reload();
        await page.getByText(/Workspaces could not be loaded/).waitFor();
        await at(page, target);
        // reloadごとの有限なreadで、403/404をexpiredへ変換してloopしない。
        assert(whoamiCalls - before <= 4);
      },
    );
  }
}
