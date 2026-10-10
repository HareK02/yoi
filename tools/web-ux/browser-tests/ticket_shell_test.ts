import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import { loadScenario } from "../src/scenario.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

// Browser boundary over the production static build and synthetic loopback API.
// No Backend, Worker, Repository, review or integration operation is executed.
Deno.test("Ticket progress, comment results and reopen work without Git while real MR evidence stays separate", async () => {
  const scenarioPath = new URL("../scenarios/tickets.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  scenario.baseUrl = `http://127.0.0.1:${port}`;
  scenario.processes![0].args![4] = String(port);
  scenario.processes![0].readyUrl = `${scenario.baseUrl}/health`;
  const output = new URL("../../../target/web-ux/t720-browser/", import.meta.url).pathname;
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    `${output}/logs`,
    [],
  );
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
          const unexpected: string[] = [];
          page.on("pageerror", (e) => unexpected.push(String(e)));
          await page.goto(`${scenario.baseUrl}/w/tickets/tickets`);
          const addTicket = page.getByRole("button", { name: "Add Ticket", exact: true });
          await addTicket.waitFor();
          const action = await addTicket.boundingBox();
          assert(action && action.x >= 0 && action.x + action.width <= width && action.y < 900);
          assertEquals(
            await page.evaluate("document.documentElement.scrollWidth > innerWidth"),
            false,
          );
          await addTicket.click();
          await page.locator(".ticket-create-form").waitFor();
          assertEquals(await page.locator(".ticket-target-row").count(), 0);
          assertEquals(
            await page.evaluate("document.documentElement.scrollWidth > innerWidth"),
            false,
          );
          await page.goto(
            `${scenario.baseUrl}/w/tickets/tickets/T-1-compare-options-without-deciding`,
          );
          await page.locator(".ticket-detail-header").waitFor();
          // Intent comes first at one-column widths, unlike the former controls-first layout.
          if (width < 1024) {
            const intent = await page.getByRole("heading", { name: "Intent", exact: true })
              .boundingBox();
            const progress = await page.getByRole("heading", { name: "Progress decision" })
              .boundingBox();
            assert(intent && progress && intent.y < progress.y);
          }
          const state = page.getByRole("combobox", { name: "State", exact: true });
          await state.waitFor();
          assertEquals(await state.locator("option").allTextContents(), [
            "planning",
            "ready",
            "queued",
            "inprogress",
            "done",
            "closed",
          ]);
          await state.selectOption("inprogress");
          await page.getByLabel("Reason", { exact: true }).first().fill(
            "Investigating options directly",
          );
          await page.getByRole("button", { name: "Apply state", exact: true }).click();
          await page.locator('.ticket-detail-kicker [data-status="inprogress"]').waitFor();
          await page.getByText("Record result or comment", { exact: true }).click();
          await page.getByLabel("Body", { exact: true }).fill(
            `Option A and B are available; return to the user without a conclusion (${colorScheme}/${width}).`,
          );
          await page.getByRole("button", { name: "Append event" }).click();
          await page.getByText(
            `Option A and B are available; return to the user without a conclusion (${colorScheme}/${width}).`,
            { exact: true },
          ).waitFor();
          await state.selectOption("done");
          await page.getByLabel("Reason", { exact: true }).first().fill(
            "Requested comparison returned",
          );
          await page.getByLabel("Result, references, and remaining work (optional)").fill(
            "Result: timeline comment. [Reference](https://example.com/options). Remaining: user choice.",
          );
          await page.getByRole("button", { name: "Complete Ticket", exact: true }).click();
          await page.locator('.ticket-detail-kicker [data-status="done"]').waitFor();
          assertEquals(
            await page.getByRole("heading", { name: "MR requirement evidence" }).count(),
            0,
          );
          assert(await page.getByText("No Merge Requests linked.", { exact: false }).isVisible());
          await state.selectOption("planning");
          await page.getByLabel("Reason", { exact: true }).first().fill(
            "User requests another comparison",
          );
          await page.getByRole("button", { name: "Reopen / apply state" }).click();
          await page.locator('.ticket-detail-kicker [data-status="planning"]').waitFor();
          await page.getByRole("button", { name: "Request Orchestrator (queue)" }).click();
          await page.locator('.ticket-detail-kicker [data-status="queued"]').waitFor();
          await page.locator(".ticket-close-card > summary").click();
          await page.getByLabel("Resolution").fill("Discussion returned; no code or MR required");
          await page.getByRole("button", { name: "Close ticket", exact: true }).click();
          await page.locator('.ticket-detail-kicker [data-status="closed"]').waitFor();
          await page.goto(`${scenario.baseUrl}/w/tickets/tickets/T-2-read-only-investigation`);
          await page.locator(".ticket-detail-header").waitFor();
          assertEquals(
            await page.getByRole("combobox", { name: "Access", exact: true }).inputValue(),
            "read_only",
          );
          assert(await page.getByRole("button", { name: "Mark ready", exact: true }).isEnabled());
          await page.getByRole("button", { name: "Mark ready", exact: true }).click();
          await page.locator('.ticket-detail-kicker [data-status="ready"]').waitFor();
          assert(await page.getByRole("combobox", { name: "Repository", exact: true }).isEnabled());
          await page.getByRole("button", { name: "Save targets", exact: true }).click();
          await page.getByRole("button", { name: "Save targets", exact: true }).waitFor({
            state: "visible",
          });
          await page.goto(
            `${scenario.baseUrl}/w/tickets/tickets/T-3-independent-review-before-merge`,
          );
          await page.locator(".ticket-detail-header").waitFor();
          await state.selectOption("done");
          await page.getByLabel("Reason", { exact: true }).first().fill(
            "Progress decision is separate from review",
          );
          await page.getByRole("button", { name: "Complete Ticket" }).click();
          await page.locator('.ticket-detail-kicker [data-status="done"]').waitFor();
          assert(await page.getByText("main: open", { exact: true }).isVisible());
          assert(
            await page.getByText("Ticket completion is not MR approval or integration.", {
              exact: true,
            }).isVisible(),
          );
          assertEquals(await page.getByText("main: merged", { exact: true }).count(), 0);
          // Member normal operations do not enable owner configuration controls.
          assertEquals(await page.getByRole("link", { name: "Secrets", exact: true }).count(), 0);
          assertEquals(
            await page.evaluate("document.documentElement.scrollWidth > innerWidth"),
            false,
          );
          await Deno.mkdir(output, { recursive: true });
          await page.screenshot({
            path: `${output}/${colorScheme}-${width}-mr-done.png`,
            fullPage: true,
          });
          assertEquals(unexpected, []);
          // Reset synthetic records for the next viewport using the fixture's ordinary state API.
          for (const key of ["T-1", "T-2", "T-3"]) {
            const detail =
              await (await context.request.get(`${scenario.baseUrl}/api/w/tickets/tickets/${key}`))
                .json();
            await context.request.post(`${scenario.baseUrl}/api/w/tickets/tickets/${key}/state`, {
              data: {
                state: "planning",
                reason: "Fixture reset",
                expected_state: detail.state,
                expected_content_digest: detail.content_digest,
              },
            });
          }
        } finally {
          await context.close();
        }
      }
    }
    const context = await browser.newContext();
    try {
      const page = await context.newPage();
      await page.goto(`${scenario.baseUrl}/w/tickets/tickets`);
      await page.getByRole("button", { name: "Add Ticket", exact: true }).click();
      await page.getByLabel("Title", { exact: true }).fill("Repository-free investigation");
      await page.getByLabel("Body", { exact: true }).fill(
        "Compare options and return without conclusion; review is unnecessary.",
      );
      await page.getByRole("button", { name: "Create Ticket", exact: true }).click();
      await page.getByRole("heading", { name: "Repository-free investigation", exact: true })
        .waitFor();
      assert(await page.getByText("No repository targets.", { exact: true }).isVisible());
      // Generic remote launch: a natural request, not an inferred code/review pipeline.
      await page.goto(
        `${scenario.baseUrl}/w/tickets/workers/new?ticketId=T-4&initialInput=Return%20without%20conclusion.%20Review%20is%20unnecessary.`,
      );
      await page.locator(".worker-launch-form").waitFor();
      await page.getByRole("combobox", { name: "Runtime", exact: true }).selectOption("remote");
      const launches: Record<string, unknown>[] = [];
      await page.route("**/api/w/tickets/workers", async (route) => {
        launches.push(route.request().postDataJSON());
        await route.fulfill({
          status: 403,
          json: { error: "permission_denied", message: "Permission denied", diagnostics: [] },
        });
      });
      await page.getByRole("button", { name: "Start Worker", exact: true }).click();
      await page.getByText("permission_denied: Permission denied", { exact: true }).waitFor();
      assertEquals(launches.length, 1);
      assertEquals(launches[0].workdir_attachments, []);
      assertEquals(launches[0].profile, null);
      assertEquals(launches[0].initial_submit, [{
        kind: "text",
        content: "Return without conclusion. Review is unnecessary.",
      }]);
      assertEquals((launches[0].ticket_assignment as { ticket_id: string }).ticket_id, "T-4");
      await page.unroute("**/api/w/tickets/workers");
      for (
        const [status, message] of [[409, "Update conflict."], [403, "Permission denied."], [
          503,
          "Outcome unknown or request rejected.",
        ]] as const
      ) {
        await page.goto(
          `${scenario.baseUrl}/w/tickets/tickets/T-1-compare-options-without-deciding`,
        );
        await page.locator(".ticket-detail-header").waitFor();
        await page.route(
          "**/tickets/*/state",
          (route) => route.fulfill({ status, json: { error: "failure", message } }),
        );
        await page.getByRole("combobox", { name: "State", exact: true }).selectOption("done");
        await page.getByLabel("Reason", { exact: true }).first().fill(
          "Keep this draft on rejection",
        );
        await page.getByRole("button", { name: "Complete Ticket" }).click();
        await page.getByRole("alert").getByText(message, { exact: false }).waitFor();
        assertEquals(
          await page.getByLabel("Reason", { exact: true }).first().inputValue(),
          "Keep this draft on rejection",
        );
        assert(await page.locator('.ticket-detail-kicker [data-status="planning"]').isVisible());
        assert(await page.getByRole("button", { name: "Refresh Ticket status" }).isEnabled());
        await page.unroute("**/tickets/*/state");
      }
    } finally {
      await context.close();
    }
    const requests = await (await fetch(`${scenario.baseUrl}/fixture-state`)).json();
    assert(
      requests.some((r: { body: { body?: string } }) =>
        r.body.body?.includes("Remaining: user choice")
      ),
    );
    assert(!requests.some((r: { path: string }) => /merge|review|workers/.test(r.path)));
  } finally {
    await browser.close();
    await stopOwnedProcesses(processes);
  }
});
