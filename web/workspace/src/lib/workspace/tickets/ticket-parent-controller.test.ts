import type {
  TicketChildrenPage,
  TicketLinkSummary,
  TicketParentLink,
} from "#lib/generated/ticket-api.ts";
import type { TicketParentClient } from "#lib/workspace/api/ticket-parent.ts";
import {
  TicketParentController,
  type TicketParentState,
} from "./ticket-parent-controller.ts";

declare const Deno: { test(name: string, fn: () => Promise<void>): void };
function assert(value: unknown): void {
  if (!value) throw new Error("Assertion failed");
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const summary = (id: string): TicketLinkSummary => ({
  id,
  resource_key: `T-${id}`,
  title: `Ticket ${id}`,
  state: "open",
  closure_reason: null,
});
const link = (
  id: string,
  parent: string | null = null,
  key: string | null = null,
): TicketParentLink => ({
  ticket_id: id,
  observation: { parent_ticket_id: parent, last_operation_key: key },
  parent: parent ? summary(parent) : null,
});
const page = (id: string): TicketChildrenPage => ({
  parent_ticket_id: id,
  items: [],
  next_cursor: null,
  has_more: false,
});
function harness(
  overrides: Partial<TicketParentClient> = {},
  storage?: Pick<Storage, "getItem" | "setItem" | "removeItem">,
) {
  const client: TicketParentClient = {
    parent: async (_, id) => link(id),
    children: async (_, id) => page(id),
    search: async () => ({ items: [summary("8")], hasMore: false }),
    resolve: async () => summary("8"),
    set: async () => link("1", "8", "set-1"),
    ...overrides,
  };
  const controller = new TicketParentController(client, storage);
  let state!: TicketParentState;
  controller.subscribe((value) => {
    state = value;
  });
  return {
    controller,
    get state() {
      return state;
    },
  };
}
Deno.test("same canonical parent and already-unlinked changes do not send or retain an operation", async () => {
  for (const currentParent of ["8", null]) {
    let sends = 0;
    let retainedWrites = 0;
    const observed = link("1", currentParent, "previous-operation");
    const h = harness({
      parent: async () => observed,
      set: async () => {
        ++sends;
        return observed;
      },
    }, {
      getItem: () => null,
      setItem: () => {
        ++retainedWrites;
      },
      removeItem: () => {},
    });
    await h.controller.select("ws", "1", observed, true);
    await h.controller.change(
      currentParent === null
        ? null
        : { ...summary(currentParent), title: "Updated title" },
      "unused-new-key",
    );
    assert(sends === 0 && retainedWrites === 0);
    assert(
      !h.state.pending && !h.state.saving && !h.state.saved &&
        !h.state.operationError,
    );
    assert(h.state.parent?.observation.parent_ticket_id === currentParent);
    assert(
      h.state.parent?.observation.last_operation_key === "previous-operation",
    );
  }
});
Deno.test("no-op choices during an unknown save preserve the full request and leave exact retry available", async () => {
  for (const currentParent of ["8", null]) {
    let observed = link("1", currentParent, "previous-operation");
    const entries = new Map<string, string>();
    const sent: { path: string; method: string; body?: string }[] = [];
    let writes = 0;
    const h = harness({
      parent: async () => observed,
      set: async (operation) => {
        sent.push({ ...operation });
        if (sent.length === 1) throw new TypeError("response lost");
        observed = link("1", "9", "original-key");
        return observed;
      },
    }, {
      getItem: (key) => entries.get(key) ?? null,
      setItem: (key, value) => {
        ++writes;
        entries.set(key, value);
      },
      removeItem: (key) => {
        entries.delete(key);
      },
    });
    await h.controller.select("ws", "1", observed, true);
    await h.controller.change(summary("9"), "original-key");
    const retained = JSON.stringify([...entries]);
    const pending = JSON.stringify(h.state.pending);
    const error = h.state.operationError;
    const retainedWrites = writes;
    assert(h.state.pending?.key === "original-key" && !h.state.saved);
    const originalBody = JSON.parse(sent[0].body!);
    assert(originalBody.expected.parent_ticket_id === currentParent);
    assert(originalBody.expected.last_operation_key === "previous-operation");
    await h.controller.change(
      currentParent === null ? null : summary(currentParent),
      "unused-no-op-key",
    );
    assert(sent.length === 1 && writes === retainedWrites);
    assert(JSON.stringify([...entries]) === retained);
    assert(JSON.stringify(h.state.pending) === pending);
    assert(h.state.operationError === error && !h.state.saved);

    // desiredが現在の親と一致しても、別keyの再読では先行要求の成否を確定しない。
    observed = link("1", "9", "intervening-operation");
    await h.controller.readParent();
    await h.controller.change(summary("9"), "unused-same-parent-key");
    h.controller.useRefreshedObservation();
    assert(sent.length === 1 && writes === retainedWrites);
    assert(JSON.stringify([...entries]) === retained);
    assert(JSON.stringify(h.state.pending) === pending);
    assert(!h.state.saved && !h.state.saving);

    // retryはchangeのno-op抑止を通さず、元のexpected/key/bodyを完全一致で送る。
    await h.controller.retry();
    assert(sent.length === 2);
    assert(JSON.stringify(sent[1]) === JSON.stringify(sent[0]));
    assert(entries.size === 0 && !h.state.pending && h.state.saved);
    assert(h.state.parent?.observation.last_operation_key === "original-key");
  }
});
Deno.test("unknown parent save retains exact key body and expected observation across navigation", async () => {
  const sent: string[] = [];
  let attempt = 0;
  const h = harness({
    set: async (operation) => {
      sent.push(operation.body!);
      if (++attempt === 1) throw new TypeError("response lost");
      return link("1", "8", "set-1");
    },
  });
  await h.controller.select(
    "ws",
    "1",
    link("1", null, "unlink-previous"),
    true,
  );
  await h.controller.change(summary("8"), "set-1");
  assert(h.state.pending && !h.state.saved);
  const body = JSON.parse(sent[0]);
  assert(
    body.operation_key === "set-1" && body.expected.last_operation_key === null,
  );
  await h.controller.select("ws", "2", link("2"), true);
  await h.controller.select("ws", "1", link("1"), true);
  assert(h.state.pending && h.state.reconciled);
  h.controller.useRefreshedObservation();
  assert(h.state.pending);
  await h.controller.change(null, "must-not-send");
  assert(sent.length === 1);
  await h.controller.retry();
  assert(sent.length === 2 && sent[0] === sent[1]);
  assert(!h.state.pending && h.state.saved);
  // 再送応答の親は8だが、直後のGETでは親なしを観測する。
  assert(h.state.parent?.observation.parent_ticket_id === null);
});
Deno.test("permission and CAS rejections never report success or automatically rebase", async () => {
  for (const status of [403, 409]) {
    const h = harness({
      set: async () => {
        throw new Error(`request failed (${status})`);
      },
    });
    await h.controller.select("ws", "1", link("1"), true);
    await h.controller.change(summary("8"), `rejected-${status}`);
    assert(h.state.pending?.rejected && !h.state.saved);
    assert(h.state.parent?.observation.parent_ticket_id === null);
    h.controller.useRefreshedObservation();
    assert(h.state.pending);
    await h.controller.readParent();
    h.controller.useRefreshedObservation();
    assert(!h.state.pending && !h.state.saved);
  }
});
Deno.test("rejection after an ambiguous send cannot discard that earlier unknown outcome", async () => {
  let count = 0;
  const h = harness({
    set: async () => {
      if (++count === 1) throw new TypeError("lost");
      throw new Error("request failed (403)");
    },
  });
  await h.controller.select("ws", "1", link("1"), true);
  await h.controller.change(summary("8"), "same-key");
  await h.controller.retry();
  await h.controller.readParent();
  h.controller.useRefreshedObservation();
  assert(h.state.pending && !h.state.pending.rejected && !h.state.saved);
});
Deno.test("parent children and search responses from an old A epoch cannot overwrite A after ABA navigation", async () => {
  const oldParent = deferred<TicketParentLink>();
  const oldChildren = deferred<TicketChildrenPage>();
  const oldSearch = deferred<
    { items: TicketLinkSummary[]; hasMore: boolean }
  >();
  let reads = 0;
  const h = harness({
    parent: async (_, id) => ++reads === 1 ? oldParent.promise : link(id),
    children: async (_, id) =>
      id === "1" && reads === 1 ? oldChildren.promise : page(id),
    search: async () => oldSearch.promise,
  });
  const first = h.controller.select("ws", "1", link("1"), true);
  const searching = h.controller.search("old");
  await h.controller.select("ws", "2", link("2"), true);
  await h.controller.select("ws", "1", link("1"), true);
  oldParent.resolve(link("1", "8", "old"));
  oldChildren.resolve({ ...page("1"), items: [summary("9")] });
  oldSearch.resolve({ items: [summary("8")], hasMore: true });
  await first;
  await searching;
  assert(h.state.parent?.observation.parent_ticket_id === null);
  assert(
    h.state.children?.items.length === 0 && h.state.candidates.length === 0,
  );
  assert(
    !h.state.loadingParent && !h.state.loadingChildren && !h.state.searching,
  );
});
Deno.test("old mutation success does not change a new route or clear its retained unknown attempt", async () => {
  const saving = deferred<TicketParentLink>();
  const h = harness({ set: async () => saving.promise });
  await h.controller.select("ws", "1", link("1"), true);
  const operation = h.controller.change(summary("8"), "old-save");
  await h.controller.select("ws", "2", link("2"), true);
  saving.resolve(link("1", "8", "old-save"));
  await operation;
  assert(h.state.ticketId === "2" && !h.state.saved && !h.state.pending);
  await h.controller.select("ws", "1", link("1"), true);
  assert(h.state.pending?.key === "old-save" && !h.state.saved);
});
Deno.test("children continuation stays scoped and preserves rows when the next page is rejected", async () => {
  const cursors: (string | null)[] = [];
  const h = harness({
    children: async (_, id, cursor) => {
      cursors.push(cursor);
      return cursor === null
        ? {
          ...page(id),
          items: [summary("8")],
          next_cursor: "opaque",
          has_more: true,
        }
        : { ...page("wrong-parent"), items: [summary("9")] };
    },
  });
  await h.controller.select("ws", "1", link("1"), true);
  await h.controller.readChildren(true);
  assert(cursors[0] === null && cursors[1] === "opaque");
  assert(
    h.state.children?.items[0].id === "8" && h.state.childrenError !== null,
  );
});
Deno.test("tab-session retained request survives component remount without rebasing or automatic resend", async () => {
  const entries = new Map<string, string>();
  const storage = {
    getItem: (key: string) => entries.get(key) ?? null,
    setItem: (key: string, value: string) => {
      entries.set(key, value);
    },
    removeItem: (key: string) => {
      entries.delete(key);
    },
  };
  const sent: string[] = [];
  const first = harness({
    set: async (operation) => {
      sent.push(operation.body!);
      throw new TypeError("lost");
    },
  }, storage);
  await first.controller.select("ws", "1", link("1"), true);
  await first.controller.change(summary("8"), "tab-save");
  first.controller.dispose();
  const next = harness({
    set: async (operation) => {
      sent.push(operation.body!);
      return link("1", "8", "tab-save");
    },
  }, storage);
  await next.controller.select("ws", "1", link("1"), true);
  assert(next.state.pending?.key === "tab-save" && sent.length === 1);
  await next.controller.change(null, "unused-unlink-key");
  assert(next.state.pending?.key === "tab-save" && sent.length === 1);
  await next.controller.retry();
  assert(sent[0] === sent[1] && !next.state.pending && entries.size === 0);
});
Deno.test("unreadable request retention fails closed before any parent mutation", async () => {
  let sends = 0;
  const h = harness({
    set: async () => {
      ++sends;
      return link("1");
    },
  }, {
    getItem: () => "corrupt",
    setItem: () => {},
    removeItem: () => {},
  });
  await h.controller.select("ws", "1", link("1"), true);
  await h.controller.change(summary("8"), "do-not-send");
  assert(h.state.retentionError && sends === 0);
});
Deno.test("ineligible parent actions cannot search or mutate even with visible links", async () => {
  let calls = 0;
  const h = harness({
    search: async () => {
      ++calls;
      return { items: [], hasMore: false };
    },
    set: async () => {
      ++calls;
      return link("1");
    },
  });
  await h.controller.select("ws", "1", link("1", "8"), false);
  await h.controller.search("anything");
  await h.controller.change(null, "forbidden");
  assert(calls === 0 && !h.state.pending && !h.state.saved);
});
