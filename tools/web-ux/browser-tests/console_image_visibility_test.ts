/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { fromFileUrl } from "@std/path";
import { type Browser, chromium, type Page, type Request } from "playwright";
import type {
  SessionConversationTurn,
  SessionHistoryPage,
  SessionSnapshotEntry,
  SessionToolAttachment,
} from "../../../web/workspace/src/lib/generated/protocol.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

// 本番static assetsとConsole描画を接続し、履歴APIと画像bytesだけをloopback fixtureで代替する。
// Backend認証・Worker実行は検証しない。callerが標準rootへ隔離buildを配置する。buildは起動しない。
const staticRoot = fromFileUrl(new URL("../../../web/workspace/build/", import.meta.url));
const output = fromFileUrl(new URL("../../../.yoi/dev/console-image-review/", import.meta.url));
const fixtureServer = fromFileUrl(new URL("console_history_fixture_server.ts", import.meta.url));
const consolePath = "/w/console-history-review/workers/W-900-console-fixture/console";
const inventoryPath = "/api/w/console-history-review/workers";
const imagePath = "/api/w/console-history-review/runtimes/runtime-a/workers/worker-a" +
  "/sessions/session-a/attachments/console-image-visibility";
const imageSvg =
  '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" viewBox="0 0 320 180"><rect width="320" height="180" fill="#28789e"/><circle cx="160" cy="80" r="40" fill="#fff"/><text x="160" y="150" text-anchor="middle" fill="#fff" font-size="18">Synthetic ViewImage fixture</text></svg>';
const attachment: SessionToolAttachment = {
  attachment_id: "console-image-visibility",
  media_type: "image/svg+xml",
  byte_len: new TextEncoder().encode(imageSvg).byteLength,
};
const imageEntries: SessionSnapshotEntry[] = [
  {
    kind: "message",
    entry_id: "user-7",
    timestamp: 7,
    provenance: "human_input",
    role: "user",
    content: [{ kind: "text", text: "question 7" }],
  },
  {
    kind: "tool_call",
    entry_id: "read-before-image",
    timestamp: 7,
    provenance: "model_output",
    call_id: "read-before-image",
    name: "Read",
    arguments: '{"file_path":"fixture.txt"}',
  },
  {
    kind: "tool_result",
    entry_id: "read-before-image-result",
    timestamp: 7,
    provenance: "tool_output",
    call_id: "read-before-image",
    summary: "Read fixture",
    content: "Read fixture",
    is_error: false,
  },
  {
    kind: "tool_call",
    entry_id: "image-call",
    timestamp: 7,
    provenance: "model_output",
    call_id: "image-call",
    name: "ViewImage",
    arguments: '{"path":"fixture.svg"}',
  },
  {
    kind: "tool_result",
    entry_id: "image-result",
    timestamp: 7,
    provenance: "tool_output",
    call_id: "image-call",
    summary: "Synthetic image attached",
    content: "Synthetic image attached",
    is_error: false,
    attachments: [attachment],
  },
  {
    kind: "message",
    entry_id: "following-image-text",
    timestamp: 7,
    provenance: "model_output",
    role: "assistant",
    content: [{
      kind: "text",
      // 画像の後ろに実際の本文を置き、scroll rootから十分離す。CSSによるfixture余白は作らない。
      text: Array.from(
        { length: 140 },
        (_, index) => `Following image paragraph ${index + 1}: retained conversation text.`,
      ).join("\n\n"),
    }],
  },
];
const imageTurn: SessionConversationTurn = { turn_id: "user-7", entries: imageEntries };

async function withFixture(run: (browser: Browser, base: string) => Promise<void>) {
  await Deno.stat(`${staticRoot}index.html`);
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  const processes = await startOwnedProcesses(
    [{
      id: "console-image-history",
      command: Deno.execPath(),
      args: ["run", "--allow-net", "--allow-read", fixtureServer, String(port), staticRoot],
      readyUrl: `${base}/health`,
      readyTimeoutMs: 5_000,
    }],
    fixtureServer,
    `${output}after/logs/${port}`,
    [],
  );
  let browser: Browser | undefined;
  try {
    browser = await chromium.launch({ headless: true, timeout: 10_000 });
    await run(browser, base);
  } finally {
    try {
      await browser?.close();
    } finally {
      assertEquals(await stopOwnedProcesses(processes), [], "owned fixture cleanup");
    }
  }
}

async function installHistory(page: Page) {
  const requests: string[] = [];
  const errors: string[] = [];
  const documentStatus: { value: number | null } = { value: null };
  const inventory = {
    subscribedOrder: 0,
    requests: [] as { order: number; status?: number; finished?: boolean }[],
    cancellations: [] as { url: string; error: string; order: number; reason: string }[],
  };
  const inventoryRequests: Request[] = [];
  const replacement = Promise.withResolvers<void>();
  let order = 0;
  let inventoryVerified = false;
  page.on("websocket", (socket) =>
    socket.on("framereceived", ({ payload }) => {
      const frame = JSON.parse(String(payload));
      if (
        frame.frame === "response" && frame.message?.result === "subscribed" &&
        frame.message.payload?.selector?.topic === "workspace_workers"
      ) {
        inventory.subscribedOrder = ++order;
      }
    }));
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "GET" && url.pathname === inventoryPath && !url.search) {
      inventoryRequests.push(request);
      inventory.requests.push({ order: ++order });
    }
  });
  page.on("requestfinished", (request) => {
    const index = inventoryRequests.indexOf(request);
    if (index < 0) return;
    inventory.requests[index].finished = true;
    if (index !== 1) return;
    void (async () => {
      try {
        const response = await request.response();
        inventory.requests[index].status = response?.status();
        assertEquals(response?.status(), 200, "replacement inventory GET must succeed");
        const body = await response!.json();
        assertEquals(body.workspace_id, "console-history-review");
        assert(
          body.items.some((worker: { resource_key: string }) => worker.resource_key === "W-900"),
          "finished replacement inventory must contain the fixture Worker",
        );
      } catch (error) {
        errors.push(`replacement inventory: ${error}`);
      } finally {
        replacement.resolve();
      }
    })();
  });
  page.on("response", (response) => {
    if (response.request().isNavigationRequest() && response.frame() === page.mainFrame()) {
      documentStatus.value = response.status();
    }
    if (response.status() >= 400) errors.push(`${response.status()} ${response.url()}`);
  });
  page.on("requestfailed", (request) => {
    const error = request.failure()?.errorText ?? "unknown request failure";
    const failedOrder = ++order;
    // subscribedでrefreshCatalogが初回GETをabortする所有lifecycleだけを候補にする。
    // 後続GETのrequestfinished・200・workspace/Worker一致をverifyInventoryで確認してから許容する。
    if (
      !inventoryVerified && request === inventoryRequests[0] &&
      inventoryRequests.length === 2 && error === "net::ERR_ABORTED" &&
      !inventory.requests[0].finished &&
      inventory.requests[0].order < inventory.subscribedOrder &&
      inventory.subscribedOrder < inventory.requests[1].order
    ) {
      inventory.cancellations.push({
        url: request.url(),
        error,
        order: failedOrder,
        reason: "initial inventory superseded by workspace_workers subscription snapshot",
      });
    } else {
      errors.push(`${request.url()}: ${error}`);
    }
    if (request === inventoryRequests[1]) replacement.resolve();
  });
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === imagePath) requests.push(request.url());
  });
  await page.route(`**${imagePath}`, (route) =>
    route.fulfill({
      status: 200,
      contentType: attachment.media_type,
      body: imageSvg,
    }));
  await page.route("**/session/history*", async (route) => {
    const response = await route.fetch({ timeout: 5_000 });
    const body: { availability: string; page: SessionHistoryPage } = await response.json();
    assertEquals(body.availability, "page");
    // snapshotと重なる最古turnをhistory側で置換し、追加pagingによる距離の変動を除く。
    body.page.turns = [imageTurn, ...body.page.turns.filter((turn) => turn.turn_id !== "user-7")];
    body.page.has_more = false;
    body.page.next_cursor = null;
    await route.fulfill({ response, json: body });
  });
  async function verifyInventory() {
    let timer: number | undefined;
    try {
      await Promise.race([
        replacement.promise,
        new Promise<never>((_, reject) => {
          timer = setTimeout(
            () => reject(new Error("replacement inventory GET did not finish")),
            5_000,
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
    assertEquals(inventory.requests.length, 2, "initial and subscription-triggered inventory GETs");
    assertEquals(inventoryRequests[1].url(), inventoryRequests[0].url(), "same inventory endpoint");
    assert(inventory.requests[0].order < inventory.subscribedOrder);
    assert(inventory.subscribedOrder < inventory.requests[1].order);
    assertEquals(
      inventory.requests[1].finished,
      true,
      "replacement inventory must finish, not abort",
    );
    assertEquals(inventory.requests[1].status, 200, "replacement inventory must return 200");
    assert(inventory.cancellations.length <= 1, "only the initial owned cancellation is expected");
    inventoryVerified = true;
    assertEquals(errors, [], "inventory replacement and all unrelated requests must succeed");
  }
  return { requests, errors, documentStatus, inventory, verifyInventory };
}

async function paint(page: Page) {
  // Svelte更新・scroll・native IntersectionObserverに描画機会を渡す。時間sleepで完了を推測しない。
  await page.evaluate(async () => {
    for (let frame = 0; frame < 3; frame++) {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    }
  });
}

async function scroll(page: Page, near: boolean) {
  await page.locator(".console-scroll").evaluate((element, near) => {
    element.scrollTop = near ? 0 : element.scrollHeight;
  }, near);
  await page.waitForFunction((near) => {
    const root = document.querySelector(".console-scroll")!;
    return near ? root.scrollTop <= 1 : root.scrollHeight - root.clientHeight - root.scrollTop <= 1;
  }, near);
  await paint(page);
}

async function decodedImage(page: Page) {
  await page.waitForFunction((id) => {
    const image = document.querySelector<HTMLImageElement>(
      `[data-attachment-id="${id}"] img.thumbnail`,
    );
    return image?.complete && image.naturalWidth === 320 && image.naturalHeight === 180;
  }, attachment.attachment_id);
}

async function capture(
  page: Page,
  name: string,
  state: Awaited<ReturnType<typeof installHistory>>,
) {
  await paint(page);
  const directory = `${output}after`;
  await Deno.mkdir(directory, { recursive: true });
  const screenshot = `${directory}/${name}.png`;
  await page.screenshot({ path: screenshot });
  const overview = await page.getByRole("switch", { name: "Overview", exact: true }).getAttribute(
    "aria-checked",
  );
  const accessibility = await page.locator("body").ariaSnapshot();
  const context = await page.evaluate(() => ({
    path: location.pathname,
    title: document.title,
    viewport: { width: innerWidth, height: innerHeight },
    theme: matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light",
    images: [...document.querySelectorAll<HTMLImageElement>(".console-attachment img")].map((
      image,
    ) => ({
      alt: image.alt,
      complete: image.complete,
      width: image.naturalWidth,
      height: image.naturalHeight,
    })),
    alerts: [...document.querySelectorAll('[role="alert"]')].map((node) => node.textContent),
    pageOverflow: document.documentElement.scrollWidth > innerWidth,
  }));
  const digest = await crypto.subtle.digest("SHA-256", await Deno.readFile(screenshot));
  const sha256 = [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join(
    "",
  );
  await Deno.writeTextFile(
    `${directory}/${name}.review-context.json`,
    JSON.stringify(
      {
        ...context,
        overview,
        documentStatus: state.documentStatus.value,
        accessibility: accessibility.slice(0, 20_000),
        accessibilityTruncated: accessibility.length > 20_000,
        staticRoot,
        screenshot,
        sha256,
        imageRequests: state.requests.length,
        errors: state.errors,
        inventory: state.inventory,
        boundary: "synthetic loopback API; production Console rendering",
      },
      null,
      2,
    ),
  );
}

Deno.test("ViewImage loads only in Normal near the Console viewport and stays hidden in Overview", async () => {
  await withFixture(async (browser, base) => {
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme,
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          page.setDefaultTimeout(5_000);
          page.setDefaultNavigationTimeout(10_000);
          const state = await installHistory(page);
          const name = `${colorScheme}-${width}`;
          await page.goto(`${base}${consolePath}`);
          await state.verifyInventory();
          await page.getByText("Following image paragraph 1:", { exact: false }).first().waitFor();
          const overview = page.getByRole("switch", { name: "Overview", exact: true });
          assertEquals(await overview.getAttribute("aria-checked"), "true");
          await scroll(page, true);
          const first = page.locator('[data-console-line-id*="user-7"]').first();
          const near = await first.evaluate((element) => {
            const row = element.getBoundingClientRect();
            const root = element.closest(".console-scroll")!.getBoundingClientRect();
            return row.top >= root.top - 1 && row.bottom <= root.bottom;
          });
          assert(near, "earliest image turn must be near the console viewport in Overview");
          assertEquals(state.requests.length, 0, "Overview must not request even a nearby image");
          assertEquals(await page.locator(".console-attachment").count(), 0);
          assertEquals(await page.locator(".console-line.tool").count(), 0);
          assertEquals(
            await page.locator(".activity-summary").filter({ hasText: "ViewImage" })
              .allTextContents(),
            ["1 file read・ViewImage"],
            "ViewImage must join the adjacent Read in one Overview activity",
          );
          await capture(page, `${name}-overview-near`, state);

          await scroll(page, false);
          await overview.click();
          assertEquals(await overview.getAttribute("aria-checked"), "false");
          await scroll(page, false);
          const image = page.locator(`[data-attachment-id="${attachment.attachment_id}"]`);
          await image.waitFor({ state: "attached" });
          const distance = await image.evaluate((element) => {
            const root = element.closest(".console-scroll")!.getBoundingClientRect();
            return root.top - element.getBoundingClientRect().bottom;
          });
          assert(
            distance > 2_000,
            `Normal image must be far above the console viewport: ${distance}px`,
          );
          assertEquals(state.requests.length, 0, "Normal must not request a far-off image");
          assertEquals(await image.locator("img").count(), 0);
          await capture(page, `${name}-normal-far`, state);

          const response = page.waitForResponse((response) =>
            new URL(response.url()).pathname === imagePath
          );
          await scroll(page, true);
          assertEquals((await response).status(), 200);
          await decodedImage(page);
          assertEquals(
            state.requests.length,
            1,
            "scrolling near must issue one real image request",
          );
          assert(await image.locator("img.thumbnail").isVisible());
          await capture(page, `${name}-normal-near`, state);

          await overview.click();
          assertEquals(await overview.getAttribute("aria-checked"), "true");
          await paint(page);
          assertEquals(
            await page.locator(".console-attachment").count(),
            0,
            "Overview must remove a loaded image",
          );
          assertEquals(state.requests.length, 1);
          await capture(page, `${name}-overview-again`, state);
          await overview.click();
          await scroll(page, true);
          await decodedImage(page);
          assertEquals(
            state.requests.length,
            1,
            "Normal remount must reuse the loaded attachment cache",
          );
          assertEquals(state.errors, []);
          assertEquals(
            await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
            false,
          );
        } finally {
          await context.close();
        }
      }
    }
  });
});
