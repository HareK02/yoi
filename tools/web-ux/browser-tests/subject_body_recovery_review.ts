#!/usr/bin/env -S deno run -A
/** 実Webと合成HTTP/WebSocketで失敗予約の回復導線を検証・撮影する。実Runtimeの証拠ではない。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { type Browser, chromium } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const [buildArg, runId, mode, scenario] = Deno.args;
const diagnosticScenario = scenario === "diagnostics";
if (
  !buildArg || !runId || !/^[a-zA-Z0-9._-]+$/.test(runId) || !["before", "after"].includes(mode) ||
  (scenario !== undefined && scenario !== "diagnostics")
) {
  throw new Error(
    "usage: subject_body_recovery_review.ts <actual-build-path> <unique-run-id> <before|after> [diagnostics]",
  );
}
const buildRoot = resolve(buildArg);
const output = join(root, "target/web-ux/body-recovery", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output);
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const faults: string[] = [];
const captures: unknown[] = [];
const source = await new Deno.Command("git", {
  args: ["rev-parse", "HEAD"],
  cwd: root,
  stdout: "piped",
}).output();
const patch = await new Deno.Command("git", { args: ["diff", "HEAD"], cwd: root, stdout: "piped" })
  .output();
if (!source.success || !patch.success) throw new Error("Cannot identify reviewed source");
const sourceDigest = new Uint8Array(await crypto.subtle.digest("SHA-256", patch.stdout));
const manifest = {
  source: {
    commit: new TextDecoder().decode(source.stdout).trim(),
    diffSha256: Array.from(sourceDigest, (b) => b.toString(16).padStart(2, "0")).join(""),
    runnerSha256: await sha256File(fromFileUrl(import.meta.url)),
  },
  evidence: "actual-Web-with-synthetic-HTTP-and-WebSocket-NOT-real-Backend-or-Runtime",
  mode,
  buildRoot,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  captures,
  scenarios: [] as unknown[],
  failures: [] as string[],
  faults,
  status: "running",
  visualVerdict: "unreviewed",
};
const owned = await startOwnedProcesses(
  [{
    id: "body-recovery-fixture",
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
try {
  browser = await chromium.launch({ headless: true });
  for (
    const { width, theme } of [1440, 768, 390].flatMap((width) =>
      (diagnosticScenario ? ["light", "dark"] as const : ["light"] as const).map((theme) => ({
        width,
        theme,
      }))
    )
  ) {
    const context = await browser.newContext({
      viewport: { width, height: 1000 },
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    // Worker notifications only invalidate the observation; the body GET supplies authority.
    let invalidateBody: (() => void) | undefined;
    await context.routeWebSocket("**/protocol/ws", (socket) => {
      const server = socket.connectToServer();
      server.onMessage((data) => {
        const frame = JSON.parse(String(data));
        if (
          frame.frame === "response" && frame.message.result === "subscribed" &&
          frame.message.payload.selector.topic === "workspace_workers"
        ) {
          const subscriptionId = frame.message.payload.subscription_id;
          invalidateBody = () =>
            socket.send(JSON.stringify({
              protocol_version: 2,
              frame: "event",
              message: {
                event: "event",
                data: {
                  subscription_id: subscriptionId,
                  payload: {
                    event: "worker_removed",
                    data: { worker_id: "unrelated-worker", runtime_id: "runtime" },
                  },
                },
              },
            }));
        }
        socket.send(data);
      });
    });
    const page = await context.newPage();
    page.setDefaultTimeout(15000);
    page.on("pageerror", (e) => faults.push(e.message));
    page.on("console", (e) => {
      if (e.type() === "error") faults.push(e.text());
    });
    page.on("response", (response) => {
      if (response.status() >= 400) faults.push(`HTTP ${response.status()} ${response.url()}`);
    });
    page.on("requestfailed", (request) => {
      if (request.failure()?.errorText !== "net::ERR_ABORTED") {
        faults.push(`${request.method()} ${request.url()}: ${request.failure()?.errorText}`);
      }
    });
    const current = { runtime_id: "runtime", worker_id: "failed-worker", generation: "2" };
    let recovered = diagnosticScenario;
    let authority = true;
    const requests: unknown[] = [];
    const observations: unknown[] = [];
    manifest.scenarios.push({ width, theme, requests, observations });
    const body = () => ({
      workspace_id: "memory-review",
      subject_id: "empty-subject",
      current: recovered ? null : current,
      observation: recovered ? { kind: "absent" } : mode === "before"
        ? {
          kind: "unavailable",
          diagnostics: [{
            code: "subject_body_observation_unavailable",
            severity: "warning",
            message: "Unknown Worker",
          }],
        }
        : { kind: "failed_create" },
      can_start: authority,
      can_stop: authority,
    });
    await context.route(
      "**/api/w/memory-review/subjektiv/subjects/empty-subject/body**",
      async (route) => {
        const request = route.request();
        if (request.method() === "GET") {
          const observation = body();
          observations.push(observation);
          return await route.fulfill({ json: observation });
        }
        if (diagnosticScenario) {
          assert(request.url().endsWith("/start"));
          const input = request.postDataJSON();
          requests.push(input);
          return await route.fulfill({
            json: {
              request_id: input.request_id,
              disposition: requests.length === 1 ? "unknown" : "rejected",
              diagnostics: [{
                code: "subject_body_start_outcome_unknown",
                severity: "warning",
                message:
                  'workspace backend config error: failed to resolve Profile "builtin:async-chat": profile source archive import is not declared in manifest import map: ./base.dcdl',
              }],
              body: body(),
            },
          });
        }
        assert(request.url().endsWith("/recover"), "recovery must never dispatch start or stop");
        const input = request.postDataJSON();
        requests.push(input);
        assertEquals(input.expected_current, current);
        assert(!("expected_observation_token" in input));
        if (requests.length > 1) recovered = true;
        await route.fulfill({
          json: {
            request_id: input.request_id,
            disposition: recovered ? "accepted" : "unknown",
            diagnostics: [],
            body: body(),
          },
        });
      },
    );
    // Historical capture mode used HTTP; keep that bridge isolated from current WS assertions.
    if (mode === "before" && !diagnosticScenario) {
      const conversation = {
        id: "conversation",
        subject_id: "empty-subject",
        participant_account_id: "owner",
        last_user_message_id: null,
      };
      await context.route(
        "**/api/w/memory-review/chat/conversations**",
        (route) =>
          route.fulfill({
            json: route.request().url().includes("/messages")
              ? { conversation, messages: [], has_more: false }
              : conversation,
          }),
      );
    }
    const capture = async (state: string) => {
      if (mode === "after" && state !== "failure") {
        await page.locator('section.subject-body[aria-busy="false"]').waitFor();
      }
      await assertNoPageWideOverflow(page);
      const image = join(output, `${width}-${theme}-${state}.png`);
      await page.screenshot({ path: image });
      captures.push({
        state,
        viewport: page.viewportSize(),
        route: new URL(page.url()).pathname,
        image,
        imageSha256: await sha256File(image),
        accessibility: await page.locator("body").ariaSnapshot(),
        visibleAlerts: await page.getByRole("alert").allTextContents(),
      });
    };
    try {
      const document = await page.goto(`${base}/w/memory-review/subjektiv/empty-subject/chat`);
      assertEquals(document?.status(), 200);
      await page.getByText("No public messages yet.").waitFor();
      if (diagnosticScenario) {
        await page.getByText("Subject controls", { exact: true }).click();
        await page.getByRole("button", { name: "Start Subject", exact: true }).click();
        await page.getByText("Could not confirm the operation.", { exact: true }).waitFor();
        await capture("diagnostic-collapsed");
        if (mode === "after") {
          await page.getByText("Operation details", { exact: true }).click();
          await page.getByText(/workspace backend config error: failed to resolve Profile/)
            .waitFor();
          await capture("diagnostic-expanded");
        }
        await page.getByRole("button", { name: "Retry", exact: true }).click();
        await page.getByText("Could not complete the operation.", { exact: true }).waitFor();
        assertEquals(requests.length, 2);
        assertEquals(requests[0], requests[1]);
        await capture("diagnostic-rejected");
        continue;
      }
      if (mode === "before") {
        await page.getByText("Body owner present; observation unavailable.", { exact: true })
          .waitFor();
        await capture("retained");
        continue;
      }
      await page.getByText("Subject controls", { exact: true }).click();
      const recover = page.getByRole("button", { name: "Recover start", exact: true });
      await recover.waitFor();
      await recover.focus();
      await capture("retained");
      await recover.click();
      const retry = page.getByRole("button", { name: "Retry", exact: true });
      await retry.waitFor();
      await page.getByText("Could not confirm the operation.", { exact: true }).waitFor();
      assertEquals(requests.length, 1);
      await capture("unknown");
      await page.reload();
      await page.getByText("Subject controls", { exact: true }).click();
      await page.getByRole("button", { name: "Retry", exact: true }).click();
      await page.getByRole("button", { name: "Start Subject", exact: true }).waitFor();
      assertEquals(requests.length, 2);
      assertEquals(requests[0], requests[1]);
      await capture("released");
      authority = false;
      recovered = false;
      // Preserve the live authority-withdrawal check through the current notification refresh path.
      assert(invalidateBody, "the live Worker subscription must exist before invalidation");
      const observationsBeforeInvalidation = observations.length;
      invalidateBody();
      await page.getByText("Subject controls are unavailable.", { exact: true }).waitFor();
      assert(
        observations.length > observationsBeforeInvalidation,
        "Worker invalidation must trigger a fresh body observation",
      );
      assertEquals(await recover.count(), 0);
      assertEquals(
        await page.getByRole("region", { name: "Subject controls", exact: true })
          .getByRole("button").count(),
        0,
      );
      assertEquals(requests.length, 2, "observation refresh must not dispatch another mutation");
      await capture("no-authority");
    } catch (error) {
      manifest.failures.push(`${width}: ${error instanceof Error ? error.stack : error}`);
      await capture("failure").catch(() => {});
      throw error;
    } finally {
      await context.close();
    }
  }
  assertEquals(faults, []);
  manifest.status = "browser-assertions-passed";
} catch (error) {
  manifest.status = "failed";
  throw error;
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
