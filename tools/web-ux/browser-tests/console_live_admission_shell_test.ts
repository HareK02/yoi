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
const output = Deno.env.get("WEB_UX_LIFECYCLE_OUTPUT") ??
  join(root, "target/web-ux/console-live-lifecycle");
type State = {
  methods: {
    method: string;
    params?: {
      input?: import("../../../web/workspace/src/lib/generated/protocol.ts").Segment[];
      submission_request_id?: string;
    };
  }[];
  events: { event: string }[];
  requests: { method: string; path: string }[];
  uploads: string[];
  upload_attempts: string[];
  deletes: string[];
  held_uploads: string[];
};
const state = async (base: string): Promise<State> =>
  await (await fetch(base + "/fixture-state")).json();
const control = async (base: string, value: unknown): Promise<void> => {
  const response = await fetch(base + "/fixture-control", {
    method: "POST",
    body: JSON.stringify(value),
  });
  assert(response.ok);
  await response.body?.cancel();
};
async function eventually(base: string, predicate: (value: State) => boolean): Promise<State> {
  const deadline = Date.now() + 5000;
  do {
    const value = await state(base);
    if (predicate(value)) return value;
    await new Promise((resolve) => setTimeout(resolve, 25));
  } while (Date.now() < deadline);
  throw new Error("Fixture evidence did not reach expected state");
}
type FixtureBody = (
  page: Page,
  base: string,
  errors: string[],
  caseOutput: string,
) => Promise<void>;
async function fixture(body: FixtureBody, matrix = false): Promise<void> {
  for (const theme of matrix ? ["light", "dark"] as const : ["light"] as const) {
    for (const width of matrix ? [1440, 768, 390] : [1440]) await fixtureCase(body, theme, width);
  }
}
async function fixtureCase(
  body: FixtureBody,
  theme: "light" | "dark",
  width: number,
): Promise<void> {
  const caseOutput = join(output, `${theme}-${width}`);
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
      Deno.env.get("WEB_UX_BUILD_ROOT") ?? join(root, "web/workspace/build"),
    ],
    stdout: "null",
    stderr: "inherit",
  }).spawn();
  try {
    const deadline = Date.now() + 10000;
    let started = false;
    while (!started && Date.now() < deadline) {
      try {
        const response = await fetch(base + "/health");
        started = response.ok;
        await response.body?.cancel();
      } catch { /* bounded owned startup */ }
      if (!started) await new Promise((resolve) => setTimeout(resolve, 25));
    }
    assert(started);
    const browser = await chromium.launch({ headless: true });
    try {
      const page = await browser.newPage({
        viewport: { width, height: width === 390 ? 844 : 900 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      page.setDefaultTimeout(5000);
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(String(error)));
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      await prepareNativePicker(page);
      await observePicker(page);
      await page.goto(base + route);
      await page.locator(".cm-content[contenteditable=true]").waitFor();
      await page.getByText("Ready. Choose a Feature", { exact: false }).waitFor();
      await Deno.mkdir(caseOutput, { recursive: true });
      try {
        await body(page, base, errors, caseOutput);
        await Deno.writeTextFile(
          join(caseOutput, `picker-${crypto.randomUUID()}.json`),
          JSON.stringify(await pickerDiagnostics(page), null, 2),
        );
      } catch (error) {
        const id = crypto.randomUUID();
        await page.screenshot({ path: join(caseOutput, `admission-failure-${id}.png`) });
        await Deno.writeTextFile(
          join(caseOutput, `admission-failure-${id}.json`),
          JSON.stringify(
            {
              error: String(error),
              body: await page.locator("body").innerText(),
              errors,
              picker: await pickerDiagnostics(page),
              fixture: await state(base),
            },
            null,
            2,
          ),
        );
        throw error;
      }
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
}
async function draft(page: Page, text: string): Promise<void> {
  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Backspace");
  await page.keyboard.type(text);
}
async function review(page: Page): Promise<void> {
  await draft(page, "/rev");
  await page.getByRole("option").filter({ hasText: "review" }).waitFor();
  await page.keyboard.press("Tab");
  await page.keyboard.press("Escape");
  await page.keyboard.type('mode="brief", count=2, enabled=true)');
  await page.locator(".composer-typed-chip").waitFor();
}
async function attach(page: Page, name: string): Promise<void> {
  await draft(page, "/att");
  await page.getByRole("option").filter({ hasText: "attach" }).waitFor();
  const chooser = page.waitForEvent("filechooser");
  await page.keyboard.press("Enter");
  const nativeChooser = await chooser;
  const diagnostics = await pickerDiagnostics(page) as {
    kind: string;
    active: boolean;
    connected: boolean;
    disabled?: boolean;
  }[];
  const click = diagnostics.filter((record) => record.kind === "input.click").at(-1);
  assert(
    click?.active && click.connected && !click.disabled,
    "native picker must run synchronously with active real gesture and connected enabled input",
  );
  await nativeChooser.setFiles({
    name,
    mimeType: "text/markdown",
    buffer: Buffer.from("Synthetic fixture document"),
  });
}
Deno.test("production leading slash remains responsive after loop fix", () =>
  fixture(async (page, base, errors, caseOutput) => {
    await draft(page, "/");
    await page.getByRole("option").filter({ hasText: "review" }).waitFor();
    assertEquals(
      (await state(base)).methods.filter((method) => method.method !== "list_completions"),
      [],
    );
    assertEquals((await state(base)).uploads, []);
    assertEquals(errors, []);
    await Deno.writeTextFile(
      join(caseOutput, "slash-fix-result.json"),
      JSON.stringify({ status: "pass", fixture: await state(base), errors }, null, 2),
    );
  }));
Deno.test("production Submit rejection retains typed draft and only authoritative acceptance clears it", () =>
  fixture(async (page, base, errors, caseOutput) => {
    await review(page);
    await attach(page, "admission.md");
    await page.locator(".composer-typed-chip").waitFor();
    // Keep the uploaded atom and add a completion-selected Feature next to it.
    await page.keyboard.press("Control+End");
    await page.keyboard.type(" /rev");
    await page.getByRole("option").filter({ hasText: "review" }).waitFor();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Escape");
    await page.keyboard.type('mode="brief", count=2, enabled=true)');
    await page.waitForFunction(() =>
      document.querySelectorAll(".composer-typed-chip").length === 2
    );
    await control(base, { submit: "reject" });
    await page.keyboard.press("Control+Enter");
    await page.getByText("Synthetic admission rejection; draft was not accepted.", { exact: false })
      .waitFor();
    const rejected = await eventually(
      base,
      (value) => value.events.some((event) => event.event === "submission_rejected"),
    );
    assertEquals(await page.locator(".composer-typed-chip").count(), 2);
    assertEquals(rejected.deletes, [], "rejection must keep staged file ownership");
    await page.screenshot({ path: join(caseOutput, "admission-rejected.png") });
    await control(base, { submit: "hold" });
    await page.locator(".cm-content[contenteditable=true]").waitFor();
    await page.locator(".cm-content").focus();
    await page.keyboard.press("Control+Enter");
    const pending = await eventually(
      base,
      (value) => value.methods.filter((method) => method.method === "submit").length === 2,
    );
    assertEquals(
      await page.locator(".composer-typed-chip").count(),
      2,
      "transport send is not admission",
    );
    assertEquals(pending.deletes, []);
    await page.screenshot({ path: join(caseOutput, "admission-pending.png") });
    await control(base, { accept_pending: true });
    await page.waitForFunction(() =>
      document.querySelectorAll(".composer-typed-chip").length === 0
    );
    const accepted = await eventually(
      base,
      (value) => value.events.some((event) => event.event === "submission_accepted"),
    );
    assertEquals(accepted.deletes, [], "accepted file ownership transfers; no staging DELETE");
    const submissions = accepted.methods.filter((method) => method.method === "submit");
    assertEquals(submissions.length, 2);
    assert(
      submissions.every((method) =>
        method.params?.input?.some((segment) => segment.kind === "uploaded_file")
      ),
    );
    assert(
      submissions.every((method) =>
        method.params?.input?.some((segment) => segment.kind === "feature_invoke")
      ),
    );
    assertEquals(
      submissions[0].params?.input,
      submissions[1].params?.input,
      "retry preserves structured invocation IDs and values",
    );
    assertEquals(errors, []);
    await page.screenshot({ path: join(caseOutput, "admission-accepted.png") });
    await Deno.writeTextFile(
      join(caseOutput, "admission-result.json"),
      JSON.stringify({ status: "pass", rejected, pending, accepted, errors }, null, 2),
    );
  }, true));
Deno.test("production attachment upload failure and in-flight cancellation never Submit and record DELETE", () =>
  fixture(async (page, base, errors, caseOutput) => {
    await control(base, { upload: "fail" });
    await attach(page, "failure.md");
    await page.getByText("Upload failed (503).", { exact: true }).waitFor();
    await page.screenshot({ path: join(caseOutput, "upload-failed.png") });
    await page.getByRole("button", { name: "Remove failure.md", exact: true }).click();
    const failed = await state(base);
    assertEquals(failed.uploads, []);
    assertEquals(failed.methods.filter((method) => method.method === "submit"), []);
    await control(base, { upload: "hold" });
    await attach(page, "cancel.md");
    await eventually(base, (value) => value.held_uploads.length === 1);
    await page.screenshot({ path: join(caseOutput, "upload-pending.png") });
    await page.getByRole("button", { name: "Remove cancel.md", exact: true }).click();
    const cancelled = await eventually(
      base,
      (value) => value.deletes.length > failed.deletes.length && value.held_uploads.length === 0,
    );
    assertEquals(cancelled.uploads, []);
    assertEquals(cancelled.methods.filter((method) => method.method === "submit"), []);
    assertEquals(await page.locator(".composer-typed-chip").count(), 0);
    await page.screenshot({ path: join(caseOutput, "upload-cancelled.png") });
    assert(
      cancelled.requests.filter((request) => request.method === "DELETE").every((request) =>
        request.path.includes("/attachment-uploads/")
      ),
    );
    // The fixture's deliberate HTTP503 emits a browser resource error, not a page exception.
    assert(errors.every((error) => /503/.test(error)), JSON.stringify(errors));
    await Deno.writeTextFile(
      join(caseOutput, "upload-failure-cancellation-result.json"),
      JSON.stringify({ status: "pass", failed, cancelled, expectedErrors: errors }, null, 2),
    );
  }, true));

Deno.test("production upload retry retains upload ID and late completion cannot resurrect deleted reservation", () =>
  fixture(async (page, base, errors, caseOutput) => {
    await control(base, { upload: "fail" });
    await attach(page, "retry.md");
    await page.getByText("Upload failed (503).", { exact: true }).waitFor();
    const failed = await state(base);
    assertEquals(failed.upload_attempts.length, 1);
    await control(base, { upload: "normal" });
    await page.getByRole("button", { name: "Retry", exact: true }).click();
    await page.locator(".composer-typed-chip").filter({ hasText: "Attached retry.md" }).waitFor();
    const retried = await state(base);
    assertEquals(
      retried.upload_attempts,
      [failed.upload_attempts[0], failed.upload_attempts[0]],
      "retry keeps staging identity",
    );
    assertEquals(retried.uploads, [failed.upload_attempts[0]]);
    await page.screenshot({ path: join(caseOutput, "upload-retried.png") });
    // Submit authoritative acceptance is covered separately; this draft is abandoned.
    await draft(page, "deleted upload");
    await page.keyboard.press("Escape");
    // Clearing/accepting this text boundary releases the abandoned staged resource.
    await page.keyboard.press("Control+Enter");
    const deleted = await eventually(base, (value) => value.deletes.includes(retried.uploads[0]));
    await control(base, { upload: "hold" });
    await attach(page, "late.md");
    const held = await eventually(base, (value) => value.held_uploads.length === 1);
    const lateId = held.held_uploads[0];
    await page.locator(".composer-typed-chip").filter({ hasText: "Uploading late.md" }).waitFor();
    // Delete only the reservation, without aborting its real pending upload.
    await page.locator(".cm-content").focus();
    await page.keyboard.press("Control+End");
    await page.keyboard.press("Backspace");
    assertEquals(await page.locator(".composer-typed-chip").count(), 0);
    await page.screenshot({ path: join(caseOutput, "late-upload-deleted.png") });
    const response = page.waitForResponse((response) =>
      response.request().method() === "PUT" && response.url().endsWith(lateId)
    );
    await control(base, { release_uploads: true });
    assertEquals((await response).status(), 200);
    const late = await eventually(
      base,
      (value) => value.uploads.includes(lateId) && value.held_uploads.length === 0,
    );
    assertEquals(
      await page.locator(".composer-typed-chip").count(),
      0,
      "late success cannot resurrect deleted atom",
    );
    assertEquals(
      late.methods.filter((method) => method.method === "submit").length,
      1,
      "upload/retry/late completion never submits",
    );
    // Deleted resources remain undo-owned until the next accepted draft boundary.
    await page.screenshot({ path: join(caseOutput, "late-upload-completed.png") });
    await draft(page, "finish late deletion");
    await page.keyboard.press("Control+Enter");
    const completed = await eventually(base, (value) => value.deletes.includes(lateId));
    assertEquals(completed.methods.filter((method) => method.method === "submit").length, 2);
    assertEquals(
      completed.methods.filter((method) => method.method === "submit").at(-1)?.params?.input,
      [{ kind: "text", content: "finish late deletion" }],
    );
    assert(errors.every((error) => /503/.test(error)), JSON.stringify(errors));
    await Deno.writeTextFile(
      join(caseOutput, "retry-late-result.json"),
      JSON.stringify(
        { status: "pass", failed, retried, deleted, held, late, completed, expectedErrors: errors },
        null,
        2,
      ),
    );
  }, true));

Deno.test("production selected invocation preserves multiple positional typed arguments", () =>
  fixture(async (page, base, errors, caseOutput) => {
    await draft(page, "/rev");
    await page.getByRole("option").filter({ hasText: "review" }).waitFor();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Escape");
    await page.keyboard.type('"brief", 2, true)');
    await page.locator(".composer-typed-chip").waitFor({ timeout: 2000 });
    assertEquals((await state(base)).methods.filter((method) => method.method === "submit"), []);
    assertEquals(errors, []);
    await Deno.writeTextFile(
      join(caseOutput, "positional-result.json"),
      JSON.stringify({ status: "pass", fixture: await state(base), errors }, null, 2),
    );
  }));
