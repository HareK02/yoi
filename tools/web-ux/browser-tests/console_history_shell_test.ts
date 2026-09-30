import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium } from "playwright";

const repositoryRoot = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const workspaceRoot = join(repositoryRoot, "web/workspace");
const fixtureServer = join(
  repositoryRoot,
  "tools/web-ux/browser-tests/console_history_fixture_server.ts",
);
const workspaceId = "console-history-review";

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

async function historyRequestCount(baseUrl: string): Promise<number> {
  const response = await fetch(`${baseUrl}/fixture-state`);
  const state = await response.json();
  return state.history_requests;
}

Deno.test("production Console pages backward by real turns while preserving both scroll anchors", async () => {
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
      const context = await browser.newContext({
        viewport: { width: 1440, height: 600 },
        colorScheme: "dark",
      });
      const page = await context.newPage();
      const errors: string[] = [];
      const responses: string[] = [];
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      page.on("pageerror", (error) => errors.push(String(error)));
      page.on("response", (response) => {
        if (response.status() >= 400 || response.url().includes("/api/")) {
          responses.push(`${response.status()} ${response.url()}`);
        }
      });

      await page.goto(
        `${baseUrl}/w/${workspaceId}/workers/W-900-console-fixture/console`,
      );
      const transcript = page.getByRole("article", { name: "main transcript" });
      try {
        await transcript.getByText("question 8", { exact: true }).waitFor({ timeout: 5_000 });
      } catch (error) {
        throw new Error(
          `Console did not render retained history: ${error}\nresponses=${
            JSON.stringify(responses)
          }\nerrors=${JSON.stringify(errors)}\nbody=${await page.locator("body").innerText()}`,
        );
      }
      const navigation = page.getByRole("navigation", { name: "Conversation turns" });
      await navigation.getByRole("button", { name: /^Turn \d+: question 7$/ }).waitFor();
      assertEquals(await historyRequestCount(baseUrl), 1);
      assertEquals(await transcript.getByText(/^question (7|8|9|10|11|12)$/).count(), 6);
      assertEquals(await navigation.getByRole("button", { name: /^Turn \d+: question (7|8|9|10|11|12)$/ }).count(), 6);
      const currentQuestionTop = await transcript
        .getByText("question 7", { exact: true })
        .evaluate((element) => element.getBoundingClientRect().top);
      const retainedQuestionTop = await transcript
        .getByText("question 8", { exact: true })
        .evaluate((element) => element.getBoundingClientRect().top);
      assert(
        currentQuestionTop < retainedQuestionTop,
        "the unmatched current-snapshot turn must precede overlapping retained history",
      );

      const question12 = navigation.getByRole("button", {
        name: /^Turn \d+: question 12$/,
      });
      await question12.hover();
      const preview = navigation.getByRole("tooltip");
      await preview.waitFor();
      assert((await preview.textContent())?.includes("detail 12.1"));
      assert((await preview.textContent())?.includes("detail 12.2"));
      assert(!(await preview.textContent())?.includes("detail 12.3"));
      await page.keyboard.press("Escape");

      const consoleScroll = transcript.locator("xpath=..");
      const topAlignment = await page.evaluate(() => {
        const document = (globalThis as any).document;
        const body = document.querySelector(".console-scroll")?.getBoundingClientRect();
        const turns = document.querySelector(".turn-navigation")?.getBoundingClientRect();
        return body && turns ? Math.abs(body.top - turns.top) : Number.POSITIVE_INFINITY;
      });
      assert(topAlignment <= 1, `transcript and turn bar top edges differ by ${topAlignment}px`);

      const navigationAnchoredTop = await navigation
        .locator('[data-turn-id="user-8"]')
        .evaluate((element) => element.getBoundingClientRect().top);
      const anchoredTop = await consoleScroll.evaluate((element) => {
        element.scrollTop = 0;
        const anchor = element.querySelector('[data-console-line-id*="user-8"]');
        const top = anchor?.getBoundingClientRect().top ?? Number.NaN;
        element.dispatchEvent(new Event("scroll"));
        return top;
      });
      await transcript.getByText("question 3", { exact: true }).waitFor();
      await navigation.getByRole("button", { name: /^Turn \d+: question 3$/ }).waitFor();
      const restoredTop = await transcript
        .locator('[data-console-line-id*="user-8"]')
        .evaluate((element) => element.getBoundingClientRect().top);
      assert(
        Math.abs(restoredTop - anchoredTop) <= 1,
        `transcript anchor moved from ${anchoredTop}px to ${restoredTop}px`,
      );
      await page.waitForTimeout(250);
      assertEquals(
        await historyRequestCount(baseUrl),
        2,
        "a stationary top sentinel must not drain another page",
      );

      const turnList = navigation.locator(".turn-list");
      const navigationAfterPrepend = await navigation
        .locator('[data-turn-id="user-8"]')
        .evaluate((element) => element.getBoundingClientRect().top);
      assert(
        Math.abs(navigationAfterPrepend - navigationAnchoredTop) <= 1,
        `turn-bar anchor moved from ${navigationAnchoredTop}px to ${navigationAfterPrepend}px`,
      );
      await turnList.evaluate((element) => {
        element.scrollTop = Math.max(2, element.scrollTop);
        element.dispatchEvent(new Event("scroll"));
        element.scrollTop = 0;
        element.dispatchEvent(new Event("scroll"));
      });
      await transcript.getByText("question 1", { exact: true }).waitFor();
      await navigation.getByRole("button", { name: /^Turn \d+: question 1$/ }).waitFor();
      assertEquals(await historyRequestCount(baseUrl), 3);
      assertEquals(await page.getByText("Start of conversation", { exact: true }).count(), 1);

      await navigation.getByRole("button", {
        name: /^Turn \d+: question 3$/,
      }).click();
      await page.waitForFunction(() => {
        const document = (globalThis as any).document;
        const element = document.querySelector('[data-console-line-id*="user-3"]');
        const parent = element?.closest(".console-scroll");
        if (!element || !parent) return false;
        const item = element.getBoundingClientRect();
        const viewport = parent.getBoundingClientRect();
        return item.top >= viewport.top - 1 && item.top < viewport.bottom;
      });
      const jumpGeometry = await transcript
        .locator('[data-console-line-id*="user-3"]')
        .evaluate((element) => {
          const parent = element.closest(".console-scroll")!;
          const item = element.getBoundingClientRect();
          const viewport = parent.getBoundingClientRect();
          return { itemTop: item.top, viewportTop: viewport.top, viewportBottom: viewport.bottom };
        });
      assert(jumpGeometry.itemTop >= jumpGeometry.viewportTop - 1);
      assert(jumpGeometry.itemTop < jumpGeometry.viewportBottom);

      const centering = await question12.evaluate((button) => {
        const turn = button.getBoundingClientRect();
        const bar = button.querySelector(".turn-bar")!.getBoundingClientRect();
        return Math.abs((turn.top + turn.bottom) / 2 - (bar.top + bar.bottom) / 2);
      });
      assert(centering <= 1, `turn marker is not vertically centered (${centering}px)`);
      assertEquals(errors, []);
      await context.close();
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
});
