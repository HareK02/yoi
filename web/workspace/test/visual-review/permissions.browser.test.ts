/// <reference lib="dom" />
import { assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
import { freePort } from "../../../../tools/web-ux/browser-tests/memory_document_harness.ts";
import { sha256File } from "../../../../tools/web-ux/src/artifacts.ts";
import { permissionSource } from "../permissions-fixture.ts";

// Actual built UI, synthetic HTTP only. Does not test Backend authentication,
// persistence, resolver or real Worker effects. Run -- --before on the baseline build.
Deno.test("Subject permissions source / catalog responsive visual review", async () => {
  const before = Deno.args.includes("--before");
  const root = new URL("../../../../", import.meta.url).pathname;
  const output = `${root}web/workspace/.svelte-kit/t837-visual/${
    before ? "before" : "after-atomic-source"
  }`;
  await Deno.mkdir(output, { recursive: true });
  const port = await freePort();
  const processes = await startOwnedProcesses(
    [{
      id: "permissions-fixture",
      command: Deno.execPath(),
      cwd: root,
      args: [
        "run",
        "-A",
        "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
        String(port),
        `${root}web/workspace/build`,
        "3",
        "permissions",
      ],
      readyUrl: `http://127.0.0.1:${port}/health`,
    }],
    `${output}/scenario.json`,
    output,
    [],
  );
  const browser = await chromium.launch({ headless: true });
  const captures: unknown[] = [];
  const matrix: { state: string; theme: "light" | "dark"; width: number }[] =
    [];
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      matrix.push({ state: "owner", theme, width });
    }
  }
  if (!before) {
    matrix.push(
      { state: "draft", theme: "light", width: 390 },
      { state: "empty", theme: "dark", width: 768 },
      { state: "non-owner", theme: "light", width: 390 },
      { state: "anonymous", theme: "dark", width: 1440 },
      { state: "source-error", theme: "dark", width: 390 },
      { state: "missing-resolution", theme: "light", width: 768 },
      { state: "loading", theme: "dark", width: 390 },
    );
  }
  try {
    for (const { state, theme, width } of matrix) {
      const page = await browser.newPage({
        viewport: { width, height: 900 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const errors: string[] = [];
      const permissionRequests: { path: string; method: string }[] = [];
      page.on("request", (request) => {
        const path = new URL(request.url()).pathname;
        if (
          path.includes("/config/adoption") ||
          path.includes("/permissions/validate")
        ) permissionRequests.push({ path, method: request.method() });
      });
      const expectedFault = [
        "non-owner",
        "anonymous",
        "source-error",
      ].includes(state);
      page.on("pageerror", (error) => errors.push(`page: ${error.message}`));
      page.on("console", (message) => {
        if (message.type() === "error") {
          errors.push(`console: ${message.text()}`);
        }
      });
      page.on(
        "requestfailed",
        (request) =>
          errors.push(
            `request: ${request.method()} ${request.url()} ${request.failure()?.errorText}`,
          ),
      );
      page.on("response", (response) => {
        if (response.status() >= 400) {
          errors.push(`http: ${response.status()} ${response.url()}`);
        }
      });
      let releaseLoad: (() => void) | undefined;
      let loadHeld = false;
      if (!before) {
        await page.route("**/config/adoption", async (route) => {
          if (state === "loading" && !loadHeld) {
            loadHeld = true;
            await new Promise<void>((resolve) => {
              releaseLoad = resolve;
            });
          }
          const source = permissionSource(
            "memory-review",
            "permissions-legacy",
          );
          if (state === "draft") {
            source.source_stored = false;
            source.history_head = null;
          }
          if (state === "empty") source.policies = [];
          const status = state === "non-owner"
            ? 403
            : state === "anonymous"
            ? 401
            : state === "source-error"
            ? 503
            : 200;
          const { execution_resolution: _missing, ...incompletePolicy } =
            source.policies[0] ?? {};
          await route.fulfill({
            status,
            json: status === 200
              ? state === "missing-resolution"
                ? { ...source, policies: [incompletePolicy] }
                : source
              : { error: "Intentional owner/source failure fixture" },
          });
        });
      }
      try {
        const response = await page.goto(
          `http://127.0.0.1:${port}/w/memory-review/subjektiv/permissions-legacy/permissions`,
        );
        await page.locator(".permissions").waitFor();
        if (before) {
          await page.getByRole("heading", {
            name: "Direct operations",
            exact: true,
          }).waitFor();
        } else if (state === "loading") {
          await page.getByText(
            "Reading configuration source and permissions…",
          ).waitFor();
        } else if (expectedFault || state === "missing-resolution") {
          await page.getByRole("alert").waitFor();
        } else await page.locator('[data-permission-state="ready"]').waitFor();
        if (!before && state === "owner") {
          await page.getByText("Permission origin", { exact: true }).click();
          await page.getByText("Feature declaration catalog", { exact: true })
            .click();
          assertEquals(
            await page.locator(".resolved > li > code").allTextContents(),
            ["ticket.hoge.fuga.finish"],
          );
          await page.getByRole("button", { name: "Reload permissions" })
            .focus();
          assertEquals(
            await page.locator("button:focus").textContent(),
            "Reload permissions",
          );
          await page.keyboard.press("Tab");
          assertEquals(
            await page.locator("a:focus").textContent(),
            "Edit configuration sources",
          );
          await page.keyboard.press("Tab");
          assertEquals(
            await page.locator("summary:focus").textContent(),
            "Permission origin",
          );
          await page.getByRole("button", { name: "Reload permissions" })
            .focus();
        }
        const id = `${state}-${theme}-${width}`;
        const image = `${output}/${id}.png`;
        await page.screenshot({ path: image, fullPage: true });
        const accessibility = await page.locator("body").ariaSnapshot();
        let catalogCapture: { image: string; sha256: string } | null = null;
        if (!before && state === "owner") {
          await page.locator(".catalog").scrollIntoViewIfNeeded();
          const catalogImage = `${output}/${id}-catalog.png`;
          await page.screenshot({ path: catalogImage });
          catalogCapture = {
            image: catalogImage,
            sha256: await sha256File(catalogImage),
          };
        }
        const overflow = await page.evaluate(() =>
          document.documentElement.scrollWidth > innerWidth
        );
        const faultStatus = state === "non-owner"
          ? 403
          : state === "anonymous"
          ? 401
          : 503;
        const faultPath = "/config/adoption";
        // Existing Sidebar loadWorkspaceWorkerCatalog cancels superseded GETs
        // (sidebar/worker-subscription.ts); keep them as separate evidence.
        const shellCancellations = errors.filter((error) =>
          /^request: GET .*\/api\/w\/memory-review\/workers net::ERR_ABORTED$/
            .test(error)
        );
        const unexpectedErrors = errors.filter((error) =>
          !shellCancellations.includes(error) && (!expectedFault || !(
            (error.startsWith(`http: ${faultStatus} `) &&
              error.endsWith(faultPath)) ||
            (error.startsWith("console: Failed to load resource:") &&
              error.includes(`status of ${faultStatus}`)) ||
            (error.startsWith("request:") && error.includes(faultPath) &&
              error.endsWith("net::ERR_ABORTED"))
          ))
        );
        captures.push({
          id,
          route: new URL(page.url()).pathname,
          persona: state === "non-owner" || state === "anonymous"
            ? state
            : "owner",
          documentStatus: response?.status(),
          errors,
          permissionRequests,
          expectedFault,
          expectedDiagnostic: expectedFault
            ? `HTTP ${faultStatus} from ${faultPath}; cancelled error response body is expected`
            : state === "missing-resolution"
            ? "HTTP 200 source snapshot missing inline execution_resolution; parser must reject"
            : null,
          unexpectedErrors,
          shellCancellations,
          overflow,
          image,
          catalogCapture,
          sha256: await sha256File(image),
          accessibility,
          alerts: await page.getByRole("alert").allTextContents(),
        });
        if (!before) {
          assertEquals(permissionRequests, [{
            path: "/api/w/memory-review/config/adoption",
            method: "GET",
          }]);
        }
        assertEquals(overflow, false);
        assertEquals(unexpectedErrors, []);
      } finally {
        releaseLoad?.();
        if (state === "loading") {
          await page.locator('[data-permission-state="ready"]').waitFor();
        }
        await page.close();
      }
    }
  } finally {
    await Deno.writeTextFile(
      `${output}/review-context.json`,
      JSON.stringify(
        {
          evidence:
            "Actual production Web build; synthetic HTTP only, not Backend persistence or authentication",
          before,
          browser: browser.version(),
          buildSha256: await sha256File(
            `${root}web/workspace/build/index.html`,
          ),
          sourceHashes: await Promise.all(
            ["permissions.ts", "SubjectPermissions.svelte"].map(async (
              file,
            ) => ({
              file,
              sha256: await sha256File(
                `${root}web/workspace/src/lib/workspace/memory/${file}`,
              ),
            })),
          ),
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
