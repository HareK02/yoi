import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import { loadScenario } from "../../../../tools/web-ux/src/scenario.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
import { creationResponse } from "../../src/lib/workspace/api/workspace-catalog.test-fixtures.ts";

// Browser-only contract over the production static build and loopback API fixtures;
// no actual Backend, Runtime, Worker, account or repository is created.
Deno.test("production empty Workspace supports optional Git validation, reopening, and Workdir-less Worker submission at all review widths", async () => {
  const scenarioPath =
    new URL("./optional-repository.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  async function freePort() {
    const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
    const port = (listener.addr as Deno.NetAddr).port;
    listener.close();
    return port;
  }
  const upstreamPort = await freePort();
  let proxyPort = await freePort();
  while (proxyPort === upstreamPort) proxyPort = await freePort();
  scenario.baseUrl = `http://127.0.0.1:${proxyPort}`;
  scenario.processes![0].args![4] = String(upstreamPort);
  scenario.processes![0].readyUrl = `http://127.0.0.1:${upstreamPort}/health`;
  scenario.processes![1].args![3] = String(proxyPort);
  scenario.processes![1].args![4] = `http://127.0.0.1:${upstreamPort}`;
  scenario.processes![1].readyUrl = `${scenario.baseUrl}/health`;
  const output =
    new URL("../../.svelte-kit/t717/browser/", import.meta.url).pathname;
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    `${output}/process-logs`,
    [],
  );
  try {
    const browser = await chromium.launch({ headless: true });
    try {
      for (const colorScheme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390, 320]) {
          const context = await browser.newContext({
            viewport: { width, height: 900 },
            colorScheme,
            reducedMotion: "reduce",
          });
          try {
            const page = await context.newPage();
            const errors: string[] = [];
            page.on("pageerror", (error) => errors.push(String(error)));
            page.on("console", (message) => {
              if (message.type() === "error") errors.push(message.text());
            });
            page.on("response", (response) => {
              if (response.status() >= 400) {
                errors.push(`${response.status()} ${response.url()}`);
              }
            });
            const creationBodies: Record<string, unknown>[] = [];
            await page.route("**/api/workspaces", async (route) => {
              if (route.request().method() !== "POST") {
                return await route.continue();
              }
              creationBodies.push(route.request().postDataJSON());
              await route.fulfill({ json: creationResponse() });
            });
            await page.goto(scenario.baseUrl!);
            const name = page.getByLabel("Workspace display name");
            await name.fill("New Workspace");
            const toggle = page.getByRole("checkbox", {
              name: "Add an initial repository (optional)",
            });
            await toggle.check();
            await page.getByRole("button", {
              name: "Create Workspace",
              exact: true,
            }).click();
            assertEquals(creationBodies.length, 0); // Native required fields stop partial repository submission.
            await page.getByLabel("Repository key", { exact: true }).fill(
              "Invalid key",
            );
            await page.getByLabel("Initial repository absolute path or URI")
              .fill("/srv/platform");
            assertEquals(
              await page.getByLabel("Repository key", { exact: true }).evaluate(
                (input) =>
                  (input as unknown as { checkValidity(): boolean })
                    .checkValidity(),
              ),
              false,
            );
            await page.getByLabel("Repository key", { exact: true }).fill(
              "platform",
            );
            assertEquals(
              await page.locator("form").evaluate((form) =>
                (form as unknown as { checkValidity(): boolean })
                  .checkValidity()
              ),
              true,
            );
            await Deno.mkdir(output, { recursive: true });
            await page.screenshot({
              path: `${output}/${colorScheme}-${width}-repository.png`,
              fullPage: true,
            });
            assertEquals(
              await page.evaluate(
                "document.documentElement.scrollWidth > innerWidth",
              ),
              false,
              `${colorScheme} ${width}px`,
            );
            await toggle.uncheck();
            await page.getByRole("button", {
              name: "Create Workspace",
              exact: true,
            }).click();
            await page.waitForURL("**/w/home-empty");
            assertEquals(creationBodies.length, 1);
            assertEquals(creationBodies[0].repository, null);
            await page.locator("[data-feed-ready=home-active]").waitFor();
            assertEquals(await page.locator("main [role=alert]").count(), 0);
            // Reopening is based on Workspace identity, not a repository record.
            await page.goto(scenario.baseUrl!);
            await page.locator('a.workspace-catalog-card[href="/w/home-empty"]')
              .click();
            await page.waitForURL("**/w/home-empty");
            await page.goto(`${scenario.baseUrl}/w/home-empty/workers/new`);
            await page.locator(".worker-launch-form").waitFor();
            assert(
              await page.getByRole("button", { name: "Start Worker" })
                .isEnabled(),
            );
            await page.getByRole("combobox", { name: "Runtime", exact: true })
              .selectOption("remote");
            assert(
              await page.getByRole("button", { name: "Start Worker" })
                .isDisabled(),
            );
            await page.getByLabel("Attachment 1 Workdir").selectOption(
              "__new_working_directory__",
            );
            assert(
              await page.getByRole("button", {
                name: "Create Workdir",
                exact: true,
              }).isDisabled(),
            );
            await page.getByRole("combobox", { name: "Runtime", exact: true })
              .selectOption("embedded");
            const submitted = page.waitForRequest((request) =>
              request.url().endsWith("/api/w/home-empty/workers") &&
              request.method() === "POST"
            );
            await page.route("**/api/w/home-empty/workers", async (route) => {
              if (route.request().method() !== "POST") {
                return await route.continue();
              }
              // Successful, synthetic launch projection; the test only exercises browser/API wiring.
              await route.fulfill({
                json: {
                  workspace_id: "home-empty",
                  runtime_id: "embedded",
                  worker_id: "fixture-worker",
                  console_href: "/w/home-empty",
                  diagnostics: [],
                  worker: {
                    runtime_id: "embedded",
                    worker_id: "fixture-worker",
                    host_id: "fixture-host",
                    display_name: "Worker",
                    label: "Worker",
                    profile: "builtin:companion",
                    singleton_key: null,
                    tags: [],
                    workspace: {
                      visibility: "workspace",
                      identity: "home-empty",
                      workspace_id: "home-empty",
                    },
                    state: "idle",
                    last_seen_at: null,
                    pinned: false,
                    retention_state: "active",
                    implementation: { kind: "yoi", display_hint: "Yoi" },
                    workdir_attachments: [],
                    diagnostics: [],
                  },
                },
              });
            });
            await page.getByRole("button", { name: "Start Worker" }).click();
            const body = (await submitted).postDataJSON();
            assertEquals(body.workdir_attachments, []);
            assertEquals(body.ticket_assignment, null);
            assertEquals(body.runtime_id, "embedded");
            await page.waitForURL("**/w/home-empty");
            assertEquals(errors, []);
          } finally {
            await context.close();
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
