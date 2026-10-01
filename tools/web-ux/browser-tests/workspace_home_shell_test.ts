/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Page } from "playwright";

const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const ready = async (page: Page) => {
  for (const id of ["home-attention", "home-active", "home-recent"]) {
    await page.locator(`[data-feed-ready="${id}"]`).waitFor();
  }
};
Deno.test("production Home dashboard isolates failures, shows authoritative records, and supports responsive keyboard navigation", async () => {
  const build = await new Deno.Command(Deno.execPath(), {
    args: ["task", "build"],
    cwd: join(root, "web/workspace"),
    stdout: "piped",
    stderr: "piped",
  }).output();
  assert(build.success, new TextDecoder().decode(build.stderr));
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const url = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      join(root, "tools/web-ux/browser-tests/workspace_home_fixture_server.ts"),
      String(port),
      join(root, "web/workspace/build"),
    ],
    stdout: "null",
    stderr: "null",
  }).spawn();
  try {
    let started = false;
    for (let n = 0; n < 100; n++) {
      try {
        const response = await fetch(`${url}/health`);
        await response.body?.cancel();
        if (response.ok) {
          started = true;
          break;
        }
      } catch { /* fixture is starting */ }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert(started);
    const browser = await chromium.launch({ headless: true });
    try {
      for (const colorScheme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390, 320]) {
          const context = await browser.newContext({
            viewport: { width, height: 900 },
            colorScheme,
            reducedMotion: "reduce",
          });
          try {
            const page = await context.newPage();
            const errors: string[] = [];
            const requests: string[] = [];
            page.on("pageerror", (error) => errors.push(String(error)));
            page.on("console", (message) => {
              if (message.type() === "error") errors.push(message.text());
            });
            page.on("requestfailed", (request) => errors.push(request.url()));
            page.on("request", (request) => requests.push(request.url()));
            for (const workspace of ["home-owner", "home-member", "home-long"]) {
              await page.goto(`${url}/w/${workspace}`);
              await ready(page);
              assertEquals(await page.locator("main h1").allTextContents(), ["Activity"]);
              assertEquals(await page.locator("main nav").count(), 0);
              assertEquals(await page.locator("main [role=alert]").count(), 0);
              assertEquals(await page.locator("[data-feed-ready=home-attention] a").count(), 3);
              assertEquals(await page.locator("[data-feed-ready=home-active] a").count(), 2);
              assertEquals(await page.locator("[data-feed-ready=home-recent] a").count(), 4);
              await page.getByText("Blocked by T-99", { exact: true }).waitFor();
              await page.getByText("W-7", { exact: true }).waitFor();
              const geometry = await page.locator(".workspace-home").evaluate((home) => {
                const main = home.closest("main")!;
                const sections = [...home.querySelectorAll("section")].map((section) =>
                  section.getBoundingClientRect()
                );
                return {
                  pageOverflow: document.documentElement.scrollWidth > innerWidth,
                  mainOverflow: main.scrollWidth > main.clientWidth,
                  firstLinkTop: home.querySelector("a")!.getBoundingClientRect().top,
                  sideBySide: sections[1].left > sections[0].right &&
                    sections[2].left > sections[1].right,
                  stacked: sections[1].top >= sections[0].bottom &&
                    sections[2].top >= sections[1].bottom,
                };
              });
              assertEquals(geometry.pageOverflow, false);
              assertEquals(geometry.mainOverflow, false);
              assert(geometry.firstLinkTop < 900);
              assert(width === 1440 ? geometry.sideBySide : geometry.stacked);
              const links = page.locator("main a");
              await links.first().focus();
              assertEquals(
                await links.first().evaluate((link) => getComputedStyle(link).outlineStyle),
                "solid",
              );
              await page.keyboard.press("Tab");
              assertEquals(
                await links.nth(1).evaluate((link) => document.activeElement === link),
                true,
              );
              for (const link of await links.all()) {
                assert(
                  new RegExp(`/w/${workspace}/(tickets|objectives|merge-requests)/[^/]+$`).test(
                    (await link.getAttribute("href"))!,
                  ),
                );
              }
            }
            const beforeRefresh = requests.filter((request) =>
              request.includes("/merge-requests?")
            ).length;
            await page.getByRole("button", { name: "Refresh", exact: true }).click();
            await page.getByRole("button", { name: "Refresh", exact: true }).waitFor();
            await ready(page);
            assertEquals(
              requests.filter((request) => request.includes("/merge-requests?")).length,
              beforeRefresh + 1,
            );
            const objective = page.locator("main a[href*='/objectives/O-12-']");
            await objective.focus();
            await page.keyboard.press("Enter");
            await page.waitForURL("**/objectives/O-12-*");
            await page.getByText("Home dashboard navigation destination.", { exact: true })
              .waitFor();
            assertEquals(
              requests.some((request) => new URL(request).pathname.endsWith("/hosts")),
              false,
            );
            assertEquals(errors, []);
          } finally {
            await context.close();
          }
        }
      }
      const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
      try {
        await page.goto(`${url}/w/home-empty`);
        await ready(page);
        assertEquals(await page.locator("main a").count(), 0);
        await page.getByText("No open Merge Requests.", { exact: true }).waitFor();
        await page.goto(`${url}/w/home-error`);
        await ready(page);
        assertEquals(
          await page.locator("[data-feed-ready=home-attention] [role=alert]").count(),
          1,
        );
        assertEquals(await page.getByText("No open Merge Requests.", { exact: true }).count(), 0);
        assertEquals(await page.locator("[data-feed-ready=home-active] a").count(), 2);
        assertEquals(await page.locator("[data-feed-ready=home-recent] a").count(), 4);
        let release!: () => void;
        const held = new Promise<void>((resolve) => {
          release = resolve;
        });
        await page.route("**/merge-requests?*", async (route) => {
          await held;
          await route.continue();
        });
        await page.goto(`${url}/w/home-owner`);
        await page.locator("[data-feed-ready=home-active]").waitFor();
        assertEquals(await page.getByText("Loading needs attention…", { exact: true }).count(), 1);
        release();
        await ready(page);
      } finally {
        await page.close();
      }
    } finally {
      await browser.close();
    }
  } finally {
    try {
      server.kill("SIGTERM");
    } catch (error) {
      if (!(error instanceof Deno.errors.NotFound)) throw error;
    }
    await server.status;
  }
});
