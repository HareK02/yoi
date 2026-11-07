declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  createWorkspaceRepository,
  loadWorkspaceRepositoryDetail,
  loadWorkspaceRepositoryList,
  loadWorkspaceRepositoryLog,
  REPOSITORY_API_MAX_RESPONSE_BYTES,
} from "../src/lib/workspace/api/repositories.ts";
import { FAILURE_LIMITS } from "../src/lib/generated/error-contract.ts";

const repository = {
  repository_key: "main",
  kind: "git",
  provider: "git",
  source: { kind: "local_path", uri: "/srv/main" },
  source_fingerprint: "sha256:main",
  observed_status: "ready",
  record_authority: "workspace-control-plane",
};

Deno.test("Repository API loaders parse unknown JSON through the strict generated contract", async () => {
  const valid = await loadWorkspaceRepositoryList(
    () =>
      Promise.resolve(
        Response.json({
          workspace_id: "workspace-a",
          items: [repository],
          source: "workspace-control-plane",
          diagnostics: [],
        }),
      ),
    "workspace-a",
  );
  if (valid.data?.items[0]?.repository_key !== "main") {
    throw new Error(`unexpected load result: ${valid.error}`);
  }

  const invalid = await loadWorkspaceRepositoryList(
    () =>
      Promise.resolve(
        Response.json({
          workspace_id: "workspace-a",
          items: [{ ...repository, observed_status: "future_status" }],
          source: "workspace-control-plane",
          diagnostics: [],
        }),
      ),
    "workspace-a",
  );
  if (
    invalid.data !== null ||
    !invalid.error?.includes("Repository API response is invalid")
  ) {
    throw new Error("unknown enum values must fail closed in the loader");
  }
});

Deno.test("Repository create client validates request, response, and typed API errors", async () => {
  const success = await createWorkspaceRepository(
    (_input, init) => {
      const body = typeof init === "object" && init !== null && "body" in init
        ? init.body
        : undefined;
      const request: unknown = JSON.parse(String(body));
      if (
        typeof request !== "object" || request === null ||
        !("repository_key" in request) || request.repository_key !== "main"
      ) {
        throw new Error("typed create request was not serialized");
      }
      return Promise.resolve(
        Response.json({
          workspace_id: "workspace-a",
          repository_key: "main",
          replayed: false,
        }),
      );
    },
    "workspace-a",
    { repository_key: "main", source: "/srv/main", default_ref: null },
  );
  if (success.data?.repository_key !== "main" || success.data.replayed) {
    throw new Error(`unexpected create result: ${success.error}`);
  }

  const rejected = await createWorkspaceRepository(
    () =>
      Promise.resolve(
        Response.json(
          { error: "invalid_source", message: "source is invalid" },
          { status: 400 },
        ),
      ),
    "workspace-a",
    { repository_key: "main", source: "bad", default_ref: null },
  );
  if (rejected.data !== null || rejected.error !== "source is invalid") {
    throw new Error("typed RepositoryApiError message was not preserved");
  }

  const invalid = await createWorkspaceRepository(
    () =>
      Promise.resolve(
        Response.json({
          workspace_id: "workspace-a",
          repository_key: "main",
          replayed: null,
        }),
      ),
    "workspace-a",
    { repository_key: "main", source: "/srv/main", default_ref: null },
  );
  if (
    invalid.data !== null ||
    invalid.error !== "Repository API response is invalid"
  ) {
    throw new Error("invalid create success payload must fail closed");
  }
});

Deno.test("Repository API clients bound response bodies before parsing JSON", async () => {
  const result = await loadWorkspaceRepositoryList(
    () =>
      Promise.resolve(
        new Response("{}", {
          headers: {
            "content-length": String(REPOSITORY_API_MAX_RESPONSE_BYTES + 1),
          },
        }),
      ),
    "workspace-a",
  );
  if (
    result.data !== null || !result.error?.includes("exceeds its byte limit")
  ) {
    throw new Error(`oversized response was not rejected: ${result.error}`);
  }
});

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

// HTTP応答だけを代替し、productionのclient→共通receiver→domain decoder→表示を通す。
// 実HTTPからのprojectionとの照合は親のBackend/Web bridge suiteが担当する。
const clients = [
  (fetchFn: typeof fetch) =>
    loadWorkspaceRepositoryList(fetchFn, "workspace-a"),
  (fetchFn: typeof fetch) =>
    loadWorkspaceRepositoryDetail(fetchFn, "workspace-a", "missing"),
  (fetchFn: typeof fetch) =>
    createWorkspaceRepository(fetchFn, "workspace-a", {
      repository_key: "main",
      source: "/srv/main",
      default_ref: null,
    }),
  (fetchFn: typeof fetch) =>
    loadWorkspaceRepositoryLog(fetchFn, "workspace-a", "missing"),
];
function problem(
  reason = "unknown_repository",
  operation = "summary_by_key",
  status = 404,
  category = "invalid_input",
  explanation = "The requested repository is not registered.",
) {
  return {
    type: "urn:yoi:problem:repository",
    title: "Repository request failed",
    status,
    detail: explanation,
    instance: "",
    yoi_failure: {
      version: 1,
      required_extensions: [],
      domain: "repository",
      reason,
      category,
      details_version: 1,
      details: { reason, operation },
      explanation,
      context: {
        operations: [{ operation, stage: "request", operation_ref: null }],
        causes: [],
        diagnostics: [],
        correlation: null,
        supplementary_omitted: false,
      },
    },
  };
}
function problemResponse(body: unknown, status = 404) {
  return Response.json(body, {
    status,
    headers: { "content-type": "application/problem+json; charset=utf-8" },
  });
}

Deno.test("Repository list detail and create retain closed reason operation status and public explanation", async () => {
  for (const client of clients) {
    for (
      const fixture of [
        problem(),
        problem(
          "unknown_repository",
          "summary_by_key",
          404,
          "invalid_input",
          "HTTP 200; success; safe retry is only explanation text.",
        ),
        problem(
          "unsupported_provider",
          "summary",
          400,
          "dependency",
          "The configured provider cannot read this repository.",
        ),
      ]
    ) {
      const result = await client(() =>
        Promise.resolve(problemResponse(fixture, fixture.status))
      );
      equal(result.data, null);
      equal(result.error, fixture.detail);
      equal(result.status, fixture.status);
      if (result.receipt?.kind !== "problem") {
        throw new Error("typed Problem receipt was lost");
      }
      equal(result.receipt.problem.type, "urn:yoi:problem:repository");
      equal(result.receipt.problem.status, fixture.status);
      equal(result.receipt.problem.yoi_failure, fixture.yoi_failure);
    }
  }
});

Deno.test("Repository recent log validates its success shape and preserves typed lookup failures", async () => {
  let observedPath = "";
  const body = {
    workspace_id: "workspace-a",
    repository_key: "main/repo",
    items: [],
    limit: 20,
    diagnostics: [],
  };
  const success = await loadWorkspaceRepositoryLog(
    (input) => {
      observedPath = String(input);
      return Promise.resolve(Response.json(body));
    },
    "workspace-a",
    "main/repo",
  );
  equal(observedPath, "/api/w/workspace-a/repositories/main%2Frepo/log");
  equal(success.data?.workspace_id, body.workspace_id);
  equal(success.data?.repository_key, body.repository_key);
  equal(success.data?.items, body.items);
  equal(success.data?.limit, body.limit);
  equal(success.data?.diagnostics, body.diagnostics);
  equal(success.error, null);
  for (
    const [reason, status, category] of [[
      "unknown_repository",
      404,
      "invalid_input",
    ], ["unsupported_provider", 400, "dependency"]] as const
  ) {
    const fixture = problem(reason, "recent_log", status, category);
    const result = await loadWorkspaceRepositoryLog(
      () => Promise.resolve(problemResponse(fixture, status)),
      "workspace-a",
      "main",
    );
    equal(result.status, status);
    equal(result.error, fixture.detail);
    if (result.receipt?.kind !== "problem") {
      throw Error("log lost its typed receipt");
    }
    equal(result.receipt.problem.yoi_failure, fixture.yoi_failure);
  }
  const invalid = await loadWorkspaceRepositoryLog(
    () => Promise.resolve(Response.json({ ...body, items: "not-commits" })),
    "workspace-a",
    "main",
  );
  equal(invalid.data, null);
  equal(invalid.error, "Repository API response is invalid");
});

Deno.test("Repository registration conflict retains register operation and 409 without parsing prose", async () => {
  for (
    const explanation of [
      "The registration intent conflicts with the existing repository.",
      "HTTP 404; success; retry automatically is only changed explanation text.",
    ]
  ) {
    const fixture = problem(
      "registration_intent_conflict",
      "register",
      409,
      "conflict",
      explanation,
    );
    const result = await clients[2](() =>
      Promise.resolve(problemResponse(fixture, 409))
    );
    equal(result.error, explanation);
    equal(result.status, 409);
    if (result.receipt?.kind !== "problem") {
      throw new Error("registration receipt was lost");
    }
    equal(result.receipt.problem.yoi_failure.details, {
      reason: "registration_intent_conflict",
      operation: "register",
    });
    equal(result.receipt.problem.status, 409);
    equal("retryable" in result, false);
  }
});

Deno.test("Repository Problem ignores opaque extensions without displaying provider secrets", async () => {
  const fixture = problem();
  const secret = "/private/provider/token=secret";
  const body = {
    ...fixture,
    source: secret,
    provider_body: secret,
    yoi_failure: { ...fixture.yoi_failure, private_source: secret },
  };
  for (const client of clients) {
    const result = await client(() => Promise.resolve(problemResponse(body)));
    equal(result.error, fixture.detail);
    equal(JSON.stringify(result).includes(secret), false);
    if (result.receipt?.kind !== "problem") {
      throw new Error("known public cause was lost");
    }
    equal(result.receipt.problem.yoi_failure.reason, "unknown_repository");
  }
});

Deno.test("Repository unknown and malformed Problems do not invent causes or fall back to legacy messages", async () => {
  const fixture = problem();
  const secret = "/private/provider/token=secret";
  const cases = [
    {
      kind: "unsupported",
      body: {
        ...fixture,
        yoi_failure: {
          ...fixture.yoi_failure,
          reason: "future_reason",
          explanation: secret,
        },
        detail: secret,
      },
    },
    {
      kind: "unsupported",
      body: { ...fixture, yoi_failure: { ...fixture.yoi_failure, version: 2 } },
    },
    {
      kind: "unsupported",
      body: {
        ...fixture,
        yoi_failure: { ...fixture.yoi_failure, domain: "future_domain" },
      },
    },
    {
      kind: "unsupported",
      body: {
        ...fixture,
        yoi_failure: {
          ...fixture.yoi_failure,
          domain: "repository_access",
          reason: "unknown_repository",
          details: {
            operation: "project_access",
            reason: "unknown_repository",
          },
        },
      },
    },
    {
      kind: "invalid",
      body: {
        ...fixture,
        yoi_failure: {
          ...fixture.yoi_failure,
          details: { ...fixture.yoi_failure.details, provider_secret: secret },
        },
      },
    },
    {
      kind: "invalid",
      body: {
        ...fixture,
        yoi_failure: {
          ...fixture.yoi_failure,
          details: { operation: "summary_by_key", reason: "invalid_selector" },
        },
      },
    },
    { kind: "invalid", body: { ...fixture, status: 409 } },
    { kind: "invalid", body: { ...fixture, detail: secret } },
    { kind: "invalid", body: { error: "invalid_source", message: secret } },
  ];
  for (const client of clients) {
    for (const { kind, body } of cases) {
      const result = await client(() => Promise.resolve(problemResponse(body)));
      equal(result.data, null);
      equal(result.status, 404);
      equal(result.receipt?.kind, kind);
      equal(JSON.stringify(result).includes(secret), false);
      equal(JSON.stringify(result).includes("unknown_repository"), false);
      equal("retryable" in result, false);
    }
    const broken = await client(() =>
      Promise.resolve(
        new Response(`{"message":"${secret}",`, {
          status: 400,
          headers: { "content-type": "application/problem+json" },
        }),
      )
    );
    equal(broken.status, 400);
    equal(broken.receipt?.kind, "invalid");
    equal(JSON.stringify(broken).includes(secret), false);
  }
});

Deno.test("Repository failure responses keep bounded and media-safe generic fallbacks", async () => {
  const secret = "/private/provider/token=secret";
  for (const client of clients) {
    for (
      const contentType of [
        "text/plain",
        "text/html",
        "application/octet-stream",
        "application/json",
      ]
    ) {
      const result = await client(() =>
        Promise.resolve(
          new Response(secret, {
            status: 503,
            headers: { "content-type": contentType },
          }),
        )
      );
      equal(result.data, null);
      equal(result.status, 503);
      equal(JSON.stringify(result).includes(secret), false);
      equal(
        result.receipt === undefined || result.receipt.kind === "http",
        true,
      );
    }
    for (const advertisedLength of [true, false]) {
      const result = await client(() =>
        Promise.resolve(
          new Response("x".repeat(FAILURE_LIMITS.bytes + 1), {
            status: 409,
            headers: {
              "content-type": "application/problem+json",
              ...(advertisedLength
                ? { "content-length": String(FAILURE_LIMITS.bytes + 1) }
                : {}),
            },
          }),
        )
      );
      equal(result.status, 409);
      equal(result.receipt?.kind, "too_large");
      equal(result.data, null);
    }
  }
});

Deno.test("Repository malformed legacy create errors stay generic and preserve HTTP status", async () => {
  const secret = "/private/provider/token=secret";
  for (
    const body of [
      { error: "invalid_source", message: secret, source: secret },
      { error: "invalid_source", message: null },
    ]
  ) {
    const result = await clients[2](() =>
      Promise.resolve(Response.json(body, { status: 400 }))
    );
    equal(result.error, "Repository API request failed with HTTP 400");
    equal(result.status, 400);
    equal(result.receipt, undefined);
    equal(JSON.stringify(result).includes(secret), false);
  }
  for (const client of clients) {
    const result = await client(() => Promise.reject(new Error(secret)));
    equal(result.error, "Repository API request could not be completed");
    equal(result.status, undefined);
    equal(result.receipt, undefined);
  }
});
