import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { join } from "@std/path";
import { type Browser, chromium, type Page } from "npm:playwright@1.59.1";
import { loadScenario } from "../src/scenario.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

function artifactPath(path: string): string {
  return join(Deno.env.get("WEB_UX_TEST_ARTIFACTS") ?? new URL("../../../target/web-ux", import.meta.url).pathname, "t743-browser", path);
}

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
  const logs = artifactPath(`${port}/logs`);
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

Deno.test("Ticket parser failures stay local, retain Workspace navigation, and recover on retry", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    const wire = await detail(base, "T-1");
    delete wire.initial_post;
    let invalid = true;
    await page.route("**/api/w/tickets/tickets/T-1", async (route) => {
      if (invalid) await route.fulfill({ status: 200, json: wire });
      else await route.continue();
    });
    await page.goto(`${base}/w/tickets/tickets/T-1`);
    await page.getByRole("alert").filter({ hasText: "Ticket detail.initial_post must be an object" }).waitFor();
    assertEquals(await page.getByText("The selected Workspace cannot be opened", { exact: true }).count(), 0);
    assertEquals(await page.getByText("Workspace unavailable", { exact: true }).count(), 0);
    assert(await page.getByRole("navigation", { name: "Current workspace location" }).isVisible());
    invalid = false;
    await page.getByRole("button", { name: "Retry", exact: true }).click();
    await page.locator(".ticket-detail-header").waitFor();
    assertEquals(await page.getByRole("alert").filter({ hasText: "initial_post must be an object" }).count(), 0);
    await page.close();
  });
});

Deno.test("Workspace HTTP failure retains its real status and recovers without recreating the Workspace", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    let unavailable = true;
    await page.route("**/api/w/tickets/workspace", async (route) => {
      if (unavailable) await route.fulfill({ status: 503, body: "private server failure" });
      else await route.continue();
    });
    await page.goto(`${base}/w/tickets/tickets/T-1`);
    await page.getByRole("heading", { name: "Workspace details could not be loaded" }).waitFor();
    assert(await page.locator(".workspace-page [role=alert]").filter({ hasText: "HTTP 503" }).isVisible());
    assertEquals(await page.getByText("The selected Workspace cannot be opened", { exact: true }).count(), 0);
    assertEquals(await page.getByText("private server failure", { exact: true }).count(), 0);
    unavailable = false;
    await page.getByRole("button", { name: "Retry", exact: true }).click();
    await page.locator(".ticket-detail-header").waitFor();
    await page.close();
  });
});

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

async function assertThreadLayout(page: Page) {
  const main = page.getByRole("main", { name: "Ticket thread", exact: true });
  const rail = page.getByRole("complementary", { name: "Ticket details" });
  assertEquals(await main.locator(":scope > *").count(), 1);
  assertEquals(await main.locator(".ticket-thread > *").evaluateAll((elements) =>
    elements.map((element) => element.className)
  ), ["ticket-timeline", "ticket-control-form ticket-thread-composer"]);
  assertEquals(await main.locator(".ticket-timeline > :first-child").getAttribute("class"), "ticket-thread-event");
  assertEquals(await main.locator(".ticket-thread-event[data-initial=true] header strong").textContent(), "Mika");
  assert((await main.locator(".ticket-thread-event[data-initial=true] time").textContent())?.trim());
  assertEquals(await main.getByRole("heading", { name: /^(Intent|Timeline)$/ }).count(), 0);
  assertEquals(await main.locator(".ticket-timeline > *").evaluateAll((elements) =>
    elements.every((element, index) => index === 0
      ? element.matches("article.ticket-thread-event[data-initial=true]")
      : index % 2 === 1 ? element.matches(".bevel-line") : element.matches("article.ticket-thread-event"))
  ), true);
  assert(await main.locator(".ticket-thread-composer").evaluate((element) =>
    element.previousElementSibling?.matches(".ticket-timeline") &&
    element.firstElementChild?.matches(".bevel-line")
  ));
  const metadata = ".ticket-handling, .ticket-tags, .ticket-relations, .ticket-orchestrator-worker, .ticket-worker-observation, .ticket-worker-card, .ticket-resources";
  assertEquals(await main.locator(metadata).count(), 0);
  assertEquals(await rail.locator(metadata).count(), 7);
  assertEquals(await page.locator(".ticket-delivery").count(), 0);
  assertEquals(await page.getByText("Delivery and execution", { exact: true }).count(), 0);
  assertEquals(await rail.locator(".ticket-thread-event[data-initial=true], .ticket-thread-event, .ticket-thread-composer").count(), 0);
  for (const selector of [".ticket-orchestrator-worker", ".ticket-worker-observation", ".ticket-worker-card", ".ticket-resources"]) {
    const details = rail.locator(selector);
    assertEquals(await details.evaluate((element) => element.tagName), "DETAILS");
    assertEquals(await details.getAttribute("open"), null);
    assert(await details.locator(":scope > summary").isVisible());
  }
  assertEquals(await rail.locator(".ticket-worker-card > summary").textContent(), "Role assignments and manual Workers");
  assert(/^Repository resources \(\d+\)$/.test((await rail.locator(".ticket-resources > summary").textContent())!));
  const evidence = rail.locator("details").filter({ has: page.locator("summary", { hasText: /^MR requirement evidence$/ }) });
  if (await evidence.count()) assertEquals(await evidence.getAttribute("open"), null);
}

async function assertThreadDividers(page: Page) {
  const lines = page.locator(".ticket-timeline > .bevel-line, .ticket-thread-composer > .bevel-line");
  assertEquals(await lines.count(), await page.locator(".ticket-thread-event").count());
  for (const line of await lines.all()) {
    await line.scrollIntoViewIfNeeded();
    assert(await line.isVisible());
    assert(await line.evaluate((element) => {
      const view = element.ownerDocument.defaultView!;
      const style = view.getComputedStyle(element);
      const bounds = element.getBoundingClientRect();
      const content = element.nextElementSibling!.getBoundingClientRect();
      return element.getAttribute("aria-hidden") === "true" &&
        style.borderTopStyle === "solid" && parseFloat(style.borderTopWidth) > 0 &&
        style.borderTopColor !== "rgba(0, 0, 0, 0)" && style.opacity !== "0" &&
        Math.abs(bounds.left - content.left) < 1 && Math.abs(bounds.right - content.right) < 1;
    }));
  }
}

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
    await page.getByLabel("Comment", { exact: true }).fill(
      "Ordinary retained discussion, not an answer input",
    );
    const eventResponse = page.waitForResponse((response) =>
      response.url().endsWith("/events") &&
      response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Add comment", exact: true })
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
    await page.locator(".ticket-thread-composer").getByLabel("Comment", {
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
    await page.getByRole("button", { name: "Comment and close", exact: true })
      .waitFor();
    const reopened = await detail(base, "T-4");
    assertEquals(reopened.state, "open");
    assertEquals(reopened.assignment_tracking[0].tracking.status, "done");
    assertEquals(reopened.closure_history.length, 2);
    await page.locator(".ticket-thread-composer").getByLabel("Comment", {
      exact: true,
    }).fill("The full requested investigation is delivered");
    await page.locator(".ticket-close-options > summary").click();
    await page.locator(".ticket-thread-composer").getByRole("combobox", {
      name: "Closure reason", exact: true,
    }).selectOption("completed");
    const closeResponse = page.waitForResponse((response) =>
      response.url().endsWith("/case") && response.request().method() === "POST"
    );
    await page.getByRole("button", { name: "Comment and close", exact: true })
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
    assert(!requests.some((r: { path: string }) => r.path.includes("assignment-diagnostics")));
    assertEquals(await page.locator(".ticket-delivery").count(), 0);
    assertEquals(await page.getByText("Delivery and execution", { exact: true }).count(), 0);
    assert(
      !requests.some((r: { path: string }) => /\/(queue|ready|state)$/.test(r.path)),
    );
  });
});

Deno.test("timeline footer comments and closes with one case request, renders the comment once and still accepts closed-case comments", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets/T-1`);
    const composer = page.locator(".ticket-thread-composer");
    await composer.waitFor();
    assertEquals(await page.locator("aside .ticket-thread-composer").count(), 0);
    assertEquals(await page.locator(".ticket-case-card").count(), 0);
    assert(await composer.evaluate((element) => {
      const timeline = element.parentElement?.querySelector(".ticket-timeline");
      return !!timeline && !!element.closest("main") &&
        !!(timeline.compareDocumentPosition(element) & 4);
    }));
    const comment = composer.getByLabel("Comment", { exact: true });
    const add = composer.getByRole("button", { name: "Add comment", exact: true });
    const close = composer.getByRole("button", { name: "Comment and close", exact: true });
    assert(await add.isDisabled());
    assert(await close.isDisabled());
    await comment.fill("   \n  ");
    assert(await add.isDisabled());
    assert(await close.isDisabled());
    const text = "The options are recorded; close this discussion without choosing one.";
    await comment.fill(text);
    assert(await add.isEnabled());
    assert(await close.isEnabled());
    const before = await detail(base, "T-1");
    const response = page.waitForResponse((response) =>
      response.url().endsWith("/case") && response.request().method() === "POST"
    );
    await close.click();
    const request = (await response).request().postDataJSON();
    assertEquals(request.explanation, text);
    assertEquals(request.action, { action: "close", reason: "unknown" });
    assertEquals(request.expected_state, "open");
    assertEquals(request.expected_content_digest, before.content_digest);
    assertEquals(request.expected_closure_operation_key, before.closure_operation_key);
    assert(typeof request.operation_key === "string" && request.operation_key.length > 0);
    await page.locator(".ticket-detail-header").getByText("Case: closed", {
      exact: true,
    }).waitFor();
    const rendered = page.locator(".ticket-timeline").getByText(text, { exact: true });
    await rendered.waitFor();
    assertEquals(await rendered.count(), 1);
    const closed = await detail(base, "T-1");
    assertEquals(closed.state, "closed");
    assertEquals(closed.current_closure.explanation, text);
    assertEquals(closed.events.length, before.events.length + 1);
    assertEquals(closed.events.at(-1).kind, "close");
    assertEquals(closed.events.at(-1).body, text);
    assertEquals(closed.events.at(-1).reason, text);
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    assertEquals(requests.filter((row: { path: string }) => row.path.endsWith("/case")).length, 1);
    assertEquals(requests.filter((row: { path: string }) => row.path.endsWith("/events")).length, 0);
    assertEquals(await comment.inputValue(), "");
    assert(await add.isDisabled());
    const reopen = composer.getByRole("button", { name: "Reopen ticket", exact: true });
    assert(await reopen.isDisabled());
    await comment.fill("A retained comment after closure does not reopen the case.");
    const eventResponse = page.waitForResponse((response) =>
      response.url().endsWith("/events") && response.request().method() === "POST"
    );
    await add.click();
    assertEquals((await eventResponse).request().postDataJSON(), {
      role: "comment",
      body: "A retained comment after closure does not reopen the case.",
    });
    await page.locator(".ticket-timeline").getByText(
      "A retained comment after closure does not reopen the case.",
      { exact: true },
    ).waitFor();
    assertEquals((await detail(base, "T-1")).state, "closed");
  });
});

Deno.test("ambiguous comment-and-close retries exact case JSON without a duplicate timeline event", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets/T-1`);
    const composer = page.locator(".ticket-thread-composer");
    await composer.waitFor();
    const text = "Retain this close comment exactly once, even if the response is lost.";
    const before = await detail(base, "T-1");
    await composer.getByLabel("Comment", { exact: true }).fill(text);
    await control(base, { failure: "unknown_saved" });
    await composer.getByRole("button", { name: "Comment and close", exact: true }).click();
    await page.getByRole("alert").filter({ hasText: /Outcome unknown/ }).waitFor();
    assertEquals(await composer.getByRole("button", {
      name: "Refresh and reconcile case", exact: true,
    }).count(), 1);
    await composer.getByRole("button", { name: "Retry exact case decision", exact: true }).click();
    await page.locator(".ticket-detail-header").getByText("Case: closed", {
      exact: true,
    }).waitFor();
    const rendered = page.locator(".ticket-timeline").getByText(text, { exact: true });
    await rendered.waitFor();
    assertEquals(await rendered.count(), 1);
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    const attempts = requests.filter((row: { path: string }) => row.path.endsWith("/case"));
    assertEquals(attempts.length, 2);
    assertEquals(attempts[0].body, attempts[1].body);
    assertEquals(requests.filter((row: { path: string }) => row.path.endsWith("/events")).length, 0);
    const closed = await detail(base, "T-1");
    assertEquals(closed.events.length, before.events.length + 1);
    assertEquals(closed.closure_history.length, before.closure_history.length + 1);
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

Deno.test("initial and later posts use the same editor and preserve authorship while title edit remains metadata-only", async () => {
  await withFixture(async (browser, base) => {
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets/T-1`);
    await page.locator(".ticket-detail-header").waitFor();
    const composer = page.getByRole("form", { name: "Add a comment" });
    await composer.getByLabel("Comment", { exact: true }).fill("A later discussion post");
    await composer.getByRole("button", { name: "Add comment", exact: true }).click();
    await page.locator(".ticket-timeline").getByText("A later discussion post", { exact: true }).waitFor();
    for (const name of ["Initial post", "Discussion post"]) {
      const article = page.getByRole("article", { name, exact: true });
      const originalTime = await article.locator("time").first().getAttribute("datetime");
      const originalAuthor = await article.locator("header strong").textContent();
      await article.getByRole("button", { name: "Edit post", exact: true }).click();
      await article.getByLabel("Post body", { exact: true }).fill(`Revised ${name}`);
      await article.getByRole("button", { name: "Save post", exact: true }).click();
      await article.getByText(`Revised ${name}`, { exact: true }).waitFor();
      assertEquals(await article.locator("time").first().getAttribute("datetime"), originalTime);
      assertEquals(await article.locator("header strong").textContent(), originalAuthor);
      assert((await article.locator(".ticket-post-edited").textContent())?.includes("by Rin"));
      assertEquals(await page.locator(".ticket-timeline article").count(), 2);
    }
    const rail = page.getByRole("complementary", { name: "Ticket details" });
    await rail.getByRole("button", { name: "Edit title", exact: true }).click();
    assertEquals(await rail.getByLabel("Post body", { exact: true }).count(), 0);
    await rail.getByLabel("Title", { exact: true }).fill("A clearer Ticket title");
    await rail.getByRole("button", { name: "Save title", exact: true }).click();
    await page.getByRole("heading", { name: "A clearer Ticket title", exact: true }).waitFor();
    const requests = await (await fetch(`${base}/fixture-state`)).json();
    const edits = requests.filter((row: { path: string; method?: string }) => row.method === "PATCH" && row.path.includes("/posts/"));
    assertEquals(edits.length, 2);
    for (const edit of edits) assertEquals(Object.keys(edit.body).sort(), ["body", "expected_body_digest"]);
    const metadata = requests.findLast((row: { body?: { title?: string } }) => row.body?.title === "A clearer Ticket title");
    assertEquals(metadata.body, { title: "A clearer Ticket title" });
    assertEquals(await page.locator(".ticket-timeline article").count(), 2);
    await page.close();
  });
});

Deno.test("canonical thread renders long initial post and chronological multiline events once with content-aligned dividers", async () => {
  await withFixture(async (browser, base) => {
    const output = artifactPath("thread");
    await Deno.mkdir(output, { recursive: true });
    const paragraphs = Array.from({ length: 8 }, (_, index) =>
      `Initial post paragraph ${index + 1}: ` + "The requested investigation preserves context before any discussion. ".repeat(5)
    );
    const events = ["Initial findings", "Follow-up decision", "Final investigation result"].map((heading, index) => ({
      is_initial: false, edited_at: null, edited_by: null, body_digest: "d".repeat(64), body_truncated: false, editable: index !== 1,
      sequence: index + 1,
      kind: index === 1 ? "decision" : "comment",
      heading,
      author: `Thread author ${index + 1}`,
      at: `2026-10-13T0${index + 1}:00:00Z`,
      body: `${heading} first paragraph.\n\n${heading} second paragraph with enough detail to wrap on narrow screens.\n\n- ${heading} evidence one\n- ${heading} evidence two`,
      reason: null,
      from: null,
      to: null,
      references: [],
      attributes: {},
      event_ref: `thread-fixture-${index + 1}`,
    }));
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme, reducedMotion: "reduce" });
        await page.route("**/api/w/tickets/tickets/T-1", async (route) => {
          const response = await route.fetch();
          const ticket = await response.json();
          await route.fulfill({ response, json: {
            ...ticket,
            body: "Compatibility body must not render",
            initial_post: { ...ticket.initial_post, body: paragraphs.join("\n\n"), at: "2026-10-12T12:00:00Z" },
            created_at: "2026-10-12T12:00:00Z",
            updated_at: events.at(-1)!.at,
            events,
            event_count: events.length,
          } });
        });
        await page.goto(`${base}/w/tickets/tickets/T-1`);
        await page.locator(".ticket-detail-header").waitFor();
        await assertThreadLayout(page);
        const timeline = page.locator(".ticket-timeline");
        assertEquals(await timeline.locator("article header strong").allTextContents(), ["Mika", ...events.map((event) => event.author)]);
        const expectedDates = ["2026-10-12T12:00:00Z", ...events.map((event) => event.at)];
        assertEquals(await timeline.locator("article header time").allTextContents(), await page.evaluate((dates) => dates.map((date) => new Date(date).toLocaleString()), expectedDates));
        for (const paragraph of paragraphs) assertEquals(await timeline.getByText(paragraph, { exact: true }).count(), 1);
        for (const event of events) {
          const article = timeline.locator(".ticket-thread-event").filter({ has: page.locator("header strong", { hasText: event.author }) });
          assertEquals(await article.count(), 1);
          assertEquals(await article.getByText(`${event.heading} first paragraph.`, { exact: true }).count(), 1);
          assertEquals(await article.getByText(`${event.heading} second paragraph with enough detail to wrap on narrow screens.`, { exact: true }).count(), 1);
          assertEquals(await article.locator("li").count(), 2);
        }
        await assertThreadDividers(page);
        assertEquals(await page.evaluate("document.documentElement.scrollWidth > innerWidth"), false);
        await page.screenshot({ path: `${output}/${colorScheme}-${width}-full.png`, fullPage: true });
        await page.locator(".ticket-thread-event").first().scrollIntoViewIfNeeded();
        await page.screenshot({ path: `${output}/${colorScheme}-${width}-events.png` });
        await page.locator(".ticket-thread-composer").scrollIntoViewIfNeeded();
        await page.screenshot({ path: `${output}/${colorScheme}-${width}-composer.png` });
        await timeline.locator("article").first().getByRole("button", { name: "Edit post", exact: true }).click();
        const description = timeline.locator(".ticket-thread-event[data-initial=true]");
        await description.locator(".ticket-editor").waitFor();
        assertEquals(await description.getByLabel("Post body", { exact: true }).inputValue(), paragraphs.join("\n\n"));
        assertEquals(await page.locator("aside .ticket-editor").count(), 0);
        await page.getByRole("button", { name: "Cancel post edit", exact: true }).click();
        await description.getByText(paragraphs[0], { exact: true }).waitFor();
        await page.close();
      }
    }
  });
});

Deno.test("Ticket metadata stays in the rail with flat links, wrapping labels and keyboard navigation", async () => {
  await withFixture(async (browser, base) => {
    const output = artifactPath("metadata");
    await Deno.mkdir(output, { recursive: true });
    for (const colorScheme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme });
        await page.route("**/api/w/tickets/tickets/T-1*", async (route) => {
          const response = await route.fetch();
          const body = await response.json();
          if (body.resource_key === "T-1" && body.relations) {
            body.tags[0].name = "Classification-with-a-long-name-".repeat(4);
            body.relations.outgoing = [{ at: "2026-01-01T00:00:00Z", author: "fixture-owner", kind: "related", target: "internal-T-2", target_resource_key: "T-2", ticket_id: body.id, note: "Related investigation with a long note. ".repeat(5) }];
          }
          await route.fulfill({ response, json: body });
        });
        await page.goto(`${base}/w/tickets/tickets/T-1`);
        const rail = page.getByRole("complementary", { name: "Ticket details" });
        const link = rail.getByRole("link", { name: /T-2/ });
        await link.waitFor();
        assertEquals(await link.getAttribute("href"), "/w/tickets/tickets/T-2");
        assertEquals(await page.locator(".ticket-detail-main .ticket-tags, .ticket-detail-main .ticket-relations").count(), 0);
        assertEquals(await page.evaluate("document.documentElement.scrollWidth > innerWidth"), false);
        assert(await rail.locator(".ticket-tag-chip, .ticket-relations-list a").evaluateAll((elements) => elements.every((element) => {
          const style = element.ownerDocument.defaultView!.getComputedStyle(element);
          return style.borderTopWidth === "0px" && style.borderRadius === "0px" && style.backgroundColor === "rgba(0, 0, 0, 0)";
        })));
        await link.focus();
        assert(await link.evaluate((element) => element === element.ownerDocument.activeElement));
        assert(await link.evaluate((element) => element.ownerDocument.defaultView!.getComputedStyle(element).outlineStyle !== "none"));
        await page.screenshot({ path: `${output}/${colorScheme}-${width}.png` });
        await link.press("Enter");
        await page.waitForURL(/\/tickets\/T-2/);
        await page.getByRole("heading", { name: "Read-only investigation", exact: true }).waitFor();
        await page.close();
      }
    }
  });
});

Deno.test("case and handling matrix renders without overflow in both themes and empty/error/unknown states stay distinct", async () => {
  await withFixture(async (browser, base) => {
    const output = artifactPath("matrix");
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
              ["tickets", "tickets/T-2", ".ticket-detail-header"],
              ["tickets", "tickets/T-3", ".ticket-detail-header"],
              ["tickets", "tickets/T-4", ".ticket-detail-header"],
              [
                "tickets-readonly",
                "tickets/T-2",
                ".ticket-detail-header",
              ],
              [
                "tickets-running",
                "tickets/T-2",
                ".ticket-detail-header",
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
            if (route.startsWith("tickets/")) {
              await assertThreadLayout(page);
              await assertThreadDividers(page);
              const rail = page.getByRole("complementary", { name: "Ticket details" });
              assertEquals(await rail.locator(".ticket-tags").count(), 1);
              assertEquals(await rail.getByRole("heading", { name: "Relations", exact: true }).count(), 1);
              assertEquals(await page.locator(".ticket-detail-main .ticket-tags, .ticket-detail-main .ticket-relations").count(), 0);
              const sections = page.locator(".ticket-detail-page .ticket-detail-section, .ticket-detail-page .ticket-control-card");
              assert(await sections.evaluateAll((elements) => elements.every((element) => {
                const style = element.ownerDocument.defaultView!.getComputedStyle(element);
                return [style.borderTopWidth, style.borderRightWidth, style.borderBottomWidth, style.borderLeftWidth, style.borderRadius].every((value) => value === "0px") && style.backgroundColor === "rgba(0, 0, 0, 0)";
              })));
              const mainBounds = await page.locator(".ticket-detail-main").boundingBox();
              const railBounds = await rail.boundingBox();
              assert(mainBounds && railBounds);
              if (width === 1440) assert(railBounds.x >= mainBounds.x + mainBounds.width);
              else assert(railBounds.y >= mainBounds.y + mainBounds.height);
            }
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
            if (["tickets/T-2", "tickets/T-3"].includes(route)) {
              await page.locator("aside .ticket-worker-observation > summary").click();
              await page.locator("aside .ticket-worker-observation strong").waitFor();
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
                await page.locator("aside .ticket-orchestrator-worker > summary").click();
                await page.locator("aside .ticket-orchestrator-worker strong").waitFor();
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
            if (workspace === "tickets" && ["tickets/T-1", "tickets/T-4"].includes(route)) {
              const composer = page.locator(".ticket-thread-composer");
              await composer.waitFor();
              assertEquals(await page.locator("aside .ticket-thread-composer").count(), 0);
              await composer.scrollIntoViewIfNeeded();
              await page.screenshot({
                path: `${output}/${colorScheme}-${width}-${route.replaceAll("/", "-")}-footer.png`,
              });
            }
          }
          assertEquals(errors, []);
        } finally {
          await context.close();
        }
      }
    }
  });
});
