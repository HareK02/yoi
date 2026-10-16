/// <reference lib="dom" />
// 本番shell/router/clientを合成HTTP/WebAuthnで接続する。Server認証/DBのE2Eではない。
import { assert, assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { type Browser, chromium, type Page } from "playwright";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";

const root = resolve(import.meta.dirname!, "../../../..");
const origin = "http://127.0.0.1:15391";
type Mutation = { path: string; keys: string[] };
type Harness = {
  page: Page;
  mutations: Mutation[];
  go: (path: string, mode?: string) => Promise<void>;
};
const login = async (page: Page) => {
  await page.getByLabel("Account ID (handle)", { exact: true }).fill("alice");
  await page.getByRole("button", { name: "Log in with passkey", exact: true })
    .click();
};
const register = async (page: Page) => {
  await page.getByLabel("Account ID (handle)", { exact: true }).fill("alice");
  await page.getByLabel("Display name", { exact: true }).fill("Alice");
  await page.getByRole("button", { name: "Register new account", exact: true })
    .click();
};
const count = (mutations: Mutation[], path: string) =>
  mutations.filter((entry) => entry.path === path).length;
const at = async (page: Page, path: string) => {
  await page.waitForURL((url) =>
    url.origin === origin && url.pathname + url.search === path
  );
};
const noOverflow = async (page: Page) =>
  assertEquals(
    await page.evaluate(() =>
      document.documentElement.scrollWidth > innerWidth
    ),
    false,
  );
async function isolated(
  browser: Browser,
  body: (harness: Harness) => Promise<void>,
  colorScheme: "light" | "dark" = "light",
  width = 1440,
) {
  const context = await browser.newContext({
    viewport: { width, height: 900 },
    colorScheme,
    reducedMotion: "reduce",
  });
  try {
    // 不正returnToでも外部へ接続しない。遷移した事実は下のURL assertionで失敗する。
    await context.route("**/*", (route) => {
      const url = new URL(route.request().url());
      return url.origin === origin ? route.continue() : route.abort();
    });
    const page = await context.newPage();
    const mutations: Mutation[] = [];
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("request", (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === "POST" && path.startsWith("/api/")) {
        // credential本文/secretは保持せず、対象を自由指定しない契約のfield名だけを観測する。
        mutations.push({
          path,
          keys: Object.keys(request.postDataJSON() ?? {}).sort(),
        });
      }
    });
    await body({
      page,
      mutations,
      go: async (path, mode = "anonymous") => {
        const url = new URL(path, origin);
        url.searchParams.set("fixture", mode);
        await page.goto(url.href);
      },
    });
    await noOverflow(page);
    assertEquals(errors, []);
  } finally {
    await context.close();
  }
}

Deno.test("standalone auth separates login, registration, own account and explicit device approval", async (t) => {
  const processes = await startOwnedProcesses(
    [{
      id: "auth-browser",
      command: "deno",
      cwd: root,
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-read",
        "--allow-net",
        "web/workspace/test/auth-browser/fixture-server.ts",
        "15391",
        "web/workspace/build",
      ],
      readyUrl: `${origin}/health`,
    }],
    `${root}/tools/web-ux/scenarios/anonymous-entry.json`,
    `${root}/target/web-ux/auth-interactions/processes`,
    [],
  );
  let browser: Browser | undefined;
  try {
    browser = await chromium.launch();
    for (const theme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const step = (
          name: string,
          body: (harness: Harness) => Promise<void>,
        ) =>
          t.step(
            `${theme}/${width}: ${name}`,
            () => isolated(browser!, body, theme, width),
          );
        await step(
          "anonymous account replaces history with login then returns to own account",
          async ({ page, go, mutations }) => {
            await page.goto(`${origin}/health`);
            const historyBefore = await page.evaluate(() => history.length);
            await go("/account");
            await at(page, "/login?returnTo=%2Faccount");
            assertEquals(
              await page.evaluate(() => history.length),
              historyBefore + 1,
            );
            const id = page.getByLabel("Account ID (handle)", { exact: true });
            await id.waitFor();
            assertEquals(
              await page.getByLabel("Display name", { exact: true }).count(),
              0,
            );
            assertEquals(await page.locator("input").count(), 1);
            assertEquals(
              await page.getByRole("link", {
                name: "Create a new account",
                exact: true,
              }).getAttribute("href"),
              "/register?returnTo=%2Faccount",
            );
            await page.getByRole("link", {
              name: "Activate an account with a code",
              exact: true,
            }).waitFor();
            await id.fill("alice");
            await id.press("Tab");
            const button = page.getByRole("button", {
              name: "Log in with passkey",
              exact: true,
            });
            assert(
              await button.evaluate((element) =>
                element === document.activeElement &&
                getComputedStyle(element).outlineStyle !== "none"
              ),
            );
            await page.keyboard.press("Enter");
            await at(page, "/account");
            await page.getByRole("button", {
              name: "Add passkey to alice",
              exact: true,
            }).waitFor();
            assertEquals(await page.locator("input").count(), 0);
            await page.getByText("Alice", { exact: true }).first().waitFor();
            await noOverflow(page);
            await page.getByRole("button", {
              name: "Add passkey to alice",
              exact: true,
            }).click();
            await page.getByRole("status").filter({
              hasText: /added|registered/i,
            }).waitFor();
            const additions = mutations.filter((entry) =>
              entry.path.includes("/addition/")
            );
            assertEquals(additions.map((entry) => entry.path), [
              "/api/auth/passkeys/addition/options",
              "/api/auth/passkeys/addition/complete",
            ]);
            assertEquals(additions[0].keys, ["browser_origin"]);
            assertEquals(additions[1].keys, ["challenge_id", "credential"]);
            assertEquals(
              mutations.some((entry) => entry.path.includes("/registration/")),
              false,
            );
            await page.getByRole("button", { name: "Log out", exact: true })
              .click();
            await page.getByRole("button", {
              name: "Log in with passkey",
              exact: true,
            }).waitFor();
            assertEquals(new URL(page.url()).pathname, "/login");
            await page.reload();
            await page.getByRole("button", {
              name: "Log in with passkey",
              exact: true,
            }).waitFor();
            assertEquals(count(mutations, "/api/auth/logout"), 1);
          },
        );
        await step(
          "direct registration has only ID and display name and returns safely",
          async ({ page, go, mutations }) => {
            await go("/register?returnTo=%2Faccount");
            await page.getByLabel("Display name", { exact: true }).waitFor();
            assertEquals(await page.locator("input").count(), 2);
            await register(page);
            await at(page, "/account");
            await page.getByRole("button", {
              name: "Add passkey to alice",
              exact: true,
            }).waitFor();
            assertEquals(
              count(mutations, "/api/auth/passkeys/registration/options"),
              1,
            );
            assertEquals(
              count(mutations, "/api/auth/passkeys/registration/complete"),
              1,
            );
            assertEquals(
              mutations.some((entry) => entry.path.includes("/addition/")),
              false,
            );
          },
        );
        await step(
          "device login restoration never approves without an explicit separate action",
          async ({ page, go, mutations }) => {
            await go("/login/device?user_code=ABCD-EFGH");
            await page.getByLabel("User code", { exact: true }).fill(
              "WXYZ-1234",
            );
            const entry = page.getByRole("link", {
              name: "Log in to continue",
              exact: true,
            });
            const href = await entry.getAttribute("href");
            assert(href);
            const returned = new URL(href, origin).searchParams.get("returnTo");
            assert(returned?.startsWith("/login/device?"));
            assertEquals(
              new URL(returned!, origin).searchParams.get("user_code"),
              "WXYZ-1234",
            );
            assertEquals(
              await page.getByLabel("Account ID (handle)", { exact: true })
                .count(),
              0,
            );
            assertEquals(count(mutations, "/api/auth/device-login/approve"), 0);
            await entry.click();
            await login(page);
            await page.waitForURL((url) => url.pathname === "/login/device");
            const approve = page.getByRole("button", {
              name: "Approve device login",
              exact: true,
            });
            await approve.waitFor();
            assertEquals(
              await page.getByLabel("User code", { exact: true }).inputValue(),
              "WXYZ-1234",
            );
            assertEquals(count(mutations, "/api/auth/device-login/approve"), 0);
            await approve.click();
            await page.getByText(
              "Device login approved. You can return to the CLI.",
              { exact: true },
            ).waitFor();
            assertEquals(count(mutations, "/api/auth/device-login/approve"), 1);
          },
        );
        await step(
          "disabled registration retains existing-user login",
          async ({ page, go, mutations }) => {
            await go("/register", "policy-disabled");
            await page.getByText(
              "New account registration is disabled. Existing users can still log in.",
              { exact: true },
            ).waitFor();
            assertEquals(
              await page.getByRole("button", {
                name: "Register new account",
                exact: true,
              }).count(),
              0,
            );
            assertEquals(
              await page.getByLabel("Account ID (handle)", { exact: true })
                .count(),
              0,
            );
            const link = page.locator('a[href^="/login"]').first();
            await link.click();
            await login(page);
            await at(page, "/dashboard");
            await page.goto(`${origin}/account`);
            await page.getByRole("button", {
              name: "Add passkey to alice",
              exact: true,
            }).click();
            await page.getByRole("status").filter({ hasText: /added/i })
              .waitFor();
            assertEquals(
              count(mutations, "/api/auth/passkeys/addition/complete"),
              1,
            );
            assertEquals(
              mutations.some((entry) => entry.path.includes("/registration/")),
              false,
            );
          },
        );
        await step(
          "registration stopped during ceremony displays HTTP403 and removes registration after policy refresh",
          async ({ page, go, mutations }) => {
            await go("/register", "policy-stopped");
            await register(page);
            await page.getByRole("alert").filter({ hasText: "403" }).waitFor();
            await page.getByText(
              "New account registration is disabled. Existing users can still log in.",
              { exact: true },
            ).waitFor();
            assertEquals(await page.locator("form").count(), 0);
            assertEquals(
              await page.getByRole("button", {
                name: "Register new account",
                exact: true,
              }).count(),
              0,
            );
            await page.getByRole("link", {
              name: "Log in with an existing account",
              exact: true,
            }).waitFor();
            assertEquals(new URL(page.url()).pathname, "/register");
            assertEquals(
              count(mutations, "/api/auth/passkeys/registration/complete"),
              1,
            );
            await noOverflow(page);
          },
        );
        for (
          const mode of [
            "session-error",
            "session-malformed",
            "session-loading",
          ]
        ) {
          await step(
            `${mode} does not become known anonymous or allow mutation`,
            async ({ page, go, mutations }) => {
              await go("/login", mode);
              if (mode === "session-loading") {
                await page.getByText("Loading session…", { exact: true })
                  .waitFor();
              } else {
                await page.getByRole("alert").waitFor();
                await page.getByRole("button", {
                  name: "Retry session",
                  exact: true,
                }).waitFor();
              }
              const loginButton = page.getByRole("button", {
                name: "Log in with passkey",
                exact: true,
              });
              if (await loginButton.count()) {
                assertEquals(await loginButton.isEnabled(), false);
              }
              assertEquals(mutations, []);
            },
          );
        }
      }
    }
    for (
      const path of [
        "/account",
        "/register",
        "/login/device?user_code=ABCD-EFGH",
      ]
    ) {
      await t.step(
        `unknown session on ${path} offers only recovery, never anonymous mutations`,
        () =>
          isolated(browser!, async ({ page, go, mutations }) => {
            await go(path, "session-error");
            await page.getByRole("button", {
              name: "Retry session",
              exact: true,
            }).waitFor();
            assertEquals(
              await page.getByRole("link", {
                name: "Log in to continue",
                exact: true,
              }).count(),
              0,
            );
            assertEquals(
              await page.getByLabel("Account ID (handle)", { exact: true })
                .count(),
              0,
            );
            assertEquals(
              await page.getByRole("button", {
                name:
                  /^(Add passkey|Register new account|Approve device login)/,
              }).count(),
              0,
            );
            assertEquals(
              new URL(page.url()).pathname,
              new URL(path, origin).pathname,
            );
            assertEquals(mutations, []);
          }),
      );
    }
    await t.step(
      "policy lookup failure is unknown availability, not disabled or permitted registration",
      () =>
        isolated(browser!, async ({ page, go, mutations }) => {
          await go("/register", "policy-error");
          await page.getByRole("alert").waitFor();
          assertEquals(
            await page.getByRole("button", {
              name: "Register new account",
              exact: true,
            }).count(),
            0,
          );
          await page.getByRole("link", {
            name: "Log in with an existing account",
            exact: true,
          }).waitFor();
          assertEquals(mutations, []);
        }),
    );
    await t.step(
      "leaving a pending login never completes or redirects after navigation",
      () =>
        isolated(browser!, async ({ page, go, mutations }) => {
          await go("/login", "login-pending");
          await login(page);
          await page.waitForFunction(() =>
            typeof (window as unknown as { __authFixtureRelease?: unknown })
              .__authFixtureRelease === "function"
          );
          await page.getByRole("link", {
            name: "Create a new account",
            exact: true,
          }).click();
          await page.getByLabel("Display name", { exact: true }).waitFor();
          await page.evaluate(() =>
            (window as unknown as { __authFixtureRelease: () => void })
              .__authFixtureRelease()
          );
          // 次のpaintまで旧promiseのmicrotaskを進め、固定sleepで完了を推測しない。
          await page.evaluate(() =>
            new Promise<void>((resolve) =>
              requestAnimationFrame(() =>
                requestAnimationFrame(() => resolve())
              )
            )
          );
          assertEquals(new URL(page.url()).pathname, "/register");
          assertEquals(
            count(mutations, "/api/auth/passkeys/login/complete"),
            0,
          );
        }),
    );
    await t.step(
      "retry rechecks unknown session before allowing login",
      () =>
        isolated(browser!, async ({ page, go }) => {
          await go("/login", "session-retry");
          await page.getByRole("button", { name: "Retry session", exact: true })
            .click();
          await login(page);
          await at(page, "/dashboard");
        }),
    );
    for (const route of ["login", "register"]) {
      await t.step(
        `${route} without returnTo uses root fallback`,
        () =>
          isolated(browser!, async ({ page, go }) => {
            await go(`/${route}`);
            await (route === "login" ? login(page) : register(page));
            await at(page, "/dashboard");
            await page.reload();
            assertEquals(new URL(page.url()).pathname, "/dashboard");
          }),
      );
      for (
        const rejected of [
          "https://example.invalid/",
          "//example.invalid/",
          "/\\example.invalid/",
          "account",
          "/login",
          "/register?returnTo=%2Flogin",
          "/activate",
          "/login/../login",
        ]
      ) {
        await t.step(
          `${route} rejects external or auth-loop returnTo ${rejected}`,
          () =>
            isolated(browser!, async ({ page, go }) => {
              await go(`/${route}?returnTo=${encodeURIComponent(rejected)}`);
              await (route === "login" ? login(page) : register(page));
              await at(page, "/dashboard");
            }),
        );
      }
    }
    for (const purpose of ["login", "registration", "addition"] as const) {
      const route = purpose === "registration"
        ? "/register"
        : purpose === "addition"
        ? "/account"
        : "/login";
      for (const failure of ["cancel", "error", "http-error", "expired"]) {
        await t.step(
          `${purpose} ${failure} is visible and does not claim successful authentication`,
          () =>
            isolated(browser!, async ({ page, go, mutations }) => {
              await go(route, `${purpose}-${failure}`);
              if (purpose === "login") await login(page);
              else if (purpose === "registration") await register(page);
              else {await page.getByRole("button", {
                  name: "Add passkey to alice",
                  exact: true,
                }).click();}
              await page.getByRole("alert").waitFor();
              const path = new URL(page.url()).pathname;
              assert(
                path === route || (purpose === "addition" && path === "/login"),
              );
              if (
                failure === "cancel" || failure === "error" ||
                failure === "http-error"
              ) {
                assertEquals(
                  count(mutations, `/api/auth/passkeys/${purpose}/complete`),
                  0,
                );
              } else {assertEquals(
                  count(mutations, `/api/auth/passkeys/${purpose}/complete`),
                  1,
                );}
            }),
        );
      }
      await t.step(
        `${purpose} repeated submit while prompt is pending sends one ceremony`,
        () =>
          isolated(browser!, async ({ page, go, mutations }) => {
            await go(route, `${purpose}-pending`);
            const label = purpose === "login"
              ? "Log in with passkey"
              : purpose === "registration"
              ? "Register new account"
              : "Add passkey to alice";
            if (purpose === "login") await login(page);
            else if (purpose === "registration") await register(page);
            else {await page.getByRole("button", { name: label, exact: true })
                .click();}
            await page.waitForFunction(() =>
              typeof (window as unknown as { __authFixtureRelease?: unknown })
                .__authFixtureRelease === "function"
            );
            assertEquals(
              count(mutations, `/api/auth/passkeys/${purpose}/options`),
              1,
            );
            assertEquals(
              count(mutations, `/api/auth/passkeys/${purpose}/complete`),
              0,
            );
            await page.getByRole("button", { name: label, exact: true })
              .evaluate((button) => {
                button.dispatchEvent(
                  new MouseEvent("click", { bubbles: true }),
                );
                button.dispatchEvent(
                  new MouseEvent("click", { bubbles: true }),
                );
              });
            await page.locator("form").evaluateAll((forms) => {
              for (const form of forms) {
                form.dispatchEvent(
                  new Event("submit", { bubbles: true, cancelable: true }),
                );
                form.dispatchEvent(
                  new Event("submit", { bubbles: true, cancelable: true }),
                );
              }
            });
            await page.evaluate(() =>
              (window as unknown as { __authFixtureRelease: () => void })
                .__authFixtureRelease()
            );
            if (purpose === "addition") {
              await page.getByRole("status").filter({
                hasText: /added|registered/i,
              }).waitFor();
            } else await at(page, "/dashboard");
            assertEquals(
              count(mutations, `/api/auth/passkeys/${purpose}/options`),
              1,
            );
            assertEquals(
              count(mutations, `/api/auth/passkeys/${purpose}/complete`),
              1,
            );
          }),
      );
    }
    await t.step(
      "expired device request stays unapproved",
      () =>
        isolated(browser!, async ({ page, go, mutations }) => {
          await go("/login/device?user_code=ABCD-EFGH", "device-expired");
          await page.getByRole("link", {
            name: "Log in to continue",
            exact: true,
          }).click();
          await login(page);
          await page.getByRole("button", {
            name: "Approve device login",
            exact: true,
          }).click();
          await page.getByRole("alert").filter({ hasText: /invalid|expired/i })
            .waitFor();
          assertEquals(count(mutations, "/api/auth/device-login/approve"), 1);
        }),
    );
    await t.step(
      "activation entry remains limited until credential completion",
      () =>
        isolated(browser!, async ({ page, go, mutations }) => {
          await go("/login", "activation-pending");
          await page.getByRole("link", {
            name: "Activate an account with a code",
            exact: true,
          }).click();
          // 同名labelのlogin入力が残る間にfillしない。遷移先の固有controlで同期する。
          await page.waitForURL((url) => url.pathname === "/activate");
          await page.locator("#activation-handle").fill("alice");
          await page.locator("#activation-code").fill(
            "synthetic-input-only",
          );
          await page.getByRole("button", {
            name: "Register Passkey and activate",
            exact: true,
          }).click();
          await page.getByText(
            "Passkey registration pending. Account activation has not been confirmed.",
            { exact: true },
          ).waitFor();
          assertEquals(count(mutations, "/api/auth/activation/complete"), 0);
          assertEquals(
            mutations.some((entry) =>
              /device-login|addition|registration/.test(entry.path)
            ),
            false,
          );
          assertEquals(await page.locator(".app-shell").count(), 0);
          await page.getByRole("button", {
            name: "Cancel registration",
            exact: true,
          }).click();
          await page.getByRole("alert").waitFor();
          const link = page.locator('a[href="/login"]');
          await link.waitFor();
          await link.click();
          await page.waitForURL((url) => url.pathname === "/login");
          await page.getByRole("button", {
            name: "Log in with passkey",
            exact: true,
          }).waitFor();
          assertEquals(count(mutations, "/api/auth/device-login/approve"), 0);
        }),
    );
  } finally {
    await browser?.close();
    await stopOwnedProcesses(processes);
  }
});
