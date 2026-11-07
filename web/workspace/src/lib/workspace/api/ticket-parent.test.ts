import {
  parseTicketChildrenPage,
  parseTicketLinkSummary,
  parseTicketParentLink,
} from "./ticket-browser.ts";
import { createTicketParentClient } from "./ticket-parent.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};
function assert(value: unknown): asserts value {
  if (!value) throw new Error("Assertion failed");
}
function rejects(fn: () => unknown): void {
  let rejected = false;
  try {
    fn();
  } catch {
    rejected = true;
  }
  assert(rejected);
}
const parent = {
  id: "parent-id",
  resource_key: "T-8",
  title: "Parent",
  state: "closed",
  closure_reason: "not_planned",
};
Deno.test("reference resolution reads only link-summary and preserves metadata closure reason", async () => {
  const originalFetch = globalThis.fetch;
  const paths: string[] = [];
  globalThis.fetch = async (input) => {
    paths.push(String(input));
    return Response.json(parent);
  };
  try {
    const client = createTicketParentClient();
    for (const reference of ["T-8", "canonical/id"]) {
      const result = await client.resolve("workspace one", reference);
      assert(result.id === parent.id && result.resource_key === "T-8");
      assert(
        result.state === "closed" && result.closure_reason === "not_planned",
      );
      assert(
        paths.at(-1) ===
          `/api/w/workspace%20one/tickets/${
            encodeURIComponent(reference)
          }/link-summary`,
      );
    }
    assert(paths.length === 2);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
Deno.test("reference resolution denial never falls back to Ticket detail", async () => {
  const originalFetch = globalThis.fetch;
  const paths: string[] = [];
  globalThis.fetch = async (input) => {
    paths.push(String(input));
    return new Response(null, { status: 403 });
  };
  try {
    let denied = false;
    try {
      await createTicketParentClient().resolve("ws", "T-8");
    } catch (cause) {
      denied = cause instanceof Error && cause.message.includes("(403)");
    }
    assert(denied);
    assert(
      paths.length === 1 && paths[0] === "/api/w/ws/tickets/T-8/link-summary",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
Deno.test("parent JSON preserves closure reasons and the full ABA observation", () => {
  const result = parseTicketParentLink({
    ticket_id: "child-id",
    observation: { parent_ticket_id: "parent-id", last_operation_key: "set-1" },
    parent,
  });
  assert(result.parent?.closure_reason === "not_planned");
  assert(result.observation.last_operation_key === "set-1");
  const detached = parseTicketParentLink({
    ticket_id: "child-id",
    observation: { parent_ticket_id: null, last_operation_key: "unlink-2" },
    parent: null,
  });
  assert(detached.observation.last_operation_key === "unlink-2");
  assert(
    parseTicketLinkSummary({
      ...parent,
      closure_reason: { other: "Research returned" },
    }).closure_reason !== null,
  );
});
Deno.test("parent JSON rejects stale shapes mismatched identities and invalid case semantics", () => {
  for (
    const value of [
      { ticket_id: "child-id", parent },
      {
        ticket_id: "child-id",
        observation: {
          parent_ticket_id: "different",
          last_operation_key: null,
        },
        parent,
      },
      {
        ticket_id: "child-id",
        observation: { parent_ticket_id: null },
        parent: null,
      },
      {
        ticket_id: "child-id",
        observation: {
          parent_ticket_id: null,
          last_operation_key: null,
          permission: true,
        },
        parent: null,
      },
    ]
  ) rejects(() => parseTicketParentLink(value));
  rejects(() => parseTicketLinkSummary({ ...parent, state: "done" }));
  rejects(() =>
    parseTicketLinkSummary({ ...parent, closure_reason: "success" })
  );
});
Deno.test("children JSON enforces bounded pages and usable continuation", () => {
  const page = {
    parent_ticket_id: "parent-id",
    items: [parent],
    next_cursor: "opaque+cursor",
    has_more: true,
  };
  assert(parseTicketChildrenPage(page).next_cursor === "opaque+cursor");
  rejects(() =>
    parseTicketChildrenPage({ ...page, items: Array(101).fill(parent) })
  );
  rejects(() => parseTicketChildrenPage({ ...page, next_cursor: null }));
  rejects(() => parseTicketChildrenPage({ ...page, has_more: false }));
  rejects(() => parseTicketChildrenPage({ ...page, next_cursor: "" }));
});
