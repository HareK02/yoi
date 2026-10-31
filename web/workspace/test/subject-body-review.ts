#!/usr/bin/env -S deno run -A
/** Actual Web build + synthetic HTTP only. No real Backend/Runtime is contacted. */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium } from "playwright";
import { sha256File } from "../../../tools/web-ux/src/artifacts.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../tools/web-ux/src/processes.ts";
import {
  assertNoPageWideOverflow,
  freePort,
} from "../../../tools/web-ux/browser-tests/memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const run = Deno.args[0] ?? "review";
if (!/^[\w-]+$/.test(run)) throw new Error("safe unique run id required");
const output = join(root, "target/t807-web", run);
await Deno.mkdir(output); // Never replace previous evidence.
const buildRoot = join(root, "web/workspace/build");
await Deno.stat(join(buildRoot, "index.html"));
const git = async (args: string[]) => {
  const result = await new Deno.Command("git", {
    args,
    cwd: root,
    stdout: "piped",
  }).output();
  assert(result.success);
  return new TextDecoder().decode(result.stdout).trim();
};
const manifest = {
  evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend-or-Runtime",
  source: {
    commit: await git(["rev-parse", "HEAD"]),
    status: await git(["status", "--short"]),
  },
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  browser: "",
  status: "running",
  visualVerdict: "unreviewed",
  expectedFaults: [
    "structured session.record rejection",
    "unknown start/stop receipts",
    "stale generation rejection",
    "503 body GET",
    "navigation teardown aborts GET /workers refresh",
  ],
  captures: [] as unknown[],
  assertions: [] as string[],
  failures: [] as string[],
};
const persist = () =>
  Deno.writeTextFile(
    join(output, "review-context.json"),
    JSON.stringify(manifest, null, 2),
  );
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const owned = await startOwnedProcesses(
  [{
    id: "synthetic-memory-http",
    command: Deno.execPath(),
    cwd: join(root, "tools/web-ux"),
    args: [
      "run",
      "--cached-only",
      "-A",
      "browser-tests/memory_document_fixture_server.ts",
      String(port),
      buildRoot,
      "3",
    ],
    readyUrl: `${base}/health`,
  }],
  join(output, "scenario.json"),
  join(output, "logs"),
  [],
);
let browser;
try {
  browser = await chromium.launch({ headless: true });
  manifest.browser = browser.version();
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const prefix = `${colorScheme}-${width}`;
      const context = await browser.newContext({
        viewport: { width, height: width === 390 ? 844 : 900 },
        colorScheme,
        reducedMotion: "reduce",
      });
      try {
        let accountId = "owner";
        let observation: "absent" | "idle" | "stopped" | "unavailable" =
          "absent";
        let permission = true;
        let getFailure = false;
        let action: "reject" | "unknown" | "accepted" | "stale" = "reject";
        let outcome: "pending" | "stopped" = "pending";
        let generation = "17";
        const posts: { path: string; body: string }[] = [];
        const owner = () => ({
          runtime_id: "runtime",
          worker_id: "worker",
          generation,
        });
        function body(workspace: string, subject: string) {
          return {
            workspace_id: workspace,
            subject_id: subject,
            can_start: permission,
            can_stop: permission,
            current: observation === "absent" ? null : owner(),
            observation: observation === "absent"
              ? { kind: "absent" }
              : observation === "unavailable"
              ? {
                kind: "unavailable",
                diagnostics: [{
                  code: "runtime_unavailable",
                  severity: "warning",
                  message:
                    "Runtime observation unavailable; no replacement body will be started.",
                }],
              }
              : {
                kind: "available",
                worker: {
                  runtime_id: "runtime",
                  worker_id: "worker",
                  resource_key: "W-1",
                  host_id: "host",
                  label: "Conversation body",
                  workspace: {
                    visibility: "workspace",
                    identity: workspace,
                    workspace_id: workspace,
                  },
                  implementation: { kind: "embedded", display_hint: "" },
                  availability: "observed",
                  state: observation,
                  worker_state: observation === "stopped"
                    ? null
                    : { last_command_id: 1, state: { kind: "idle" } },
                  restore_observation_token:
                    `token-${generation}-${observation}`,
                },
              },
          };
        }
        await context.route("**/api/auth/whoami", async (route) => {
          const response = await route.fetch();
          const value = await response.json();
          value.actor.account_id = accountId;
          await route.fulfill({ response, json: value });
        });
        // The second Workspace uses a synthetic, silent protocol channel too.
        await context.routeWebSocket(
          "**/api/w/other-review/protocol/ws",
          (socket) => {
            socket.onMessage(() => {});
          },
        );
        // Reuse the existing isolated fixture for a second Workspace identity.
        await context.route(
          /\/api\/w\/other-review(?:\/.*)?$/,
          async (route) => {
            const response = await route.fetch({
              url: route.request().url().replace(
                "/api/w/other-review/",
                "/api/w/memory-review/",
              ),
            });
            await route.fulfill({
              response,
              body: (await response.text()).replaceAll(
                "memory-review",
                "other-review",
              ),
            });
          },
        );
        await context.route(
          /\/api\/w\/[^/]+\/subjektiv\/subjects\/[^/]+\/body(?:\/.*)?$/,
          async (route) => {
            const request = route.request();
            const path = new URL(request.url()).pathname;
            const [, workspace, subject] = path.match(
              /\/api\/w\/([^/]+)\/subjektiv\/subjects\/([^/]+)\/body/,
            )!;
            if (request.method() === "GET") {
              await route.fulfill({
                status: getFailure ? 503 : 200,
                json: getFailure
                  ? { error: "Synthetic body observation unavailable" }
                  : body(workspace, subject),
              });
              return;
            }
            const raw = request.postData()!;
            posts.push({ path, body: raw });
            const input = JSON.parse(raw);
            assert(!("accountId" in input));
            if (path.endsWith("/outcome")) {
              await route.fulfill({
                json: { request_id: input.request_id, outcome, body: null },
              });
              return;
            }
            if (action === "stale") generation = "18";
            if (action === "accepted" && path.endsWith("/start")) {
              observation = "idle";
            }
            await route.fulfill({
              json: {
                request_id: input.request_id,
                disposition: action === "reject" || action === "stale"
                  ? "rejected"
                  : action,
                diagnostics: action === "reject"
                  ? [{
                    code: "permission_missing",
                    severity: "error",
                    message:
                      "Required permission session.record is missing. Ask an administrator to review the default profile; no permissions were added.",
                  }]
                  : action === "stale"
                  ? [{
                    code: "stale_owner",
                    severity: "warning",
                    message:
                      "The current body generation changed. Refresh before acting.",
                  }]
                  : [],
                body: body(workspace, subject),
              },
            });
          },
        );
        const page = await context.newPage();
        const errors: unknown[] = [];
        let documentStatus: number | null = null;
        page.on(
          "pageerror",
          (e) => errors.push({ kind: "page", message: e.message }),
        );
        page.on("console", (e) => {
          if (["warning", "error"].includes(e.type())) {
            errors.push({ kind: "console", message: e.text() });
          }
        });
        page.on(
          "requestfailed",
          (r) =>
            errors.push({
              kind: "request",
              url: r.url(),
              message: r.failure()?.errorText,
            }),
        );
        page.on("response", (r) => {
          if (r.request().isNavigationRequest()) documentStatus = r.status();
          if (r.status() >= 400) {
            errors.push({ kind: "http", status: r.status(), url: r.url() });
          }
        });
        const panel = () =>
          page.getByRole("region", { name: "Conversation body", exact: true });
        const button = (name: string) =>
          panel().getByRole("button", { name, exact: true });
        async function ready() {
          await page.locator(".subject-body header button:not(:disabled)")
            .waitFor();
        }
        async function refresh() {
          await ready();
          await button("Refresh body state").click();
          await ready();
        }
        async function capture(name: string, expectedErrors: string[] = []) {
          await panel().scrollIntoViewIfNeeded();
          const id = `${prefix}-${name}`;
          const image = join(output, `${id}.png`);
          const aria = join(output, `${id}.aria.txt`);
          const panelImage = join(output, `${id}.panel.png`);
          await page.screenshot({ path: image });
          await panel().screenshot({ path: panelImage });
          await Deno.writeTextFile(
            aria,
            await page.locator("body").ariaSnapshot(),
          );
          manifest.captures.push({
            id,
            route: new URL(page.url()).pathname,
            accountId,
            viewport: page.viewportSize(),
            theme: colorScheme,
            documentStatus,
            visibleAlerts: await page.getByRole("alert").allTextContents(),
            bodyText: await panel().innerText(),
            errors: [...errors],
            expectedErrors,
            image,
            imageSha256: await sha256File(image),
            panelImage,
            panelImageSha256: await sha256File(panelImage),
            accessibilitySnapshot: aria,
            reviewed: false,
          });
          await persist();
          await assertNoPageWideOverflow(page);
        }
        // Enter through the ordinary Subjects list, not a special-purpose shell.
        await page.goto(`${base}/w/memory-review/memory`);
        await page.locator('a.subject-row[href$="/empty-subject"]').click();
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        await button("Refresh body state").focus();
        await page.keyboard.press("Tab");
        assert(
          await button("Start conversation body").evaluate((el) =>
            el === el.ownerDocument.activeElement
          ),
        );
        await capture("before-absent-focus");
        await page.keyboard.press("Enter");
        await panel().getByText("Request rejected.", { exact: true }).waitFor();
        await ready();
        await capture("permission-rejection");
        action = "unknown";
        await button("Start conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const unknownInput = posts.at(-1)!.body;
        observation = "idle";
        await refresh();
        assertEquals(await button("Start conversation body").count(), 0);
        await capture("unknown-after-get");
        await page.reload();
        await panel().getByText(/An unresolved start request is retained/)
          .waitFor();
        await ready();
        action = "accepted";
        await button("Retry same start request").click();
        await panel().getByText("Request accepted.", { exact: true }).waitFor();
        await ready();
        assertEquals(posts.at(-1)!.body, unknownInput);
        await capture("running-reconciled");
        action = "unknown";
        await button("Stop conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const stopInput = posts.at(-1)!.body;
        await button("Check stop outcome").click();
        await panel().getByText("Stop outcome pending.", { exact: true })
          .waitFor();
        await ready();
        observation = "stopped";
        await refresh();
        assertEquals(await button("Restart conversation body").count(), 0);
        await capture("stopped-but-unresolved");
        outcome = "stopped";
        await button("Check stop outcome").click();
        await panel().getByText("Historical stop confirmed.", { exact: true })
          .waitFor();
        await ready();
        assertEquals(posts.at(-1)!.body, stopInput);
        await capture("stop-confirmed");
        action = "accepted";
        await button("Restart conversation body").click();
        await panel().getByText("Request accepted.", { exact: true }).waitFor();
        await ready();
        const restart = JSON.parse(posts.at(-1)!.body);
        assertEquals(restart.expected_observation_token, "token-17-stopped");
        assertEquals(restart.expected_current.generation, "17");
        assert(restart.request_id !== JSON.parse(stopInput).request_id);
        await capture("restarted-running");
        action = "stale";
        await button("Stop conversation body").click();
        await panel().getByText(
          "The current body generation changed. Refresh before acting.",
          { exact: true },
        ).waitFor();
        await ready();
        assertEquals(
          JSON.parse(posts.at(-1)!.body).expected_current.generation,
          "17",
        );
        await capture("old-generation-rejected");
        permission = false;
        await refresh();
        assertEquals(await button("Stop conversation body").count(), 0);
        await capture("read-only");
        permission = true;
        observation = "unavailable";
        await refresh();
        assertEquals(await button("Start conversation body").count(), 0);
        await capture("unavailable-owner");
        // Account identity changes must not expose another account's retained retry.
        observation = "absent";
        action = "unknown";
        await refresh();
        await button("Start conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const beforeIdentitySwitch = posts.length;
        accountId = "other-account";
        await page.reload();
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        assertEquals(await button("Retry same start request").count(), 0);
        assertEquals(posts.length, beforeIdentitySwitch);
        // Ordinary back-to-list and Subject navigation must isolate the old request too.
        await page.getByRole("link", { name: "← All subjects", exact: true })
          .click();
        await page.locator('a.subject-row[href$="/stale-subject"]').click();
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        assertEquals(await button("Retry same start request").count(), 0);
        await capture("identity-subject-switched");
        await button("Start conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const beforeWorkspaceSwitch = posts.length;
        await page.goto(`${base}/w/other-review/memory/stale-subject`);
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        assertEquals(await button("Retry same start request").count(), 0);
        assertEquals(posts.length, beforeWorkspaceSwitch);
        await capture("workspace-switched");
        await page.goto(`${base}/w/memory-review/memory/stale-subject`);
        await panel().getByText(/An unresolved start request is retained/)
          .waitFor();
        await ready();
        assertEquals(posts.length, beforeWorkspaceSwitch);
        getFailure = true;
        await refresh();
        await panel().getByRole("alert").waitFor();
        assert(await button("Retry same start request").isDisabled());
        await capture("get-failure", [
          "intentional 503 body GET; browser console reports failed resource",
        ]);
        const unexpected = errors.filter((e: any) => {
          if (e.kind === "request") {
            return !(e.message === "net::ERR_ABORTED" &&
              /^\/api\/w\/(memory-review|other-review)\/workers$/.test(
                new URL(e.url).pathname,
              ));
          }
          if (e.kind === "http") {
            return e.status !== 503 || !e.url.endsWith("/body");
          }
          if (e.kind === "console") {
            return !e.message.includes("503 (Service Unavailable)");
          }
          return true;
        });
        assertEquals(unexpected, [], "unexpected browser diagnostics");
        manifest.assertions.push(
          `${prefix}: normal entry, keyboard focus/activation, missing session.record, unknown GET/reload/exact retry, pending stop, confirmed stop/restart, old generation, read-only, unavailable owner, account, Workspace and Subject isolation, GET failure`,
        );
      } finally {
        await context.close();
      }
    }
  }
  manifest.status = "pass";
} catch (error) {
  manifest.status = "fail";
  manifest.failures.push(
    error instanceof Error ? error.stack ?? error.message : String(error),
  );
  throw error;
} finally {
  await persist();
  await browser?.close();
  await stopOwnedProcesses(owned);
}
console.log(output);
