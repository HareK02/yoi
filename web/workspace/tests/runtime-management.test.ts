declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import type { RepositoryApiError } from "../src/lib/generated/repository-api.ts";
import type { RuntimeManagementApiError } from "../src/lib/generated/runtime-api.ts";
import {
  createRemoteRuntime,
  parseRuntimeRemovalOperationResponse,
  parseRuntimeTrustConflict,
  parseRuntimeTrustKeyRevealResponse,
  parseWorkspaceRuntimeDetail,
  parseWorkspaceRuntimeList,
  previewRuntimePublicKeyFingerprint,
  removeRemoteRuntime,
  revokeRuntimeTrustKey,
  RuntimeRemovalAttempt,
  RuntimeTrustConflictError,
  RuntimeTrustRequestError,
  RuntimeTrustRouteFence,
  updateRemoteRuntime,
} from "../src/lib/workspace/api/runtime-management.ts";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertThrows(operation: () => unknown, expected: string): void {
  try {
    operation();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes(expected)) return;
    throw new Error(
      `expected error containing ${expected}, received ${message}`,
    );
  }
  throw new Error("expected operation to throw");
}

function runtime() {
  return {
    management: {
      built_in: false,
      config_managed: true,
      removable: false,
      endpoint_configured: true,
      token_ref_configured: false,
      binding: {
        binding_id: "binding-3",
        revoked_at: null,
      },
    },
    runtime_id: "arcadia",
    label: "Arcadia",
    kind: "remote",
    status: "started",
    source: {
      kind: "remote_http",
      status: "active",
      identity_authority: "server_runtime_configuration",
      note: "Configured by Server authority",
    },
    host_ids: ["host-a"],
    worker_creation_available: true,
    os: "linux",
    arch: "x86_64",
    diagnostics: [],
  };
}

function detail() {
  return {
    workspace_id: "workspace-a",
    runtime: runtime(),
    endpoint: "https://runtime.example.test",
    trust_key: {
      status: "active",
      fingerprint: "SHA256:current",
      binding_id: "binding-3",
      created_at: "2026-09-01T12:00:00Z",
      updated_at: "2026-09-01T13:00:00Z",
      revoked_at: null,
    },
    recent_audit: [{
      action: "created",
      actor_account_id: "account-a",
      old_fingerprint: null,
      new_fingerprint: "SHA256:current",
      binding_id: "binding-3",
      at: "2026-09-01T13:00:00Z",
    }],
  };
}

Deno.test("Runtime list and detail parsers return generated Runtime DTO shapes", () => {
  const list = parseWorkspaceRuntimeList({
    workspace_id: "workspace-a",
    limit: 200,
    items: [runtime()],
    source: "workspace-control-plane",
    diagnostics: [],
  });
  assert(
    list.items[0]?.runtime_id === "arcadia",
    "Runtime ID was not preserved",
  );

  assert(
    list.items[0]?.management.binding?.binding_id === "binding-3",
    "binding identity was not preserved",
  );

  const parsed = parseWorkspaceRuntimeDetail(detail());
  assert(
    parsed.trust_key.binding_id === "binding-3",
    "binding identity was not preserved",
  );
  assert(
    parsed.recent_audit[0]?.binding_id === "binding-3",
    "audit binding_id was not normalized",
  );
});

Deno.test("Runtime list parser accepts the built-in Runtime's internal binding", () => {
  const embedded = runtime();
  embedded.runtime_id = "embedded";
  embedded.label = "Embedded Runtime";
  embedded.kind = "embedded";
  embedded.management.built_in = true;
  embedded.management.endpoint_configured = false;
  const binding = embedded.management.binding as Partial<
    typeof embedded.management.binding
  >;
  delete binding.revoked_at;

  const list = parseWorkspaceRuntimeList({
    workspace_id: "workspace-a",
    limit: 200,
    items: [embedded],
    source: "workspace-control-plane",
    diagnostics: [],
  });
  assert(
    list.items[0]?.management.binding?.revoked_at === null,
    "built-in Runtime binding was not preserved",
  );
});

Deno.test("Runtime list accepts migrated remote bindings without Workspace identity metadata", () => {
  const migrated = runtime();
  const binding = migrated.management.binding as Partial<
    typeof migrated.management.binding
  >;
  delete binding.revoked_at;
  const list = parseWorkspaceRuntimeList({
    workspace_id: "workspace-a",
    limit: 200,
    items: [migrated],
    source: "fixture",
    diagnostics: [],
  });
  assert(
    list.items[0]?.management.binding?.binding_id === "binding-3",
    "migrated binding was rejected",
  );
  assert(
    list.items[0]?.management.binding?.revoked_at === null,
    "missing revocation should remain unrevoked",
  );
});

Deno.test("Runtime binding rejects cached approval and copied Workspace identity fields", () => {
  for (
    const field of [
      "state",
      "connection_state",
      "verification",
      "workspace_key_id",
      "workspace_trust_id",
    ]
  ) {
    const payload = detail();
    Object.assign(payload.runtime.management.binding, { [field]: "obsolete" });
    assertThrows(
      () => parseWorkspaceRuntimeDetail(payload),
      "not part of the wire contract",
    );
  }
});

Deno.test("Runtime validators reject unknown object keys and enum variants", () => {
  assertThrows(
    () => parseWorkspaceRuntimeDetail({ ...detail(), head_tree: "stale" }),
    "head_tree is not part",
  );

  const futureSource = structuredClone(detail());
  futureSource.runtime.source.kind = "future_transport";
  assertThrows(
    () => parseWorkspaceRuntimeDetail(futureSource),
    "contains an unknown enum value",
  );

  assertThrows(
    () =>
      parseRuntimeTrustConflict({
        error: "future_conflict",
        message: "conflict",
        current_binding_id: "binding-4",
        current_fingerprint: "SHA256:new",
      }),
    "contains an unknown enum value",
  );
});

Deno.test("Runtime validators reject invalid binding identities and bounded collection overflow", () => {
  const invalidBinding = structuredClone(detail());
  Object.assign(invalidBinding.trust_key, { binding_id: 1 });
  assertThrows(
    () => parseWorkspaceRuntimeDetail(invalidBinding),
    "must be a string",
  );

  const tooMuchAudit = structuredClone(detail());
  tooMuchAudit.recent_audit = Array.from(
    { length: 21 },
    () => structuredClone(detail().recent_audit[0]),
  );
  assertThrows(
    () => parseWorkspaceRuntimeDetail(tooMuchAudit),
    "must contain at most 20 items",
  );

  const tooManyItems = Array.from({ length: 201 }, () => runtime());
  assertThrows(
    () =>
      parseWorkspaceRuntimeList({
        workspace_id: "workspace-a",
        limit: 200,
        items: tooManyItems,
        source: "workspace-control-plane",
        diagnostics: [],
      }),
    "must contain at most 200 items",
  );
});

Deno.test("Runtime detail rejects unbounded strings and incoherent trust state", () => {
  assertThrows(
    () =>
      parseRuntimeTrustKeyRevealResponse({
        public_key: "x".repeat(16 * 1024 + 1),
      }),
    "must be at most 16384 UTF-8 bytes",
  );

  const activeWithoutFingerprint = structuredClone(detail()) as Record<
    string,
    unknown
  >;
  (activeWithoutFingerprint.trust_key as Record<string, unknown>).fingerprint =
    null;
  assertThrows(
    () => parseWorkspaceRuntimeDetail(activeWithoutFingerprint),
    "must include fingerprint",
  );
});

Deno.test("mismatched revoke fingerprint never sends a request", async () => {
  let requests = 0;
  const fetchImpl: typeof fetch = () => {
    requests += 1;
    return Promise.reject(new Error("request must not be sent"));
  };
  let rejected = false;
  try {
    await revokeRuntimeTrustKey(
      "workspace-a",
      "runtime-a",
      { expected_binding_id: "binding-3" },
      "sha256:current",
      "sha256:different",
      fetchImpl,
    );
  } catch (error) {
    rejected = error instanceof Error &&
      error.message.includes("current fingerprint exactly");
  }
  assert(rejected, "mismatched fingerprint should be rejected locally");
  assert(requests === 0, "mismatched fingerprint sent a revoke request");
});

Deno.test("Runtime metadata update never sends public key authority", async () => {
  let requestedUrl = "";
  let requestedMethod = "";
  let requestedBody: unknown = null;
  const fetchImpl = ((input: string | URL | Request, init?: RequestInit) => {
    requestedUrl = String(input);
    requestedMethod = init?.method ?? "GET";
    requestedBody = JSON.parse(String(init?.body));
    return Promise.resolve(Response.json(detail()));
  }) as typeof fetch;

  await updateRemoteRuntime(
    "workspace-a",
    "arcadia",
    {
      display_name: "Updated Runtime",
      endpoint: "https://runtime.example.test/v2",
    },
    fetchImpl,
  );

  assert(
    requestedUrl === "/api/w/workspace-a/runtimes/arcadia",
    `unexpected update URL: ${requestedUrl}`,
  );
  assert(requestedMethod === "POST", "Runtime update must use POST");
  assert(
    JSON.stringify(requestedBody) ===
      JSON.stringify({
        display_name: "Updated Runtime",
        endpoint: "https://runtime.example.test/v2",
      }),
    `unexpected update body: ${JSON.stringify(requestedBody)}`,
  );
  const body = requestedBody as Record<string, unknown>;
  assert(!("public_bundle" in body), "metadata update sent public_bundle");
  assert(!("public_key" in body), "metadata update sent public_key");
});

Deno.test("Runtime removal attempt retains its id across response-loss retry", async () => {
  const attempt = new RuntimeRemovalAttempt();
  const operationIds: string[] = [];
  let calls = 0;
  const submit = async () => {
    const operationId = attempt.operationId(() => "stable-removal-operation");
    operationIds.push(operationId);
    const operation = await removeRemoteRuntime(
      "workspace-a",
      "runtime-a",
      { operation_id: operationId, expected_binding_id: "binding-4" },
      () => {
        calls += 1;
        if (calls === 1) return Promise.reject(new Error("response lost"));
        return Promise.resolve(Response.json({
          operation_id: operationId,
          workspace_id: "workspace-a",
          runtime_id: "runtime-a",
          state: "succeeded",
          binding_removed: true,
          runtime_registration_removed: true,
          created_at: "2026-01-01T00:00:00Z",
          updated_at: "2026-01-01T00:00:01Z",
          completed_at: "2026-01-01T00:00:01Z",
        }));
      },
    );
    attempt.complete(operation.operation_id);
  };

  try {
    await submit();
    throw new Error("expected response-loss retry to fail");
  } catch (error) {
    assert(
      error instanceof Error && error.message.includes("response lost"),
      `unexpected response-loss error: ${String(error)}`,
    );
  }
  await submit();
  assert(
    operationIds.length === 2 &&
      operationIds.every((id) => id === "stable-removal-operation"),
    `response-loss retry changed operation id: ${operationIds.join(",")}`,
  );
  assert(
    attempt.operationId(() => "next-removal-operation") ===
      "next-removal-operation",
    "authoritative success did not clear the completed operation id",
  );
});

Deno.test("Runtime removal uses the Workspace-scoped operation route", async () => {
  let requestedUrl = "";
  let requestedMethod = "";
  let requestedBody: unknown = null;
  const fetchImpl = ((input: string | URL | Request, init?: RequestInit) => {
    requestedUrl = String(input);
    requestedMethod = init?.method ?? "GET";
    requestedBody = JSON.parse(String(init?.body));
    return Promise.resolve(Response.json({
      operation_id: "remove-runtime-a",
      workspace_id: "workspace a",
      runtime_id: "runtime/a",
      state: "succeeded",
      binding_removed: true,
      runtime_registration_removed: true,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:01Z",
      completed_at: "2026-01-01T00:00:01Z",
    }));
  }) as typeof fetch;

  const operation = await removeRemoteRuntime(
    "workspace a",
    "runtime/a",
    { operation_id: "remove-runtime-a", expected_binding_id: "binding-7" },
    fetchImpl,
  );

  assert(
    requestedUrl === "/api/w/workspace%20a/runtimes/runtime%2Fa",
    `unexpected removal URL: ${requestedUrl}`,
  );
  assert(requestedMethod === "DELETE", "Runtime removal must use DELETE");
  assert(
    JSON.stringify(requestedBody) === JSON.stringify({
      operation_id: "remove-runtime-a",
      expected_binding_id: "binding-7",
    }),
    "Runtime removal request body drifted",
  );
  assert(operation.state === "succeeded", "Runtime removal did not complete");

  assertThrows(
    () =>
      parseRuntimeRemovalOperationResponse({
        ...operation,
        unknown: "rejected",
      }),
    "not part of the wire contract",
  );
});

Deno.test("Runtime route fence rejects a delayed reveal from the prior Runtime", async () => {
  const fence = new RuntimeTrustRouteFence();
  fence.enter("runtime-a");
  const operation = fence.capture("runtime-a");
  let renderedKey: string | null = null;
  let resolveReveal!: (key: string) => void;
  const delayedReveal = new Promise<string>((resolve) => {
    resolveReveal = resolve;
  }).then((key) => {
    if (fence.isCurrent(operation, "runtime-b")) renderedKey = key;
  });

  fence.enter("runtime-b");
  resolveReveal("runtime-a-public-key");
  await delayedReveal;
  assert(
    renderedKey === null,
    "Runtime A key rendered after navigating to Runtime B",
  );
});

Deno.test("Runtime public key preview matches the Server fingerprint contract", async () => {
  const fingerprint = await previewRuntimePublicKeyFingerprint(
    "yoi-ed25519-pub:v1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
  );
  assert(
    fingerprint ===
      "sha256:66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925",
    "fingerprint preview drifted from the Server SHA-256 contract",
  );
});

Deno.test("Runtime registration sends only Runtime connection data without Workspace identity copies", async () => {
  const request = {
    public_bundle: { identity_id: "arcadia", public_key: "test-key" },
    endpoint: "https://runtime.example",
  };
  let body: unknown;
  await createRemoteRuntime(
    "workspace-a",
    request,
    ((_input: string | URL | Request, init?: RequestInit) => {
      body = JSON.parse(String(init?.body));
      return Promise.resolve(Response.json(runtime()));
    }) as typeof fetch,
  );
  assert(
    JSON.stringify(body) === JSON.stringify(request),
    "registration copied Workspace identity or approval fields",
  );
});

Deno.test("Runtime trust replacement recognizes RepositoryApiError conflicts without retrying", async () => {
  for (const errorKind of ["stale_binding", "fingerprint_in_use"] as const) {
    for (
      const diagnostics of [undefined, [], [{
        code: errorKind,
        severity: "error" as const,
        message: "Reload Workspace trust before retrying.",
      }]]
    ) {
      const request = {
        public_bundle: {
          identity_id: "runtime-a",
          public_key: "yoi-ed25519-pub:v1:test",
        },
        display_name: null,
        endpoint: "https://runtime.example",
        expected_binding_id: "observed-binding-id",
      };
      const conflict: RepositoryApiError = {
        error: errorKind,
        message: "Reload the authoritative Runtime trust.",
        ...(diagnostics === undefined ? {} : { diagnostics }),
      };
      let calls = 0;
      const fetchImpl = ((input: RequestInfo | URL, init?: RequestInit) => {
        calls++;
        assert(
          String(input) === "/api/w/workspace-a/runtimes",
          "wrong trust route",
        );
        assert(
          init?.body === JSON.stringify(request),
          "observed binding ID changed",
        );
        return Promise.resolve(Response.json(conflict, { status: 409 }));
      }) as typeof fetch;
      try {
        await createRemoteRuntime("workspace-a", request, fetchImpl);
        throw new Error("expected trust replacement to reject");
      } catch (error) {
        assert(
          error instanceof RuntimeTrustConflictError,
          `binding conflict was hidden: ${String(error)}`,
        );
        assert(
          error.message === conflict.message &&
            JSON.stringify(error.conflict) === JSON.stringify(conflict),
          "RepositoryApiError conflict was changed",
        );
      }
      assert(calls === 1, "trust replacement was retried");
    }
  }
});

Deno.test("Runtime revoke preserves typed conflict binding identities and diagnostics", async () => {
  const conflict: RuntimeManagementApiError = {
    error: "stale_binding",
    message: "Reload the authoritative Runtime trust.",
    current_binding_id: "current-binding-id",
    current_fingerprint: "sha256:current-key",
    diagnostics: [],
  };
  try {
    await revokeRuntimeTrustKey(
      "workspace-a",
      "runtime-a",
      { expected_binding_id: "observed-binding-id" },
      "sha256:observed-key",
      "sha256:observed-key",
      () => Promise.resolve(Response.json(conflict, { status: 409 })),
    );
    throw new Error("expected revoke to reject");
  } catch (error) {
    assert(
      error instanceof RuntimeTrustConflictError,
      "revoke conflict was hidden",
    );
    assert(
      JSON.stringify(error.conflict) === JSON.stringify(conflict),
      "typed revoke conflict identity was changed",
    );
  }
});

Deno.test("Runtime create surfaces other RepositoryApiError 409 messages as request errors", async () => {
  for (
    const diagnostics of [undefined, [], [{
      code: "runtime_binding_replacement_blocked",
      severity: "error" as const,
      message: "An active Worker occupies this Runtime.",
    }]]
  ) {
    const payload: RepositoryApiError = {
      error: "conflict",
      message: "Runtime binding replacement is blocked by an active Worker.",
      ...(diagnostics === undefined ? {} : { diagnostics }),
    };
    let calls = 0;
    try {
      await createRemoteRuntime("workspace-a", {
        public_bundle: { identity_id: "runtime-a", public_key: "test-key" },
        endpoint: "https://runtime.example",
      }, () => {
        calls++;
        return Promise.resolve(Response.json(payload, { status: 409 }));
      });
      throw new Error("expected occupancy conflict to reject");
    } catch (error) {
      assert(
        error instanceof RuntimeTrustRequestError,
        "occupancy error was misclassified",
      );
      assert(error.message === payload.message, "occupancy message was hidden");
    }
    assert(calls === 1, "occupancy conflict was retried");
  }
});

Deno.test("Runtime create bounds legitimate RepositoryApiError 409 messages", async () => {
  const message = "Runtime is occupied. ".repeat(30);
  try {
    await createRemoteRuntime("workspace-a", {
      public_bundle: { identity_id: "runtime-a", public_key: "test-key" },
      endpoint: "https://runtime.example",
    }, () =>
      Promise.resolve(Response.json(
        {
          error: "conflict",
          message,
        } satisfies RepositoryApiError,
        { status: 409 },
      )));
    throw new Error("expected occupancy conflict to reject");
  } catch (error) {
    assert(
      error instanceof RuntimeTrustRequestError,
      "occupancy error was misclassified",
    );
    assert(
      error.message === message.slice(0, 256),
      "bounded backend message was lost",
    );
  }
});

Deno.test("Runtime create rejects malformed RepositoryApiError conflicts without exposing their message", async () => {
  const validDiagnostic = {
    code: "conflict",
    severity: "error",
    message: "detail",
  };
  for (const errorKind of ["stale_binding", "fingerprint_in_use", "conflict"]) {
    const valid = {
      error: errorKind,
      message: "Do not expose malformed payload text",
    };
    for (
      const payload of [
        { ...valid, unexpected_field: true },
        { ...valid, current_binding_id: "not-in-creation-contract" },
        { ...valid, current_fingerprint: "not-in-creation-contract" },
        { ...valid, error: 409 },
        { ...valid, error: "" },
        { ...valid, error: "x".repeat(257) },
        { error: errorKind },
        { ...valid, message: null },
        { ...valid, message: "" },
        { ...valid, message: "é".repeat(513) },
        { ...valid, diagnostics: null },
        { ...valid, diagnostics: {} },
        { ...valid, diagnostics: Array(65).fill(validDiagnostic) },
        {
          ...valid,
          diagnostics: [{ ...validDiagnostic, severity: "unknown" }],
        },
        { ...valid, diagnostics: [{ ...validDiagnostic, code: 1 }] },
        { ...valid, diagnostics: [{ ...validDiagnostic, extra: true }] },
        {
          ...valid,
          diagnostics: [{ ...validDiagnostic, message: "x".repeat(2049) }],
        },
      ]
    ) {
      try {
        await createRemoteRuntime("workspace-a", {
          public_bundle: { identity_id: "runtime-a", public_key: "test-key" },
          endpoint: "https://runtime.example",
          expected_binding_id: "observed-binding-id",
        }, () => Promise.resolve(Response.json(payload, { status: 409 })));
        throw new Error("expected malformed conflict to reject");
      } catch (error) {
        assert(
          error instanceof RuntimeTrustRequestError &&
            error.message === "Runtime trust conflict response was invalid",
          `malformed conflict did not fail closed: ${JSON.stringify(payload)}`,
        );
      }
    }
  }
});

Deno.test("Runtime create surfaces bounded Settings error details", async () => {
  const fetchImpl = (() =>
    Promise.resolve(
      new Response(
        JSON.stringify({
          error: "remote_runtime_endpoint_not_allowed",
          details: "Runtime endpoint must use public https egress",
        }),
        { status: 400, headers: { "content-type": "application/json" } },
      ),
    )) as typeof fetch;

  try {
    await createRemoteRuntime(
      "workspace-a",
      {
        public_bundle: {
          identity_id: "runtime-a",
          public_key: "yoi-ed25519-pub:v1:test",
        },
        display_name: null,
        endpoint: "https://runtime.example",
        expected_binding_id: null,
      },
      fetchImpl,
    );
    throw new Error("expected create to reject");
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    assert(
      message === "Runtime endpoint must use public https egress",
      `unexpected create error: ${message}`,
    );
  }
});
