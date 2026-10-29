import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import { loadScenario } from "../../../../tools/web-ux/src/scenario.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";

// 実production buildのnative selectとloopback fixtureだけを検証する。
// 実Backend認可、Worker起動、通知配送の成功とは区別する。
Deno.test("Subject selection, paging, audit, permission failure and exact retry without singleton startup", async () => {
  const scenarioPath =
    new URL("./subject-handling-light.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  function freePort() {
    const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
    const port = (listener.addr as Deno.NetAddr).port;
    listener.close();
    return port;
  }
  const upstream = freePort();
  let proxy = freePort();
  while (upstream === proxy) proxy = freePort();
  scenario.processes![0].args![4] = String(upstream);
  scenario.processes![0].readyUrl = `http://127.0.0.1:${upstream}/health`;
  scenario.processes![1].args![3] = String(proxy);
  scenario.processes![1].args![4] = `http://127.0.0.1:${upstream}`;
  scenario.processes![1].readyUrl = `http://127.0.0.1:${proxy}/health`;
  const base = `http://127.0.0.1:${proxy}`;
  const output = new URL(
    "../../../../target/web-ux/t778-validation/browser/",
    import.meta.url,
  ).pathname;
  await Deno.mkdir(output, { recursive: true });
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    output + "/process-logs",
    [],
  );
  const browser = await chromium.launch({ headless: true });
  const captures: unknown[] = [];
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
          let documentStatus: number | null = null;
          page.on("response", (response) => {
            if (response.request().isNavigationRequest()) {
              documentStatus = response.status();
            }
          });
          const errors: string[] = [];
          const pageErrors: string[] = [];
          const requestFailures: string[] = [];
          page.on("requestfailed", (request) => {
            requestFailures.push(
              `${request.method()} ${
                new URL(request.url()).pathname
              }: ${request.failure()?.errorText}`,
            );
          });
          async function captureState(state: string, expectedFailure = false) {
            assertEquals(pageErrors, []);
            const image = `${colorScheme}-${width}-${state}.png`;
            const bytes = await page.screenshot({
              path: output + image,
              fullPage: true,
            });
            const sha256 = Array.from(
              new Uint8Array(
                await crypto.subtle.digest("SHA-256", new Uint8Array(bytes)),
              ),
            ).map((byte) => byte.toString(16).padStart(2, "0")).join("");
            captures.push({
              image,
              sha256,
              route: page.url(),
              documentStatus,
              colorScheme,
              width,
              expectedFailure,
              errors: [...errors],
              pageErrors: [...pageErrors],
              requestFailures: [...requestFailures],
              horizontalOverflow: await page.evaluate(
                "document.documentElement.scrollWidth > innerWidth",
              ),
              snapshot: await page.locator("main").first().ariaSnapshot(),
            });
          }
          const paths: string[] = [];
          page.on("pageerror", (error) => pageErrors.push(String(error)));
          page.on("console", (message) => {
            if (message.type() === "error") errors.push(message.text());
          });
          page.on("request", (request) => {
            paths.push(new URL(request.url()).pathname);
          });
          await page.goto(base + "/w/tickets/tickets/T-2");
          await page.getByText("Self-reported handling: waiting", {
            exact: true,
          }).waitFor();
          await page.getByText("Choose responsible Subject", { exact: true })
            .click();
          const select = page.getByRole("combobox", {
            name: "Subject",
            exact: true,
          });
          await select.selectOption("subject-a");
          await page.getByRole("button", { name: "More Subjects", exact: true })
            .click();
          await page.locator('option[value="subject-b"]').waitFor({
            state: "attached",
          });
          assertEquals(
            await select.locator('option[value="subject-a"]').count(),
            1,
          );
          assert(
            await select.locator('option[value="subject-retired"]').evaluate((
              option,
            ) => (option as unknown as { disabled: boolean }).disabled),
            await select.innerHTML(),
          );
          // Native select remains keyboard operable after paging.
          await select.focus();
          await select.press("Home");
          await select.press("ArrowDown");
          assertEquals(await select.inputValue(), "subject-a");
          await select.press("ArrowDown");
          assertEquals(await select.inputValue(), "subject-b");
          assert(
            await page.evaluate("document.activeElement?.tagName === 'SELECT'"),
          );
          await select.scrollIntoViewIfNeeded();
          await captureState("selection");
          assertEquals(
            await page.evaluate(
              "document.documentElement.scrollWidth > innerWidth",
            ),
            false,
          );
          assertEquals(errors, []);
          assertEquals(requestFailures, []);
          const sent: string[] = [];
          await page.route("**/assignments/orchestrator", async (route) => {
            if (route.request().method() === "PUT") {
              sent.push(route.request().postData()!);
              await route.abort("failed");
            } else await route.continue();
          });
          await page.getByText("Replace this delegation", { exact: true })
            .click();
          page.once("dialog", (dialog) => dialog.accept());
          await page.getByRole("button", {
            name: "Replace assignment",
            exact: true,
          }).click();
          await page.getByRole("alert").filter({ hasText: "Outcome unknown" })
            .waitFor();
          assertEquals(sent.length, 1);
          const input = JSON.parse(sent[0]);
          assertEquals(input.principal, {
            kind: "subject",
            subject_id: "subject-b",
          });
          assertEquals(input.expected_assignment_id, "assignment-T-2");
          assert(await select.isDisabled());
          await page.getByRole("alert").filter({ hasText: "Outcome unknown" })
            .scrollIntoViewIfNeeded();
          await captureState("unknown", true);
          assertEquals(errors, ["Failed to load resource: net::ERR_FAILED"]);
          assertEquals(requestFailures.length, 1);
          assert(requestFailures[0].startsWith("PUT /api/w/tickets/tickets/"));
          assert(
            requestFailures[0].endsWith(
              "/assignments/orchestrator: net::ERR_FAILED",
            ),
          );
          const failed = page.waitForEvent(
            "requestfailed",
            (r) =>
              r.method() === "PUT" &&
              r.url().endsWith("/assignments/orchestrator"),
          );
          await page.getByRole("button", {
            name: "Retry exact operation",
            exact: true,
          }).click();
          await failed;
          assertEquals(sent.length, 2);
          assertEquals(sent[1], sent[0]);
          assertEquals(
            paths.filter((path) => /\/api\/w\/[^/]+\/orchestrator/.test(path)),
            [],
          );
          await page.unroute("**/assignments/orchestrator");
          for (const persona of ["legacy", "readonly", "empty", "denied"]) {
            errors.length = 0;
            requestFailures.length = 0;
            await page.goto(`${base}/w/tickets-${persona}/tickets/T-2`);
            await page.locator(".ticket-handling").waitFor();
            if (persona === "legacy") {
              await page.getByText(/Legacy assignment retained for audit only/)
                .scrollIntoViewIfNeeded();
              assertEquals(
                await page.getByRole("textbox", {
                  name: "Answer / continue this handling",
                }).count(),
                0,
              );
            } else if (persona === "readonly") {
              assertEquals(
                await page.getByText("Choose responsible Subject", {
                  exact: true,
                }).count(),
                0,
              );
            } else {
              await page.getByText("Choose responsible Subject", {
                exact: true,
              }).click();
              if (persona === "empty") {
                await page.getByText(/No Subjects available/).waitFor();
              } else {await page.getByRole("button", { name: "Retry Subjects" })
                  .waitFor();}
              await page.getByText("Replace this delegation", { exact: true })
                .click();
              assert(
                await page.getByRole("button", {
                  name: "Replace assignment",
                  exact: true,
                }).isDisabled(),
              );
              await page.getByRole("button", {
                name: "Replace assignment",
                exact: true,
              }).scrollIntoViewIfNeeded();
            }
            if (persona === "readonly") {
              await page.locator(".ticket-handling").scrollIntoViewIfNeeded();
            }
            await captureState(persona, persona === "denied");
            assertEquals(
              await page.evaluate(
                "document.documentElement.scrollWidth > innerWidth",
              ),
              false,
            );
            if (persona !== "denied") {
              assertEquals(errors, []);
              assertEquals(requestFailures, []);
            } else {
              assertEquals(errors, [
                "Failed to load resource: the server responded with a status of 403 (Forbidden)",
              ]);
              assertEquals(requestFailures, [
                "GET /api/w/tickets-denied/subjektiv/subjects: net::ERR_ABORTED",
              ]);
            }
          }
        } finally {
          await context.close();
        }
      }
    }
  } finally {
    await Deno.writeTextFile(
      output + "/review-context.json",
      JSON.stringify(
        {
          source: "production Web build, synthetic API boundary only",
          captures,
        },
        null,
        2,
      ),
    );
    await browser.close();
    await stopOwnedProcesses(processes);
  }
});
