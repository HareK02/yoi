declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  parseRuntimeTrustConflict,
  parseRuntimeTrustKeyRevealResponse,
  parseWorkspaceRuntimeDetail,
  parseWorkspaceRuntimeList,
  previewRuntimePublicKeyFingerprint,
  putRuntimeTrustKey,
  revokeRuntimeTrustKey,
  RuntimeTrustConflictError,
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
      revision: 3,
      created_at: "2026-09-01T12:00:00Z",
      updated_at: "2026-09-01T13:00:00Z",
      revoked_at: null,
    },
    recent_audit: [{
      action: "created",
      actor_account_id: "account-a",
      old_fingerprint: null,
      new_fingerprint: "SHA256:current",
      revision: 3,
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

  const parsed = parseWorkspaceRuntimeDetail(detail());
  assert(
    parsed.trust_key.revision === 3,
    "revision was not preserved as a safe integer",
  );
  assert(
    parsed.recent_audit[0]?.revision === 3,
    "audit revision was not normalized",
  );
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
        current_revision: 4,
        current_fingerprint: "SHA256:new",
      }),
    "contains an unknown enum value",
  );
});

Deno.test("Runtime validators reject unsafe revisions and bounded collection overflow", () => {
  const unsafeRevision = structuredClone(detail());
  unsafeRevision.trust_key.revision = Number.MAX_SAFE_INTEGER + 1;
  assertThrows(
    () => parseWorkspaceRuntimeDetail(unsafeRevision),
    "must be a safe integer",
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
      { expected_revision: 3 },
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

Deno.test("typed trust conflict is validated and preserves authoritative revision", async () => {
  let sentBody: unknown = null;
  const fetchImpl = ((_: RequestInfo | URL, init?: RequestInit) => {
    sentBody = JSON.parse(String(init?.body)) as unknown;
    return Promise.resolve(
      new Response(
        JSON.stringify({
          error: "stale_revision",
          message: "Runtime trust changed",
          current_revision: 4,
          current_fingerprint: "SHA256:new",
        }),
        { status: 409, headers: { "content-type": "application/json" } },
      ),
    );
  }) as typeof fetch;

  try {
    await putRuntimeTrustKey(
      "workspace-a",
      "arcadia",
      { public_key: "ssh-ed25519 AAAA-new", expected_revision: 3 },
      fetchImpl,
    );
    throw new Error("expected mutation to reject");
  } catch (error) {
    assert(
      error instanceof RuntimeTrustConflictError,
      "expected typed conflict",
    );
    assert(
      error.conflict.current_revision === 4,
      "authoritative revision was lost",
    );
  }

  assert(
    JSON.stringify(sentBody) ===
      JSON.stringify({
        public_key: "ssh-ed25519 AAAA-new",
        expected_revision: 3,
      }),
    "request should serialize the generated bigint revision as a safe JSON integer",
  );
});
