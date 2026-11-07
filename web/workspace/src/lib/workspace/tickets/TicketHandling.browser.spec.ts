// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { TicketAssignmentTrackingProjection, TicketDetail } from "#lib/generated/ticket-api.ts";
import { fixtureDetail } from "#lib/workspace/home/dashboard.test-fixtures.ts";
import TicketHandling from "./TicketHandling.svelte";

afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const assignmentsPath = "/workspaces/team/tickets/internal-T-1/assignment-tracking/assignments?limit=20";
const assignmentPath = "/api/w/team/tickets/internal-T-1/assignments/orchestrator";
const answerPath = "/api/w/team/tickets/internal-T-1/assignment-answer";
function assigned(status: "waiting" | "done" = "waiting"): TicketDetail {
  const row: TicketAssignmentTrackingProjection = {
    assignment_id: "assignment-1", role: "orchestrator", ticket_id: "internal-T-1", workspace_id: "team",
    assigned_at: "2026-10-13T09:00:00Z", assigned_by: "Mika", current: true,
    principal: { kind: "subject", subject_id: "subject-a" }, last_event_id: "handling-1",
    tracking: { status, waiting_reason: "Choose the policy", progress: "Compared options", next_action: "Confirm the choice", result: "Comparison delivered" },
  };
  return { ...fixtureDetail("T-1"), assignment_tracking: [row],
    assignments: [{ assignment_id: row.assignment_id, role: row.role, principal: row.principal, assigned_at: row.assigned_at, assigned_by: row.assigned_by }],
    action_eligibility: { ...fixtureDetail("T-1").action_eligibility, can_assign_orchestrator: true },
  };
}
function installFetch(handle: (path: string, init?: RequestInit) => Response | Promise<Response> = () => { throw new Error("Unexpected request"); }, emptyAssignments = true) {
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    if (path.includes("/subjektiv/subjects")) return Response.json(subjectPage);
    if (emptyAssignments && path === assignmentsPath) return Response.json({ items: [], next_assignment_id: null });
    return await handle(path, init);
  });
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
}
function mount(ticket = assigned(), refresh = vi.fn(async () => {})) {
  return { ...render(TicketHandling, { workspaceId: "team", ticket, refresh }), refresh };
}
function expectNoDiagnostics(fetcher: ReturnType<typeof installFetch>, view: ReturnType<typeof mount>) {
  expect(view.container.querySelector(".ticket-delivery")).toBeNull();
  expect(view.queryByText("Delivery and execution")).toBeNull();
  expect(view.queryByRole("button", { name: /delivery diagnostics/i })).toBeNull();
  expect(fetcher.mock.calls.map(([input]) => String(input)).some((path) => path.includes("assignment-diagnostics"))).toBe(false);
}

test("mount retains assignment and self-reported handling, progress and result without delivery UI or diagnostics reads", async () => {
  const fetcher = installFetch();
  const view = mount();
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(view.getByText("Self-reported handling: waiting")).toBeTruthy();
  for (const text of ["Choose the policy", "Compared options", "Confirm the choice", "Comparison delivered", "Assignment details"]) expect(view.getByText(text)).toBeTruthy();
  expect(view.container.querySelector(".ticket-subject-worker")).not.toBeNull();
  expectNoDiagnostics(fetcher, view);
});

test("assignment save refreshes assignment history without diagnostics or delivery/execution notice", async () => {
  const fetcher = installFetch((path, init) => {
    expect(path).toBe(assignmentPath);
    expect(init?.method).toBe("PUT");
    return Response.json({ ticket_id: "internal-T-1", workspace_id: "team", assignment: {
      assignment_id: "assignment-2", assigned_at: "2026-10-13T10:00:00Z", assigned_by: "Mika", role: "orchestrator",
      ticket_id: "internal-T-1", workspace_id: "team", principal: { kind: "subject", subject_id: "subject-a" },
    } });
  });
  const view = mount(assigned("done"));
  await chooseSubject(view);
  await fireEvent.click(view.getByRole("button", { name: "Request again (new assignment)" }));
  await waitFor(() => {
    expect(fetcher).toHaveBeenCalledTimes(4);
    expect(view.queryByText("Loading assignment history…")).toBeNull();
  });
  expect(view.refresh).toHaveBeenCalledTimes(1);
  expect(view.getByRole("status").textContent).toBe("Assignment operation saved: assignment-2.");
  expectNoDiagnostics(fetcher, view);
});

test("answer save keeps handling unchanged and refreshes history without diagnostics reads", async () => {
  const fetcher = installFetch((path, init) => {
    expect(path).toBe(answerPath);
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toMatchObject({ assignment_id: "assignment-1", expected_last_event_id: "handling-1", body: "Use read-only comparison" });
    return Response.json({ assignment_id: "assignment-1", input_event_id: "input-1", event_index: 1 });
  });
  const view = mount();
  await fireEvent.input(view.getByRole("textbox", { name: "Answer / continue this handling" }), { target: { value: "Use read-only comparison" } });
  await fireEvent.click(view.getByRole("button", { name: "Send answer to this assignment" }));
  await waitFor(() => {
    expect(fetcher).toHaveBeenCalledTimes(4);
    expect(view.queryByText("Loading assignment history…")).toBeNull();
  });
  expect(view.refresh).toHaveBeenCalledTimes(1);
  expect(view.getByRole("status").textContent).toContain("Handling status is unchanged.");
  expect(view.getByText("Self-reported handling: waiting")).toBeTruthy();
  expectNoDiagnostics(fetcher, view);
});

test("uncertain assignment reconciliation only refreshes the projection and assignment history", async () => {
  const fetcher = installFetch(() => { throw new Error("Response lost"); });
  const view = mount(assigned("done"));
  await chooseSubject(view);
  await fireEvent.click(view.getByRole("button", { name: "Request again (new assignment)" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("Outcome unknown"));
  await fireEvent.click(view.getByRole("button", { name: "Refresh and reconcile assignment" }));
  await waitFor(() => {
    expect(fetcher).toHaveBeenCalledTimes(4);
    expect(view.queryByText("Loading assignment history…")).toBeNull();
  });
  expect(view.refresh).toHaveBeenCalledTimes(1);
  expect(view.queryByRole("alert")).toBeNull();
  expectNoDiagnostics(fetcher, view);
});

const trackingRoot = "/workspaces/team/tickets/internal-T-1/assignment-tracking";
function historicalDone(action: "replaced" | "unassigned" = "replaced"): TicketAssignmentTrackingProjection {
  const row = assigned("done").assignment_tracking[0];
  return { ...row, current: false, release: {
    action, actor: "Mika", at: "2026-10-13T10:00:00Z", event_id: "release-1",
    replacement_assignment_id: action === "replaced" ? "assignment-2" : null,
    last_tracking: row.tracking,
  } };
}

test("paged retained done history preserves its result and replacement while Answer uses only current waiting observation", async () => {
  const old = historicalDone();
  const current = { ...assigned().assignment_tracking[0], assignment_id: "assignment-2", last_event_id: "handling-2",
    assigned_at: "2026-10-13T10:00:00Z", tracking: { status: "waiting" as const, waiting_reason: "Choose the new policy" } };
  const ticket = { ...assigned(), assignment_tracking: [current], assignments: [{ ...assigned().assignments[0], assignment_id: current.assignment_id }] };
  const historyEntry = (event_id: string, tracking: TicketAssignmentTrackingProjection["tracking"]) => ({
    event_id, tracking, at: "2026-10-13T09:00:00Z", operation_key: event_id, source_context: { kind: "workspace_agent", agent_key: "workspace-orchestrator" },
  });
  const fetcher = installFetch((path, init) => {
    if (path === assignmentsPath) return Response.json({ items: [old], next_assignment_id: old.assignment_id });
    if (path === `${assignmentsPath}&after_assignment_id=assignment-1`) return Response.json({ items: [current], next_assignment_id: null });
    if (path === `${trackingRoot}?assignment_id=assignment-1`) return Response.json([old]);
    if (path === `${trackingRoot}/history?assignment_id=assignment-1&limit=20`) return Response.json({
      items: [historyEntry("old-waiting", { status: "waiting", waiting_reason: "Old question" })], next_event_id: "old-waiting",
    });
    if (path === `${trackingRoot}/history?assignment_id=assignment-1&limit=20&after_event_id=old-waiting`) return Response.json({
      items: [historyEntry("handling-1", old.tracking)], next_event_id: null,
    });
    if (path === `${trackingRoot}?assignment_id=assignment-2`) return Response.json([current]);
    if (path === `${trackingRoot}/history?assignment_id=assignment-2&limit=20`) return Response.json({
      items: [historyEntry("handling-2", current.tracking)], next_event_id: null,
    });
    expect(path).toBe(answerPath);
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ assignment_id: "assignment-2", expected_last_event_id: "handling-2", operation_key: expect.any(String), body: "Use the new policy" });
    return Response.json({ assignment_id: "assignment-2", input_event_id: "new-input", event_index: 1 });
  }, false);
  const view = mount(ticket);
  await fireEvent.click(view.getByText("Past assignment and release history"));
  await waitFor(() => expect(view.getByRole("button", { name: "More assignments" })).toBeTruthy());
  await fireEvent.click(view.getByRole("button", { name: "More assignments" }));
  await waitFor(() => expect(view.queryByRole("button", { name: "More assignments" })).toBeNull());
  expect(view.getByRole("button", { name: /Current · waiting/ })).toBeTruthy();
  expect(view.getByText(/Comparison delivered/)).toBeTruthy();
  await fireEvent.click(view.getByRole("button", { name: /Past · done/ }));
  await waitFor(() => expect(view.getByText("Previous result: Comparison delivered")).toBeTruthy());
  expect(view.getByText(/replaced by Mika/)).toBeTruthy();
  await waitFor(() => expect(view.getByRole("button", { name: "More handling history" })).toBeTruthy());
  await fireEvent.click(view.getByRole("button", { name: "More handling history" }));
  await waitFor(() => expect(view.getByText("Result: Comparison delivered")).toBeTruthy());
  expect(view.getByText("Waiting reason: Old question")).toBeTruthy();
  expect(view.queryByRole("button", { name: "More handling history" })).toBeNull();
  expect(view.getByText("Self-reported handling: waiting")).toBeTruthy();
  await fireEvent.input(view.getByRole("textbox", { name: "Answer / continue this handling" }), { target: { value: "Use the new policy" } });
  await fireEvent.click(view.getByRole("button", { name: "Send answer to this assignment" }));
  await waitFor(() => expect(view.getByText(/^Answer saved to assignment assignment-2/)).toBeTruthy());
  expect(view.refresh).toHaveBeenCalledTimes(1);
  await fireEvent.click(view.getByRole("button", { name: "Read replacement assignment" }));
  await waitFor(() => {
    expect(view.getByText(/· current · last handling waiting/)).toBeTruthy();
    expect(view.queryByText("Previous result: Comparison delivered")).toBeNull();
    expect(view.queryByText("Result: Comparison delivered")).toBeNull();
  });
  expectNoDiagnostics(fetcher, view);
});

test("released done is retained as past result without current Answer or implicit request", async () => {
  const old = historicalDone("unassigned");
  const ticket = { ...assigned(), assignment_tracking: [], assignments: [] };
  const fetcher = installFetch((path) => {
    if (path === assignmentsPath) return Response.json({ items: [old], next_assignment_id: null });
    if (path === `${trackingRoot}?assignment_id=assignment-1`) return Response.json([old]);
    expect(path).toBe(`${trackingRoot}/history?assignment_id=assignment-1&limit=20`);
    return Response.json({ items: [], next_event_id: null });
  }, false);
  const view = mount(ticket);
  await fireEvent.click(view.getByText("Past assignment and release history"));
  await waitFor(() => expect(view.getByRole("button", { name: /Past · done/ })).toBeTruthy());
  await fireEvent.click(view.getByRole("button", { name: /Past · done/ }));
  await waitFor(() => expect(view.getByText(/unassigned by Mika/)).toBeTruthy());
  expect(view.getByText("Previous result: Comparison delivered")).toBeTruthy();
  expect(view.getByText("No current Subject assignment.")).toBeTruthy();
  expect(view.queryByRole("textbox", { name: "Answer / continue this handling" })).toBeNull();
  expect(view.queryByRole("button", { name: "Read replacement assignment" })).toBeNull();
  expect(view.getByRole("button", { name: "Assign Subject" })).toBeTruthy();
  expect(fetcher.mock.calls.every(([, init]) => !init?.method || init.method === "GET")).toBe(true);
  expectNoDiagnostics(fetcher, view);
});

test("case close and reopen keep done result non-actionable and require an explicit new assignment", async () => {
  const fetcher = installFetch();
  const ticket = assigned("done");
  const view = mount(ticket);
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  for (const state of ["closed", "open"] as const) {
    await view.rerender({ workspaceId: "team", ticket: { ...ticket, state }, refresh: view.refresh });
    expect(view.getByText(`This handling was returned. Ticket ${state} is unchanged.`)).toBeTruthy();
    expect(view.getByText("Comparison delivered")).toBeTruthy();
    expect(view.queryByRole("textbox", { name: "Answer / continue this handling" })).toBeNull();
    expect(view.getByRole("button", { name: "Request again (new assignment)" })).toBeTruthy();
  }
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(view.refresh).not.toHaveBeenCalled();
  expectNoDiagnostics(fetcher, view);
});

test("releasing responsibility while reading an old result uses current assignment rather than retained identity", async () => {
  const old = historicalDone();
  const ticket = assigned();
  ticket.assignment_tracking[0] = { ...ticket.assignment_tracking[0], assignment_id: "assignment-2", last_event_id: "handling-2" };
  ticket.assignments[0] = { ...ticket.assignments[0], assignment_id: "assignment-2" };
  ticket.action_eligibility.can_unassign_orchestrator = true;
  vi.stubGlobal("confirm", vi.fn(() => true));
  const fetcher = installFetch((path, init) => {
    if (path === assignmentsPath) return Response.json({ items: [old], next_assignment_id: null });
    if (path === `${trackingRoot}?assignment_id=assignment-1`) return Response.json([old]);
    if (path === `${trackingRoot}/history?assignment_id=assignment-1&limit=20`) return Response.json({ items: [], next_event_id: null });
    const url = new URL(path, "http://fixture.invalid");
    expect(url.pathname).toBe(assignmentPath);
    expect(init?.method).toBe("DELETE");
    expect(url.searchParams.get("assignment_id")).toBe("assignment-2");
    expect(url.searchParams.get("operation_id")).toBeTruthy();
    return Response.json({ workspace_id: "team", ticket_id: "internal-T-1", assignment: null });
  }, false);
  const view = mount(ticket);
  await fireEvent.click(view.getByText("Past assignment and release history"));
  await waitFor(() => expect(view.getByRole("button", { name: /Past · done/ })).toBeTruthy());
  await fireEvent.click(view.getByRole("button", { name: /Past · done/ }));
  await waitFor(() => expect(view.getByText("Previous result: Comparison delivered")).toBeTruthy());
  await fireEvent.click(view.getByRole("button", { name: "Release assignment" }));
  await waitFor(() => expect(view.getByText("Assignment operation saved: released.")).toBeTruthy());
  expect(view.refresh).toHaveBeenCalledTimes(1);
  expect(view.getByText("Previous result: Comparison delivered")).toBeTruthy();
  expect(fetcher.mock.calls.filter(([, init]) => init?.method === "DELETE")).toHaveLength(1);
  expectNoDiagnostics(fetcher, view);
});

const subjectPage = { limit: 100, items: [{ id: "subject-a", role: "Research", state: "active", behavior_md: "", memory_fingerprint: "f", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", current_worker: null }], has_more: false, next_cursor: null };
async function chooseSubject(view: ReturnType<typeof mount>) {
  await fireEvent.click(view.getByText("Choose responsible Subject"));
  await waitFor(() => expect(view.queryByText("Loading Subjects…")).toBeNull());
  // Current Subject is the observed reissue candidate; different-Subject native selection is covered in Playwright.
}

test("legacy principal is audit-only with no Answer or mandatory singleton observation", async () => {
  const ticket = assigned();
  ticket.assignment_tracking[0].principal = { kind: "workspace_agent", agent_key: "workspace-orchestrator" };
  ticket.assignments[0].principal = ticket.assignment_tracking[0].principal;
  const fetcher = installFetch();
  const view = mount(ticket);
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(view.getByText(/Legacy assignment retained for audit only/)).toBeTruthy();
  expect(view.getByText("Workspace agent: workspace-orchestrator")).toBeTruthy();
  expect(view.queryByRole("textbox", { name: "Answer / continue this handling" })).toBeNull();
  expect(fetcher.mock.calls.some(([path]) => String(path).endsWith("/orchestrator"))).toBe(false);
  expect(view.queryByLabelText("Principal kind")).toBeNull();
});
