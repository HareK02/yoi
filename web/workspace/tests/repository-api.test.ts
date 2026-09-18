declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  createWorkspaceRepository,
  loadWorkspaceRepositoryList,
  REPOSITORY_API_MAX_RESPONSE_BYTES,
} from "../src/lib/workspace/api/repositories.ts";

const repository = {
  repository_key: "main",
  kind: "git",
  provider: "git",
  source: { kind: "local_path", uri: "/srv/main" },
  source_revision: 1,
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
