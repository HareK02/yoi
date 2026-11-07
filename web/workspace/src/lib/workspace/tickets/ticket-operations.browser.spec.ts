// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type {
  TicketAssignmentTrackingProjection,
  TicketDetail,
} from "#lib/generated/ticket-api.ts";
import { fixtureDetail } from "../home/dashboard.test-fixtures.ts";
import TicketHandling from "./TicketHandling.svelte";
import TicketTags from "./TicketTags.svelte";
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
const assignment = (
  id = "delegation-1",
  status: "waiting" | "done" = "waiting",
): TicketAssignmentTrackingProjection => ({
  workspace_id: "team",
  ticket_id: "T-1",
  assignment_id: id,
  role: "orchestrator",
  principal: { kind: "subject", subject_id: "subject-a" },
  assigned_by: "user",
  assigned_at: "2026-01-01T00:00:00Z",
  current: true,
  tracking: {
    status,
    waiting_reason: status === "waiting" ? "User answer needed" : null,
    result: status === "done" ? "Options returned" : null,
  },
  last_event_id: "event-1",
});
function detail(row = assignment()): TicketDetail {
  return {
    ...fixtureDetail("T-1"),
    action_eligibility: {
      ...fixtureDetail("T-1").action_eligibility,
      can_assign_orchestrator: true,
      can_unassign_orchestrator: true,
    },
    assignment_tracking: [row],
    assignments: [{
      assignment_id: row.assignment_id,
      role: row.role,
      principal: row.principal,
      assigned_by: row.assigned_by,
      assigned_at: row.assigned_at,
    }],
  };
}
function reads(path: string, row: TicketAssignmentTrackingProjection) {
  if (path.includes("/subjektiv/subjects")) return Response.json({ limit: 100, items: [{ id: "subject-a", role: "Research", state: "active", behavior_md: "", memory_fingerprint: "f", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" }], has_more: false, next_cursor: null });
  if (path.includes("/assignment-tracking/assignments")) {
    return Response.json({ items: [row], next_assignment_id: null });
  }
  throw new Error(`Unexpected read ${path}`);
}
test("an answer response loss keeps exact assignment/event/key/body for retry even when observed responsibility changes", async () => {
  const row = assignment();
  const sent: string[] = [];
  let attempts = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      if (path.endsWith("/assignment-answer")) {
        sent.push(String(init?.body));
        if (++attempts === 1) throw new Error("response lost");
        return Response.json({
          assignment_id: row.assignment_id,
          input_event_id: "input-2",
          event_index: 2,
        });
      }
      return reads(path, row);
    }),
  );
  const refresh = vi.fn(async () => {});
  const view = render(TicketHandling, {
    workspaceId: "team",
    ticket: detail(row),
    refresh,
  });
  await fireEvent.input(
    view.getByLabelText("Answer / continue this handling"),
    { target: { value: "Use option B" } },
  );
  await fireEvent.click(view.getByText("Send answer to this assignment"));
  await waitFor(() => expect(view.getByText(/Outcome unknown\./)).toBeTruthy());
  await view.rerender({ ticket: detail(assignment("delegation-2")) });
  expect(
    (view.getByText("Send answer to this assignment") as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await fireEvent.click(view.getByText("Retry exact operation"));
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(1));
  expect(sent).toHaveLength(2);
  expect(sent[1]).toBe(sent[0]);
  expect(JSON.parse(sent[1])).toEqual({
    assignment_id: "delegation-1",
    expected_last_event_id: "event-1",
    body: "Use option B",
    operation_key: expect.any(String),
  });
  expect(view.container.textContent).toContain("Handling status is unchanged");
});
test("a done reissue persists before a failed refresh and does not pretend delivery or execution succeeded", async () => {
  const row = assignment("delegation-1", "done");
  const sent: unknown[] = [];
  const paths: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      paths.push(String(input));
      if (init?.method === "PUT") {
        sent.push(JSON.parse(String(init.body)));
        return Response.json({
          workspace_id: "team",
          ticket_id: "T-1",
          assignment: {
            assignment_id: "delegation-2",
            workspace_id: "team",
            ticket_id: "T-1",
            role: "orchestrator",
            principal: row.principal,
            assigned_at: row.assigned_at,
            assigned_by: row.assigned_by,
          },
        });
      }
      return reads(String(input), row);
    }),
  );
  const view = render(TicketHandling, {
    workspaceId: "team",
    ticket: detail(row),
    refresh: vi.fn(async () => {
      throw new Error("refresh unavailable");
    }),
  });
  await fireEvent.click(view.getByText("Choose responsible Subject"));
  await waitFor(() => expect(view.queryByText("Loading Subjects…")).toBeNull());
  // Current Subject is the observed reissue candidate; different-Subject native selection is covered in Playwright.
  await fireEvent.click(view.getByText("Request again (new assignment)"));
  await waitFor(() =>
    expect(view.getByText(/Assignment operation saved: delegation-2/))
      .toBeTruthy()
  );
  expect(sent).toHaveLength(1);
  expect(sent[0]).toMatchObject({ expected_assignment_id: "delegation-1" });
  expect(view.queryByText("Retry exact operation")).toBeNull();
  await waitFor(() =>
    expect(view.getByRole("alert").textContent).toContain(
      "Operation saved; latest projection unavailable",
    )
  );
  expect(view.container.querySelector(".ticket-delivery")).toBeNull();
  expect(view.queryByText("Delivery and execution")).toBeNull();
  expect(paths.some((path) => path.includes("assignment-diagnostics"))).toBe(false);
  expect(view.container.textContent).toContain("Self-reported handling: done");
});
test("classification conflict keeps stable IDs and the original observed set across an exact retry", async () => {
  const tag = {
    workspace_id: "team",
    id: "tag-1",
    name: "closed",
    last_operation_key: "tag-create",
    deleted: false,
  };
  const requests: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      if (init?.method === "PUT") {
        requests.push(String(init.body));
        return new Response("", { status: 409 });
      }
      return Response.json([tag]);
    }),
  );
  const view = render(TicketTags, {
    workspaceId: "team",
    ticket: { ...detail(), tags: [] },
    refresh: vi.fn(async () => {}),
  });
  await waitFor(() => expect(view.queryByText("Loading tag catalog…")).toBeNull());
  await fireEvent.click(view.getByLabelText("closed"));
  await fireEvent.click(view.getByText("Save classification"));
  await waitFor(() => expect(view.getByText(/Stale observation/)).toBeTruthy());
  await view.rerender({ ticket: { ...detail(), tags: [tag] } });
  await fireEvent.click(view.getByText("Retry exact tag operation"));
  await waitFor(() => expect(requests).toHaveLength(2));
  expect(requests[1]).toBe(requests[0]);
  expect(JSON.parse(requests[1])).toMatchObject({
    expected_tag_ids: [],
    tag_ids: ["tag-1"],
  });
  expect((view.getByText("Save classification") as HTMLButtonElement).disabled)
    .toBe(true);
});
