declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  createSubjektivSubject,
  parseMemoryStagingListResponse,
  parseSubjektivMemoryListRevisionsResponse,
  parseSubjektivMemoryQueryResponse,
  parseSubjektivMemoryReadResponse,
  parseSubjektivResidentSurfaceResponse,
  parseSubjektivSubjectListResponse,
  parseSubjektivSubjectResponse,
  SubjektivSubjectCreateError,
  updateSubjektivSubjectBehavior,
  validateSubjektivSubjectCreateRequest,
} from "../src/lib/workspace/memory/api.ts";

function stable(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stable);
  if (typeof value === "object" && value !== null) {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, stable(item)]),
    );
  }
  return value;
}

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(stable(actual)) !== JSON.stringify(stable(expected))) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertThrows(fn: () => void, expectedMessage: string): void {
  try {
    fn();
  } catch (error) {
    if (error instanceof Error && error.message.includes(expectedMessage)) {
      return;
    }
    throw error;
  }
  throw new Error(`expected function to throw ${expectedMessage}`);
}

async function assertSubjectCreateRejects(
  fn: () => Promise<unknown>,
  expectedKind: SubjektivSubjectCreateError["kind"],
  expectedMessage: string,
  expectedStatus: number | null = null,
): Promise<void> {
  try {
    await fn();
  } catch (error) {
    if (!(error instanceof SubjektivSubjectCreateError)) throw error;
    assertEquals(error.kind, expectedKind);
    assertEquals(error.status, expectedStatus);
    if (!error.message.includes(expectedMessage)) {
      throw new Error(
        `expected ${JSON.stringify(error.message)} to include ${
          JSON.stringify(expectedMessage)
        }`,
      );
    }
    return;
  }
  throw new Error(`expected Subject creation to reject with ${expectedKind}`);
}

function subject(id = "subject-1") {
  return {
    id,
    role: "Release coordinator",
    behavior_md: "Prefer explicit evidence.",
    behavior_revision: 2,
    state: "active",
    store_revision: 12,
    created_at: "2026-09-01T00:00:00Z",
    updated_at: "2026-09-02T00:00:00Z",
  };
}

function queryItem(id = "memory-1") {
  return {
    id,
    revision: 3,
    kind: "decision",
    state: "active",
    claim: "Keep provenance typed.",
    excerpt: "Preserve source and derivation references.",
    updated_at: "2026-09-02T03:04:05Z",
  };
}

function detailFixture() {
  const origin = {
    kind: "flow_instruction",
    workspace_id: "workspace-1",
    runtime_id: "runtime-1",
    worker_id: "worker-1",
    flow_selector: "builtin:coder-review",
    flow_definition_id: "flow-1",
    flow_definition_revision: 7,
  };
  return {
    memory_id: "memory-1",
    revision: 2,
    current_revision: 3,
    kind: "decision",
    state: "resolved",
    claim: "Keep provenance typed.",
    body_md: "# Committed Memory\n\nBounded body segment.",
    why_useful: "Prevents origin loss.",
    staleness: null,
    change_reason: "Resolved after implementation.",
    created_at: "2026-09-01T00:00:00Z",
    updated_at: "2026-09-02T03:04:05Z",
    body_offset: 0,
    body_byte_offset: 0,
    body_next_offset: 2,
    body_next_byte_offset: 0,
    body_truncated: true,
    source_candidate_ids: ["candidate-1"],
    source_candidates: [{
      candidate_id: "candidate-1",
      evidence: [{
        id: "evidence-1",
        kind: "message",
        entry_range: [10, 12],
        origin,
        excerpt: "Use exact DTOs.",
        summary: "The decision was explicit.",
      }],
      evidence_total: 2,
      evidence_truncated: true,
      source_refs: [{
        session_id: "session-1",
        segment_id: "segment-1",
        entry_range: [10, 12],
        evidence_id: "evidence-1",
        origin,
        evidence_kind: "message",
        label: "Decision discussion",
        summary: "Bounded host summary.",
      }],
      source_refs_total: 2,
      source_refs_truncated: true,
    }],
    derived_from: [{ memory_id: "memory-parent", revision: 4 }],
    evidence_next_cursor: "evidence-page-2",
    evidence_has_more: true,
  };
}

Deno.test("Subject parsers enforce identity, exact enums, safe revisions, and bounds", () => {
  assertEquals(
    parseSubjektivSubjectResponse(subject(), "subject-1"),
    subject(),
  );
  assertThrows(
    () => parseSubjektivSubjectResponse(subject(), "another-subject"),
    "identity does not match",
  );
  assertThrows(
    () => parseSubjektivSubjectResponse({ ...subject(), state: "paused" }),
    "unknown Subject state",
  );
  assertThrows(
    () =>
      parseSubjektivSubjectResponse({
        ...subject(),
        store_revision: Number.MAX_SAFE_INTEGER + 1,
      }),
    "safe integer",
  );
  assertThrows(
    () => parseSubjektivSubjectResponse({ ...subject(), id: "x".repeat(513) }),
    "bounded identifier",
  );
  assertThrows(
    () => parseSubjektivSubjectResponse({ ...subject(), future: true }),
    "unknown field",
  );
  assertThrows(
    () =>
      parseSubjektivSubjectListResponse({
        limit: 100,
        items: Array.from({ length: 101 }, (_, index) =>
          subject(`subject-${index}`)),
        has_more: false,
      }),
    "bounded array",
  );
});

Deno.test("Subject list parser preserves the exact bounded response", () => {
  const response = { limit: 100, items: [subject()], has_more: false };
  assertEquals(parseSubjektivSubjectListResponse(response), response);
  const nextPage = {
    limit: 1,
    items: [subject()],
    next_cursor: "subjektiv.subjects.next",
    has_more: true,
  };
  assertEquals(parseSubjektivSubjectListResponse(nextPage), nextPage);
  assertThrows(
    () =>
      parseSubjektivSubjectListResponse({
        limit: 1,
        items: [subject()],
        has_more: true,
      }),
    "cursor does not match has_more",
  );
  assertThrows(
    () =>
      parseSubjektivSubjectListResponse({
        limit: 100,
        items: [subject("duplicate"), subject("duplicate")],
        has_more: false,
      }),
    "Subject ids must be unique",
  );
});

Deno.test("Subject create request mirrors Backend role validation without rewriting input", () => {
  assertEquals(
    validateSubjektivSubjectCreateRequest({ role: "  Release coordinator  " }),
    { role: "  Release coordinator  ", behavior_md: "" },
  );
  assertThrows(
    () => validateSubjektivSubjectCreateRequest({ role: "   " }),
    "must not be empty",
  );
  assertThrows(
    () => validateSubjektivSubjectCreateRequest({ role: "line\nbreak" }),
    "control characters",
  );
  assertThrows(
    () => validateSubjektivSubjectCreateRequest({ role: "界".repeat(86) }),
    "at most 256 bytes",
  );
  assertThrows(
    () =>
      validateSubjektivSubjectCreateRequest({
        role: "Reviewer",
        behavior_md: " ".repeat(3),
      }),
    "empty or contain non-whitespace",
  );
  assertThrows(
    () =>
      validateSubjektivSubjectCreateRequest({
        role: "Reviewer",
        behavior_md: "x".repeat(16 * 1024 + 1),
      }),
    "at most 16384 bytes",
  );
});

Deno.test("Subject behavior update uses CAS and preserves exact text", async () => {
  const requests: Array<{ path: string; init?: RequestInit }> = [];
  const fetchFn = (async (path: string | URL | Request, init?: RequestInit) => {
    requests.push({ path: String(path), init });
    return Response.json({
      ...subject("subject/one"),
      behavior_md: "Line one.\nLine two.",
      behavior_revision: 3,
    });
  }) as typeof fetch;

  const updated = await updateSubjektivSubjectBehavior(
    fetchFn,
    "workspace one",
    "subject/one",
    { expected_behavior_revision: 2, behavior_md: "Line one.\nLine two." },
  );
  assertEquals(updated.behavior_revision, 3);
  assertEquals(updated.behavior_md, "Line one.\nLine two.");
  assertEquals(
    requests[0].path,
    "/api/w/workspace%20one/subjektiv/subjects/subject%2Fone/behavior",
  );
  assertEquals(requests[0].init?.method, "PATCH");
  assertEquals(
    requests[0].init?.body,
    '{"expected_behavior_revision":2,"behavior_md":"Line one.\\nLine two."}',
  );
});

Deno.test("Subject behavior update reports conflicts without claiming Worker application", async () => {
  const conflictFetch =
    (async () =>
      Response.json({ message: "revision conflict" }, {
        status: 409,
      })) as typeof fetch;
  await assertSubjectCreateRejects(
    () =>
      updateSubjektivSubjectBehavior(conflictFetch, "workspace", "subject", {
        expected_behavior_revision: 1,
        behavior_md: "new",
      }),
    "rejected",
    "changed elsewhere",
    409,
  );
});

Deno.test("Subject create client sends the exact typed body once and parses the created Subject", async () => {
  const requests: Array<{ path: string; init?: RequestInit }> = [];
  const fetchFn = (async (path: string | URL | Request, init?: RequestInit) => {
    requests.push({ path: String(path), init });
    return Response.json({
      ...subject("subject-created"),
      role: "  Release coordinator  ",
      behavior_md: "Stay concise.",
    }, { status: 201 });
  }) as typeof fetch;

  const created = await createSubjektivSubject(fetchFn, "workspace one", {
    role: "  Release coordinator  ",
    behavior_md: "Stay concise.",
  });
  assertEquals(created, {
    ...subject("subject-created"),
    role: "  Release coordinator  ",
    behavior_md: "Stay concise.",
  });
  assertEquals(requests.length, 1);
  assertEquals(requests[0].path, "/api/w/workspace%20one/subjektiv/subjects");
  assertEquals(requests[0].init?.method, "POST");
  assertEquals(
    requests[0].init?.body,
    '{"role":"  Release coordinator  ","behavior_md":"Stay concise."}',
  );
});

Deno.test("Subject create client distinguishes rejected and unknown outcomes without retrying", async () => {
  let rejectedCalls = 0;
  const rejectedFetch = (async () => {
    rejectedCalls += 1;
    return Response.json(
      {
        error: "Forbidden",
        message: "workspace permission denied",
        diagnostics: [],
      },
      { status: 403 },
    );
  }) as typeof fetch;
  await assertSubjectCreateRejects(
    () =>
      createSubjektivSubject(rejectedFetch, "workspace-1", {
        role: "Reviewer",
      }),
    "rejected",
    "workspace permission denied",
    403,
  );
  assertEquals(rejectedCalls, 1);

  let serverErrorCalls = 0;
  const serverErrorFetch = (async () => {
    serverErrorCalls += 1;
    return Response.json({ error: "Internal Server Error" }, { status: 500 });
  }) as typeof fetch;
  await assertSubjectCreateRejects(
    () =>
      createSubjektivSubject(serverErrorFetch, "workspace-1", {
        role: "Reviewer",
      }),
    "unknown_outcome",
    "may have been created",
    500,
  );
  assertEquals(serverErrorCalls, 1);

  let mismatchCalls = 0;
  const mismatchFetch = (async () => {
    mismatchCalls += 1;
    return Response.json(subject("subject-created"), { status: 201 });
  }) as typeof fetch;
  await assertSubjectCreateRejects(
    () =>
      createSubjektivSubject(mismatchFetch, "workspace-1", {
        role: "Reviewer",
      }),
    "unknown_outcome",
    "could not be confirmed",
  );
  assertEquals(mismatchCalls, 1);

  let unknownCalls = 0;
  const unknownFetch = (async () => {
    unknownCalls += 1;
    throw new TypeError("connection reset");
  }) as typeof fetch;
  await assertSubjectCreateRejects(
    () =>
      createSubjektivSubject(unknownFetch, "workspace-1", { role: "Reviewer" }),
    "unknown_outcome",
    "may have been created",
  );
  assertEquals(unknownCalls, 1);
});

Deno.test("Resident surface parser accepts ready-empty and enforces snapshot invariants", () => {
  const readyEmpty = {
    subject_id: "subject-1",
    availability: "ready",
    snapshot: {
      snapshot_id: "snapshot-empty",
      body_md: "",
      memory_refs: [],
      built_from_store_revision: 0,
      created_at: "2026-09-02T00:00:00Z",
    },
  };
  assertEquals(
    parseSubjektivResidentSurfaceResponse(readyEmpty, "subject-1"),
    readyEmpty,
  );
  assertEquals(
    parseSubjektivResidentSurfaceResponse({
      subject_id: "subject-1",
      availability: "ungenerated",
    }),
    { subject_id: "subject-1", availability: "ungenerated" },
  );
  assertThrows(
    () =>
      parseSubjektivResidentSurfaceResponse({
        subject_id: "subject-1",
        availability: "ready",
      }),
    "requires a snapshot",
  );
  for (const availability of ["ungenerated", "stale", "failed"]) {
    assertThrows(
      () =>
        parseSubjektivResidentSurfaceResponse({
          ...readyEmpty,
          availability,
        }),
      "must not include a snapshot",
    );
  }
  assertThrows(
    () =>
      parseSubjektivResidentSurfaceResponse({
        subject_id: "subject-1",
        availability: "refreshing",
      }),
    "unknown resident surface availability",
  );
  assertThrows(
    () => parseSubjektivResidentSurfaceResponse(readyEmpty, "subject-2"),
    "identity does not match",
  );
});

Deno.test("Current Memory list parser rejects unknown variants, unsafe integers, cursors, and oversized pages", () => {
  const response = {
    items: [queryItem()],
    has_more: true,
    next_cursor: "memory-page-2",
  };
  assertEquals(parseSubjektivMemoryQueryResponse(response), response);
  assertThrows(
    () =>
      parseSubjektivMemoryQueryResponse({
        ...response,
        items: [{ ...queryItem(), state: "pending" }],
      }),
    "unknown Memory state",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryQueryResponse({
        ...response,
        items: [{ ...queryItem(), kind: "future_kind" }],
      }),
    "unknown Memory candidate kind",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryQueryResponse({
        ...response,
        items: [{ ...queryItem(), revision: 0 }],
      }),
    "positive safe integer",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryQueryResponse({
        items: [queryItem()],
        has_more: true,
      }),
    "cursor does not match",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryQueryResponse({
        items: Array.from({ length: 101 }, (_, index) =>
          queryItem(`memory-${index}`)),
        has_more: false,
      }),
    "bounded array",
  );
});

Deno.test("Memory detail parser preserves body/evidence continuation, sources, origins, and derivation", () => {
  const fixture = detailFixture();
  const parsed = parseSubjektivMemoryReadResponse(fixture, "memory-1");
  assertEquals(parsed, fixture);
  assertEquals(parsed.body_next_offset, 2);
  assertEquals(parsed.body_next_byte_offset, 0);
  assertEquals(parsed.evidence_next_cursor, "evidence-page-2");
  assertEquals(
    parsed.source_candidates[0].evidence[0].origin,
    fixture.source_candidates[0].evidence[0].origin,
  );
  assertEquals(
    parsed.source_candidates[0].source_refs,
    fixture.source_candidates[0].source_refs,
  );
  assertEquals(parsed.derived_from, [{
    memory_id: "memory-parent",
    revision: 4,
  }]);
});

Deno.test("Memory detail parser fails closed on identity, variants, bounds, and continuation mismatch", () => {
  const fixture = detailFixture();
  assertThrows(
    () => parseSubjektivMemoryReadResponse(fixture, "memory-2"),
    "identity does not match",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryReadResponse({
        ...fixture,
        current_revision: Number.MAX_SAFE_INTEGER + 1,
      }),
    "safe integer",
  );
  const missingBodyContinuation = structuredClone(fixture) as Record<
    string,
    unknown
  >;
  delete missingBodyContinuation.body_next_byte_offset;
  assertThrows(
    () => parseSubjektivMemoryReadResponse(missingBodyContinuation),
    "body continuation fields",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryReadResponse({
        ...fixture,
        source_candidate_ids: ["different-candidate"],
      }),
    "source candidate ids do not match",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryReadResponse({
        ...fixture,
        source_candidates: [{
          ...fixture.source_candidates[0],
          evidence: [{
            ...fixture.source_candidates[0].evidence[0],
            entry_range: [12, 10],
          }],
        }],
      }),
    "must be ordered",
  );
  const unknownOrigin = structuredClone(fixture);
  unknownOrigin.source_candidates[0].evidence[0].origin.kind = "future_origin";
  assertThrows(
    () => parseSubjektivMemoryReadResponse(unknownOrigin),
    "unknown Memory evidence origin kind",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryReadResponse({
        ...fixture,
        evidence_has_more: false,
      }),
    "cursor does not match",
  );
});

Deno.test("Memory revision parser preserves immutable states and checks response identity", () => {
  const response = {
    memory_id: "memory-1",
    current_revision: 3,
    items: [{
      revision: 3,
      kind: "decision",
      state: "active",
      claim: "Current claim.",
      change_reason: "Clarified wording.",
      updated_at: "2026-09-03T00:00:00Z",
    }, {
      revision: 2,
      kind: "decision",
      state: "resolved",
      claim: "Earlier claim.",
      change_reason: "Resolved.",
      updated_at: "2026-09-02T00:00:00Z",
    }, {
      revision: 1,
      kind: "decision",
      state: "retracted",
      claim: "Original claim.",
      change_reason: "Retracted.",
      updated_at: "2026-09-01T00:00:00Z",
    }],
    has_more: false,
  };
  assertEquals(
    parseSubjektivMemoryListRevisionsResponse(response, "memory-1"),
    response,
  );
  assertThrows(
    () => parseSubjektivMemoryListRevisionsResponse(response, "memory-2"),
    "identity does not match",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryListRevisionsResponse({
        ...response,
        items: [{ ...response.items[0], state: "archived" }],
      }),
    "unknown Memory state",
  );
  assertThrows(
    () =>
      parseSubjektivMemoryListRevisionsResponse({
        ...response,
        items: [{ ...response.items[0], revision: 4 }],
      }),
    "future revision",
  );
});

Deno.test("Legacy staging parser remains strict for deprecated compatibility", () => {
  const origin = { kind: "worker_input", worker_id: "worker-1" };
  const response = {
    limit: 100,
    returned_count: 1,
    total_valid_count: 1,
    invalid_count: 0,
    truncated: false,
    order: "imported_at_desc_candidate_id_asc",
    record_authority: "sqlite_workspace_authority.memory_staging",
    items: [{
      id: "candidate-1",
      byte_len: 128,
      record: {
        schema_version: 2,
        id: "candidate-1",
        extract_run_id: "extract-run-1",
        source: { segment_id: "segment-1", range: [10, 20] },
        kind: "decision",
        claim: "Keep provenance typed.",
        why_useful: "Prevents origin loss.",
        staleness: null,
        evidence: [{
          id: "evidence-1",
          kind: "message",
          entry_range: [10, 10],
          origin,
          excerpt: null,
          summary: "bounded summary",
        }],
        source_refs: [{
          session_id: "session-1",
          segment_id: "segment-1",
          entry_range: [10, 10],
          evidence_id: "evidence-1",
          origin,
          evidence_kind: "message",
          label: "source",
          summary: null,
        }],
      },
    }],
    diagnostics: [],
  };
  assertEquals(parseMemoryStagingListResponse(response), response);
  assertThrows(
    () =>
      parseMemoryStagingListResponse({
        ...response,
        items: [{
          ...response.items[0],
          record: {
            ...response.items[0].record,
            source: { segment_id: "segment-1", range: [20, 10] },
          },
        }],
      }),
    "must be ordered",
  );
});
