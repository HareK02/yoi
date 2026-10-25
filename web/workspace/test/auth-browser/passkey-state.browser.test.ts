/// <reference lib="dom" />
// 本番shellと可視状態の試験。HTTP/認証器を合成fixtureへ置換するため、暗号/保存の成功根拠ではない。
// native CTAP2 + 実Backend経路はtests/e2eの別suiteが所有する。
import { assert, assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { type Browser, chromium } from "playwright";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";

Deno.test("account addition keeps pending and recovery visible at desktop and mobile widths", async (t) => {
  const root = resolve(import.meta.dirname!, "../../../..");
  const reservation = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (reservation.addr as Deno.NetAddr).port;
  reservation.close();
  const origin = `http://127.0.0.1:${port}`;
  const out = `${root}/target/web-ux/t783-after`;
  const processes = await startOwnedProcesses(
    [{
      id: "passkey-state",
      command: "deno",
      cwd: root,
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-read",
        "--allow-net",
        "web/workspace/test/auth-browser/fixture-server.ts",
        String(port),
        "web/workspace/build",
      ],
      readyUrl: `${origin}/health`,
    }],
    `${root}/tools/web-ux/scenarios/anonymous-entry.json`,
    `${out}/processes`,
    [],
  );
  let browser: Browser | undefined;
  const records: unknown[] = [];
  try {
    browser = await chromium.launch();
    for (const theme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const passed = await t.step(
          `${theme}/${width}: visible pending, stop and migration metadata`,
          async () => {
            const context = await browser!.newContext({
              viewport: { width, height: 900 },
              colorScheme: theme,
              reducedMotion: "reduce",
            });
            try {
              const page = await context.newPage();
              page.setDefaultTimeout(5000);
              const pageErrors: string[] = [];
              const statuses: number[] = [];
              const consoleErrors: string[] = [];
              const requestErrors: string[] = [];
              page.on(
                "requestfailed",
                () => requestErrors.push("request-failed"),
              );
              page.on("pageerror", () => pageErrors.push("page-error"));
              page.on("console", (msg) => {
                if (msg.type() === "error") {
                  consoleErrors.push("console-error");
                }
              });
              page.on("response", (response) => {
                if (response.status() >= 400) statuses.push(response.status());
              });
              await context.route(
                "**/*",
                (route) =>
                  new URL(route.request().url()).origin === origin
                    ? route.continue()
                    : route.abort(),
              );
              await page.request.get(`${origin}/?fixture=addition-pending`);
              const documentResponse = await page.goto(`${origin}/account`);
              const add = page.getByRole("button", {
                name: "Add passkey to alice",
                exact: true,
              });
              await add.waitFor();
              await page.getByRole("list", { name: "Registered keys" })
                .waitFor();
              await page.screenshot({
                path: `${out}/account-${theme}-${width}.png`,
                fullPage: true,
              });
              await add.click();
              const pending = page.getByRole("status").filter({
                hasText: "Waiting for passkey addition",
              });
              await pending.waitFor({ state: "visible" });
              const box = await pending.boundingBox();
              assert(
                box && box.y >= 0 && box.y + box.height <= 900,
                "pending must be in the initial visible viewport",
              );
              assertEquals(await add.isDisabled(), true);
              assertEquals(
                await page.getByText("Loading session…", { exact: true })
                  .isVisible(),
                false,
              );
              await page.screenshot({
                path: `${out}/pending-${theme}-${width}.png`,
                fullPage: true,
              });
              const pendingAccessibility = await page.locator("main")
                .ariaSnapshot();
              await page.getByRole("button", {
                name: "Stop waiting",
                exact: true,
              }).click();
              await page.evaluate(() =>
                (window as unknown as { __authFixtureRelease: () => void })
                  .__authFixtureRelease()
              );
              await page.getByRole("alert").filter({
                hasText: "Completion was not sent",
              }).waitFor();
              await page.getByRole("button", {
                name: "Refresh registered keys",
                exact: true,
              }).click();
              await page.getByText(
                "Not yet verified — storage alone does not prove support",
                { exact: true },
              ).waitFor();
              await page.getByText(
                "Verify migration before retiring older-key login",
                { exact: true },
              ).click();
              await page.screenshot({
                path: `${out}/migration-${theme}-${width}.png`,
                fullPage: true,
              });
              assertEquals(
                await page.evaluate(() =>
                  document.documentElement.scrollWidth > innerWidth
                ),
                false,
              );
              assertEquals(documentResponse?.status(), 200);
              assertEquals(pageErrors, []);
              assertEquals(consoleErrors, []);
              assertEquals(statuses, []);
              assertEquals(requestErrors, []);
              const images = [];
              for (
                const name of [
                  `account-${theme}-${width}.png`,
                  `pending-${theme}-${width}.png`,
                  `migration-${theme}-${width}.png`,
                ]
              ) {
                const digest = await crypto.subtle.digest(
                  "SHA-256",
                  await Deno.readFile(`${out}/${name}`),
                );
                images.push({
                  name,
                  sha256: Array.from(
                    new Uint8Array(digest),
                    (byte) => byte.toString(16).padStart(2, "0"),
                  ).join(""),
                });
              }
              records.push({
                route: "/account",
                actor: "synthetic Alice",
                theme,
                width,
                documentStatus: documentResponse?.status(),
                pageErrors,
                consoleErrors,
                requestErrors,
                statuses,
                pendingAccessibility,
                visible: await page.locator("main").innerText(),
                images,
              });
            } finally {
              await context.close();
            }
          },
        );
        if (!passed) {
          throw new Error(
            "Stop on the first failed visibility condition; investigate before repeating the matrix",
          );
        }
      }
    }
  } finally {
    await browser?.close();
    await stopOwnedProcesses(processes);
    await Deno.writeTextFile(
      `${out}/review-context.json`,
      JSON.stringify(records, null, 2),
    );
  }
});
