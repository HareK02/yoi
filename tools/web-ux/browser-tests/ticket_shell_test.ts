import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { type Browser, chromium, type Page } from "npm:playwright@1.59.1";
import { loadScenario } from "../src/scenario.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

// Explicit browser boundary: production static build + loopback API fixture.
// These tests prove UI/request semantics, not real Backend delivery or model judgment.
async function withFixture(
  run: (browser: Browser, baseUrl: string) => Promise<void>,
) {
  const scenarioPath = new URL("../scenarios/tickets.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const baseUrl = `http://127.0.0.1:${port}`;
  scenario.processes![0].args![4] = String(port);
  scenario.processes![0].readyUrl = `${baseUrl}/health`;
  const logs = new URL(`../../../target/web-ux/t743-browser/${port}/logs`, import.meta.url)
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

async function search(page: Page, value: string) {
  await page.getByRole("searchbox", { name: "Search tickets", exact: true }).fill(value);
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await page.waitForURL((url) => url.searchParams.get("q") === value);
}
function ticketKey(page: Page, key: string) {
  return page.locator(".ticket-list-key").filter({ hasText: new RegExp(`^${key}$`) });
}
async function queryRequests(base: string) {
  const requests = await (await fetch(`${base}/fixture-state`)).json();
  return requests.filter((row: { path: string }) => row.path === "/tickets/query");
}

Deno.test("single Ticket query searches beyond loaded rows and preserves qualifiers on pagination", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets-pages/tickets`);
    await page.locator(".ticket-list-row").first().waitFor();
    assertEquals(await page.locator("a.ticket-list-row").count(), 30);
    assertEquals(await ticketKey(page, "T-35").count(), 0);
    assertEquals(
      await page.getByRole("searchbox", { name: "Search tickets" }).inputValue(),
      "state:open",
    );
    assertEquals(await page.getByRole("form", { name: "Search tickets" }).count(), 1);
    assertEquals(await page.locator(".ticket-kanban, .ticket-lane").count(), 0);
    assertEquals(await page.getByRole("combobox").count(), 0);
    assertEquals(await page.getByRole("checkbox").count(), 0);
    assertEquals((await queryRequests(base)).length, 1);
    const filtered = queryResponse(page, (body) => (body.tag_ids as string[])?.includes("tag-ui"));
    await search(page, 'state:open tag:"UI" assignee:orchestrator');
    assertEquals((await filtered).request().postDataJSON(), {
      query: null,
      states: ["open"],
      tag_ids: ["tag-ui"],
      assigned_principal: { kind: "workspace_agent", agent_key: "workspace-orchestrator" },
      handling_status: null,
      sort: "updated_desc",
      limit: 30,
      cursor: null,
    });
    await ticketKey(page, "T-35").waitFor();
    assertEquals(await page.locator(".ticket-list-row").count(), 2);
    assertEquals((await queryRequests(base)).length, 2);
    await page.goto(
      `${base}/w/tickets-pages/tickets?q=${
        encodeURIComponent("state:open assignee:orchestrator tag:tag-investigation")
      }`,
    );
    await page.getByRole("button", { name: "Load more tickets", exact: true }).waitFor();
    const next = queryResponse(page, (body) => body.cursor === "30");
    await page.getByRole("button", { name: "Load more tickets", exact: true }).click();
    const nextRequest = (await next).request().postDataJSON();
    assertEquals(nextRequest.tag_ids, ["tag-investigation"]);
    assertEquals(nextRequest.states, ["open"]);
    assertEquals(nextRequest.assigned_principal, {
      kind: "workspace_agent",
      agent_key: "workspace-orchestrator",
    });
    await ticketKey(page, "T-40").waitFor();
    assertEquals(await page.locator(".ticket-list-row").count(), 39);
    assertEquals(new Set(await page.locator(".ticket-list-key").allTextContents()).size, 39);
    const finalRow = page.locator("a.ticket-list-row").filter({ has: ticketKey(page, "T-40") });
    const href = await finalRow.getAttribute("href");
    assert(href);
    await finalRow.focus();
    await finalRow.press("Enter");
    await page.waitForURL((url) => url.pathname === new URL(href, base).pathname);
    await page.locator(".ticket-detail-header").waitFor();
  });
});

Deno.test("Orchestrator retains handling filters and assignment pagination independently of Ticket search", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
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
    await page.locator(".orchestrator-assignment").filter({ hasText: "T-40" }).waitFor();
    assertEquals(await page.locator(".orchestrator-assignment").count(), 38);
    await page.getByRole("combobox", { name: "Handling (self-reported)", exact: true })
      .selectOption("waiting");
    const waiting = queryResponse(
      page,
      (body) => body.handling_status === "waiting" && body.cursor === null,
    );
    await page.getByRole("button", { name: "Apply filters", exact: true }).click();
    await waiting;
    await page.locator(".orchestrator-assignment").filter({
      hasText: "Awaiting the user's policy choice",
    }).waitFor();
    assertEquals(await page.locator(".orchestrator-assignment").count(), 1);
    assertEquals(await page.getByRole("link", { name: /T-1 / }).count(), 0);
  });
});

Deno.test("query URL and back-forward restore text, states and a fresh first page", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets-pages/tickets`);
    await page.locator(".ticket-list-row").first().waitFor();
    const all = queryResponse(page, (body) => (body.states as string[])?.length === 0);
    await search(page, "");
    assertEquals((await all).request().postDataJSON().query, null);
    assertEquals(new URL(page.url()).searchParams.get("q"), "");
    await ticketKey(page, "T-4").waitFor();
    const more = queryResponse(page, (body) => body.cursor === "30");
    await page.getByRole("button", { name: "Load more tickets", exact: true }).click();
    await more;
    await ticketKey(page, "T-40").waitFor();
    const closed = queryResponse(page, (body) => (body.states as string[])?.[0] === "closed");
    await search(page, "state:closed");
    assertEquals((await closed).request().postDataJSON().cursor, null);
    await ticketKey(page, "T-4").waitFor();
    assertEquals(await page.locator(".ticket-list-row").count(), 1);
    assertEquals(new URL(page.url()).searchParams.get("q"), "state:closed");
    const back = queryResponse(
      page,
      (body) => (body.states as string[])?.length === 0 && body.cursor === null,
    );
    await page.goBack();
    await back;
    await ticketKey(page, "T-1").waitFor();
    assertEquals(await page.getByRole("searchbox", { name: "Search tickets" }).inputValue(), "");
    assertEquals(await page.locator(".ticket-list-row").count(), 30);
    const forward = queryResponse(
      page,
      (body) => (body.states as string[])?.[0] === "closed" && body.cursor === null,
    );
    await page.goForward();
    await forward;
    await ticketKey(page, "T-4").waitFor();
    assertEquals(
      await page.getByRole("searchbox", { name: "Search tickets" }).inputValue(),
      "state:closed",
    );
    assertEquals(await page.locator(".ticket-list-row").count(), 1);
    const text = queryResponse(page, (body) => body.query === "investigation");
    await search(page, "investigation");
    assertEquals((await text).request().postDataJSON().states, []);
    await ticketKey(page, "T-2").waitFor();
    await ticketKey(page, "T-4").waitFor();
    const stateAll = queryResponse(
      page,
      (body) => body.query === null && (body.states as string[])?.length === 0,
    );
    await search(page, "state:all");
    await stateAll;
    await ticketKey(page, "T-1").waitFor();
    assertEquals(await page.locator(".ticket-list-row").count(), 30);
  });
});

Deno.test("quoted free text containing tag syntax does not depend on the tag catalog", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    let tagRequests = 0;
    await page.route(/\/ticket-tags(?:\?|$)/, async (route) => {
      tagRequests++;
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ message: "Tag catalog unavailable" }),
      });
    });
    const response = queryResponse(page, (body) => body.query === "text tag:Ready");
    await page.goto(`${base}/w/tickets/tickets?q=${encodeURIComponent('"text tag:Ready"')}`);
    const body = (await response).request().postDataJSON();
    assertEquals(body.query, "text tag:Ready");
    assertEquals(body.tag_ids, []);
    assertEquals(body.states, []);
    await page.getByText("No tickets match this query.", { exact: true }).waitFor();
    assertEquals(tagRequests, 0);
    assertEquals(await page.getByRole("alert").count(), 0);
    assertEquals((await queryRequests(base)).length, 1);
    // 障害routeがquery string付きの実際のタグ取得も捕捉することを確認する。
    await search(page, "tag:Ready");
    await page.getByRole("alert").waitFor();
    assertEquals(tagRequests, 1);
    assertEquals((await queryRequests(base)).length, 1);
  });
});

Deno.test("recognized invalid qualifiers show an error without issuing a broad Ticket query", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets`);
    await page.locator(".ticket-list-row").first().waitFor();
    const before = (await queryRequests(base)).length;
    for (
      const value of [
        "state:close",
        "state:waiting",
        "assignee:user/",
        "assignee:worker/embedded",
        "tag:",
        'reason:"',
      ]
    ) {
      await search(page, value);
      await page.waitForURL((url) => url.searchParams.get("q") === value);
      await page.getByRole("alert").first().waitFor();
      assertEquals((await queryRequests(base)).length, before);
      assertEquals(
        await page.getByRole("button", { name: "Load more tickets", exact: true }).count(),
        0,
      );
    }
    const recovered = queryResponse(page, (body) => (body.states as string[])?.[0] === "open");
    await search(page, "state:open");
    await recovered;
    await ticketKey(page, "T-1").waitFor();
    assertEquals(await page.getByRole("alert").count(), 0);
  });
});

Deno.test("current close-reason query sends canonical known/custom reasons and never closes cases", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets`);
    await page.locator(".ticket-list-row").first().waitFor();
    const response = queryResponse(page, (body) => body.closure_reason === "completed");
    await search(page, "state:closed reason:completed");
    assertEquals((await response).request().postDataJSON().states, ["closed"]);
    await ticketKey(page, "T-4").waitFor();
    assertEquals(await page.locator(".ticket-list-row").count(), 1);
    const custom = queryResponse(
      page,
      (body) => (body.closure_reason as { other: string })?.other === "Superseded plan",
    );
    await search(page, 'reason:"Superseded plan"');
    assertEquals((await custom).request().postDataJSON().closure_reason, {
      other: "Superseded plan",
    });
    await page.getByText("No tickets match this query.", { exact: true }).waitFor();
    assertEquals((await detail(base, "T-4")).current_closure.reason, "completed");
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    assert(!requests.some((row: { path: string }) => row.path.endsWith("/case")));
  });
});

Deno.test("assignee qualifiers send exact user and runtime-worker identities, not Worker liveness", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets`);
    await page.locator(".ticket-list-row").first().waitFor();
    for (
      const [value, principal] of [
        ["assignee:user/alice", { kind: "user", account_id: "alice" }],
        ["assignee:worker/embedded/running-worker", {
          kind: "worker",
          runtime_id: "embedded",
          worker_id: "running-worker",
        }],
      ] as const
    ) {
      const response = queryResponse(
        page,
        (body) => JSON.stringify(body.assigned_principal) === JSON.stringify(principal),
      );
      await search(page, value);
      assertEquals((await response).request().postDataJSON().assigned_principal, principal);
      // All fixture delegations are to the Workspace agent. T-3's Running
      // Worker observation must not masquerade as an assigned principal.
      await page.getByText("No tickets match this query.", { exact: true }).waitFor();
      assertEquals(await page.locator(".ticket-list-row").count(), 0);
    }
  });
});

async function boundedGate(promise: Promise<void>) {
  let timer: number | undefined;
  try {
    await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("Pagination response gate timed out")), 30_000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

Deno.test("late old-query pagination cannot append rows after query navigation", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets-pages/tickets`);
    await page.getByRole("button", { name: "Load more tickets", exact: true }).waitFor();
    const held = Promise.withResolvers<void>();
    const release = Promise.withResolvers<void>();
    const finished = Promise.withResolvers<void>();
    await page.route("**/tickets/query", async (route) => {
      if (route.request().postDataJSON().cursor !== "30") return await route.continue();
      const response = await route.fetch();
      held.resolve();
      await release.promise;
      await route.fulfill({ response });
      finished.resolve();
    });
    try {
      await page.getByRole("button", { name: "Load more tickets", exact: true }).click();
      await boundedGate(held.promise);
      const changed = queryResponse(
        page,
        (body) => (body.states as string[])?.[0] === "closed" && body.cursor === null,
      );
      await search(page, "state:closed");
      await changed;
      await ticketKey(page, "T-4").waitFor();
      release.resolve();
      await boundedGate(finished.promise);
      await page.evaluate(
        "new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))",
      );
      assertEquals(await page.locator(".ticket-list-key").allTextContents(), ["T-4"]);
      assertEquals(
        await page.getByRole("button", { name: "Load more tickets", exact: true }).count(),
        0,
      );
    } finally {
      release.resolve();
    }
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
      !requests.some((r: { path: string }) => /\/(queue|ready|state)$/.test(r.path)),
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
      !requests.some((row: { path: string }) => /assignment|\/case$/.test(row.path)),
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
    const saved = (await detail(base, "T-1")).assignment_tracking[0].assignment_id;
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
        expectedAssignment = (await replacement.json()).assignment.assignment_id;
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
    const output = new URL("../../../target/web-ux/t743-browser/matrix", import.meta.url)
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
              ["tickets", "tickets?q=", ".ticket-list-row"],
              ["tickets", "orchestrator", ".orchestrator-assignment"],
              ["tickets-long", "tickets?q=", ".ticket-list-row"],
              ["tickets-empty", "orchestrator", ".orchestrator-assignments"],
              ["tickets-unknown", "orchestrator", ".orchestrator-assignment"],
              ["tickets-error", "tickets", "[role=alert]"],
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
                1,
              );
              assertEquals(
                await page.getByText("No tickets match this query.", { exact: true }).count(),
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
