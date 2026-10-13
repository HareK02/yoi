/// <reference lib="dom" />
import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { type Browser, chromium, type Page, type Request, type Route } from "npm:playwright@1.59.1";
import { loadScenario } from "../src/scenario.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

// Explicit browser boundary: production static assets + synthetic loopback API.
// No real Backend auth, Worker execution, or application test hooks.
// The caller supplies the built workspace via the standard suite layout; for isolated
// runs, copy this suite into a tree whose web/workspace/build points at isolated assets.
// This suite NEVER runs a build, mutates assets, or falls back to another static root.
const staticRoot = new URL("../../../web/workspace/build/", import.meta.url).pathname;
const output = new URL("../../../.yoi/dev/navigation-visual/", import.meta.url).pathname;
const phase = "after";
const listPath = "/w/tickets/tickets";
const detailPath = "/w/tickets/tickets/T-2-read-only-investigation";
const otherDetailPath = "/w/tickets/tickets/T-3-independent-review-before-merge";

async function withFixture(run: (browser: Browser, base: string) => Promise<void>) {
  // A missing build is an explicit failure; no alternate root or implicit build.
  await Deno.stat(`${staticRoot}index.html`);
  const scenarioPath = new URL("../scenarios/tickets.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  scenario.processes![0].args![4] = String(port);
  scenario.processes![0].args![5] = staticRoot;
  scenario.processes![0].readyUrl = `${base}/health`;
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    `${output}${phase}/logs/${port}`,
    [],
  );
  let browser: Browser | null = null;
  try {
    browser = await chromium.launch({ headless: true });
    await run(browser, base);
  } finally {
    await browser?.close();
    assertEquals(await stopOwnedProcesses(processes), [], "owned fixture cleanup");
  }
}

async function bounded<T>(promise: Promise<T>): Promise<T> {
  let timer: number | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("Response gate timed out")), 15_000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

// Fetch the real synthetic response, but do not deliver it to the browser until release.
// held means response bytes exist in Playwright, NOT requestfinished in the browser.
async function responseGate(
  page: Page,
  pattern: string | RegExp,
  matches: (request: Request) => boolean = () => true,
  replacement?: { status: number; body: string; contentType: string },
  transform?: (body: Record<string, unknown>) => unknown,
) {
  const held = Promise.withResolvers<void>();
  const releaseSignal = Promise.withResolvers<void>();
  const delivered = Promise.withResolvers<void>();
  let closed = true;
  let intercepted = false;
  let browserFinished = false;
  let gatedRequest: Request | undefined;
  const onFinished = (request: Request) => {
    if (request === gatedRequest) browserFinished = true;
  };
  page.on("requestfinished", onFinished);
  const handler = async (route: Route) => {
    if (intercepted || !matches(route.request())) return await route.fallback();
    intercepted = true;
    gatedRequest = route.request();
    try {
      const response = replacement ? undefined : await route.fetch();
      const json = transform && response ? transform(await response.json()) : undefined;
      held.resolve();
      await releaseSignal.promise;
      if (replacement) await route.fulfill(replacement);
      else await route.fulfill({ response, ...(json === undefined ? {} : { json }) });
      delivered.resolve();
    } catch (error) {
      held.reject(error);
      delivered.reject(error);
    }
  };
  await page.route(pattern, handler);
  // Attach handlers immediately so cancellation during teardown isn't an unhandled rejection.
  void held.promise.catch(() => {});
  void delivered.promise.catch(() => {});
  return {
    held: () => bounded(held.promise),
    assertClosed() {
      assert(intercepted, "API response must be intercepted first");
      assert(closed, "response gate must remain CLOSED during navigation assertions");
      assertEquals(browserFinished, false, "browser must not have finished the held request");
    },
    release() {
      closed = false;
      releaseSignal.resolve();
    },
    delivered: () => bounded(delivered.promise),
    async dispose() {
      closed = false;
      releaseSignal.resolve();
      await page.unroute(pattern, handler);
      page.off("requestfinished", onFinished);
    },
  };
}
type Gate = Awaited<ReturnType<typeof responseGate>>;

async function loading(page: Page, gate: Gate, path: string, title: RegExp, updating = false) {
  await gate.held();
  gate.assertClosed();
  await page.waitForURL((url) => url.pathname + url.search === path, { timeout: 5_000 });
  await page.getByRole("heading", { name: title, level: 1 }).waitFor({ timeout: 5_000 });
  await page.getByRole("status").filter({ hasText: updating ? /^Updating…$/ : /Loading/ })
    .first().waitFor({ timeout: 5_000 });
  gate.assertClosed();
}

// Synthetic links exercise the public SvelteKit delegated link handler, not a full reload.
// Used only where the fixture has no product link (cross-workspace and stale slug scenarios).
async function navigate(page: Page, path: string) {
  await page.evaluate((path) => {
    const link = document.createElement("a");
    link.href = path;
    link.dataset.sveltekitPreloadData = "off";
    document.body.append(link);
    link.click();
    link.remove();
  }, path);
}
async function clickTickets(page: Page) {
  // Actual product sidebar link, even when the mobile sidebar is folded.
  await page.locator("a.sidebar-link").filter({ hasText: /^Tickets$/ })
    .evaluate((link) => (link as HTMLAnchorElement).click());
}
async function paint(page: Page) {
  await page.evaluate(() =>
    new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    )
  );
}
async function capture(page: Page, name: string, gate?: Gate) {
  gate?.assertClosed();
  await paint(page);
  const directory = `${output}${phase}`;
  await Deno.mkdir(directory, { recursive: true });
  await page.screenshot({ path: `${directory}/${name}.png`, fullPage: true });
  // Safe, fixture-only evidence. Deliberately no cookies, storage, headers, trace or HAR.
  const context = await page.evaluate(() => ({
    path: location.pathname + location.search,
    title: document.title,
    headings: [...document.querySelectorAll("h1")].map((node) => node.textContent),
    statuses: [...document.querySelectorAll('[role="status"]')].map((node) => node.textContent),
    liveRegions: [...document.querySelectorAll('[aria-live="polite"]')].map((node) =>
      node.textContent
    ),
    alerts: [...document.querySelectorAll('[role="alert"]')].map((node) => node.textContent),
    viewport: { width: innerWidth, height: innerHeight },
    theme: matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light",
    pageOverflow: document.documentElement.scrollWidth > innerWidth,
  }));
  assertEquals(context.pageOverflow, false, "navigation state must not overflow viewport");
  await Deno.writeTextFile(
    `${directory}/${name}.json`,
    JSON.stringify(
      {
        ...context,
        responseGate: gate ? "closed" : "resolved",
        staticRoot,
        boundary: "synthetic loopback fixture; no real Backend authentication",
      },
      null,
      2,
    ),
  );
  gate?.assertClosed();
}
async function rows(page: Page) {
  await page.locator("a.ticket-list-row").first().waitFor();
}
async function search(page: Page, query: string) {
  await page.getByRole("searchbox", { name: "Search tickets", exact: true }).fill(query);
  await page.getByRole("button", { name: "Search", exact: true }).click();
}
const queryPattern = "**/api/w/tickets/tickets/query";

Deno.test("immediate navigation: Orchestrator -> Tickets -> detail before responses, visual matrix", async () => {
  await withFixture(async (browser, base) => {
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390, 320]) {
        const context = await browser.newContext({
          colorScheme,
          viewport: { width, height: 900 },
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          const errors: string[] = [];
          page.on("pageerror", (error) => errors.push(String(error)));
          await page.goto(`${base}/w/tickets/orchestrator`);
          await page.locator(".orchestrator-assignment").first().waitFor();
          const listGate = await responseGate(page, queryPattern);
          try {
            await clickTickets(page);
            await loading(page, listGate, listPath, /^Tickets$/);
            assertEquals(await page.locator(".orchestrator-assignment").count(), 0);
            await capture(page, `${colorScheme}-${width}-tickets-gated`, listGate);
            listGate.release();
            await rows(page);
            await capture(page, `${colorScheme}-${width}-tickets-resolved`);
          } finally {
            await listGate.dispose();
          }
          const detailGate = await responseGate(page, "**/api/w/tickets/tickets/T-2");
          try {
            await page.locator(`a.ticket-list-row[href="${detailPath}"]`).click();
            await loading(page, detailGate, detailPath, /^Ticket$/);
            assertEquals(await page.locator("a.ticket-list-row").count(), 0);
            assertEquals(
              await page.getByRole("button", { name: "Release assignment", exact: true }).count(),
              0,
            );
            await capture(page, `${colorScheme}-${width}-detail-gated`, detailGate);
            detailGate.release();
            await page.getByRole("heading", { name: "Read-only investigation", exact: true })
              .waitFor();
            await capture(page, `${colorScheme}-${width}-detail-resolved`);
          } finally {
            await detailGate.dispose();
          }
          assertEquals(errors, []);
        } finally {
          await context.close();
        }
      }
    }
  });
});

Deno.test("immediate navigation: query and back/forward commit URLs with response gates closed", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}${listPath}`);
    await rows(page);
    const open = await responseGate(page, queryPattern);
    try {
      await search(page, "state:closed");
      await loading(page, open, `${listPath}?q=state%3Aclosed`, /^Tickets$/, true);
      open.release();
      await page.locator(".ticket-list-key").filter({ hasText: /^T-4$/ }).waitFor();
    } finally {
      await open.dispose();
    }
    for (const direction of ["back", "forward"] as const) {
      const gate = await responseGate(page, queryPattern);
      try {
        // Don't await goBack/goForward's optional load lifecycle while API is gated.
        await page.evaluate(
          (direction) => direction === "back" ? history.back() : history.forward(),
          direction,
        );
        const path = direction === "back" ? listPath : `${listPath}?q=state%3Aclosed`;
        await loading(page, gate, path, /^Tickets$/, true);
        await capture(page, `query-${direction}-gated`, gate);
        gate.release();
        await page.getByRole("searchbox", { name: "Search tickets", exact: true })
          .waitFor();
        await page.waitForFunction(
          (value) =>
            (document.querySelector('input[type="search"]') as HTMLInputElement)?.value === value,
          direction === "back" ? "state:open" : "state:closed",
        );
        await rows(page);
        if (direction === "forward") {
          assertEquals(await page.locator(".ticket-list-key").allTextContents(), ["T-4"]);
        }
      } finally {
        await gate.dispose();
      }
    }
  });
});

Deno.test("immediate navigation: rapid A -> B rejects late A data and stale canonical redirects", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}${listPath}`);
    await rows(page);
    // First canonical A (late data), then an obsolete slug A (late canonical redirect).
    for (const pathA of [detailPath, "/w/tickets/tickets/T-2-obsolete-title"]) {
      const gateA = await responseGate(page, "**/api/w/tickets/tickets/T-2");
      const gateB = await responseGate(page, "**/api/w/tickets/tickets/T-3");
      try {
        await navigate(page, pathA);
        await loading(page, gateA, pathA, /^Ticket$/);
        await navigate(page, otherDetailPath);
        await loading(page, gateB, otherDetailPath, /^Ticket$/);
        gateA.assertClosed();
        gateB.release();
        await page.getByRole("heading", { name: "Independent review before merge", exact: true })
          .waitFor();
        const response = page.waitForResponse((response) =>
          new URL(response.url()).pathname === "/api/w/tickets/tickets/T-2"
        );
        gateA.release();
        await response;
        await gateA.delivered();
        await paint(page);
        assertEquals(new URL(page.url()).pathname, otherDetailPath);
        assert(
          await page.getByRole("heading", {
            name: "Independent review before merge",
            exact: true,
          }).isVisible(),
        );
        assertEquals(
          await page.getByRole("heading", { name: "Read-only investigation", exact: true })
            .count(),
          0,
        );
        await capture(
          page,
          pathA === detailPath
            ? "late-old-response-resolved"
            : "stale-canonical-redirect-resolved",
        );
      } finally {
        await gateA.dispose();
        await gateB.dispose();
      }
      await navigate(page, listPath);
      await rows(page);
    }
  });
});

Deno.test("immediate navigation: slow workspace switch clears old identity and permission projection", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    // Same existing fixture server, distinct identities and explicit opposite permissions.
    await page.route("**/api/w/*/workspace", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      const old = body.workspace_id === "tickets";
      body.display_name = old ? "Old privileged workspace" : "New read-only workspace";
      body.permissions = {
        manage_repositories: old,
        manage_secrets: old,
        manage_runtimes: old,
        delete_workspace: old,
      };
      await route.fulfill({ response, json: body });
    });
    await page.goto(`${base}${detailPath}`);
    await page.getByRole("heading", { name: "Read-only investigation", exact: true }).waitFor();
    await page.getByText("Old privileged workspace", { exact: true }).first().waitFor();
    assert(
      await page.getByRole("button", { name: "Release assignment", exact: true }).isVisible(),
    );
    // route.fetch bypasses other Playwright handlers: transform this held response too.
    const gate = await responseGate(
      page,
      "**/api/w/tickets-readonly/workspace",
      () => true,
      undefined,
      (body) => ({ ...body, display_name: "New read-only workspace" }),
    );
    const target = detailPath.replace("/w/tickets/", "/w/tickets-readonly/");
    try {
      await navigate(page, target);
      await loading(page, gate, target, /^Workspace$/);
      assertEquals(await page.getByText("Old privileged workspace", { exact: true }).count(), 0);
      assertEquals(
        await page.getByRole("button", { name: "Release assignment", exact: true }).count(),
        0,
      );
      assertEquals(
        await page.locator('a[href="/w/tickets/settings"], a[href="/w/tickets"]').count(),
        0,
      );
      await capture(page, "workspace-switch-gated", gate);
      gate.release();
      await page.getByRole("heading", { name: "Read-only investigation", exact: true }).waitFor();
      await page.getByText("New read-only workspace", { exact: true }).first().waitFor();
      assertEquals(
        await page.getByRole("button", { name: "Release assignment", exact: true }).count(),
        0,
      );
      await capture(page, "workspace-switch-resolved");
    } finally {
      await gate.dispose();
    }
  });
});

Deno.test("immediate navigation: cold load and unrelated global catalog response cannot block destination", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    const catalog = await responseGate(page, /\/api\/workspaces(?:\?|$)/);
    const tickets = await responseGate(page, queryPattern);
    try {
      await page.goto(`${base}${listPath}`, { waitUntil: "domcontentloaded" });
      await catalog.held();
      await loading(page, tickets, listPath, /^Tickets$/);
      catalog.assertClosed();
      await capture(page, "cold-load-gated", tickets);
      tickets.release();
      await rows(page);
      // Global catalog remains unavailable while the page is fully usable.
      catalog.assertClosed();
      const detail = await responseGate(page, "**/api/w/tickets/tickets/T-2");
      try {
        await page.locator(`a.ticket-list-row[href="${detailPath}"]`).click();
        await loading(page, detail, detailPath, /^Ticket$/);
        catalog.assertClosed();
        detail.release();
        await page.getByRole("heading", { name: "Read-only investigation", exact: true })
          .waitFor();
        catalog.assertClosed();
        await capture(page, "global-catalog-delayed-detail-resolved");
      } finally {
        await detail.dispose();
      }
      catalog.release();
    } finally {
      await catalog.dispose();
      await tickets.dispose();
    }
  });
});

Deno.test("immediate navigation: failed read stays at destination and gated Retry recovers", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/orchestrator`);
    await page.locator(".orchestrator-assignment").first().waitFor();
    const failure = await responseGate(page, queryPattern, () => true, {
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ message: "Navigation fixture unavailable" }),
    });
    try {
      await clickTickets(page);
      await loading(page, failure, listPath, /^Tickets$/);
      failure.release();
      await page.getByRole("alert").filter({
        hasText: /Ticket Browser API request failed with HTTP 503/,
      })
        .waitFor();
      assertEquals(new URL(page.url()).pathname, listPath);
      assertEquals(
        await page.getByText("No tickets match this query.", { exact: true }).count(),
        0,
      );
      await capture(page, "read-error");
    } finally {
      await failure.dispose();
    }
    const retry = await responseGate(page, queryPattern);
    try {
      await page.getByRole("button", { name: "Retry", exact: true }).click();
      // Product Retry performs a local query read, not SvelteKit invalidation.
      // Its polite live-region status must be visible while that response stays CLOSED.
      await retry.held();
      retry.assertClosed();
      assertEquals(new URL(page.url()).pathname, listPath);
      await page.getByRole("heading", { name: "Tickets", level: 1, exact: true }).waitFor();
      await page.locator('.ticket-list-page-state[aria-live="polite"]')
        .filter({ hasText: /^Loading…$/ }).waitFor();
      retry.assertClosed();
      await capture(page, "read-retry-gated", retry);
      retry.release();
      await rows(page);
      assertEquals(await page.getByRole("alert").count(), 0);
      await capture(page, "read-retry-resolved");
    } finally {
      await retry.dispose();
    }
  });
});
