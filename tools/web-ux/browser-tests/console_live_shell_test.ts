/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Page } from "playwright";
import { Buffer } from "node:buffer";
import {
  observePicker,
  pickerDiagnostics,
  prepareNativePicker,
} from "./console_live_picker_diagnostics.ts";

const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const route = "/w/console-live-review/workers/W-901-console-live-fixture/console";
// Build outside this test. Allows isolated snapshots without writes to production sources.
const buildRoot = Deno.env.get("WEB_UX_BUILD_ROOT") ?? join(root, "web/workspace/build");
const outputRoot = Deno.env.get("WEB_UX_LIFECYCLE_OUTPUT") ??
  join(root, "target/web-ux/console-live-lifecycle");
async function ready(url: string): Promise<void> {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      await response.body?.cancel();
      if (response.ok) return;
    } catch { /* owned server startup */ }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("Owned live fixture did not start");
}
type Method = {
  method: string;
  params?: {
    input?: Array<{ kind: string; invocation?: { identity: string; arguments: unknown[] } }>;
  };
};
type FixtureState = { methods: Method[]; uploads: string[]; deletes: string[] };
async function fixtureState(base: string): Promise<FixtureState> {
  return await (await fetch(`${base}/fixture-state`)).json();
}
async function replaceDraft(page: Page, text: string): Promise<void> {
  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Backspace");
  await page.keyboard.type(text);
}
async function chooseReview(page: Page): Promise<void> {
  await replaceDraft(page, "/rev");
  await page.getByRole("option").filter({ hasText: "review" }).waitFor();
  await page.keyboard.press("Tab");
}
Deno.test("production live Console completes, edits/removes typed drafts and stages/cancels attachment adapter", async () => {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      join(root, "tools/web-ux/browser-tests/console_live_fixture_server.ts"),
      String(port),
      buildRoot,
    ],
    stdout: "null",
    stderr: "inherit",
  }).spawn();
  await Deno.mkdir(outputRoot, { recursive: true });
  try {
    await ready(`${base}/health`);
    const browser = await chromium.launch({ headless: true });
    try {
      for (const theme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390]) {
          const context = await browser.newContext({
            viewport: { width, height: width === 390 ? 844 : 900 },
            colorScheme: theme,
            reducedMotion: "reduce",
          });
          const page = await context.newPage();
          await prepareNativePicker(page);
          await observePicker(page);
          page.setDefaultTimeout(5000);
          const errors: string[] = [];
          const supersededCatalogReads: string[] = [];
          const acknowledgedDeletes = new Set<string>();
          page.on("console", (message) => {
            if (message.type() === "error") errors.push(message.text());
          });
          page.on("pageerror", (error) => errors.push(String(error)));
          page.on("requestfailed", (request) => {
            // Subscription snapshots deliberately supersede the bootstrap GET.
            if (
              request.url() === `${base}/api/w/console-live-review/workers` &&
              request.method() === "GET" &&
              request.failure()?.errorText === "net::ERR_ABORTED"
            ) supersededCatalogReads.push(request.url());
            else errors.push(
              `failed ${request.method()} ${request.url()} ${request.failure()?.errorText}`,
            );
          });
          page.on("response", (response) => {
            if (response.request().method() === "DELETE" && response.status() === 204) {
              acknowledgedDeletes.add(response.url());
            }
            if (response.status() >= 400) {
              errors.push(`HTTP ${response.status()} ${response.url()}`);
            }
          });
          try {
            assertEquals((await page.goto(base + route))?.status(), 200);
            await page.locator(".cm-content[contenteditable=true]").waitFor();
            await page.getByText("Ready. Choose a Feature", { exact: false }).waitFor();
            const start = await fixtureState(base);
            await chooseReview(page);
            await page.getByRole("option").filter({ hasText: "brief" }).waitFor();
            await page.keyboard.type("m");
            await page.getByRole("option").filter({ hasText: "mode=" }).click();
            await page.getByRole("option").filter({ hasText: "brief" }).click();
            await page.keyboard.type(", count=2, enabled=");
            await page.getByRole("option").filter({ hasText: "true" }).click();
            await page.keyboard.type(', path="docs/');
            await page.getByRole("option").filter({ hasText: "docs/release-notes.md" }).click();
            await page.keyboard.type(")");
            await page.locator(".composer-typed-chip").waitFor();
            await page.screenshot({ path: join(outputRoot, `${theme}-${width}-invocation.png`) });
            let current = await fixtureState(base);
            assertEquals(current.uploads, start.uploads, "Feature discovery must not stage files");
            assertEquals(
              current.methods.filter((method) => method.method !== "list_completions"),
              start.methods.filter((method) => method.method !== "list_completions"),
              "completion must not submit, notify or execute controls",
            );
            // Pointer chip edit opens readable syntax, keyboard editing stays unexecuted.
            await page.locator(".composer-typed-chip").click();
            await page.waitForFunction(() =>
              document.querySelector(".cm-content")?.textContent?.includes("count=2")
            );
            for (
              let index = 0;
              index < ', enabled=true, path="docs/release-notes.md"'.length;
              index++
            ) {
              await page.keyboard.press("ArrowLeft");
            }
            await page.keyboard.press("Backspace");
            await page.keyboard.type("3");
            await page.waitForFunction(() =>
              document.querySelector(".cm-content")?.textContent?.includes("count=3")
            );
            await page.screenshot({ path: join(outputRoot, `${theme}-${width}-editing.png`) });
            await page.keyboard.press("Escape");
            await page.keyboard.press("Control+End");
            await page.keyboard.type(" ");
            await page.locator(".composer-typed-chip").waitFor();
            await page.keyboard.press("Backspace");
            await page.keyboard.press("Backspace");
            assertEquals(await page.locator(".composer-typed-chip").count(), 0);
            await page.keyboard.press("Control+z");
            await page.locator(".composer-typed-chip").waitFor();
            // Cancel a native picker; no upload and no accidental invocation remains.
            await replaceDraft(page, "before /att");
            await page.getByRole("option").filter({ hasText: "attach" }).waitFor();
            const picker = page.waitForEvent("filechooser");
            await page.keyboard.press("Enter");
            await (await picker).setFiles([]);
            assertEquals((await fixtureState(base)).uploads, start.uploads);
            assertEquals(await page.locator(".composer-typed-chip").count(), 0);
            assertEquals((await page.locator(".cm-content").innerText()).trim(), "before");
            await page.screenshot({
              path: join(outputRoot, `${theme}-${width}-picker-cancelled.png`),
            });
            // Same generic adapter, this time choose a synthetic local file.
            await page.keyboard.type("/att");
            await page.getByRole("option").filter({ hasText: "attach" }).waitFor();
            const uploadPicker = page.waitForEvent("filechooser");
            await page.keyboard.press("Enter");
            await (await uploadPicker).setFiles({
              name: "release-notes.md",
              mimeType: "text/markdown",
              buffer: Buffer.from("Synthetic release notes"),
            });
            await page.locator(".composer-typed-chip").waitFor();
            await page.screenshot({ path: join(outputRoot, `${theme}-${width}-attachment.png`) });
            current = await fixtureState(base);
            assertEquals(current.uploads.length, start.uploads.length + 1);
            await page.keyboard.press("Control+End");
            await page.keyboard.press("Backspace");
            assertEquals(await page.locator(".composer-typed-chip").count(), 0);
            await page.screenshot({ path: join(outputRoot, `${theme}-${width}-deleted.png`) });
            // Staged resource is retained through undo, then released when draft is reset.
            await page.keyboard.press("Control+z");
            await page.locator(".composer-typed-chip").waitFor();
            await replaceDraft(page, "literal /review(mode=brief)");
            assertEquals(
              await page.locator(".composer-typed-chip").count(),
              0,
              "unselected slash prose stays literal",
            );
            await page.keyboard.press("Escape");
            // Explicit selection becomes the sole structured Feature payload on Submit.
            await chooseReview(page);
            await page.keyboard.press("Escape");
            await page.keyboard.type('mode="thorough", count=3, enabled=false)');
            await page.locator(".composer-typed-chip").waitFor();
            const release = page.waitForResponse((response) =>
              response.request().method() === "DELETE" && response.url().includes("/attachments/")
            );
            await page.keyboard.press("Control+Enter");
            await page.waitForFunction(() => !document.querySelector(".composer-typed-chip"));
            const releasedResponse = await release;
            assertEquals(releasedResponse.status(), 204);
            current = await fixtureState(base);
            assert(
              current.deletes.includes(current.uploads.at(-1)!),
              "accepted draft boundary releases the abandoned uploaded atom",
            );
            const submissions = current.methods.slice(start.methods.length).filter((method) =>
              method.method === "submit"
            );
            assertEquals(submissions.length, 1);
            assertEquals(submissions[0].params?.input?.[0].kind, "feature_invoke");
            assertEquals(submissions[0].params?.input?.[0].invocation?.identity, "fixture.review");
            assertEquals(submissions[0].params?.input?.[0].invocation?.arguments, [
              { name: "mode", value: { kind: "string", value: "thorough" } },
              { name: "count", value: { kind: "integer", value: 3 } },
              { name: "enabled", value: { kind: "boolean", value: false } },
            ]);
            // Chromium may report ERR_ABORTED after receiving the empty 204 cleanup
            // response. Retain that diagnostic, but verify delivery by both response
            // status and server DELETE evidence instead of claiming a lost operation.
            assert(
              await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
              "no page horizontal overflow",
            );
            const pickerEvidence = await pickerDiagnostics(page);
            // Snapshot diagnostics only after the final browser awaits; classification
            // and serialization must use the same immutable set of observed errors.
            const recordedErrors = [...errors];
            const acknowledgedDeleteDiagnostics = recordedErrors.filter((error) =>
              [...acknowledgedDeletes].some((url) =>
                error === `failed DELETE ${url} net::ERR_ABORTED`
              )
            );
            assertEquals(
              recordedErrors.filter((error) => !acknowledgedDeleteDiagnostics.includes(error)),
              [],
            );
            await Deno.writeTextFile(
              join(outputRoot, `${theme}-${width}-result.json`),
              JSON.stringify(
                {
                  theme,
                  width,
                  errors: recordedErrors,
                  supersededCatalogReads,
                  acknowledgedDeleteDiagnostics,
                  acknowledgedDeleteResponseStatus: releasedResponse.status(),
                  picker: pickerEvidence,
                  status: "pass",
                  before: start,
                  after: current,
                },
                null,
                2,
              ),
            );
          } catch (error) {
            await page.screenshot({ path: join(outputRoot, `${theme}-${width}-failure.png`) });
            await Deno.writeTextFile(
              join(outputRoot, `${theme}-${width}-failure.json`),
              JSON.stringify(
                {
                  error: String(error),
                  errors,
                  body: await page.locator("body").innerText(),
                  fixture: await fixtureState(base),
                },
                null,
                2,
              ),
            );
            throw error;
          } finally {
            await context.close();
          }
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
});
