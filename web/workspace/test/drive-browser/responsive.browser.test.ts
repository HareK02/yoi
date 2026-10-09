/// <reference lib="dom" />
// Responsive DOM geometry audit over the frozen production shell and fake API.
// Read-only observation: no UI/CSS mutation or actual Backend connection.
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium } from "playwright";
const root = join(dirname(fromFileUrl(import.meta.url)), "../../../..");
const auditId = Deno.args[1] ?? "verified";
Deno.test("production Drive and Workers constrain page overflow and expose keyboard grant controls at every viewport", async () => {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const origin = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-read",
      "--allow-net",
      "--allow-run",
      "--allow-env",
      join(root, "tools/web-ux/drive-fixture/server.ts"),
      String(port),
      join(root, Deno.args[0] ?? "web/workspace/build"),
    ],
    stdout: "null",
    stderr: "inherit",
  }).spawn();
  const evidence: unknown[] = [];
  try {
    const deadline = Date.now() + 10000;
    while (true) {
      try {
        const response = await fetch(`${origin}/health`);
        await response.body?.cancel();
        if (response.ok) break;
      } catch { /* startup */ }
      if (Date.now() > deadline) throw new Error("fixture failed startup");
      await new Promise((resolve) => setTimeout(resolve, 30));
    }
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
            const pageErrors: string[] = [];
            page.on("pageerror", (e) => pageErrors.push(String(e)));
            for (
              const [workspace, surface] of [
                ["home-owner", "workers"],
                ["home-member", "workers"],
                ["home-empty", "workers"],
                ["workers-error", "workers"],
                ["home-long", "workers"],
                ["home-owner", "drive"],
                ["home-member", "drive"],
                ["home-empty", "drive"],
                ["home-error", "drive"],
                ["drive-paged", "drive"],
                ["home-owner", "drive/10"],
                ["home-owner", "drive/5"],
              ]
            ) {
              await page.goto(`${origin}/w/${workspace}/${surface}`);
              if (surface.startsWith("drive")) {
                await page.locator('[data-drive-ready="true"]').waitFor();
              } else if (workspace === "workers-error") {
                await page.locator(".workers-page .section-state.error")
                  .waitFor();
              } else if (workspace === "home-empty") {
                await page.getByText("No Workers are visible.", { exact: true })
                  .waitFor();
              } else {await page.locator(".workers-table tbody tr").first()
                  .waitFor();}
              if (surface === "drive/5") {
                await page.waitForFunction(() => {
                  const image = document.querySelector("main .drive-image");
                  return image instanceof HTMLImageElement && image.complete &&
                    image.naturalWidth === 1200 && image.naturalHeight === 800;
                });
              }
              if (workspace === "home-owner" && surface === "workers") {
                await page.getByRole("combobox", {
                  name: "Worker",
                  exact: true,
                }).selectOption({
                  label: "Documentation Worker · fixture-runtime / fixture-worker-a",
                });
                await page.getByRole("button", {
                  name: "Add Drive grant",
                  exact: true,
                }).click();
                await page.getByRole("combobox", {
                  name: "Access",
                  exact: true,
                }).waitFor();
                await page.getByRole("button", {
                  name: "Refresh grants",
                  exact: true,
                }).press("Tab");
                const focus = await page.evaluate(() => ({
                  tag: document.activeElement?.tagName,
                  outline: document.activeElement
                    ? getComputedStyle(document.activeElement).outlineStyle
                    : "none",
                }));
                assert(
                  focus.tag === "BUTTON" && focus.outline !== "none",
                  JSON.stringify(focus),
                );
              }
              if (workspace === "workers-error" && width === 320 && colorScheme === "dark") {
                await page.screenshot({
                  path: join(root, `web/workspace/.svelte-kit/t724/workers-error-${auditId}.png`),
                  fullPage: false,
                });
              }
              const geometry = await page.evaluate(() => {
                const main = document.querySelector("main")! as HTMLElement;
                const grants = main.querySelector(".drive-grants");
                const boundedTables = [
                  ...main.querySelectorAll(".table-wrap, .drive-table-region"),
                ].map((el) => ({
                  width: el.clientWidth,
                  scrollWidth: el.scrollWidth,
                  overflowX: getComputedStyle(el).overflowX,
                }));
                const raster = main.querySelector(".drive-image");
                const image = raster instanceof HTMLImageElement
                  ? {
                    naturalWidth: raster.naturalWidth,
                    naturalHeight: raster.naturalHeight,
                    renderedWidth: raster.getBoundingClientRect().width,
                    renderedHeight: raster.getBoundingClientRect().height,
                    availableWidth: raster.parentElement!.getBoundingClientRect().width,
                    heightLimit: innerHeight * 0.6,
                  }
                  : null;
                return {
                  image,
                  pageWidth: document.documentElement.scrollWidth,
                  viewportWidth: innerWidth,
                  mainWidth: main.clientWidth,
                  mainScrollWidth: main.scrollWidth,
                  documentScrollY: scrollY,
                  mainScrollTop: main.scrollTop,
                  mainTop: main.getBoundingClientRect().top,
                  firstHeadingTop: main.querySelector("h1")?.getBoundingClientRect().top ?? null,
                  grantsWidth: grants?.getBoundingClientRect().width ?? null,
                  boundedTables,
                };
              });
              evidence.push({
                colorScheme,
                width,
                workspace,
                surface,
                ...geometry,
              });
              assert(
                geometry.pageWidth <= width,
                JSON.stringify(evidence.at(-1)),
              );
              assert(
                geometry.mainScrollWidth <= geometry.mainWidth + 1,
                JSON.stringify(evidence.at(-1)),
              );
              if (surface === "drive/5") {
                const image = geometry.image;
                assert(image !== null);
                assertEquals([image.naturalWidth, image.naturalHeight], [1200, 800]);
                assert(image.renderedWidth > 100 && image.renderedHeight > 100);
                assert(image.renderedWidth <= image.availableWidth + 1);
                assert(image.renderedHeight <= image.heightLimit + 1);
                assert(Math.abs(image.renderedWidth / image.renderedHeight - 1.5) < 0.02);
              }
              if (workspace === "home-member" && surface === "workers") {
                assertEquals(
                  await page.getByRole("region", {
                    name: "Worker Drive grants",
                  }).count(),
                  0,
                );
              }
            }
            assertEquals(pageErrors, []);
          } finally {
            await context.close();
          }
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    await Deno.writeTextFile(
      join(root, `web/workspace/.svelte-kit/t724/responsive-${auditId}.json`),
      JSON.stringify(
        { authority: "fixture, not actual Backend", evidence },
        null,
        2,
      ),
    );
    server.kill("SIGTERM");
    await server.status;
  }
});
