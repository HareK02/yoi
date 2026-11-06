#!/usr/bin/env -S deno run -A
// 本番Web buildと合成HTTP。署名認可・Runtime・保存/queryの検証はRust suiteが所有する。
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
    id: "workdir-management-synthetic-http",
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
  evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend",
  source: new TextDecoder().decode(source.stdout).trim(),
  before,
  pageSourceSha256: await sha256File(
    join(
      root,
      "web/workspace/src/routes/w/[workspaceId]/settings/runtimes/[runtimeId]/workdirs/PageContent.svelte",
    ),
  ),
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  visualVerdict: "UNREVIEWED",
  captures: [] as unknown[],
  failures: [] as string[],
};
const explanation = "Workdir inventory failed: the Runtime could not enumerate materializations";
const omission =
  "Workdir external-checkout was omitted because External grant grant-legacy-permissions uses legacy non-hierarchical permissions; revoke the old grant and share again with READ, WRITE, or COMMAND";
const failure = (reason: string, category: string, explanation: string) => ({
  version: 1,
  required_extensions: [],
  domain: "workdir_management",
  reason,
  category,
  details_version: 1,
  details: { operation: "list", reason },
  explanation,
  context: {
    operations: [],
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
      for (
        const persona of width === 390 && colorScheme === "light" ? ["owner", "member"] : ["owner"]
      ) {
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
                  account_id: persona === "owner" ? "fixture-owner" : "fixture-member",
                  handle: persona,
                  display_name: "Workdir management review",
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
            workspace.permissions.delete_subjects = persona === "owner";
            await route.fulfill({ json: workspace });
          });
          let reads = 0, mutations = 0;
          await context.route("**/runtimes/fixture-runtime/working-directories*", async (route) => {
            if (route.request().method() !== "GET") {
              mutations++;
              await route.abort();
              return;
            }
            reads++;
            if (reads === 1) {
              const response = await route.fetch();
              const inventory = await response.json();
              inventory.diagnostics = [{
                code: "external_workdir_legacy_permissions",
                severity: "warning",
                message: omission,
                failure: JSON.stringify(
                  failure("external_workdir_legacy_permissions", "authorization", omission),
                ),
              }];
              await route.fulfill({ json: inventory });
            } else {
              await route.fulfill({
                status: 400,
                contentType: "application/problem+json",
                json: {
                  type: "urn:yoi:problem:workdir-management",
                  title: "Workdir management failed",
                  status: 400,
                  detail: explanation,
                  instance: "",
                  yoi_failure: failure("working_directory_list_failed", "dependency", explanation),
                },
              });
            }
          });
          const page = await context.newPage();
          page.setDefaultTimeout(10000);
          const pageErrors: string[] = [],
            consoleErrors: string[] = [],
            requestErrors: string[] = [];
          page.on("pageerror", (error) => pageErrors.push(error.message));
          page.on("console", (message) => {
            if (message.type() === "error") consoleErrors.push(message.text());
          });
          page.on(
            "requestfailed",
            (request) =>
              requestErrors.push(`${request.method()} ${new URL(request.url()).pathname}`),
          );
          const workspace = persona === "owner" ? "home-owner" : "home-member";
          const documentResponse = await page.goto(
            `${base}/w/${workspace}/settings/runtimes/fixture-runtime/workdirs`,
          );
          await page.getByRole("heading", { name: "Workdirs", exact: true }).waitFor();
          await page.getByRole("region", { name: "Workdir inventory", exact: true }).waitFor();
          const capture = async (state: string) => {
            await assertNoPageWideOverflow(page);
            assertEquals(mutations, 0);
            const id = `${persona}-${colorScheme}-${width}-${state}`;
            const image = join(output, `${id}.png`);
            await page.screenshot({ path: image });
            await Deno.writeTextFile(
              join(output, `${id}.aria.txt`),
              await page.locator("body").ariaSnapshot(),
            );
            manifest.captures.push({
              id,
              image,
              imageSha256: await sha256File(image),
              route: new URL(page.url()).pathname,
              persona: `synthetic-${persona}`,
              documentStatus: documentResponse?.status(),
              viewport: page.viewportSize(),
              colorScheme,
              visibleAlerts: await page.locator('[role="alert"], [role="status"]')
                .allTextContents(),
              reads,
              mutations,
              pageErrors: [...pageErrors],
              consoleErrors: [...consoleErrors],
              requestErrors: [...requestErrors],
            });
          };
          if (!before) await page.getByText(omission, { exact: true }).waitFor();
          if (persona === "member") {
            assertEquals(
              await page.getByRole("button", { name: "Delete retry-dir", exact: true }).count(),
              0,
            );
          }
          await capture("partial-inventory");
          await page.getByRole("button", { name: "Refresh", exact: true }).focus();
          await page.keyboard.press("Enter");
          await page.getByRole("alert").filter({
            hasText: before ? "could not be refreshed" : explanation,
          }).waitFor();
          assertEquals(reads, 2, "rendering must not retry failed inventory or issue a mutation");
          if (persona === "owner") {
            assert(
              await page.getByRole("button", { name: "Delete retry-dir", exact: true })
                .isDisabled(),
            );
          }
          await capture("refresh-failed");
          assertEquals(pageErrors, []);
          assertEquals(requestErrors, []);
          // 障害fixtureのHTTP400だけを既知console項目とし、その他の描画例外は失敗にする。
          assert(
            consoleErrors.every((error) =>
              error.includes("400") && error.includes("Failed to load resource")
            ),
            consoleErrors.join("\n"),
          );
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
