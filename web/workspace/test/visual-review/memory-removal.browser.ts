/** 実production buildと隔離fixture HTTPで、旧Memory撤去とSubject導線を確認する。Backend認可の証拠ではない。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium } from "playwright";
import { sha256File } from "../../../../tools/web-ux/src/artifacts.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
import { freePort } from "../../../../tools/web-ux/browser-tests/memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../../..");
const phase = Deno.args[0];
if (phase !== "before" && phase !== "after") {
  throw new Error("expected before|after");
}
const output = join(root, "web/workspace/.svelte-kit/memory-removal", phase);
await Deno.mkdir(output, { recursive: true });
const port = await freePort(), base = `http://127.0.0.1:${port}`;
const ws = "/w/memory-review", subjects = `${ws}/subjektiv`;
const captures: unknown[] = [], faults: unknown[] = [];
const manifest = {
  phase,
  evidence: "production-static-build / synthetic HTTP / no live services",
  source: new TextDecoder().decode(
    (await new Deno.Command("git", {
      args: ["diff", "--", "web", "crates/server-api", "crates/client"],
      cwd: root,
    }).output()).stdout,
  ),
  buildIndexSha256: await sha256File(
    join(root, "web/workspace/build/index.html"),
  ),
  captures,
  faults,
  status: "running",
  visualVerdict: "UNREVIEWED",
};
const save = () =>
  Deno.writeTextFile(
    join(output, "review-context.json"),
    JSON.stringify(manifest, null, 2),
  );
const owned = await startOwnedProcesses(
  [{
    id: "memory-removal-fixture",
    command: Deno.execPath(),
    cwd: root,
    args: [
      "run",
      "--config",
      "tools/web-ux/deno.json",
      "-A",
      "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
      String(port),
      join(root, "web/workspace/build"),
      "3",
      "navigation",
    ],
    readyUrl: `${base}/health`,
  }],
  join(output, "scenario.json"),
  join(output, "logs"),
  [],
);
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
try {
  browser = await chromium.launch({
    headless: true,
    executablePath: Deno.env.get("WEB_UX_CHROMIUM_EXECUTABLE"),
  });
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const context = await browser.newContext({
        colorScheme,
        viewport: { width, height: 900 },
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(10000);
      page.on(
        "pageerror",
        (e) => faults.push({ kind: "pageerror", message: e.message }),
      );
      page.on("console", (m) => {
        if (m.type() === "error") {
          faults.push({ kind: "console", message: m.text() });
        }
      });
      page.on("requestfailed", (r) => {
        if (r.failure()?.errorText !== "net::ERR_ABORTED") {
          faults.push({
            kind: "request",
            path: new URL(r.url()).pathname,
            error: r.failure()?.errorText,
          });
        }
      });
      const response = await page.goto(base + subjects);
      assertEquals(response?.status(), 200);
      await page.locator('[data-memory-view="subjects"] .subject-row').first()
        .waitFor();
      if (width === 390) {
        await page.getByRole("button", { name: "Show sidebar", exact: true })
          .click();
      }
      const nav = page.getByRole("navigation", {
        name: "Workspace sections",
        exact: true,
      });
      assertEquals(
        await nav.getByRole("link", { name: "Subjects", exact: true })
          .getAttribute("aria-current"),
        "page",
      );
      assertEquals(
        await nav.getByRole("link", { name: "Staging", exact: true }).count(),
        phase === "before" ? 1 : 0,
      );
      assertEquals(
        await page.evaluate(() =>
          document.documentElement.scrollWidth > innerWidth
        ),
        false,
      );
      const image = `${colorScheme}-${width}-navigation.png`;
      await page.screenshot({ path: join(output, image) });
      captures.push({
        image,
        sha256: await sha256File(join(output, image)),
        route: page.url(),
        documentStatus: response?.status(),
        colorScheme,
        width,
        mainWidth: await page.locator("main").evaluate((el) => el.clientWidth),
        alerts: await page.getByRole("alert").allTextContents(),
        snapshot: await page.locator("body").ariaSnapshot(),
      });
      if (phase === "after") {
        if (width === 390) {
          await page.getByRole("button", { name: "Hide sidebar", exact: true })
            .click();
        }
        await page.goto(
          base +
            `${ws}/memory/empty-subject/chat?from=bookmark#conversation-subject-heading`,
        );
        await page.waitForURL(
          base +
            `${subjects}/empty-subject/chat?from=bookmark#conversation-subject-heading`,
        );
        await page.locator('.conversation-page [role="textbox"]').waitFor();
        await page.goto(base + `${ws}/memory/staging`);
        await page.getByText("Error 404", { exact: true }).waitFor();
        assertEquals(new URL(page.url()).pathname, `${ws}/memory/staging`);
        assertEquals(
          await page.locator('[data-memory-view="staging"]').count(),
          0,
        );
        assertEquals(
          await page.locator('a[href$="/memory/staging"]').count(),
          0,
        );
      }
      await context.close();
      await save();
    }
  }
  assertEquals(faults, []);
  manifest.status = "passed-automated-not-visual-verdict";
} finally {
  await browser?.close();
  await stopOwnedProcesses(owned);
  await save();
}
console.log(output);
