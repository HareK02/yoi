// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { TicketDetail } from "#lib/generated/ticket-api.ts";
import { fixtureDetail } from "#lib/workspace/home/dashboard.test-fixtures.ts";
import TicketHierarchy from "./TicketHierarchy.svelte";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});
const parent = {
  id: "parent-id",
  resource_key: "T-8",
  title: "Parent work",
  state: "closed",
  closure_reason: "not_planned",
} as const;
function detail(canSetParent = false): TicketDetail {
  const ticket = fixtureDetail("T-1");
  return {
    ...ticket,
    action_eligibility: {
      ...ticket.action_eligibility,
      can_set_parent: canSetParent,
    },
    parent_link: {
      ticket_id: ticket.id,
      observation: {
        parent_ticket_id: parent.id,
        last_operation_key: "parent-1",
      },
      parent,
    },
  };
}
function reads(ticket: TicketDetail, path: string): Response {
  if (path.endsWith("/parent")) return Response.json(ticket.parent_link);
  if (path.includes("/children?")) {
    return Response.json({
      parent_ticket_id: ticket.id,
      items: [],
      next_cursor: null,
      has_more: false,
    });
  }
  throw new Error(`Unexpected request ${path}`);
}
test("read-only hierarchy preserves canonical navigation and non-success closure reason without mutation actions", async () => {
  const ticket = detail();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => reads(ticket, String(input))),
  );
  const view = render(TicketHierarchy, {
    workspaceId: "ws",
    routeTicketId: ticket.id,
    ticket,
  });
  await waitFor(() =>
    expect(view.getByText("No direct children.")).toBeTruthy()
  );
  const link = view.getByRole("link", {
    name: /T-8 Parent work closed · not_planned/,
  });
  expect(link.getAttribute("href")).toBe("/w/ws/tickets/T-8-parent-work");
  expect(view.queryByText("Change or unlink parent")).toBeNull();
  expect(view.queryByRole("button", { name: "Unlink parent" })).toBeNull();
});
test("existing Ticket search selection sends observed CAS body and keeps exact retry after response loss", async () => {
  const ticket = detail(true);
  const bodies: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      if (init?.method === "PUT") {
        bodies.push(String(init.body));
        throw new TypeError("response lost");
      }
      if (path.endsWith("/tickets/browse")) {
        return Response.json({
          record_authority: "sqlite",
          items: [{
            id: "candidate-id",
            resource_key: "T-9",
            title: "Existing work",
            state: "open",
            tags: [],
            assignment_tracking: [],
            current_closure: null,
          }],
          page: {
            has_more: false,
            next_cursor: null,
            returned: 1,
            limit: 20,
            sort: "updated_desc",
            source_truncated: false,
          },
        });
      }
      return reads(ticket, path);
    }),
  );
  const view = render(TicketHierarchy, {
    workspaceId: "ws",
    routeTicketId: ticket.id,
    ticket,
  });
  await waitFor(() =>
    expect(view.getByText("Change or unlink parent")).toBeTruthy()
  );
  await fireEvent.click(view.getByText("Change or unlink parent"));
  await fireEvent.input(view.getByLabelText("Search existing Tickets"), {
    target: { value: "Existing" },
  });
  await fireEvent.click(view.getByRole("button", { name: "Search Tickets" }));
  await waitFor(() =>
    expect(view.getByRole("radio", { name: /T-9 Existing work/ })).toBeTruthy()
  );
  await fireEvent.click(view.getByRole("radio", { name: /T-9 Existing work/ }));
  await fireEvent.click(
    view.getByRole("button", { name: "Set selected parent" }),
  );
  await waitFor(() => {
    expect(view.getByText(/Outcome unavailable or unknown/)).toBeTruthy();
    expect(
      (view.getByRole("button", {
        name: "Retry exact parent operation",
      }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });
  expect(
    view.getByText(
      "Unresolved parent operation retained. New changes are blocked.",
    ),
  ).toBeTruthy();
  expect(
    (view.getByRole("button", {
      name: "Set selected parent",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  expect(
    (view.getByRole("button", {
      name: "Unlink parent",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  const body = JSON.parse(bodies[0]);
  expect(body.expected).toEqual(ticket.parent_link.observation);
  expect(body.parent).toBe("candidate-id");
  expect(typeof body.operation_key).toBe("string");
  await fireEvent.click(
    view.getByRole("button", { name: "Retry exact parent operation" }),
  );
  await waitFor(() => expect(bodies).toHaveLength(2));
  expect(bodies[1]).toBe(bodies[0]);
  expect(view.queryByText(/Parent operation saved\./)).toBeNull();
});
