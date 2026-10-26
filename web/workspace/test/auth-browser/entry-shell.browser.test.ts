/// <reference lib="dom" />
// 本番build/shell/router/clientと合成HTTP境界の検証。実Server/SQLite/SoftPasskey E2Eとは別。
import { assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { chromium, type Page } from "playwright";
import { creationResponse } from "../../src/lib/workspace/api/workspace-catalog.test-fixtures.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const origin = "http://127.0.0.1:15411";
const ready = (page: Page, name: string) =>
  page.getByRole("main").getByRole("link", { name, exact: true }).waitFor();
async function focusWithoutSessionCheck(page: Page) {
  let checks = 0;
  const observe = (request: import("playwright").Request) => {
    if (new URL(request.url()).pathname === "/api/auth/whoami") checks++;
  };
  page.on("request", observe);
  try {
    await page.evaluate(async () => {
      window.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("pageshow"));
      document.dispatchEvent(new Event("visibilitychange"));
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
      );
    });
    assertEquals(checks, 0);
    assertEquals(await page.getByText("Loading session…").count(), 0);
  } finally {
    page.off("request", observe);
  }
}
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
    `${root}/target/web-ux/t767-validation/entry-browser/processes`,
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
                assertEquals(
                  await page.locator(
                    'a[href^="/register"], a[href="/activate"]',
                  ).count(),
                  0,
                );
                assertEquals(await page.getByText(/registration/i).count(), 0);
                await page.getByRole("link", { name: "Log in", exact: true })
                  .click();
                await page.getByRole("button", { name: "Log in with passkey" })
                  .waitFor();
                assertEquals(
                  await page.getByRole("link", {
                    name: "Activate an account with a code",
                    exact: true,
                  }).getAttribute("href"),
                  "/activate",
                );
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
                assertEquals(workspaceCalls, 0);
                assertEquals(
                  await page.locator(".workspace-catalog-alert").count(),
                  0,
                );
              },
            );
          }
          await step(
            "Workspace notifications do not cross logout into the public entry",
            async (page) => {
              page.setDefaultTimeout(5000);
              await page.routeWebSocket(/\/protocol\/ws/, (socket) => {
                socket.onMessage((message) => {
                  const frame = JSON.parse(String(message));
                  if (frame.message?.method !== "subscribe_events") return;
                  socket.send(JSON.stringify({
                    protocol_version: 2,
                    frame: "response",
                    message: {
                      result: "subscribed",
                      payload: {
                        request_id: frame.message.params.request_id,
                        subscription_id: "fixture-private-alert",
                        selector: { topic: "workspace_workers" },
                        snapshot: {
                          topic: "workers",
                          data: {
                            workers: [{
                              worker_id: "private-worker",
                              runtime_id: "fixture-runtime",
                              resource_key: "W-1",
                              availability: "observed",
                              state: "idle",
                              has_running_internal_workers: false,
                              display_name: "Private Workspace Orion",
                              workdir_attachments: [],
                            }],
                          },
                        },
                      },
                    },
                  }));
                });
              });
              await page.route(
                "**/api/w/home-owner/runtimes/fixture-runtime/workers/private-worker/stop",
                (route) =>
                  route.fulfill({
                    json: {
                      state: "accepted",
                      runtime_id: "fixture-runtime",
                      worker_id: "private-worker",
                      diagnostics: [],
                    },
                  }),
              );
              await page.goto(`${origin}/w/home-owner?fixture=workspace-one`);
              if (width === 390) {
                await page.getByRole("button", { name: "Show sidebar" })
                  .click();
              }
              const actions = page.getByRole("button", {
                name: "Actions for Private Workspace Orion",
              });
              await actions.focus();
              await actions.press("Enter");
              await page.getByRole("menuitem", { name: "Stop", exact: true })
                .click();
              await page.getByText("Private Workspace Orion stopped", {
                exact: true,
              }).waitFor();
              if (width === 390) {
                const hideSidebar = page.getByRole("button", {
                  name: "Hide sidebar",
                });
                await hideSidebar.focus();
                await hideSidebar.press("Enter");
              }
              const accountLink = page.getByRole("link", {
                name: "Open Account",
                exact: true,
              });
              await accountLink.focus();
              await accountLink.press("Enter");
              await page.getByRole("button", { name: "Log out", exact: true })
                .waitFor();
              const retainedCounts = [
                await page.locator(".workspace-alert").count(),
              ];
              await page.getByRole("button", { name: "Log out", exact: true })
                .click();
              await ready(page, "Log in");
              assertEquals(new URL(page.url()).pathname, "/");
              retainedCounts.push(
                await page.locator(".workspace-alert").count(),
              );
              await page.getByRole("link", { name: "Log in", exact: true })
                .click();
              await page.getByRole("button", { name: "Log in with passkey" })
                .waitFor();
              retainedCounts.push(
                await page.locator(".workspace-alert").count(),
              );
              await page.getByRole("link", { name: "Yoi home", exact: true })
                .click();
              await ready(page, "Log in");
              retainedCounts.push(
                await page.locator(".workspace-alert").count(),
              );
              const folder =
                `${root}/target/web-ux/t767-validation/entry-browser/private-alerts-${colorScheme}-${width}`;
              await Deno.mkdir(folder, { recursive: true });
              const bytes = await page.screenshot({
                path: folder + "/viewport.png",
                fullPage: true,
              });
              const source = await new Deno.Command("git", {
                cwd: root,
                args: ["rev-parse", "HEAD"],
                stdout: "piped",
              }).output();
              const dirty = await new Deno.Command("git", {
                cwd: root,
                args: ["status", "--porcelain"],
                stdout: "piped",
              }).output();
              const hash = Array.from(
                new Uint8Array(
                  await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes)),
                ),
              )
                .map((v) => v.toString(16).padStart(2, "0")).join("");
              await Deno.writeTextFile(
                folder + "/review-context.json",
                JSON.stringify(
                  {
                    scenario:
                      "Workspace Stop notification → Account logout → public home → login → public home (SPA)",
                    boundary:
                      "synthetic HTTP and Worker subscription; production UI/router/store",
                    path: new URL(page.url()).pathname,
                    source: {
                      commitSha: new TextDecoder().decode(source.stdout).trim(),
                      dirty:
                        new TextDecoder().decode(dirty.stdout).trim() !== "",
                    },
                    screenshot: { path: "viewport.png", sha256: hash },
                    standaloneNotificationCounts: retainedCounts,
                    colorScheme,
                    viewport: { width, height: 900 },
                    visibleNotifications: await page.locator(".workspace-alert")
                      .allTextContents(),
                    accessibility: await page.locator("body").ariaSnapshot(),
                  },
                  null,
                  2,
                ),
              );
              assertEquals(retainedCounts, [0, 0, 0, 0]);
              assertEquals(await page.locator(".workspace-alert").count(), 0);
              await page.route(
                "**/api/auth/whoami",
                (route) =>
                  route.fulfill({
                    status: 503,
                    body: "synthetic unavailable session",
                  }),
              );
              await focusWithoutSessionCheck(page);
              assertEquals(await page.locator(".workspace-alert").count(), 0);
              await page.reload();
              await page.getByText(/Login status is unknown/).waitFor();
              assertEquals(await page.locator(".workspace-alert").count(), 0);
              assertEquals(
                await page.getByText("Private Workspace Orion stopped", {
                  exact: true,
                }).count(),
                0,
              );
            },
          );
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
            "session retry and page access refresh actor/admin links",
            async (page) => {
              await page.goto(`${origin}/?fixture=session-retry`);
              await page.getByRole("button", { name: "Retry session" }).click();
              await ready(page, "Log in");
              await page.goto(`${origin}/?fixture=admin`);
              await ready(page, "Hosting settings");
              await ready(page, "Open dashboard");
              await page.request.post(`${origin}/api/auth/logout`);
              await focusWithoutSessionCheck(page);
              await page.reload();
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
          await step(
            "pending creation survives same-actor focus without a second POST",
            async (page) => {
              let release!: () => void;
              const held = new Promise<void>((resolve) => {
                release = resolve;
              });
              let started!: () => void;
              const submitted = new Promise<void>((resolve) => {
                started = resolve;
              });
              let posts = 0;
              await page.route("**/api/workspaces", async (route) => {
                if (route.request().method() !== "POST") {
                  return route.continue();
                }
                posts++;
                started();
                await held;
                await route.fulfill({ json: creationResponse() });
              });
              await page.goto(
                `${origin}/dashboard?fixture=account#workspace-create-title`,
              );
              await page.getByLabel("Workspace display name").fill(
                "Pending draft",
              );
              await page.getByRole("button", {
                name: "Create Workspace",
                exact: true,
              }).click();
              await submitted;
              await focusWithoutSessionCheck(page);
              await page.getByRole("button", { name: "Creating…", exact: true })
                .waitFor();
              assertEquals(
                await page.getByRole("button", {
                  name: "Creating…",
                  exact: true,
                }).isDisabled(),
                true,
              );
              assertEquals(
                await page.getByLabel("Workspace display name").inputValue(),
                "Pending draft",
              );
              assertEquals(posts, 1);
              release();
              await page.waitForURL("**/w/home-empty");
              await page.locator(".workspace-home").waitFor();
              assertEquals(posts, 1);
            },
          );
          await step(
            "SPA page access checks once and fences key-list reads without hiding the admitted page",
            async (page) => {
              let checks = 0;
              page.on("request", (request) => {
                if (new URL(request.url()).pathname === "/api/auth/whoami") {
                  checks++;
                }
              });
              await page.goto(origin + "/dashboard?fixture=account");
              await ready(page, "Account");
              assertEquals(checks, 1);
              let release!: () => void;
              let started!: () => void;
              const held = new Promise<void>((resolve) => {
                release = resolve;
              });
              const requested = new Promise<void>((resolve) => {
                started = resolve;
              });
              await page.route("**/api/auth/whoami", async (route) => {
                started();
                await held;
                await route.continue();
              });
              try {
                await page.getByRole("link", { name: "Account", exact: true })
                  .click();
                await requested;
                assertEquals(checks, 2);
                assertEquals(
                  await page.getByRole("heading", {
                    name: "Dashboard",
                    exact: true,
                  }).isVisible(),
                  true,
                );
                assertEquals(
                  await page.getByText("Loading session…").count(),
                  0,
                );
                release();
                await page.getByRole("button", {
                  name: "Add passkey to alice",
                  exact: true,
                }).waitFor();
                assertEquals(new URL(page.url()).pathname, "/account");
                await page.getByRole("list", { name: "Registered keys" })
                  .waitFor();
                // page入場の一回に加え、本人鍵一覧は読取りの前後で本人bindingを確認する。
                assertEquals(checks, 4);
              } finally {
                release();
              }
            },
          );
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
                await focusWithoutSessionCheck(page);
                await page.getByRole("button", {
                  name: "Retry creation",
                  exact: true,
                }).waitFor();
                assertEquals(
                  await page.getByLabel("Workspace display name").inputValue(),
                  "New Workspace",
                );
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
