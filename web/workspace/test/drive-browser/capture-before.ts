/// <reference lib="dom" />
// Captures the preserved production baseline; no UI mutations or Backend connection.
import { chromium } from "playwright";
import { dirname, fromFileUrl, join } from "@std/path";
const root = join(dirname(fromFileUrl(import.meta.url)), "../../../..");
const artifactRoot = join(root, "web/workspace/.svelte-kit/t724/before-build");
const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
const port = (listener.addr as Deno.NetAddr).port;
listener.close();
const server = new Deno.Command(Deno.execPath(), {
  args: [
    "run",
    "--allow-net",
    "--allow-read",
    join(root, "tools/web-ux/browser-tests/workspace_home_fixture_server.ts"),
    String(port),
    join(artifactRoot, "site"),
  ],
  stdout: "null",
  stderr: "piped",
}).spawn();
const origin = `http://127.0.0.1:${port}`;
const evidence: unknown[] = [];
try {
  await Deno.mkdir(join(artifactRoot, "screenshots"), { recursive: true });
  const deadline = Date.now() + 10000;
  while (true) {
    try {
      const response = await fetch(`${origin}/health`);
      await response.body?.cancel();
      if (response.ok) break;
    } catch { /* startup */ }
    if (Date.now() > deadline) throw new Error("Home fixture did not start");
    await new Promise((resolve) => setTimeout(resolve, 50));
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
          // Shared Home server predates the sidebar's authoritative /workers fetch.
          // Complete that API fixture only, without changing the production shell.
          await page.route("**/api/w/*/workers", async (route) => {
            const workspaceId = new URL(route.request().url()).pathname.split("/")[3];
            await route.fulfill({
              json: {
                workspace_id: workspaceId,
                items: [],
                limit: 100,
                source: "fixture",
                diagnostics: [],
              },
            });
          });
          for (const state of ["owner", "member", "error", "empty"]) {
            const errors: string[] = [];
            const record = (error: Error) => errors.push(String(error));
            page.on("pageerror", record);
            await page.goto(`${origin}/w/home-${state}`);
            if (state === "error") {
              await page.locator("main [role=alert]").first().waitFor();
            } else {for (
                const feed of ["home-attention", "home-active", "home-recent"]
              ) await page.locator(`[data-feed-ready="${feed}"]`).waitFor();}
            await page.evaluate(() => document.fonts.ready);
            const filename = `home-sidebar-workers-${state}-${colorScheme}-${width}.png`;
            await page.screenshot({
              path: join(artifactRoot, "screenshots", filename),
              fullPage: true,
            });
            evidence.push({
              filename,
              state,
              colorScheme,
              width,
              title: await page.locator("main h1").allTextContents(),
              sidebar: await page.locator("aside").allTextContents(),
              errors,
              overflow: await page.evaluate(() =>
                document.documentElement.scrollWidth > innerWidth
              ),
            });
            page.off("pageerror", record);
          }
        } finally {
          await context.close();
        }
      }
    }
  } finally {
    await browser.close();
  }
  await Deno.writeTextFile(
    join(artifactRoot, "capture.json"),
    JSON.stringify(
      { authority: "fixture, not actual Backend", evidence },
      null,
      2,
    ),
  );
  console.log(
    `Captured ${evidence.length} baseline screenshots at ${artifactRoot}/screenshots`,
  );
} finally {
  server.kill("SIGTERM");
  await server.status;
}
