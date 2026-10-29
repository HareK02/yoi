// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import type { TicketDetail } from "#lib/generated/ticket-api.ts";
import { fixtureDetail, fixturePost } from "#lib/workspace/home/dashboard.test-fixtures.ts";
import PageContent from "./PageContent.svelte";

// 本番の詳細画面とrequest組立てを接続し、HTTP境界だけを代替する。
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const detailPath = "/api/w/team/tickets/T-1";
const casePath = "/workspaces/team/tickets/T-1/case";
function pageData(ticket: TicketDetail) {
  return { workspaceId: "team", ticketId: "T-1", ticket: { data: ticket, error: null },
    repositories: { data: { items: [] }, error: null }, orchestrator: { data: null, error: null },
  } as unknown as ComponentProps<typeof PageContent>["data"];
}
function initial(): TicketDetail { return fixtureDetail("T-1"); }
function commentEvent(body: string, kind = "comment") {
  return { sequence: 1, kind, heading: kind === "close" ? "Closed" : "Comment", author: "User",
    at: "2026-10-13T00:00:00Z", body, reason: kind === "close" ? body : null,
    from: kind === "close" ? "open" : null, to: kind === "close" ? "closed" : null, references: [],
    attributes: {}, event_ref: "event-1", is_initial: false, body_truncated: false,
    body_digest: "d".repeat(64), editable: kind === "comment", edited_at: null, edited_by: null,
  };
}
function closed(ticket: TicketDetail, body: string): TicketDetail {
  return { ...ticket, state: "closed", closure_operation_key: "saved-close",
    events: [commentEvent(body, "close")], event_count: 1 };
}
function savedSummary(ticket: TicketDetail) {
  return { id: ticket.id, resource_key: ticket.resource_key, slug: "fixture", kind: "task",
    labels: [], tags: ticket.tags, title: ticket.title, priority: ticket.priority,
    status: "Closed", workflow_state: "closed", workflow_state_explicit: true };
}
function installFetch(handle: (path: string, init?: RequestInit) => Response | Promise<Response>) {
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    if (path.endsWith("/assignment-tracking/assignments?limit=20")) return Response.json({ items: [], next_assignment_id: null });
    if (path.endsWith("/tags")) return Response.json([]);
    return await handle(path, init);
  });
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
}
function mount(ticket = initial()) {
  const view = render(PageContent, { data: pageData(ticket) });
  const form = view.getByRole("form", { name: "Add a comment" });
  return { ...view, form, composer: within(form) };
}
async function typeComment(view: ReturnType<typeof mount>, body: string) {
  await fireEvent.input(view.composer.getByRole("textbox", { name: "Comment" }), { target: { value: body } });
}

test("canonical initial post and discussion share markup and editing while all metadata stays in the sidebar", async () => {
  installFetch(() => { throw new Error("Unexpected request"); });
  const ticket = { ...initial(), body: "Compatibility body must not render", initial_post: fixturePost("Original user request"),
    events: [commentEvent("Later discussion"), { ...commentEvent("Immutable closure", "close"), event_ref: "close-2", sequence: 2 }], event_count: 2 };
  const view = mount(ticket);
  const main = view.getByRole("main", { name: "Ticket thread" });
  const rail = view.getByRole("complementary", { name: "Ticket details" });
  const timeline = main.querySelector(".ticket-timeline")!;
  const first = within(main).getByRole("article", { name: "Initial post" });
  const later = within(main).getByRole("article", { name: "Discussion post" });
  expect(timeline.firstElementChild).toBe(first);
  expect(first.className).toBe(later.className);
  expect(within(first).getByText("Original user request")).toBeTruthy();
  expect(view.queryByText("Compatibility body must not render")).toBeNull();
  expect(within(first).getByText("Mika")).toBeTruthy();
  expect(first.querySelector("time")?.dateTime).toBe(ticket.initial_post.at);
  expect(within(main).getAllByRole("button", { name: "Edit post" })).toHaveLength(2);
  expect(Array.from(timeline.children).map((element) => element.matches(".bevel-line") ? "divider" : element.className))
    .toEqual(["ticket-thread-event", "divider", "ticket-thread-event", "divider", "ticket-thread-event"]);
  expect(timeline.nextElementSibling).toBe(view.form);
  expect(view.form.firstElementChild?.matches(".bevel-line")).toBe(true);
  for (const selector of [".ticket-metadata", ".ticket-handling", ".ticket-tags", ".ticket-relations", ".ticket-worker-observation", ".ticket-worker-card", ".ticket-resources"]) {
    expect(main.querySelector(selector)).toBeNull();
    expect(rail.querySelector(selector)).not.toBeNull();
  }
  expect(view.container.querySelector(".ticket-delivery")).toBeNull();
  expect(view.queryByText("Delivery and execution")).toBeNull();
  expect(rail.querySelector(".ticket-orchestrator-worker")).toBeNull();
  for (const selector of [".ticket-worker-observation", ".ticket-worker-card", ".ticket-resources"]) {
    expect(rail.querySelector(selector)?.hasAttribute("open")).toBe(false);
  }
  expect(within(rail).getByText("Created by")).toBeTruthy();
  expect(within(rail).getByText("Mika")).toBeTruthy();
  await fireEvent.click(within(first).getByRole("button", { name: "Edit post" }));
  expect(within(first).getByRole("textbox", { name: "Post body" })).toHaveProperty("value", ticket.initial_post.body);
  expect(within(first).queryByRole("textbox", { name: "Title" })).toBeNull();
});

test("title editing only patches metadata and leaves the initial post untouched", async () => {
  const ticket = initial();
  const requests: unknown[] = [];
  installFetch((path, init) => {
    requests.push({ path, method: init?.method, body: JSON.parse(String(init?.body)) });
    return Response.json({ ...ticket, title: "A clearer title" });
  });
  const view = mount(ticket);
  const rail = view.getByRole("complementary", { name: "Ticket details" });
  await fireEvent.click(within(rail).getByRole("button", { name: "Edit title" }));
  await fireEvent.input(within(rail).getByRole("textbox", { name: "Title" }), { target: { value: "A clearer title" } });
  expect(within(rail).queryByRole("textbox", { name: "Post body" })).toBeNull();
  await fireEvent.click(within(rail).getByRole("button", { name: "Save title" }));
  await waitFor(() => expect(view.getByRole("heading", { name: "A clearer title" })).toBeTruthy());
  expect(requests).toEqual([{ path: detailPath, method: "PATCH", body: { title: "A clearer title" } }]);
  expect(within(view.getByRole("article", { name: "Initial post" })).getByText(ticket.initial_post.body!)).toBeTruthy();
});

test.each([true, false])("initial=%s edit uses the post identity and observed body digest, not item edit", async (isInitial) => {
  const ticket = { ...initial(), events: [commentEvent("Discussion")], event_count: 1 };
  const post = isInitial ? ticket.initial_post : ticket.events[0];
  const requests: unknown[] = [];
  installFetch((path, init) => {
    requests.push({ path, method: init?.method, body: JSON.parse(String(init?.body)) });
    const updated = { ...post, body: "Revised post", edited_by: "Rin", edited_at: "2026-10-13T12:00:00Z", body_digest: "e".repeat(64) };
    return Response.json({ ...ticket, initial_post: isInitial ? updated : ticket.initial_post, events: isInitial ? ticket.events : [updated] });
  });
  const view = mount(ticket);
  const article = view.getByRole("article", { name: isInitial ? "Initial post" : "Discussion post" });
  await fireEvent.click(within(article).getByRole("button", { name: "Edit post" }));
  await fireEvent.input(within(article).getByRole("textbox", { name: "Post body" }), { target: { value: "Revised post" } });
  await fireEvent.click(within(article).getByRole("button", { name: "Save post" }));
  await waitFor(() => expect(within(article).getByText("Revised post")).toBeTruthy());
  expect(requests).toEqual([{ path: `${detailPath}/posts/${post.event_ref}`, method: "PATCH", body: { body: "Revised post", expected_body_digest: post.body_digest } }]);
  expect(within(article).getByText(post.author!)).toBeTruthy();
  expect(article.querySelector("time")?.dateTime).toBe(post.at);
  expect(article.querySelector(".ticket-post-edited")?.textContent).toContain("by Rin");
  expect(view.container.querySelector(".ticket-timeline")?.textContent).not.toMatch(/Created|Item edit/);
});

test("truncated initial post uses the full-read route and never saves preview text", async () => {
  const ticket = initial();
  ticket.initial_post = { ...ticket.initial_post, body: "Preview", body_truncated: true };
  const full = { ...ticket.initial_post, body: "The full initial post with its ending", body_truncated: false, body_digest: "f".repeat(64) };
  const writes: unknown[] = [];
  installFetch((path, init) => {
    expect(path).toBe(`${detailPath}/posts/${ticket.initial_post.event_ref}`);
    if (init?.method === "PATCH") {
      writes.push(JSON.parse(String(init.body)));
      return Response.json({ ...ticket, initial_post: full });
    }
    return Response.json(full);
  });
  const view = mount(ticket);
  const article = within(view.getByRole("article", { name: "Initial post" }));
  await fireEvent.click(article.getByRole("button", { name: "Edit post" }));
  await waitFor(() => expect(article.getByRole("textbox", { name: "Post body" })).toHaveProperty("value", full.body));
  await fireEvent.click(article.getByRole("button", { name: "Save post" }));
  await waitFor(() => expect(writes).toEqual([{ body: full.body, expected_body_digest: full.body_digest }]));
});

test("HTTP conflict does not append an edit event, overwrite the displayed post, or lose the draft", async () => {
  const ticket = initial();
  let writes = 0;
  installFetch((path, init) => {
    expect(path).toBe(`${detailPath}/posts/${ticket.initial_post.event_ref}`);
    expect(init?.method).toBe("PATCH");
    writes++;
    return new Response("The body has changed", { status: 409 });
  });
  const view = mount(ticket);
  const article = within(view.getByRole("article", { name: "Initial post" }));
  await fireEvent.click(article.getByRole("button", { name: "Edit post" }));
  await fireEvent.input(article.getByRole("textbox", { name: "Post body" }), { target: { value: "Keep my changes" } });
  await fireEvent.click(article.getByRole("button", { name: "Save post" }));
  await waitFor(() => expect(article.getByRole("alert").textContent).toContain("Post update conflict"));
  expect(article.getByRole("textbox", { name: "Post body" })).toHaveProperty("value", "Keep my changes");
  expect(article.getByRole("button", { name: "Save post" })).toHaveProperty("disabled", true);
  expect(view.container.querySelectorAll(".ticket-timeline article")).toHaveLength(1);
  expect(writes).toBe(1);
});

test("comment composer follows the timeline, and Add comment does not close the Ticket", async () => {
  const posts: { path: string; body: unknown }[] = [];
  const ticket = initial();
  installFetch((path, init) => {
    posts.push({ path, body: JSON.parse(String(init?.body)) });
    expect(path).toBe(`${detailPath}/events`);
    return Response.json({ ...ticket, events: [commentEvent("Still investigating")], event_count: 1 });
  });
  const view = mount(ticket);
  const timeline = view.container.querySelector(".ticket-timeline")!;
  expect(timeline.nextElementSibling).toBe(view.form);
  expect(view.container.querySelector("aside .ticket-thread-composer")).toBeNull();
  expect(view.queryByText("Case decision", { exact: true })).toBeNull();
  expect(view.composer.getByRole("button", { name: "Add comment" })).toHaveProperty("disabled", true);
  expect(view.composer.getByRole("button", { name: "Comment and close" })).toHaveProperty("disabled", true);
  await typeComment(view, "Still investigating");
  await fireEvent.click(view.composer.getByRole("button", { name: "Add comment" }));
  await waitFor(() => expect(within(timeline as HTMLElement).getByText("Still investigating")).toBeTruthy());
  expect(posts).toEqual([{ path: `${detailPath}/events`, body: { role: "comment", body: "Still investigating" } }]);
  expect(view.getByText("Case: open")).toBeTruthy();
  expect(view.composer.getByRole("textbox", { name: "Comment" })).toHaveProperty("value", "");
});

test("Comment and close saves one explanation with the case change and renders it once in the timeline", async () => {
  const ticket = initial();
  const writes: { path: string; body: Record<string, unknown> }[] = [];
  installFetch((path, init) => {
    if (path === casePath) {
      writes.push({ path, body: JSON.parse(String(init?.body)) });
      return Response.json(savedSummary(ticket));
    }
    expect(path).toBe(detailPath);
    return Response.json(closed(ticket, "Delivered the findings"));
  });
  const view = mount(ticket);
  await typeComment(view, "Delivered the findings");
  await fireEvent.change(view.composer.getByLabelText("Closure reason"), { target: { value: "completed" } });
  await fireEvent.click(view.composer.getByRole("button", { name: "Comment and close" }));
  await waitFor(() => expect(view.getByText("Case: closed")).toBeTruthy());
  await waitFor(() => expect(view.container.querySelector(".ticket-timeline")?.textContent).toContain("Delivered the findings"));
  expect(writes).toHaveLength(1);
  expect(writes[0]).toEqual({ path: casePath, body: {
    action: { action: "close", reason: "completed" }, explanation: "Delivered the findings",
    expected_content_digest: ticket.content_digest, expected_state: "open", expected_closure_operation_key: null,
    operation_key: expect.any(String), references: [],
  } });
  expect(within(view.container.querySelector(".ticket-timeline") as HTMLElement).getAllByText("Delivered the findings")).toHaveLength(1);
  expect(view.composer.queryByRole("button", { name: "Comment and close" })).toBeNull();
  expect(view.composer.getByRole("button", { name: "Reopen ticket" })).toBeTruthy();
});

test("lost Close response retains the exact request and preserves later edits on retry", async () => {
  const ticket = initial();
  const requests: string[] = [];
  installFetch((path, init) => {
    if (path === casePath) {
      requests.push(String(init?.body));
      if (requests.length === 1) throw new Error("Response lost");
      return Response.json(savedSummary(ticket));
    }
    expect(path).toBe(detailPath);
    return Response.json(closed(ticket, "Original close comment"));
  });
  const view = mount(ticket);
  await typeComment(view, "Original close comment");
  await fireEvent.click(view.composer.getByRole("button", { name: "Comment and close" }));
  await waitFor(() => expect(view.composer.getByRole("alert").textContent).toContain("Outcome unknown"));
  expect(view.composer.getByRole("button", { name: "Add comment" })).toHaveProperty("disabled", true);
  await typeComment(view, "A later draft");
  await fireEvent.click(view.composer.getByRole("button", { name: "Retry exact case decision" }));
  await waitFor(() => expect(view.getByText("Case: closed")).toBeTruthy());
  expect(requests).toHaveLength(2);
  expect(requests[1]).toBe(requests[0]);
  expect(view.composer.getByRole("textbox", { name: "Comment" })).toHaveProperty("value", "A later draft");
});

test("saved Close followed by a failed refresh reports success separately and retries only the read", async () => {
  const ticket = initial();
  let writes = 0, reads = 0;
  installFetch((path) => {
    if (path === casePath) { writes++; return Response.json(savedSummary(ticket)); }
    expect(path).toBe(detailPath);
    if (++reads === 1) throw new Error("Read failed");
    return Response.json(closed(ticket, "Saved once"));
  });
  const view = mount(ticket);
  await typeComment(view, "Saved once");
  await fireEvent.click(view.composer.getByRole("button", { name: "Comment and close" }));
  await waitFor(() => expect(view.composer.getByRole("alert").textContent).toContain("saved, but the timeline"));
  expect(view.getByText("Case: closed")).toBeTruthy();
  expect(view.composer.queryByRole("button", { name: "Retry exact case decision" })).toBeNull();
  expect(view.composer.getByRole("textbox", { name: "Comment" })).toHaveProperty("value", "");
  await fireEvent.click(view.composer.getByRole("button", { name: "Refresh thread" }));
  await waitFor(() => expect(view.composer.queryByRole("alert")).toBeNull());
  expect(writes).toBe(1);
  expect(reads).toBe(2);
});

test("rejected Close keeps the draft and requires reconciliation rather than silently replacing its observation", async () => {
  const ticket = initial();
  let writes = 0;
  installFetch((path) => {
    if (path === casePath) { writes++; return new Response("stale", { status: 409 }); }
    expect(path).toBe(detailPath);
    return Response.json(ticket);
  });
  const view = mount(ticket);
  await typeComment(view, "Keep this draft");
  await fireEvent.click(view.composer.getByRole("button", { name: "Comment and close" }));
  await waitFor(() => expect(view.composer.getByRole("alert").textContent).toContain("Update conflict"));
  expect(view.getByText("Case: open")).toBeTruthy();
  expect(view.composer.getByRole("textbox", { name: "Comment" })).toHaveProperty("value", "Keep this draft");
  expect(view.composer.getByRole("button", { name: "Comment and close" })).toHaveProperty("disabled", true);
  await fireEvent.click(view.composer.getByRole("button", { name: "Refresh and reconcile case" }));
  await waitFor(() => expect(view.composer.getByRole("button", { name: "Comment and close" })).toHaveProperty("disabled", false));
  expect(writes).toBe(1);
});
