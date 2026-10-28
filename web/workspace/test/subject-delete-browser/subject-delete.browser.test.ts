import { assert, assertEquals } from "@std/assert";
import { chromium, type Page } from "playwright";
import { join, resolve } from "@std/path";
import {
  assertNoPageWideOverflow,
  freePort,
  waitForServer,
} from "../../../../tools/web-ux/browser-tests/memory_document_harness.ts";

const root = resolve(import.meta.dirname!, "../../../..");
const artifact = join(root, "target/web-ux/t786-browser");
async function confirm(page: Page) {
  await page.locator(".subject-delete-open").click();
  const button = page.getByRole("button", {
    name: "Permanently delete Subject",
    exact: true,
  });
  assert(await button.isDisabled());
  await page.getByRole("checkbox").check();
  await button.click();
}
Deno.test({
  name:
    "Subject deletion shell: owner confirmation, pending/reload/retry, unknown/rejected and route fencing",
  sanitizeOps: false,
  sanitizeResources: false,
  fn: async () => {
    const build = await new Deno.Command(Deno.execPath(), {
      args: ["task", "build"],
      cwd: join(root, "web/workspace"),
      stdout: "null",
      stderr: "piped",
    }).output();
    assert(build.success, new TextDecoder().decode(build.stderr));
    const port = await freePort();
    const server = new Deno.Command(Deno.execPath(), {
      args: [
        "run",
        "-A",
        join(import.meta.dirname!, "fixture-server.ts"),
        String(port),
        join(root, "web/workspace/build"),
      ],
      stdout: "null",
      stderr: "inherit",
    }).spawn();
    const origin = `http://127.0.0.1:${port}`;
    await Deno.mkdir(artifact, { recursive: true });
    const browser = await chromium.launch();
    try {
      await waitForServer(`${origin}/health`);
      for (const theme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390]) {
          const context = await browser.newContext({
            viewport: { width, height: 900 },
            colorScheme: theme,
          });
          const page = await context.newPage();
          const pageErrors: string[] = [];
          page.on("pageerror", (error) => pageErrors.push(error.message));
          const requests: unknown[] = [];
          let release!: () => void;
          const held = new Promise<void>((resolve) => {
            release = resolve;
          });
          await page.route(
            "**/subjektiv/subjects/pending-subject",
            async (route) => {
              if (route.request().method() !== "DELETE") {
                return route.continue();
              }
              requests.push(route.request().postDataJSON());
              if (requests.length === 1) await held;
              await route.continue();
            },
          );
          await page.goto(`${origin}/w/memory-review/memory/pending-subject`);
          await page.locator(".subject-delete-open").waitFor();
          await page.screenshot({
            path: join(artifact, `${theme}-${width}-initial.png`),
          });
          await page.locator(".subject-delete-open").click();
          assert(
            await page.getByText("This is irreversible.", { exact: true })
              .isVisible(),
          );
          await assertNoPageWideOverflow(page);
          await page.screenshot({
            path: join(artifact, `${theme}-${width}-confirmation.png`),
          });
          await page.getByRole("button", { name: "Cancel", exact: true })
            .click();
          assertEquals(requests.length, 0);
          await confirm(page);
          await page.locator('[data-delete-state="running"]').waitFor();
          assertEquals(await page.locator(".subject-overview").count(), 0);
          await page.screenshot({
            path: join(artifact, `${theme}-${width}-running.png`),
          });
          release();
          await page.locator('[data-delete-state="pending"]').waitFor();
          assertEquals(requests.length, 1);
          const saved = await page.evaluate(() =>
            Object.keys(sessionStorage).filter((key) =>
              key.startsWith("yoi.subject-delete:")
            ).map((key) => JSON.parse(sessionStorage.getItem(key)!))
          );
          assertEquals(saved, [{
            subjectId: "pending-subject",
            operationId: (requests[0] as { operation_id: string }).operation_id,
          }]);
          await page.screenshot({
            path: join(artifact, `${theme}-${width}-pending.png`),
          });
          await page.reload();
          await page.locator('[data-delete-state="unknown"]').waitFor();
          assertEquals(await page.locator(".subject-overview").count(), 0);
          await page.locator(".subject-delete-retry").click();
          await page.locator('[data-delete-state="completed"]').waitFor();
          assertEquals(requests.length, 2);
          assertEquals(requests[0], requests[1]);
          assertEquals(
            await page.evaluate(() =>
              Object.keys(sessionStorage).filter((key) =>
                key.startsWith("yoi.subject-delete:")
              )
            ),
            [],
          );
          await page.screenshot({
            path: join(artifact, `${theme}-${width}-completed.png`),
          });
          await assertNoPageWideOverflow(page);
          assertEquals(pageErrors, []);
          await context.close();
        }
      }
      for (const subject of ["unknown-subject", "legacy-subject"]) {
        const page = await browser.newPage();
        const requests: unknown[] = [];
        page.on("request", (request) => {
          if (request.method() === "DELETE") {
            requests.push(request.postDataJSON());
          }
        });
        await page.goto(`${origin}/w/memory-review/memory/${subject}`);
        await page.locator(".subject-delete-open").waitFor();
        if (subject === "legacy-subject") {
          assert(
            await page.getByText("Legacy ended Subject", { exact: true })
              .isVisible(),
          );
        }
        await confirm(page);
        await page.locator(
          `[data-delete-state="${
            subject === "unknown-subject" ? "unknown" : "pending"
          }"]`,
        ).waitFor();
        await page.locator(".subject-delete-retry").click();
        await page.locator('[data-delete-state="completed"]').waitFor();
        assertEquals(requests.length, 2);
        assertEquals(requests[0], requests[1]);
        await page.close();
      }
      // 4xxでもkeyを捨てない。権限の変化を新しい依頼へ置換しない。
      {
        const page = await browser.newPage();
        const requests: unknown[] = [];
        await page.route(
          "**/subjektiv/subjects/pending-subject",
          async (route) => {
            if (route.request().method() !== "DELETE") return route.continue();
            requests.push(route.request().postDataJSON());
            if (requests.length === 1) {
              await route.fulfill({
                status: 403,
                json: { error: "forbidden", message: "Permission changed" },
              });
            } else {await route.fulfill({
                json: {
                  subject_id: "pending-subject",
                  operation_id: (requests[0] as any).operation_id,
                  completed: true,
                },
              });}
          },
        );
        await page.goto(`${origin}/w/memory-review/memory/pending-subject`);
        await confirm(page);
        await page.locator('[data-delete-state="failed"]').waitFor();
        await page.screenshot({ path: join(artifact, "rejected.png") });
        await page.locator(".subject-delete-retry").click();
        await page.locator('[data-delete-state="completed"]').waitFor();
        assertEquals(requests[0], requests[1]);
        await page.close();
      }
      // 同一SPA内で他対象へ移動してから旧応答が届いても、新対象を消さない。
      {
        const page = await browser.newPage();
        let release!: () => void;
        const held = new Promise<void>((resolve) => {
          release = resolve;
        });
        await page.route(
          "**/subjektiv/subjects/pending-subject",
          async (route) => {
            if (route.request().method() !== "DELETE") return route.continue();
            const body = route.request().postDataJSON();
            await held;
            await route.fulfill({
              json: {
                subject_id: "pending-subject",
                operation_id: body.operation_id,
                completed: true,
              },
            });
          },
        );
        await page.goto(`${origin}/w/memory-review/memory/pending-subject`);
        await confirm(page);
        await page.locator('[data-delete-state="running"]').waitFor();
        await page.getByRole("link", { name: "← All subjects", exact: true })
          .click();
        await page.locator(".subject-list a").first().click();
        await page.locator(".subject-overview").waitFor();
        const target = page.url();
        const response = page.waitForResponse((r) =>
          r.request().method() === "DELETE"
        );
        release();
        await response;
        assertEquals(page.url(), target);
        assertEquals(
          await page.locator('[data-delete-state="completed"]').count(),
          0,
        );
        assertEquals(await page.locator(".subject-overview").count(), 1);
        await page.close();
      }
      {
        const page = await browser.newPage();
        await page.goto(
          `${origin}/w/memory-review/memory/legacy-subject?persona=reader`,
        );
        await page.locator(".subject-overview").waitFor();
        assertEquals(await page.locator(".subject-deletion").count(), 0);
        await page.screenshot({ path: join(artifact, "non-owner.png") });
        await page.close();
      }
    } finally {
      await browser.close();
      server.kill("SIGTERM");
      await server.status;
    }
  },
});
