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
const subjectId =
  "release-coordination-subject-with-a-deliberately-long-stable-identity-for-responsive-review";
const memoryId =
  "memory-decision-with-a-deliberately-long-stable-identity-for-overflow-review-0000000001";

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
    const marker = [...document.querySelectorAll("h3")].find((heading) =>
      heading.textContent === "T-672 UNIQUE END MARKER"
    );
    if (!main || !marker) return false;
    const mainRect = main.getBoundingClientRect();
    const markerRect = marker.getBoundingClientRect();
    return markerRect.top >= mainRect.top && markerRect.bottom <= mainRect.bottom;
  });
}

async function assertNoPageWideOverflow(page: Page): Promise<void> {
  assertEquals(
    await page.evaluate(() => {
      const document = (globalThis as any).document;
      return document.documentElement.scrollWidth === document.documentElement.clientWidth;
    }),
    true,
  );
}

async function assertMainOwnsVerticalScroll(page: Page): Promise<void> {
  const nested = await page.locator("main").evaluate((main) =>
    [...main.querySelectorAll("*")]
      .filter((element) => {
        const style = (globalThis as any).getComputedStyle(element);
        return ["auto", "scroll"].includes(style.overflowY) &&
          element.scrollHeight > element.clientHeight + 1;
      })
      .map((element) => `${element.tagName.toLowerCase()}.${element.className}`)
  );
  assertEquals(nested, []);
}

async function openSubjectPage(
  page: Page,
  baseUrl: string,
  mobile = false,
): Promise<void> {
  await page.goto(`${baseUrl}/w/${workspaceId}/memory/${encodeURIComponent(subjectId)}`);
  const heading = page.locator("h1", { hasText: "Release coordination" });
  await heading.waitFor({ state: "attached" });
  if (mobile) {
    const showSidebar = page.getByRole("button", { name: "Show sidebar" });
    await showSidebar.waitFor();
    assertEquals(await showSidebar.getAttribute("aria-expanded"), "false");
    assertEquals(
      await page.locator("main").evaluate((main) => (main as unknown as { inert: boolean }).inert),
      false,
    );

    await showSidebar.click();
    const hideSidebar = page.getByRole("button", { name: "Hide sidebar" });
    await hideSidebar.waitFor();
    assertEquals(await hideSidebar.getAttribute("aria-expanded"), "true");
    assertEquals(
      await page.locator("main").evaluate((main) => (main as unknown as { inert: boolean }).inert),
      true,
    );

    await hideSidebar.click();
    await showSidebar.waitFor();
    assertEquals(await showSidebar.getAttribute("aria-expanded"), "false");
    assertEquals(
      await page.locator("main").evaluate((main) => (main as unknown as { inert: boolean }).inert),
      false,
    );
  }
  await heading.waitFor();
}

async function swipeMainToEnd(page: Page): Promise<void> {
  const session = await page.context().newCDPSession(page);
  const viewport = page.viewportSize();
  if (!viewport) throw new Error("touch viewport is unavailable");
  const x = Math.max(20, Math.min(300, viewport.width - 20));
  const startY = Math.max(120, viewport.height - 100);
  const moveYs = [0.8, 0.62, 0.44, 0.26, 0.12].map((ratio) =>
    Math.max(20, Math.round(viewport.height * ratio))
  );
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const state = await mainScrollState(page);
    if (state.scrollTop + state.clientHeight >= state.scrollHeight - 1) return;
    await session.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [{ x, y: startY, id: 1 }],
    });
    for (const y of moveYs) {
      await session.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: [{ x, y, id: 1 }],
      });
    }
    await session.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
    await page.waitForTimeout(20);
  }
  throw new Error("touch input did not reach the end of the resident Memory surface");
}

Deno.test("production subject Memory and Worker launch shells preserve exact scoped behavior", async () => {
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
          {
            label: "compact-mobile",
            width: 320,
            height: 700,
            colorScheme: "dark" as const,
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
        await openSubjectPage(page, baseUrl, scenario.mobile);

        const initial = await mainScrollState(page);
        assert(initial.scrollHeight > initial.clientHeight);
        assertEquals(await markerIsInsideMain(page), false);
        await assertNoPageWideOverflow(page);
        await assertMainOwnsVerticalScroll(page);
        assertEquals(
          await page.locator('aside a[href$="/memory"]', { hasText: "Subjects" }).count(),
          1,
        );
        assertEquals(await page.locator("aside a", { hasText: "Staging" }).count(), 0);

        if (scenario.label === "wheel") {
          assertEquals(
            await page.getByRole("heading", {
              name: "Release coordination",
              level: 1,
              exact: true,
            }).count(),
            1,
          );
          assertEquals(
            await page.locator("strong").filter({ hasText: "important context" }).count() > 0,
            true,
          );
          assertEquals(await page.locator("blockquote").count() > 0, true);
          assertEquals(await page.locator("ul li").count() > 0, true);
          for (
            const heading of [
              { selector: "h5", text: "Deep surface heading" },
              { selector: "h6", text: "Deepest surface heading" },
            ]
          ) {
            const element = page.locator(heading.selector, { hasText: heading.text });
            assertEquals(await element.count(), 1);
            assert(
              await element.evaluate((node) =>
                Number.parseFloat((globalThis as any).getComputedStyle(node).fontSize) >= 12
              ),
              `${heading.selector} fell below the design language minimum text size`,
            );
          }
          assertEquals(await page.locator('a[href="https://example.com"]').count(), 1);
          assertEquals(await page.locator('a[href^="javascript:"]').count(), 0);
          assertEquals(
            await page.getByRole("article", { name: "Resident surface" }).locator("img, script")
              .count(),
            0,
          );
          for (const state of ["Active", "Resolved", "Retracted"]) {
            assertEquals(
              await page.locator(".memory-pill", { hasText: state }).count(),
              1,
            );
          }

          for (const label of ["Markdown table", "rust code block"]) {
            const region = page.getByRole("region", { name: label });
            assertEquals(await region.getAttribute("tabindex"), "0");
            assert(await region.evaluate((element) => element.scrollWidth > element.clientWidth));
          }

          await page.locator("main").hover();
          await page.mouse.wheel(0, 100_000);
        } else if (scenario.label === "keyboard") {
          await page.getByText("Surface provenance", { exact: true }).focus();
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

      const detailContext = await browser.newContext({ viewport: { width: 1440, height: 900 } });
      const detailPage = await detailContext.newPage();
      await detailPage.goto(
        `${baseUrl}/w/${workspaceId}/memory/${encodeURIComponent(subjectId)}/${
          encodeURIComponent(memoryId)
        }`,
      );
      await detailPage.getByRole("heading", {
        name: "Keep subject Memory provenance typed and visible",
        level: 1,
      }).waitFor();
      assertEquals(await detailPage.getByText("candidate-release-decision-0001").count(), 1);
      assertEquals(await detailPage.getByText("T-672 product direction").count(), 1);
      assertEquals(await detailPage.getByText("memory-derived-source-0002").count(), 1);
      assertEquals(await detailPage.getByRole("heading", { name: "Revision history" }).count(), 1);
      assertEquals(await detailPage.getByText("Revision 3", { exact: true }).count(), 1);
      assertEquals(await detailPage.getByText("Revision 2", { exact: true }).count(), 1);
      assertEquals(await detailPage.getByText("Revision 1", { exact: true }).count(), 0);
      await detailPage.getByRole("navigation", { name: "Revision history pages" }).getByRole(
        "link",
        { name: "Next page →" },
      ).click();
      await detailPage.getByText("Revision 1", { exact: true }).waitFor();
      assert(new URL(detailPage.url()).searchParams.has("revision_cursor"));
      assertEquals(
        await detailPage.getByRole("navigation", { name: "Revision history pages" }).getByRole(
          "link",
          { name: "First page" },
        ).count(),
        1,
      );
      assertEquals(await detailPage.locator('a[href^="javascript:"]').count(), 0);
      assertEquals(
        await detailPage.getByRole("article", { name: "Committed Memory body" }).locator(
          "script, img",
        ).count(),
        0,
      );
      for (const label of ["Markdown table", "text code block"]) {
        const region = detailPage.getByRole("region", { name: label });
        assertEquals(await region.getAttribute("tabindex"), "0");
        assert(await region.evaluate((element) => element.scrollWidth > element.clientWidth));
      }
      await assertNoPageWideOverflow(detailPage);
      await assertMainOwnsVerticalScroll(detailPage);
      await detailContext.close();

      const paginationContext = await browser.newContext({ viewport: { width: 768, height: 900 } });
      const paginationPage = await paginationContext.newPage();
      await paginationPage.goto(`${baseUrl}/w/${workspaceId}/memory`);
      await paginationPage.getByRole("heading", { name: "Subjects", level: 1 }).waitFor();
      assertEquals(await paginationPage.getByText("Subject on the next page").count(), 0);
      await paginationPage.getByRole("navigation", { name: "Subject pages" }).getByRole(
        "link",
        { name: "Next page →" },
      ).click();
      await paginationPage.getByText("Subject on the next page").waitFor();
      assert(new URL(paginationPage.url()).searchParams.has("cursor"));
      assertEquals(
        await paginationPage.getByRole("navigation", { name: "Subject pages" }).getByRole(
          "link",
          { name: "First page" },
        ).count(),
        1,
      );

      await paginationPage.goto(
        `${baseUrl}/w/${workspaceId}/memory/${encodeURIComponent(subjectId)}`,
      );
      await paginationPage.getByRole("heading", { name: "Current Memories", level: 2 }).waitFor();
      await paginationPage.getByRole("navigation", { name: "Current Memory pages" }).getByRole(
        "link",
        { name: "Next page →" },
      ).click();
      await paginationPage.getByText("Memory on the next page").waitFor();
      assert(new URL(paginationPage.url()).searchParams.has("cursor"));
      await assertNoPageWideOverflow(paginationPage);
      await paginationContext.close();

      const stateContext = await browser.newContext({ viewport: { width: 390, height: 844 } });
      const stateCases = [
        {
          id: "empty-subject",
          text: "Resident surface is ready and empty.",
          role: "status" as const,
        },
        { id: "stale-subject", text: "Resident surface is stale.", role: "status" as const },
        {
          id: "failed-subject",
          text: "Resident surface generation failed.",
          role: "alert" as const,
        },
        {
          id: "ungenerated-subject",
          text: "Resident surface has not been generated.",
          role: "status" as const,
        },
        { id: "error-subject", text: "Resident surface unavailable.", role: "alert" as const },
      ];
      for (const stateCase of stateCases) {
        const page = await stateContext.newPage();
        await page.goto(`${baseUrl}/w/${workspaceId}/memory/${stateCase.id}`);
        const state = page.getByRole(stateCase.role).filter({ hasText: stateCase.text });
        await state.waitFor();
        assert((await state.textContent())?.includes(stateCase.text));
        if (stateCase.id !== "empty-subject") {
          assertEquals(await page.getByRole("article", { name: "Resident surface" }).count(), 0);
        }
        await assertNoPageWideOverflow(page);
        await page.close();
      }
      await stateContext.close();

      const launchContext = await browser.newContext({ viewport: { width: 768, height: 900 } });
      const launchPage = await launchContext.newPage();
      await launchPage.goto(`${baseUrl}/w/${workspaceId}/workers/new`);
      await launchPage.getByRole("heading", { name: "New Worker", level: 1 }).waitFor();
      const subjectConnection = launchPage.locator(
        '[data-worker-feature-connection="subjektiv"]',
      );
      await subjectConnection.waitFor();
      const subjectSelect = launchPage.getByRole("combobox", {
        name: "Subject Memory connection",
      });
      assertEquals(await subjectSelect.locator('option[value="error-subject"]').count(), 0);
      assertEquals(await subjectSelect.locator('option[value="paged-subject"]').count(), 0);
      await launchPage.getByRole("button", { name: "Load more subjects" }).click();
      await subjectSelect.locator('option[value="paged-subject"]').waitFor({ state: "attached" });
      await subjectSelect.selectOption(subjectId);
      assertEquals(await subjectSelect.inputValue(), subjectId);
      assertEquals(
        await subjectConnection.locator("code", { hasText: subjectId }).count(),
        1,
      );

      const profileSelect = launchPage.getByRole("combobox", { name: "Profile" });
      await profileSelect.selectOption("builtin:standalone");
      assertEquals(await subjectConnection.count(), 0);
      await profileSelect.selectOption("builtin:companion");
      await subjectConnection.waitFor();
      assertEquals(await subjectSelect.inputValue(), "");

      await subjectSelect.selectOption(subjectId);
      const createRequestPromise = launchPage.waitForRequest((request) =>
        request.method() === "POST" &&
        new URL(request.url()).pathname === `/api/w/${workspaceId}/workers`
      );
      await launchPage.getByRole("button", { name: "Start Worker" }).click();
      const createRequest = await createRequestPromise;
      const createPayload = createRequest.postDataJSON();
      assertEquals(createPayload.profile, "builtin:companion");
      assertEquals(createPayload.feature_connections, {
        subjektiv: { subject_id: subjectId },
      });
      await launchContext.close();
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
});
