/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Page } from "playwright";
import { metadataFixture } from "../../../web/workspace/src/lib/workspace/settings/identity.test-fixtures.ts";
const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const evidence = join(root, "target/web-ux/settings-interactions");
async function ready(page: Page, owner = true) {
  await page.locator("[data-name-ready=true]").waitFor();
  if (owner) await page.locator("[data-identity-ready=true]").waitFor();
}
async function noOverflow(page: Page) {
  assertEquals(
    await page.locator("main").evaluate((main) => ({
      page: document.documentElement.scrollWidth > innerWidth,
      main: main.scrollWidth > main.clientWidth,
    })),
    { page: false, main: false },
  );
}
Deno.test("production Workspace Settings separates read/edit, discloses details, and preserves keyboard and destructive safety", async () => {
  const build = await new Deno.Command(Deno.execPath(), {
    args: ["task", "build"],
    cwd: join(root, "web/workspace"),
    stdout: "piped",
    stderr: "piped",
  }).output();
  assert(build.success, new TextDecoder().decode(build.stderr));
  await Deno.mkdir(evidence, { recursive: true });
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
      } catch { /* starting */ }
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
            const mutations: string[] = [];
            page.on("pageerror", (error) => errors.push(String(error)));
            page.on("console", (message) => {
              if (message.type() === "error") errors.push(message.text());
            });
            page.on("requestfailed", (request) => errors.push(request.url()));
            page.on("request", (request) => {
              if (request.method() !== "GET") mutations.push(request.url());
            });
            for (
              const workspace of ["home-owner", "home-member", "home-long"]
            ) {
              const owner = workspace !== "home-member";
              await page.goto(`${url}/w/${workspace}/settings`);
              await ready(page, owner);
              assertEquals(await page.locator("main h1").allTextContents(), [
                "Workspace Identity",
              ]);
              assertEquals(
                await page.locator("main input:visible, main textarea:visible")
                  .count(),
                0,
              );
              assertEquals(await page.locator("main details[open]").count(), 0);
              assertEquals(await page.locator("main .badge").count(), 0);
              assertEquals(
                await page.getByRole("button", { name: "Copy bundle" }).count(),
                owner ? 1 : 0,
              );
              const edit = page.getByRole("button", { name: "Edit name" });
              assert((await edit.boundingBox())!.y < 900);
              await noOverflow(page);
              const saved = await page.locator(".saved-name").innerText();
              await edit.focus();
              await page.keyboard.press("Enter");
              const input = page.getByRole("textbox", {
                name: "New display name",
              });
              assertEquals(
                await input.evaluate((node) => node === document.activeElement),
                true,
              );
              assertEquals(await input.inputValue(), saved);
              await input.fill("Discard draft");
              if (workspace === "home-long") {
                await page.screenshot({
                  path: join(evidence, `edit-${colorScheme}-${width}.png`),
                });
              }
              await page.getByRole("button", { name: "Cancel", exact: true })
                .click();
              assertEquals(
                await edit.evaluate((node) => node === document.activeElement),
                true,
              );
              assertEquals(
                await page.locator(".saved-name").innerText(),
                saved,
              );
              const technical = page.getByText("Technical details", {
                exact: true,
              });
              await technical.focus();
              await page.keyboard.press("Enter");
              assertEquals(
                await page.getByText("Source", { exact: true }).isVisible(),
                true,
              );
              await page.keyboard.press("Enter");
              assertEquals(
                await page.getByText("Source", { exact: true }).isVisible(),
                false,
              );
              if (owner) {
                const bundle = page.getByText("Public bundle and key details", {
                  exact: true,
                });
                await bundle.focus();
                await page.keyboard.press("Enter");
                assertEquals(
                  await page.getByRole("textbox", {
                    name: "Public identity bundle",
                  }).isVisible(),
                  true,
                );
                assertEquals(
                  await page.getByRole("textbox", {
                    name: "Public identity bundle",
                  }).getAttribute("readonly"),
                  "",
                );
                await noOverflow(page);
                if (workspace === "home-owner") {
                  await page.screenshot({
                    path: join(evidence, `bundle-${colorScheme}-${width}.png`),
                  });
                }
                await bundle.click();
                const danger = page.locator(".deletion > summary");
                await danger.focus();
                await page.keyboard.press("Enter");
                await page.getByRole("button", {
                  name: "Review deletion impact",
                }).click();
                const confirmation = page.getByRole("textbox", {
                  name: /^Type .* to confirm$/,
                });
                await confirmation.waitFor();
                assertEquals(
                  await confirmation.evaluate((node) => node === document.activeElement),
                  true,
                );
                const deletion = page.getByRole("button", {
                  name: "Delete Workspace",
                  exact: true,
                });
                assertEquals(await deletion.isDisabled(), true);
                await confirmation.fill("wrong");
                assertEquals(await deletion.isDisabled(), true);
                await confirmation.fill(saved);
                assertEquals(await deletion.isDisabled(), false);
                await noOverflow(page);
                if (workspace === "home-owner") {
                  await page.screenshot({
                    path: join(evidence, `delete-${colorScheme}-${width}.png`),
                  });
                }
                await page.getByRole("button", { name: "Cancel", exact: true })
                  .click();
                assertEquals(
                  await danger.evaluate((node) => node === document.activeElement),
                  true,
                );
                assertEquals(await page.locator(".deletion[open]").count(), 0);
              }
            }
            assertEquals(mutations, []);
            assertEquals(errors, []);
            await page.getByRole("button", { name: "Edit name" }).click();
            const renamed = `Confirmed name ${width} ${colorScheme}`;
            await page.getByRole("textbox", { name: "New display name" }).fill(
              renamed,
            );
            await page.getByRole("button", { name: "Save name" }).click();
            await page.getByText("Workspace name saved.", { exact: true })
              .waitFor();
            assertEquals(
              await page.locator(".saved-name").innerText(),
              renamed,
            );
            assertEquals(await page.locator("main input:visible").count(), 0);
            assertEquals(mutations.length, 1);
            assertEquals(errors, []);
            // Restore the synthetic name so every width actually exercises long text.
            const currentResponse = await context.request.get(`${url}/api/w/home-long/settings`);
            const current = await currentResponse.json();
            const reset = await context.request.put(`${url}/api/w/home-long/settings`, {
              data: {
                display_name: metadataFixture("home-long").display_name,
                expected_updated_at: current.updated_at,
              },
            });
            assertEquals(reset.status(), 200);
          } finally {
            await context.close();
          }
        }
      }
      const page = await browser.newPage({
        viewport: { width: 390, height: 844 },
      });
      try {
        await page.goto(`${url}/w/home-empty/settings`);
        await ready(page);
        await page.getByText("Not provisioned", { exact: true }).waitFor();
        await page.getByRole("button", { name: "Provision identity" }).click();
        await page.getByRole("button", { name: "Copy bundle" }).waitFor();
        await page.goto(`${url}/w/home-error/settings`);
        await ready(page);
        assertEquals(
          await page.getByRole("button", { name: "Edit name" }).isVisible(),
          true,
        );
        assertEquals(
          await page.getByRole("button", { name: "Retry identity" })
            .isVisible(),
          true,
        );
        await page.goto(`${url}/w/home-owner/settings`);
        await ready(page);
        await page.route("**/settings", async (route) => {
          if (route.request().method() === "PUT") {
            await route.fulfill({
              status: 409,
              body: "Workspace metadata changed. Reload the saved name.",
            });
          } else await route.continue();
        });
        await page.getByRole("button", { name: "Edit name" }).click();
        await page.getByRole("textbox", { name: "New display name" }).fill(
          "Keep this draft",
        );
        await page.getByRole("button", { name: "Save name" }).click();
        await page.getByRole("alert").waitFor();
        assertEquals(
          await page.getByRole("textbox", { name: "New display name" })
            .inputValue(),
          "Keep this draft",
        );
        await page.screenshot({
          path: join(evidence, "save-error-mobile.png"),
        });
        await page.getByRole("button", { name: "Reload saved name" }).click();
        await page.getByRole("button", { name: "Edit name" }).waitFor();
        assertEquals(await page.getByRole("alert").count(), 0);
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
