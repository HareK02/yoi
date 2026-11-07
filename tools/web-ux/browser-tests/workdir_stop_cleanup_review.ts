#!/usr/bin/env -S deno run -A
// 本番Web buildと合成HTTP/WS。実RuntimeのStop・close・認可の証拠はRust suiteが所有する。
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const buildRoot = resolve(Deno.args[0]);
const runId = Deno.args[1];
const before = Deno.args[2] === "before";
if (!runId || !/^[a-zA-Z0-9._-]+$/.test(runId)) throw new Error("supply a unique safe run id");
const output = join(root, "target/web-ux/t815", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output);
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const processes = await startOwnedProcesses(
  [{
    id: "stop-cleanup-synthetic-http",
    command: Deno.execPath(),
    cwd: root,
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      "tools/web-ux/browser-tests/workspace_home_fixture_server.ts",
      String(port),
      buildRoot,
    ],
    readyUrl: `${base}/health`,
  }],
  import.meta.url,
  join(output, "processes"),
  [],
);
const source = await new Deno.Command("git", { args: ["rev-parse", "HEAD"], cwd: root }).output();
const manifest = {
  evidence: "actual-Web-with-synthetic-HTTP-WS-NOT-real-Backend",
  source: new TextDecoder().decode(source.stdout).trim(),
  before,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  visualVerdict: "UNREVIEWED",
  captures: [] as unknown[],
  failures: [] as string[],
};
const explanation =
  "the Workdir provider is unavailable; Workdir session closing could not complete";
const cleanup = JSON.stringify({
  version: 1,
  required_extensions: [],
  domain: "workdir",
  reason: "provider_unavailable",
  category: "dependency",
  details_version: 1,
  details: {
    operation: "session_close",
    classification: "unavailable",
    denial_reason: null,
    failure_reason: "provider_unavailable",
  },
  explanation,
  context: {
    operations: [{ operation: "session_close", stage: "provider", operation_ref: null }],
    causes: [],
    diagnostics: [],
    correlation: null,
    supplementary_omitted: false,
  },
});
const browser = await chromium.launch({ headless: true });
try {
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        colorScheme,
        reducedMotion: "reduce",
      });
      try {
        await context.route("**/api/auth/whoami", (route) =>
          route.fulfill({
            json: {
              authentication_reference: "fixture-authentication",
              session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
              actor: {
                user_id: "fixture-user",
                account_id: "fixture-owner",
                handle: "reviewer",
                display_name: "Workdir cleanup review",
                auth_method: "browser_session",
              },
            },
          }));
        await context.route(
          "**/api/hosting/access",
          (route) => route.fulfill({ json: { is_admin: false } }),
        );
        await context.route("**/api/w/*/workspace", async (route) => {
          const response = await route.fetch();
          const workspace = await response.json();
          workspace.permissions.delete_subjects = true;
          await route.fulfill({ json: workspace });
        });
        let stops = 0;
        const worker = () => ({
          worker_id: "cleanup-worker",
          resource_key: "W-1",
          runtime_id: "fixture-runtime",
          availability: "observed",
          state: stops ? "stopped" : "idle",
          has_running_internal_workers: false,
          workspace_id: "home-owner",
          display_name: "Cleanup checkout",
          profile: null,
          workdir_attachments: [],
        });
        await context.routeWebSocket("**/protocol/ws", (socket) =>
          socket.onMessage((message) => {
            const frame = JSON.parse(String(message));
            if (frame?.message?.method !== "subscribe_events") return;
            socket.send(
              JSON.stringify({
                protocol_version: 2,
                frame: "response",
                message: {
                  result: "subscribed",
                  payload: {
                    request_id: frame.message.params.request_id,
                    subscription_id: "fixture-workers",
                    selector: { topic: "workspace_workers" },
                    snapshot: { topic: "workers", data: { workers: [worker()] } },
                  },
                },
              }),
            );
          }));
        await context.route("**/workers/cleanup-worker/stop", (route) => {
          stops++;
          return route.fulfill({
            json: {
              state: "accepted",
              replayed: false,
              runtime_id: "fixture-runtime",
              worker_id: "cleanup-worker",
              diagnostics: [],
              cleanup_failures: [cleanup],
            },
          });
        });
        const page = await context.newPage();
        page.setDefaultTimeout(10000);
        const errors: string[] = [];
        page.on("pageerror", (error) => errors.push(error.message));
        page.on("console", (message) => {
          if (message.type() === "error") errors.push(message.text());
        });
        page.on(
          "requestfailed",
          (request) => errors.push(`${request.method()} ${new URL(request.url()).pathname}`),
        );
        const documentResponse = await page.goto(`${base}/w/home-owner`);
        await page.getByRole("heading", { name: "Activity", exact: true }).waitFor();
        const sidebarToggle = page.getByRole("button", { name: "Show sidebar", exact: true });
        if (await sidebarToggle.count()) await sidebarToggle.click();
        await page.getByRole("button", { name: "Actions for Cleanup checkout", exact: true })
          .focus();
        await page.keyboard.press("Enter");
        await page.getByRole("menuitem", { name: "Stop", exact: true }).click();
        const alert = page.getByRole(before ? "alert" : "status").filter({
          hasText: before ? "Worker stop failed" : "Stop accepted; cleanup needs attention",
        });
        await alert.waitFor();
        if (!before) {
          assert((await alert.innerText()).includes(explanation));
          assert(!(await alert.innerText()).includes("Worker stop failed"));
        }
        assertEquals(stops, 1, "rendering must not retry Stop or cleanup");
        await assertNoPageWideOverflow(page);
        const id = `${colorScheme}-${width}-accepted-stop-cleanup`;
        const image = join(output, `${id}.png`);
        await page.screenshot({ path: image });
        const aria = await page.locator("body").ariaSnapshot();
        await Deno.writeTextFile(join(output, `${id}.aria.txt`), aria);
        manifest.captures.push({
          id,
          image,
          imageSha256: await sha256File(image),
          route: new URL(page.url()).pathname,
          persona: "synthetic-owner",
          documentStatus: documentResponse?.status(),
          viewport: page.viewportSize(),
          colorScheme,
          visibleAlerts: await page.locator('[role="alert"], [role="status"]').allTextContents(),
          errors,
          stops,
        });
        assertEquals(errors, []);
      } catch (error) {
        const page = context.pages()[0];
        if (page) {
          await page.screenshot({ path: join(output, "failed.png") });
          await Deno.writeTextFile(
            join(output, "failed.aria.txt"),
            await page.locator("body").ariaSnapshot(),
          );
        }
        throw error;
      } finally {
        await context.close();
      }
    }
  }
} catch (error) {
  manifest.failures.push(String(error));
  throw error;
} finally {
  await browser.close();
  await stopOwnedProcesses(processes);
  await Deno.writeTextFile(
    join(output, "review-context.json"),
    JSON.stringify(manifest, null, 2) + "\n",
  );
}
