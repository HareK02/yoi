/// <reference lib="dom" />
// 本番build/shell/router/clientと合成HTTP境界の検証。実Server/SQLite/SoftPasskey E2Eとは別。
import { assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { chromium, type Page } from "playwright";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const origin = "http://127.0.0.1:15411";
const ready = (page: Page, name: string) =>
  page.getByRole("link", { name, exact: true }).waitFor();
Deno.test("public entry isolates Workspace authority and authenticated dashboard supports explicit selection", async (t) => {
  const processes = await startOwnedProcesses(
    [
      {
        id: "workspace-fixture",
        command: "deno",
        cwd: root,
        args: [
          "run",
          "--allow-read",
          "--allow-net",
          "tools/web-ux/browser-tests/workspace_home_fixture_server.ts",
          "15412",
          "web/workspace/build",
        ],
        readyUrl: "http://127.0.0.1:15412/health",
      },
      {
        id: "entry-fixture",
        command: "deno",
        cwd: root,
        args: [
          "run",
          "--config",
          "tools/web-ux/deno.json",
          "--allow-read",
          "--allow-net",
          "web/workspace/test/auth-browser/fixture-server.ts",
          "15411",
          "web/workspace/build",
          "http://127.0.0.1:15412",
        ],
        readyUrl: `${origin}/health`,
      },
    ],
    `${root}/tools/web-ux/scenarios/anonymous-entry.json`,
    `${root}/target/web-ux/entry-browser/processes`,
    [],
  );
  try {
    const browser = await chromium.launch();
    try {
      for (const colorScheme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390]) {
          const step = async (
            name: string,
            body: (page: Page) => Promise<void>,
          ) =>
            await t.step(`${colorScheme}/${width}: ${name}`, async () => {
              const context = await browser.newContext({
                viewport: { width, height: 900 },
                colorScheme,
                reducedMotion: "reduce",
              });
              try {
                const page = await context.newPage();
                const errors: string[] = [];
                page.on("pageerror", (error) => errors.push(error.message));
                await body(page);
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
          for (
            const mode of [
              "anonymous",
              "policy-disabled",
              "policy-error",
              "policy-loading",
            ]
          ) {
            await step(
              `public/login policy ${mode}, no catalog calls even if unavailable`,
              async (page) => {
                let workspaceCalls = 0;
                await page.route("**/api/workspaces*", (route) => {
                  workspaceCalls++;
                  return route.fulfill({ status: 503 });
                });
                await page.goto(`${origin}/?fixture=${mode}`);
                await page.getByRole("heading", { name: "Yoi", exact: true })
                  .waitFor();
                await ready(page, "Log in");
                if (mode === "anonymous") {
                  await ready(page, "Create a new account");
                  assertEquals(
                    await page.getByRole("link", {
                      name: "Create a new account",
                    }).getAttribute("href"),
                    "/register?returnTo=%2Fdashboard",
                  );
                } else {
                  await page.getByText(
                    mode === "policy-disabled"
                      ? /registration is disabled/
                      : mode === "policy-error"
                      ? /availability is unknown/
                      : /Checking registration/,
                  ).waitFor();
                  assertEquals(
                    await page.getByRole("link", {
                      name: "Create a new account",
                    }).count(),
                    0,
                  );
                }
                await page.getByRole("link", { name: "Log in", exact: true })
                  .click();
                await page.getByRole("button", { name: "Log in with passkey" })
                  .waitFor();
                if (mode === "policy-disabled") {
                  await page.getByText(/registration is disabled/).waitFor();
                  assertEquals(
                    await page.getByRole("link", {
                      name: "Create a new account",
                    }).count(),
                    0,
                  );
                }
                assertEquals(workspaceCalls, 0);
                assertEquals(
                  await page.locator(".workspace-catalog-alert").count(),
                  0,
                );
              },
            );
          }
          for (const mode of ["session-error", "session-loading"]) {
            await step(
              `public ${mode} is not anonymous or authenticated`,
              async (page) => {
                await page.goto(`${origin}/?fixture=${mode}`);
                await page.getByText(
                  mode === "session-error"
                    ? /Login status is unknown/
                    : "Loading session…",
                ).waitFor();
                assertEquals(
                  await page.getByRole("link", { name: "Log in", exact: true })
                    .count(),
                  0,
                );
                assertEquals(
                  await page.getByRole("link", { name: "Create a new account" })
                    .count(),
                  0,
                );
                assertEquals(
                  await page.getByRole("link", { name: "Open dashboard" })
                    .count(),
                  0,
                );
              },
            );
          }
          await step(
            "session retry and focus/session change remove stale actor/admin links",
            async (page) => {
              await page.goto(`${origin}/?fixture=session-retry`);
              await page.getByRole("button", { name: "Retry session" }).click();
              await ready(page, "Log in");
              await page.goto(`${origin}/?fixture=admin`);
              await ready(page, "Hosting settings");
              await ready(page, "Open dashboard");
              await page.request.post(`${origin}/api/auth/logout`);
              await page.evaluate(() =>
                window.dispatchEvent(new Event("yoi:browser-session-changed"))
              );
              await ready(page, "Log in");
              assertEquals(
                await page.getByRole("link", { name: "Hosting settings" })
                  .count(),
                0,
              );
              assertEquals(
                await page.getByRole("link", { name: "Open dashboard" })
                  .count(),
                0,
              );
            },
          );
          for (
            const mode of [
              "account",
              "workspace-one",
              "workspace-many",
              "admin",
            ]
          ) {
            await step(
              `actor ${mode}: dashboard and explicit Workspace navigation`,
              async (page) => {
                await page.goto(`${origin}/?fixture=${mode}`);
                await ready(page, "Open dashboard");
                await ready(page, "Account");
                if (mode === "admin") await ready(page, "Hosting settings");
                else {assertEquals(
                    await page.getByRole("link", { name: "Hosting settings" })
                      .count(),
                    0,
                  );}
                await page.getByRole("link", { name: "Open dashboard" })
                  .click();
                await page.getByRole("heading", {
                  name: "Dashboard",
                  exact: true,
                }).waitFor();
                if (mode === "workspace-one" || mode === "workspace-many") {
                  const links = page.locator(".workspace-catalog-card");
                  await links.first().waitFor();
                  assertEquals(
                    await links.count(),
                    mode === "workspace-one" ? 1 : 3,
                  );
                  assertEquals(new URL(page.url()).pathname, "/dashboard");
                  await links.first().click();
                  await page.waitForURL("**/w/home-empty");
                  await page.locator(".workspace-home").waitFor();
                  // canonical direct link remains independently reopenable, including another Workspace.
                  await page.goto(`${origin}/w/home-member`);
                  await page.locator(".workspace-home").waitFor();
                  assertEquals(new URL(page.url()).pathname, "/w/home-member");
                } else {
                  await page.getByText("No accessible Workspaces", {
                    exact: true,
                  }).waitFor();
                  await page.locator("summary#workspace-create-title").click();
                  await page.getByLabel("Workspace display name").fill(
                    "New Workspace",
                  );
                  const submitted = page.waitForRequest((request) =>
                    new URL(request.url()).pathname === "/api/workspaces" &&
                    request.method() === "POST"
                  );
                  await page.getByRole("button", {
                    name: "Create Workspace",
                    exact: true,
                  }).click();
                  const keys = Object.keys((await submitted).postDataJSON())
                    .sort();
                  assertEquals(keys, [
                    "display_name",
                    "operation_key",
                    "repository",
                  ]);
                  await page.waitForURL("**/w/home-empty");
                  await page.locator(".workspace-home").waitFor();
                }
              },
            );
          }
          for (
            const mode of [
              "workspace-unauthorized",
              "workspace-error",
              "workspace-loading",
            ]
          ) {
            await step(
              `catalog ${mode} does not erase account or imply empty`,
              async (page) => {
                await page.goto(`${origin}/dashboard?fixture=${mode}`);
                await ready(page, "Account");
                await page.getByText(
                  mode === "workspace-loading"
                    ? "Loading Workspaces…"
                    : /Workspaces could not be loaded/,
                ).waitFor();
                assertEquals(
                  await page.getByText("No accessible Workspaces", {
                    exact: true,
                  }).count(),
                  0,
                );
                assertEquals(new URL(page.url()).pathname, "/dashboard");
                await page.getByRole("link", { name: "Yoi", exact: true })
                  .click();
                await ready(page, "Open dashboard");
                assertEquals(await page.getByRole("alert").count(), 0);
              },
            );
          }
          for (const mode of ["creation-denied", "creation-error"]) {
            await step(
              `explicit creation ${mode} retains input and exact retry operation`,
              async (page) => {
                await page.goto(
                  `${origin}/dashboard?fixture=${mode}#workspace-create-title`,
                );
                await page.getByLabel("Workspace display name").fill(
                  "New Workspace",
                );
                const bodies: unknown[] = [];
                page.on("request", (request) => {
                  if (request.method() === "POST") {
                    bodies.push(request.postDataJSON());
                  }
                });
                await page.getByRole("button", {
                  name: "Create Workspace",
                  exact: true,
                }).click();
                await page.getByRole("alert").waitFor();
                await page.getByRole("button", {
                  name: "Retry creation",
                  exact: true,
                }).click();
                await page.getByRole("button", {
                  name: "Retry creation",
                  exact: true,
                }).waitFor();
                assertEquals(bodies.length, 2);
                assertEquals(bodies[0], bodies[1]);
                assertEquals(
                  await page.getByLabel("Workspace display name").inputValue(),
                  "New Workspace",
                );
                assertEquals(new URL(page.url()).pathname, "/dashboard");
              },
            );
          }
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    assertEquals(await stopOwnedProcesses(processes), []);
  }
});
