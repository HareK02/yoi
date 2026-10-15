/// <reference lib="dom" />
// UI projectionのみ。実APIのBearer認可・WebAuthn・cookie発行は検証しない。
// requestを観測する場合もmethod/pathだけとし、headers/bodyは取得・保存しない。
import { assert, assertEquals } from "@std/assert";
import { resolve } from "@std/path";
import { chromium, type Page } from "playwright";
import { startOwnedProcesses, stopOwnedProcesses } from "../../../../tools/web-ux/src/processes.ts";
const root = resolve(import.meta.dirname!, "../../../..");
const evidence = `${root}/target/web-ux/hosting/setup-interactions`;
async function noOverflow(page: Page) {
  assertEquals(
    await page.evaluate(() => ({
      page: document.documentElement.scrollWidth > innerWidth,
      main: [...document.querySelectorAll("main")].some((main) =>
        main.scrollWidth > main.clientWidth
      ),
    })),
    { page: false, main: false },
  );
}
Deno.test("setup UI separates masked host-token enrollment, rejected attempts, explicit login and verified settings navigation", async () => {
  const processes = await startOwnedProcesses(
    [{
      id: "setup-interactions",
      command: "deno",
      cwd: root,
      args: [
        "run",
        "--config",
        "tools/web-ux/deno.json",
        "--allow-net",
        "--allow-read",
        "web/workspace/test/hosting-browser/fixture-server.ts",
        "15380",
        "web/workspace/build",
      ],
      readyUrl: "http://127.0.0.1:15380/health",
      readyTimeoutMs: 10000,
    }],
    `${root}/tools/web-ux/scenarios/hosting-setup-light.json`,
    evidence,
    [],
  );
  const browser = await chromium.launch();
  const results = [];
  try {
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme,
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          const pageErrors: string[] = [],
            consoleErrors: string[] = [],
            httpErrors: string[] = [],
            requestFailures: string[] = [];
          const methodsAndPaths: string[] = [];
          page.on("pageerror", (error) => pageErrors.push(String(error)));
          page.on("console", (message) => {
            if (message.type() === "error") consoleErrors.push(message.text());
          });
          page.on("response", (response) => {
            if (response.status() >= 400) {
              httpErrors.push(`${response.status()} ${new URL(response.url()).pathname}`);
            }
          });
          page.on(
            "requestfailed",
            (request) => requestFailures.push(new URL(request.url()).pathname),
          );
          page.on(
            "request",
            (request) =>
              methodsAndPaths.push(`${request.method()} ${new URL(request.url()).pathname}`),
          );
          await page.goto("http://127.0.0.1:15380/hosting/setup?fixture=setup-flow");
          const input = page.locator("#host-setup-token");
          await input.waitFor();
          const enroll = page.getByRole("button", {
            name: "Add administrator passkey",
            exact: true,
          });
          assertEquals(await input.getAttribute("type"), "password");
          assertEquals(await enroll.isDisabled(), true);
          assert(
            !methodsAndPaths.some((entry) => entry.startsWith("POST ")),
          );
          assertEquals(
            await page.getByRole("link", { name: "Open hosting settings", exact: true }).count(),
            0,
          );
          await noOverflow(page);
          await page.locator("summary").focus();
          await page.keyboard.press("Enter");
          await page.locator("details[open]").waitFor();
          await noOverflow(page);
          await page.keyboard.press("Enter");
          const synthetic = `ui-projection-${crypto.randomUUID()}`;
          await input.fill(synthetic);
          assertEquals(await enroll.isEnabled(), true);
          await input.focus();
          await page.keyboard.press("Tab");
          assertEquals(
            await enroll.evaluate((element) => element === document.activeElement),
            true,
          );
          assertEquals(
            await enroll.evaluate((element) => getComputedStyle(element).outlineStyle !== "none"),
            true,
          );
          await page.keyboard.press("Enter");
          const login = page.getByRole("button", { name: "Sign in with new passkey", exact: true });
          await login.waitFor();
          await page.getByRole("status").filter({ hasText: "Enrollment is not a passkey login." })
            .waitFor();
          assertEquals(await input.count(), 0);
          assertEquals(
            await page.getByRole("link", { name: "Open hosting settings", exact: true }).count(),
            0,
          );
          assertEquals(
            methodsAndPaths.filter((entry) => entry.startsWith("POST ")),
            [
              "POST /api/auth/passkeys/addition/options",
              "POST /api/auth/passkeys/addition/complete",
            ],
          );
          await noOverflow(page);
          await login.focus();
          await page.keyboard.press("Enter");
          const settings = page.getByRole("link", { name: "Open hosting settings", exact: true });
          await settings.waitFor();
          await page.getByRole("status").filter({
            hasText: "Passkey login and hosting administrator access verified.",
          }).waitFor();
          await noOverflow(page);
          await settings.focus();
          await page.keyboard.press("Enter");
          await page.getByRole("button", { name: "Edit registration policy", exact: true })
            .waitFor();
          await noOverflow(page);
          await page.goto("http://127.0.0.1:15380/hosting/setup?fixture=setup-reject");
          await input.fill(synthetic);
          await enroll.click();
          const error = page.getByRole("alert");
          await error.waitFor();
          assert(!(await input.inputValue()));
          assertEquals(await enroll.isDisabled(), true);
          assert(!(await error.innerText()).includes(synthetic));
          assertEquals(await settings.count(), 0);
          await noOverflow(page);
          assertEquals(pageErrors, []);
          assertEquals(httpErrors, ["403 /api/auth/passkeys/addition/options"]);
          assertEquals(consoleErrors.length, 1);
          assert(consoleErrors[0].includes("403"));
          assert(requestFailures.every((path) => path === "/api/auth/passkeys/addition/options"));
          results.push({
            width,
            theme: colorScheme,
            emptyDisabled: true,
            passwordMasked: true,
            keyboard: "pass",
            tokenClearedAfterRejectedAttempt: true,
            enrollmentNotLogin: true,
            explicitLoginBeforeSettings: true,
            overflow: "none",
            unexpectedErrors: 0,
            requestHeadersOrBodiesRecorded: false,
            realAuthenticationTested: false,
          });
        } finally {
          await context.close();
        }
      }
    }
    await Deno.writeTextFile(`${evidence}/results.json`, JSON.stringify(results, null, 2));
  } finally {
    await browser.close();
    await stopOwnedProcesses(processes);
  }
});
