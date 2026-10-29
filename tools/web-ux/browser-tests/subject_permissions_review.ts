#!/usr/bin/env -S deno run -A
/** 実Web buildを合成HTTPで操作する。実Backend認証やWorker動作の証拠ではない。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium, type Page } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const buildRoot = resolve(Deno.args[0] ?? join(root, "web/workspace/build"));
const runId = Deno.args[1] ?? `after-${new Date().toISOString().replaceAll(/[:.]/g, "-")}`;
if (!/^[a-zA-Z0-9._-]+$/.test(runId)) throw new Error("run id must be a safe filename");
// Web buildは親の責務。欠落時は即失敗させ、合成shellで代替しない。
for (const name of ["index.html", "_app/immutable"]) {
  await Deno.stat(join(buildRoot, name)).catch(() => {
    throw new Error(`Actual Web build missing: ${join(buildRoot, name)}; parent must supply it.`);
  });
}
const output = join(root, "target/web-ux/t-802", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output); // 同じrun IDで既存の証拠を上書きしない。
const sourceCommit = await new Deno.Command("git", {
  args: ["rev-parse", "HEAD"],
  cwd: root,
  stdout: "piped",
}).output();
const sourceStatus = await new Deno.Command("git", {
  args: ["status", "--porcelain"],
  cwd: root,
  stdout: "piped",
}).output();
const manifest: {
  evidence: string;
  buildRoot: string;
  buildIndexSha256: string;
  visualVerdict: string;
  status: string;
  captures: unknown[];
  failures: string[];
  source: { commitSha: string | null; dirty: boolean | null };
  browser: { name: string; version: string } | null;
  expectedFaults: string[];
} = {
  evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend",
  buildRoot,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  visualVerdict: "UNREVIEWED — parent must ViewImage and inspect diagnostics before pass",
  status: "running",
  captures: [],
  failures: [],
  source: {
    commitSha: sourceCommit.success ? new TextDecoder().decode(sourceCommit.stdout).trim() : null,
    dirty: sourceStatus.success
      ? new TextDecoder().decode(sourceStatus.stdout).trim().length > 0
      : null,
  },
  browser: null,
  expectedFaults: [
    "One intentionally aborted POST response after synthetic persistence per width/theme",
    "HTTP 409 for stale two-page policy save",
    "HTTP 503 for permissions-unavailable GET",
  ],
};
const browser = await chromium.launch({ headless: true });
manifest.browser = { name: "chromium", version: browser.version() };
type BrowserDiagnostic = {
  kind: string;
  message: string;
  url?: string;
  method?: string;
  resourceType?: string;
  pageUrl?: string;
};
const diagnostics = new WeakMap<Page, BrowserDiagnostic[]>();
const documents = new WeakMap<Page, number>();
const errorCounts = new WeakMap<Page, number>();
function observe(page: Page) {
  const errors: BrowserDiagnostic[] = [];
  diagnostics.set(page, errors);
  const add = (e: typeof errors[number]) => {
    errorCounts.set(page, (errorCounts.get(page) ?? 0) + 1);
    if (errors.length < 100) errors.push(e);
  };
  page.on("pageerror", (e) => add({ kind: "page", message: e.stack || e.message || String(e) }));
  page.on("console", (e) => {
    if (e.type() === "error" || e.type() === "warning") add({ kind: "console", message: e.text() });
  });
  page.on(
    "requestfailed",
    (r) =>
      add({
        kind: "request",
        message: r.failure()?.errorText ?? "request failed",
        url: r.url(),
        method: r.method(),
        resourceType: r.resourceType(),
        pageUrl: page.url(),
      }),
  );
  page.on("response", (r) => {
    if (r.request().isNavigationRequest()) documents.set(page, r.status());
    if (r.status() >= 400) {
      add({
        kind: "http",
        message: `HTTP ${r.status()}`,
        url: r.url(),
        method: r.request().method(),
        resourceType: r.request().resourceType(),
        pageUrl: page.url(),
      });
    }
  });
}
async function capturePoint(page: Page, id: string, expectedErrors: string[] = []) {
  // 製品CSSやlayoutを変えず、確認対象の判断が見える位置へ移動する。
  if (id.includes("preview")) await page.locator(".diff").scrollIntoViewIfNeeded();
  else if (/saved|re-read|unknown|recovered|conflict/.test(id)) {
    await page.locator(".permissions > header").scrollIntoViewIfNeeded();
  }
  // overflow検査が失敗する場合も、先に画像を保存して根拠を残す。
  const image = join(output, `${id}.png`);
  const snapshot = join(output, `${id}.aria.txt`);
  await page.screenshot({ path: image });
  await Deno.writeTextFile(snapshot, await page.locator("body").ariaSnapshot());
  const mobileRemoveButtons = page.viewportSize()?.width === 390
    ? await page.locator(".lane button").evaluateAll((buttons) =>
      buttons.map((button) => {
        const style = button.ownerDocument.defaultView!.getComputedStyle(button);
        const bounds = button.getBoundingClientRect();
        const lane = button.closest(".lane")!.getBoundingClientRect();
        const range = button.ownerDocument.createRange();
        range.selectNodeContents(button);
        const textBounds = range.getBoundingClientRect();
        return {
          label: button.getAttribute("aria-label"),
          text: button.textContent?.trim(),
          whiteSpace: style.whiteSpace,
          flexShrink: style.flexShrink,
          textLineBoxes: Array.from(range.getClientRects()).filter((rect) => rect.width > 0).length,
          textFitsButton: textBounds.left >=
              bounds.left + parseFloat(style.borderLeftWidth) + parseFloat(style.paddingLeft) - 1 &&
            textBounds.right <=
              bounds.right - parseFloat(style.borderRightWidth) - parseFloat(style.paddingRight) +
                1 &&
            textBounds.top >= bounds.top && textBounds.bottom <= bounds.bottom,
          buttonFitsLane: bounds.left >= lane.left - 1 && bounds.right <= lane.right + 1,
          noInternalClipping: button.scrollWidth <= button.clientWidth &&
            button.scrollHeight <= button.clientHeight,
          width: bounds.width,
          textWidth: textBounds.width,
        };
      })
    )
    : [];
  manifest.captures.push({
    id,
    route: new URL(page.url()).pathname,
    viewport: page.viewportSize(),
    theme: await page.evaluate(() =>
      (globalThis as any).matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
    ),
    documentStatus: documents.get(page) ?? null,
    visibleAlerts: await page.getByRole("alert").allTextContents(),
    permissionState: await page.locator("[data-permission-state]").getAttribute(
      "data-permission-state",
    ),
    image,
    imageSha256: await sha256File(image),
    accessibilitySnapshot: snapshot,
    expectedErrors,
    // 再読込みで復旧の証拠を消さないよう、pageごとにerrorを累積する。
    errors: [...diagnostics.get(page) ?? []],
    errorSummary: {
      observed: errorCounts.get(page) ?? 0,
      retained: diagnostics.get(page)?.length ?? 0,
      truncated: (errorCounts.get(page) ?? 0) > 100,
      limit: 100,
    },
    mobileRemoveButtons,
    reviewed: false,
  });
  await Deno.writeTextFile(join(output, "review-context.json"), JSON.stringify(manifest, null, 2));
  for (const button of mobileRemoveButtons) {
    assertEquals(button.text, "Remove", `${id}: expected Remove control`);
    assertEquals(button.whiteSpace, "nowrap", `${id}: ${button.label} can wrap`);
    assertEquals(button.flexShrink, "0", `${id}: ${button.label} can shrink`);
    assertEquals(button.textLineBoxes, 1, `${id}: ${button.label} text breaks across lines`);
    assert(
      button.textFitsButton && button.buttonFitsLane && button.noInternalClipping,
      `${id}: ${button.label} clips or exceeds its lane`,
    );
  }
  await assertNoPageWideOverflow(page);
}
async function open(page: Page, base: string, id: string, state = "ready") {
  await page.goto(`${base}/w/memory-review/memory/${id}/permissions`);
  await page.locator(`[data-permission-state=${state}][aria-busy=false]`).waitFor();
}
async function add(
  page: Page,
  lane: "execution" | "delegation",
  operation: string,
  target: string,
  resourceCaptureId?: string,
) {
  await page.getByRole("combobox", { name: "Permission scope", exact: true }).selectOption(lane);
  await page.getByRole("combobox", { name: "Operation", exact: true }).selectOption(operation);
  // 内部JSON入力ではなく、実UIの名称・説明・資源入力を操作する。
  await page.getByText(
    operation === "ticket.read"
      ? "Read the selected ticket. Does not allow editing or closing it."
      : "Append a comment to the selected ticket; no completion or assignment authority.",
    { exact: true },
  ).waitFor();
  await page.getByLabel("Ticket identifier", { exact: true }).fill(target);
  if (resourceCaptureId) {
    await page.getByLabel("Ticket identifier", { exact: true }).focus();
    await page.keyboard.press("Tab");
    assertEquals(
      await page.getByRole("button", { name: "Add to unsaved changes", exact: true }).evaluate((
        element,
      ) => element === element.ownerDocument.activeElement),
      true,
    );
    await capturePoint(page, resourceCaptureId);
  }
  await page.getByRole("button", { name: "Add to unsaved changes", exact: true }).click();
  await page.locator("[data-permission-state=unsaved]").waitFor();
}
async function save(page: Page, state = "saved") {
  await page.getByLabel("I reviewed the operation, resource and delegation differences.").check();
  await page.getByRole("button", { name: "Save permissions", exact: true }).click();
  await page.locator(`[data-permission-state=${state}][aria-busy=false]`).waitFor();
}
try {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const prefix = `${theme}-${width}`;
      const port = await freePort();
      const base = `http://127.0.0.1:${port}`;
      const owned = await startOwnedProcesses(
        [{
          id: `t-802-${prefix}-synthetic-http`,
          command: Deno.execPath(),
          cwd: join(root, "tools/web-ux"),
          args: [
            "run",
            "-A",
            join(root, "tools/web-ux/browser-tests/memory_document_fixture_server.ts"),
            String(port),
            buildRoot,
            "3",
            "permissions",
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
      try {
        const page = await context.newPage();
        observe(page);
        // 通常のSubject詳細から管理導線へ進む。
        await page.goto(`${base}/w/memory-review/memory/permissions-empty`);
        await page.locator(".subject-overview").waitFor();
        await page.locator('main a[href$="/permissions"]').click();
        await page.locator("[data-permission-state=ready]").waitFor();
        await page.getByText("Not configured · no permissions", { exact: true }).waitFor();
        await capturePoint(page, `${prefix}-empty`);
        await add(
          page,
          "execution",
          "ticket.read",
          "T-802",
          `${prefix}-operation-description-resource-input`,
        );
        await add(page, "delegation", "ticket.comment", "T-802");
        // 名前付きlaneで検査し、一つのgrantが両方へ黙って追加されないことを確認する。
        assertEquals(
          await page.locator('.lane[aria-label="Direct operations"]').getByText("Read ticket", {
            exact: true,
          }).count(),
          1,
        );
        assertEquals(
          await page.locator('.lane[aria-label="Direct operations"]').getByText(
            "Comment on ticket",
            { exact: true },
          ).count(),
          0,
        );
        assertEquals(
          await page.locator('.lane[aria-label="Background Worker delegation ceiling"]').getByText(
            "Comment on ticket",
            { exact: true },
          ).count(),
          1,
        );
        await capturePoint(page, `${prefix}-preview-separate-lanes`);
        await save(page);
        await capturePoint(page, `${prefix}-saved`);
        await page.reload();
        await page.locator("[data-permission-state=ready]").waitFor();
        await capturePoint(page, `${prefix}-re-read`);

        // 合成HTTP fixtureがPOSTを受け付けた後、実browserで応答を消失させる。
        await add(page, "execution", "ticket.comment", "T-803");
        let lost = false;
        await page.route("**/subjektiv/subjects/permissions-empty/permissions", async (route) => {
          if (route.request().method() === "POST" && !lost) {
            const accepted = await route.fetch();
            assertEquals(accepted.status(), 200);
            lost = true;
            await route.abort("failed");
          } else await route.continue();
        });
        await save(page, "unknown");
        assert(lost);
        // 障害注入は一度だけとし、撮影前に通常のbrowser通信へ戻す。
        await page.unroute("**/subjektiv/subjects/permissions-empty/permissions");
        await capturePoint(page, `${prefix}-unknown`, [
          "Intentional POST response loss after synthetic save",
        ]);
        await page.reload();
        await page.locator("[data-permission-state=unknown][aria-busy=false]").waitFor();
        await page.getByRole("button", { name: "Reconcile the same save", exact: true }).click();
        await page.locator("[data-permission-state=saved]").waitFor();
        const evidence = await (await fetch(`${base}/fixture/permissions/evidence`)).json();
        const saves = evidence.requests.filter((r: { subject_id: string }) =>
          r.subject_id === "permissions-empty"
        );
        assertEquals(saves.length, 3);
        assertEquals(saves[1].request, saves[2].request);
        assertEquals(saves[2].outcome, "replay");
        await capturePoint(page, `${prefix}-recovered-same-operation`, [
          "Intentional earlier POST response loss",
        ]);

        // 二つのbrowser画面で同じ合成change identityを観測してから保存する。
        const other = await context.newPage();
        observe(other);
        await open(page, base, "permissions-edit");
        await open(other, base, "permissions-edit");
        await add(page, "delegation", "ticket.comment", "T-802");
        await other.getByRole("button", {
          name: "Remove Read ticket from direct operations",
          exact: true,
        }).click();
        await save(page);
        await save(other, "error");
        await other.getByRole("alert").filter({ hasText: "Conflict:" }).waitFor();
        await capturePoint(other, `${prefix}-two-page-conflict`, [
          "Intentional stale policy POST returns HTTP 409",
        ]);
        await other.close();
        await page.getByRole("combobox", { name: "Operation", exact: true }).selectOption(
          "worker.messaging",
        );
        await page.getByText("Target unavailable:", { exact: false }).waitFor();
        assertEquals(
          await page.getByRole("button", { name: "Add to unsaved changes", exact: true }).count(),
          0,
        );
        await capturePoint(page, `${prefix}-unavailable-operation`);
        await open(page, base, "permissions-reader");
        assertEquals(await page.locator("form.permission-form").count(), 0);
        assertEquals(
          await page.getByRole("button", { name: "Save permissions", exact: true }).count(),
          0,
        );
        await capturePoint(page, `${prefix}-read-only`);
        await open(page, base, "permissions-unavailable", "error");
        await capturePoint(page, `${prefix}-service-unavailable`, [
          "Intentional permission GET returns HTTP 503",
        ]);
        await open(page, base, "permissions-legacy");
        await page.getByText(
          "Preserved, not editable here. No conversion or expansion is implied.",
          { exact: true },
        ).waitFor();
        await add(page, "execution", "ticket.comment", "T-802");
        await save(page);
        const legacy = await (await fetch(
          `${base}/api/w/memory-review/subjektiv/subjects/permissions-legacy/permissions`,
        )).json();
        assertEquals(legacy.permissions.delegation, [{
          operation: "worker.notify",
          resource: { kind: "worker", runtime_id: "legacy-runtime", worker_id: "legacy-recipient" },
        }]);
        await capturePoint(page, `${prefix}-legacy-preserved`, [
          "Intentional earlier service unavailable GET",
        ]);
        // 直接操作の失効を確認する。ここで証明するのはUIの入力であり、実取消ではない。
        await page.getByRole("button", {
          name: "Remove Read ticket from direct operations",
          exact: true,
        }).click();
        await save(page);
        await capturePoint(page, `${prefix}-revoked-saved`);
        const finalAudit = await (await fetch(`${base}/fixture/permissions/evidence`)).json();
        const finalLegacy = await (await fetch(
          `${base}/api/w/memory-review/subjektiv/subjects/permissions-legacy/permissions`,
        )).json();
        assertEquals(finalAudit.requests.length, 7);
        assertEquals(
          finalAudit.requests.filter((r: { outcome: string }) => r.outcome === "conflict").length,
          1,
        );
        assertEquals(finalLegacy.permissions.execution, [{
          operation: "ticket.comment",
          resource: { kind: "ticket", workspace_id: "memory-review", ticket_id: "T-802" },
        }]);
        assertEquals(finalLegacy.permissions.delegation, legacy.permissions.delegation);
        await Deno.writeTextFile(
          join(output, `${prefix}-synthetic-http-evidence.json`),
          JSON.stringify(
            {
              authority: "synthetic-http-not-real-backend",
              replayAssertions: {
                originalAndRetryInputsEqual: true,
                retryOutcome: saves[2].outcome,
              },
              legacyDelegationPreserved: legacy.permissions.delegation,
              requests: finalAudit.requests,
              currentLegacyPolicyAfterShrink: finalLegacy,
            },
            null,
            2,
          ),
        );
      } catch (e) {
        manifest.failures.push(
          `${prefix}: ${e instanceof Error ? e.stack ?? e.message : String(e)}`,
        );
        for (const [index, failedPage] of context.pages().entries()) {
          const image = join(output, `${prefix}-failure-${index}.png`);
          const snapshot = join(output, `${prefix}-failure-${index}.aria.txt`);
          await failedPage.screenshot({ path: image, timeout: 5_000 }).catch(() => {});
          await Deno.writeTextFile(snapshot, await failedPage.locator("body").ariaSnapshot());
          manifest.captures.push({
            id: `${prefix}-failure-${index}`,
            route: failedPage.url(),
            documentStatus: documents.get(failedPage) ?? null,
            image,
            accessibilitySnapshot: snapshot,
            errors: diagnostics.get(failedPage) ?? [],
            reviewed: false,
          });
        }
        throw e;
      } finally {
        await context.close();
        await stopOwnedProcesses(owned);
      }
    }
  }
  assertEquals(
    await sha256File(join(buildRoot, "index.html")),
    manifest.buildIndexSha256,
    "Web build changed during the browser matrix",
  );
  manifest.status = "browser-assertions-passed-visual-unreviewed";
} catch (e) {
  manifest.status = "failed";
  throw e;
} finally {
  await browser.close();
  await Deno.writeTextFile(join(output, "review-context.json"), JSON.stringify(manifest, null, 2));
  console.log(join(output, "review-context.json"));
}
