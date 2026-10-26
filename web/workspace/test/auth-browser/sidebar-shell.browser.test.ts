/// <reference lib="dom" />
// 本番shell/routerと合成HTTP。実アカウントや認証器は操作しない。
import { assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { chromium } from "playwright";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";

const root = resolve(import.meta.dirname!, "../../../..");
const origin = "http://127.0.0.1:15421";
const output = `${root}/target/web-ux/sidebar-restoration`;

Deno.test("only authenticated account and home show the sidebar; public entry remains standalone", async (t) => {
  const processes = await startOwnedProcesses(
    [{
      id: "sidebar-fixture",
      command: "deno",
      cwd: root,
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-read",
        "--allow-net",
        "web/workspace/test/auth-browser/fixture-server.ts",
        "15421",
        "web/workspace/build",
      ],
      readyUrl: `${origin}/health`,
    }],
    `${root}/tools/web-ux/scenarios/anonymous-entry.json`,
    `${output}/processes`,
    [],
  );
  try {
    const browser = await chromium.launch();
    try {
      for (const width of [1440, 390]) {
        for (
          const [path, mode] of [
            ["/account", "workspace-one"],
            ["/account", "account"],
            ["/account", "workspace-error"],
            ["/account", "workspace-loading"],
            ["/", "anonymous"],
            ["/", "account"],
            ["/", "session-error"],
            ["/", "session-loading"],
          ]
        ) {
          await t.step(`${width}: ${path} ${mode}`, async () => {
            const context = await browser.newContext({
              viewport: { width, height: 900 },
              colorScheme: width === 390 ? "dark" : "light",
              reducedMotion: "reduce",
            });
            try {
              const page = await context.newPage();
              page.setDefaultTimeout(10_000);
              const errors: string[] = [];
              let catalogCalls = 0;
              page.on("pageerror", (error) => errors.push(error.message));
              page.on("request", (request) => {
                if (new URL(request.url()).pathname === "/api/workspaces") {
                  catalogCalls++;
                }
              });
              await page.request.get(`${origin}/?fixture=${mode}`);
              await page.goto(`${origin}${path}`);
              await page.getByRole("heading", {
                name: path === "/" ? "Yoi" : "Account",
                exact: true,
              }).waitFor();
              if (path === "/" && mode !== "account") {
                if (mode === "anonymous") {
                  await page.getByRole("link", { name: "Log in", exact: true })
                    .waitFor();
                } else if (mode === "session-error") {
                  await page.getByRole("alert").waitFor();
                } else {
                  await page.getByText("Loading session…", { exact: true })
                    .waitFor();
                }
                assertEquals(await page.locator(".sidebar-frame").count(), 0);
                assertEquals(
                  await page.locator(".app-shell__topbar").count(),
                  0,
                );
                if (mode === "anonymous") {
                  await page.screenshot({
                    path: `${output}/anonymous-${width}.png`,
                  });
                  await page.getByRole("link", { name: "Log in", exact: true })
                    .click();
                  await page.getByRole("button", {
                    name: "Log in with passkey",
                    exact: true,
                  }).waitFor();
                  assertEquals(await page.locator(".sidebar-frame").count(), 0);
                  await page.goBack();
                  await page.getByRole("link", { name: "Log in", exact: true })
                    .waitFor();
                  assertEquals(await page.locator(".sidebar-frame").count(), 0);
                }
                assertEquals(catalogCalls, 0);
                assertEquals(errors, []);
                return;
              }
              const sidebar = page.getByRole("complementary", {
                name: "Sidebar",
                exact: true,
              });
              if (width === 390) {
                await page.getByRole("button", {
                  name: "Show sidebar",
                  exact: true,
                }).click();
                assertEquals(
                  await page.locator("main").evaluate((main) =>
                    main.hasAttribute("inert")
                  ),
                  true,
                );
              }
              const navigation = sidebar.getByRole("navigation", {
                name: "Global pages",
                exact: true,
              });
              await navigation.getByRole("link", {
                name: "Dashboard / Workspaces",
                exact: true,
              }).waitFor();
              const account = navigation.getByRole("link", {
                name: "Account",
                exact: true,
              });
              assertEquals(await account.getAttribute("href"), "/account");
              assertEquals(
                await account.getAttribute("aria-current"),
                path === "/account" ? "page" : null,
              );
              if (mode === "workspace-one") {
                await navigation.getByRole("link", {
                  name: "New Workspace",
                  exact: true,
                }).waitFor();
              } else if (path === "/account" && mode === "account") {
                await navigation.getByText("No accessible Workspaces.", {
                  exact: true,
                }).waitFor();
              } else {
                assertEquals(
                  await navigation.getByText("No accessible Workspaces.", {
                    exact: true,
                  }).count(),
                  0,
                );
              }
              if (path === "/") assertEquals(catalogCalls, 0);
              if (
                mode === "workspace-one" ||
                (path === "/" && mode === "account")
              ) {
                await page.screenshot({
                  path: `${output}/${
                    path === "/" ? "home" : "account"
                  }-${width}.png`,
                });
              }
              if (width === 390) {
                await page.getByRole("button", {
                  name: "Hide sidebar",
                  exact: true,
                }).click();
                assertEquals(
                  await page.locator("main").evaluate((main) =>
                    main.hasAttribute("inert")
                  ),
                  false,
                );
              }
              if (path === "/account") {
                await page.getByRole("button", {
                  name: "Add passkey to alice",
                  exact: true,
                }).waitFor();
                await page.getByRole("button", { name: "Log out", exact: true })
                  .waitFor();
              } else {
                // The authenticated home has a sidebar; logout must remove it on return.
                await page.getByRole("main").getByRole("link", {
                  name: "Account",
                  exact: true,
                }).click();
                await page.getByRole("button", { name: "Log out", exact: true })
                  .click();
                await page.getByRole("link", { name: "Log in", exact: true })
                  .waitFor();
                assertEquals(new URL(page.url()).pathname, "/");
                assertEquals(await page.locator(".sidebar-frame").count(), 0);
              }
              assertEquals(
                await page.evaluate(() =>
                  document.documentElement.scrollWidth > innerWidth
                ),
                false,
              );
              assertEquals(errors, []);
            } finally {
              await context.close();
            }
          });
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    await stopOwnedProcesses(processes);
  }
});
