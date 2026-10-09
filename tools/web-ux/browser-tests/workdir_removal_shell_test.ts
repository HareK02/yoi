/// <reference lib="dom" />
import { assert, assertEquals, assertStringIncludes } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Page } from "playwright";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const evidence = join(root, "target/web-ux/t730-browser");

async function noOverflow(page: Page) {
  assertEquals(
    await page.locator("main").evaluate((main) => ({
      page: document.documentElement.scrollWidth > innerWidth,
      main: main.scrollWidth > main.clientWidth,
    })),
    { page: false, main: false },
  );
}

Deno.test("ordinary removal refreshes failed and successful inventory, supports exact retry, and preserves permission and live guards across viewports and themes", async () => {
  await Deno.mkdir(evidence, { recursive: true });
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const url = `http://127.0.0.1:${port}`;
  const server = await startOwnedProcesses(
    [{
      id: "workdirs",
      command: Deno.execPath(),
      cwd: root,
      args: [
        "run",
        "--allow-net",
        "--allow-read",
        "tools/web-ux/browser-tests/workspace_home_fixture_server.ts",
        String(port),
        "web/workspace/build",
      ],
      readyUrl: `${url}/health`,
    }],
    join(root, "tools/web-ux/scenarios/workdir-removal-light.json"),
    join(evidence, "process-logs"),
    [],
  );
  const observations: unknown[] = [];
  const injectedConsoleErrors: string[] = [];
  const injectedUnexpectedErrors: string[] = [];
  const observeInjectedFailure = (page: Page) => {
    page.on("console", (message) => {
      if (message.type() === "error") {
        injectedConsoleErrors.push(message.text());
      }
    });
    page.on(
      "pageerror",
      (error) => injectedUnexpectedErrors.push(String(error)),
    );
    page.on(
      "requestfailed",
      (request) => injectedUnexpectedErrors.push(request.url()),
    );
  };
  const control = async (value: unknown) => {
    const response = await fetch(`${url}/fixture-workdirs-control`, {
      method: "POST",
      body: JSON.stringify(value),
    });
    assert(response.ok);
    await response.body?.cancel();
  };
  const state = async () => await (await fetch(`${url}/fixture-workdirs`)).json();
  const browser = await chromium.launch({ headless: true });
  try {
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390, 320]) {
        await control({ reset: true });
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme,
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          const errors: string[] = [];
          page.on("pageerror", (error) => errors.push(String(error)));
          page.on("console", (message) => {
            if (message.type() === "error") errors.push(message.text());
          });
          page.on("requestfailed", (request) => errors.push(request.url()));
          const route = `${url}/w/home-owner/settings/runtimes/fixture-runtime/workdirs`;
          await page.goto(route);
          const deletion = page.getByRole("button", {
            name: "Delete retry-dir",
            exact: true,
          });
          await deletion.waitFor();
          assert(
            await deletion.isEnabled(),
            "past pending/unknown state permanently disabled ordinary removal",
          );
          assert(
            await page.getByRole("button", {
              name: "Delete dirty-dir",
              exact: true,
            }).isDisabled(),
          );
          assert(
            await page.getByRole("button", {
              name: "Delete occupied-dir",
              exact: true,
            }).isDisabled(),
          );
          await noOverflow(page);
          assert(
            await deletion.evaluate((button) => {
              const r = button.getBoundingClientRect();
              return r.left >= 0 && r.right <= innerWidth;
            }),
            "Delete is outside initial viewport",
          );
          await page.screenshot({
            path: join(evidence, `${colorScheme}-${width}-initial.png`),
          });
          // Hold the actual request boundary to verify duplicate gestures cannot dispatch another DELETE.
          let release!: () => void;
          const gate = new Promise<void>((resolve) => release = resolve);
          let observed!: () => void;
          const entered = new Promise<void>((resolve) => observed = resolve);
          let deletes = 0;
          await page.route(
            "**/working-directories/retry-dir",
            async (intercept) => {
              deletes++;
              observed();
              await gate;
              await intercept.continue();
            },
          );
          await deletion.focus();
          await page.keyboard.press("Enter");
          await page.getByRole("button", {
            name: "Delete retry-dir",
            exact: true,
          }).filter({ hasText: "Deleting…" }).waitFor();
          assert(await deletion.isDisabled());
          await page.keyboard.press("Enter");
          await entered;
          assertEquals(deletes, 1);
          await page.screenshot({
            path: join(evidence, `${colorScheme}-${width}-deleting.png`),
          });
          release();
          await page.getByRole("alert").filter({
            hasText: "mounted filesystem",
          }).waitFor();
          const retry = page.getByRole("button", {
            name: "Retry Delete retry-dir",
            exact: true,
          });
          await retry.waitFor();
          await page.waitForFunction(() =>
            !(document.querySelector(
              "button[aria-label='Retry Delete retry-dir']",
            ) as HTMLButtonElement)?.disabled
          );
          const failed = await state();
          const beforeGets = failed.requests.filter((
            r: { workspace: string; method: string },
          ) => r.workspace === "home-owner" && r.method === "GET");
          assertEquals(
            beforeGets.filter((r: { path: string }) => r.path.endsWith("/working-directories"))
              .length,
            2,
          );
          assertEquals(
            beforeGets.filter((r: { path: string }) => r.path.endsWith("/cleanup-plan")).length,
            2,
          );
          assertEquals(await page.locator("tbody tr").count(), 3);
          await noOverflow(page);
          await page.screenshot({
            path: join(evidence, `${colorScheme}-${width}-mount-failure.png`),
          });
          await control({ mountReleased: true });
          await retry.focus();
          await page.keyboard.press("Enter");
          await page.getByRole("status").filter({
            hasText: "Deletion confirmed",
          }).waitFor();
          await page.waitForFunction(() => document.querySelectorAll("tbody tr").length === 2);
          assertEquals(
            await page.getByRole("button", { name: /retry-dir/ }).count(),
            0,
          );
          assert(
            await page.getByRole("button", {
              name: "Delete dirty-dir",
              exact: true,
            }).isDisabled(),
          );
          const final = await state();
          const mutations = final.requests.filter((r: { method: string }) => r.method === "DELETE");
          assertEquals(mutations.length, 2);
          assertEquals(
            mutations[0],
            mutations[1],
            "ordinary retry changed its request",
          );
          const gets = final.requests.filter((
            r: { workspace: string; method: string },
          ) => r.workspace === "home-owner" && r.method === "GET");
          assertEquals(
            gets.filter((r: { path: string }) => r.path.endsWith("/working-directories")).length,
            3,
          );
          assertEquals(
            gets.filter((r: { path: string }) => r.path.endsWith("/cleanup-plan")).length,
            3,
          );
          await page.screenshot({
            path: join(evidence, `${colorScheme}-${width}-removed.png`),
          });
          await page.goto(
            `${url}/w/home-member/settings/runtimes/fixture-runtime/workdirs`,
          );
          await page.locator("tbody tr").first().waitFor();
          assertEquals(
            await page.getByRole("button", { name: /Delete/ }).count(),
            0,
          );
          await noOverflow(page);
          await page.screenshot({
            path: join(evidence, `${colorScheme}-${width}-member.png`),
          });
          await page.goto(
            `${url}/w/home-empty/settings/runtimes/fixture-runtime/workdirs`,
          );
          await page.getByText("No workdirs are visible for this Runtime.")
            .waitFor();
          await noOverflow(page);
          assertEquals(errors, []);
          observations.push({
            colorScheme,
            width,
            requests: final.requests,
            errors,
          });
        } finally {
          await context.close();
        }
      }
    }

    // Lost/HTTP failure responses still trigger fresh inventory and eligibility. Never echo provider bodies.
    await control({ reset: true });
    const page = await browser.newPage();
    observeInjectedFailure(page);
    await page.goto(
      `${url}/w/home-error/settings/runtimes/fixture-runtime/workdirs`,
    );
    await page.getByRole("button", { name: "Delete retry-dir", exact: true })
      .click();
    await page.getByRole("alert").filter({ hasText: "provider is unavailable" })
      .waitFor();
    await page.waitForFunction(() =>
      !(document.querySelector(
        "button[aria-label='Delete retry-dir']",
      ) as HTMLButtonElement)?.disabled
    );
    assert(
      !(await page.locator("main").innerText()).includes("Synthetic private"),
    );
    const failed = await state();
    assertEquals(
      failed.requests.filter((r: { method: string; path: string }) =>
        r.method === "GET" && r.path.endsWith("/working-directories")
      ).length,
      2,
    );
    assertEquals(
      failed.requests.filter((r: { method: string; path: string }) =>
        r.method === "GET" && r.path.endsWith("/cleanup-plan")
      ).length,
      2,
    );
    await page.screenshot({ path: join(evidence, "http-failure.png") });
    await page.close();

    // Plan refresh failure cannot leave stale delete authority enabled; manual Refresh recovers it.
    await control({ reset: true });
    const stale = await browser.newPage();
    observeInjectedFailure(stale);
    let planGets = 0;
    let planFails = true;
    await stale.route("**/cleanup-plan", async (route) => {
      planGets++;
      if (planGets > 1 && planFails) {
        await route.fulfill({
          status: 503,
          json: { message: "Synthetic private path" },
        });
      } else await route.continue();
    });
    await stale.goto(
      `${url}/w/home-owner/settings/runtimes/fixture-runtime/workdirs`,
    );
    await stale.getByRole("button", { name: "Delete retry-dir", exact: true })
      .click();
    await stale.getByRole("alert").filter({ hasText: "could not be refreshed" })
      .waitFor();
    assert(
      await stale.getByRole("button", {
        name: "Retry Delete retry-dir",
        exact: true,
      }).isDisabled(),
    );
    assert(
      !(await stale.locator("main").innerText()).includes("Synthetic private"),
    );
    await stale.screenshot({ path: join(evidence, "refresh-failure.png") });
    planFails = false;
    await stale.getByRole("button", { name: "Refresh", exact: true }).click();
    await stale.waitForFunction(() =>
      !(document.querySelector(
        "button[aria-label='Retry Delete retry-dir']",
      ) as HTMLButtonElement)?.disabled
    );
    await stale.close();

    // Current dirty changes after a failure override a retryable historical result.
    await control({ reset: true });
    const changed = await browser.newPage();
    let changedGets = 0;
    await changed.route("**/cleanup-plan", async (route) => {
      const response = await route.fetch();
      const plan = await response.json();
      if (++changedGets > 1) plan.workdirs[0].action = "workdir_dirty_discard";
      await route.fulfill({ response, json: plan });
    });
    await changed.goto(
      `${url}/w/home-owner/settings/runtimes/fixture-runtime/workdirs`,
    );
    await changed.getByRole("button", { name: "Delete retry-dir", exact: true })
      .click();
    await changed.getByRole("button", {
      name: "Retry Delete retry-dir",
      exact: true,
    }).waitFor();
    await changed.waitForFunction(() =>
      document.querySelector(".workdir-identity .removal-guard")?.textContent
        ?.includes("Changes are protected")
    );
    assert(
      await changed.getByRole("button", {
        name: "Retry Delete retry-dir",
        exact: true,
      }).isDisabled(),
    );
    assertStringIncludes(
      await changed.locator("main").innerText(),
      "Changes are protected",
    );
    await changed.screenshot({
      path: join(evidence, "new-changes-protected.png"),
    });
    await changed.close();
    // A malformed response may follow a real side effect. Inventory still removes the now-absent row.
    await control({ reset: true, mountReleased: true });
    const lost = await browser.newPage();
    observeInjectedFailure(lost);
    await lost.route("**/working-directories/retry-dir", async (route) => {
      await route.fetch();
      await route.fulfill({
        status: 200,
        json: {
          message: "Synthetic private path",
          failure_category: "/private/path token=synthetic",
        },
      });
    });
    await lost.goto(
      `${url}/w/home-owner/settings/runtimes/fixture-runtime/workdirs`,
    );
    await lost.getByRole("button", { name: "Delete retry-dir", exact: true })
      .click();
    await lost.getByRole("alert").filter({
      hasText: "Removal could not be confirmed",
    }).waitFor();
    await lost.waitForFunction(() => document.querySelectorAll("tbody tr").length === 2);
    assert(
      !(await lost.locator("main").innerText()).includes("Synthetic private"),
    );
    assert(!(await lost.locator("main").innerText()).includes("/private/path"));
    await lost.screenshot({
      path: join(evidence, "malformed-response-refreshed.png"),
    });
    await lost.close();

    // Even a failed initial inventory can recover into a confirmed empty list with manual Refresh.
    const emptyRecovery = await browser.newPage();
    observeInjectedFailure(emptyRecovery);
    let inventoryFails = true;
    await emptyRecovery.route(
      "**/runtimes/fixture-runtime/working-directories",
      async (route) => {
        if (inventoryFails) {
          await route.fulfill({
            status: 503,
            json: { message: "Synthetic private path" },
          });
        } else await route.continue();
      },
    );
    await emptyRecovery.goto(
      `${url}/w/home-empty/settings/runtimes/fixture-runtime/workdirs`,
    );
    await emptyRecovery.getByText("Workdir inventory is unavailable.")
      .waitFor();
    inventoryFails = false;
    await emptyRecovery.getByRole("button", { name: "Refresh", exact: true })
      .click();
    await emptyRecovery.getByText("No workdirs are visible for this Runtime.")
      .waitFor();
    await emptyRecovery.close();
    assertEquals(injectedUnexpectedErrors, []);
    assert(
      injectedConsoleErrors.every((message) => message.includes("503")),
      "unexpected injected failure console diagnostic",
    );
    await Deno.writeTextFile(
      join(evidence, "evidence.json"),
      JSON.stringify(
        {
          observations,
          injectedConsoleErrors,
          injectedUnexpectedErrors,
          failureRefresh: failed.requests,
          refreshFailureRecovered: true,
          newChangesProtected: true,
          malformedResponseReconciled: true,
          emptyInventoryRecovered: true,
          expectedFailureDiagnostics: [
            "Synthetic DELETE HTTP 503 on home-error",
            "Synthetic cleanup-plan HTTP 503 after removal",
            "Malformed DELETE result after fixture physical removal",
            "Synthetic initial inventory HTTP 503",
          ],
        },
        null,
        2,
      ),
    );
  } finally {
    await browser.close();
    assertEquals(await stopOwnedProcesses(server), []);
  }
});
