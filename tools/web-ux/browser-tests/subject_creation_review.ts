#!/usr/bin/env -S deno run -A
/** 実Webの作成・再送を合成HTTP境界で検証する。実Backendの保存・認可は証明しない。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, relative, resolve } from "@std/path";
import { chromium, type Page } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const [buildArg, runId, modeArg] = Deno.args;
const mode = modeArg ?? (runId?.startsWith("before") ? "before" : "after");
if (
  !buildArg || !runId || !/^[a-zA-Z0-9._-]+$/.test(runId) || !["before", "after"].includes(mode)
) {
  throw new Error(
    "usage: subject_creation_review.ts <actual-build-path> <unique-run-id> [before|after]",
  );
}
const buildRoot = resolve(buildArg);
for (const name of ["index.html", "_app/immutable"]) {
  await Deno.stat(join(buildRoot, name)).catch(() => {
    throw new Error(`Actual Web build missing: ${join(buildRoot, name)}; parent must supply it.`);
  });
}
const output = join(root, "target/web-ux/t806", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output); // 既存runの画像や判断を上書きしない。

async function git(args: string[]) {
  const result = await new Deno.Command("git", { args, cwd: root, stdout: "piped" }).output();
  return result.success ? new TextDecoder().decode(result.stdout).trim() : null;
}
async function inventory(path: string): Promise<{ path: string; sha256: string }[]> {
  const files: { path: string; sha256: string }[] = [];
  for await (const entry of Deno.readDir(path)) {
    const child = join(path, entry.name);
    if (entry.isDirectory) files.push(...await inventory(child));
    else if (entry.isFile) {
      files.push({ path: relative(buildRoot, child), sha256: await sha256File(child) });
    }
  }
  return files.sort((a, b) => a.path.localeCompare(b.path));
}
const buildFiles = await inventory(buildRoot);
const sourceStatus = await git(["status", "--porcelain"]);
const manifest = {
  evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend",
  mode,
  buildRoot,
  buildFiles,
  source: {
    commitSha: await git(["rev-parse", "HEAD"]),
    dirty: sourceStatus === null ? null : sourceStatus.length > 0,
    status: sourceStatus,
    captureSources: await Promise.all([
      "tools/web-ux/browser-tests/subject_creation_review.ts",
      "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
      "tools/web-ux/browser-tests/memory_document_harness.ts",
    ].map(async (path) => ({ path, sha256: await sha256File(join(root, path)) }))),
    buildProvenance:
      "parent-supplied build; checkout metadata is capture-time, not a build attestation",
  },
  visualVerdict: "UNREVIEWED — parent must ViewImage and inspect review-context.json",
  status: "running",
  browser: { name: "chromium", version: "" },
  expectedFaults: [
    "Per width/theme: first POST reply aborted only after synthetic persistence (HTTP 201)",
    ...(mode === "after"
      ? ["Per width/theme: second POST intentionally returns HTTP 403 before fixture"]
      : []),
  ],
  baselineLimit: mode === "before"
    ? "Capture only. Missing operation_id is added ONLY to fixture transport for synthetic persistence; original browser body is recorded. No replay or disabled-input success asserted."
    : null,
  captures: [] as unknown[],
  scenarios: [] as unknown[],
  failures: [] as string[],
};
async function saveManifest() {
  await Deno.writeTextFile(join(output, "review-context.json"), JSON.stringify(manifest, null, 2));
}
type Fault = {
  kind: string;
  message: string;
  url?: string;
  method?: string;
  expected: boolean;
};
function observe(page: Page, createUrl: string) {
  const faults: Fault[] = [];
  let documentStatus: number | null = null;
  let allowAbort = false;
  let allow403 = false;
  page.on(
    "pageerror",
    (e) => faults.push({ kind: "pageerror", message: e.stack ?? e.message, expected: false }),
  );
  page.on("console", (e) => {
    if (!["warning", "error"].includes(e.type())) return;
    const url = e.location().url;
    // 意図した同一endpointのネットワーク診断だけを許容し、pageerrorは常に失敗にする。
    const expected = url === createUrl && (
      (allowAbort && /Failed to load resource: net::ERR_FAILED/.test(e.text())) ||
      (allow403 && /Failed to load resource:.*403/.test(e.text()))
    );
    faults.push({ kind: `console-${e.type()}`, message: e.text(), url, expected });
  });
  page.on("requestfailed", (r) =>
    faults.push({
      kind: "requestfailed",
      message: r.failure()?.errorText ?? "request failed",
      url: r.url(),
      method: r.method(),
      expected: allowAbort && r.url() === createUrl && r.method() === "POST" &&
        r.failure()?.errorText === "net::ERR_FAILED",
    }));
  page.on("response", (r) => {
    if (r.request().isNavigationRequest()) documentStatus = r.status();
    if (r.status() >= 400) {
      faults.push({
        kind: "http",
        message: `HTTP ${r.status()}`,
        url: r.url(),
        method: r.request().method(),
        expected: allow403 && r.url() === createUrl && r.request().method() === "POST" &&
          r.status() === 403,
      });
    }
  });
  return {
    faults,
    documentStatus: () => documentStatus,
    expectAbort: () => {
      allowAbort = true;
    },
    expect403: () => {
      allow403 = true;
    },
  };
}
async function controls(page: Page) {
  const role = page.getByRole("textbox", { name: "Role", exact: true });
  const behavior = page.getByRole("textbox", { name: /Behavior/ });
  if (!await role.count()) return null;
  return {
    role: await role.inputValue(),
    behavior: await behavior.inputValue(),
    roleDisabled: await role.isDisabled(),
    behaviorDisabled: await behavior.isDisabled(),
    submitting: await page.locator("#subject-create-panel").getAttribute("aria-busy"),
  };
}
async function capture(page: Page, observation: ReturnType<typeof observe>, id: string) {
  const image = join(output, `${id}.png`);
  const snapshot = join(output, `${id}.aria.txt`);
  await page.screenshot({ path: image });
  await Deno.writeTextFile(snapshot, await page.locator("body").ariaSnapshot());
  manifest.captures.push({
    id,
    route: new URL(page.url()).pathname,
    viewport: page.viewportSize(),
    theme: await page.evaluate(() =>
      (globalThis as any).matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
    ),
    documentStatus: observation.documentStatus(),
    visibleAlerts: await page.getByRole("alert").allTextContents(),
    controls: await controls(page),
    focus: await page.locator(":focus").evaluateAll((elements) =>
      elements.map((e) => ({ tag: e.tagName, text: e.textContent, id: e.id }))
    ),
    mainBounds: await page.locator("main").evaluate((e) => ({
      width: e.clientWidth,
      scrollTop: e.scrollTop,
    })),
    image,
    imageSha256: await sha256File(image),
    accessibilitySnapshot: snapshot,
    accessibilitySha256: await sha256File(snapshot),
    faults: [...observation.faults],
    unexpectedFaults: observation.faults.filter((f) => !f.expected),
    reviewed: false,
  });
  await saveManifest();
  await assertNoPageWideOverflow(page);
  assertEquals(observation.documentStatus(), 200, `${id}: document must render successfully`);
}
const roleText = "  Release steward — evidence and uncertainty  ";
const behaviorText =
  "Challenge unsupported assumptions before release.\nAsk before irreversible actions; preserve the exact pending request.\nEvidence: https://example.com/review/subject-creation";
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
async function assertPending(page: Page) {
  assertEquals(await controls(page), {
    role: roleText,
    behavior: behaviorText,
    roleDisabled: true,
    behaviorDisabled: true,
    submitting: "false",
  });
  assert(await page.getByRole("button", { name: "Create Subject", exact: true }).isEnabled());
}
async function reopen(page: Page) {
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.locator("#subject-create-panel").waitFor({ state: "detached" });
  assert(
    await page.getByRole("button", { name: "New Subject", exact: true }).evaluate((e) =>
      e === e.ownerDocument.activeElement
    ),
  );
  await page.getByRole("button", { name: "New Subject", exact: true }).click();
  await page.locator('#subject-create-panel[aria-busy="false"]').waitFor();
}
const browser = await chromium.launch({ headless: true });
manifest.browser.version = browser.version();
try {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const prefix = `${theme}-${width}`;
      const port = await freePort();
      const base = `http://127.0.0.1:${port}`;
      const createUrl = `${base}/api/w/memory-review/subjektiv/subjects`;
      const owned = await startOwnedProcesses(
        [{
          id: `subject-create-${prefix}`,
          command: Deno.execPath(),
          cwd: join(root, "tools/web-ux"),
          args: [
            "run",
            "-A",
            join(root, "tools/web-ux/browser-tests/memory_document_fixture_server.ts"),
            String(port),
            buildRoot,
            "3",
          ],
          readyUrl: `${base}/health`,
        }],
        join(output, "scenario.json"),
        join(output, `logs-${prefix}`),
        [],
      );
      const context = await browser.newContext({
        viewport: { width, height: width === 390 ? 844 : 900 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(15_000);
      const observation = observe(page, createUrl);
      const requests: unknown[] = [];
      const transportOverrides: unknown[] = [];
      const scenario = {
        prefix,
        requests,
        transportOverrides,
        syntheticEvidence: null as unknown,
        faults: observation.faults,
      };
      manifest.scenarios.push(scenario);
      try {
        await page.goto(`${base}/w/memory-review/memory`);
        await page.getByRole("heading", { name: "Subjects", level: 1, exact: true }).waitFor();
        await page.getByRole("button", { name: "New Subject", exact: true }).click();
        const role = page.getByRole("textbox", { name: "Role", exact: true });
        const behavior = page.getByRole("textbox", { name: /Behavior/ });
        await role.fill(roleText);
        await behavior.fill(behaviorText);
        await role.focus();
        await capture(page, observation, `${prefix}-normal-form`);
        await page.keyboard.press("Tab");
        assert(await behavior.evaluate((e) => e === e.ownerDocument.activeElement));
        await page.keyboard.press("Tab");
        assert(
          await page.getByRole("button", { name: "Create Subject", exact: true }).evaluate((e) =>
            e === e.ownerDocument.activeElement
          ),
        );
        await capture(page, observation, `${prefix}-normal-actions-keyboard`);

        let attempt = 0;
        await page.route(createUrl, async (route) => {
          if (route.request().method() !== "POST") return await route.continue();
          attempt += 1;
          const body = route.request().postDataJSON();
          requests.push(body);
          if (attempt === 1) {
            observation.expectAbort();
            // 旧buildの比較撮影だけ、fixtureへ送るコピーにoperation_idを足す。
            // 実browser要求はrequestsに未変更で保存し、旧UIの契約成功とは扱わない。
            const bridged = mode === "before" && !body.operation_id;
            const transport = bridged ? { ...body, operation_id: crypto.randomUUID() } : body;
            if (bridged) {
              transportOverrides.push({
                attempt,
                reason: "baseline-only fixture bridge",
                body: transport,
              });
            }
            const persisted = await route.fetch({ postData: JSON.stringify(transport) });
            assertEquals(
              persisted.status(),
              201,
              "Fault must follow synthetic persistence, not validation rejection",
            );
            await persisted.dispose();
            await route.abort("failed");
          } else if (attempt === 2 && mode === "after") {
            observation.expect403();
            await route.fulfill({
              status: 403,
              contentType: "application/json",
              body: JSON.stringify({
                error: "Forbidden",
                message: "synthetic later permission rejection",
              }),
            });
          } else await route.continue();
        });
        await page.getByRole("button", { name: "Create Subject", exact: true }).click();
        await page.getByRole("alert").filter({ hasText: /unknown|may have been created/i })
          .waitFor();
        await page.locator('#subject-create-panel[aria-busy="false"]').waitFor();
        await page.getByRole("alert").scrollIntoViewIfNeeded();
        await capture(page, observation, `${prefix}-unknown-after-persisted-response-loss`);
        await role.scrollIntoViewIfNeeded();
        await capture(page, observation, `${prefix}-unknown-fields`);
        if (mode === "after") {
          await assertPending(page);
          const first = requests[0] as { operation_id: string; role: string; behavior_md: string };
          assert(uuid.test(first.operation_id), "UI must generate a UUID per creation attempt");
          assertEquals(first, {
            operation_id: first.operation_id,
            role: roleText,
            behavior_md: behaviorText,
          });
          await reopen(page);
          await assertPending(page);
          await page.getByRole("alert").scrollIntoViewIfNeeded();
          await capture(page, observation, `${prefix}-cancel-reopen-pending`);
          await page.getByRole("button", { name: "Create Subject", exact: true }).click();
          await page.getByRole("alert").filter({ hasText: /do not have permission/ }).waitFor();
          await page.locator('#subject-create-panel[aria-busy="false"]').waitFor();
          assertEquals(
            requests[1],
            first,
            "Retry after cancel/reopen must retain the complete request",
          );
          await assertPending(page);
          await page.getByRole("alert").scrollIntoViewIfNeeded();
          await capture(page, observation, `${prefix}-later-403-still-pending`);
          await role.scrollIntoViewIfNeeded();
          await capture(page, observation, `${prefix}-later-403-disabled-fields`);
          await reopen(page);
          await assertPending(page);
          await page.getByRole("alert").scrollIntoViewIfNeeded();
          await capture(page, observation, `${prefix}-later-403-reopened-pending`);
          await page.getByRole("button", { name: "Create Subject", exact: true }).click();
          await page.waitForURL("**/memory/created-subject-0001");
          await page.getByRole("heading", { name: roleText.trim(), level: 1, exact: true })
            .waitFor();
          assertEquals(
            requests,
            [first, first, first],
            "All retries must use the same operation and full input",
          );
          const article = page.getByRole("article", { name: "User-managed Subject behavior" });
          await article.waitFor();
          assert((await article.textContent())?.includes("preserve the exact pending request."));
          await page.getByText("No committed Memories yet.").waitFor();
          await capture(page, observation, `${prefix}-recovered-actual-detail`);
        }
        scenario.syntheticEvidence = await (await fetch(`${base}/fixture/subject-create-evidence`))
          .json();
        const evidence = scenario.syntheticEvidence as {
          created: { id: string; role: string; behavior_md: string }[];
          requests: unknown[];
          outcomes: unknown[];
        };
        assertEquals(evidence.created.length, 1, "Only one synthetic Subject may be created");
        assertEquals(evidence.created[0].role, roleText);
        assertEquals(evidence.created[0].behavior_md, behaviorText);
        if (mode === "after") {
          assertEquals(evidence.requests, [requests[0], requests[0]]);
          assertEquals(evidence.outcomes, [
            { outcome: "created", subject_id: "created-subject-0001" },
            { outcome: "replay", subject_id: "created-subject-0001" },
          ]);
          assertEquals(observation.faults.filter((f) => f.kind === "http" && f.expected).length, 1);
        }
        assertEquals(
          observation.faults.filter((f) => f.kind === "requestfailed" && f.expected).length,
          1,
        );
        assertEquals(
          observation.faults.filter((f) => !f.expected),
          [],
          `${prefix}: unexpected browser faults`,
        );
        await Deno.writeTextFile(
          join(output, `${prefix}-synthetic-http-evidence.json`),
          JSON.stringify(scenario, null, 2),
        );
      } catch (e) {
        manifest.failures.push(
          `${prefix}: ${e instanceof Error ? e.stack ?? e.message : String(e)}`,
        );
        await capture(page, observation, `${prefix}-failure`).catch((captureError) =>
          manifest.failures.push(`Failure capture: ${captureError}`)
        );
        throw e;
      } finally {
        try {
          scenario.syntheticEvidence = await fetch(`${base}/fixture/subject-create-evidence`)
            .then((response) => response.json()).catch(() => scenario.syntheticEvidence);
          await Deno.writeTextFile(
            join(output, `${prefix}-synthetic-http-evidence.json`),
            JSON.stringify(scenario, null, 2),
          );
        } finally {
          try {
            await context.close();
          } finally {
            await stopOwnedProcesses(owned);
            await saveManifest();
          }
        }
      }
    }
  }
  assertEquals(
    await inventory(buildRoot),
    buildFiles,
    "Parent build changed during the capture matrix",
  );
  manifest.status = mode === "before"
    ? "baseline-capture-only-visual-unreviewed"
    : "browser-assertions-passed-visual-unreviewed";
} catch (e) {
  manifest.status = "failed";
  throw e;
} finally {
  await browser.close();
  await saveManifest();
  console.log(join(output, "review-context.json"));
}
