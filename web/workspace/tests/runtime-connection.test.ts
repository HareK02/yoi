declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  parseRuntimeConnectionTestResponse,
  testRuntimeConnection,
} from "../src/lib/workspace/api/runtime-connection.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function compatibleResponse(): Record<string, unknown> {
  return {
    workspace_id: "workspace-a",
    runtime_id: "runtime-a",
    checked_at: "2026-09-01T12:00:00Z",
    status: "compatible",
    failure_kind: null,
    expected_protocol_version: 1,
    actual_protocol_version: 1,
    diagnostics: [],
  };
}

Deno.test("runtime connection response accepts the exact compatible contract", () => {
  assertEquals(
    parseRuntimeConnectionTestResponse(compatibleResponse()),
    compatibleResponse(),
  );
});

Deno.test("runtime connection response rejects unknown fields and incoherent compatibility", () => {
  assertEquals(
    parseRuntimeConnectionTestResponse({
      ...compatibleResponse(),
      capabilities: ["shell"],
    }),
    null,
  );
  assertEquals(
    parseRuntimeConnectionTestResponse({
      ...compatibleResponse(),
      actual_protocol_version: 2,
    }),
    null,
  );
  assertEquals(
    parseRuntimeConnectionTestResponse({
      ...compatibleResponse(),
      failure_kind: "timeout",
    }),
    null,
  );
});

Deno.test("runtime connection response rejects unknown failure kinds and unbounded diagnostics", () => {
  const failed = {
    ...compatibleResponse(),
    status: "failed",
    failure_kind: "future_failure",
    actual_protocol_version: null,
    diagnostics: [],
  };
  assertEquals(parseRuntimeConnectionTestResponse(failed), null);
  assertEquals(
    parseRuntimeConnectionTestResponse({
      ...failed,
      failure_kind: "timeout",
      diagnostics: Array.from({ length: 17 }, () => ({
        code: "timeout",
        severity: "error",
        message: "Timed out",
      })),
    }),
    null,
  );
});

Deno.test("runtime connection request rejects a mismatched response identity", async () => {
  const fetchImpl = (() =>
    Promise.resolve(
      new Response(
        JSON.stringify({ ...compatibleResponse(), runtime_id: "runtime-b" }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    )) as typeof fetch;
  let message = "";
  try {
    await testRuntimeConnection("workspace-a", "runtime-a", fetchImpl);
  } catch (error) {
    message = error instanceof Error ? error.message : String(error);
  }
  assertEquals(
    message,
    "Connection test response did not match the selected Runtime",
  );
});
