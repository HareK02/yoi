import {
  compileTicketSearch,
  DEFAULT_TICKET_SEARCH,
  ticketSearchNeedsTags,
} from "./ticket-search.ts";
import type { WorkspaceTag } from "../../generated/ticket-api.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertSearchError(
  input: string,
  messagePart: string,
  availableTags = tags,
): void {
  try {
    compileTicketSearch(input, availableTags);
  } catch (error) {
    if (!(error instanceof Error) || !error.message.includes(messagePart)) {
      throw new Error(
        `expected error containing ${JSON.stringify(messagePart)}, got ${
          String(error)
        }`,
      );
    }
    return;
  }
  throw new Error(`expected ${JSON.stringify(input)} to fail`);
}

const tags: readonly WorkspaceTag[] = Object.freeze([
  Object.freeze({
    workspace_id: "workspace",
    id: "tag-1",
    name: "Ready",
    last_operation_key: "create-1",
    deleted: false,
  }),
  Object.freeze({
    workspace_id: "workspace",
    id: "tag-2",
    name: "Needs review",
    last_operation_key: "create-2",
    deleted: false,
  }),
  Object.freeze({
    workspace_id: "workspace",
    id: "tag-3",
    name: "Retired",
    last_operation_key: "delete-3",
    deleted: true,
  }),
]);

Deno.test("only actual tag qualifiers require the tag catalog", () => {
  for (
    const input of [
      "",
      "state:open",
      '"text tag:Ready"',
      'reason:"text tag:Ready"',
      "TAG:Ready",
      "https://example.test/tag:Ready",
    ]
  ) {
    assertEquals(ticketSearchNeedsTags(input), false);
  }
  for (const input of ['tag:"Needs review"', "text tag:Ready", "tag:"]) {
    assertEquals(ticketSearchNeedsTags(input), true);
  }
});

Deno.test("empty search selects all states with bounded first-page defaults", () => {
  for (const input of ["", " \t\n "]) {
    assertEquals<unknown>(compileTicketSearch(input, tags), {
      query: null,
      states: [],
      tag_ids: [],
      assigned_principal: null,
      handling_status: null,
      sort: "updated_desc",
      limit: 30,
      cursor: null,
    });
  }
  assertEquals(DEFAULT_TICKET_SEARCH, "state:open");
  assertEquals(compileTicketSearch(DEFAULT_TICKET_SEARCH, tags).states, [
    "open",
  ]);
});

Deno.test("plain words preserve case and do not implicitly restrict states", () => {
  const request = compileTicketSearch("  Fix\tHTTP   Cache\nBug  ", tags);
  assertEquals(request.query, "Fix HTTP Cache Bug");
  assertEquals(request.states, []);
});

Deno.test("state filters never leak into backend free text", () => {
  for (
    const [value, expected] of [["open", ["open"]], ["closed", ["closed"]], [
      "all",
      [],
    ]] as const
  ) {
    const request = compileTicketSearch(`Fix state:${value} HTTP`, tags);
    assertEquals(request.states, [...expected]);
    assertEquals(request.query, "Fix HTTP");
    assertEquals(compileTicketSearch(`state:${value}`, tags).query, null);
  }
});

Deno.test("unknown qualifiers and URLs remain ordinary free text", () => {
  const input =
    'https://Example.test/state:closed label:Ready handling:done sort:oldest close:yes field:"Some Text"';
  const request = compileTicketSearch(input, tags);
  assertEquals(request.query, input);
  assertEquals(request.states, []);
  assertEquals(request.tag_ids, []);
  assertEquals(request.handling_status, null);
  assertEquals(request.sort, "updated_desc");
});

Deno.test("quoted ordinary text stays text while quoted qualifier values are decoded", () => {
  const request = compileTicketSearch(
    '"state:closed" "HTTP Cache" tag:"Needs review" state:"open"',
    tags,
  );
  assertEquals(request.query, "state:closed HTTP Cache");
  assertEquals(request.states, ["open"]);
  assertEquals(request.tag_ids, ["tag-2"]);
  assertEquals(
    compileTicketSearch("Don't lose Case", tags).query,
    "Don't lose Case",
  );
});

Deno.test("active tag names and stable IDs resolve canonically and deduplicate without mutation", () => {
  const request = compileTicketSearch(
    'tag:Ready tag:tag-1 tag:"Needs review" tag:tag-2 tag:Ready',
    tags,
  );
  assertEquals(request.tag_ids, ["tag-1", "tag-2"]);
  assertEquals(request.query, null);
  request.tag_ids?.push("not-in-catalog");
  assertEquals(compileTicketSearch("tag:Ready", tags).tag_ids, ["tag-1"]);
  assertEquals(tags[0].name, "Ready");
});

Deno.test("unknown deleted and case-mismatched tags fail instead of broadening search", () => {
  for (const value of ["Missing", "Retired", "tag-3", "ready"]) {
    assertSearchError(`tag:${value}`, "tag");
  }
  assertSearchError("tag:Ready", "active tag", []);
});

Deno.test("ambiguous tag names require a stable ID and stable IDs take precedence", () => {
  const catalog = [...tags, { ...tags[0], id: "tag-4" }, {
    ...tags[1],
    name: "tag-1",
  }];
  assertSearchError("tag:Ready", "stable tag ID", catalog);
  assertEquals(compileTicketSearch("tag:tag-1", catalog).tag_ids, ["tag-1"]);
});

Deno.test("assignee forms compile to generated principal snapshots without altering case", () => {
  for (
    const [input, expected] of [
      ["orchestrator", {
        kind: "workspace_agent",
        agent_key: "workspace-orchestrator",
      }],
      ["user/Account-A", { kind: "user", account_id: "Account-A" }],
      ["worker/Runtime-A/Worker-B", {
        kind: "worker",
        runtime_id: "Runtime-A",
        worker_id: "Worker-B",
      }],
    ] as const
  ) {
    const request = compileTicketSearch(`assignee:${input}`, tags);
    assertEquals(request.assigned_principal, expected);
    assertEquals(request.query, null);
    assertEquals(request.states, []);
  }
});

Deno.test("malformed assignees reject missing extra or whitespace-separated ID components", () => {
  for (
    const input of [
      "user",
      "user/",
      "user/a/b",
      "worker",
      "worker/r",
      "worker//w",
      "worker/r/",
      "worker/r/w/extra",
      "orchestrator/extra",
      "workspace-orchestrator",
      '"user/Account A"',
      '"worker/ /w"',
    ]
  ) {
    assertSearchError(`assignee:${input}`, "user/<account-id>");
  }
});

Deno.test("built-in reasons remain built-in even when quoted and do not imply closed state", () => {
  for (
    const value of ["completed", "not_planned", "duplicate", "unknown"] as const
  ) {
    for (const input of [`reason:${value}`, `reason:"${value}"`]) {
      const request = compileTicketSearch(input, tags);
      assertEquals(request.closure_reason, value);
      assertEquals(request.states, []);
      assertEquals(request.query, null);
    }
  }
});

Deno.test("custom reasons require quotes and preserve exact text", () => {
  for (
    const value of ["Superseded Plan", "Completed", "other", "  Exact  text  "]
  ) {
    assertEquals(
      compileTicketSearch(`reason:"${value}"`, tags).closure_reason,
      { other: value },
    );
  }
  assertSearchError("reason:Superseded", "double-quote custom exact text");
  assertSearchError("reason:other", "double-quote custom exact text");
});

Deno.test("recognized qualifiers reject empty values immediately after the colon", () => {
  for (const qualifier of ["state", "tag", "assignee", "reason"]) {
    for (const suffix of ["", '""', '"   "', " word"]) {
      assertSearchError(`${qualifier}:${suffix}`, `Empty ${qualifier}:`);
    }
  }
});

Deno.test("state close and other unrecognized state values are not aliases or text", () => {
  for (const value of ["close", "Closed", "OPEN", "done", "*", "open,closed"]) {
    assertSearchError(`state:${value}`, "use open, closed, or all");
  }
});

Deno.test("conflicting singleton filters fail rather than applying the last value", () => {
  for (
    const [input, qualifier] of [
      ["state:open state:closed", "state"],
      ["state:all state:open", "state"],
      ["assignee:user/a assignee:user/b", "assignee"],
      ["assignee:orchestrator assignee:worker/r/w", "assignee"],
      ["reason:completed reason:duplicate", "reason"],
      ['reason:"Custom" reason:"custom"', "reason"],
    ]
  ) {
    assertSearchError(input, `Conflicting ${qualifier}:`);
  }
});

Deno.test("equivalent singleton filters may repeat without changing their meaning", () => {
  const request = compileTicketSearch(
    'state:closed state:"closed" assignee:user/a assignee:"user/a" reason:completed reason:"completed"',
    tags,
  );
  assertEquals(request.states, ["closed"]);
  assertEquals(request.assigned_principal, { kind: "user", account_id: "a" });
  assertEquals(request.closure_reason, "completed");
});

Deno.test("unterminated quotes report an actionable error in filters and ordinary text", () => {
  for (
    const input of [
      'tag:"Needs review',
      'reason:"Custom',
      '"ordinary text',
      'url:"unfinished',
    ]
  ) {
    assertSearchError(input, "add a closing double quote");
  }
});

Deno.test("malformed quoted qualifier values fail rather than concatenating fragments", () => {
  for (
    const input of [
      'state:"open"extra',
      'tag:Ready"oops"',
      'reason:"custom""text"',
    ]
  ) {
    assertSearchError(input, "one pair of double quotes");
  }
});

Deno.test("mixed search compiles every supported filter and only remaining words", () => {
  assertEquals<unknown>(
    compileTicketSearch(
      'Fix state:closed tag:Ready HTTP assignee:worker/Runtime/Worker reason:"Superseded plan"',
      tags,
    ),
    {
      query: "Fix HTTP",
      states: ["closed"],
      tag_ids: ["tag-1"],
      assigned_principal: {
        kind: "worker",
        runtime_id: "Runtime",
        worker_id: "Worker",
      },
      handling_status: null,
      sort: "updated_desc",
      limit: 30,
      cursor: null,
      closure_reason: { other: "Superseded plan" },
    },
  );
});

Deno.test("Subject search preserves exact identity, rejects blanks and conflicts", () => {
  for (const subject of ["subject-a", "subject-b"]) {
    assertEquals(
      compileTicketSearch("assignee:subject:" + subject, tags)
        .assigned_principal,
      { kind: "subject", subject_id: subject },
    );
  }
  assertSearchError("assignee:subject:", "Subject identity");
  assertSearchError('assignee:"subject:   "', "Subject identity");
  assertSearchError(
    "assignee:subject:a assignee:subject:b",
    "Conflicting assignee:",
  );
});
