import {
  parseCompanionMessageResponse,
  parseCompanionStatusResponse,
  parseCompanionTranscriptProjection,
} from "./api.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertThrows(fn: () => unknown, message: string): void {
  try {
    fn();
  } catch {
    return;
  }
  throw new Error(message);
}

const worker = {
  subject: {
    kind: "runtime_worker",
    runtime_id: "arcadia",
    worker_id: "worker-7",
  },
  resource_key: "W-7",
  display_name: "Companion",
  profile: "builtin:companion",
  status: "idle",
};

Deno.test("Companion status boundary accepts every public lifecycle state", () => {
  for (const state of ["idle", "running", "stopped"] as const) {
    const parsed = parseCompanionStatusResponse({
      state,
      worker,
      transport: {
        mode: "worker_runtime",
        available: state !== "stopped",
      },
      diagnostics: [],
    });
    assertEquals(parsed.state, state);
    assertEquals(parsed.worker?.subject, worker.subject);
    assertEquals(parsed.worker?.resource_key, "W-7");
    assertEquals(parsed.worker?.display_name, "Companion");
  }
});

Deno.test("Companion message boundary accepts accepted and rejected fixtures", () => {
  assertEquals(
    parseCompanionMessageResponse({
      state: "accepted",
      message: "accepted",
    }),
    { state: "accepted", message: "accepted" },
  );
  assertEquals(
    parseCompanionMessageResponse({
      state: "rejected",
      message: "rejected",
    }),
    { state: "rejected", message: "rejected" },
  );
  assertThrows(
    () =>
      parseCompanionMessageResponse({
        state: "accepted",
        message: "accepted",
        provider_request_id: "private-request",
      }),
    "private message response fields should be rejected",
  );
});

Deno.test("Companion transcript boundary accepts only bounded user-visible items", () => {
  const fixture = {
    state: "idle" as const,
    start: 0,
    limit: 2,
    total: 2,
    next: null,
    items: [
      {
        sequence: 1,
        role: "user" as const,
        content: "hello",
        created_at: "2026-08-31T00:00:00Z",
      },
      {
        sequence: 2,
        role: "assistant" as const,
        content: "hi",
        created_at: "2026-08-31T00:00:01Z",
      },
    ],
  };
  assertEquals(parseCompanionTranscriptProjection(fixture), fixture);
  assertEquals(
    parseCompanionTranscriptProjection({
      state: "stopped",
      start: 0,
      limit: 0,
      total: 0,
      next: null,
      items: [],
    }),
    {
      state: "stopped",
      start: 0,
      limit: 0,
      total: 0,
      next: null,
      items: [],
    },
  );

  assertThrows(
    () =>
      parseCompanionTranscriptProjection({
        ...fixture,
        items: [...fixture.items, fixture.items[0]],
      }),
    "items beyond the declared limit should be rejected",
  );
});

Deno.test("Companion transcript boundary rejects system and private fields", () => {
  const base = {
    state: "idle",
    start: 0,
    limit: 1,
    total: 1,
    next: null,
  };
  assertThrows(
    () =>
      parseCompanionTranscriptProjection({
        ...base,
        items: [{
          sequence: 1,
          role: "system",
          content: "raw system prompt",
          created_at: "2026-08-31T00:00:00Z",
        }],
      }),
    "system transcript content should be rejected",
  );
  assertThrows(
    () =>
      parseCompanionTranscriptProjection({
        ...base,
        items: [{
          sequence: 1,
          role: "assistant",
          content: "visible",
          created_at: "2026-08-31T00:00:00Z",
          reasoning: "hidden",
          credential: "secret",
          provider_session_id: "private-session",
        }],
      }),
    "private transcript fields should be rejected",
  );
});

Deno.test("Companion status boundary does not use display_name as Worker identity", () => {
  const fixture = {
    state: "idle",
    worker: { ...worker, display_name: "W-999" },
    transport: { mode: "worker_runtime", available: true },
    diagnostics: [],
  };
  const parsed = parseCompanionStatusResponse(fixture);
  assertEquals(parsed.worker?.subject, worker.subject);
  assertEquals(parsed.worker?.resource_key, "W-7");
  assertEquals(parsed.worker?.display_name, "W-999");

  assertThrows(
    () =>
      parseCompanionStatusResponse({
        ...fixture,
        worker: { ...worker, resource_key: "Companion" },
      }),
    "display names must not substitute for canonical Worker resource keys",
  );
});
