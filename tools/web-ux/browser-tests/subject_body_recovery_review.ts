#!/usr/bin/env -S deno run -A
/** 実Webと合成HTTPで失敗予約の回復導線を検証・撮影する。実Runtimeの証拠ではない。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { type Browser, chromium } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const [buildArg, runId, mode] = Deno.args;
if (
  !buildArg || !runId || !/^[a-zA-Z0-9._-]+$/.test(runId) || !["before", "after"].includes(mode)
) {
  throw new Error(
    "usage: subject_body_recovery_review.ts <actual-build-path> <unique-run-id> <before|after>",
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
  evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend-or-Runtime",
  mode,
  buildRoot,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  captures,
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
  for (const width of [1440, 768, 390]) {
    const context = await browser.newContext({
      viewport: { width, height: 1000 },
      colorScheme: "light",
      reducedMotion: "reduce",
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
    let recovered = false;
    let authority = true;
    const requests: unknown[] = [];
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
        if (request.method() === "GET") return await route.fulfill({ json: body() });
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
    const capture = async (state: string) => {
      await assertNoPageWideOverflow(page);
      const image = join(output, `${width}-${state}.png`);
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
      if (mode === "before") {
        await page.getByText("Body owner present; observation unavailable.", { exact: true })
          .waitFor();
        await capture("retained");
        continue;
      }
      const recover = page.getByRole("button", { name: "Release failed start reservation" });
      await recover.waitFor();
      await recover.focus();
      await capture("retained");
      await recover.click();
      const retry = page.getByRole("button", { name: "Retry same recover request" });
      await retry.waitFor();
      await capture("unknown");
      await page.reload();
      await page.getByRole("button", { name: "Retry same recover request" }).click();
      await page.getByRole("button", { name: "Start conversation body", exact: true }).waitFor();
      assertEquals(requests.length, 2);
      assertEquals(requests[0], requests[1]);
      await capture("released");
      authority = false;
      recovered = false;
      await page.getByRole("button", { name: "Refresh body state" }).click();
      await page.getByText("Body creation failed; reservation retained.", { exact: true })
        .waitFor();
      assertEquals(await recover.count(), 0);
      await capture("no-authority");
    } finally {
      await context.close();
    }
  }
  assertEquals(faults, []);
  manifest.status = "browser-assertions-passed";
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
