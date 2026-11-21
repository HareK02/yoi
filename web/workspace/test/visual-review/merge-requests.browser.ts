import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium, type Page } from "npm:playwright@1.59.1";
import { loadScenario } from "../../../../tools/web-ux/src/scenario.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";
import {
  mergeRequestDetailFixture,
  mergeRequestFindingsFixture,
  mergeRequestListFixture,
} from "../../src/lib/workspace/merge-request.test-fixtures.ts";

// 実Web buildと既存shell fixtureを接続する。Backendの認可・配送の証拠とは区別する。
const phase = Deno.args[0] ?? "after";
const scenarioPath =
  new URL("./subject-handling-light.json", import.meta.url).pathname;
const scenario = await loadScenario(scenarioPath);
const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
const port = (listener.addr as Deno.NetAddr).port;
listener.close();
const process = scenario.processes![0];
process.args![4] = String(port);
process.readyUrl = `http://127.0.0.1:${port}/health`;
process.args!.splice(1, 0, "--frozen");
const output = (Deno.args[1] ??
  new URL(`../../.svelte-kit/mr-review/${phase}`, import.meta.url).pathname)
  .replace(/\/$/, "") + "/";
await Deno.mkdir(output, { recursive: true });
const processes = await startOwnedProcesses(
  [process],
  scenarioPath,
  output + "logs",
  [],
);
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
const records: unknown[] = [];
try {
  browser = await chromium.launch({ headless: true });
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        colorScheme,
        reducedMotion: "reduce",
      });
      try {
        const page = await context.newPage();
        const errors: string[] = [],
          pageErrors: string[] = [],
          failedRequests: string[] = [];
        const queries: URLSearchParams[] = [],
          historyQueries: URLSearchParams[] = [];
        const methods: string[] = [];
        page.on("console", (message) => {
          if (message.type() === "error") errors.push(message.text());
        });
        page.on("pageerror", (error) => pageErrors.push(String(error)));
        page.on("requestfailed", (request) =>
          failedRequests.push(
            `${
              new URL(request.url()).pathname
            }: ${request.failure()?.errorText}`,
          ));
        let state = "normal";
        await page.route("**/api/w/tickets/merge-requests**", (route) => {
          const url = new URL(route.request().url());
          methods.push(route.request().method());
          if (url.pathname.endsWith("/MR-1")) {
            historyQueries.push(url.searchParams);
            const current = state === "findings"
              ? mergeRequestFindingsFixture()
              : mergeRequestDetailFixture();
            if (state === "findings" && url.searchParams.has("after")) {
              current.thread = [{
                kind: "comment",
                event_id: "later-comment",
                sequence: 51,
                author: null,
                body:
                  "Later history page; current review remains request_changes.",
                created_at: current.updated_at,
                operation_id: null,
                owner_operation_id: null,
              }];
              current.event_page = {
                ...current.event_page,
                has_more: false,
                next_after: null,
              };
            } else if (state === "rejected") {
              const comment = current.thread[0];
              if (comment.kind !== "comment") {
                throw new Error("comment fixture required");
              }
              comment.body =
                "Push rejected: target moved. Inspect the target before the next action.";
              current.target = { ...current.source, ref: "b".repeat(40) };
              current.worker_assignment!.worker = null;
              current.worker_resource_key = null;
            } else if (state === "completed") {
              current.state = {
                kind: "completed",
                completion: {
                  kind: "published",
                  merge_event_id: "merge-1",
                  started_by: "approval-1",
                },
              };
              current.summary.review_status = "approved";
              current.summary.review = {
                decision: "approve",
                event: { event_id: "approval-1", sequence: 1 },
                request_event_id: "request-1",
                subject_ref: current.source.ref!,
                ticket_content_digest: "digest",
                findings_count: 0,
              };
              current.target = { ...current.source };
            } else if (state === "canceled") {
              current.state = {
                kind: "canceled",
                from: { kind: "integration", started_by: "approval-1" },
              };
            }
            return route.fulfill({ json: current });
          }
          queries.push(url.searchParams);
          if (state === "error") {
            return route.fulfill({
              status: 403,
              json: { error: "Permission denied" },
            });
          }
          return route.fulfill({
            json: {
              items: state === "empty" ? [] : [mergeRequestListFixture()],
              next_cursor: state === "normal" && !url.searchParams.has("cursor")
                ? "next-page"
                : null,
            },
          });
        });
        async function capture(name: string, status: number | null) {
          const image = `${colorScheme}-${width}-${name}.png`;
          const bytes = await page.screenshot({
            path: output + image,
            fullPage: true,
          });
          const hash = Array.from(
            new Uint8Array(
              await crypto.subtle.digest("SHA-256", new Uint8Array(bytes)),
            ),
          )
            .map((byte) => byte.toString(16).padStart(2, "0")).join("");
          const overflow = await page.locator("html, main").evaluateAll((
            elements,
          ) =>
            elements.some((element) =>
              element.scrollWidth > element.clientWidth
            )
          );
          const visibleErrors = await page.locator(
            '[role="alert"], .workspace-callout.is-error',
          ).allTextContents();
          records.push({
            phase,
            state,
            image,
            hash,
            route: page.url(),
            documentStatus: status,
            persona: "synthetic authenticated member; read-only MR requests",
            colorScheme,
            width,
            errors: [...errors],
            pageErrors: [...pageErrors],
            failedRequests: [...failedRequests],
            visibleErrors,
            overflow,
            snapshot: await page.locator("main").first().ariaSnapshot(),
          });
          assertEquals(pageErrors, []);
          if (state !== "error") {
            assertEquals(visibleErrors, []);
          } else {
            assertEquals(visibleErrors, [
              "Ticket Browser API request failed with HTTP 403",
            ]);
          }
          assert(
            errors.every((error) => state === "error" && error.includes("403")),
            errors.join("\n"),
          );
          assert(
            failedRequests.every((failure) =>
              failure.endsWith("net::ERR_ABORTED")
            ),
            failedRequests.join("\n"),
          );
          assertEquals(overflow, false);
          assertEquals(status, 200);
        }
        async function noPublicationControls(page: Page) {
          assertEquals(
            await page.getByRole("heading", {
              name: "Publication operations",
              exact: true,
            }).count(),
            0,
          );
          assertEquals(
            await page.getByRole("button", { name: /publish|complete|retry/i })
              .count(),
            0,
          );
        }
        const base = `http://127.0.0.1:${port}/w/tickets/merge-requests`;
        let response = await page.goto(base);
        await page.locator(".ticket-row").waitFor();
        await capture("list", response?.status() ?? null);
        await page.getByLabel("Repository", { exact: true }).fill("main");
        await page.getByLabel("Target", { exact: true }).fill("develop");
        await page.getByLabel("Phase", { exact: true }).selectOption(
          "integration",
        );
        await page.getByRole("button", { name: "Search", exact: true }).click();
        await page.waitForURL(/phase=integration/);
        await page.getByRole("link", { name: "Next page", exact: true })
          .click();
        await page.waitForURL(/cursor=next-page/);
        await page.locator(".ticket-row").waitFor();
        assertEquals(queries.at(-1)?.get("repository_key"), "main");
        assertEquals(queries.at(-1)?.get("selector_to"), "develop");
        assertEquals(queries.at(-1)?.get("phase"), "integration");
        await page.getByRole("link", { name: "First page", exact: true })
          .click();
        await page.waitForURL((url) => !url.searchParams.has("cursor"));
        response = await page.goto(base + "/MR-1");
        await page.getByRole("heading", { name: "Thread", exact: true })
          .waitFor();
        assert(
          await page.getByText("Provider observation unavailable", {
            exact: false,
          }).isVisible(),
        );
        assert(
          await page.getByText(/Publication outcome is unknown/).isVisible(),
        );
        await noPublicationControls(page);
        await capture("detail", response?.status() ?? null);
        for (const next of ["rejected", "completed", "canceled"]) {
          state = next;
          response = await page.goto(base + "/MR-1");
          const text = next === "completed"
            ? "Target integration recorded in retained completion evidence."
            : next === "canceled"
            ? "Merge Request canceled; cancellation is not evidence of target publication."
            : "Push rejected: target moved. Inspect the target before the next action.";
          await page.getByText(text, { exact: true }).waitFor();
          await noPublicationControls(page);
          await capture(next, response?.status() ?? null);
        }
        state = "findings";
        response = await page.goto(base + "/MR-1");
        const findings = page.getByRole("list", { name: "Review findings" });
        await findings.waitFor();
        const review = mergeRequestFindingsFixture().thread[0];
        if (review.kind !== "review") {
          throw new Error("review fixture required");
        }
        assertEquals(
          await findings.locator("li").count(),
          review.findings.length,
        );
        for (const [index, finding] of review.findings.entries()) {
          assertEquals(
            await findings.locator("li").nth(index).locator(".mr-finding-body")
              .textContent(),
            finding.body,
          );
        }
        assertEquals(await page.locator(".mr-event-body").count(), 0);
        await findings.scrollIntoViewIfNeeded();
        await capture("findings", response?.status() ?? null);
        // The production shell scrolls main, so fullPage alone cannot show every long finding.
        for (let index = 0; index < review.findings.length; index++) {
          await findings.locator("li").nth(index).evaluate((element) =>
            element.scrollIntoView({ block: "start" })
          );
          await capture(`finding-${index + 1}`, response?.status() ?? null);
        }
        // Keyboard activation must actually load the next page, not just expose an href.
        await page.getByRole("link", { name: "Next events", exact: true })
          .focus();
        await capture("history-focus", response?.status() ?? null);
        await page.keyboard.press("Enter");
        await page.waitForURL(/after=50/);
        await page.getByText(
          "Later history page; current review remains request_changes.",
          { exact: true },
        ).waitFor();
        assertEquals(historyQueries.at(-1)?.get("after"), "50");
        assertEquals(historyQueries.at(-1)?.get("limit"), "100");
        assertEquals(
          await page.getByRole("link", { name: "Next events", exact: true })
            .count(),
          0,
        );
        assertEquals(await findings.count(), 0);
        assert(
          await page.getByText(/Current source requests changes/).isVisible(),
        );
        await capture("history", response?.status() ?? null);
        await page.getByRole("link", { name: "First events", exact: true })
          .click();
        await page.waitForURL((url) => !url.searchParams.has("after"));
        await findings.waitFor();
        assertEquals(historyQueries.at(-1)?.has("after"), false);
        for (const next of ["empty", "error"]) {
          state = next;
          response = await page.goto(base);
          await page.locator(
            next === "empty"
              ? ".workspace-empty-copy"
              : ".workspace-callout.is-error",
          ).first().waitFor();
          await capture(next, response?.status() ?? null);
        }
        assert(
          methods.length > 0 && methods.every((method) => method === "GET"),
        );
      } finally {
        await context.close();
      }
    }
  }
} finally {
  await Deno.writeTextFile(
    output + "review-context.json",
    JSON.stringify(records, null, 2),
  );
  await browser?.close();
  const cleanupErrors = await stopOwnedProcesses(processes);
  assertEquals(cleanupErrors, []);
}
