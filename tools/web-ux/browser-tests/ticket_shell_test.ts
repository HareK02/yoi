import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { type Browser, chromium, type Page } from "npm:playwright@1.59.1";
import { loadScenario } from "../src/scenario.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

// Explicit browser boundary: production static build + loopback API fixture.
// These tests prove UI/request semantics, not real Backend delivery or model judgment.
async function withFixture(
  run: (browser: Browser, baseUrl: string) => Promise<void>,
) {
  const scenarioPath =
    new URL("../scenarios/tickets.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const baseUrl = `http://127.0.0.1:${port}`;
  scenario.processes![0].args![4] = String(port);
  scenario.processes![0].readyUrl = `${baseUrl}/health`;
  const logs =
    new URL(`../../../target/web-ux/t743-browser/${port}/logs`, import.meta.url)
      .pathname;
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    logs,
    [],
  );
  let browser: Browser | null = null;
  try {
    browser = await chromium.launch({ headless: true });
    await run(browser, baseUrl);
  } finally {
    await browser?.close();
    await stopOwnedProcesses(processes);
  }
}
async function detail(base: string, key: string) {
  return await (await fetch(`${base}/api/w/tickets/tickets/${key}`)).json();
}
async function control(base: string, body: unknown) {
  const response = await fetch(`${base}/fixture-control`, {
    method: "POST",
    body: JSON.stringify(body),
  });
  assert(response.ok);
  await response.json();
}
function queryResponse(
  page: Page,
  predicate: (body: Record<string, unknown>) => boolean,
) {
  return page.waitForResponse((response) =>
    response.url().endsWith("/tickets/query") &&
    response.request().method() === "POST" &&
    predicate(response.request().postDataJSON())
  );
}

Deno.test("Ticket filters and Orchestrator handling query the server beyond loaded pages and preserve filters on pagination", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets-pages/tickets`);
    await page.locator('.ticket-lane[data-state="open"] .ticket-card').first()
      .waitFor();
    assertEquals(
      await page.locator('.ticket-lane[data-state="open"] .ticket-card')
        .count(),
      30,
    );
    assertEquals(
      await page.locator(".ticket-card-id").filter({ hasText: /^T-35$/ })
        .count(),
      0,
    );
    await page.getByRole("checkbox", { name: "UI", exact: true }).check();
    await page.getByRole("combobox", {
      name: "Assigned principal",
      exact: true,
    }).selectOption("orchestrator");
    const filtered = queryResponse(
      page,
      (body) =>
        (body.tag_ids as string[])?.includes("tag-ui") &&
        (body.assigned_principal as { kind: string })?.kind ===
          "workspace_agent" &&
        (body.states as string[]).includes("open"),
    );
    await page.getByRole("button", { name: "Apply filters", exact: true })
      .click();
    const response = await filtered;
    assertEquals(response.request().postDataJSON(), {
      query: null,
      states: ["open"],
      tag_ids: ["tag-ui"],
      assigned_principal: {
        kind: "workspace_agent",
        agent_key: "workspace-orchestrator",
      },
      handling_status: null,
      sort: "updated_desc",
      limit: 30,
      cursor: null,
    });
    await page.locator(".ticket-card-id").filter({ hasText: /^T-35$/ })
      .waitFor();
    assertEquals(
      await page.locator('.ticket-lane[data-state="open"] .ticket-card')
        .count(),
      2,
    );
    await page.goto(
      `${base}/w/tickets-pages/tickets?case=open&principal=orchestrator&tag=tag-investigation`,
    );
    await page.getByRole("button", { name: "Load more open tickets" })
      .waitFor();
    const next = queryResponse(page, (body) => body.cursor === "30");
    await page.getByRole("button", { name: "Load more open tickets" }).click();
    const nextRequest = (await next).request().postDataJSON();
    assertEquals(nextRequest.tag_ids, ["tag-investigation"]);
    assertEquals(nextRequest.states, ["open"]);
    assertEquals(nextRequest.assigned_principal, {
      kind: "workspace_agent",
      agent_key: "workspace-orchestrator",
    });
    await page.locator(".ticket-card-id").filter({ hasText: /^T-40$/ })
      .waitFor();
    assertEquals(await page.locator(".ticket-card").count(), 39);
    await page.goto(`${base}/w/tickets-pages/orchestrator?handling=done`);
    await page.getByRole("button", { name: "Load more assignments" }).waitFor();
    const moreHandling = queryResponse(
      page,
      (body) => body.cursor === "30" && body.handling_status === "done",
    );
    await page.getByRole("button", { name: "Load more assignments" }).click();
    const handlingRequest = (await moreHandling).request().postDataJSON();
    assertEquals(handlingRequest.assigned_principal, {
      kind: "workspace_agent",
      agent_key: "workspace-orchestrator",
    });
    assertEquals(handlingRequest.states, []);
    await page.locator(".orchestrator-assignment").filter({ hasText: "T-40" })
      .waitFor();
    assertEquals(await page.locator(".orchestrator-assignment").count(), 38);
    await page.getByRole("combobox", {
      name: "Handling (self-reported)",
      exact: true,
    }).selectOption("waiting");
    const waiting = queryResponse(
      page,
      (body) => body.handling_status === "waiting" && body.cursor === null,
    );
    await page.getByRole("button", { name: "Apply filters", exact: true })
      .click();
    await waiting;
    await page.locator(".orchestrator-assignment").filter({
      hasText: "Awaiting the user's policy choice",
    }).waitFor();
    assertEquals(await page.locator(".orchestrator-assignment").count(), 1);
    assertEquals(await page.getByRole("link", { name: /T-1 / }).count(), 0);
  });
});

Deno.test("current close-reason filtering sends canonical known/custom reasons and never closes cases", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets`);
    await page.getByText("Close reason filter", { exact: true }).click();
    await page.getByRole("combobox", {
      name: "Current close reason",
      exact: true,
    }).selectOption("completed");
    await page.getByRole("combobox", { name: "Case", exact: true })
      .selectOption("closed");
    const response = queryResponse(
      page,
      (body) => body.closure_reason === "completed",
    );
    await page.getByRole("button", { name: "Apply filters", exact: true })
      .click();
    assertEquals((await response).request().postDataJSON().states, ["closed"]);
    await page.locator('.ticket-lane[data-state="open"]').waitFor({
      state: "hidden",
    });
    assertEquals(await page.locator(".ticket-card").count(), 1);
    assertEquals(await page.locator(".ticket-card-id").textContent(), "T-4");
    await page.getByRole("combobox", {
      name: "Current close reason",
      exact: true,
    }).selectOption("other");
    await page.getByLabel("Close reason text", { exact: true }).fill(
      "Superseded plan",
    );
    const custom = queryResponse(
      page,
      (body) =>
        (body.closure_reason as { other: string })?.other === "Superseded plan",
    );
    await page.getByRole("button", { name: "Apply filters", exact: true })
      .click();
    assertEquals((await custom).request().postDataJSON().closure_reason, {
      other: "Superseded plan",
    });
    await page.getByText("No tickets", { exact: true }).waitFor();
    assertEquals(
      (await detail(base, "T-4")).current_closure.reason,
      "completed",
    );
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    assert(
      !requests.some((row: { path: string }) => row.path.endsWith("/case")),
    );
  });
});

Deno.test("waiting answers retain a delegation, done reissue retains results, and reasoned close/reopen leaves handling separate", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets/T-2`);
    await page.getByText("Self-reported handling: waiting", { exact: true })
      .waitFor();
    const before = await detail(base, "T-2");
    const row = before.assignment_tracking[0];
    await page.getByText("Record result or comment", { exact: true }).click();
    await page.getByLabel("Body", { exact: true }).fill(
      "Ordinary retained discussion, not an answer input",
    );
    const eventResponse = page.waitForResponse((response) =>
      response.url().endsWith("/events") &&
      response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Append event", exact: true })
      .click();
    assertEquals((await eventResponse).request().postDataJSON(), {
      role: "comment",
      body: "Ordinary retained discussion, not an answer input",
    });
    await page.getByText("Ordinary retained discussion, not an answer input", {
      exact: true,
    }).waitFor();
    assertEquals((await detail(base, "T-2")).assignment_tracking[0], row);
    await page.getByLabel("Answer / continue this handling", { exact: true })
      .fill("Choose read-only comparison; keep the same delegation");
    const answerResponse = page.waitForResponse((response) =>
      response.url().endsWith("/assignment-answer") &&
      response.request().method() === "POST"
    );
    await page.getByRole("button", {
      name: "Send answer to this assignment",
      exact: true,
    }).click();
    const answer = (await answerResponse).request().postDataJSON();
    assertEquals(answer.assignment_id, row.assignment_id);
    assertEquals(answer.expected_last_event_id, row.last_event_id);
    assertEquals(
      answer.body,
      "Choose read-only comparison; keep the same delegation",
    );
    assert(typeof answer.operation_key === "string");
    assertEquals(Object.keys(answer).toSorted(), [
      "assignment_id",
      "body",
      "expected_last_event_id",
      "operation_key",
    ]);
    await page.getByRole("status").filter({
      hasText: "Answer saved to assignment",
    }).waitFor();
    const after = await detail(base, "T-2");
    assertEquals(after.assignment_tracking[0].assignment_id, row.assignment_id);
    assertEquals(after.assignment_tracking[0].tracking.status, "waiting");
    assertEquals(after.events.at(-1).kind, "assignment_answer");
    await page.goto(`${base}/w/tickets/tickets/T-1`);
    await page.getByText("Self-reported handling: done", { exact: true })
      .waitFor();
    const original = (await detail(base, "T-1")).assignment_tracking[0];
    const reissueResponse = page.waitForResponse((response) =>
      response.url().endsWith("/assignments/orchestrator") &&
      response.request().method() === "PUT"
    );
    await page.getByRole("button", {
      name: "Request again (new assignment)",
      exact: true,
    }).click();
    const request = (await reissueResponse).request().postDataJSON();
    assertEquals(request.expected_assignment_id, original.assignment_id);
    assertEquals(request.principal, {
      kind: "workspace_agent",
      agent_key: "workspace-orchestrator",
    });
    assert(typeof request.operation_id === "string");
    await page.getByText("Self-reported handling: waiting", { exact: true })
      .waitFor();
    const reissued = await detail(base, "T-1");
    assert(
      reissued.assignment_tracking[0].assignment_id !== original.assignment_id,
    );
    assertEquals(reissued.state, "open");
    await page.getByText("Past assignment and release history", { exact: true })
      .click();
    await page.getByLabel("Assignment identity", { exact: true }).fill(
      original.assignment_id,
    );
    await page.getByRole("button", { name: "Read assignment", exact: true })
      .click();
    await page.getByText(/Previous result: Compared options/).waitFor();
    await page.getByText(/replaced by fixture-owner/).waitFor();
    await page.goto(`${base}/w/tickets/tickets/T-4`);
    await page.getByText("Self-reported handling: done", { exact: true })
      .waitFor();
    await page.locator(".ticket-case-card").getByLabel("Explanation", {
      exact: true,
    }).fill("Reconsider the case without delegating again");
    const reopenResponse = page.waitForResponse((response) =>
      response.url().endsWith("/case") && response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Reopen ticket", exact: true })
      .click();
    const reopen = (await reopenResponse).request().postDataJSON();
    assertEquals(reopen.action, { action: "reopen" });
    assertEquals(reopen.expected_state, "closed");
    assertEquals(reopen.expected_closure_operation_key, "close-T-4");
    await page.getByRole("button", { name: "Close ticket", exact: true })
      .waitFor();
    const reopened = await detail(base, "T-4");
    assertEquals(reopened.state, "open");
    assertEquals(reopened.assignment_tracking[0].tracking.status, "done");
    assertEquals(reopened.closure_history.length, 2);
    await page.locator(".ticket-case-card").getByLabel("Explanation", {
      exact: true,
    }).fill("The full requested investigation is delivered");
    await page.locator(".ticket-case-card select").selectOption("completed");
    const closeResponse = page.waitForResponse((response) =>
      response.url().endsWith("/case") && response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Close ticket", exact: true })
      .click();
    const close = (await closeResponse).request().postDataJSON();
    assertEquals(close.action, { action: "close", reason: "completed" });
    assertEquals(close.expected_state, "open");
    await page.getByRole("button", { name: "Reopen ticket", exact: true })
      .waitFor();
    const closed = await detail(base, "T-4");
    assertEquals(closed.current_closure.reason, "completed");
    assertEquals(closed.tags, reopened.tags);
    assertEquals(
      closed.assignment_tracking[0].assignment_id,
      reopened.assignment_tracking[0].assignment_id,
    );
    await page.getByText("Past assignment and release history", { exact: true })
      .click();
    await page.getByLabel("Assignment identity", { exact: true }).fill(
      "released-T-4",
    );
    await page.getByRole("button", { name: "Read assignment", exact: true })
      .click();
    await page.getByText(/unassigned by fixture-owner/).waitFor();
    await page.getByText(
      "Previous result: Delivered the full requested investigation",
      {
        exact: true,
      },
    ).waitFor();
    assert(
      await page.getByText("Self-reported handling: done", { exact: true })
        .isVisible(),
    );
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    assert(
      !requests.some((r: { path: string }) =>
        /\/(queue|ready|state)$/.test(r.path)
      ),
    );
  });
});

Deno.test("repository-free creation and canonical tag create/attach/rename/delete do not close or delegate the case", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets`);
    await page.getByRole("button", { name: "Add Ticket", exact: true }).click();
    await page.getByLabel("Title", { exact: true }).fill(
      "Repository-free investigation",
    );
    await page.getByLabel("Body", { exact: true }).fill(
      "Compare options and return without deciding",
    );
    const createResponse = page.waitForResponse((response) =>
      response.url().endsWith("/tickets") &&
      response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Create Ticket", exact: true })
      .click();
    const create = (await createResponse).request().postDataJSON();
    assertEquals(create.targets, []);
    assertEquals(create.workflow_state, "open");
    assertEquals(create.title, "Repository-free investigation");
    await page.getByRole("heading", {
      name: "Repository-free investigation",
      exact: true,
    }).waitFor();
    await page.getByText("No current Orchestrator assignment.", { exact: true })
      .waitFor();
    await page.getByText("Manage Workspace tags", { exact: true }).click();
    await page.getByLabel("Tag name", { exact: true }).fill("Comparisons");
    const tagCreate = page.waitForResponse((response) =>
      response.url().endsWith("/ticket-tags") &&
      response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Create tag", exact: true }).click();
    const tagResult = await tagCreate;
    const tag = await tagResult.json();
    assertEquals(tagResult.request().postDataJSON().action, {
      kind: "create",
      name: "Comparisons",
    });
    await page.getByText("Edit classification", { exact: true }).click();
    await page.getByRole("checkbox", { name: "Comparisons", exact: true })
      .check();
    await page.getByRole("checkbox", { name: "Investigation", exact: true })
      .check();
    const attach = page.waitForResponse((response) =>
      response.url().endsWith("/tags") && response.request().method() === "PUT"
    );
    await page.getByRole("button", { name: "Save classification", exact: true })
      .click();
    const attachRequest = (await attach).request().postDataJSON();
    assertEquals(attachRequest.expected_tag_ids, []);
    assertEquals(
      attachRequest.tag_ids.toSorted(),
      [tag.id, "tag-investigation"].toSorted(),
    );
    await page.locator(".ticket-tag-chips").getByText("Comparisons", {
      exact: true,
    }).waitFor();
    await page.getByRole("combobox", { name: "Existing tag", exact: true })
      .selectOption(tag.id);
    await page.getByLabel("Tag name", { exact: true }).fill("Options");
    const renamed = page.waitForResponse((response) =>
      response.url().endsWith("/ticket-tags") &&
      response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Rename tag", exact: true }).click();
    const rename = (await renamed).request().postDataJSON();
    assertEquals(rename.action, {
      kind: "rename",
      tag_id: tag.id,
      expected_last_operation_key: tag.last_operation_key,
      name: "Options",
    });
    await page.locator(".ticket-tag-chips").getByText("Options", {
      exact: true,
    }).waitFor();
    const deleted = page.waitForResponse((response) =>
      response.url().endsWith("/ticket-tags") &&
      response.request().method() === "POST"
    );
    page.once("dialog", (dialog) => dialog.accept());
    await page.getByRole("button", { name: "Delete tag", exact: true }).click();
    assertEquals((await deleted).request().postDataJSON().action, {
      kind: "delete",
      tag_id: tag.id,
      expected_last_operation_key: rename.operation_key,
    });
    await page.locator(".ticket-tag-chips").getByText("Options", {
      exact: true,
    }).waitFor({ state: "hidden" });
    const ticket = await detail(base, "T-41");
    assertEquals(ticket.state, "open");
    assertEquals(ticket.assignment_tracking, []);
    assertEquals(ticket.tags.map((tag: { id: string }) => tag.id), [
      "tag-investigation",
    ]);
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    assert(
      !requests.some((row: { path: string }) =>
        /assignment|\/case$/.test(row.path)
      ),
    );
  });
});

Deno.test("ambiguous saved Assign retries exact JSON once, while stale and permission denials never claim success", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets/T-1`);
    await page.getByRole("button", {
      name: "Request again (new assignment)",
      exact: true,
    }).waitFor();
    await control(base, { failure: "unknown_saved" });
    await page.getByRole("button", {
      name: "Request again (new assignment)",
      exact: true,
    }).click();
    await page.getByRole("alert").filter({ hasText: "Outcome unknown" })
      .waitFor();
    assertEquals(
      await page.getByRole("status").filter({
        hasText: "Assignment operation saved",
      }).count(),
      0,
    );
    const saved =
      (await detail(base, "T-1")).assignment_tracking[0].assignment_id;
    const beforeRetry = await (await fetch(`${base}/fixture-state`)).json();
    const first = beforeRetry.filter((item: { path: string }) =>
      item.path.endsWith("/assignments/orchestrator")
    );
    assertEquals(first.length, 1);
    await page.getByRole("button", {
      name: "Retry exact operation",
      exact: true,
    }).click();
    await page.getByRole("status").filter({
      hasText: "Assignment operation saved",
    }).waitFor();
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    const assignmentRequests = requests.filter((item: { path: string }) =>
      item.path.endsWith("/assignments/orchestrator")
    );
    assertEquals(assignmentRequests.length, 2);
    assertEquals(assignmentRequests[0].body, assignmentRequests[1].body);
    assertEquals(
      (await detail(base, "T-1")).assignment_tracking[0].assignment_id,
      saved,
    );
    for (const failure of ["stale", "denied", "unknown_unsaved"] as const) {
      await control(base, {
        reset: true,
        failure: failure === "stale" ? "none" : failure,
      });
      await page.goto(`${base}/w/tickets/tickets/T-1`);
      await page.getByRole("button", {
        name: "Request again (new assignment)",
        exact: true,
      }).waitFor();
      let expectedAssignment = "assignment-T-1";
      if (failure === "stale") {
        const replacement = await fetch(
          `${base}/api/w/tickets/tickets/internal-T-1/assignments/orchestrator`,
          {
            method: "PUT",
            body: JSON.stringify({
              operation_id: "concurrent-replacement",
              expected_assignment_id: "assignment-T-1",
              principal: {
                kind: "workspace_agent",
                agent_key: "workspace-orchestrator",
              },
            }),
          },
        );
        assert(replacement.ok);
        expectedAssignment =
          (await replacement.json()).assignment.assignment_id;
      }
      await page.getByRole("button", {
        name: "Request again (new assignment)",
        exact: true,
      }).click();
      await page.getByRole("alert").filter({
        hasText: failure === "stale"
          ? "Stale observation"
          : failure === "denied"
          ? "Permission denied"
          : "Outcome unknown",
      }).waitFor();
      assertEquals(
        (await detail(base, "T-1")).assignment_tracking[0].assignment_id,
        expectedAssignment,
      );
      assertEquals(
        await page.getByRole("status").filter({
          hasText: "Assignment operation saved",
        }).count(),
        0,
      );
      const attempts = await (await fetch(`${base}/fixture-state`)).json();
      assertEquals(
        attempts.filter((
          r: { path: string; body: { operation_id?: string } },
        ) =>
          r.path.endsWith("/assignments/orchestrator") &&
          r.body.operation_id !== "concurrent-replacement"
        ).length,
        1,
      );
    }
  });
});

Deno.test("case and handling matrix renders without overflow in both themes and empty/error/unknown states stay distinct", async () => {
  await withFixture(async (browser, base) => {
    const output =
      new URL("../../../target/web-ux/t743-browser/matrix", import.meta.url)
        .pathname;
    await Deno.mkdir(output, { recursive: true });
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme,
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          const errors: string[] = [];
          page.on("pageerror", (error) => errors.push(String(error)));
          for (
            const [workspace, route, ready] of [
              ["tickets", "tickets", ".ticket-card"],
              ["tickets", "orchestrator", ".orchestrator-assignment"],
              ["tickets-long", "tickets", ".ticket-card"],
              ["tickets-empty", "orchestrator", ".orchestrator-assignments"],
              ["tickets-unknown", "orchestrator", ".orchestrator-assignment"],
              ["tickets-error", "tickets", ".ticket-lane-page-error"],
              ["tickets", "tickets/T-1", ".ticket-detail-header"],
              ["tickets", "tickets/T-2", ".ticket-worker-observation strong"],
              ["tickets", "tickets/T-3", ".ticket-worker-observation strong"],
              ["tickets", "tickets/T-4", ".ticket-detail-header"],
              [
                "tickets-readonly",
                "tickets/T-2",
                ".ticket-worker-observation strong",
              ],
              [
                "tickets-running",
                "tickets/T-2",
                ".ticket-worker-observation strong",
              ],
            ]
          ) {
            await page.goto(`${base}/w/${workspace}/${route}`);
            await page.locator(ready).first().waitFor();
            assertEquals(
              await page.evaluate(
                "document.documentElement.scrollWidth > innerWidth",
              ),
              false,
            );
            if (workspace === "tickets-empty") {
              assert(
                await page.getByText(
                  "No current assignments match these filters.",
                  { exact: true },
                ).isVisible(),
              );
            }
            if (workspace === "tickets-error") {
              assertEquals(
                await page.getByRole("button", { name: "Retry", exact: true })
                  .count(),
                2,
              );
              assertEquals(
                await page.getByText("No tickets", { exact: true }).count(),
                0,
              );
            }
            if (workspace === "tickets-unknown") {
              assertEquals(
                await page.getByText("Handling: Unknown (not reported)", {
                  exact: true,
                }).count(),
                4,
              );
            }
            if (route === "tickets/T-2") {
              assert(
                await page.getByText("Self-reported handling: waiting", {
                  exact: true,
                }).isVisible(),
              );
              assertEquals(
                await page.locator(".ticket-worker-observation strong")
                  .textContent(),
                "Stopped",
              );
              if (workspace === "tickets-readonly") {
                assertEquals(
                  await page.getByRole("button", {
                    name: "Replace assignment",
                    exact: true,
                  }).count(),
                  0,
                );
              }
              if (workspace === "tickets-running") {
                assert(
                  await page.locator(".ticket-orchestrator-worker").getByText(
                    "Running",
                    { exact: true },
                  ).isVisible(),
                );
              }
            }
            if (route === "tickets/T-3") {
              assert(
                await page.getByText("Self-reported handling: inprogress", {
                  exact: true,
                }).isVisible(),
              );
              assertEquals(
                await page.locator(".ticket-worker-observation strong")
                  .textContent(),
                "Running",
              );
              assert(
                await page.getByText("main: open", { exact: true }).isVisible(),
              );
            }
            await page.screenshot({
              path: `${output}/${colorScheme}-${width}-${workspace}-${
                route.replaceAll("/", "-")
              }.png`,
              fullPage: true,
            });
          }
          assertEquals(errors, []);
        } finally {
          await context.close();
        }
      }
    }
  });
});
