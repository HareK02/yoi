/// <reference lib="dom" />
// Production-shell browser behavior with synthetic HTTP projections only.
// Run against prebuilt assets; never builds or uses a real session/backend.
import { assert, assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { chromium, type Page } from "playwright";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const evidence = `${root}/target/web-ux/hosting/interactions`;
async function noOverflow(page: Page) {
  assertEquals(
    await page.evaluate(() => ({
      page: document.documentElement.scrollWidth > innerWidth,
      main: [...document.querySelectorAll("main")].some((main) =>
        main.scrollWidth > main.clientWidth
      ),
    })),
    { page: false, main: false },
  );
}
Deno.test("hosting UI keyboard navigation, policy editing, denial and unknown save outcome on production shell", async () => {
  const processes = await startOwnedProcesses(
    [{
      id: "hosting-interactions",
      command: "deno",
      cwd: root,
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-net",
        "--allow-read",
        "web/workspace/test/hosting-browser/fixture-server.ts",
        "15379",
        "web/workspace/build",
      ],
      readyUrl: "http://127.0.0.1:15379/health",
      readyTimeoutMs: 10000,
    }],
    `${root}/tools/web-ux/scenarios/hosting-settings-light.json`,
    evidence,
    [],
  );
  const browser = await chromium.launch();
  const results: unknown[] = [];
  try {
    for (const theme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme: theme,
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          const errors: string[] = [];
          const consoleErrors: string[] = [];
          const httpErrors: string[] = [];
          const failedRequests: { url: string; error: string }[] = [];
          page.on("pageerror", (error) => errors.push(String(error)));
          page.on("console", (message) => {
            if (message.type() === "error") consoleErrors.push(message.text());
          });
          page.on("response", (response) => {
            if (response.status() >= 400) {
              httpErrors.push(
                `${response.status()} ${new URL(response.url()).pathname}`,
              );
            }
          });
          page.on(
            "requestfailed",
            (request) =>
              failedRequests.push({
                url: request.url(),
                error: request.failure()?.errorText ?? "unknown",
              }),
          );
          const settingsRequests: string[] = [];
          page.on("request", (request) => {
            if (new URL(request.url()).pathname === "/api/hosting/settings") {
              settingsRequests.push(request.method());
            }
          });
          await page.goto("http://127.0.0.1:15379/?fixture=admin-on");
          await page.getByRole("heading", { name: "Yoi", exact: true })
            .waitFor();
          if (
            await page.getByRole("button", {
              name: "Show sidebar",
              exact: true,
            }).isVisible()
          ) {
            await page.getByRole("button", {
              name: "Show sidebar",
              exact: true,
            }).click();
          }
          const nav = page.getByRole("navigation", { name: "Your account" });
          await nav.getByRole("link", { name: "Hosting settings", exact: true })
            .waitFor();
          await nav.getByRole("link", { name: "Account", exact: true }).focus();
          await page.keyboard.press("Tab");
          assertEquals(
            await page.locator(":focus").innerText(),
            "Hosting settings",
          );
          assertEquals(
            await page.locator(":focus").evaluate((element) =>
              getComputedStyle(element).outlineStyle !== "none"
            ),
            true,
          );
          await page.keyboard.press("Enter");
          const edit = page.getByRole("button", {
            name: "Edit registration policy",
          });
          await edit.waitFor();
          // Globalの入口からhostingへ進んだ後はWorkspace shellを離れる。
          // Workspace未作成でも管理できることと整合させる。
          assertEquals(await nav.count(), 0);
          await page.getByRole("heading", {
            name: "Hosting settings",
            exact: true,
          }).waitFor();
          if (
            await page.getByRole("button", {
              name: "Hide sidebar",
              exact: true,
            }).isVisible() &&
            width < 768
          ) {
            await page.getByRole("button", {
              name: "Hide sidebar",
              exact: true,
            }).click();
          }
          await noOverflow(page);
          await edit.focus();
          await page.keyboard.press("Enter");
          const checkbox = page.getByRole("checkbox", {
            name: "Allow new account registration",
          });
          assertEquals(
            await checkbox.evaluate((element) =>
              element === document.activeElement
            ),
            true,
          );
          assertEquals(await checkbox.isChecked(), true);
          await page.keyboard.press("Space");
          await page.getByRole("button", { name: "Cancel", exact: true })
            .click();
          assertEquals(
            await edit.evaluate((element) =>
              element === document.activeElement
            ),
            true,
          );
          await page.keyboard.press("Enter");
          assertEquals(await checkbox.isChecked(), true);
          await page.keyboard.press("Space");
          await page.keyboard.press("Tab");
          assertEquals(
            await page.locator(":focus").innerText(),
            "Save settings",
          );
          await page.keyboard.press("Enter");
          await page.getByRole("status").filter({
            hasText: "Hosting settings saved.",
          }).waitFor();
          assertEquals(
            await edit.evaluate((element) =>
              element === document.activeElement
            ),
            true,
          );
          await page.getByText("Disabled", { exact: true }).waitFor();
          assertEquals(
            settingsRequests.filter((method) => method === "PUT").length,
            1,
          );
          await noOverflow(page);
          for (const mode of ["ordinary", "anonymous"]) {
            settingsRequests.length = 0;
            await page.goto(
              `http://127.0.0.1:15379/hosting/settings?fixture=${mode}`,
            );
            if (mode === "anonymous") {
              await page.waitForURL("http://127.0.0.1:15379/");
              await page.getByRole("heading", { name: "Yoi", exact: true })
                .waitFor();
            } else {await page.getByText(
                "Hosting administrator access is required.",
                { exact: true },
              ).waitFor();}
            assertEquals(await edit.count(), 0);
            assertEquals(await checkbox.count(), 0);
            assertEquals(
              await nav.getByRole("link", {
                name: "Hosting settings",
                exact: true,
              }).count(),
              0,
            );
            assertEquals(settingsRequests, []);
            await noOverflow(page);
          }
          await page.goto(
            "http://127.0.0.1:15379/register?fixture=account-off",
          );
          await page.getByText(
            "New account registration is disabled. Existing users can still log in.",
          ).waitFor();
          assertEquals(
            await page.getByRole("button", { name: "Register new account" })
              .count(),
            0,
          );
          await page.getByRole("link", {
            name: "Log in with an existing account",
          }).waitFor();
          await noOverflow(page);
          await page.goto(
            "http://127.0.0.1:15379/account?fixture=ordinary-account-off",
          );
          await page.getByRole("button", { name: "Add passkey to alice" })
            .waitFor();
          assertEquals(
            await page.getByRole("button", { name: "Add passkey to alice" })
              .isEnabled(),
            true,
          );
          await page.goto(
            "http://127.0.0.1:15379/hosting/settings?fixture=admin-save-error",
          );
          await edit.click();
          await checkbox.press("Space");
          await page.getByRole("button", { name: "Save settings", exact: true })
            .click();
          await page.getByRole("alert").waitFor();
          assertEquals(await checkbox.isChecked(), false);
          assertEquals(await checkbox.isDisabled(), true);
          assertEquals(
            await page.getByRole("button", {
              name: "Save settings",
              exact: true,
            }).isDisabled(),
            true,
          );
          await noOverflow(page);
          await page.getByRole("button", {
            name: "Reload settings",
            exact: true,
          }).click();
          await edit.waitFor();
          assertEquals(await page.getByRole("alert").count(), 0);
          assertEquals(await edit.isEnabled(), true);
          for (
            const [mode, path, ready] of [
              [
                "admin-load-error",
                "/hosting/settings",
                'button:has-text("Retry loading settings")',
              ],
              [
                "account-error",
                "/register",
                'button:has-text("Retry registration availability")',
              ],
              [
                "admin-loading",
                "/hosting/settings",
                '[role="status"]:has-text("Loading hosting settings")',
              ],
              [
                "account-loading",
                "/register",
                '[role="status"]:has-text("Checking registration availability")',
              ],
            ]
          ) {
            await page.goto(`http://127.0.0.1:15379${path}?fixture=${mode}`);
            await page.locator(ready).waitFor();
            await noOverflow(page);
          }
          // 遷移でidentity/settingsの読み取りや遅延font取得がbrowserに取消されることはある。
          // 通信失敗一般を許すのではなく、これらの明示的な取消だけを区別する。
          assert(
            failedRequests.every(({ url, error }) =>
              error === "net::ERR_ABORTED" && ([
                "/api/hosting/settings",
                "/api/auth/registration-policy",
                "/api/auth/whoami",
                "/api/hosting/access",
              ].some((path) => url.endsWith(path)) ||
                /^\/_app\/immutable\/assets\/[^/]+\.woff2$/.test(
                  new URL(url).pathname,
                ))
            ),
            JSON.stringify(failedRequests),
          );
          assertEquals(httpErrors, [
            "503 /api/hosting/settings",
            "503 /api/hosting/settings",
            "503 /api/auth/registration-policy",
          ]);
          assertEquals(consoleErrors.length, 3);
          assert(consoleErrors.every((message) => message.includes("503")));
          assertEquals(errors, []);
          results.push({
            theme,
            width,
            keyboard: "pass",
            overflow: "none",
            deniedControls: "absent",
            save: "confirmed",
            unconfirmedSave: "blocked until reload",
            realAuthorizationTested: false,
          });
        } finally {
          await context.close();
        }
      }
    }
    await Deno.writeTextFile(
      `${evidence}/results.json`,
      JSON.stringify(results, null, 2),
    );
  } finally {
    await browser.close();
    await stopOwnedProcesses(processes);
  }
});
