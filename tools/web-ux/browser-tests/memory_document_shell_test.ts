import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Page } from "playwright";

const repositoryRoot = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const workspaceRoot = join(repositoryRoot, "web/workspace");
const fixtureServer = join(
  repositoryRoot,
  "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
);
const workspaceId = "memory-review";

async function freePort(): Promise<number> {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  return port;
}

async function waitForServer(url: string): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // Retry until the bounded deadline while the owned server starts.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`fixture server did not become ready: ${url}`);
}

async function mainScrollState(page: Page) {
  return await page.locator("main").evaluate((main) => ({
    scrollTop: main.scrollTop,
    scrollHeight: main.scrollHeight,
    clientHeight: main.clientHeight,
  }));
}

async function markerIsInsideMain(page: Page): Promise<boolean> {
  return await page.evaluate(() => {
    const document = (globalThis as any).document;
    const main = document.querySelector("main");
    const marker = [...document.querySelectorAll("h2")].find((heading) =>
      heading.textContent === "T-661 UNIQUE END MARKER"
    );
    if (!main || !marker) return false;
    const mainRect = main.getBoundingClientRect();
    const markerRect = marker.getBoundingClientRect();
    return markerRect.top >= mainRect.top && markerRect.bottom <= mainRect.bottom;
  });
}

async function openMemoryPage(
  page: Page,
  baseUrl: string,
  mobile = false,
): Promise<void> {
  await page.goto(`${baseUrl}/w/${workspaceId}/memory`);
  const heading = page.locator("h1", { hasText: "Workspace Memory" });
  await heading.waitFor({ state: "attached" });
  if (mobile) await page.getByRole("button", { name: "Hide sidebar" }).click();
  await heading.waitFor();
}

async function swipeMainToEnd(page: Page): Promise<void> {
  const session = await page.context().newCDPSession(page);
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const state = await mainScrollState(page);
    if (state.scrollTop + state.clientHeight >= state.scrollHeight - 1) return;
    await session.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [{ x: 300, y: 700, id: 1 }],
    });
    for (const y of [580, 460, 340, 220, 100]) {
      await session.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: [{ x: 300, y, id: 1 }],
      });
    }
    await session.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
    await page.waitForTimeout(20);
  }
  throw new Error("touch input did not reach the end of the Memory document");
}

Deno.test("production Memory shell renders safe Markdown and scrolls to its end by wheel, keyboard, and touch", async () => {
  const build = await new Deno.Command(Deno.execPath(), {
    args: ["task", "build"],
    cwd: workspaceRoot,
    stdout: "piped",
    stderr: "piped",
  }).output();
  if (!build.success) {
    throw new Error(
      `Workspace production build failed:\n${new TextDecoder().decode(build.stderr)}`,
    );
  }

  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      fixtureServer,
      String(port),
      join(workspaceRoot, "build"),
    ],
    stdout: "null",
    stderr: "null",
  }).spawn();

  try {
    await waitForServer(`${baseUrl}/health`);
    const browser = await chromium.launch({ headless: true });
    try {
      for (
        const scenario of [
          { label: "wheel", width: 1440, height: 900, colorScheme: "light" as const },
          { label: "keyboard", width: 768, height: 900, colorScheme: "dark" as const },
          {
            label: "touch",
            width: 390,
            height: 844,
            colorScheme: "light" as const,
            mobile: true,
          },
        ]
      ) {
        const context = await browser.newContext({
          viewport: { width: scenario.width, height: scenario.height },
          colorScheme: scenario.colorScheme,
          hasTouch: scenario.mobile ?? false,
          isMobile: scenario.mobile ?? false,
        });
        const page = await context.newPage();
        const errors: string[] = [];
        page.on("console", (message) => {
          if (message.type() === "error") errors.push(message.text());
        });
        page.on("pageerror", (error) => errors.push(String(error)));
        await openMemoryPage(page, baseUrl, scenario.mobile);

        const initial = await mainScrollState(page);
        assert(initial.scrollHeight > initial.clientHeight);
        assertEquals(await markerIsInsideMain(page), false);
        assertEquals(
          await page.evaluate(() => {
            const document = (globalThis as any).document;
            return document.documentElement.scrollWidth === document.documentElement.clientWidth;
          }),
          true,
        );

        if (scenario.label === "wheel") {
          assertEquals(await page.locator("h1").allTextContents(), ["Workspace Memory"]);
          assertEquals(
            await page.locator("strong").filter({ hasText: "important context" }).count() > 0,
            true,
          );
          assertEquals(await page.locator("blockquote").count() > 0, true);
          assertEquals(await page.locator("ul li").count() > 0, true);
          assertEquals(await page.locator('a[href="https://example.com"]').count(), 1);
          assertEquals(await page.locator('a[href^="javascript:"]').count(), 0);
          assertEquals(await page.locator("article img, article script").count(), 0);
          assertEquals(await page.getByRole("heading", { name: "Memory Document" }).count(), 0);

          for (const label of ["Markdown table", "rust code block"]) {
            const region = page.getByRole("region", { name: label });
            assertEquals(await region.getAttribute("tabindex"), "0");
            assert(await region.evaluate((element) => element.scrollWidth > element.clientWidth));
          }

          await page.locator("main").hover();
          await page.mouse.wheel(0, 100_000);
        } else if (scenario.label === "keyboard") {
          await page.getByText("Document details", { exact: true }).focus();
          await page.keyboard.press("End");
        } else {
          await swipeMainToEnd(page);
        }

        await page.waitForTimeout(500);
        const afterInteraction = await mainScrollState(page);
        assert(
          afterInteraction.scrollTop > 0,
          `${scenario.label} did not move the main scroll owner: ${
            JSON.stringify(afterInteraction)
          }`,
        );
        assertEquals(await markerIsInsideMain(page), true);
        assertEquals(errors, []);
        await context.close();
      }

      const stateContext = await browser.newContext({ viewport: { width: 1440, height: 900 } });
      const emptyPage = await stateContext.newPage();
      await emptyPage.route(`**/api/w/${workspaceId}/memory`, async (route) => {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({
            body_md: " \n",
            created_at: "2026-01-01T00:00:00Z",
            updated_at: "2026-01-02T03:04:05Z",
            bytes: 2,
            record_source: "fixture",
          }),
        });
      });
      await emptyPage.goto(`${baseUrl}/w/${workspaceId}/memory`);
      await emptyPage.getByText("Memory document is empty.").waitFor();
      assertEquals(await emptyPage.getByRole("alert").count(), 0);

      const errorPage = await stateContext.newPage();
      await errorPage.route(`**/api/w/${workspaceId}/memory`, async (route) => {
        await route.fulfill({ status: 503, contentType: "application/json", body: "{}" });
      });
      await errorPage.goto(`${baseUrl}/w/${workspaceId}/memory`);
      const alert = errorPage.getByRole("alert");
      await alert.waitFor();
      assert((await alert.textContent())?.includes("Memory document unavailable."));
      assertEquals(await errorPage.getByText("Memory document is empty.").count(), 0);
      await stateContext.close();
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
});
