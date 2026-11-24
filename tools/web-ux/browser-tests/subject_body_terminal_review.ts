#!/usr/bin/env -S deno run -A
/** T881: production Web + existing synthetic HTTP/WS fixture; no live Backend/Runtime. */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { type Browser, chromium } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const [buildArg, runId, mode, sourceCommit] = Deno.args;
if (
  !buildArg || !runId || !/^[\w.-]+$/.test(runId) || !["before", "after"].includes(mode) ||
  !/^[a-f0-9]{40}$/.test(sourceCommit ?? "")
) {
  throw new Error(
    "usage: subject_body_terminal_review.ts <build> <unique-run-id> <before|after> <exact-built-commit>",
  );
}
const buildRoot = resolve(buildArg);
const output = join(root, "target/web-ux/body-terminal", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output);
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const faults: string[] = [];
const captures: unknown[] = [];
const scenarios: unknown[] = [];
const manifest = {
  sourceCommit,
  buildRoot,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  runnerSha256: await sha256File(fromFileUrl(import.meta.url)),
  mode,
  evidence: "production-Web-with-synthetic-HTTP-and-WebSocket-NOT-real-Backend-or-Runtime",
  actor: "owner",
  route: "/w/memory-review/subjektiv/empty-subject/chat",
  captures,
  scenarios,
  faults,
  failures: [] as string[],
  status: "running",
  visualVerdict: "unreviewed",
};
const owned = await startOwnedProcesses(
  [{
    id: "body-terminal-fixture",
    command: Deno.execPath(),
    cwd: join(root, "tools/web-ux"),
    args: [
      "run",
      "--cached-only",
      "-A",
      join(root, "tools/web-ux/browser-tests/memory_document_fixture_server.ts"),
      String(port),
      buildRoot,
      "3",
      "body-recovery",
    ],
    readyUrl: `${base}/health`,
  }],
  join(output, "scenario.json"),
  join(output, "logs"),
  [],
);
let browser: Browser | undefined;
const oldRef = { runtime_id: "runtime", worker_id: "old-worker", generation: "2" };
const currentRef = { runtime_id: "runtime", worker_id: "current-worker", generation: "3" };
const storageKey = 'yoi.subject-body:["owner","memory-review","empty-subject"]';
const frozen = {
  accountId: "owner",
  workspaceId: "memory-review",
  subjectId: "empty-subject",
  action: "stop",
  request: {
    request_id: "persisted-stop-request",
    expected_current: oldRef,
    expected_observation_token: "old-observation-token",
  },
};
const cases = [
  {
    name: "interrupted",
    outcome: "interrupted",
    observation: "absent",
    action: "Start Subject",
    endpoint: "start",
  },
  {
    name: "failed-timeout",
    outcome: { failed: { cause: "timed_out" } },
    observation: "idle",
    action: "Stop Subject",
    endpoint: "stop",
  },
  {
    name: "failed-persistence",
    outcome: { failed: { cause: "stop_terminated_persistence_outcome_unknown" } },
    observation: "stopped",
    action: "Start Subject",
    endpoint: "start",
  },
  {
    name: "failed-cleanup",
    outcome: { failed: { cause: "stop_terminated_cleanup_failed" } },
    observation: "failed_create",
    action: "Recover start",
    endpoint: "recover",
  },
];
try {
  browser = await chromium.launch({
    headless: true,
    executablePath: Deno.env.get("WEB_UX_CHROMIUM_EXECUTABLE"),
  });
  for (const width of [1440, 768, 390]) {
    for (const theme of ["light", "dark"] as const) {
      for (const scenario of cases) {
        const context = await browser.newContext({
          viewport: { width, height: 1000 },
          colorScheme: theme,
          reducedMotion: "reduce",
        });
        const posts: { endpoint: string; input: unknown }[] = [];
        const observations: unknown[] = [];
        const outcomeReads: unknown[] = [];
        let outcome: unknown = "pending";
        let holdNextGet = false;
        let releaseGet: (() => void) | undefined;
        let getHeld: (() => void) | undefined;
        const held = new Promise<void>((resolve) => {
          getHeld = resolve;
        });
        const gate = new Promise<void>((resolve) => {
          releaseGet = resolve;
        });
        const body = () => ({
          workspace_id: "memory-review",
          subject_id: "empty-subject",
          can_start: true,
          can_stop: true,
          current: scenario.observation === "absent" ? null : currentRef,
          observation: scenario.observation === "absent"
            ? { kind: "absent" }
            : scenario.observation === "failed_create"
            ? { kind: "failed_create" }
            : {
              kind: "available",
              worker: {
                runtime_id: "runtime",
                worker_id: "current-worker",
                resource_key: "W-1",
                host_id: "host",
                label: "Current body",
                workspace: {
                  visibility: "workspace",
                  identity: "memory-review",
                  workspace_id: "memory-review",
                },
                implementation: { kind: "embedded", display_hint: "" },
                availability: "observed",
                state: scenario.observation,
                worker_state: scenario.observation === "stopped"
                  ? null
                  : { state_sequence: 0, last_command_id: 1, state: { kind: "idle" } },
                restore_observation_token: "current-observation-token",
              },
            },
        });
        scenarios.push({
          width,
          theme,
          name: scenario.name,
          terminalOutcome: scenario.outcome,
          currentBody: body(),
          posts,
          outcomeReads,
          observations,
        });
        await context.route(
          "**/api/w/memory-review/subjektiv/subjects/empty-subject/body**",
          async (route) => {
            const req = route.request();
            if (req.method() === "GET") {
              if (holdNextGet) {
                holdNextGet = false;
                getHeld?.();
                await gate;
              }
              const value = body();
              observations.push(value);
              return await route.fulfill({ json: value });
            }
            const input = req.postDataJSON();
            const endpoint = new URL(req.url()).pathname.split("/").at(-1)!;
            if (endpoint === "outcome") {
              outcomeReads.push(input);
              assertEquals(
                input,
                frozen.request,
                "historical query must retain the entire old request",
              );
              // Deliberately stale response body: actions MUST use independent current GET.
              return await route.fulfill({
                json: {
                  request_id: input.request_id,
                  outcome,
                  body: {
                    workspace_id: "memory-review",
                    subject_id: "empty-subject",
                    current: oldRef,
                    observation: { kind: "failed_create" },
                    can_start: false,
                    can_stop: false,
                  },
                },
              });
            }
            posts.push({ endpoint, input });
            return await route.fulfill({
              json: {
                request_id: input.request_id,
                disposition: "accepted",
                diagnostics: [],
                body: body(),
              },
            });
          },
        );
        await context.addInitScript(({ key, value }) => {
          if (!sessionStorage.getItem("terminal-fixture-seeded")) {
            sessionStorage.setItem(key, JSON.stringify(value));
            sessionStorage.setItem("terminal-fixture-seeded", "yes");
          }
        }, { key: storageKey, value: frozen });
        const page = await context.newPage();
        page.setDefaultTimeout(10000);
        page.on("pageerror", (e) => faults.push(e.message));
        page.on("console", (e) => {
          if (e.type() === "error") faults.push(e.text());
        });
        page.on("response", (r) => {
          if (r.status() >= 400) faults.push(`HTTP ${r.status()} ${r.url()}`);
        });
        page.on("requestfailed", (r) => {
          if (r.failure()?.errorText !== "net::ERR_ABORTED") {
            faults.push(`${r.method()} ${r.url()}: ${r.failure()?.errorText}`);
          }
        });
        const panel = page.getByRole("region", { name: "Subject controls", exact: true });
        const saved = () => page.evaluate((key) => sessionStorage.getItem(key), storageKey);
        const check = () => panel.getByRole("button", { name: "Check result", exact: true });
        const idle = () => page.locator('section.subject-body[aria-busy="false"]').waitFor();
        const capture = async (state: string) => {
          await idle();
          await assertNoPageWideOverflow(page);
          const image = join(output, `${width}-${theme}-${scenario.name}-${state}.png`);
          await page.screenshot({ path: image });
          captures.push({
            state,
            scenario: scenario.name,
            viewport: page.viewportSize(),
            theme,
            route: new URL(page.url()).pathname,
            documentStatus: 200,
            image,
            imageSha256: await sha256File(image),
            accessibility: await page.locator("body").ariaSnapshot(),
            visibleAlerts: await page.getByRole("alert").allTextContents(),
            panelText: await panel.innerText(),
            savedAttempt: await saved(),
          });
        };
        try {
          const doc = await page.goto(`${base}${manifest.route}`);
          assertEquals(doc?.status(), 200);
          await page.getByText("No public messages yet.", { exact: true }).waitFor();
          await page.getByText("Subject controls", { exact: true }).click();
          await check().waitFor();
          await idle();
          assertEquals(JSON.parse((await saved())!), frozen);
          await check().focus();
          await capture("unknown");
          await check().click();
          await idle();
          assertEquals(JSON.parse((await saved())!), frozen, "pending retains saved request");
          outcome = "not_found";
          await check().click();
          await idle();
          assertEquals(JSON.parse((await saved())!), frozen, "not_found retains saved request");
          await page.reload();
          await page.getByText("Subject controls", { exact: true }).click();
          await check().waitFor();
          await idle();
          assertEquals(
            JSON.parse((await saved())!),
            frozen,
            "GET/reload must not clear frozen input",
          );
          outcome = scenario.outcome;
          if (mode === "after") holdNextGet = true;
          await check().click();
          if (mode === "after") {
            await held;
            assertEquals(await saved(), null, "only terminal result clears frozen request");
            const freshAction = panel.getByRole("button", { name: scenario.action, exact: true });
            assert(await freshAction.isDisabled(), "fresh action waits for independent body GET");
            releaseGet?.();
            await idle();
            assert(await freshAction.isEnabled(), "current body GET permits fresh action");
            assertEquals(await check().count(), 0);
            const terminalAlert = panel.getByRole("alert").filter({
              hasText:
                "The operation ended without confirmed success. Check operation details and current observations.",
            });
            assert(
              await terminalAlert.isVisible(),
              "known terminal failure/interruption must be visible before opening details",
            );
            assertEquals(await panel.locator("details[open]").count(), 0);
            await capture("terminal-collapsed");
            await panel.getByText("Operation details", { exact: true }).click();
            const text = await panel.innerText();
            assert(
              text.includes(
                scenario.name === "interrupted"
                  ? "The Stop request was interrupted."
                  : "The Stop request failed (",
              ),
            );
            assert(!text.includes("Could not confirm the operation."));
            await capture("terminal-expanded");
            await freshAction.focus();
            await page.keyboard.press("Enter");
            await idle();
            assertEquals(posts.length, 1);
            assertEquals(posts[0].endpoint, scenario.endpoint);
            const input = posts[0].input as typeof frozen.request;
            assert(input.request_id !== frozen.request.request_id);
            assertEquals(
              input.expected_current,
              scenario.observation === "absent" ? null : currentRef,
            );
            if (scenario.endpoint !== "recover") {
              assertEquals(
                input.expected_observation_token,
                scenario.observation === "absent" ? null : "current-observation-token",
              );
            } else assert(!("expected_observation_token" in input));
            await capture("fresh-action");
          } else {
            await idle();
            assertEquals(
              JSON.parse((await saved())!),
              frozen,
              "baseline incorrectly retains terminal request (reproduced)",
            );
            await capture("terminal-collapsed");
            await panel.getByText("Operation details", { exact: true }).click();
            assert((await panel.innerText()).includes("Stop outcome could not be checked."));
            await capture("terminal-expanded");
            assertEquals(posts.length, 0);
          }
        } catch (e) {
          releaseGet?.();
          manifest.failures.push(
            `${width}/${theme}/${scenario.name}: ${e instanceof Error ? e.stack : String(e)}`,
          );
          await capture("failure").catch(() => {});
          throw e;
        } finally {
          releaseGet?.();
          await context.close();
        }
      }
    }
  }
  assertEquals(faults, []);
  manifest.status = mode === "before" ? "baseline-bug-reproduced" : "browser-assertions-passed";
} catch (e) {
  manifest.status = "failed";
  throw e;
} finally {
  try {
    await browser?.close();
  } finally {
    await stopOwnedProcesses(owned);
    await Deno.writeTextFile(
      join(output, "review-context.json"),
      JSON.stringify(manifest, null, 2),
    );
  }
}
