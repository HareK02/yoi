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
    principal: { kind: "workspace_agent", agent_key: "workspace-orchestrator" }, last_event_id: "handling-1",
    tracking: { status, waiting_reason: "Choose the policy", progress: "Compared options", next_action: "Confirm the choice", result: "Comparison delivered" },
  };
  return { ...fixtureDetail("T-1"), assignment_tracking: [row],
    assignments: [{ assignment_id: row.assignment_id, role: row.role, principal: row.principal, assigned_at: row.assigned_at, assigned_by: row.assigned_by }],
    action_eligibility: { ...fixtureDetail("T-1").action_eligibility, can_assign_orchestrator: true },
  };
}
function installFetch(handle: (path: string, init?: RequestInit) => Response | Promise<Response> = () => { throw new Error("Unexpected request"); }) {
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    if (path === assignmentsPath) return Response.json({ items: [], next_assignment_id: null });
    return await handle(path, init);
  });
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
}
function mount(ticket = assigned(), refresh = vi.fn(async () => {})) {
  return { ...render(TicketHandling, { workspaceId: "team", ticket, orchestrator: null, refresh }), refresh };
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
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(1));
  expect(view.getByText("Self-reported handling: waiting")).toBeTruthy();
  for (const text of ["Choose the policy", "Compared options", "Confirm the choice", "Comparison delivered", "Assignment details"]) expect(view.getByText(text)).toBeTruthy();
  expect(view.container.querySelector(".ticket-orchestrator-worker")).not.toBeNull();
  expectNoDiagnostics(fetcher, view);
});

test("assignment save refreshes assignment history without diagnostics or delivery/execution notice", async () => {
  const fetcher = installFetch((path, init) => {
    expect(path).toBe(assignmentPath);
    expect(init?.method).toBe("PUT");
    return Response.json({ ticket_id: "internal-T-1", workspace_id: "team", assignment: {
      assignment_id: "assignment-2", assigned_at: "2026-10-13T10:00:00Z", assigned_by: "Mika", role: "orchestrator",
      ticket_id: "internal-T-1", workspace_id: "team", principal: { kind: "workspace_agent", agent_key: "workspace-orchestrator" },
    } });
  });
  const view = mount(assigned("done"));
  await fireEvent.click(view.getByRole("button", { name: "Request again (new assignment)" }));
  await waitFor(() => {
    expect(fetcher).toHaveBeenCalledTimes(3);
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
    expect(fetcher).toHaveBeenCalledTimes(3);
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
  await fireEvent.click(view.getByRole("button", { name: "Request again (new assignment)" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("Outcome unknown"));
  await fireEvent.click(view.getByRole("button", { name: "Refresh and reconcile assignment" }));
  await waitFor(() => {
    expect(fetcher).toHaveBeenCalledTimes(3);
    expect(view.queryByText("Loading assignment history…")).toBeNull();
  });
  expect(view.refresh).toHaveBeenCalledTimes(1);
  expect(view.queryByRole("alert")).toBeNull();
  expectNoDiagnostics(fetcher, view);
});
