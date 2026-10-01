/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium } from "playwright";

const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");

Deno.test("production Home preserves scoped navigation, permission boundaries, responsive layout, and keyboard access", async () => {
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
    let ready = false;
    for (let n = 0; n < 100; n++) {
      try {
        const response = await fetch(`${url}/health`);
        await response.body?.cancel();
        if (response.ok) {
          ready = true;
          break;
        }
      } catch { /* owned fixture is starting */ }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert(ready, "fixture did not start");
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
              const work = page.getByRole("navigation", { name: "Work", exact: true });
              await work.waitFor();
              assertEquals(
                (await work.getByRole("link").allTextContents()).map((text) => text.trim()),
                ["Tickets", "Objectives", "Merge Requests", "Memory", "Workers"],
              );
              const settings = page.locator("main").getByRole("navigation", {
                name: "Settings",
                exact: true,
              });
              assertEquals(
                await settings.getByRole("link").count(),
                workspace === "home-member" ? 2 : 6,
              );
              assertEquals(
                await settings.getByRole("link", { name: "Runtimes", exact: true }).count(),
                workspace === "home-member" ? 0 : 1,
              );
              assertEquals(await page.locator("main h1").allTextContents(), ["Work"]);
              const geometry = await page.locator(".workspace-home").evaluate((home) => {
                const main = home.closest("main")!;
                const work = home.querySelector("section")!.getBoundingClientRect();
                const settings = home.querySelector(".home-settings")!.getBoundingClientRect();
                return {
                  pageOverflow: document.documentElement.scrollWidth > innerWidth,
                  mainOverflow: main.scrollWidth > main.clientWidth,
                  firstLinkBottom: home.querySelector("a")!.getBoundingClientRect().bottom,
                  sideBySide: settings.left > work.right,
                  stacked: settings.top >= work.bottom,
                  smallTargets: [...home.querySelectorAll("a")].filter((link) =>
                    link.getBoundingClientRect().height < 44
                  ).length,
                };
              });
              assertEquals(geometry.pageOverflow, false);
              assertEquals(geometry.mainOverflow, false);
              assert(geometry.firstLinkBottom < 900);
              assertEquals(geometry.smallTargets, 0);
              assert(width === 1440 ? geometry.sideBySide : geometry.stacked);
              const first = work.getByRole("link", { name: "Tickets", exact: true });
              await first.focus();
              const outline = await first.evaluate((link) => getComputedStyle(link).outlineStyle);
              assertEquals(outline, "solid");
              await page.keyboard.press("Tab");
              assertEquals(
                await page.evaluate(() => document.activeElement?.textContent?.trim()),
                "Objectives",
              );
              for (const link of await page.locator("main a").all()) {
                assert((await link.getAttribute("href"))!.startsWith(`/w/${workspace}/`));
              }
            }
            const memory = page.getByRole("navigation", { name: "Work", exact: true }).getByRole(
              "link",
              { name: "Memory", exact: true },
            );
            await memory.focus();
            await page.keyboard.press("Enter");
            await page.waitForURL(`**/w/home-long/memory`);
            await page.getByRole("heading", { name: "Home navigation destination" }).waitFor();
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
