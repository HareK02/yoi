declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  parseMemoryDocumentResponse,
  parseMemoryStagingListResponse,
} from "../src/lib/workspace/memory/api.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
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

function fixture(origin: Record<string, unknown>) {
  return {
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
}

Deno.test("Memory document response requires the generated DTO fields", () => {
  assertEquals(
    parseMemoryDocumentResponse({
      body_md: "# Memory\n",
      created_at: "2026-09-01T00:00:00Z",
      updated_at: "2026-09-01T00:00:00Z",
      bytes: 9,
      record_source: "sqlite_workspace_authority.memory_document",
    }).bytes,
    9,
  );
  assertThrows(
    () => parseMemoryDocumentResponse({ body_md: "# Memory\n" }),
    "missing a required field",
  );
});

for (
  const [kind, fields] of [
    ["human_input", { account_id: "account-1" }],
    [
      "worker_input",
      {
        workspace_id: "workspace-1",
        runtime_id: "runtime-1",
        worker_id: "worker-1",
      },
    ],
    ["model_output", { runtime_id: "runtime-1", worker_id: "worker-1" }],
    ["tool_output", { runtime_id: "runtime-1", worker_id: "worker-1" }],
    ["legacy_unknown", {}],
  ] as const
) {
  Deno.test(`Memory staging parser preserves ${kind} origin`, () => {
    const parsed = parseMemoryStagingListResponse(fixture({ kind, ...fields }));
    assertEquals(parsed.items[0].record.evidence[0].origin, {
      kind,
      ...fields,
    });
    assertEquals(parsed.items[0].record.source_refs[0].origin, {
      kind,
      ...fields,
    });
  });
}

Deno.test("Memory staging parser preserves Flow origin fields", () => {
  const origin = {
    kind: "flow_instruction" as const,
    workspace_id: "workspace-1",
    runtime_id: "runtime-1",
    worker_id: "worker-1",
    flow_selector: "builtin:coder-review",
    flow_definition_id: "flow-1",
    flow_definition_revision: 7,
  };
  const parsed = parseMemoryStagingListResponse(fixture(origin));
  assertEquals(parsed.items[0].record.source_refs[0].origin, origin);
});

Deno.test("Memory staging parser rejects unknown or newer origin shapes", () => {
  assertThrows(
    () => parseMemoryStagingListResponse(fixture({ kind: "future_origin" })),
    "unknown Memory evidence origin kind",
  );
  assertThrows(
    () =>
      parseMemoryStagingListResponse(
        fixture({ kind: "human_input", future_field: "must not be accepted" }),
      ),
    "unknown field",
  );
});

Deno.test("Memory staging parser rejects malformed records and unbounded origins", () => {
  const malformed = fixture({ kind: "legacy_unknown" });
  malformed.items[0].record.source_refs[0].entry_range = [1] as unknown as [
    number,
    number,
  ];
  assertThrows(
    () => parseMemoryStagingListResponse(malformed),
    "two-item entry range",
  );
  assertThrows(
    () =>
      parseMemoryStagingListResponse(
        fixture({ kind: "worker_input", worker_id: "x".repeat(513) }),
      ),
    "exceeds the Memory origin limit",
  );
});
