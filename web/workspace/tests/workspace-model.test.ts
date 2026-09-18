declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFile(path: URL): Promise<string>;
};

import {
  parseCreateWorkspaceRepositoryResponse,
  parseRepositoryApiError,
  parseRepositoryListApiResult,
  parseRepositoryListResponse,
  parseWorkspaceDeletionOperationResponse,
  parseWorkspaceDeletionPreflightResponse,
  parseWorkspaceResponse,
  REPOSITORY_API_LIMITS,
} from "../src/lib/workspace/api/workspace-model.ts";

function assertThrows(operation: () => unknown, expected: string): void {
  try {
    operation();
  } catch (error) {
    if (error instanceof Error && error.message.includes(expected)) return;
    throw error;
  }
  throw new Error("expected operation to throw");
}

const repositoryList = {
  workspace_id: "w-a",
  items: [{
    repository_key: "main",
    kind: "git",
    provider: "git",
    source: { kind: "local_path", uri: "/srv/alpha" },
    source_revision: 1,
    source_fingerprint: "sha256:alpha",
    observed_status: "ready",
    record_authority: "workspace-control-plane",
  }],
  source: "workspace-control-plane",
  diagnostics: [],
};

Deno.test("generated repository wrapper validates current Backend JSON", () => {
  const parsed = parseRepositoryListResponse(repositoryList);
  if (parsed.items[0]?.repository_key !== "main") {
    throw new Error("repository key was not preserved");
  }
  if (parsed.items[0]?.source.kind !== "local_path") {
    throw new Error("repository source kind was not preserved");
  }
});

Deno.test("repository git optional nullable fields accept omission and null", () => {
  const omitted = structuredClone(repositoryList) as Record<string, unknown>;
  const omittedItems = omitted.items as Array<Record<string, unknown>>;
  omittedItems[0].git = {
    status: "available",
    dirty: false,
    remotes: [],
  };
  const omittedParsed = parseRepositoryListResponse(omitted);
  if (
    omittedParsed.items[0]?.git?.head !== undefined ||
    omittedParsed.items[0]?.git?.branch !== undefined
  ) {
    throw new Error("omitted git fields were not preserved");
  }

  const nullable = structuredClone(repositoryList) as Record<string, unknown>;
  const nullableItems = nullable.items as Array<Record<string, unknown>>;
  nullableItems[0].git = {
    status: "available",
    head: null,
    branch: null,
    dirty: false,
    remotes: [],
  };
  const nullableParsed = parseRepositoryListResponse(nullable);
  if (
    nullableParsed.items[0]?.git?.head !== null ||
    nullableParsed.items[0]?.git?.branch !== null
  ) {
    throw new Error("null git fields were not preserved");
  }
});

Deno.test("plain HTTP repository source kind fails closed at the JSON boundary", () => {
  const stale = structuredClone(repositoryList) as Record<string, unknown>;
  const items = stale.items as Array<Record<string, unknown>>;
  items[0].source = {
    kind: "http",
    uri: "http://git.example.test/team/project.git",
  };
  assertThrows(
    () => parseRepositoryListResponse(stale),
    ".source.kind is invalid",
  );
});

Deno.test("stale repository aliases fail closed at the JSON boundary", () => {
  const stale = structuredClone(repositoryList) as Record<string, unknown>;
  const items = stale.items as Array<Record<string, unknown>>;
  items[0].id = items[0].repository_key;
  delete items[0].repository_key;
  assertThrows(
    () => parseRepositoryListResponse(stale),
    ".id is not part",
  );
});

Deno.test("repository source revisions enforce the OpenAPI integer range", () => {
  const negative = structuredClone(repositoryList) as Record<string, unknown>;
  const negativeItems = negative.items as Array<Record<string, unknown>>;
  negativeItems[0].source_revision = -1;
  assertThrows(
    () => parseRepositoryListResponse(negative),
    "must be between 0 and Number.MAX_SAFE_INTEGER",
  );

  const unsafe = structuredClone(repositoryList) as Record<string, unknown>;
  const unsafeItems = unsafe.items as Array<Record<string, unknown>>;
  unsafeItems[0].source_revision = Number.MAX_SAFE_INTEGER + 1;
  assertThrows(
    () => parseRepositoryListResponse(unsafe),
    "must be a safe integer",
  );
});

Deno.test("repository create/error responses use the generated strict contract", () => {
  const created = parseCreateWorkspaceRepositoryResponse({
    workspace_id: repositoryList.workspace_id,
    repository_key: "main",
    replayed: false,
  });
  if (created.replayed || created.repository_key !== "main") {
    throw new Error("create response fields were not preserved");
  }
  const error = parseRepositoryApiError({
    error: "invalid_request",
    message: "source is invalid",
  });
  if (error.error !== "invalid_request") {
    throw new Error("error response fields were not preserved");
  }
  assertThrows(
    () =>
      parseCreateWorkspaceRepositoryResponse({
        workspace_id: repositoryList.workspace_id,
        repository_key: "main",
        replayed: null,
      }),
    ".replayed must be a boolean",
  );
  assertThrows(
    () =>
      parseRepositoryApiError({
        error: "invalid_request",
        message: "source is invalid",
        context: {},
      }),
    ".context is not part",
  );
});

Deno.test("repository responses retain bounded strings and collections", () => {
  const oversizedString = structuredClone(repositoryList);
  oversizedString.items[0].provider = "x".repeat(
    REPOSITORY_API_LIMITS.maxStringCodeUnits + 1,
  );
  assertThrows(
    () => parseRepositoryListResponse(oversizedString),
    "string limit",
  );

  const oversizedCollection = structuredClone(repositoryList);
  oversizedCollection.items = Array.from(
    { length: REPOSITORY_API_LIMITS.maxCollectionEntries + 1 },
    () => repositoryList.items[0],
  );
  assertThrows(
    () => parseRepositoryListResponse(oversizedCollection),
    "collection limit",
  );
});

Deno.test("repository API result converts stale payloads into bounded page errors", () => {
  const result = parseRepositoryListApiResult({
    data: {
      workspace_id: "w-a",
      items: { main: repositoryList.items[0] },
      source: "workspace-control-plane",
      diagnostics: [],
    },
    error: null,
  });
  if (result.data !== null) {
    throw new Error("stale payload must not reach the page");
  }
  if (!result.error?.includes("items must be an array")) {
    throw new Error(`unexpected bounded error: ${result.error}`);
  }
});

Deno.test("workspace response requires the permission projection", () => {
  const stale = {
    workspace_id: "w-a",
    display_name: "Alpha",
    record_authority: "workspace-control-plane",
    schema_version: 46,
    auth: {
      Passkey: {
        rp_id: "example.test",
        origin: "https://example.test",
        public_base_url: "https://example.test",
        cookie_name: "yoi_session",
      },
    },
    extension_points: {
      store: "sqlite",
      event_stream: { status: "available", note: "ready", diagnostics: [] },
      host_worker_bridge: {
        status: "available",
        note: "ready",
        diagnostics: [],
      },
      companion_console: {
        status: "available",
        note: "ready",
        diagnostics: [],
      },
    },
  };
  assertThrows(
    () => parseWorkspaceResponse(stale),
    "permissions must be an object",
  );
});

Deno.test("Workspace deletion DTOs fail closed and preserve durable operation state", () => {
  const preflight = parseWorkspaceDeletionPreflightResponse({
    workspace_id: "workspace-a",
    display_name: "Alpha",
    expected_revision: "2026-01-01T00:00:00Z",
    can_delete: true,
    resources: {
      workers: 2,
      workdirs: 1,
      repositories: 1,
      runtime_bindings: 1,
      secrets: 0,
      artifacts: 3,
    },
    blockers: [],
  });
  if (preflight.resources.workers !== 2) {
    throw new Error("worker count was not preserved");
  }

  const operation = parseWorkspaceDeletionOperationResponse({
    operation_id: "delete-alpha",
    workspace_id: "workspace-a",
    display_name: "Alpha",
    state: "blocked",
    resources: preflight.resources,
    child_operation_ids: ["worker-remove:arcadia/7"],
    blockers: [{
      kind: "dirty_workdir",
      resource_kind: "workdir",
      resource_key: "WD-1",
      message: "Workdir is dirty",
    }],
    failure_category: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:01:00Z",
    completed_at: null,
  });
  if (operation.state !== "blocked") {
    throw new Error("operation state was not preserved");
  }

  assertThrows(
    () =>
      parseWorkspaceDeletionPreflightResponse({
        ...preflight,
        unexpected: true,
      }),
    "unexpected is not part",
  );
  assertThrows(
    () =>
      parseWorkspaceDeletionOperationResponse({
        ...operation,
        state: "unknown",
      }),
    ".state is invalid",
  );
  assertThrows(
    () =>
      parseWorkspaceDeletionOperationResponse({
        ...operation,
        operation_id: "x".repeat(129),
      }),
    ".operation_id is too long",
  );
  assertThrows(
    () =>
      parseWorkspaceDeletionOperationResponse({
        ...operation,
        blockers: Array.from({ length: 1025 }, () => operation.blockers[0]),
      }),
    ".blockers has too many items",
  );
});

Deno.test("Workspace settings exposes owner-gated typed destructive confirmation", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/settings/+page.svelte",
      import.meta.url,
    ),
  );
  for (
    const token of [
      "permissions.delete_workspace",
      "preflightWorkspaceDeletion",
      "startWorkspaceDeletion",
      "deletionConfirmation",
      "disposeWorkspaceMultiplexer(workspaceId)",
      "disposeWorkspaceWorkersStore(workspaceId)",
      "sessionStorage.setItem(deletionStorageKey",
      "storedDeletionRequest()",
      "trackDeletion(request.operation_id)",
    ]
  ) {
    if (!source.includes(token)) {
      throw new Error(`Workspace deletion UI should include ${token}`);
    }
  }
});

Deno.test("Repository settings consume the validated shared wire shape", async () => {
  const [loadSource, pageSource, apiSource, modelSource] = await Promise.all([
    Deno.readTextFile(
      new URL(
        "../src/routes/w/[workspaceId]/settings/repositories/+page.ts",
        import.meta.url,
      ),
    ),
    Deno.readTextFile(
      new URL(
        "../src/routes/w/[workspaceId]/settings/repositories/+page.svelte",
        import.meta.url,
      ),
    ),
    Deno.readTextFile(
      new URL("../src/lib/workspace/api/repositories.ts", import.meta.url),
    ),
    Deno.readTextFile(
      new URL("../src/lib/workspace/api/workspace-model.ts", import.meta.url),
    ),
  ]);
  const targetSources = `${loadSource}\n${pageSource}\n${apiSource}`;

  for (
    const token of [
      "loadWorkspaceRepositoryList",
      "createWorkspaceRepository",
      "parseRepositoryListResponse",
      "parseCreateWorkspaceRepositoryResponse",
      "parseRepositoryApiError",
      "repository.repository_key",
      "repository.observed_status",
      "sourceLabel(repository.source.kind)",
      "supportsRepositoryAccess(repository.source.kind)",
    ]
  ) {
    if (!targetSources.includes(token) && !modelSource.includes(token)) {
      throw new Error(`Repository settings should include ${token}`);
    }
  }
  if (
    pageSource.includes("generated/legacy-server-api") ||
    pageSource.includes("response.json()") ||
    pageSource.includes(" as CreateWorkspaceRepository")
  ) {
    throw new Error(
      "Repository create UI must use the generated contract and strict API client",
    );
  }
  for (const kind of ["ssh", "https"]) {
    if (!pageSource.includes(`kind === '${kind}'`)) {
      throw new Error(`Repository Access should support ${kind}`);
    }
  }
  if (pageSource.includes("kind === 'http'")) {
    throw new Error("Repository Access must not support plain HTTP sources");
  }

  for (
    const staleToken of [
      "repository.id",
      "repository.display_name",
      "repository.source.kind === 'remote_git'",
    ]
  ) {
    if (loadSource.includes(staleToken) || pageSource.includes(staleToken)) {
      throw new Error(`Repository settings must not use ${staleToken}`);
    }
  }
});
