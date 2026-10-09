/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Page, type Route } from "playwright";
import { Buffer } from "node:buffer";
import type { FixtureLog } from "../../../../tools/web-ux/drive-fixture/api.ts";
const root = join(dirname(fromFileUrl(import.meta.url)), "../../../..");
const ready = (page: Page) =>
  page.locator('[data-drive-ready="true"]').waitFor();
const button = (page: Page, name: string) =>
  page.getByRole("button", { name, exact: true });
const draft = (page: Page) =>
  page.getByRole("textbox", { name: /^Draft \(expected revision/ });
const preview = (page: Page) =>
  page.getByRole("article", { name: "Current saved content" });
function gate() {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => release = resolve);
  return {
    release,
    get promise() {
      return (async () => {
        let timer: ReturnType<typeof setTimeout> | undefined;
        try {
          return await Promise.race([
            promise,
            new Promise<void>((_resolve, reject) => {
              timer = setTimeout(
                () =>
                  reject(
                    new Error(
                      "Expected API gate was not observed/released within 30 seconds",
                    ),
                  ),
                30000,
              );
            }),
          ]);
        } finally {
          clearTimeout(timer);
        }
      })();
    },
  };
}
async function harness(
  run: (
    origin: string,
    browser: Awaited<ReturnType<typeof chromium.launch>>,
  ) => Promise<void>,
) {
  // Intentionally no build here. Parent must authorize and build production UI first.
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
      join(root, "web/workspace/build"),
    ],
    stdout: "null",
    stderr: "inherit",
  }).spawn();
  try {
    const deadline = Date.now() + 10000;
    while (true) {
      try {
        const response = await fetch(`${origin}/health`);
        await response.body?.cancel();
        if (response.ok) break;
      } catch { /* startup */ }
      if (Date.now() > deadline) {
        throw new Error("Drive fixture failed startup");
      }
      await new Promise((resolve) => setTimeout(resolve, 30));
    }
    const browser = await chromium.launch({ headless: true });
    try {
      await run(origin, browser);
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
}
async function editor(page: Page, text: string) {
  await ready(page);
  await button(page, "Edit text").click();
  await draft(page).fill(text);
}
async function rootFolder(page: Page) {
  await page.getByRole("navigation", { name: "Drive folders" }).getByRole(
    "link",
    {
      name: "Drive",
      exact: true,
    },
  ).click();
  await page.getByRole("heading", { name: "Drive", exact: true }).waitFor();
  await ready(page);
}
async function otherFile(page: Page) {
  await rootFolder(page);
  await page.getByRole("link", { name: "資料", exact: true }).click();
  await ready(page);
  await page.getByRole("link", { name: "notes.txt", exact: true }).click();
  await ready(page);
}
async function fault(origin: string, workspace: string, mode: string) {
  const response = await fetch(`${origin}/__fixture/fault`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ workspace, mode }),
  });
  await response.body?.cancel();
}
async function logs(origin: string) {
  return await (await fetch(`${origin}/__fixture/log`)).json() as FixtureLog[];
}
async function safeFulfill(
  route: Route,
  response: Awaited<ReturnType<Route["fetch"]>>,
) {
  try {
    await route.fulfill({ response });
  } catch { /* selection intentionally canceled this old request */ }
}

Deno.test("production Drive two tabs retain losing CAS draft and never retry with a newer revision", () =>
  harness(async (origin, browser) => {
    const context = await browser.newContext();
    const first = await context.newPage(), second = await context.newPage();
    const errors: string[] = [];
    first.on("pageerror", (e) => errors.push(String(e)));
    second.on("pageerror", (e) => errors.push(String(e)));
    try {
      await first.goto(`${origin}/w/home-owner/drive/3`);
      await second.goto(`${origin}/w/home-owner/drive/3`);
      await editor(first, "First actor wins");
      await editor(second, "Second actor unsaved draft");
      await button(first, "Save").click();
      await first.getByText("Published — DB commit confirmed", { exact: false })
        .last().waitFor();
      await button(second, "Save").click();
      await second.getByText("Revision conflict — your draft is retained.", {
        exact: true,
      })
        .waitFor();
      assertEquals(
        await draft(second).inputValue(),
        "Second actor unsaved draft",
      );
      assertEquals(await button(second, "Save").isDisabled(), true);
      await button(second, "Refresh").click();
      await ready(second);
      assertEquals(
        await draft(second).inputValue(),
        "Second actor unsaved draft",
      );
      assert(
        (await second.getByText("Draft (expected revision 1)", { exact: false })
          .count()) > 0,
      );
      assert(
        (await preview(second).textContent())!.includes("First actor wins"),
      );
      await otherFile(second);
      await editor(second, "Independent second-file draft");
      await rootFolder(second);
      await second.getByRole("link", { name: "README.md", exact: true })
        .click();
      await ready(second);
      await button(second, "Edit text").click();
      assertEquals(
        await draft(second).inputValue(),
        "Second actor unsaved draft",
      );
      const casWrites = (await logs(origin)).filter((x) =>
        x.path === "/mutate"
      );
      assertEquals(casWrites.map((x) => [x.nodeId, x.expectedRevision]), [[
        "3",
        "1",
      ], ["3", "1"]]);
      assertEquals(errors, []);
    } finally {
      await context.close();
    }
  }));

Deno.test("production Drive delayed old read cannot replace another file or workspace content", () =>
  harness(async (origin, browser) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    const received = gate(), release = gate();
    let handled: Promise<void> = Promise.resolve();
    await page.route("**/drive/read-text?**", (route) => {
      if (new URL(route.request().url()).searchParams.get("id") !== "3") {
        return route.continue();
      }
      handled = (async () => {
        const response = await route.fetch();
        received.release();
        await release.promise;
        await safeFulfill(route, response);
      })();
      return handled;
    });
    try {
      await page.goto(`${origin}/w/home-owner/drive/3`);
      await received.promise;
      await otherFile(page);
      await editor(page, "Only current file draft");
      release.release();
      await handled;
      assertEquals(await page.locator("main h1").textContent(), "notes.txt");
      assertEquals(await draft(page).inputValue(), "Only current file draft");
      await button(page, "Save").click();
      await page.getByText("Published — DB commit confirmed", { exact: false })
        .last().waitFor();
      const mutation = (await logs(origin)).filter((x) => x.path === "/mutate");
      assertEquals(
        mutation.map((x) => [x.workspace, x.nodeId, x.expectedRevision]),
        [[
          "home-owner",
          "4",
          "1",
        ]],
      );
      // Full workspace navigation must likewise never resurrect the old workspace.
      await page.goto(`${origin}/w/home-member/drive/4`);
      await ready(page);
      assert((await preview(page).textContent())!.includes("Second document"));
      assertEquals(
        await page.locator(".drive-details dd a").getAttribute("href"),
        "/w/home-member/drive/4",
      );
    } finally {
      release.release();
      await handled;
      await context.close();
    }
  }));

Deno.test("production Drive delayed old save and status preserve destination drafts and save identity", () =>
  harness(async (origin, browser) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    const sent = gate(), release = gate();
    let handled = Promise.resolve();
    await page.route("**/drive/mutate", (route) => {
      handled = (async () => {
        const response = await route.fetch();
        sent.release();
        await release.promise;
        await safeFulfill(route, response);
      })();
      return handled;
    });
    try {
      await page.goto(`${origin}/w/home-owner/drive/3`);
      await editor(page, "Old file transmitted draft");
      await button(page, "Save").click();
      await sent.promise;
      await otherFile(page);
      await editor(page, "Destination draft never rebased");
      release.release();
      await handled;
      await page.unroute("**/drive/mutate");
      assertEquals(
        await draft(page).inputValue(),
        "Destination draft never rebased",
      );
      await button(page, "Save").click();
      await page.getByText("Published — DB commit confirmed", { exact: false })
        .last().waitFor();
      await rootFolder(page);
      await page.getByRole("link", { name: "README.md", exact: true }).click();
      await ready(page);
      await page.getByRole("button", { name: "Check request result" }).click();
      await page.getByText("Published — DB commit confirmed", { exact: false })
        .last().waitFor();
      assert(
        (await preview(page).textContent())!.includes(
          "Old file transmitted draft",
        ),
      );
      await editor(page, "Another old request draft");
      await fault(origin, "home-owner", "unknown");
      await button(page, "Save").click();
      await button(page, "Check request result").waitFor();
      const statusSent = gate(), statusRelease = gate();
      let statusHandled = Promise.resolve();
      await page.route("**/drive/requests/*", (route) => {
        statusHandled = (async () => {
          const response = await route.fetch();
          statusSent.release();
          await statusRelease.promise;
          await safeFulfill(route, response);
        })();
        return statusHandled;
      });
      await button(page, "Check request result").click();
      await statusSent.promise;
      await otherFile(page);
      await editor(page, "Destination draft during old status");
      statusRelease.release();
      await statusHandled;
      assertEquals(
        await draft(page).inputValue(),
        "Destination draft during old status",
      );
      assert(
        (await preview(page).textContent())!.includes(
          "Destination draft never rebased",
        ),
      );
      await button(page, "Save").click();
      await page.getByText("Published — DB commit confirmed", { exact: false })
        .last().waitFor();
      const pinnedWrites = (await logs(origin)).filter((x) =>
        x.path === "/mutate"
      );
      assertEquals(pinnedWrites.map((x) => [x.nodeId, x.expectedRevision]), [
        ["3", "1"],
        ["4", "1"],
        ["3", "2"],
        ["4", "2"],
      ]);
    } finally {
      release.release();
      await handled;
      await context.close();
    }
  }));

Deno.test("production Drive rename move delete recreate preserve stable identity and missing old URL", () =>
  harness(async (origin, browser) => {
    const page = await browser.newPage();
    page.on("dialog", (dialog) => dialog.accept());
    try {
      await page.goto(`${origin}/w/home-owner/drive/3`);
      await ready(page);
      await page.getByLabel("More actions", { exact: true }).click();
      await button(page, "Rename / move").click();
      await page.getByRole("textbox", { name: "Name", exact: true }).fill(
        "renamed.md",
      );
      await page.getByRole("dialog").getByRole("button", {
        name: "資料",
        exact: true,
      }).click();
      await button(page, "Apply rename / move").click();
      await page.getByRole("heading", { name: "renamed.md", exact: true })
        .waitFor();
      assertEquals(new URL(page.url()).pathname, "/w/home-owner/drive/3");
      await page.getByRole("navigation", { name: "Drive folders" }).getByRole(
        "link",
        {
          name: "資料",
        },
      ).click();
      await ready(page);
      await page.getByRole("link", { name: "renamed.md", exact: true }).click();
      await ready(page);
      await page.getByLabel("More actions", { exact: true }).click();
      await button(page, "Delete").click();
      await page.getByRole("alert").filter({ hasText: /not[_ ]found/i })
        .waitFor();
      assertEquals(new URL(page.url()).pathname, "/w/home-owner/drive/3");
      await rootFolder(page);
      await page.getByRole("link", { name: "資料", exact: true }).click();
      await ready(page);
      await page.locator(".drive-menu > summary").filter({ hasText: /^New$/ })
        .click();
      await button(page, "New Markdown").click();
      await page.getByRole("textbox", { name: "Name", exact: true }).fill(
        "renamed.md",
      );
      await page.getByRole("textbox", { name: "Markdown", exact: true }).fill(
        "Recreated identity",
      );
      await button(page, "Create").click();
      await page.getByRole("link", { name: "renamed.md", exact: true })
        .waitFor();
      const href = await page.getByRole("link", {
        name: "renamed.md",
        exact: true,
      }).getAttribute(
        "href",
      );
      assert(href !== "/w/home-owner/drive/3");
      await page.goto(`${origin}/w/home-owner/drive/3`);
      await ready(page);
      await page.getByRole("alert").filter({ hasText: /not[_ ]found/i })
        .waitFor();
    } finally {
      await page.close();
    }
  }));

Deno.test("production Drive upload response loss and navigation cancellation query receipts without resend", () =>
  harness(async (origin, browser) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await page.goto(`${origin}/w/home-owner/drive`);
      await ready(page);
      await page.route("**/drive/upload?**", async (route) => {
        await route.fetch();
        await route.abort("failed");
      });
      await button(page, "Upload").click();
      await page.getByLabel("File (maximum 16 MiB)").setInputFiles({
        name: "lost-response.txt",
        mimeType: "text/plain",
        buffer: Buffer.from("Published despite response loss"),
      });
      await button(page, "Upload file").click();
      await button(page, "Check request result").waitFor();
      assertEquals(
        (await logs(origin)).filter((x) => x.path === "/upload").length,
        1,
      );
      await button(page, "Check request result").click();
      await page.getByText("Published — DB commit confirmed", { exact: false })
        .last().waitFor();
      await page.unroute("**/drive/upload?**");
      await button(page, "Refresh").click();
      await ready(page);
      await page.getByRole("link", { name: "lost-response.txt" }).waitFor();
      const transmitted = gate(), release = gate();
      let handled = Promise.resolve();
      await page.route("**/drive/upload?**", (route) => {
        handled = (async () => {
          const response = await route.fetch();
          transmitted.release();
          await release.promise;
          await safeFulfill(route, response);
        })();
        return handled;
      });
      await button(page, "Upload").click();
      await page.getByLabel("File (maximum 16 MiB)").setInputFiles({
        name: "canceled-response.txt",
        mimeType: "text/plain",
        buffer: Buffer.from("Transmitted before navigation abort"),
      });
      await button(page, "Upload file").click();
      await transmitted.promise;
      await button(page, "Cancel transfer / check outcome").click();
      await button(page, "Check request result").waitFor();
      await button(page, "Cancel form").click();
      await page.getByRole("link", { name: "資料", exact: true }).click();
      await page.getByRole("heading", { name: "資料", exact: true }).waitFor();
      await ready(page);
      release.release();
      await handled;
      await rootFolder(page);
      await button(page, "Check request result").waitFor();
      assertEquals(
        (await logs(origin)).filter((x) => x.path === "/upload").length,
        2,
      );
      await button(page, "Check request result").click();
      await page.getByText("Published — DB commit confirmed", { exact: false })
        .last().waitFor();
      assertEquals(
        (await logs(origin)).filter((x) => x.path === "/upload").length,
        2,
      );
    } catch (error) {
      console.log("UPLOAD FAILURE UI", await page.locator("main").innerText());
      console.log("UPLOAD FAILURE REQUESTS", await logs(origin));
      await page.screenshot({
        path: join(root, "web/workspace/.svelte-kit/t724/upload-failure.png"),
        fullPage: true,
      });
      throw error;
    } finally {
      await context.close();
    }
  }));

Deno.test("production Drive workspace switch fences a delayed previous workspace read", () =>
  harness(async (origin, browser) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    const sent = gate(), release = gate();
    let handled = Promise.resolve();
    await page.route("**/home-owner/drive/read-text?**", (route) => {
      handled = (async () => {
        const response = await route.fetch();
        sent.release();
        await release.promise;
        await safeFulfill(route, response);
      })();
      return handled;
    });
    try {
      await page.goto(`${origin}/w/home-owner/drive/3`);
      await sent.promise;
      await page.getByRole("button", {
        name: "Workspace Home Review",
        exact: true,
      }).first()
        .click();
      await page.getByRole("menuitem", {
        name: "Shared Workspace",
        exact: true,
      }).click();
      await page.waitForURL("**/w/home-member");
      await page.locator("aside").getByRole("link", {
        name: "Drive",
        exact: true,
      }).click();
      await ready(page);
      await page.getByRole("link", { name: "資料", exact: true }).click();
      await ready(page);
      await page.getByRole("link", { name: "notes.txt", exact: true }).click();
      await ready(page);
      await editor(page, "Member-only current draft");
      release.release();
      await handled;
      assertEquals(new URL(page.url()).pathname, "/w/home-member/drive/4");
      assertEquals(await draft(page).inputValue(), "Member-only current draft");
      assert((await preview(page).textContent())!.includes("Second document"));
      assertEquals(
        (await logs(origin)).filter((x) => x.path === "/mutate").length,
        0,
      );
    } finally {
      release.release();
      await handled;
      await context.close();
    }
  }));

Deno.test("production Drive denial empty pages truncation unsafe content and image failure remain distinct", () =>
  harness(async (origin, browser) => {
    const page = await browser.newPage();
    const external: string[] = [];
    page.on("request", (request) => {
      if (new URL(request.url()).hostname === "example.invalid") {
        external.push(request.url());
      }
    });
    try {
      await page.goto(`${origin}/w/home-member/drive/3`);
      await editor(page, "Denied member draft");
      await button(page, "Save").click();
      await page.getByText(
        "Read only / access denied by Backend. Editing and upload are unavailable.",
      ).waitFor();
      assertEquals(await draft(page).inputValue(), "Denied member draft");
      assertEquals(await button(page, "Save").count(), 0);
      assertEquals(await draft(page).getAttribute("readonly"), "");
      for (
        const [workspace, text] of [["drive-denied", "denied"], [
          "drive-offline",
          "unavailable",
        ]]
      ) {
        await page.goto(`${origin}/w/${workspace}/drive`);
        await ready(page);
        await page.getByRole("alert").filter({ hasText: new RegExp(text, "i") })
          .waitFor();
        assertEquals(
          await page.getByText("This folder is empty.", { exact: true })
            .count(),
          0,
        );
      }
      await page.goto(`${origin}/w/drive-empty/drive`);
      await ready(page);
      await page.getByText("This folder is empty.", { exact: true }).waitFor();
      await page.goto(`${origin}/w/drive-paged/drive`);
      await ready(page);
      const firstCount = await page.locator(".drive-table tbody tr").count();
      await button(page, "Load more").click();
      await ready(page);
      assert(
        (await page.locator(".drive-table tbody tr").count()) > firstCount,
      );
      await page.goto(`${origin}/w/home-owner/drive/9`);
      await ready(page);
      await page.getByText("Preview truncated at 64 KiB.", { exact: false })
        .waitFor();
      assertEquals(await button(page, "Edit text").count(), 0);
      await page.goto(`${origin}/w/home-owner/drive/6`);
      await ready(page);
      await page.getByText("Empty file.", { exact: true }).waitFor();
      await page.goto(`${origin}/w/home-owner/drive/3`);
      await ready(page);
      assertEquals(
        await page.locator('main a[href^="javascript:"]').count(),
        0,
      );
      assertEquals(await page.locator("main img[onerror]").count(), 0);
      assertEquals(external, []);
      const local = page.getByRole("link", {
        name: "Local folder",
        exact: true,
      });
      assertEquals(await local.getAttribute("href"), "/w/home-owner/drive/2");
      await local.click();
      await page.waitForURL("**/w/home-owner/drive/2");
      await page.getByRole("heading", { name: "資料", exact: true }).waitFor();
      await ready(page);
      assertEquals(new URL(page.url()).pathname, "/w/home-owner/drive/2");
      await page.goto(`${origin}/w/home-owner/drive/8`);
      await ready(page);
      await page.getByText("This file type is download-only.", { exact: false })
        .waitFor();
      assertEquals(
        await page.locator(
          "main svg:not([aria-hidden]), main iframe, main object",
        ).count(),
        0,
      );
      await page.goto(`${origin}/w/home-owner/drive/5`);
      await ready(page);
      await page.waitForFunction(() => {
        const image = document.querySelector("main .drive-image");
        return image instanceof HTMLImageElement && image.complete &&
          image.naturalWidth === 1200 &&
          image.naturalHeight === 800;
      });
      assertEquals(
        await page.getByRole("img", { name: "landscape.png", exact: true })
          .evaluate((image) =>
            image instanceof HTMLImageElement
              ? [image.naturalWidth, image.naturalHeight]
              : []
          ),
        [1200, 800],
      );
      await page.goto(`${origin}/w/home-owner/drive/7`);
      await ready(page);
      await page.getByRole("alert").filter({
        hasText: "Image could not be decoded",
      }).waitFor();
      await fault(origin, "home-owner", "image_error");
      await page.goto(`${origin}/w/home-owner/drive/5`);
      await ready(page);
      await page.getByRole("alert").filter({ hasText: /unavailable/i })
        .waitFor();
    } finally {
      await page.close();
    }
  }));

Deno.test("production Workers owner grants use the authoritative catalog while members have no grant controls", () =>
  harness(async (origin, browser) => {
    const page = await browser.newPage();
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(String(error)));
    try {
      await page.goto(`${origin}/w/home-owner/workers`);
      await page.getByRole("combobox", { name: "Worker", exact: true })
        .selectOption({
          label: "Documentation Worker · fixture-runtime / fixture-worker-a",
        });
      await button(page, "Add Drive grant").waitFor();
      await button(page, "Add Drive grant").click();
      await button(page, "Create Drive grant").click();
      const grants = page.getByRole("region", { name: "Worker Drive grants" });
      await grants.getByText("Read only", { exact: true }).waitFor();
      await page.getByRole("button", { name: /^Revoke Drive grant/ }).click();
      await grants.getByText("No active Drive grants for this Worker.", {
        exact: true,
      }).waitFor();
      await button(page, "Add Drive grant").click();
      await page.getByRole("combobox", { name: "Access", exact: true })
        .selectOption("read_write");
      await button(page, "Create Drive grant").click();
      await grants.getByText("Read and write", { exact: true }).waitFor();
      await page.getByRole("button", { name: /^Revoke Drive grant/ }).click();
      await grants.getByText("No active Drive grants for this Worker.", {
        exact: true,
      }).waitFor();
      const mutations = (await logs(origin)).filter((event) =>
        event.path.startsWith("/grants") && event.method !== "GET"
      );
      assertEquals(
        mutations.map((event) => [event.workspace, event.method, event.path]),
        [
          ["home-owner", "POST", "/grants"],
          ["home-owner", "DELETE", "/grants/1"],
          ["home-owner", "POST", "/grants"],
          ["home-owner", "DELETE", "/grants/2"],
        ],
      );
      await page.goto(`${origin}/w/home-member/workers`);
      await page.locator(".workers-table tbody tr").first().waitFor();
      assertEquals(
        await page.getByRole("region", { name: "Worker Drive grants" }).count(),
        0,
      );
      assertEquals(await button(page, "Add Drive grant").count(), 0);
      assertEquals(errors, []);
    } finally {
      await page.close();
    }
  }));

Deno.test("production Drive Markdown safe image placeholders preserve each image alt label", () =>
  harness(async (origin, browser) => {
    const page = await browser.newPage();
    try {
      await page.goto(`${origin}/w/home-owner/drive/3`);
      await ready(page);
      const labels = await page.locator(".drive-inline-image")
        .allTextContents();
      assert(
        labels.some((label) => label.includes("Image: External")),
        `External image alt label lost: ${JSON.stringify(labels)}`,
      );
      assert(
        labels.some((label) => label.includes("Image: Image")),
        `Local image alt label lost: ${JSON.stringify(labels)}`,
      );
      assertEquals(await preview(page).locator("img").count(), 0);
    } finally {
      await page.close();
    }
  }));

Deno.test("production Drive menus dialogs folder picker and search remain usable at every viewport", () =>
  harness(async (origin, browser) => {
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390, 320]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme,
          reducedMotion: "reduce",
        });
        const page = await context.newPage();
        const errors: string[] = [];
        page.on("pageerror", (error) => errors.push(String(error)));
        const capture = async (surface: string) => {
          const geometry = await page.evaluate(() => {
            const main = document.querySelector("main")!;
            const dialog = document.querySelector("dialog[open]");
            return {
              viewport: innerWidth,
              document: document.documentElement.scrollWidth,
              main: main.clientWidth,
              mainScroll: main.scrollWidth,
              dialog: dialog?.clientWidth,
              dialogScroll: dialog?.scrollWidth,
            };
          });
          assert(geometry.document <= width, JSON.stringify(geometry));
          assert(
            geometry.mainScroll <= geometry.main + 1,
            JSON.stringify(geometry),
          );
          if (geometry.dialog) {
            assert(
              geometry.dialogScroll! <= geometry.dialog + 1,
              JSON.stringify(geometry),
            );
          }
          await page.screenshot({
            path: join(
              root,
              `web/workspace/.svelte-kit/drive-redesign/${surface}-${colorScheme}-${width}.png`,
            ),
          });
        };
        try {
          const reset = await fetch(`${origin}/__fixture/reset`, {
            method: "POST",
          });
          await reset.body?.cancel();
          await page.goto(`${origin}/w/home-owner/drive`);
          await ready(page);
          const newMenu = page.locator(".drive-menu > summary").filter({
            hasText: /^New$/,
          });
          assertEquals(await button(page, "New folder").isVisible(), false);
          await capture("files");
          await newMenu.press("Enter");
          await button(page, "New folder").waitFor();
          const menuBox = await page.locator(
            ".drive-create-menu .drive-menu-panel",
          ).boundingBox();
          assert(
            menuBox && menuBox.x >= 0 && menuBox.x + menuBox.width <= width,
            JSON.stringify(menuBox),
          );
          await capture("menu");
          await button(page, "New folder").click();
          const dialog = page.getByRole("dialog", {
            name: "New folder",
            exact: true,
          });
          await dialog.waitFor();
          assert(
            await dialog.getByLabel("Name", { exact: true }).evaluate((el) =>
              el === document.activeElement
            ),
          );
          await capture("create");
          for (let i = 0; i < 8; i++) {
            await page.keyboard.press("Tab");
            // Native dialogs may cycle through browser chrome (reported as body),
            // but background page controls must stay inert.
            assert(
              await dialog.evaluate((el) =>
                el.contains(document.activeElement) ||
                document.activeElement === document.body
              ),
              "Modal must not focus background controls",
            );
          }
          await page.keyboard.press("Escape");
          await dialog.waitFor({ state: "hidden" });
          await newMenu.locator(":scope:focus").waitFor();
          await newMenu.press("Enter");
          await button(page, "New folder").focus();
          await page.keyboard.press("Escape");
          assertEquals(await button(page, "New folder").isVisible(), false);
          await newMenu.locator(":scope:focus").waitFor();
          await newMenu.press("Enter");
          await button(page, "New folder").click();
          const folderName = `Created ${colorScheme} ${width}`;
          await dialog.getByLabel("Name", { exact: true }).fill(folderName);
          await button(page, "Create").click();
          await dialog.waitFor({ state: "hidden" });
          await page.getByRole("link", { name: folderName, exact: true })
            .waitFor();
          await page.getByRole("searchbox", { name: "Search Drive" }).fill(
            folderName,
          );
          await button(page, "Search").click();
          await button(page, "Show folder").waitFor();
          await ready(page);
          assertEquals(await page.locator(".drive-table tbody tr").count(), 1);
          await button(page, "Show folder").click();
          await ready(page);
          await page.getByRole("link", { name: "README.md", exact: true })
            .click();
          await page.waitForURL("**/w/home-owner/drive/3");
          await ready(page);
          await preview(page).waitFor();
          await capture("preview");
          const more = page.getByLabel("More actions", { exact: true });
          await more.click();
          await button(page, "Rename / move").click();
          const move = page.getByRole("dialog", {
            name: "Rename / move",
            exact: true,
          });
          await move.getByRole("button", { name: "資料", exact: true }).click();
          await page.locator(".drive-folder-location strong").filter({
            hasText: "資料",
          }).waitFor();
          await capture("move");
          await button(page, "Parent folder").click();
          await move.getByRole("button", { name: "資料", exact: true })
            .waitFor();
          assertEquals(
            await move.getByRole("button", { name: "README.md", exact: true })
              .count(),
            0,
          );
          await page.keyboard.press("Escape");
          await move.waitFor({ state: "hidden" });
          await more.locator(":scope:focus").waitFor();
          assertEquals(errors, []);
        } finally {
          await context.close();
        }
      }
    }
  }));
