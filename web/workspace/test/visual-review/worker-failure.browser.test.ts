import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import { loadScenario } from "../../../../tools/web-ux/src/scenario.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
import { startupFixture } from "../../src/lib/workspace/workers/startup-failure.test-fixtures.ts";
import { verifyStartupHttpCaptures } from "../worker-startup-backend-contract.ts";

// production build + exact T812 HTTP fixtures。派生故障・操作結果はWeb試験用でありServer保証ではない。
// beforeは旧production buildで同fixtureを撮影するためだけに明示選択し、afterの成功と混同しない。
Deno.test("Worker failure cause, independent outcomes, keyboard details and safe reconciliation", async () => {
  const before = Deno.env.get("T813_PHASE") === "before";
  const captureInput = Deno.env.get("T813_HTTP_CAPTURE_LOG");
  const captureLog = captureInput
    ? await Deno.readTextFile(captureInput)
    : null;
  const captureSha256 = captureLog
    ? Array.from(
      new Uint8Array(
        await crypto.subtle.digest(
          "SHA-256",
          new TextEncoder().encode(captureLog),
        ),
      ),
    ).map((b) => b.toString(16).padStart(2, "0")).join("")
    : null;
  const captures = captureLog
    ? (await verifyStartupHttpCaptures(captureLog)).captures
    : null;
  function fixture(name: Parameters<typeof startupFixture>[0]) {
    if (captures && (name === "denial" || name === "lost")) {
      const capture = captures.find((item) =>
        item.status === (name === "denial" ? 409 : 502)
      );
      assert(capture, "Expected both validated real HTTP captures");
      return JSON.parse(capture.body);
    }
    return startupFixture(name);
  }
  const phase = before
    ? "before-real-contract"
    : captureInput
    ? "after-connected"
    : "after";
  const scenarioPath =
    new URL("./optional-repository.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  function freePort() {
    const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
    const port = (listener.addr as Deno.NetAddr).port;
    listener.close();
    return port;
  }
  const upstream = freePort();
  let proxy = freePort();
  while (proxy === upstream) proxy = freePort();
  scenario.baseUrl = `http://127.0.0.1:${proxy}`;
  scenario.processes![0].args![4] = String(upstream);
  scenario.processes![0].readyUrl = `http://127.0.0.1:${upstream}/health`;
  scenario.processes![1].args![3] = String(proxy);
  scenario.processes![1].args![4] = `http://127.0.0.1:${upstream}`;
  scenario.processes![1].readyUrl = `${scenario.baseUrl}/health`;
  const output =
    new URL(`../../../../target/web-ux/t813/${phase}/`, import.meta.url)
      .pathname;
  await Deno.mkdir(output, { recursive: true });
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    `${output}/process-logs`,
    [],
  );
  const records: unknown[] = [];
  try {
    const browser = await chromium.launch({ headless: true });
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
            let name = "denial";
            let submissions = 0;
            const errors: string[] = [];
            const failedRequests: {
              method: string;
              path: string;
              error: string | undefined;
              scenario: string | undefined;
            }[] = [];
            const requestScenarios = new WeakMap<object, string>();
            const unexpectedFetches: string[] = [];
            page.on("pageerror", (error) => errors.push(String(error)));
            page.on("console", (message) => {
              if (message.type() === "error") errors.push(message.text());
            });
            page.on(
              "requestfailed",
              (request) =>
                failedRequests.push({
                  method: request.method(),
                  path: new URL(request.url()).pathname,
                  error: request.failure()?.errorText,
                  scenario: requestScenarios.get(request),
                }),
            );
            page.on("request", (request) => {
              requestScenarios.set(request, name);
              if (request.url().includes("should-not-fetch")) {
                unexpectedFetches.push(request.url());
              }
            });
            // Existing shared fixture predates these required fields; adapt only fixture data, not authorization.
            await page.route(
              "**/api/auth/whoami",
              (route) =>
                route.fulfill({
                  json: {
                    session: {
                      kind: "authenticated",
                      expires_at: "2099-01-01T00:00:00Z",
                    },
                    authentication_reference: "fixture-authentication",
                    actor: {
                      user_id: "fixture-user",
                      account_id: "fixture-owner",
                      handle: "alice",
                      display_name: "Alice",
                      auth_method: "browser_session",
                    },
                  },
                }),
            );
            await page.route("**/api/w/home-empty/workspace", async (route) => {
              const response = await route.fetch();
              const value = await response.json();
              await route.fulfill({
                json: {
                  ...value,
                  permissions: { ...value.permissions, delete_subjects: true },
                },
              });
            });
            let body = JSON.parse(JSON.stringify(fixture("denial")));
            await page.route("**/api/w/home-empty/workers", async (route) => {
              if (route.request().method() !== "POST") return route.continue();
              submissions++;
              if (name === "transport") return route.abort("failed");
              await route.fulfill({
                status: body.status,
                contentType: "application/problem+json",
                body: name === "invalid"
                  ? "raw /private/secret token=credential"
                  : name === "oversize"
                  ? "secret".repeat(6000)
                  : JSON.stringify(body),
              });
            });
            for (
              name of before ? ["denial", "compensation"] : [
                "denial",
                "lost",
                "rejected",
                "compensation",
                "long",
                "unsupported",
                "invalid",
                "oversize",
                "transport",
              ]
            ) {
              body = JSON.parse(
                JSON.stringify(fixture(
                  name === "lost"
                    ? "lost"
                    : name === "rejected"
                    ? "rejected"
                    : name === "compensation"
                    ? "compensation"
                    : "denial",
                )),
              );
              if (name === "unsupported") body.yoi_startup.version = 2;
              if (name === "long") {
                body.detail =
                  body.yoi_failure.explanation =
                    "Subject lacks session.record; Session registration and Worker creation could not complete. "
                      .repeat(12) +
                    "<img src='https://should-not-fetch.test/private' onerror='alert(1)'>";
                body.type = "https://should-not-fetch.test/type";
                body.instance = "https://should-not-fetch.test/instance";
                body.future_extension = {
                  raw_source: "/private/secret token=credential",
                };
              }
              const documentResponse = await page.goto(
                `${scenario.baseUrl}/w/home-empty/workers/new`,
              );
              await page.locator(".worker-launch-form").waitFor({
                timeout: 10000,
              });
              await page.getByLabel("Initial request").fill(
                "Preserve this draft request",
              );
              const start = page.getByRole("button", {
                name: "Start Worker",
                exact: true,
              });
              const previousSubmissions = submissions;
              await start.click();
              const notice = page.locator(
                before ? ".worker-submit-error" : ".failure-notice",
              );
              await notice.waitFor({ timeout: 10000 });
              assertEquals(submissions, previousSubmissions + 1);
              if (!before) {
                assertEquals(
                  await page.getByLabel("Initial request").inputValue(),
                  "Preserve this draft request",
                );
                assertEquals(await start.isDisabled(), name !== "rejected");
                assertEquals(
                  await page.getByRole("link", { name: "Cancel", exact: true })
                    .count(),
                  1,
                );
                if (
                  ["denial", "lost", "rejected", "compensation", "long"]
                    .includes(name)
                ) {
                  const mainText = await notice.locator(
                    ".failure-notice__cause",
                  ).innerText();
                  assert(mainText.includes(body.detail));
                  assertEquals(
                    (await notice.innerText()).split(body.detail).length,
                    2,
                  );
                  assertEquals(
                    await notice.locator("details").getAttribute("open"),
                    null,
                  );
                  assertEquals(
                    (await notice.locator("details").innerText()).includes(
                      body.detail,
                    ),
                    false,
                  );
                  const summary = notice.locator("summary");
                  await summary.focus();
                  await page.keyboard.press("Enter");
                  assert(
                    await notice.locator("details").evaluate((node) =>
                      (node as unknown as { open: boolean }).open
                    ),
                  );
                  assert(
                    (await notice.locator("details").innerText()).includes(
                      "Support correlation",
                    ),
                  );
                  await page.keyboard.press("Enter");
                }
                assertEquals(await notice.locator("img, a").count(), 0);
                const text = await page.locator("main").innerText();
                assert(!text.includes("/private/secret"));
                assert(!text.includes("token=credential"));
                if (name === "compensation") {
                  assert(text.includes("Backend cleanup: residual."));
                }
                if (name !== "rejected") {
                  assertEquals(
                    await page.getByRole("link", { name: "Check Workers" })
                      .count(),
                    1,
                  );
                }
              }
              await notice.scrollIntoViewIfNeeded();
              assertEquals(
                await page.evaluate(
                  "document.documentElement.scrollWidth > innerWidth",
                ),
                false,
                `${name} ${width}`,
              );
              const path = `${output}/${name}-${colorScheme}-${width}.png`;
              await page.screenshot({ path, fullPage: true });
              const extraCaptures: { path: string; sha256: string }[] = [];
              // Expanded contents and reconciliation controls live in the shell's scrolling main.
              // Scroll those capture points explicitly; fullPage alone only expands the document.
              if (!before && name === "denial") {
                await notice.locator("summary").focus();
                await page.keyboard.press("Space");
                await notice.locator("dl").scrollIntoViewIfNeeded();
                const detailsPath =
                  `${output}/details-${colorScheme}-${width}.png`;
                await page.screenshot({ path: detailsPath, fullPage: true });
                await notice.locator("summary").focus();
                await page.keyboard.press("Enter");
                const actionsPath =
                  `${output}/reconciliation-${colorScheme}-${width}.png`;
                await page.getByRole("link", { name: "Check Workers" })
                  .scrollIntoViewIfNeeded();
                await page.screenshot({ path: actionsPath, fullPage: true });
                for (const extraPath of [detailsPath, actionsPath]) {
                  const bytes = await Deno.readFile(extraPath);
                  extraCaptures.push({
                    path: extraPath,
                    sha256: Array.from(
                      new Uint8Array(
                        await crypto.subtle.digest("SHA-256", bytes),
                      ),
                    ).map((b) => b.toString(16).padStart(2, "0")).join(""),
                  });
                }
              }
              const image = await Deno.readFile(path);
              records.push({
                route: "/w/home-empty/workers/new",
                persona: "synthetic owner",
                name,
                captureInput,
                captureSha256,
                fixture: ["denial", "lost"].includes(name)
                  ? captureInput
                    ? "forwarded exact T812 corrected-source HTTP capture"
                    : "T812 exact real HTTP body"
                  : "synthetic derivative/fault",
                width,
                theme: colorScheme,
                phase,
                documentStatus: documentResponse?.status(),
                expectedApiFailure: name === "transport"
                  ? "network abort"
                  : body.status,
                visibleError: await notice.innerText(),
                errors: [...errors],
                failedRequests: [...failedRequests],
                accessibility: await page.locator("main").ariaSnapshot(),
                image: path,
                extraCaptures,
                sha256: Array.from(
                  new Uint8Array(await crypto.subtle.digest("SHA-256", image)),
                ).map((b) => b.toString(16).padStart(2, "0")).join(""),
              });
            }
            assertEquals(unexpectedFetches, []);
            // Browser logs expected injected HTTP and transport failures; page JS errors are not allowed.
            assertEquals(
              errors.filter((error) =>
                !error.includes("Failed to load resource")
              ),
              [],
            );
            assert(
              failedRequests.every((request) =>
                request.path === "/api/w/home-empty/workers" &&
                ((request.method === "POST" &&
                  request.scenario === "transport" &&
                  request.error === "net::ERR_FAILED") ||
                  (request.method === "POST" &&
                    request.scenario === "oversize" &&
                    request.error === "net::ERR_ABORTED") ||
                  (request.method === "GET" &&
                    request.error === "net::ERR_ABORTED"))
              ),
              JSON.stringify(failedRequests),
            );
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
      `${output}/review-context.json`,
      JSON.stringify(records, null, 2),
    );
    assertEquals(await stopOwnedProcesses(processes), []);
  }
});
